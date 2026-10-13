"""LLM-as-judge grader (stub).

The prompts, parsing and blind, order-swapped pairwise logic are real; the call to a judge model
is not wired up yet. A judge is any `Callable[[str], str]` (prompt in, reply out). With no judge
the grader returns status "pending", so nothing is spent and nothing is guessed.
"""

from __future__ import annotations

import json
import re
from collections.abc import Callable
from dataclasses import dataclass
from typing import Literal

from kritrim_mulyankan.graders.base import Grade, Grader
from kritrim_mulyankan.suite import LLMJudgeGraderSpec, Task

Judge = Callable[[str], str]

_JSON_OBJECT = re.compile(r"\{.*\}", re.DOTALL)

ABSOLUTE_TEMPLATE = """\
You are an impartial grader. Score the RESPONSE to the TASK against the RUBRIC on an integer \
scale from 1 (fails the rubric) to 10 (fully satisfies it). Judge only what the rubric asks \
for; do not reward length or confidence.

<task>
{task}
</task>

<rubric>
{rubric}
</rubric>
{reference}
<response>
{response}
</response>

Reply with JSON only: {{"score": <integer 1-10>, "reason": "<one sentence>"}}"""

PAIRWISE_TEMPLATE = """\
You are an impartial grader. Two anonymous responses to the same TASK follow. Their order is \
random and must not influence you. Decide which better satisfies the RUBRIC.

<task>
{task}
</task>

<rubric>
{rubric}
</rubric>
{reference}
<response_1>
{first}
</response_1>

<response_2>
{second}
</response_2>

Reply with JSON only: {{"winner": "1" | "2" | "tie", "reason": "<one sentence>"}}"""


def _reference_block(task: Task) -> str:
    return (
        f"\n<reference_answer>\n{task.reference}\n</reference_answer>\n" if task.reference else ""
    )


def build_absolute_prompt(task: Task, response: str) -> str:
    return ABSOLUTE_TEMPLATE.format(
        task=task.render_prompt(),
        rubric=task.rubric,
        reference=_reference_block(task),
        response=response,
    )


def _parse_json(reply: str) -> dict:
    match = _JSON_OBJECT.search(reply)
    if not match:
        raise ValueError("judge reply contains no JSON object")
    data = json.loads(match.group(0))
    if not isinstance(data, dict):
        raise ValueError("judge reply JSON is not an object")
    return data


def parse_absolute(reply: str) -> tuple[int, str]:
    data = _parse_json(reply)
    score = data.get("score")
    if isinstance(score, bool) or not isinstance(score, int | float) or not 1 <= score <= 10:
        raise ValueError(f"judge score must be a number from 1 to 10, got {score!r}")
    return round(score), str(data.get("reason", ""))


class LLMJudgeGrader(Grader):
    name = "llm_judge"

    def __init__(self, spec: LLMJudgeGraderSpec, judge: Judge | None = None) -> None:
        self.spec = spec
        self.judge = judge

    def grade(self, task: Task, output: str) -> Grade:
        if self.spec.mode == "pairwise":
            # Pairwise needs two outputs; the engine will collect pairs after all runners finish.
            return Grade(grader=self.name, status="pending", details={"reason": "pairwise"})
        if self.judge is None:
            return Grade(
                grader=self.name, status="pending", details={"reason": "no judge configured"}
            )
        prompt = build_absolute_prompt(task, output)
        try:
            reply = self.judge(prompt)
            score, reason = parse_absolute(reply)
        except Exception as exc:  # a broken judge must not crash the run
            return Grade(grader=self.name, status="error", details={"error": repr(exc)})
        return Grade(
            grader=self.name,
            status="graded",
            passed=score >= self.spec.pass_threshold,
            score=(score - 1) / 9,
            details={"raw_score": score, "reason": reason, "judge_model": self.spec.judge_model},
        )


# --- pairwise: blind and order-swapped ----------------------------------------------------------

Winner = Literal["a", "b", "tie"]


@dataclass(frozen=True)
class PairwiseVerdict:
    winner: Winner
    votes: tuple[Winner, Winner]
    """Winner from (a shown first, b shown first). Disagreement means position bias: a tie."""


def build_pairwise_prompts(task: Task, a: str, b: str) -> tuple[str, str]:
    """Same comparison twice, a shown first then b shown first. Runner names never appear."""

    def render(first: str, second: str) -> str:
        return PAIRWISE_TEMPLATE.format(
            task=task.render_prompt(),
            rubric=task.rubric,
            reference=_reference_block(task),
            first=first,
            second=second,
        )

    return render(a, b), render(b, a)


def parse_pairwise(reply: str) -> Literal["1", "2", "tie"]:
    winner = str(_parse_json(reply).get("winner", "")).strip().lower()
    if winner not in {"1", "2", "tie"}:
        raise ValueError(f"judge winner must be '1', '2' or 'tie', got {winner!r}")
    return winner  # type: ignore[return-value]


def judge_pair(task: Task, a: str, b: str, judge: Judge) -> PairwiseVerdict:
    ab_prompt, ba_prompt = build_pairwise_prompts(task, a, b)
    ab = {"1": "a", "2": "b", "tie": "tie"}[parse_pairwise(judge(ab_prompt))]
    ba = {"1": "b", "2": "a", "tie": "tie"}[parse_pairwise(judge(ba_prompt))]
    return PairwiseVerdict(winner=ab if ab == ba else "tie", votes=(ab, ba))  # type: ignore[arg-type]
