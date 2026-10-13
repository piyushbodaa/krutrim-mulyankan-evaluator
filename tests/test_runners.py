from pathlib import Path

import pytest

from kritrim_mulyankan.runners import (
    AgentRunner,
    EchoRunner,
    HarnessRunner,
    ModelRunner,
    OracleRunner,
    RunContext,
    make_runner,
)

CTX = RunContext(workdir=Path("."), seed=0, timeout_s=10)


@pytest.mark.parametrize(
    ("spec", "cls", "kind"),
    [
        ("echo", EchoRunner, "fake"),
        ("oracle", OracleRunner, "fake"),
        ("model:anthropic/claude-opus-5-5", ModelRunner, "model"),
        ("harness:codex", HarnessRunner, "harness"),
        ("agent:hermes", AgentRunner, "agent"),
    ],
)
def test_make_runner(spec, cls, kind):
    runner = make_runner(spec)
    assert isinstance(runner, cls) and runner.kind == kind and runner.name == spec


@pytest.mark.parametrize("spec", ["nope", "model:", "model:no-provider", "harness:", "x:y"])
def test_bad_runner_specs(spec):
    with pytest.raises(ValueError):
        make_runner(spec)


def test_model_runner_parses_provider_and_model():
    r = make_runner("model:openrouter/meta-llama/llama-4")
    assert (r.provider, r.model) == ("openrouter", "meta-llama/llama-4")


def test_echo_returns_the_prompt_for_free(example_suite):
    task = example_suite.tasks[0]
    prompt = task.render_prompt()
    out = EchoRunner().run(task, prompt, CTX)
    assert out.output == prompt and out.error is None
    assert out.usage.cost_usd == 0 and out.usage.input_tokens > 0
    assert EchoRunner().estimate_cost_usd(task, prompt) == 0


def test_oracle_returns_expected_or_reference(example_suite):
    by_id = {t.id: t for t in example_suite.tasks}
    oracle = OracleRunner()
    exact = by_id["life-admin.bill-due-date"]
    assert oracle.run(exact, exact.render_prompt(), CTX).output == "2026-11-05"
    judged = by_id["writing.late-order-apology"]
    assert oracle.run(judged, judged.render_prompt(), CTX).output == judged.reference


@pytest.mark.parametrize("spec", ["harness:claude", "agent:grok-bot"])
def test_harness_and_agent_runners_are_still_stubs(spec, example_suite):
    runner = make_runner(spec)
    task = example_suite.tasks[0]
    assert runner.estimate_cost_usd(task, task.render_prompt()) is None
    assert runner.problems() == [f"{type(runner).__name__} is not implemented yet"]
    with pytest.raises(NotImplementedError):
        runner.run(task, task.render_prompt(), CTX)
