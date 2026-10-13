"""Runs every (task, runner) pair in a suite, grades it, saves raw artifacts and records results."""

from __future__ import annotations

import hashlib
import json
import math
import platform
import re
import shutil
import tempfile
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from threading import Event

from kritrim_mulyankan import __version__
from kritrim_mulyankan.graders import make_grader
from kritrim_mulyankan.graders.base import Grade
from kritrim_mulyankan.graders.llm_judge import Judge
from kritrim_mulyankan.provenance import redact, sha256_file, source_metadata
from kritrim_mulyankan.runners.base import RunContext, Runner, RunOutput, Usage
from kritrim_mulyankan.settings import Settings
from kritrim_mulyankan.store import ResultRecord, RunRecord, Store
from kritrim_mulyankan.suite import Suite, Task


@dataclass(frozen=True)
class Estimate:
    task_id: str
    runner: str
    cost_usd: float | None  # None: the runner cannot estimate


@dataclass
class RunSummary:
    run_id: str
    run_dir: Path
    status: str
    spent_usd: float
    results: list[ResultRecord] = field(default_factory=list)


def estimate(tasks: list[Task], runners: list[Runner]) -> list[Estimate]:
    return [
        Estimate(t.id, r.name, r.estimate_cost_usd(t, t.render_prompt()))
        for t in tasks
        for r in runners
    ]


class PreflightError(Exception):
    """The run would break the budget or cannot start; nothing has been called or spent."""

    def __init__(self, problems: list[str]) -> None:
        super().__init__("; ".join(problems))
        self.problems = problems


@dataclass
class Preflight:
    estimates: list[Estimate]
    worst_case_usd: float
    problems: list[str]
    warnings: list[str]

    @property
    def ok(self) -> bool:
        return not self.problems


def preflight(tasks: list[Task], runners: list[Runner], cap: float) -> Preflight:
    """Check everything that can be checked without calling anything: runner readiness (keys,
    prices), that every attempt has a worst-case estimate, and that the total fits the cap."""
    estimates = estimate(tasks, runners)
    by_id = {t.id: t for t in tasks}
    problems: list[str] = []
    if not math.isfinite(cap) or cap < 0:
        problems.append("budget cap must be finite and nonnegative")
    warnings: list[str] = []
    for runner in runners:
        own = runner.problems()
        problems.extend(f"{runner.name}: {p}" for p in own)
        unknown = sum(e.cost_usd is None for e in estimates if e.runner == runner.name)
        if unknown and not own:
            problems.append(f"{runner.name}: no cost estimate for {unknown} attempt(s)")
    if any(
        e.cost_usd is not None and (not math.isfinite(e.cost_usd) or e.cost_usd < 0)
        for e in estimates
    ):
        problems.append("cost estimates must be finite and nonnegative")
    unsupported = {t.capability_track for t in tasks} - {"text", "browser_workflow"}
    if unsupported:
        problems.append(f"unsupported capability tracks: {sorted(unsupported)}")
    over_task_budget = [
        e
        for e in estimates
        if e.cost_usd is not None
        and (budget := by_id[e.task_id].budget_usd) is not None
        and e.cost_usd > budget
    ]
    if over_task_budget:
        warnings.append(
            f"{len(over_task_budget)} attempt(s) exceed their task's budget_usd at worst case and "
            "will be skipped (lower max_output_tokens or raise budget_usd)"
        )
    worst = sum(
        e.cost_usd for e in estimates if e.cost_usd is not None and e not in over_task_budget
    )
    if worst > cap:
        problems.append(
            f"worst-case cost ${worst:.4f} exceeds the ${cap:.2f} cap; raise --max-usd, "
            "lower max_output_tokens, or run fewer tasks or runners"
        )
    return Preflight(estimates, worst, problems, warnings)


def derive_seed(run_seed: int, task_id: str) -> int:
    """Per-task seed: the same for every runner, so all systems get the same sampling seed."""
    digest = hashlib.sha256(f"{run_seed}:{task_id}".encode()).digest()
    return int.from_bytes(digest[:4], "big")


def slug(text: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]+", "_", text).strip("_") or "_"


def new_run_dir(runs_dir: Path, now: datetime) -> tuple[str, Path]:
    base = now.strftime("%Y%m%d-%H%M%S")
    run_id, n = base, 1
    runs_dir.mkdir(parents=True, exist_ok=True)
    while True:
        run_dir = runs_dir / run_id
        try:
            run_dir.mkdir()  # Atomic reservation across jobs and manual recording requests.
            return run_id, run_dir
        except FileExistsError:
            run_id, n = f"{base}-{n}", n + 1


def _now() -> str:
    return datetime.now().astimezone().isoformat()


