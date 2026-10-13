"""ModelRunner against mocked provider APIs. Nothing here touches the network or spends money."""

from pathlib import Path

import anthropic
import httpx2
import openai
import pytest
from model_mocks import FakeAPI, anthropic_message, api_error, chat_completion

from kritrim_mulyankan.engine import PreflightError, preflight, run_suite
from kritrim_mulyankan.pricing import Price, PriceTable, bundled_prices, max_input_tokens
from kritrim_mulyankan.runners import make_runner
from kritrim_mulyankan.runners.base import RunContext
from kritrim_mulyankan.runners.model import ModelOptions, ModelRunner
from kritrim_mulyankan.runners.providers import call_with_retries, credentials_problem

CTX = RunContext(workdir=Path("."), seed=1234, timeout_s=30)
PRICES = bundled_prices().merged(
    PriceTable(
        {
            "openai/gpt-test": Price(input=1.0, output=4.0),
            "xai/grok-test": Price(input=2.0, output=8.0),
            "gemini/gemini-test": Price(input=0.5, output=2.0),
        }
    )
)


def runner(provider, model, api, *, options=None, api_key="test-key", env=None, sleeps=None):
    return ModelRunner(
        provider,
        model,
        ModelOptions(**(options or {})),
        prices=PRICES,
        api_key=api_key,
        env=env or {},
        http_client=api.client(),
        sleep=(sleeps.append if sleeps is not None else lambda _: None),
    )


@pytest.fixture
def task(example_suite):
    return next(t for t in example_suite.tasks if t.id == "coding.sum-of-squares")


# --- Anthropic ----------------------------------------------------------------------------------


def test_anthropic_request_and_response(task):
    api = FakeAPI(anthropic_message("14", input_tokens=100, output_tokens=50))
    r = runner("anthropic", "claude-opus-5-5", api, options={"effort": "high"})
    out = r.run(task, task.render_prompt(), CTX)

    req = api.requests[0]
    assert str(req.url) == "https://api.anthropic.com/v1/messages"
    assert req.headers["x-api-key"] == "test-key"
    body = api.body()
    assert body["model"] == "claude-opus-5-5"
    assert body["max_tokens"] == task.max_output_tokens
    assert body["output_config"] == {"effort": "high"}
    assert body["messages"] == [{"role": "user", "content": task.render_prompt()}]
    assert "temperature" not in body and "fallbacks" not in body

    assert out.output == "14"  # thinking blocks are skipped
    assert out.usage.input_tokens == 100 and out.usage.output_tokens == 50
    assert out.usage.cost_usd == pytest.approx((100 * 4 + 50 * 20) / 1e6)
    assert out.raw["stop_reason"] == "end_turn" and out.raw["retries"] == 0
    assert out.raw["response"]["id"] == "msg_test"


def test_anthropic_cache_tokens_are_priced_and_counted(task):
    api = FakeAPI(anthropic_message(input_tokens=10, output_tokens=5, cache_read=1000))
    out = runner("anthropic", "claude-opus-5-5", api).run(task, "q", CTX)
    assert out.usage.input_tokens == 1010
    assert out.usage.cost_usd == pytest.approx((10 * 4 + 5 * 20 + 1000 * 0.2) / 1e6)


def test_anthropic_omits_effort_by_default_and_rejects_temperature(task):
    api = FakeAPI(anthropic_message())
    runner("anthropic", "claude-sonnet-5-5", api).run(task, "q", CTX)
    assert "output_config" not in api.body()

    hot = runner("anthropic", "claude-sonnet-5-5", api, options={"temperature": 0.5})
    with pytest.raises(ValueError, match="temperature"):
        hot.run(task, "q", CTX)
    assert len(api.requests) == 1  # rejected before sending


# --- OpenAI-compatible --------------------------------------------------------------------------


def test_openai_request_and_response(task):
    api = FakeAPI(chat_completion("14", prompt_tokens=120, completion_tokens=30, cached=20))
    r = runner("openai", "gpt-test", api, options={"effort": "low", "max_tokens": 777})
    out = r.run(task, "q", CTX)

    req = api.requests[0]
    assert str(req.url) == "https://api.openai.com/v1/chat/completions"
    assert req.headers["authorization"] == "Bearer test-key"
    body = api.body()
    assert body["max_completion_tokens"] == 777 and "max_tokens" not in body
    assert body["seed"] == CTX.seed
    assert body["reasoning_effort"] == "low"

    assert out.output == "14"
    assert out.usage.input_tokens == 120 and out.usage.output_tokens == 30
    # Cached prompt tokens are priced at the input rate unless the table says otherwise.
    assert out.usage.cost_usd == pytest.approx((120 * 1 + 30 * 4) / 1e6)
    assert out.raw["stop_reason"] == "stop"


