"""Grader registry: picks the grader for a task from its grader spec."""

from __future__ import annotations

from pathlib import Path

from kritrim_mulyankan.graders.base import Grade, Grader
from kritrim_mulyankan.graders.deterministic import JSONGrader, NumericGrader, RegexGrader
from kritrim_mulyankan.graders.exact import ExactMatchGrader
from kritrim_mulyankan.graders.llm_judge import Judge, LLMJudgeGrader
from kritrim_mulyankan.graders.workflow import WorkflowGrader
from kritrim_mulyankan.suite import Task


def make_grader(
    task: Task, judge: Judge | None = None, *, fixture_base: Path | None = None
) -> Grader:
    spec = task.grader
    if spec.type == "exact":
        return ExactMatchGrader(spec)
    if spec.type == "llm_judge":
        return LLMJudgeGrader(spec, judge=judge)
    if spec.type == "json":
        return JSONGrader(spec)
    if spec.type == "numeric":
        return NumericGrader(spec)
    if spec.type == "regex":
        return RegexGrader(spec)
    if spec.type == "workflow":
        return WorkflowGrader(spec, fixture_base or Path.cwd())
    raise ValueError(f"no grader for type {spec.type!r}")


__all__ = ["ExactMatchGrader", "Grade", "Grader", "LLMJudgeGrader", "make_grader"]
