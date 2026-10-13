"""Offline runners that cost nothing: for demos, tests and sanity-checking graders."""

from __future__ import annotations

import json

from kritrim_mulyankan.runners.base import RunContext, Runner, RunOutput, Usage, approx_tokens
from kritrim_mulyankan.suite import Task


class EchoRunner(Runner):
    """Replies with the prompt itself. A floor: it should fail every exact-match task."""

    kind = "fake"

    def __init__(self) -> None:
        super().__init__("echo")

    def run(self, task: Task, prompt: str, ctx: RunContext) -> RunOutput:
        return _free_output(prompt, prompt)

    def estimate_cost_usd(self, task: Task, prompt: str) -> float:
        return 0.0


class OracleRunner(Runner):
    """Replies with the task's expected or reference answer. A ceiling that checks the graders."""

    kind = "fake"

    def __init__(self) -> None:
        super().__init__("oracle")

    def run(self, task: Task, prompt: str, ctx: RunContext) -> RunOutput:
        spec = task.grader
        if spec.type == "exact":
            answer = spec.expected[0]
        elif spec.type == "json":
            answer = json.dumps(spec.expected, ensure_ascii=False)
        elif spec.type == "numeric":
            answer = str(spec.expected)
        else:
            answer = task.reference or ""
        return _free_output(prompt, answer)

    def estimate_cost_usd(self, task: Task, prompt: str) -> float:
        return 0.0


def _free_output(prompt: str, answer: str) -> RunOutput:
    return RunOutput(
        output=answer,
        usage=Usage(
            input_tokens=approx_tokens(prompt), output_tokens=approx_tokens(answer), cost_usd=0
        ),
        steps=1,
        tool_calls=0,
    )
