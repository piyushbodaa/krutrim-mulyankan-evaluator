"""History import -> automatic mining -> micro-eval -> graded run, all offline."""

from __future__ import annotations

import json
import re
import zipfile
from pathlib import Path

import pytest

from kritrim_mulyankan.engine import run_suite
from kritrim_mulyankan.history import load_history, parse_chatgpt, parse_claude, parse_claude_code
from kritrim_mulyankan.microeval import build_suite, write_micro_eval
from kritrim_mulyankan.miner import mine_failures, parse_verdict
from kritrim_mulyankan.runners import EchoRunner, OracleRunner
from kritrim_mulyankan.suite import load_suite

FIXTURES = Path(__file__).parent / "fixtures" / "history"
JUDGE = "ollama/fake-judge"


def fake_mining_judge(prompt: str) -> str:
    def reply(verdict, confidence, reason="", expected="", rubric=""):
        return json.dumps(
            {
                "verdict": verdict,
                "confidence": confidence,
                "reason": reason,
                "expected": expected,
                "rubric": rubric,
            }
        )

    if "GST" in prompt:
        return reply("wrong", 0.95, "GST applied twice", "50150", "Must equal 50150.")
    if "RLWL" in prompt:
        return reply(
            "wrong", 0.9, "Waitlisted tickets cannot board",
            "No. A waitlisted RLWL e-ticket holder cannot board.",
            "Must say the passenger cannot board.",
        )  # fmt: skip
    if "sum(x*x" in prompt:
        return reply("wrong", 0.6, "Wrong value", "14", "Must equal 14.")
    if "poem" in prompt:
        return reply("unclear", 0.9, "No checkable answer")
    if "explode" in prompt:
        raise RuntimeError("judge unavailable")
    return reply("correct", 0.9)


def all_pairs():
    history = load_history(
        [
            FIXTURES / "chatgpt-conversations.json",
            FIXTURES / "claude-conversations.json",
            FIXTURES / "claude-code-session.jsonl",
        ]
    )
    return history.pairs


# --- parsers ---------------------------------------------------------------------------------


def test_parse_chatgpt_follows_current_branch_and_keeps_model():
    convs = parse_chatgpt(FIXTURES / "chatgpt-conversations.json")
    assert len(convs) == 1
    pairs = convs[0].pairs()
    assert [(p.prompt, p.answer, p.model) for p in pairs] == [
        ("Add 18% GST to 42500. Number only.", "59177", "gpt-test"),
        ("What is 2+2 {{ math }}?", "4", "gpt-test"),
    ]  # the regenerated "50150" branch is not the kept conversation


def test_parse_chatgpt_zip(tmp_path):
    archive = tmp_path / "export.zip"
    with zipfile.ZipFile(archive, "w") as z:
        z.write(FIXTURES / "chatgpt-conversations.json", "conversations.json")
    assert len(parse_chatgpt(archive)[0].pairs()) == 2


def test_parse_claude_blocks_and_text_fallback():
    convs = parse_claude(FIXTURES / "claude-conversations.json")
    pairs = convs[0].pairs()
    assert len(pairs) == 2
    assert pairs[0].answer.startswith("Yes, you can board")
    assert pairs[1].prompt.startswith("Thanks")


def test_parse_claude_code_drops_tools_meta_and_commands():
    convs = parse_claude_code(FIXTURES / "claude-code-session.jsonl")
    pairs = convs[0].pairs()
    assert len(pairs) == 1
    assert pairs[0].answer == "30" and pairs[0].source == "claude_code"


def test_load_history_autodetects_and_combines():
    pairs = all_pairs()
    assert len(pairs) == 5
    assert {p.source for p in pairs} == {"chatgpt", "claude", "claude_code"}


def test_missing_history_file_is_a_clear_error():
    with pytest.raises(FileNotFoundError):
        load_history([FIXTURES / "nope.json"])


# --- miner -----------------------------------------------------------------------------------


def test_mine_keeps_only_confident_wrong_with_expected():
    result = mine_failures(all_pairs(), fake_mining_judge, judge_spec=JUDGE)
    assert result.checked == 5
    assert result.correct == 1 and result.unclear == 1
    assert result.below_threshold == 1  # the 0.6-confidence sum(x*x) verdict
    assert [f.pair.source for f in result.failures] == ["chatgpt", "claude"]
    assert result.failures[0].verdict.expected == "50150"


