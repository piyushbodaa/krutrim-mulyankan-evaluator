"""Import original JSON/JSONL items into the existing versioned YAML suite format."""

from __future__ import annotations

import json
from datetime import date
from pathlib import Path
from typing import Any, Literal

import yaml
from pydantic import BaseModel, ConfigDict, Field, JsonValue

from kritrim_mulyankan.graders.deterministic import strict_json
from kritrim_mulyankan.suite import Category, Suite, Task


class ImportItem(BaseModel):
    model_config = ConfigDict(extra="forbid")

    id: str
    version: int = 1
    category: Category
    language: str
    script: str | None = None
    input_style: str | None = None
    domain: str | None = None
    prompt: str
    context: dict[str, JsonValue] | str = Field(default_factory=dict)
    expected_json: JsonValue
    grading: dict[str, Any] = Field(default_factory=lambda: {"type": "json", "mode": "exact"})
    as_of: date | None = None
    provenance: list[dict[str, Any] | str] = Field(default_factory=list)
    risk_flags: list[str] = Field(default_factory=list)

    def task(self) -> Task:
        data = self.model_dump(mode="json", exclude={"grading", "expected_json"})
        grading = dict(self.grading)
        if grading.get("type", "json") != "json":
            raise ValueError(f"{self.id}: external expected_json requires a json grader")
        grading.setdefault("type", "json")
        grading["expected"] = self.expected_json
        inputs = (
            {
                k: v if isinstance(v, str) else json.dumps(v, ensure_ascii=False)
                for k, v in self.context.items()
            }
            if isinstance(self.context, dict)
            else {}
        )
        prompt = self.prompt
        # Keep contextual documents visible even when the author did not use placeholders.
        if self.context:
            prompt += "\n\nContext:\n" + (
                self.context
                if isinstance(self.context, str)
                else json.dumps(self.context, ensure_ascii=False, sort_keys=True)
            )
        return Task.model_validate(
            {
                **data,
                "prompt": prompt,
                "inputs": inputs,
                "grader": grading,
                "max_output_tokens": 512,
            }
        )


class ImportDataset(BaseModel):
    model_config = ConfigDict(extra="forbid")

    schema_version: Literal["1.0"] = "1.0"
    name: str
    version: str
    description: str = "Original deterministic items; unvalidated pilot."
    items: list[ImportItem] = Field(min_length=1)


def import_suite(source: Path, destination: Path, *, name: str, version: str) -> Suite:
    text = source.read_text(encoding="utf-8")
    if source.suffix == ".jsonl":
        data = [
            strict_json(line, decimal_numbers=False) for line in text.splitlines() if line.strip()
        ]
    else:
        data = strict_json(text, decimal_numbers=False)
    if isinstance(data, list):
        data = {"name": name, "version": version, "items": data}
    dataset = ImportDataset.model_validate(data)
    suite = Suite(
        name=dataset.name,
        version=dataset.version,
        description=dataset.description,
        tasks=[item.task() for item in dataset.items],
    )
    # Exclusive creation avoids overwriting user-authored suites.
    with destination.open("x", encoding="utf-8") as handle:
        yaml.safe_dump(suite.model_dump(mode="json"), handle, allow_unicode=True, sort_keys=False)
    return suite