@pytest.mark.parametrize(
    ("provider", "model", "url"),
    [
        ("xai", "grok-test", "https://api.x.ai/v1/chat/completions"),
        (
            "gemini",
            "gemini-test",
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
        ),
    ],
)
def test_other_openai_compatible_providers(task, provider, model, url):
    api = FakeAPI(chat_completion())
    runner(provider, model, api, options={"temperature": 0.2}).run(task, "q", CTX)
    assert str(api.requests[0].url) == url
    body = api.body()
    assert body["max_tokens"] == task.max_output_tokens
    assert "seed" not in body and body["temperature"] == 0.2


def test_ollama_is_local_free_and_keyless(task):
    api = FakeAPI(chat_completion(prompt_tokens=500, completion_tokens=500))
    r = runner(
        "ollama", "llama3.2:3b", api, api_key=None,
        env={"OLLAMA_BASE_URL": "http://gpu-box:11434/v1"},
    )  # fmt: skip
    assert r.problems() == []
    out = r.run(task, "q", CTX)
    assert str(api.requests[0].url) == "http://gpu-box:11434/v1/chat/completions"
    assert out.usage.cost_usd == 0 and r.estimate_cost_usd(task, "q") == 0


# --- retries ------------------------------------------------------------------------------------


def test_429_and_5xx_are_retried_with_retry_after(task):
    sleeps = []
    api = FakeAPI(api_error(429, retry_after="3"), api_error(529), anthropic_message())
    out = runner("anthropic", "claude-opus-5-5", api, options={"retries": 2}, sleeps=sleeps).run(
        task, "q", CTX
    )
    assert out.output == "14" and out.raw["retries"] == 2
    assert sleeps == [3.0, 2.0]  # retry-after honoured, then exponential backoff
    assert len(api.requests) == 3


def test_retries_give_up_after_two(task):
    api = FakeAPI(api_error(500))
    with pytest.raises(openai.InternalServerError):
        runner("openai", "gpt-test", api, options={"retries": 2}).run(task, "q", CTX)
    assert len(api.requests) == 3


def test_client_errors_are_not_retried(task):
    api = FakeAPI(api_error(400))
    with pytest.raises(anthropic.BadRequestError):
        runner("anthropic", "claude-opus-5-5", api).run(task, "q", CTX)
    assert len(api.requests) == 1


def test_timeouts_are_never_retried_because_they_may_be_billed(task):
    def hang(request):
        raise httpx2.ReadTimeout("timed out", request=request)

    api = FakeAPI(hang)
    with pytest.raises(anthropic.APITimeoutError):
        runner("anthropic", "claude-opus-5-5", api).run(task, "q", CTX)
    assert len(api.requests) == 1


def test_call_with_retries_passes_through_non_api_errors():
    calls = []

    def boom():
        calls.append(1)
        raise KeyError("x")

    with pytest.raises(KeyError):
        call_with_retries(boom, sleep=lambda _: None)
    assert calls == [1]


# --- estimates, readiness, specs ----------------------------------------------------------------


def test_estimate_is_a_worst_case(task):
    api = FakeAPI(anthropic_message())
    r = runner("anthropic", "claude-opus-5-5", api)
    prompt = task.render_prompt()
    expected = (max_input_tokens(prompt) * 4 + task.max_output_tokens * 20) / 1e6
    assert r.estimate_cost_usd(task, prompt) == pytest.approx(expected)
    small = runner("anthropic", "claude-opus-5-5", api, options={"max_tokens": 10})
    assert small.estimate_cost_usd(task, prompt) < expected
    assert api.requests == []  # estimating never calls the API


def test_unpriced_model_is_refused_without_calling(task):
    api = FakeAPI(chat_completion())
    r = runner("openai", "gpt-unpriced", api)
    assert r.estimate_cost_usd(task, "q") is None
    assert any("no price" in p for p in r.problems())
    with pytest.raises(RuntimeError, match="no price"):
        r.run(task, "q", CTX)
    assert api.requests == []


