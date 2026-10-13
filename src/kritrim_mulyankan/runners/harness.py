"""HarnessRunner: drives a coding/agent CLI headless in a sandbox folder. Not implemented yet."""

from __future__ import annotations

from kritrim_mulyankan.runners.base import RunContext, Runner, RunOutput
from kritrim_mulyankan.suite import Task


class HarnessRunner(Runner):
    """Spec: `harness:<cli>`, e.g. `harness:claude`, `harness:codex`, `harness:gemini`.

    Planned: run the CLI as a subprocess with cwd=ctx.workdir and a ctx.timeout_s kill, then
    capture stdout, the git diff of the workdir and any test results into RunOutput.raw.
    """

    kind = "harness"

    def __init__(self, cli: str) -> None:
        super().__init__(f"harness:{cli}")
        self.cli = cli

    def problems(self) -> list[str]:
        return ["HarnessRunner is not implemented yet"]

    def run(self, task: Task, prompt: str, ctx: RunContext) -> RunOutput:
        raise NotImplementedError("HarnessRunner is not implemented yet")
