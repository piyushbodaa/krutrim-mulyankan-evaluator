import json
from pathlib import Path

import pytest
import yaml

from kritrim_mulyankan.engine import PreflightError, derive_seed, estimate, preflight, run_suite
from kritrim_mulyankan.runners import EchoRunner, OracleRunner, make_runner
from kritrim_mulyankan.runners.base import RunContext, Runner, RunOutput, Usage
from kritrim_mulyankan.suite import Task, load_suite


class PricedRunner(Runner):
    """Pretends each attempt costs a fixed amount, to exercise the budget cap.

    `actual` lets a test simulate a runner whose estimate is too low.
    """

    kind = "fake"

    def __init__(self, cost: float, actual: float | None = None) -> None:
        super().__init__(f"priced-{cost}")
        self.cost = cost
        self.actual = cost if actual is None else actual
        self.calls = 0

    def run(self, task: Task, prompt: str, ctx: RunContext) -> RunOutput:
        self.calls += 1
        return RunOutput(output="x", usage=Usage(cost_usd=self.actual))

    def estimate_cost_usd(self, task: Task, prompt: str) -> float:
        return self.cost


class BrokenRunner(Runner):
    kind = "fake"

    def __init__(self) -> None:
        super().__init__("broken")

    def run(self, task: Task, prompt: str, ctx: RunContext) -> RunOutput:
        raise ConnectionError("provider unreachable")

    def estimate_cost_usd(self, task: Task, prompt: str) -> float:
        return 0.0


class WorkdirRunner(Runner):
    """Replies with the file names it finds in its workdir."""

    kind = "fake"

    def __init__(self) -> None:
        super().__init__("workdir")

    def run(self, task: Task, prompt: str, ctx: RunContext) -> RunOutput:
        files = sorted(str(p.relative_to(ctx.workdir)) for p in ctx.workdir.rglob("*.*"))
        return RunOutput(output=",".join(files))

    def estimate_cost_usd(self, task: Task, prompt: str) -> float:
        return 0.0


def test_echo_and_oracle_end_to_end(example_suite, store, settings):
    summary = run_suite(
        example_suite, [EchoRunner(), OracleRunner()], store=store, settings=settings
    )

    assert summary.status == "completed" and summary.spent_usd == 0
    results = {(r.task_id, r.runner): r for r in store.results_for_run(summary.run_id)}
    assert len(results) == 10

    exact_ids = [t.id for t in example_suite.tasks if t.grader.type == "exact"]
    assert all(results[(t, "oracle")].passed for t in exact_ids)
    assert not any(results[(t, "echo")].passed for t in exact_ids)
    for runner in ("echo", "oracle"):
        writing = results[("writing.late-order-apology", runner)]
        assert writing.status == "ok" and writing.grade_status == "pending"
        assert writing.passed is None

    run = store.get_run(summary.run_id)
    assert run.status == "completed" and run.finished_at


def test_raw_artifacts_are_saved(example_suite, store, settings):
    summary = run_suite(example_suite, [OracleRunner()], store=store, settings=settings, seed=7)
    run_dir = summary.run_dir
    assert run_dir.parent == settings.runs_dir

    manifest = json.loads((run_dir / "manifest.json").read_text())
    assert manifest["seed"] == 7 and len(manifest["tasks"]) == 5
    assert all(t["hash"] for t in manifest["tasks"])
    assert (run_dir / "suite.yaml").read_text() == example_suite.path.read_text()

    artifact = json.loads((run_dir / "oracle" / "coding.sum-of-squares.json").read_text())
    assert artifact["result"]["output"] == "14"
    assert artifact["grade"]["passed"] is True
    assert artifact["seed"] == derive_seed(7, "coding.sum-of-squares")


def test_run_ids_do_not_collide(example_suite, store, settings):
    a = run_suite(example_suite, [EchoRunner()], store=store, settings=settings)
    b = run_suite(example_suite, [EchoRunner()], store=store, settings=settings)
    assert a.run_id != b.run_id


def test_runner_errors_are_recorded_and_the_run_continues(example_suite, store, settings):
    broken = BrokenRunner()
    summary = run_suite(example_suite, [broken, EchoRunner()], store=store, settings=settings)
    results = store.results_for_run(summary.run_id)
    broken_results = [r for r in results if r.runner == "broken"]
    assert len(broken_results) == 5
    assert all(r.status == "error" and "ConnectionError" in r.error for r in broken_results)
    assert sum(r.runner == "echo" and r.status == "ok" for r in results) == 5
    assert summary.status == "completed"


