"""Portable recorded-answer comparisons and an atomic, local configuration registry."""

from __future__ import annotations

import json
import os
import tempfile
import threading
import uuid
from datetime import datetime
from decimal import Decimal
from pathlib import Path
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, model_validator

from kritrim_mulyankan.app_observations import AppContext, AppObservation


class NamedConfiguration(BaseModel):
    model_config = ConfigDict(extra="forbid")
    id: str = Field(pattern=r"^[A-Za-z0-9._-]{1,80}$")
    name: str = Field(min_length=1, max_length=120)
    app_context: AppContext
    origin: Literal["template", "user_created", "imported"] = "user_created"


class NewConfiguration(BaseModel):
    model_config = ConfigDict(extra="forbid")
    name: str = Field(min_length=1, max_length=120)
    app_context: AppContext


class RecordedAnswer(BaseModel):
    model_config = ConfigDict(extra="forbid", allow_inf_nan=False)
    response: str = Field(max_length=30000)
    evidence_note: str = Field(min_length=8, max_length=4000)
    evidence_references: list[str] = Field(default_factory=list, max_length=10)
    completion_status: Literal["completed", "partial", "not_completed", "unknown"] = "unknown"
    elapsed_s: float | None = Field(None, ge=0)
    observed_at: datetime | None = None
    cost_amount: Decimal | None = Field(None, ge=0)
    cost_currency: str | None = Field(None, pattern=r"^[A-Z]{3}$")
    cost_source: str | None = Field(None, max_length=500)
    kind: Literal["contributor_reported", "test_fixture"] = "contributor_reported"
    exact_prompt_used: bool = False
    attested: bool = False


class ComparisonEntry(BaseModel):
    model_config = ConfigDict(extra="forbid")
    configuration: NamedConfiguration
    answer: RecordedAnswer

    def observation(self, bundle: ComparisonBundle) -> AppObservation:
        context = AppContext.model_validate(
            {
                **self.configuration.app_context.model_dump(mode="json"),
                "observed_latency_s": self.answer.elapsed_s,
                "observed_at": self.answer.observed_at,
                "observation_kind": self.answer.kind,
            }
        )
        return AppObservation(
            task_id=bundle.task_id,
            prompt_sha256=bundle.prompt_sha256,
            app_context=context,
            response=self.answer.response,
            evidence_note=self.answer.evidence_note,
            evidence_references=self.answer.evidence_references,
            configuration_name=self.configuration.name,
            configuration_registry_id=self.configuration.id,
            completion_status=self.answer.completion_status,
            observed_cost_amount=self.answer.cost_amount,
            observed_cost_currency=self.answer.cost_currency,
            observed_cost_source=self.answer.cost_source,
            exact_prompt_used=self.answer.exact_prompt_used,
            attested=self.answer.attested,
        )


class ComparisonBundle(BaseModel):
    model_config = ConfigDict(extra="forbid")
    schema_version: Literal["1.0"] = "1.0"
    kind: Literal["manual_comparison"] = "manual_comparison"
    suite: Literal["india-pilot"] = "india-pilot"
    task_id: str
    task_version: int = Field(ge=1)
    task_sha256: str = Field(pattern=r"^[a-f0-9]{64}$")
    prompt_sha256: str = Field(pattern=r"^[a-f0-9]{64}$")
    entries: list[ComparisonEntry] = Field(min_length=2, max_length=8)
    recorded_run_id: str | None = None

    @model_validator(mode="after")
    def _unique(self) -> ComparisonBundle:
        ids = [entry.configuration.id for entry in self.entries]
        if len(set(ids)) != len(ids):
            raise ValueError("select each configuration only once")
        # Validate all answers before any registry/run changes.
        for entry in self.entries:
            entry.observation(self)
        return self


class ConfigurationRegistry:
    def __init__(self, path: Path) -> None:
        self.path, self.lock = path, threading.Lock()

    def _read(self) -> list[NamedConfiguration]:
        if self.path.is_file():
            data = json.loads(self.path.read_text())
            if data.get("schema_version") != "1.0":
                raise ValueError("unsupported configuration registry version")
            entries = [NamedConfiguration.model_validate(item) for item in data["configurations"]]
            if len({item.id for item in entries}) != len(entries):
                raise ValueError("duplicate configuration IDs in registry")
            return entries
        return [
            NamedConfiguration(
                id=slug,
                name=f"{app} · Web · plan unknown",
                origin="template",
                app_context=AppContext(app=app, client="Web browser", plan="Unknown"),
            )
            for slug, app in [
                ("chatgpt-web", "ChatGPT"),
                ("claude-web", "Claude"),
                ("gemini-web", "Gemini"),
            ]
        ]

    def _write(self, entries: list[NamedConfiguration]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        payload = {
            "schema_version": "1.0",
            "configurations": [e.model_dump(mode="json") for e in entries],
        }
        descriptor, filename = tempfile.mkstemp(prefix=".configurations-", dir=self.path.parent)
        try:
            with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
                json.dump(payload, handle, ensure_ascii=False, indent=2)
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(filename, self.path)
        finally:
            Path(filename).unlink(missing_ok=True)

    def list(self) -> list[NamedConfiguration]:
        with self.lock:
            entries = self._read()
            if not self.path.exists():
                self._write(entries)
            return entries

    def create(self, request: NewConfiguration) -> NamedConfiguration:
        config = NamedConfiguration(
            id="config-" + uuid.uuid4().hex[:16], name=request.name, app_context=request.app_context
        )
        self.ensure([config], allow_new=True)
        return config

    def ensure(self, requested: list[NamedConfiguration], *, allow_new: bool = False) -> None:
        with self.lock:
            entries = self._read()
            by_id = {item.id: item for item in entries}
            for config in requested:
                if config.id in by_id:
                    if config != by_id[config.id]:
                        raise ValueError("configuration snapshot differs from saved configuration")
                elif not allow_new:
                    raise ValueError("unknown configuration; save it or import a comparison first")
                else:
                    entries.append(config)
                    by_id[config.id] = config
            self._write(entries)
