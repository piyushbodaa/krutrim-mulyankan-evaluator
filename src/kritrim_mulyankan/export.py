"""Versioned website handoff. Raw responses remain private unless explicitly requested."""

from __future__ import annotations

import hashlib
import json
from collections import defaultdict
from pathlib import Path
from statistics import mean
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, model_validator

from kritrim_mulyankan.app_observations import AppContext
from kritrim_mulyankan.metrics import wilson_interval
from kritrim_mulyankan.provenance import redact, sha256_file
from kritrim_mulyankan.store import Store


class BenchmarkMetadata(BaseModel):
    name: str
    release: str
    validation_status: str
    suite_sha256: str | None


class RunMetadata(BaseModel):
    id: str
    started_at: str
    finished_at: str | None
    status: str
    evaluation_mode: Literal["mock", "live", "mixed", "unverified", "manual_app"]
    seed: int
    software_version: str
    source: dict[str, Any] | None
    max_usd: float
    cost_usd: float | None
    known_spent_usd: float
    reserved_usd: float


class TestedConfiguration(BaseModel):
    model_config = ConfigDict(extra="allow")
    runner_id: str
    configuration_id: str
    kind: str | None = None
    evaluation_mode: Literal["mock", "live", "unverified", "manual_app"]
    evaluation_track: str = "unverified"
    app_context: AppContext | None = None
    verification_status: str | None = None
    configuration_name: str | None = None
    configuration_registry_id: str | None = None
    provider: str | None = None
    requested_model: str | None = None
    framework: str | None = None
    framework_version: str | None = None
    tools: list[str] = []
    system_prompt: str | None = None
    options: dict[str, Any] = {}
    weights_revision: str | None = None
    tokenizer_revision: str | None = None
    weights_license: str | None = None
    quantization: str | None = None
    hardware: str | None = None
    chat_template_sha256: str | None = None

    @model_validator(mode="after")
    def _separate_app_track(self):
        if self.evaluation_track == "model_api" and self.app_context is not None:
            raise ValueError("a model API configuration cannot evaluate a consumer-app plan")
        return self


class Aggregate(BaseModel):
    runner_id: str
    configuration_id: str
    capability_track: str
    language: str
    domain: str | None
    evaluation_mode: str
    eligible_for_model_ranking: bool
    evaluation_track: str = "unverified"
    eligible_for_app_ranking: bool = False
    sample_count: int
    graded_count: int
    passed_count: int
    error_count: int
    skipped_count: int
    pending_count: int
    grader_error_count: int
    success_rate: float | None
    success_ci_95: tuple[float, float] | None
    mean_score: float | None
    mean_latency_s: float | None
    cost_usd: float | None
    small_sample: bool


class ExportResult(BaseModel):
    model_config = ConfigDict(extra="forbid")
    task_id: str
    task_version: int
    task_sha256: str
    runner_id: str
    configuration_id: str
    capability_track: str
    capability_track_version: str = "1.0"
    category: str
    language: str
    domain: str | None
    evaluation_mode: Literal["mock", "live", "unverified", "manual_app"]
    evaluation_track: str = "unverified"
    app_context: AppContext | None = None
    verification_status: str | None = None
    exact_prompt_used: bool | None = None
    evidence_note: str | None = None
    evidence_references: list[str] | None = None
    completion_status: str | None = None
    configuration_name: str | None = None
    configuration_registry_id: str | None = None
    resolved_model: str | None = None
    system_fingerprint: str | None = None
    status: Literal["ok", "error", "skipped"]
    grade_status: Literal["graded", "pending", "error"] | None
    passed: bool | None
    score: float | None
    latency_s: float | None
    attempt_started_at: str | None = None
    attempt_finished_at: str | None = None
    latency_scope: str | None = None
    usage: dict[str, int | None]
    cost: dict[str, Any]
    error: str | None
    artifact_reference: dict[str, str] | None
    output: str | None = None


class ResultExport(BaseModel):
    model_config = ConfigDict(extra="forbid")
    schema_version: Literal["1.0"] = "1.0"
    benchmark: BenchmarkMetadata
    run: RunMetadata
    models: list[TestedConfiguration]
    results: list[ExportResult]
    aggregates: list[Aggregate]
    supported_tracks: dict[str, str]
    limitations: list[str]


