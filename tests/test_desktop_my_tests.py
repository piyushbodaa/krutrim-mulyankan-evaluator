"""The desktop app's "write your own test" flow, end to end against the Rust engine.

People type questions and right answers; the engine saves them as a normal test pack in
the app's data folder, runs them with the free demo bots, and shows each reply next to the
right answer. Ready-made pilot prompts and answers stay hidden.

Skipped unless km-desktop is built (set KM_RUST_DESKTOP_BIN or build km-rs/ first).
"""

import json
import os
import re
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
UI = ROOT / "desktop" / "ui"


def _engine() -> Path | None:
    if env := os.environ.get("KM_RUST_DESKTOP_BIN"):
        return Path(env) if Path(env).is_file() else None
    for candidate in (
        ROOT / "km-rs/target/release/km-desktop",
        ROOT / "km-rs/target/debug/km-desktop",
    ):
        if candidate.is_file():
            return candidate
    return None


ENGINE = _engine()
needs_engine = pytest.mark.skipif(ENGINE is None, reason="Rust km-desktop not built")

QUESTIONS = [
    {"question": "What is the capital of Rajasthan?", "answer": "Jaipur", "check": "includes"},
    {"question": "What is 18% GST on ₹500?", "answer": "₹90", "check": "number"},
    {"question": "'सोमवार' को अंग्रेज़ी में क्या कहते हैं?", "answer": "Monday", "check": "exact"},
    {"question": "Pick one: (a) ₹5 [x] or $1?", "answer": "(a) ₹5 [x] .*+?", "check": "includes"},
]


def _call(tmp_path: Path, requests: list[dict]) -> dict:
    env = {
        **os.environ,
        "MJ_RUNS_DIR": str(tmp_path / "runs"),
        "MJ_DB": str(tmp_path / "runs/results.db"),
        "MJ_SUITES_DIR": str(ROOT / "suites"),
        "KM_RESOURCE_ROOT": str(ROOT),
    }
    for name in ("ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY", "XAI_API_KEY"):
        env.pop(name, None)
    lines = "\n".join(json.dumps(request) for request in requests) + "\n"
    proc = subprocess.run(
        [str(ENGINE)], input=lines, text=True, capture_output=True, timeout=300, env=env
    )
    assert proc.returncode == 0, proc.stderr
    replies = [json.loads(line) for line in proc.stdout.splitlines() if line.strip()]
    return {reply.get("id"): reply for reply in replies if "ok" in reply}


@needs_engine
def test_own_test_saves_runs_and_shows_every_reply(tmp_path):
    saved = _call(
        tmp_path,
        [{"id": "s", "method": "save_test", "params": {"name": "GK", "questions": QUESTIONS}}],
    )
    test_id = saved["s"]["result"]["id"]
    assert re.fullmatch(r"my-[0-9a-f]{12}", test_id)
    assert (tmp_path / "runs/my-tests" / f"{test_id}.yaml").is_file()

    run_params = {
        "suite": test_id,
        "runners": ["oracle", "echo"],
        "max_usd": 0,
        "repeats": 1,
        "confirmed": True,
    }
    replies = _call(
        tmp_path,
        [
            {"id": "g", "method": "get_test", "params": {"id": test_id}},
            {"id": "c", "method": "catalog", "params": {}},
            {"id": "p", "method": "pack_costs", "params": {"suite": test_id}},
            {"id": "r", "method": "run", "params": run_params},
        ],
    )
    form = replies["g"]["result"]
    assert form["name"] == "GK"
    assert [q["question"] for q in form["questions"]] == [q["question"] for q in QUESTIONS]
    assert [q["check"] for q in form["questions"]] == ["includes", "number", "exact", "includes"]
    assert form["questions"][3]["answer"] == "(a) ₹5 [x] .*+?"  # symbols survive the round trip
    assert form["questions"][1]["answer"] == "90"

    packs = {pack["id"]: pack for pack in replies["c"]["result"]["packs"]}
    assert packs[test_id]["mine"] is True and packs[test_id]["name"] == "GK"
    assert packs["india-pilot"]["mine"] is False
    models = {model["spec"]: model for model in replies["c"]["result"]["models"]}
    assert models["oracle"]["name"] == "Answer-key bot"
    assert models["echo"]["name"] == "Parrot bot"
    assert all(
        cost == 0 for spec, cost in replies["p"]["result"]["costs"].items() if ":" not in spec
    )

    run = replies["r"]["result"]
    assert run["status"] == "completed"
    answers = _call(
        tmp_path, [{"id": "a", "method": "answers", "params": {"run_id": run["run_id"]}}]
    )["a"]["result"]
    marks = {(row["task_id"], row["runner"]): row["passed"] for row in answers["answers"]}
    assert all(marks[(f"Q{n:02}", "oracle")] is True for n in range(1, 5))
    assert not any(marks[(f"Q{n:02}", "echo")] for n in range(1, 5))
    echo = next(
        row for row in answers["answers"] if row["runner"] == "echo" and row["task_id"] == "Q01"
    )
    assert echo["output"] == QUESTIONS[0]["question"] and echo["label"] == "Parrot bot"
    assert not any(row["edited"] for row in answers["answers"])


