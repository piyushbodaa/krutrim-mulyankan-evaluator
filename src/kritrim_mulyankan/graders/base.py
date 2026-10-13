"""The Grader interface and the Grade it produces."""

from __future__ import annotations

from abc import ABC, abstractmethod
from typing import Any, Literal

from pydantic import BaseModel, Field

from kritrim_mulyankan.suite import Task


class Grade(BaseModel):
    grader: str
    status: Literal["graded", "pending", "error"]
    passed: bool | None = None
    score: float | None = Field(None, ge=0, le=1, description="Normalised to 0..1.")
    details: dict[str, Any] = Field(default_factory=dict)


class Grader(ABC):
    name: str

    @abstractmethod
    def grade(self, task: Task, output: str) -> Grade: ...
