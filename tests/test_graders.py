import json

import pytest

from kritrim_mulyankan.graders import ExactMatchGrader, LLMJudgeGrader, make_grader
from kritrim_mulyankan.graders.llm_judge import build_pairwise_prompts, judge_pair, parse_absolute
from kritrim_mulyankan.suite import Task


def _exact(expected, **spec) -> Task:
    return Task.model_validate({
        "id": "t", "category": "reasoning", "prompt": "q",
        "grader": {"type": "exact", "expected": expected, **spec},
    })  # fmt: skip


def _judged(**spec) -> Task:
    return Task.model_validate({
        "id": "w", "category": "writing", "prompt": "Write a haiku about rain.",
        "rubric": "Three lines, 5-7-5.", "reference": "ref answer",
        "grader": {"type": "llm_judge", **spec},
    })  # fmt: skip


def test_make_grader_picks_by_type():
    assert isinstance(make_grader(_exact("4")), ExactMatchGrader)
    assert isinstance(make_grader(_judged()), LLMJudgeGrader)


@pytest.mark.parametrize("output", ["4", " 4\n", "4 "])
def test_exact_ignores_surrounding_whitespace(output):
    task = _exact("4")
    grade = make_grader(task).grade(task, output)
    assert grade.status == "graded" and grade.passed and grade.score == 1.0


def test_exact_is_case_insensitive_by_default_and_collapses_whitespace():
    task = _exact("New  Delhi")
    assert make_grader(task).grade(task, "new delhi").passed


def test_exact_case_sensitive_option():
    task = _exact("create_event", case_sensitive=True)
    assert not make_grader(task).grade(task, "Create_Event").passed


def test_exact_accepts_any_listed_answer_and_fails_otherwise():
    task = _exact(["5", "₹5"])
    grader = make_grader(task)
    assert grader.grade(task, "₹5").details["matched"] == "₹5"
    fail = grader.grade(task, "10")
    assert fail.passed is False and fail.score == 0.0


def test_llm_judge_without_judge_is_pending():
    task = _judged()
    grade = make_grader(task).grade(task, "anything")
    assert grade.status == "pending" and grade.passed is None and grade.score is None


def test_llm_judge_scores_with_a_fake_judge():
    task = _judged(pass_threshold=7)
    prompts = []

    def judge(prompt: str) -> str:
        prompts.append(prompt)
        return 'Sure! {"score": 8, "reason": "Good shape."}'

    grade = make_grader(task, judge=judge).grade(task, "pitter patter")
    assert grade.status == "graded" and grade.passed is True
    assert grade.score == pytest.approx(7 / 9)
    assert grade.details["raw_score"] == 8
    assert "pitter patter" in prompts[0] and "5-7-5" in prompts[0] and "ref answer" in prompts[0]


@pytest.mark.parametrize("reply", ["no json here", '{"score": 11}', '{"score": "high"}'])
def test_llm_judge_bad_reply_is_an_error_not_a_crash(reply):
    task = _judged()
    grade = make_grader(task, judge=lambda _: reply).grade(task, "x")
    assert grade.status == "error" and grade.passed is None


def test_parse_absolute():
    assert parse_absolute(json.dumps({"score": 10, "reason": "ok"})) == (10, "ok")


def test_pairwise_prompts_swap_order_and_hide_identities():
    task = _judged(mode="pairwise")
    ab, ba = build_pairwise_prompts(task, "ALPHA", "BRAVO")
    assert ab.index("ALPHA") < ab.index("BRAVO")
    assert ba.index("BRAVO") < ba.index("ALPHA")


def test_pairwise_position_bias_becomes_a_tie():
    task = _judged(mode="pairwise")
    verdict = judge_pair(task, "ALPHA", "BRAVO", judge=lambda _: '{"winner": "1"}')
    assert verdict.winner == "tie" and verdict.votes == ("a", "b")


def test_pairwise_consistent_preference_wins():
    task = _judged(mode="pairwise")

    def prefers_bravo(prompt: str) -> str:
        shown_first = prompt.split("<response_1>")[1].split("</response_1>")[0]
        return '{"winner": "1"}' if "BRAVO" in shown_first else '{"winner": "2"}'

    verdict = judge_pair(task, "ALPHA", "BRAVO", judge=prefers_bravo)
    assert verdict.winner == "b" and verdict.votes == ("b", "b")
