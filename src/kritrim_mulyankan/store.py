"""SQLite results store. One file holds every run, result and pairwise comparison."""

from __future__ import annotations

import json
import sqlite3
from pathlib import Path
from typing import Any

from pydantic import BaseModel

SCHEMA_VERSION = 2

SCHEMA = """
CREATE TABLE IF NOT EXISTS runs (
    id              TEXT PRIMARY KEY,          -- timestamp, also the runs/<id>/ folder name
    started_at      TEXT NOT NULL,
    finished_at     TEXT,
    status          TEXT NOT NULL,             -- running | completed | budget_exceeded | failed
    suite_name      TEXT NOT NULL,
    suite_version   TEXT NOT NULL,
    seed            INTEGER NOT NULL,
    runners         TEXT NOT NULL,             -- JSON list of runner specs
    max_usd         REAL NOT NULL,
    spent_usd       REAL NOT NULL DEFAULT 0,
    reserved_usd    REAL NOT NULL DEFAULT 0,
    uncertain_cost  INTEGER NOT NULL DEFAULT 0,
    usd_to_inr      REAL NOT NULL,
    artifacts_dir   TEXT NOT NULL,
    mj_version      TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS results (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id          TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    task_id         TEXT NOT NULL,
    task_version    INTEGER NOT NULL,
    task_hash       TEXT NOT NULL,
    category        TEXT NOT NULL,
    runner          TEXT NOT NULL,
    runner_kind     TEXT NOT NULL,
    seed            INTEGER NOT NULL,
    status          TEXT NOT NULL,             -- ok | error | skipped
    output          TEXT,
    error           TEXT,
    latency_s       REAL,
    input_tokens    INTEGER,
    output_tokens   INTEGER,
    cost_usd        REAL,
    cost_inr        REAL,
    steps           INTEGER,
    tool_calls      INTEGER,
    grader          TEXT,
    grade_status    TEXT,                      -- graded | pending | error; NULL if not graded
    passed          INTEGER,                   -- 1 | 0 | NULL
    score           REAL,                      -- 0..1
    grade_details   TEXT,                      -- JSON
    human_score     REAL,                      -- 0..1 override, wins over score when set
    human_note      TEXT,
    artifact_path   TEXT,
    created_at      TEXT NOT NULL,
    UNIQUE (run_id, task_id, runner)
);

CREATE INDEX IF NOT EXISTS results_runner ON results (runner, task_id);

-- Pairwise judgments, input to the Elo / Bradley-Terry leaderboard.
CREATE TABLE IF NOT EXISTS comparisons (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id          TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    task_id         TEXT NOT NULL,
    runner_a        TEXT NOT NULL,
    runner_b        TEXT NOT NULL,
    winner          TEXT NOT NULL,             -- a | b | tie
    votes           TEXT NOT NULL,             -- JSON: winner per presentation order
    judge_model     TEXT,
    details         TEXT,                      -- JSON
    created_at      TEXT NOT NULL
);
"""


class RunRecord(BaseModel):
    id: str
    started_at: str
    status: str
    suite_name: str
    suite_version: str
    seed: int
    runners: list[str]
    max_usd: float
    usd_to_inr: float
    artifacts_dir: str
    mj_version: str
    finished_at: str | None = None
    spent_usd: float = 0.0
    reserved_usd: float = 0.0
    uncertain_cost: bool = False


class ResultRecord(BaseModel):
    run_id: str
    task_id: str
    task_version: int
    task_hash: str
    category: str
    runner: str
    runner_kind: str
    seed: int
    status: str
    created_at: str
    output: str | None = None
    error: str | None = None
    latency_s: float | None = None
    input_tokens: int | None = None
    output_tokens: int | None = None
    cost_usd: float | None = None
    cost_inr: float | None = None
    steps: int | None = None
    tool_calls: int | None = None
    grader: str | None = None
    grade_status: str | None = None
    passed: bool | None = None
    score: float | None = None
    grade_details: dict[str, Any] | None = None
    human_score: float | None = None
    human_note: str | None = None
    artifact_path: str | None = None


