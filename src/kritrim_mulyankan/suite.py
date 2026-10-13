"""Task suite schema (Pydantic) and the YAML loader."""

from __future__ import annotations

import hashlib
import json
import re
from collections import Counter
from datetime import date
from decimal import Decimal
from pathlib import Path
from typing import Annotated, Any, Literal

import yaml
from pydantic import BaseModel, ConfigDict, Field, JsonValue, field_validator, model_validator

Category = str  # Preserve source taxonomies; capability_track determines execution support.

_PLACEHOLDER = re.compile(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*)\s*\}\}")


class _Strict(BaseModel):
    model_config = ConfigDict(extra="forbid", frozen=True, allow_inf_nan=False)


class ExactGraderSpec(_Strict):
    type: Literal["exact"] = "exact"
    expected: list[str] = Field(min_length=1, description="Any of these answers passes.")
    case_sensitive: bool = False
    normalize_whitespace: bool = True

    @field_validator("expected", mode="before")
    @classmethod
    def _one_or_many(cls, v: Any) -> Any:
        return [v] if isinstance(v, str) else v


class LLMJudgeGraderSpec(_Strict):
    type: Literal["llm_judge"] = "llm_judge"
    mode: Literal["absolute", "pairwise"] = "absolute"
    judge_model: str | None = Field(None, description="Overrides the configured judge model.")
    pass_threshold: int = Field(7, ge=1, le=10, description="Absolute scores >= this pass.")


class JSONGraderSpec(_Strict):
    type: Literal["json"] = "json"
    expected: JsonValue
    mode: Literal["exact"] = "exact"


class NumericGraderSpec(_Strict):
    type: Literal["numeric"] = "numeric"
    expected: Decimal
    tolerance: Decimal = Field(Decimal("0"), ge=0)


class RegexGraderSpec(_Strict):
    type: Literal["regex"] = "regex"
    pattern: str
    case_sensitive: bool = True

    @field_validator("pattern")
    @classmethod
    def _regex(cls, value: str) -> str:
        re.compile(value)
        return value


class WorkflowGraderSpec(_Strict):
    type: Literal["workflow"] = "workflow"
    fixture: Path
    expected: dict[str, JsonValue]


GraderSpec = Annotated[
    ExactGraderSpec
    | LLMJudgeGraderSpec
    | JSONGraderSpec
    | NumericGraderSpec
    | RegexGraderSpec
    | WorkflowGraderSpec,
    Field(discriminator="type"),
]