def test_mine_counts_judge_errors_instead_of_crashing():
    pair = all_pairs()[0]
    boom = type(pair)(
        source=pair.source, conversation_id="x", conversation_title="",
        turn_index=0, created_at=None, model=None, prompt="explode now", answer="x",
    )  # fmt: skip
    result = mine_failures([boom], fake_mining_judge, judge_spec=JUDGE)
    assert result.errors == 1 and result.failures == []


def test_parse_verdict_rejects_garbage():
    with pytest.raises(ValueError):
        parse_verdict("no json here")
    with pytest.raises(ValueError):
        parse_verdict('{"verdict": "maybe", "confidence": 0.5}')
    with pytest.raises(ValueError):
        parse_verdict('{"verdict": "wrong", "confidence": 2}')


# --- micro-eval builder ----------------------------------------------------------------------


def mined():
    return mine_failures(all_pairs(), fake_mining_judge, judge_spec=JUDGE).failures


def test_build_suite_is_valid_and_keeps_prompts_verbatim():
    suite = build_suite(mined(), name="mine-test", version="2026-10-18", judge_spec=JUDGE)
    assert len(suite.tasks) == 2
    first = suite.tasks[0]
    assert first.render_prompt() == "Add 18% GST to 42500. Number only."
    assert first.grader.type == "llm_judge" and first.reference == "50150"
    assert first.provenance[0]["judge_reason"] == "GST applied twice"


def test_write_micro_eval_roundtrip_and_evidence(tmp_path):
    dest = tmp_path / "my.yaml"
    suite, evidence_path = write_micro_eval(
        mined(), dest, name="mine-test", version="2026-10-18", judge_spec=JUDGE
    )
    assert load_suite(dest).tasks[0].id == suite.tasks[0].id
    evidence = json.loads(evidence_path.read_text())
    assert evidence["items"][0]["original_answer"] == "59177"
    with pytest.raises(FileExistsError):
        write_micro_eval(mined(), dest, name="mine-test", version="1", judge_spec=JUDGE)


def test_placeholder_prompt_survives_as_task():
    failures = mined()
    pair = all_pairs()[1]  # "What is 2+2 {{ math }}?"
    from kritrim_mulyankan.miner import MinedFailure, MineVerdict

    fake = MinedFailure(
        pair=pair,
        verdict=MineVerdict("wrong", 0.9, "r", "4", "Must equal 4."),
        judge_spec=JUDGE,
    )
    suite = build_suite([fake, *failures], name="x", version="1", judge_spec=JUDGE)
    assert suite.tasks[0].render_prompt() == "What is 2+2 {{math}}?"


# --- end to end: mine -> suite -> graded run -------------------------------------------------


def fake_grading_judge(prompt: str) -> str:
    task = re.search(r"<task>\n(.*?)\n</task>", prompt, re.DOTALL).group(1)
    response = re.search(r"<response>\n(.*?)\n</response>", prompt, re.DOTALL).group(1)
    score = 1 if response.strip() == task.strip() else 10
    return json.dumps({"score": score, "reason": "fake judge"})


def test_mined_suite_runs_and_grades(settings, store):
    suite = build_suite(mined(), name="mine-test", version="1", judge_spec=JUDGE)
    summary = run_suite(
        suite,
        [EchoRunner(), OracleRunner()],
        store=store,
        settings=settings,
        judge=fake_grading_judge,
    )
    by_runner: dict[str, list] = {}
    for record in summary.results:
        by_runner.setdefault(record.runner, []).append(record.passed)
    assert by_runner["oracle"] == [True, True]
    assert by_runner["echo"] == [False, False]


# --- CLI -------------------------------------------------------------------------------------


def test_cli_history_counts_only(km):
    result = km("history", str(FIXTURES / "chatgpt-conversations.json"))
    assert result.exit_code == 0, result.output
    assert "1 conversations, 2 prompts" in result.output
    assert "GST" not in result.output  # counts only, never prompt content


