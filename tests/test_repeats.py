"""Repeat runs end to end with free runners: engine, store, export, report, CLI and jobs."""

import json
import sqlite3
import time

import pytest
from pydantic import ValidationError

from kritrim_mulyankan.engine import derive_seed, preflight, run_suite
from kritrim_mulyankan.export import explanation_key, result_export
from kritrim_mulyankan.jobs import TERMINAL, JobManager, JobRequest
from kritrim_mulyankan.report import build_report, to_html, to_markdown
from kritrim_mulyankan.runners.base import RunContext
from kritrim_mulyankan.runners.fake import EchoRunner, OracleRunner
from kritrim_mulyankan.store import SCHEMA_VERSION, Store
from kritrim_mulyankan.suite import load_suite


class FlakyRunner(OracleRunner):
    """Free test runner: the oracle's answer on tries whose seed is even, the prompt otherwise."""

    def __init__(self) -> None:
        super(OracleRunner, self).__init__("flaky")

    def run(self, task, prompt, ctx: RunContext):
        return (
            super().run(task, prompt, ctx)
            if ctx.seed % 2 == 0
            else EchoRunner().run(task, prompt, ctx)
        )


class PricedRunner(EchoRunner):
    def estimate_cost_usd(self, task, prompt):
        return 0.01


def test_repeats_run_whole_rounds_with_their_own_seeds(example_suite, store, settings):
    order = []
    summary = run_suite(
        example_suite, [OracleRunner(), EchoRunner()], store=store, settings=settings,
        seed=7, repeats=3, on_result=lambda r: order.append(r.repeat_index),
    )  # fmt: skip
    tasks = len(example_suite.tasks)
    assert len(summary.results) == 3 * tasks * 2
    assert order == sorted(order)  # round 0 finishes before round 1 starts
    first = [r for r in summary.results if r.repeat_index == 0]
    assert all(r.seed == derive_seed(7, r.task_id) for r in first)  # try 0 keeps today's seed
    seeds = {(r.task_id, r.runner): set() for r in summary.results}
    for r in summary.results:
        seeds[(r.task_id, r.runner)].add(r.seed)
    assert all(len(found) == 3 for found in seeds.values())
    assert store.get_run(summary.run_id).repeats == 3
    manifest = json.loads((summary.run_dir / "manifest.json").read_text())
    assert manifest["repeats"] == 3
    oracle_dir = summary.run_dir / "oracle"
    assert (oracle_dir / "coding.sum-of-squares.json").is_file()
    assert (
        json.loads((oracle_dir / "coding.sum-of-squares.repeat-2.json").read_text())["repeat_index"]
        == 2
    )


def test_preflight_budgets_every_try(example_suite):
    once = preflight(example_suite.tasks, [PricedRunner()], cap=10)
    thrice = preflight(example_suite.tasks, [PricedRunner()], cap=10, repeats=3)
    assert thrice.worst_case_usd == pytest.approx(3 * once.worst_case_usd)
    assert not preflight(example_suite.tasks, [EchoRunner()], cap=0, repeats=11).ok


def test_export_counts_consistency_and_compares_on_the_same_track(example_suite, store, settings):
    summary = run_suite(
        example_suite, [OracleRunner(), FlakyRunner()], store=store, settings=settings,
        seed=3, repeats=4,
    )  # fmt: skip
    export = result_export(store, summary.run_id)
    assert export.schema_version == "1.1" and export.run.repeats == 4
    assert {r.repeat_index for r in export.results} == {0, 1, 2, 3}
    text = {a.runner_id: a for a in export.aggregates if a.capability_track == "text"}
    oracle, flaky = text["oracle"], text["flaky"]
    assert oracle.tries_per_task == 4 and oracle.success_ci_method == "task_bootstrap"
    assert oracle.always_correct == oracle.task_count and oracle.never_correct == 0
    assert flaky.sometimes_correct + flaky.never_correct > 0
    assert flaky.always_correct + flaky.sometimes_correct + flaky.never_correct == flaky.task_count
    pairs = {(v.runner_a, v.runner_b, v.metric) for v in export.verdicts}
    assert ("oracle", "flaky", "accuracy") in pairs


def test_report_shows_tries_and_the_verdict(example_suite, store, settings):
    summary = run_suite(
        example_suite, [OracleRunner(), EchoRunner()], store=store, settings=settings, repeats=3
    )
    rep = build_report(store, summary.run_id)
    assert rep.cells[("coding.sum-of-squares", "oracle")] == "✓ 3/3"
    assert rep.cells[("coding.sum-of-squares", "echo")] == "✗ 0/3"
    markdown = to_markdown(rep)
    assert "## Consistency (3 tries per task)" in markdown
    assert "## Is the difference real?" in markdown
    assert "Too close to call" in markdown  # four graded tasks can never be conclusive
    assert "<h2>Is the difference real?</h2>" in to_html(rep)


def test_single_try_cells_are_unchanged(example_suite, store, settings):
    summary = run_suite(example_suite, [OracleRunner()], store=store, settings=settings)
    assert build_report(store, summary.run_id).cells[("coding.sum-of-squares", "oracle")] == (
        "✓ 1.00"
    )