class Store:
    def __init__(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        self.path = path
        self.conn = sqlite3.connect(path)
        self.conn.row_factory = sqlite3.Row
        self.conn.execute("PRAGMA foreign_keys = ON")
        self._migrate()

    def _migrate(self) -> None:
        version = self.conn.execute("PRAGMA user_version").fetchone()[0]
        if version > SCHEMA_VERSION:
            raise RuntimeError(
                f"{self.path} has schema v{version}; this km knows v{SCHEMA_VERSION}"
            )
        if version == SCHEMA_VERSION:
            return  # Opening a current database for a read must not acquire a write lock.
        migration = ""
        if version == 1:
            # One transaction: a failed migration rolls back without partial ALTERs.
            migration = """
            ALTER TABLE runs ADD COLUMN reserved_usd REAL NOT NULL DEFAULT 0;
            ALTER TABLE runs ADD COLUMN uncertain_cost INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE results RENAME TO results_v1;
            DROP INDEX IF EXISTS results_runner;
            """
        restore = (
            "INSERT INTO results SELECT * FROM results_v1; DROP TABLE results_v1;"
            if version == 1
            else ""
        )
        try:
            self.conn.executescript(
                f"BEGIN IMMEDIATE; {migration} {SCHEMA} {restore} "
                f"PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"
            )
        except sqlite3.Error:
            self.conn.rollback()
            raise

    def close(self) -> None:
        self.conn.close()

    def __enter__(self) -> Store:
        return self

    def __exit__(self, *exc: object) -> None:
        self.close()

    # --- writes ---------------------------------------------------------------------------------

    def create_run(self, run: RunRecord) -> None:
        row = run.model_dump()
        row["runners"] = json.dumps(row["runners"])
        self._insert("runs", row)

    def finish_run(
        self,
        run_id: str,
        *,
        status: str,
        finished_at: str,
        spent_usd: float,
        reserved_usd: float = 0,
        uncertain_cost: bool = False,
    ) -> None:
        with self.conn:
            self.conn.execute(
                "UPDATE runs SET status = ?, finished_at = ?, spent_usd = ?, reserved_usd = ?, "
                "uncertain_cost = ? WHERE id = ?",
                (status, finished_at, spent_usd, reserved_usd, uncertain_cost, run_id),
            )

    def add_result(self, result: ResultRecord) -> int:
        row = result.model_dump()
        row["grade_details"] = json.dumps(row["grade_details"]) if row["grade_details"] else None
        return self._insert("results", row)

    def _insert(self, table: str, row: dict[str, Any]) -> int:
        cols = ", ".join(row)
        marks = ", ".join(f":{c}" for c in row)
        with self.conn:
            cur = self.conn.execute(f"INSERT INTO {table} ({cols}) VALUES ({marks})", row)
        return cur.lastrowid or 0

    # --- reads ----------------------------------------------------------------------------------

    def get_run(self, run_id: str) -> RunRecord | None:
        row = self.conn.execute("SELECT * FROM runs WHERE id = ?", (run_id,)).fetchone()
        return _run_from_row(row) if row else None

    def latest_run_id(self) -> str | None:
        row = self.conn.execute("SELECT id FROM runs ORDER BY started_at DESC LIMIT 1").fetchone()
        return row["id"] if row else None

    def list_runs(self, limit: int = 20) -> list[RunRecord]:
        rows = self.conn.execute(
            "SELECT * FROM runs ORDER BY started_at DESC LIMIT ?", (limit,)
        ).fetchall()
        return [_run_from_row(r) for r in rows]

    def results_for_run(self, run_id: str) -> list[ResultRecord]:
        rows = self.conn.execute(
            "SELECT * FROM results WHERE run_id = ? ORDER BY id", (run_id,)
        ).fetchall()
        return [_result_from_row(r) for r in rows]


def _run_from_row(row: sqlite3.Row) -> RunRecord:
    data = dict(row)
    data["runners"] = json.loads(data["runners"])
    return RunRecord.model_validate(data)


def _result_from_row(row: sqlite3.Row) -> ResultRecord:
    data = dict(row)
    data.pop("id")
    data["passed"] = None if data["passed"] is None else bool(data["passed"])
    data["grade_details"] = json.loads(data["grade_details"]) if data["grade_details"] else None
    return ResultRecord.model_validate(data)