def test_worst_case_over_the_cap_is_refused_before_anything_runs(example_suite, store, settings):
    priced = PricedRunner(0.04)  # 5 x 0.04 = 0.20 worst case
    with pytest.raises(PreflightError, match="worst-case"):
        run_suite(example_suite, [priced], store=store, settings=settings, max_usd=0.10)
    assert priced.calls == 0
    assert store.list_runs() == []


def test_running_cap_still_stops_a_runner_that_underestimates(example_suite, store, settings):
    # Estimates 0.01 (passes preflight: 0.05 <= 0.10) but really costs 0.04 per attempt.
    priced = PricedRunner(0.01, actual=0.04)
    summary = run_suite(example_suite, [priced], store=store, settings=settings, max_usd=0.10)
    statuses = [r.status for r in store.results_for_run(summary.run_id)]
    assert statuses == ["ok", "ok", "ok", "skipped", "skipped"]
    assert priced.calls == 3
    assert summary.status == "budget_exceeded"
    assert summary.spent_usd == pytest.approx(0.12)  # why estimates must be upper bounds
    assert store.get_run(summary.run_id).spent_usd == summary.spent_usd


def test_task_budget_skips_expensive_attempts(example_suite, store, settings):
    priced = PricedRunner(0.30)  # over every example task's 0.25 budget
    check = preflight(example_suite.tasks, [priced], cap=10)
    assert check.ok and "task's budget_usd" in check.warnings[0]
    summary = run_suite(example_suite, [priced], store=store, settings=settings, max_usd=10)
    results = store.results_for_run(summary.run_id)
    assert all(r.status == "skipped" and "task budget" in r.error for r in results)
    assert priced.calls == 0 and summary.status == "completed"


def test_preflight_reports_unready_runners(example_suite):
    check = preflight(example_suite.tasks, [make_runner("harness:claude"), EchoRunner()], cap=1)
    assert check.problems == ["harness:claude: HarnessRunner is not implemented yet"]


def test_costs_are_reported_in_inr(example_suite, store, settings):
    settings = settings.model_copy(update={"usd_to_inr": 80.0})
    summary = run_suite(
        example_suite, [PricedRunner(0.01)], store=store, settings=settings, max_usd=10
    )
    r = store.results_for_run(summary.run_id)[0]
    assert r.cost_usd == 0.01 and abs(r.cost_inr - 0.8) < 1e-9


def test_fixtures_are_copied_into_a_fresh_workdir(tmp_path, store, settings):
    (tmp_path / "data").mkdir()
    (tmp_path / "data" / "notes.txt").write_text("hello")
    suite_path = tmp_path / "fx.yaml"
    suite_path.write_text(yaml.safe_dump({
        "name": "fx", "version": 1,
        "tasks": [{
            "id": "fx.one", "category": "coding", "prompt": "list files",
            "fixtures": ["data/notes.txt"],
            "grader": {"type": "exact", "expected": "data/notes.txt"},
        }],
    }))  # fmt: skip
    suite = load_suite(suite_path)
    summary = run_suite(suite, [WorkdirRunner()], store=store, settings=settings)
    assert summary.results[0].passed is True


def test_seeds_are_per_task_and_deterministic():
    assert derive_seed(0, "a") == derive_seed(0, "a")
    assert derive_seed(0, "a") != derive_seed(0, "b")
    assert derive_seed(0, "a") != derive_seed(1, "a")


def test_estimate_covers_every_pair(example_suite):
    est = estimate(example_suite.tasks, [EchoRunner(), make_runner("harness:claude")])
    assert len(est) == 10
    assert {e.cost_usd for e in est if e.runner == "echo"} == {0.0}
    assert {e.cost_usd for e in est if e.runner == "harness:claude"} == {None}


def test_suite_path_is_used_for_suite_snapshot(example_suite):
    assert isinstance(example_suite.path, Path)


def test_concurrent_run_directory_reservations_are_unique(tmp_path):
    from concurrent.futures import ThreadPoolExecutor
    from datetime import datetime

    from kritrim_mulyankan.engine import new_run_dir

    timestamp = datetime(2026, 10, 12, 12, 0, 0)
    with ThreadPoolExecutor(max_workers=8) as pool:
        reservations = list(
            pool.map(lambda _: new_run_dir(tmp_path / "runs", timestamp), range(16))
        )
    assert len({identity for identity, _ in reservations}) == 16
    assert all(directory.is_dir() for _, directory in reservations)
