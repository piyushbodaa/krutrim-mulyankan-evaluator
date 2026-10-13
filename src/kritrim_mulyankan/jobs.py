"""Versioned local jobs. UI and agents use one bounded engine execution protocol."""

from __future__ import annotations

import os
import re
import shutil
import threading
import uuid
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from datetime import UTC, datetime
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, model_validator

from kritrim_mulyankan.engine import preflight, run_suite
from kritrim_mulyankan.export import explanation_key, result_export, write_export
from kritrim_mulyankan.provenance import redact
from kritrim_mulyankan.runners.base import Runner
from kritrim_mulyankan.runners.claude_draft import ClaudeDraftRunner
from kritrim_mulyankan.runners.fake import EchoRunner, OracleRunner
from kritrim_mulyankan.settings import Settings
from kritrim_mulyankan.store import Store
from kritrim_mulyankan.suite import Suite

TERMINAL = {"completed", "failed", "cancelled", "interrupted"}


def now() -> str:
    return datetime.now(UTC).isoformat()


class JobRequest(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True, allow_inf_nan=False)
    api_version: Literal["1.0"] = "1.0"
    suite: Literal["india-pilot", "railway-pilot"] = "india-pilot"
    task_ids: list[str] = Field(min_length=1, max_length=24)
    adapter_ids: list[str] = Field(min_length=1, max_length=4)
    seed: int = Field(12, ge=0, le=2**31 - 1)
    repeats: int = Field(1, ge=1, le=5, description="Tries per task and adapter")
    idempotency_key: str = Field(pattern=r"^[A-Za-z0-9_-]{8,100}$")

    @model_validator(mode="after")
    def unique_selections(self):
        if len(set(self.task_ids)) != len(self.task_ids) or len(set(self.adapter_ids)) != len(
            self.adapter_ids
        ):
            raise ValueError("task and adapter selections must be unique")
        return self


class JobEvent(BaseModel):
    at: str
    stage: str
    task_id: str | None = None
    runner_id: str | None = None
    repeat_index: int | None = None
    status: str | None = None
    grade_status: str | None = None
    passed: bool | None = None


class JobState(BaseModel):
    model_config = ConfigDict(extra="forbid")
    api_version: Literal["1.0"] = "1.0"
    id: str
    request: JobRequest
    status: Literal[
        "queued", "running", "cancelling", "completed", "failed", "cancelled", "interrupted"
    ]
    created_at: str
    updated_at: str
    started_at: str | None = None
    finished_at: str | None = None
    run_id: str | None = None
    parent_job_id: str | None = None
    total: int
    processed: int = 0
    execution_errors: int = 0
    grader_errors: int = 0
    pending: int = 0
    graded: int = 0
    passed: int = 0
    skipped: int = 0
    error: str | None = None
    events: list[JobEvent] = Field(default_factory=list)
    result_url: str | None = None
    private_result_url: str | None = None


