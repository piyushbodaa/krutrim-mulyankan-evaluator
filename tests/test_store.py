import sqlite3

import pytest

from kritrim_mulyankan.store import SCHEMA_VERSION, ResultRecord, RunRecord, Store


def _run(run_id="r1", started="2026-10-11T10:00:00+05:30") -> RunRecord:
    return RunRecord(
        id=run_id, started_at=started, status="running", suite_name="s", suite_version="1",
        seed=0, runners=["echo", "oracle"], max_usd=1.0, usd_to_inr=88.0,
        artifacts_dir="runs/r1", mj_version="0.1.0",
    )  # fmt: skip


def _result(**overrides) -> ResultRecord:
    base = dict(
        run_id="r1", task_id="t", task_version=1, task_hash="abc", category="reasoning",
        runner="echo", runner_kind="fake", seed=1, status="ok", created_at="now",
        passed=True, score=1.0, grade_status="graded", grade_details={"matched": "4"},
    )  # fmt: skip
    return ResultRecord(**{**base, **overrides})


def test_schema_is_created_and_versioned(settings):
    with Store(settings.db_path) as store:
        tables = {
            r[0] for r in store.conn.execute("SELECT name FROM sqlite_master WHERE type='table'")
        }
        assert {"runs", "results", "comparisons"} <= tables
        assert store.conn.execute("PRAGMA user_version").fetchone()[0] == SCHEMA_VERSION
    Store(settings.db_path).close()  # reopening is idempotent


def test_round_trip(store):
    store.create_run(_run())
    store.add_result(_result())
    store.add_result(_result(runner="oracle", passed=False, score=0.0, grade_details=None))
    store.finish_run("r1", status="completed", finished_at="later", spent_usd=0.5)

    run = store.get_run("r1")
    assert run.status == "completed" and run.spent_usd == 0.5 and run.runners == ["echo", "oracle"]
    results = store.results_for_run("r1")
    assert [r.runner for r in results] == ["echo", "oracle"]
    assert results[0].passed is True and results[0].grade_details == {"matched": "4"}
    assert results[1].passed is False and results[1].grade_details is None


def test_one_result_per_task_and_runner(store):
    store.create_run(_run())
    store.add_result(_result())
    with pytest.raises(sqlite3.IntegrityError):
        store.add_result(_result())


def test_latest_and_list_runs(store):
    store.create_run(_run("old", "2026-10-10T10:00:00+05:30"))
    store.create_run(_run("new", "2026-10-11T10:00:00+05:30"))
    assert store.latest_run_id() == "new"
    assert [r.id for r in store.list_runs()] == ["new", "old"]


def test_refuses_a_newer_schema(settings):
    Store(settings.db_path).close()
    conn = sqlite3.connect(settings.db_path)
    conn.execute(f"PRAGMA user_version = {SCHEMA_VERSION + 1}")
    conn.close()
    with pytest.raises(RuntimeError, match="schema"):
        Store(settings.db_path)


def test_current_database_reader_does_not_migrate_under_writer_lock(tmp_path):
    path = tmp_path / "concurrent.db"
    with Store(path) as writer:
        writer.conn.execute("BEGIN IMMEDIATE")
        with Store(path) as reader:
            assert reader.list_runs() == []
        writer.conn.rollback()
