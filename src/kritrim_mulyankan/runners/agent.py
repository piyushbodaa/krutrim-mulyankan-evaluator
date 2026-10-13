"""AgentRunner: personal assistants judged on life-admin tasks. Not implemented yet."""

from __future__ import annotations

from kritrim_mulyankan.runners.base import RunContext, Runner, RunOutput
from kritrim_mulyankan.suite import Task


class AgentRunner(Runner):
    """Spec: `agent:<name>`, e.g. `agent:grok-bot`, `agent:hermes`.

    Planned: first import manually recorded transcripts (one per task id), then add live
    adapters per assistant.
    """

    kind = "agent"

    def __init__(self, agent: str) -> None:
        super().__init__(f"agent:{agent}")
        self.agent = agent

    def problems(self) -> list[str]:
        return ["AgentRunner is not implemented yet"]

    def run(self, task: Task, prompt: str, ctx: RunContext) -> RunOutput:
        raise NotImplementedError("AgentRunner is not implemented yet")