def test_cli_micro_eval_dry_run_calls_nothing(km):
    result = km(
        "micro-eval", str(FIXTURES / "claude-conversations.json"),
        "--judge", JUDGE, "--dry-run",
    )  # fmt: skip
    assert result.exit_code == 0, result.output
    assert "Dry run" in result.output and "$0.0000" in result.output  # ollama is free


def test_cli_micro_eval_requires_a_judge(km):
    result = km("micro-eval", str(FIXTURES / "claude-conversations.json"))
    assert result.exit_code == 1
    assert "judge is required" in result.output


def test_cli_micro_eval_builds_suite(km, monkeypatch):
    monkeypatch.setattr("kritrim_mulyankan.cli.make_judge", lambda spec, **_: fake_mining_judge)
    out = km.tmp_path / "suites" / "mine.yaml"
    out.parent.mkdir()
    result = km(
        "micro-eval", str(FIXTURES / "chatgpt-conversations.json"),
        str(FIXTURES / "claude-conversations.json"),
        "--judge", JUDGE, "--out", str(out), "--name", "cli-mine",
    )  # fmt: skip
    assert result.exit_code == 0, result.output
    assert "2 failures kept" in result.output
    assert len(load_suite(out).tasks) == 2
    assert (km.tmp_path / "suites" / "mine.evidence.json").is_file()


# --- sprint: redaction, dedupe, sampling, cache, golden parity -----------------


def test_golden_micro_eval_matches_both_engines():
    from kritrim_mulyankan.miner import judge_prompt_hash

    golden = json.loads((FIXTURES / "expected-micro-eval.json").read_text())
    assert judge_prompt_hash() == golden["judge_prompt_hash"]
    suite = build_suite(mined(), name="mine-test", version="1", judge_spec=JUDGE)
    actual = [
        {"id": t.id, "prompt": t.render_prompt(), "reference": t.reference, "rubric": t.rubric}
        for t in suite.tasks
    ]
    assert actual == golden["tasks"]


def test_secrets_redacted_before_judging():
    from kritrim_mulyankan.miner import build_mine_prompt, redact_secrets

    fake_key = "sk" + "-ant-" + "abcdef12345678"
    assert fake_key not in redact_secrets(f"key {fake_key} end")
    pair = all_pairs()[0]
    secret_pair = type(pair)(
        source=pair.source, conversation_id="s", conversation_title="",
        turn_index=0, created_at=None, model=None,
        prompt="Use api_key=supersecretvalue99 in this snippet", answer="done",
    )  # fmt: skip
    assert "supersecretvalue99" not in build_mine_prompt(secret_pair)
    result = mine_failures([secret_pair], fake_mining_judge, judge_spec=JUDGE)
    assert result.redacted_pairs == 1


def test_select_dedupes_oversizes_and_samples_evenly():
    from kritrim_mulyankan.miner import MAX_PAIR_CHARS, select_pairs

    base = all_pairs()
    many = base + base[:1]  # one exact duplicate
    big = type(base[0])(
        source="chatgpt", conversation_id="big", conversation_title="",
        turn_index=9, created_at=None, model=None,
        prompt="x" * (MAX_PAIR_CHARS + 1), answer="y",
    )  # fmt: skip
    selection = select_pairs([*many, big], limit=2)
    assert selection.duplicates == 1 and selection.oversized == 1
    assert len(selection.pairs) == 2
    assert selection.pairs[0] is base[0] and selection.pairs[1] is not base[1]  # evenly spaced


def test_verdict_cache_avoids_second_judge_call(tmp_path):
    from kritrim_mulyankan.miner import VerdictCache

    calls = {"n": 0}

    def counting_judge(prompt):
        calls["n"] += 1
        return fake_mining_judge(prompt)

    cache = VerdictCache(tmp_path / "cache.jsonl")
    first = mine_failures(all_pairs(), counting_judge, judge_spec=JUDGE, cache=cache)
    assert calls["n"] == 5 and first.cache_hits == 0
    again = VerdictCache(tmp_path / "cache.jsonl")  # reload from disk
    second = mine_failures(all_pairs(), counting_judge, judge_spec=JUDGE, cache=again)
    assert calls["n"] == 5 and second.cache_hits == 5
    assert len(second.failures) == 2


def test_cli_doctor(km):
    result = km("doctor")
    assert result.exit_code == 0, result.output
    assert "suites" in result.output and "MJ_JUDGE" in result.output