class JobManager:
    def __init__(
        self,
        suites: dict[str, Suite],
        settings: Settings,
        factories: dict[str, Callable[[], Runner]] | None = None,
    ):
        self.suites, self.settings = suites, settings
        self.directory = settings.runs_dir / "jobs"
        self.directory.mkdir(parents=True, exist_ok=True)
        self.factories = factories or {
            "mock:oracle": OracleRunner,
            "mock:echo": EchoRunner,
            "harness:claude-draft": lambda: ClaudeDraftRunner(
                settings.runs_dir / "automation-access.json"
            ),
        }
        self.lock = threading.RLock()
        self.states: dict[str, JobState] = {}
        self.cancellations: dict[str, threading.Event] = {}
        self.executor = ThreadPoolExecutor(max_workers=1, thread_name_prefix="km-job")
        for path in self.directory.glob("*/job.json"):
            state = JobState.model_validate_json(path.read_text())
            self.states[state.id] = state
            if state.status not in TERMINAL:
                state.status = "interrupted"
                state.error = "Server stopped before completion. No automatic retry was made."
                state.finished_at = now()
                self._save(state)

    def _save(self, state: JobState) -> None:
        state.updated_at = now()
        directory = self.directory / state.id
        directory.mkdir(exist_ok=True)
        target = directory / "job.json"
        temporary = target.with_suffix(".tmp")
        with temporary.open("w") as stream:
            stream.write(state.model_dump_json(indent=2) + "\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, target)

    def capabilities(self) -> dict:
        adapters = []
        for identity, title, track in [
            ("mock:oracle", "Correct-answer control", "mock_validation"),
            ("mock:echo", "Echo control", "mock_validation"),
            ("harness:claude-draft", "Claude Code · tool-free draft", "harness"),
        ]:
            if identity not in self.factories:
                continue
            problems = self.factories[identity]().problems()
            adapters.append(
                {
                    "id": identity,
                    "name": title,
                    "track": track,
                    "ready": not problems,
                    "mode": "mock" if track == "mock_validation" else "live",
                    "reason": "; ".join(problems) if problems else "Ready",
                    "max_tasks": 3 if track == "harness" else 24,
                    "cost": "Known zero API cost"
                    if track == "mock_validation"
                    else "Included-plan verified; actual charge unknown",
                }
            )
        for provider in ("anthropic", "openai", "gemini", "xai", "openrouter", "compatible"):
            adapters.append(
                {
                    "id": "provider:" + provider,
                    "name": provider + " API",
                    "track": "model_api",
                    "ready": False,
                    "mode": "live",
                    "reason": "No approved paid budget; configured access/model/pricing unverified",
                    "cost": "Unknown",
                }
            )
        for runtime in ("ollama", "vllm"):
            adapters.append(
                {
                    "id": "local:" + runtime,
                    "name": runtime + " local weights",
                    "track": "model_api",
                    "ready": False,
                    "mode": "live",
                    "reason": "No running local server/loaded model verified",
                    "cost": "Hardware/electricity excluded",
                }
            )
        for cli in ("codex", "antigravity"):
            adapters.append(
                {
                    "id": "harness:" + cli,
                    "name": cli + " CLI",
                    "track": "harness",
                    "installed": bool(shutil.which(cli)),
                    "ready": False,
                    "mode": "live",
                    "reason": "No-spend and tool boundaries need verification before enablement",
                    "cost": "Unknown",
                }
            )
        adapters.append(
            {
                "id": "consumer:manual",
                "name": "Consumer apps · recorded answers",
                "track": "consumer_app",
                "ready": False,
                "mode": "manual_app",
                "reason": "Record answers manually; browser automation is not connected",
                "cost": "Unknown",
            }
        )
        return {
            "api_version": "1.0",
            "adapters": adapters,
            "execution_policy": "One active job; explicit retries; paid APIs disabled",
            "request_schema_url": "/api/v1/schemas/job-request",
            "state_schema_url": "/api/v1/schemas/job-state",
        }

    def preview(self, request: JobRequest) -> tuple[dict, list[Runner]]:
        with self.lock:
            return self._preview(request)

    def _preview(self, request: JobRequest) -> tuple[dict, list[Runner]]:
        suite = self.suites[request.suite]
        tasks = suite.select(request.task_ids)
        if any(identity not in self.factories for identity in request.adapter_ids):
            raise ValueError("Unsupported or gated adapter; inspect /api/v1/capabilities")
        if "harness:claude-draft" in request.adapter_ids:
            if request.suite != "india-pilot" or len(tasks) > 3:
                raise ValueError("Claude draft is limited to 1–3 frozen synthetic text tasks")
            submitted = sum(
                len(s.request.task_ids) * s.request.repeats
                for s in self.states.values()
                if "harness:claude-draft" in s.request.adapter_ids
                and (datetime.now(UTC) - datetime.fromisoformat(s.created_at)).total_seconds()
                < 3600
            )
            if submitted + len(tasks) * request.repeats > 8:
                raise ValueError("Bounded CLI allowance reached: maximum 8 submitted attempts/hour")
        runners = [self.factories[identity]() for identity in request.adapter_ids]
        check = preflight(tasks, runners, 0, repeats=request.repeats)
        if not check.ok:
            raise ValueError("; ".join(check.problems))
        return {
            "api_version": "1.0",
            "attempts": len(tasks) * len(runners) * request.repeats,
            "repeats": request.repeats,
            "paid_api_budget_usd": 0,
            "actual_cost_known": all(r.kind == "fake" for r in runners),
            "modes": sorted({r.configuration()["evaluation_mode"] for r in runners}),
            "warnings": check.warnings,
            "tasks": [{"id": t.id, "sha256": t.content_hash()} for t in tasks],
            "configurations": [r.configuration() for r in runners],
        }, runners

    def create(self, request: JobRequest, *, parent_job_id: str | None = None) -> dict:
        with self.lock:
            for state in self.states.values():
                if state.request.idempotency_key == request.idempotency_key:
                    if state.request != request:
                        raise ValueError("Idempotency key already has a different request")
                    return state.model_dump(mode="json")
            if sum(s.status not in TERMINAL for s in self.states.values()) >= 4:
                raise RuntimeError("Local queue is full; wait for a job or cancel one")
            _, runners = self.preview(request)
            identity = uuid.uuid4().hex
            state = JobState(
                id=identity,
                request=request,
                status="queued",
                created_at=now(),
                updated_at=now(),
                total=len(request.task_ids) * len(runners) * request.repeats,
                parent_job_id=parent_job_id,
            )
            self.states[identity] = state
            self.cancellations[identity] = threading.Event()
            self._save(state)
            self.executor.submit(self._execute, identity, runners)
            return state.model_dump(mode="json")

    def get(self, identity: str) -> dict:
        if not re.fullmatch(r"[a-f0-9]{32}", identity):
            raise ValueError("unknown job")
        with self.lock:
            if identity not in self.states:
                raise ValueError("unknown job")
            return self.states[identity].model_dump(mode="json")

    def list(self) -> list[dict]:
        with self.lock:
            return [
                s.model_dump(mode="json")
                for s in sorted(self.states.values(), key=lambda s: s.created_at, reverse=True)
            ][:50]

    def cancel(self, identity: str) -> dict:
        self.get(identity)
        with self.lock:
            state = self.states[identity]
            if state.status not in TERMINAL:
                self.cancellations[identity].set()
                state.status = "cancelling"
                self._save(state)
            return state.model_dump(mode="json")

    def retry(self, identity: str, key: str) -> dict:
        self.get(identity)
        with self.lock:
            state = self.states[identity]
            if state.status not in TERMINAL:
                raise ValueError("Wait for terminal status before retrying")
            request = state.request.model_copy(update={"idempotency_key": key})
            request = JobRequest.model_validate(request.model_dump())
            return self.create(request, parent_job_id=identity)

    def result(self, identity: str, *, outputs: bool = False) -> dict:
        state = self.get(identity)
        if not state["run_id"]:
            raise ValueError("No evaluator run exists yet")
        with Store(self.settings.db_path) as store:
            payload = result_export(store, state["run_id"], include_outputs=outputs).model_dump(
                mode="json"
            )
            if outputs:
                payload["explanations"] = {
                    explanation_key(r): r.grade_details
                    for r in store.results_for_run(state["run_id"])
                }
            return payload

    def _execute(self, identity: str, runners: list[Runner]) -> None:
        state = self.states[identity]
        cancel = self.cancellations[identity]
        with self.lock:
            if cancel.is_set():
                state.status, state.finished_at = "cancelled", now()
                self._save(state)
                return
            state.status, state.started_at = "running", now()
            self._save(state)

        def started(run_id):
            with self.lock:
                state.run_id = run_id
                state.result_url = f"/api/v1/jobs/{identity}/export"
                state.private_result_url = f"/api/v1/jobs/{identity}/results"
                self._save(state)

        def attempting(task_id, runner_id):
            with self.lock:
                state.events.append(
                    JobEvent(at=now(), stage="executing", task_id=task_id, runner_id=runner_id)
                )
                self._save(state)

        def recorded(record):
            with self.lock:
                state.processed += 1
                state.execution_errors += record.status == "error"
                state.grader_errors += record.grade_status == "error"
                state.pending += record.grade_status == "pending"
                state.skipped += record.status == "skipped"
                state.graded += record.grade_status == "graded"
                state.passed += record.passed is True
                state.events.append(
                    JobEvent(
                        at=now(),
                        stage="persisted",
                        task_id=record.task_id,
                        runner_id=record.runner,
                        repeat_index=record.repeat_index,
                        status=record.status,
                        grade_status=record.grade_status,
                        passed=record.passed,
                    )
                )
                self._save(state)

        try:
            with Store(self.settings.db_path) as store:
                summary = run_suite(
                    self.suites[state.request.suite],
                    runners,
                    store=store,
                    settings=self.settings,
                    seed=state.request.seed,
                    repeats=state.request.repeats,
                    max_usd=0,
                    task_ids=state.request.task_ids,
                    on_start=started,
                    on_attempt=attempting,
                    on_result=recorded,
                    cancel_event=cancel,
                )
                write_export(store, summary.run_id, summary.run_dir / "results.json")
            with self.lock:
                state.status = (
                    "cancelled"
                    if summary.status == "cancelled"
                    else ("failed" if state.execution_errors else "completed")
                )
                state.finished_at = now()
                self._save(state)
        except Exception as exc:
            with self.lock:
                state.status, state.finished_at = "failed", now()
                state.error = redact(f"{type(exc).__name__}: {exc}")[:250]
                self._save(state)

    def close(self):
        with self.lock:
            for identity, event in self.cancellations.items():
                if self.states[identity].status not in TERMINAL:
                    event.set()
        self.executor.shutdown(wait=True, cancel_futures=False)