def test_credentials_problems(tmp_path):
    empty = {"XDG_CONFIG_HOME": str(tmp_path)}
    assert credentials_problem("openai", empty) == "OPENAI_API_KEY is not set"
    assert credentials_problem("openai", {**empty, "OPENAI_API_KEY": "k"}) is None
    assert "ANTHROPIC_API_KEY" in credentials_problem("anthropic", empty)
    assert credentials_problem("anthropic", {**empty, "ANTHROPIC_AUTH_TOKEN": "t"}) is None
    (tmp_path / "anthropic").mkdir()  # an `ant auth login` profile
    assert credentials_problem("anthropic", empty) is None
    assert credentials_problem("ollama", {}) is None


def test_missing_key_is_a_problem_before_any_call(tmp_path):
    r = ModelRunner("xai", "grok-test", prices=PRICES, env={"XDG_CONFIG_HOME": str(tmp_path)})
    assert r.problems() == ["XAI_API_KEY is not set"]


def test_spec_parsing_and_canonical_names():
    r = make_runner("model:anthropic/claude-opus-5-5,max_tokens=900,effort=high", prices=PRICES)
    assert r.name == "model:anthropic/claude-opus-5-5,effort=high,max_tokens=900"
    assert r.options.max_tokens == 900 and r.options.effort == "high"
    local = make_runner("model:ollama/llama3.2:3b", prices=PRICES)
    assert (local.provider, local.model) == ("ollama", "llama3.2:3b")


@pytest.mark.parametrize(
    "spec",
    [
        "model:nope/x",
        "model:anthropic/x,effort=extreme",
        "model:anthropic/x,bogus=1",
        "model:anthropic/x,effort",
        "model:anthropic/x,max_tokens=0",
    ],
)
def test_bad_model_specs(spec):
    with pytest.raises(ValueError):
        make_runner(spec, prices=PRICES)


# --- through the engine -------------------------------------------------------------------------


def test_engine_records_real_usage_and_cost(example_suite, store, settings):
    answers = {
        "bat": "5", "sum(x": "14", "tool": "create_event", "electricity": "2026-11-05",
    }  # fmt: skip

    def reply(request):
        prompt = request.content.decode()
        text = next((a for k, a in answers.items() if k in prompt), "Sorry. Next one's free.")
        return anthropic_message(text, input_tokens=200, output_tokens=40)

    api = FakeAPI(reply)
    r = runner("anthropic", "claude-opus-5-5", api)
    summary = run_suite(example_suite, [r], store=store, settings=settings)

    per_attempt = (200 * 4 + 40 * 20) / 1e6
    results = store.results_for_run(summary.run_id)
    assert len(api.requests) == 5
    assert sum(bool(x.passed) for x in results) == 4
    assert all(x.cost_usd == pytest.approx(per_attempt) for x in results)
    assert results[0].cost_inr == pytest.approx(per_attempt * settings.usd_to_inr)
    assert summary.spent_usd == pytest.approx(5 * per_attempt)
    assert store.get_run(summary.run_id).spent_usd == pytest.approx(5 * per_attempt)


def test_engine_refuses_before_calling_when_worst_case_exceeds_cap(example_suite, store, settings):
    api = FakeAPI(anthropic_message())
    r = runner("anthropic", "claude-fable-5-1", api)
    check = preflight(example_suite.tasks, [r], cap=0.10)
    assert not check.ok and "worst-case" in check.problems[0]
    with pytest.raises(PreflightError, match="worst-case"):
        run_suite(example_suite, [r], store=store, settings=settings, max_usd=0.10)
    assert api.requests == []
    assert not [p for p in settings.runs_dir.iterdir() if p.is_dir()]  # no run folder created


def test_network_guard_blocks_unmocked_calls(task):
    # Without a mock transport the real HTTP transport is used, and conftest refuses it.
    real = ModelRunner("anthropic", "claude-haiku-5-5", prices=PRICES, api_key="sk-not-real")
    with pytest.raises(anthropic.APIConnectionError) as info:
        real.run(task, "q", CTX)
    assert "network disabled in tests" in str(info.value.__cause__)