def test_cli_repeats_and_verdicts(km):
    result = km("run", "example", "-r", "echo", "-r", "oracle", "--repeats", "2", "--max-usd", "0")
    assert result.exit_code == 0, result.output
    assert "(try 2/2)" in result.output
    assert "Consistency over 2 tries per task" in result.output
    assert "Is the difference real?" in result.output and "Too close to call" in result.output
    assert km("run", "example", "-r", "echo", "--repeats", "11").exit_code != 0


def test_v2_database_migrates_to_repeat_runs(tmp_path):
    path = tmp_path / "v2.db"
    conn = sqlite3.connect(path)
    conn.executescript(
        """
        CREATE TABLE runs (id TEXT PRIMARY KEY, started_at TEXT NOT NULL, finished_at TEXT,
            status TEXT NOT NULL, suite_name TEXT NOT NULL, suite_version TEXT NOT NULL,
            seed INTEGER NOT NULL, runners TEXT NOT NULL, max_usd REAL NOT NULL,
            spent_usd REAL NOT NULL DEFAULT 0, reserved_usd REAL NOT NULL DEFAULT 0,
            uncertain_cost INTEGER NOT NULL DEFAULT 0, usd_to_inr REAL NOT NULL,
            artifacts_dir TEXT NOT NULL, mj_version TEXT NOT NULL);
        CREATE TABLE results (id INTEGER PRIMARY KEY AUTOINCREMENT,
            run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE, task_id TEXT NOT NULL,
            task_version INTEGER NOT NULL, task_hash TEXT NOT NULL, category TEXT NOT NULL,
            runner TEXT NOT NULL, runner_kind TEXT NOT NULL, seed INTEGER NOT NULL,
            status TEXT NOT NULL, output TEXT, error TEXT, latency_s REAL, input_tokens INTEGER,
            output_tokens INTEGER, cost_usd REAL, cost_inr REAL, steps INTEGER,
            tool_calls INTEGER, grader TEXT, grade_status TEXT, passed INTEGER, score REAL,
            grade_details TEXT, human_score REAL, human_note TEXT, artifact_path TEXT,
            created_at TEXT NOT NULL, UNIQUE (run_id, task_id, runner));
        CREATE TABLE comparisons (id INTEGER PRIMARY KEY AUTOINCREMENT, run_id TEXT NOT NULL,
            task_id TEXT NOT NULL, runner_a TEXT NOT NULL, runner_b TEXT NOT NULL,
            winner TEXT NOT NULL, votes TEXT NOT NULL, judge_model TEXT, details TEXT,
            created_at TEXT NOT NULL);
        INSERT INTO runs VALUES ('r1', 'then', 'later', 'completed', 's', '1', 0, '["echo"]', 1,
            0, 0, 0, 88, 'runs/r1', '0.1.0');
        INSERT INTO results (run_id, task_id, task_version, task_hash, category, runner,
            runner_kind, seed, status, passed, created_at)
            VALUES ('r1', 't', 1, 'h', 'c', 'echo', 'fake', 5, 'ok', 1, 'now');
        PRAGMA user_version = 2;
        """
    )
    conn.close()
    with Store(path) as store:
        assert store.conn.execute("PRAGMA user_version").fetchone()[0] == SCHEMA_VERSION == 3
        assert store.get_run("r1").repeats == 1
        (old,) = store.results_for_run("r1")
        assert old.repeat_index == 0 and old.seed == 5 and old.passed is True
        store.add_result(old.model_copy(update={"repeat_index": 1}))  # a second try is allowed
        with pytest.raises(sqlite3.IntegrityError):
            store.add_result(old.model_copy(update={"repeat_index": 1}))


def test_job_requests_bound_repeats_and_count_every_try(settings, suites_dir):
    with pytest.raises(ValidationError):
        JobRequest(task_ids=["DEMO01"], adapter_ids=["mock:oracle"], repeats=6,
                   idempotency_key="key-123456")  # fmt: skip
    manager = JobManager({"india-pilot": load_suite(suites_dir / "india-pilot.yaml")}, settings)
    try:
        request = JobRequest(
            task_ids=["DEMO01", "DEMO02"], adapter_ids=["mock:oracle", "mock:echo"], repeats=2,
            idempotency_key="repeat-job-1",
        )  # fmt: skip
        preview, _ = manager.preview(request)
        assert preview["attempts"] == 8 and preview["repeats"] == 2
        job = manager.create(request)
        assert job["total"] == 8
        deadline = time.monotonic() + 20
        while manager.get(job["id"])["status"] not in TERMINAL and time.monotonic() < deadline:
            time.sleep(0.05)
        state = manager.get(job["id"])
        assert state["status"] == "completed" and state["processed"] == 8
        assert {
            event["repeat_index"] for event in state["events"] if event["stage"] == "persisted"
        } == {0, 1}
        payload = manager.result(job["id"], outputs=True)
        assert {v["metric"] for v in payload["verdicts"]} == {"accuracy", "speed"}
        assert "DEMO01:oracle#1" in payload["explanations"]
    finally:
        manager.close()


def test_explanation_keys_keep_the_first_try_unchanged(example_suite, store, settings):
    summary = run_suite(example_suite, [OracleRunner()], store=store, settings=settings, repeats=2)
    keys = {explanation_key(r) for r in summary.results if r.task_id == "coding.sum-of-squares"}
    assert keys == {"coding.sum-of-squares:oracle", "coding.sum-of-squares:oracle#1"}
