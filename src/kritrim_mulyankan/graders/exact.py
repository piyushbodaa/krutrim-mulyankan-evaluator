"""Exact-match grader with light, configurable normalisation."""

from __future__ import annotations

from kritrim_mulyankan.graders.base import Grade, Grader
from kritrim_mulyankan.suite import ExactGraderSpec, Task


class ExactMatchGrader(Grader):
    name = "exact"

    def __init__(self, spec: ExactGraderSpec) -> None:
        self.spec = spec

    def normalize(self, text: str) -> str:
        text = " ".join(text.split()) if self.spec.normalize_whitespace else text.strip()
        return text if self.spec.case_sensitive else text.casefold()

    def grade(self, task: Task, output: str) -> Grade:
        got = self.normalize(output)
        matched = next((e for e in self.spec.expected if self.normalize(e) == got), None)
        return Grade(
            grader=self.name,
            status="graded",
            passed=matched is not None,
            score=1.0 if matched is not None else 0.0,
            details={"matched": matched, "normalized_output": got[:500]},
        )