def run_suite(
    suite: Suite,
    runners: list[Runner],
    *,
    store: Store,
    settings: Settings,
    seed: int = 0,
    max_usd: float | None = None,
    task_ids: list[str] | None = None,
    judge: Judge | None = None,
    on_result: Callable[[ResultRecord], None] | None = None,
    on_start: Callable[[str], None] | None = None,
    on_attempt: Callable[[str, str], None] | None = None,
    cancel_event: Event | None = None,
) -> RunSummary:
    tasks = suite.select(task_ids)
    if len({r.name for r in runners}) != len(runners) or not runners:
        raise PreflightError(["runners must be nonempty and unique"])
    cap = settings.max_usd if max_usd is None else max_usd
    check = preflight(tasks, runners, cap)
    if not check.ok:
        raise PreflightError(check.problems)
    run_id, run_dir = new_run_dir(settings.runs_dir, datetime.now())

    if suite.path:
        shutil.copyfile(suite.path, run_dir / "suite.yaml")
        for fixture in {p for task in tasks for p in task.fixtures}:
            destination = run_dir / fixture
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(suite.path.parent / fixture, destination)
    manifest = {
        "schema_version": "1.0",
        "run_id": run_id,
        "started_at": _now(),
        "suite": {
            "name": suite.name,
            "version": suite.version,
            "validation_status": suite.validation_status,
            "sha256": sha256_file(suite.path)
            if suite.path
            else hashlib.sha256(suite.model_dump_json().encode()).hexdigest(),
        },
        "tasks": [
            {
                "id": t.id,
                "version": t.version,
                "hash": t.content_hash(),
                "metadata": t.model_dump(
                    mode="json",
                    include={
                        "category",
                        "capability_track",
                        "capability_track_version",
                        "language",
                        "script",
                        "input_style",
                        "domain",
                        "as_of",
                        "provenance",
                        "risk_flags",
                    },
                ),
                "fixtures": {str(p): sha256_file(suite.path.parent / p) for p in t.fixtures}
                if suite.path
                else {},
            }
            for t in tasks
        ],
        "runners": [{"spec": r.name, **r.configuration()} for r in runners],
        "seed": seed,
        "max_usd": cap,
        "usd_to_inr": settings.usd_to_inr,
        "mj_version": __version__,
        "python": platform.python_version(),
        "source": source_metadata(),
        "budget_policy": "Reserve worst-case cost when billing is unknown; paid retries disabled.",
    }
    (run_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")

    store.create_run(
        RunRecord(
            id=run_id,
            started_at=manifest["started_at"],
            status="running",
            suite_name=suite.name,
            suite_version=suite.version,
            seed=seed,
            runners=[r.name for r in runners],
            max_usd=cap,
            usd_to_inr=settings.usd_to_inr,
            artifacts_dir=str(run_dir),
            mj_version=__version__,
        )
    )

    spent = 0.0
    reserved = 0.0
    uncertain_cost = False
    status = "completed"
    summary = RunSummary(run_id=run_id, run_dir=run_dir, status=status, spent_usd=0.0)
    try:
        if on_start:
            on_start(run_id)
        for task in tasks:
            prompt = task.render_prompt()
            grader = make_grader(
                task, judge=judge, fixture_base=suite.path.parent if suite.path else None
            )
            task_seed = derive_seed(seed, task.id)
            for runner in runners:
                est = runner.estimate_cost_usd(task, prompt)
                skip = None
                if cancel_event and cancel_event.is_set():
                    skip = "Cancelled before this attempt; no adapter call made"
                    status = "cancelled"
                elif task.budget_usd is not None and est is not None and est > task.budget_usd:
                    skip = f"estimate ${est:.4f} exceeds task budget ${task.budget_usd:.4f}"
                elif reserved + (est or 0.0) > cap:
                    skip = f"run budget cap ${cap:.2f} reached (spent ${spent:.4f})"
                    status = "budget_exceeded"

                if skip:
                    out, latency, run_status, grade = (
                        RunOutput(error=skip, usage=Usage(cost_usd=0)),
                        None,
                        "skipped",
                        None,
                    )
                else:
                    if on_attempt:
                        on_attempt(task.id, runner.name)
                    out, latency = _attempt(runner, task, prompt, suite, task_seed, cancel_event)
                    if cancel_event and cancel_event.is_set():
                        status = "cancelled"
                    run_status = "error" if out.error else "ok"
                    # Account before grading so a broken grader never loses model usage/cost.
                    grade = None
                    if run_status == "ok":
                        try:
                            grade = grader.grade(task, out.output)
                        except Exception as exc:
                            grade = Grade(
                                grader=grader.name,
                                status="error",
                                details={"error": redact(f"{type(exc).__name__}: {exc}")},
                            )
                if (
                    out.usage.cost_usd is None
                    and est == 0
                    and runner.configuration().get("evaluation_mode") != "manual_app"
                    and runner.kind != "harness"
                ):
                    out.usage.cost_usd = 0  # explicitly zero-priced local/mock execution
                spent += out.usage.cost_usd or 0
                reserved += out.usage.cost_usd if out.usage.cost_usd is not None else (est or 0)
                uncertain_cost |= out.usage.cost_usd is None
                if reserved > cap:
                    status = "budget_exceeded"

                record = _record(
                    run_id, task, runner, task_seed, run_status, out, latency, grade, settings
                )
                record.artifact_path = str(
                    _save_artifact(run_dir, task, runner, prompt, out, record, grade)
                )
                store.add_result(record)
                summary.results.append(record)
                if on_result:
                    on_result(record)
    except BaseException:
        status = "failed"
        raise
    finally:
        finished_at = _now()
        store.finish_run(
            run_id,
            status=status,
            finished_at=finished_at,
            spent_usd=spent,
            reserved_usd=reserved,
            uncertain_cost=uncertain_cost,
        )
        manifest.update(
            status=status,
            finished_at=finished_at,
            known_spent_usd=spent,
            reserved_usd=reserved,
            uncertain_cost=uncertain_cost,
        )
        (run_dir / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        summary.status, summary.spent_usd = status, spent
    return summary


def _attempt(
    runner: Runner,
    task: Task,
    prompt: str,
    suite: Suite,
    seed: int,
    cancel_event: Event | None = None,
) -> tuple[RunOutput, float | None]:
    with tempfile.TemporaryDirectory(prefix="km-") as tmp:
        workdir = Path(tmp)
        base = suite.path.parent if suite.path else Path.cwd()
        for fixture in task.fixtures:
            dest = workdir / fixture
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(base / fixture, dest)
        ctx = RunContext(
            workdir=workdir, seed=seed, timeout_s=task.timeout_s, cancel_event=cancel_event
        )
        start = time.perf_counter()
        started_at = _now()
        try:
            out = runner.run(task, prompt, ctx)
        except Exception as exc:  # one broken attempt must not sink the whole run
            out = RunOutput(error=redact(f"{type(exc).__name__}: {exc}"))
        latency = time.perf_counter() - start
        out.raw["attempt_timing"] = {
            "started_at": started_at,
            "finished_at": _now(),
            "wall_elapsed_s": latency,
            "scope": "adapter invocation including local overhead",
        }
        if runner.configuration().get("evaluation_mode") == "manual_app":
            # Import latency is not app response time. Preserve only observed/reported timing.
            latency = runner.configuration()["app_context"].get("observed_latency_s")
        return out, latency


def _record(
    run_id: str,
    task: Task,
    runner: Runner,
    seed: int,
    status: str,
    out: RunOutput,
    latency: float | None,
    grade: Grade | None,
    settings: Settings,
) -> ResultRecord:
    return ResultRecord(
        run_id=run_id,
        task_id=task.id,
        task_version=task.version,
        task_hash=task.content_hash(),
        category=task.category,
        runner=runner.name,
        runner_kind=runner.kind,
        seed=seed,
        status=status,
        created_at=_now(),
        output=out.output if status != "skipped" else None,
        error=out.error,
        latency_s=latency,
        input_tokens=out.usage.input_tokens,
        output_tokens=out.usage.output_tokens,
        cost_usd=out.usage.cost_usd,
        cost_inr=out.usage.cost_usd * settings.usd_to_inr
        if out.usage.cost_usd is not None
        else None,
        steps=out.steps,
        tool_calls=out.tool_calls,
        grader=grade.grader if grade else None,
        grade_status=grade.status if grade else None,
        passed=grade.passed if grade else None,
        score=grade.score if grade else None,
        grade_details=grade.details if grade else None,
    )


def _save_artifact(
    run_dir: Path,
    task: Task,
    runner: Runner,
    prompt: str,
    out: RunOutput,
    record: ResultRecord,
    grade: Grade | None,
) -> Path:
    suffix = hashlib.sha256(runner.name.encode()).hexdigest()[:10]
    # Preserve familiar fake paths; disambiguate provider specs that sanitize to the same slug.
    directory = slug(runner.name) if runner.kind == "fake" else f"{slug(runner.name)}-{suffix}"
    path = run_dir / directory / f"{slug(task.id)}.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "task": {"id": task.id, "version": task.version, "hash": record.task_hash},
        "runner": {"spec": runner.name, "kind": runner.kind},
        "seed": record.seed,
        "status": record.status,
        "latency_s": record.latency_s,
        "prompt": prompt,
        "result": out.model_dump(mode="json"),
        "grade": grade.model_dump(mode="json") if grade else None,
    }
    path.write_text(
        json.dumps(redact(payload), indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    return path
