"""Paired verdicts: exact sign test, Holm adjustment, bootstrap range and track separation."""

import math
from statistics import mean

import pytest

from kritrim_mulyankan.verdicts import (
    ALPHA,
    bootstrap_ci,
    holm,
    paired_verdicts,
    sign_test,
    task_tries,
    task_waits,
)

# The published 13 Oct 2026 CLI pilot: task, Claude Code (passed, seconds), Muse Code (passed, s).
PILOT = [
    ("NUM01", 1, 4.29, 1, 29.32), ("NUM02", 1, 3.75, 1, 20.79), ("NUM03", 0, 3.58, 1, 27.97),
    ("NUM04", 1, 4.44, 1, 54.83), ("DATE01", 1, 4.09, 1, 15.57), ("DATE02", 1, 3.75, 1, 51.74),
    ("DATE03", 1, 4.30, 1, 102.61), ("DATE04", 1, 4.65, 1, 14.98), ("DOC01", 1, 4.16, 1, 34.18),
    ("DOC02", 1, 3.92, 1, 66.20), ("DOC03", 1, 4.80, 0, 42.20), ("DOC04", 1, 3.55, 1, 25.39),
    ("LANG01", 1, 3.70, 1, 14.64), ("LANG02", 1, 4.48, 1, 22.22), ("LANG03", 1, 4.18, 1, 20.22),
    ("LANG04", 1, 4.66, 1, 50.57), ("EVID01", 1, 4.09, 1, 55.02), ("EVID02", 1, 3.51, 1, 21.49),
    ("EVID03", 1, 4.37, 0, 17.57), ("EVID04", 1, 4.00, 1, 14.31), ("ROB01", 1, 3.95, 1, 40.35),
    ("ROB02", 1, 3.52, 1, 20.24), ("ROB03", 1, 4.45, 1, 21.16), ("ROB04", 1, 3.62, 1, 29.59),
]  # fmt: skip
HARNESS = [
    {"runner_id": "claude-code", "evaluation_track": "harness"},
    {"runner_id": "muse-code", "evaluation_track": "harness"},
]


def row(task, runner, passed, latency=1.0, *, status="ok", grade="graded", repeat=0):
    return {
        "task_id": task, "runner_id": runner, "status": status, "grade_status": grade,
        "passed": bool(passed), "latency_s": latency, "capability_track": "text",
        "repeat_index": repeat,
    }  # fmt: skip


def pilot_rows():
    rows = []
    for task, claude, claude_s, muse, muse_s in PILOT:
        rows += [row(task, "claude-code", claude, claude_s), row(task, "muse-code", muse, muse_s)]
    return rows


def test_sign_test_matches_exact_binomial_values():
    assert sign_test(0, 0) == 1.0
    assert sign_test(2, 1) == 1.0  # McNemar exact for the pilot's 2 vs 1 discordant tasks
    assert sign_test(5, 0) == pytest.approx(0.0625)  # five one-sided wins cannot reach 0.05
    assert sign_test(6, 0) == pytest.approx(0.03125)
    assert sign_test(0, 24) == pytest.approx(2 * 0.5**24)
    assert sign_test(7, 3) == sign_test(3, 7) == pytest.approx(0.34375)


def test_holm_adjustment():
    assert holm([0.01, 0.04, 0.03]) == pytest.approx([0.03, 0.06, 0.06])
    assert holm([0.2]) == [0.2]
    assert holm([]) == []


def test_bootstrap_range_is_reproducible_and_needs_two_tasks():
    values = [0.0, 1.0, 0.5, 1.0, 0.0, 1.0]
    assert bootstrap_ci([1.0]) is None
    low, high = bootstrap_ci(values)
    assert (low, high) == bootstrap_ci(values)
    assert low < mean(values) < high


def test_pilot_accuracy_is_too_close_to_call():
    verdicts = {v.metric: v for v in paired_verdicts(pilot_rows(), HARNESS)}
    accuracy = verdicts["accuracy"]
    assert accuracy.verdict == "too_close_to_call"
    assert (accuracy.a_better, accuracy.b_better, accuracy.tied) == (2, 1, 21)
    assert accuracy.p_value == accuracy.p_value_adjusted == 1.0
    assert accuracy.favours == "claude-code"
    assert accuracy.estimate == pytest.approx(100 / 24)
    low, high = accuracy.ci_95
    assert low < 0 < high
    assert "too few to rule out chance" in accuracy.explanation


def test_pilot_speed_is_clearly_different():
    speed = {v.metric: v for v in paired_verdicts(pilot_rows(), HARNESS)}["speed"]
    expected = math.exp(mean(math.log(muse_s / claude_s) for _, _, claude_s, _, muse_s in PILOT))
    assert speed.verdict == "clearly_different"
    assert (speed.a_better, speed.b_better, speed.tied) == (24, 0, 0)
    assert speed.p_value == pytest.approx(2 * 0.5**24)
    assert speed.favours == "claude-code" and speed.estimate == pytest.approx(expected)
    assert 1 < speed.ci_95[0] < speed.estimate < speed.ci_95[1]
    assert speed.explanation.startswith("claude-code was faster on all 24 tasks.")


def test_a_one_sided_sweep_of_six_tasks_is_clearly_different():
    rows = [row(f"T{i}", "a", 1) for i in range(6)] + [row(f"T{i}", "b", 0) for i in range(6)]
    verdict = paired_verdicts(rows, [{"runner_id": "a"}, {"runner_id": "b"}])[0]
    assert verdict.verdict == "clearly_different" and verdict.p_value < ALPHA


def test_systems_on_different_tracks_are_never_compared():
    models = [
        {"runner_id": "claude-code", "evaluation_track": "harness"},
        {"runner_id": "muse-code", "evaluation_track": "model_api"},
    ]
    assert paired_verdicts(pilot_rows(), models) == []


def test_three_systems_are_holm_adjusted():
    rows = pilot_rows() + [row(t, "third", 1, 10.0) for t, *_ in PILOT]
    models = [*HARNESS, {"runner_id": "third", "evaluation_track": "harness"}]
    verdicts = paired_verdicts(rows, models)
    assert len(verdicts) == 6  # 3 pairs x accuracy and speed
    assert all(v.p_value_adjusted >= v.p_value for v in verdicts)


def test_repeats_compare_the_share_of_tries_right_per_task():
    rows = []
    for task in ("T1", "T2", "T3"):
        for repeat in range(3):
            rows.append(row(task, "steady", 1, repeat=repeat))
            rows.append(row(task, "shaky", repeat != 2, repeat=repeat))  # right 2 of 3 tries
    assert task_tries([r for r in rows if r["runner_id"] == "shaky"]) == {
        "T1": (2, 3), "T2": (2, 3), "T3": (2, 3)
    }  # fmt: skip
    accuracy = paired_verdicts(rows, [{"runner_id": "steady"}, {"runner_id": "shaky"}])[0]
    assert accuracy.tries_per_task == 3 and accuracy.a_better == 3
    assert accuracy.estimate == pytest.approx(100 / 3)
    assert accuracy.verdict == "too_close_to_call"  # three tasks can never be conclusive


def test_errors_count_as_wrong_and_skipped_or_pending_tries_are_left_out():
    rows = [
        row("T1", "a", 0, status="error", grade=None),
        row("T2", "a", 1),
        row("T3", "a", 0, status="skipped", grade=None),
        row("T4", "a", 0, grade="pending"),
    ]
    assert task_tries(rows) == {"T1": (0, 1), "T2": (1, 1)}
    assert task_waits(rows) == {"T2": 1.0, "T4": 1.0}