@needs_engine
def test_editing_flags_old_replies_and_delete_removes_only_own_tests(tmp_path):
    first = _call(
        tmp_path,
        [
            {
                "id": "s",
                "method": "save_test",
                "params": {"name": "Mine", "questions": QUESTIONS[:2]},
            }
        ],
    )
    test_id = first["s"]["result"]["id"]
    run_params = {
        "suite": test_id,
        "runners": ["oracle"],
        "max_usd": 0,
        "repeats": 1,
        "confirmed": True,
    }
    run_id = _call(tmp_path, [{"id": "r", "method": "run", "params": run_params}])["r"]["result"][
        "run_id"
    ]
    edited = [{**QUESTIONS[0], "question": "What is the capital of Rajasthan state?"}, QUESTIONS[1]]
    replies = _call(
        tmp_path,
        [
            {
                "id": "e",
                "method": "save_test",
                "params": {"id": test_id, "name": "Mine", "questions": edited},
            },
            {"id": "a", "method": "answers", "params": {"run_id": run_id}},
            {"id": "pilot", "method": "get_test", "params": {"id": "india-pilot"}},
            {"id": "escape", "method": "delete_test", "params": {"id": "../india-pilot"}},
            {"id": "d", "method": "delete_test", "params": {"id": test_id}},
            {"id": "c", "method": "catalog", "params": {}},
        ],
    )
    assert replies["e"]["result"]["id"] == test_id
    flags = {row["task_id"]: row["edited"] for row in replies["a"]["result"]["answers"]}
    assert flags == {"Q01": True, "Q02": False}
    assert replies["pilot"]["ok"] is False and replies["escape"]["ok"] is False
    assert replies["d"]["ok"] is True
    ids = {pack["id"] for pack in replies["c"]["result"]["packs"]}
    assert test_id not in ids and {"india-pilot", "example", "railway-pilot"} <= ids


@needs_engine
def test_bad_questions_get_plain_errors_and_pilot_answers_stay_hidden(tmp_path):
    bad = [
        ({"name": "", "questions": QUESTIONS}, "Give your test a name."),
        ({"name": "x", "questions": []}, "Add at least one question."),
        (
            {"name": "x", "questions": [{"question": "q", "answer": "", "check": "includes"}]},
            "needs the right answer",
        ),
        (
            {"name": "x", "questions": [{"question": "q", "answer": "abc", "check": "number"}]},
            "must be one number",
        ),
        (
            {
                "name": "x",
                "questions": [{"question": "a {{ b }}", "answer": "1", "check": "includes"}],
            },
            "curly brackets",
        ),
    ]
    run_params = {
        "suite": "example",
        "runners": ["oracle"],
        "max_usd": 0,
        "repeats": 1,
        "confirmed": True,
    }
    replies = _call(
        tmp_path,
        [
            {"id": str(i), "method": "save_test", "params": params}
            for i, (params, _) in enumerate(bad)
        ]
        + [{"id": "r", "method": "run", "params": run_params}],
    )
    for i, (_, message) in enumerate(bad):
        assert replies[str(i)]["ok"] is False and message in replies[str(i)]["error"]
    assert not (tmp_path / "runs/my-tests").exists() or not any(
        (tmp_path / "runs/my-tests").iterdir()
    )
    run_id = replies["r"]["result"]["run_id"]
    hidden = _call(tmp_path, [{"id": "a", "method": "answers", "params": {"run_id": run_id}}])["a"]
    assert hidden["ok"] is False and "yourself" in hidden["error"]


def test_everyday_copy_avoids_jargon():
    """The main screens speak plainly: no tokens, suites, runners or MTok.

    The side-by-side screen ("side." strings) is the one place that shows
    tokens and context, because comparing models live is about them.
    """
    script = (UI / "i18n.js").read_text(encoding="utf-8")
    block = script.split("  en: {", 1)[1].split("  hi: {", 1)[0]
    lines = block.splitlines()
    english = "\n".join(line for line in lines if not line.strip().startswith('"side.'))
    for word in ("MTok", "token", "suite", "runner", "API key", "worst case", "harness"):
        assert word not in english, word
    html = (UI / "index.html").read_text(encoding="utf-8")
    for screen in (
        "home",
        "questions",
        "builder",
        "ais",
        "start",
        "running",
        "results",
        "accounts",
    ):
        assert f'<section id="{screen}" class="screen"' in html
    assert "Unvalidated pilot" in html and "Not a ranking" in html and "Too close to call" in html
