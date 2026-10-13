"""The common Runner interface every system under test is adapted to."""

from __future__ import annotations

from abc import ABC, abstractmethod
from dataclasses import dataclass
from pathlib import Path
from threading import Event
from typing import Any, ClassVar, Literal

from pydantic import BaseModel, Field

from kritrim_mulyankan import __version__
from kritrim_mulyankan.suite import Task

RunnerKind = Literal["fake", "model", "harness", "agent", "app"]


class Usage(BaseModel):
    input_tokens: int | None = None
    output_tokens: int | None = None
    cost_usd: float | None = Field(None, ge=0)


class RunOutput(BaseModel):
    """What a runner hands back. Latency is measured by the engine, not the runner."""

    output: str = ""
    usage: Usage = Field(default_factory=Usage)
    steps: int | None = Field(None, description="Agent/harness turns, if the runner knows them.")
    tool_calls: int | None = None
    error: str | None = None
    raw: dict[str, Any] = Field(default_factory=dict, description="Saved verbatim under runs/.")


@dataclass(frozen=True)
class RunContext:
    workdir: Path
    """Fresh temp directory with the task's fixtures copied in; deleted after the attempt."""
    seed: int
    timeout_s: float
    cancel_event: Event | None = None


class Runner(ABC):
    kind: ClassVar[RunnerKind]

    def __init__(self, name: str) -> None:
        self.name = name

    @abstractmethod
    def run(self, task: Task, prompt: str, ctx: RunContext) -> RunOutput:
        """Attempt one task. Raise or set RunOutput.error on failure; enforce ctx.timeout_s."""

    def estimate_cost_usd(self, task: Task, prompt: str) -> float | None:
        """Worst-case cost of one attempt, used by dry runs and the budget cap.

        None means unknown, and the run is refused. It must be an upper bound, or the cap can be
        exceeded.
        """
        return None

    def problems(self) -> list[str]:
        """Reasons this runner cannot run right now (missing key, no price, ...). Checked first."""
        return []

    def __repr__(self) -> str:
        return f"{type(self).__name__}({self.name!r})"

    def configuration(self) -> dict[str, Any]:
        return {
            "runner_id": self.name,
            "kind": self.kind,
            "evaluation_mode": "mock" if self.kind == "fake" else "live",
            "framework": "kritrim_mulyankan",
            "framework_version": __version__,
            "evaluation_track": {
                "fake": "mock_validation",
                "model": "model_api",
                "harness": "harness",
                "agent": "personal_agent",
                "app": "consumer_app",
            }[self.kind],
            "tools": [],
            "system_prompt": None,
        }


def approx_tokens(text: str) -> int:
    """Rough token count (about 4 characters per token) for runners without a tokenizer."""
    return max(1, len(text) // 4) if text else 0
