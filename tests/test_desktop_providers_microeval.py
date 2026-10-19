"""Every lab is connectable in the desktop app, and a micro-eval mined from
the person's own chats can be built and run there. Regression coverage for
the accounts screen that only offered Anthropic."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from kritrim_mulyankan.desktop_bridge import DesktopSession

FIXTURES = Path(__file__).parent / "fixtures" / "history"
ALL_EXPORTS = [
    str(FIXTURES / "chatgpt-conversations.json"),
    str(FIXTURES / "claude-conversations.json"),
    str(FIXTURES / "claude-code-session.jsonl"),
]


@pytest.fixture
def session(settings):
    desktop = DesktopSession(settings=settings, env={})
    yield desktop
    desktop.close()


def test_catalog_offers_every_lab_with_openrouter_first(session):
    catalog = session.catalog()
    providers = [p["id"] for p in catalog["providers"]]
    assert providers[0] == "openrouter"
    assert catalog["providers"][0]["recommended"] is True
    runnable = {}
    for model in catalog["models"]:
        if model["runnable"] and not model["free"]:
            runnable.setdefault(model["provider"], []).append(model["spec"])
    # No lab is a dead-end "add a price first" card any more.
    for provider in ("openrouter", "openai", "anthropic", "gemini", "xai"):
        assert runnable.get(provider), provider
    assert "model:openrouter/anthropic/claude-sonnet-4.6" in runnable["openrouter"]
    assert "model:openrouter/openai/gpt-5-mini" in runnable["openrouter"]
    assert "model:openrouter/google/gemini-2.5-flash" in runnable["openrouter"]
    assert not any(model["spec"] is None for model in catalog["models"])


def test_openrouter_run_is_priced_and_key_gated(session):
    spec = "model:openrouter/openai/gpt-5-mini"
    refused = session.estimate(suite="india-pilot", runners=[spec], max_usd=5)
    assert refused["ok"] is False
    assert refused["worst_case_usd"] > 0  # priced, just missing the key
    session.set_keys({"OPENROUTER_API_KEY": "sk-or-test-key"})
    allowed = session.estimate(suite="india-pilot", runners=[spec], max_usd=5)
    assert allowed["ok"] is True
    assert allowed["paid"] is True


def test_history_preview_counts_without_content(session):
    preview = session.history_preview(ALL_EXPORTS)
    assert preview["prompts"] >= 3
    assert set(preview["by_source"]) == {"chatgpt", "claude", "claude_code"}
    blob = json.dumps(preview)
    assert "GST" not in blob and "RLWL" not in blob  # counts only, no prompts


def _fake_judge(prompt: str) -> str:
    if "checking whether" in prompt:  # mining template
        if "GST" in prompt:
            verdict, expected, rubric = "wrong", "50150", "Must equal 50150."
        elif "RLWL" in prompt:
            verdict, expected, rubric = (
                "wrong",
                "No. A waitlisted RLWL e-ticket holder cannot board.",
                "Must say the passenger cannot board.",
            )
        else:
            return json.dumps({"verdict": "correct", "confidence": 0.9})
        return json.dumps(
            {"verdict": verdict, "confidence": 0.95, "expected": expected, "rubric": rubric}
        )
    # Grading template: the oracle runner echoes the reference answer.
    return json.dumps({"score": 9, "reason": "matches the reference"})


def test_micro_eval_mines_into_my_tests_and_runs_graded(session, settings, monkeypatch):
    monkeypatch.setattr(
        "kritrim_mulyankan.desktop_bridge.make_judge",
        lambda spec, env=None: _fake_judge,
    )
    estimate = session.micro_eval_estimate(paths=ALL_EXPORTS, judge="ollama/fake-judge")
    assert estimate["worst_case_usd"] == 0  # local judge is free
    assert estimate["paid"] is False
    assert estimate["selected"] >= 3

    built = session.micro_eval(
        paths=ALL_EXPORTS,
        judge="ollama/fake-judge",
        name="Work chats",
        max_usd=1,
        confirmed=True,
    )
    assert built["built"] is True
    assert built["tasks"] >= 1
    suite_id = built["suite_id"]
    assert suite_id.startswith("my-")
    # The private evidence sidecar sits next to the suite, never inside it.
    assert (settings.runs_dir / "my-tests" / f"{suite_id}.evidence.json").is_file()

    packs = {pack["id"]: pack for pack in session.catalog()["packs"]}
    assert packs[suite_id]["mine"] is True
    assert packs[suite_id]["judged"] is True

    # The run is judge-graded through the desktop bridge, not left pending.
    run_estimate = session.estimate(suite=suite_id, runners=["oracle"], max_usd=1)
    assert run_estimate["ok"] is True
    assert run_estimate["judge"]["spec"] == "ollama/fake-judge"
    result = session.run(suite=suite_id, runners=["oracle"], confirmed=True, max_usd=1)
    report = session.report(result["run_id"])
    graded = [row for row in report["results"] if row["grade_status"] == "graded"]
    assert graded and all(row["passed"] for row in graded)


def test_micro_eval_refuses_without_confirmation_or_judge_key(session):
    with pytest.raises(ValueError, match="Confirm"):
        session.micro_eval(paths=ALL_EXPORTS, judge="ollama/fake-judge", confirmed=False)
    estimate = session.micro_eval_estimate(paths=ALL_EXPORTS, judge="openrouter/openai/gpt-5-mini")
    assert estimate["worst_case_usd"] is None  # no key: nothing may be called
    assert estimate["problems"]