def result_export(store: Store, run_id: str, *, include_outputs: bool = False) -> ResultExport:
    run = store.get_run(run_id)
    if run is None:
        raise KeyError(f"no run {run_id!r}")
    directory = Path(run.artifacts_dir)
    manifest_path = directory / "manifest.json"
    manifest = json.loads(manifest_path.read_text()) if manifest_path.is_file() else {}
    tasks = {item["id"]: item for item in manifest.get("tasks", [])}
    configurations = {item["spec"]: item for item in manifest.get("runners", [])}
    models = []
    for runner_id in run.runners:
        config = dict(configurations.get(runner_id, {"runner_id": runner_id}))
        config.pop("spec", None)
        config.setdefault("evaluation_mode", "unverified")
        config["configuration_id"] = hashlib.sha256(
            json.dumps(
                {
                    "configuration": config,
                    "source": manifest.get("source"),
                    "budget_usd": run.max_usd,
                },
                sort_keys=True,
            ).encode()
        ).hexdigest()
        models.append(config)
    by_runner = {model["runner_id"]: model for model in models}
    results = []
    for record in store.results_for_run(run_id):
        metadata = tasks.get(record.task_id, {}).get("metadata", {})
        config = by_runner[record.runner]
        artifact = Path(record.artifact_path) if record.artifact_path else None
        reference = None
        raw = {}
        if (
            artifact
            and artifact.is_file()
            and artifact.resolve().is_relative_to(directory.resolve())
        ):
            reference = {
                "path": str(artifact.relative_to(directory)),
                "sha256": sha256_file(artifact),
            }
            raw = json.loads(artifact.read_text()).get("result", {}).get("raw", {})
        results.append(
            ExportResult(
                task_id=record.task_id,
                task_version=record.task_version,
                task_sha256=record.task_hash,
                runner_id=record.runner,
                configuration_id=config["configuration_id"],
                capability_track=metadata.get("capability_track", "text"),
                capability_track_version=metadata.get("capability_track_version", "1.0"),
                category=record.category,
                language=metadata.get("language", "und"),
                domain=metadata.get("domain"),
                evaluation_mode=config["evaluation_mode"],
                evaluation_track=config.get("evaluation_track", "unverified"),
                app_context=config.get("app_context"),
                verification_status=config.get("verification_status"),
                exact_prompt_used=config.get("exact_prompt_used"),
                evidence_note=raw.get("observation", {}).get("evidence_note")
                if include_outputs
                else None,
                evidence_references=raw.get("observation", {}).get("evidence_references")
                if include_outputs
                else None,
                completion_status=raw.get("observation", {}).get("completion_status"),
                configuration_name=config.get("configuration_name"),
                configuration_registry_id=config.get("configuration_registry_id"),
                status=record.status,
                resolved_model=raw.get("resolved_model"),
                system_fingerprint=raw.get("response", {}).get("system_fingerprint"),
                grade_status=record.grade_status,
                passed=record.passed,
                score=record.human_score if record.human_score is not None else record.score,
                latency_s=record.latency_s,
                attempt_started_at=raw.get("attempt_timing", {}).get("started_at"),
                attempt_finished_at=raw.get("attempt_timing", {}).get("finished_at"),
                latency_scope=raw.get("attempt_timing", {}).get("scope"),
                usage={
                    "input_tokens": record.input_tokens,
                    "output_tokens": record.output_tokens,
                    **(
                        {
                            "cache_read_input_tokens": raw.get("response", {})
                            .get("usage", {})
                            .get("cache_read_input_tokens"),
                            "cache_creation_input_tokens": raw.get("response", {})
                            .get("usage", {})
                            .get("cache_creation_input_tokens"),
                        }
                        if config.get("evaluation_track") == "harness"
                        else {}
                    ),
                },
                cost={
                    "reported_amount": raw.get("observation", {}).get("observed_cost_amount"),
                    "reported_currency": raw.get("observation", {}).get("observed_cost_currency"),
                    "reported_source": raw.get("observation", {}).get("observed_cost_source"),
                    "reported_cost_verified": False,
                    "estimated_token_price_usd": raw.get("reported_token_price_usd"),
                    "estimated_token_price_source": "CLI list-price estimate; not actual charge"
                    if raw.get("reported_token_price_usd") is not None
                    else None,
                    "usd": record.cost_usd,
                    "inr": record.cost_inr,
                    "currency": "USD",
                    "usd_to_inr": run.usd_to_inr,
                    "known": record.cost_usd is not None,
                    "method": "manual_app_observation"
                    if config["evaluation_mode"] == "manual_app"
                    else "mock_execution"
                    if config["evaluation_mode"] == "mock"
                    else "subscription_cli_charge_unverified"
                    if config.get("evaluation_track") == "harness"
                    else "configured_rate_times_provider_usage",
                    "verified_provider_charge_usd": None,
                    "fx_source": "configured MJ_USD_INR; no live FX lookup",
                    "price": config.get("price"),
                    "scope": config.get("cost_scope", "mock execution"),
                },
                error=redact(record.error),
                artifact_reference=reference,
                output=redact(record.output) if include_outputs else None,
            )
        )
    groups = defaultdict(list)
    for result in results:
        # Never combine text, browser plans, or different configuration identities in one ranking.
        groups[(result.runner_id, result.capability_track, result.language, result.domain)].append(
            result
        )
    aggregates = []
    for (runner_id, track, language, domain), rows in sorted(
        groups.items(), key=lambda item: str(item[0])
    ):
        graded = [r for r in rows if r.grade_status == "graded" and r.status == "ok"]
        errors = sum(r.status == "error" for r in rows)
        passed = sum(r.passed is True for r in graded)
        denominator = len(graded) + errors
        costs = [r.cost["usd"] for r in rows]
        latencies = [r.latency_s for r in rows if r.latency_s is not None]
        scores = [r.score for r in graded if r.score is not None]
        mode = rows[0].evaluation_mode
        aggregates.append(
            {
                "runner_id": runner_id,
                "configuration_id": rows[0].configuration_id,
                "capability_track": track,
                "language": language,
                "domain": domain,
                "evaluation_mode": mode,
                "evaluation_track": rows[0].evaluation_track,
                "eligible_for_model_ranking": mode == "live"
                and rows[0].evaluation_track == "model_api",
                "eligible_for_app_ranking": False,
                "sample_count": len(rows),
                "graded_count": len(graded),
                "passed_count": passed,
                "error_count": errors,
                "skipped_count": sum(r.status == "skipped" for r in rows),
                "pending_count": sum(r.grade_status == "pending" for r in rows),
                "grader_error_count": sum(r.grade_status == "error" for r in rows),
                "success_rate": passed / denominator if denominator else None,
                "success_ci_95": wilson_interval(passed, denominator),
                "mean_score": mean(scores) if scores else None,
                "mean_latency_s": mean(latencies) if latencies else None,
                "cost_usd": sum(costs) if all(c is not None for c in costs) else None,
                "small_sample": len(graded) < 30,
            }
        )
    modes = {model["evaluation_mode"] for model in models}
    return ResultExport(
        benchmark={
            "name": run.suite_name,
            "release": run.suite_version,
            "validation_status": manifest.get("suite", {}).get(
                "validation_status", "unvalidated_pilot"
            ),
            "suite_sha256": manifest.get("suite", {}).get("sha256"),
        },
        run={
            "id": run.id,
            "started_at": run.started_at,
            "finished_at": run.finished_at,
            "status": run.status,
            "evaluation_mode": next(iter(modes)) if len(modes) == 1 else "mixed",
            "seed": run.seed,
            "software_version": run.mj_version,
            "source": manifest.get("source"),
            "max_usd": run.max_usd,
            "cost_usd": None if run.uncertain_cost else run.spent_usd,
            "known_spent_usd": run.spent_usd,
            "reserved_usd": run.reserved_usd,
        },
        models=models,
        results=results,
        aggregates=aggregates,
        supported_tracks={
            "consumer_app": "manual evidence imports; contributor-reported; review pending",
            "model_api": "direct API adapters; separate from app/subscription plans",
            "text": "supported",
            "browser_workflow": "offline action-plan state checks",
            "harness": "Claude tool-free synthetic drafts; other harnesses are stubs",
            "personal_agent": "stub",
            "image_understanding": "unsupported",
            "image_generation": "unsupported",
            "asr": "unsupported",
            "tts": "unsupported",
            "video_analysis": "unsupported",
            "video_generation": "unsupported",
        },
        limitations=[
            "Auditable pilot; no India-wide validated ranking.",
            "Mock checks are not model evaluations; live labels identify adapter execution, "
            "not validation.",
            "Success denominator is graded attempts plus execution errors; "
            "pending/skipped excluded.",
            "Browser workflow executes an offline state reducer, not a real browser agent.",
            "Costs cover API usage; subscriptions, hardware and electricity are excluded.",
            "Raw artifacts are private; default export omits prompts and output text.",
            "Manual observations grade recorded responses; "
            "app/plan context is contributor-reported. "
            "An API model result never evaluates an app subscription tier.",
        ],
    )


def write_export(
    store: Store, run_id: str, destination: Path, *, include_outputs: bool = False
) -> Path:
    payload = result_export(store, run_id, include_outputs=include_outputs)
    destination.write_text(payload.model_dump_json(indent=2) + "\n", encoding="utf-8")
    return destination