class Task(_Strict):
    id: str = Field(pattern=r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
    version: int = Field(1, ge=1, description="Bump whenever the task changes meaning.")
    category: Category = Field(min_length=1)
    capability_track_version: str = "1.0"
    capability_track: Literal[
        "text",
        "browser_workflow",
        "image_understanding",
        "image_generation",
        "asr",
        "tts",
        "video_analysis",
        "video_generation",
    ] = "text"
    language: str = "und"
    script: str | None = None
    input_style: str | None = None
    domain: str | None = None
    as_of: date | None = None
    provenance: list[dict[str, Any] | str] = Field(default_factory=list)
    risk_flags: list[str] = Field(default_factory=list)
    context: dict[str, JsonValue] | str = Field(default_factory=dict)
    prompt: str = Field(min_length=1, description="May reference inputs as {{ name }}.")
    inputs: dict[str, str] = Field(default_factory=dict)
    fixtures: list[Path] = Field(
        default_factory=list, description="Files copied into the runner's workdir."
    )
    grader: GraderSpec
    rubric: str | None = None
    reference: str | None = Field(None, description="Reference answer, shown to judges.")
    max_output_tokens: int = Field(
        4096, ge=1, le=128_000, description="Output ceiling (thinking included); bounds cost."
    )
    timeout_s: float = Field(120, gt=0)
    budget_usd: float | None = Field(None, ge=0, description="Max estimated cost per attempt.")

    @model_validator(mode="after")
    def _check(self) -> Task:
        if any(p.is_absolute() or ".." in p.parts for p in self.fixtures):
            raise ValueError("fixtures must be relative paths inside the suite directory")
        if self.grader.type == "workflow" and self.grader.fixture not in self.fixtures:
            raise ValueError("workflow grader fixture must be listed in task fixtures")
        missing = set(_PLACEHOLDER.findall(self.prompt)) - set(self.inputs)
        if missing:
            raise ValueError(f"prompt references unknown inputs: {sorted(missing)}")
        if self.grader.type == "llm_judge" and not self.rubric:
            raise ValueError("llm_judge tasks need a rubric")
        return self

    def render_prompt(self) -> str:
        return _PLACEHOLDER.sub(lambda m: self.inputs[m.group(1)], self.prompt)

    def content_hash(self) -> str:
        """Stable hash of the task definition; catches edits made without a version bump."""
        payload = json.dumps(self.model_dump(mode="json"), sort_keys=True, ensure_ascii=False)
        return hashlib.sha256(payload.encode()).hexdigest()


class Suite(_Strict):
    name: str
    version: str
    description: str = ""
    validation_status: Literal["smoke", "unvalidated_pilot", "validated"] = "unvalidated_pilot"
    tasks: list[Task] = Field(min_length=1)
    path: Path | None = Field(None, exclude=True)

    @field_validator("version", mode="before")
    @classmethod
    def _version_as_str(cls, v: Any) -> Any:
        return str(v) if isinstance(v, int | float) else v

    @model_validator(mode="after")
    def _unique_ids(self) -> Suite:
        dupes = [tid for tid, n in Counter(t.id for t in self.tasks).items() if n > 1]
        if dupes:
            raise ValueError(f"duplicate task ids: {sorted(dupes)}")
        return self

    def select(self, task_ids: list[str] | None) -> list[Task]:
        if not task_ids:
            return list(self.tasks)
        if len(task_ids) != len(set(task_ids)):
            raise ValueError("duplicate selected task ids")
        by_id = {t.id: t for t in self.tasks}
        unknown = [t for t in task_ids if t not in by_id]
        if unknown:
            raise ValueError(f"unknown task ids in suite {self.name!r}: {unknown}")
        return [by_id[t] for t in task_ids]


def load_suite(path: Path) -> Suite:
    data = yaml.safe_load(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise ValueError(f"{path}: expected a YAML mapping at the top level")
    suite = Suite.model_validate({**data, "path": path})
    for task in suite.tasks:
        for fixture in task.fixtures:
            if not (path.parent / fixture).resolve().is_relative_to(path.parent.resolve()):
                raise ValueError(f"fixture escapes suite directory: {fixture}")
            if not (path.parent / fixture).is_file():
                raise ValueError(f"{path}: task {task.id!r} fixture not found: {fixture}")
    return suite


def resolve_suite(name_or_path: str, suites_dir: Path) -> Path:
    """Accept a path to a YAML file or a bare suite name found in suites_dir.

    `suites/example.yaml` and `example.yaml` also resolve against suites_dir,
    so the installed-file spelling works from a folder with no suites/ in it
    (for example right after installing `km` into ~/.local/bin).
    """
    candidate = Path(name_or_path)
    if candidate.is_file():
        return candidate
    for ext in (".yaml", ".yml"):
        named = suites_dir / f"{name_or_path}{ext}"
        if named.is_file():
            return named
    in_suites_dir = suites_dir / candidate.name
    if in_suites_dir.is_file():
        return in_suites_dir
    raise FileNotFoundError(f"no suite {name_or_path!r} (looked for a file and in {suites_dir}/)")


def discover_suites(suites_dir: Path) -> list[Path]:
    if not suites_dir.is_dir():
        return []
    return sorted([*suites_dir.glob("*.yaml"), *suites_dir.glob("*.yml")])
