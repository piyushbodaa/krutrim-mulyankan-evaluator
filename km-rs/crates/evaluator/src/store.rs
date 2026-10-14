//! SQLite results store — the same schema (v3) and file format as store.py,
//! so Python and Rust runs share one results.db.

use rusqlite::{params, Connection};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: i64 = 3;

const RESULT_COLUMNS_V2: &str = "id, run_id, task_id, task_version, task_hash, category, runner, runner_kind, seed, status, output, error, latency_s, input_tokens, output_tokens, cost_usd, cost_inr, steps, tool_calls, grader, grade_status, passed, score, grade_details, human_score, human_note, artifact_path, created_at";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS runs (
    id TEXT PRIMARY KEY, started_at TEXT NOT NULL, finished_at TEXT,
    status TEXT NOT NULL, suite_name TEXT NOT NULL, suite_version TEXT NOT NULL,
    seed INTEGER NOT NULL, repeats INTEGER NOT NULL DEFAULT 1,
    runners TEXT NOT NULL, max_usd REAL NOT NULL,
    spent_usd REAL NOT NULL DEFAULT 0, reserved_usd REAL NOT NULL DEFAULT 0,
    uncertain_cost INTEGER NOT NULL DEFAULT 0, usd_to_inr REAL NOT NULL,
    artifacts_dir TEXT NOT NULL, mj_version TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS results (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL, task_version INTEGER NOT NULL, task_hash TEXT NOT NULL,
    category TEXT NOT NULL, runner TEXT NOT NULL, runner_kind TEXT NOT NULL,
    seed INTEGER NOT NULL, repeat_index INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL, output TEXT, error TEXT, latency_s REAL,
    input_tokens INTEGER, output_tokens INTEGER, cost_usd REAL, cost_inr REAL,
    steps INTEGER, tool_calls INTEGER, grader TEXT, grade_status TEXT,
    passed INTEGER, score REAL, grade_details TEXT, human_score REAL,
    human_note TEXT, artifact_path TEXT, created_at TEXT NOT NULL,
    UNIQUE (run_id, task_id, runner, repeat_index)
);
CREATE INDEX IF NOT EXISTS results_runner ON results (runner, task_id);
CREATE TABLE IF NOT EXISTS comparisons (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL, runner_a TEXT NOT NULL, runner_b TEXT NOT NULL,
    winner TEXT NOT NULL, votes TEXT NOT NULL, judge_model TEXT, details TEXT,
    created_at TEXT NOT NULL
);
";

#[derive(Debug, Clone)]
pub struct RunRecord {
    pub id: String,
    pub started_at: String,
    pub status: String,
    pub suite_name: String,
    pub suite_version: String,
    pub seed: i64,
    pub runners: Vec<String>,
    pub max_usd: f64,
    pub usd_to_inr: f64,
    pub artifacts_dir: String,
    pub mj_version: String,
    pub finished_at: Option<String>,
    pub spent_usd: f64,
    pub reserved_usd: f64,
    pub uncertain_cost: bool,
    pub repeats: i64,
}

#[derive(Debug, Clone)]
pub struct ResultRecord {
    pub run_id: String,
    pub task_id: String,
    pub task_version: i64,
    pub task_hash: String,
    pub category: String,
    pub runner: String,
    pub runner_kind: String,
    pub seed: i64,
    pub status: String,
    pub created_at: String,
    pub repeat_index: i64,
    pub output: Option<String>,
    pub error: Option<String>,
    pub latency_s: Option<f64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub cost_inr: Option<f64>,
    pub steps: Option<i64>,
    pub tool_calls: Option<i64>,
    pub grader: Option<String>,
    pub grade_status: Option<String>,
    pub passed: Option<bool>,
    pub score: Option<f64>,
    pub grade_details: Option<Map<String, Value>>,
    pub human_score: Option<f64>,
    pub human_note: Option<String>,
    pub artifact_path: Option<String>,
}

pub struct Store {
    pub path: PathBuf,
    conn: Connection,
}

type SResult<T> = Result<T, String>;

fn db_err(e: rusqlite::Error) -> String {
    e.to_string()
}

impl Store {
    pub fn open(path: &Path) -> SResult<Store> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        let conn = Connection::open(path).map_err(db_err)?;
        conn.execute("PRAGMA foreign_keys = ON", [])
            .map_err(db_err)?;
        let mut store = Store {
            path: path.to_path_buf(),
            conn,
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> SResult<()> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(db_err)?;
        if version > SCHEMA_VERSION {
            return Err(format!(
                "{} has schema v{version}; this km knows v{SCHEMA_VERSION}",
                self.path.display()
            ));
        }
        if version == SCHEMA_VERSION {
            return Ok(());
        }
        let mut migration = String::new();
        let mut restore = String::new();
        if version == 1 || version == 2 {
            if version == 1 {
                migration.push_str(
                    "ALTER TABLE runs ADD COLUMN reserved_usd REAL NOT NULL DEFAULT 0;
                     ALTER TABLE runs ADD COLUMN uncertain_cost INTEGER NOT NULL DEFAULT 0;",
                );
            }
            migration.push_str(
                "ALTER TABLE runs ADD COLUMN repeats INTEGER NOT NULL DEFAULT 1;
                 ALTER TABLE results RENAME TO results_old;
                 DROP INDEX IF EXISTS results_runner;",
            );
            restore = format!(
                "INSERT INTO results ({RESULT_COLUMNS_V2}) SELECT {RESULT_COLUMNS_V2} \
                 FROM results_old; DROP TABLE results_old;"
            );
        }
        let sql = format!(
            "BEGIN IMMEDIATE; {migration} {SCHEMA} {restore} \
             PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"
        );
        self.conn.execute_batch(&sql).map_err(db_err)
    }

    pub fn create_run(&self, run: &RunRecord) -> SResult<()> {
        self.conn
            .execute(
                "INSERT INTO runs (id, started_at, finished_at, status, suite_name, \
                 suite_version, seed, repeats, runners, max_usd, spent_usd, reserved_usd, \
                 uncertain_cost, usd_to_inr, artifacts_dir, mj_version) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                params![
                    run.id,
                    run.started_at,
                    run.finished_at,
                    run.status,
                    run.suite_name,
                    run.suite_version,
                    run.seed,
                    run.repeats,
                    serde_json::to_string(&run.runners).unwrap_or_default(),
                    run.max_usd,
                    run.spent_usd,
                    run.reserved_usd,
                    run.uncertain_cost as i64,
                    run.usd_to_inr,
                    run.artifacts_dir,
                    run.mj_version,
                ],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn finish_run(
        &self,
        run_id: &str,
        status: &str,
        finished_at: &str,
        spent_usd: f64,
        reserved_usd: f64,
        uncertain_cost: bool,
    ) -> SResult<()> {
        self.conn
            .execute(
                "UPDATE runs SET status = ?, finished_at = ?, spent_usd = ?, \
                 reserved_usd = ?, uncertain_cost = ? WHERE id = ?",
                params![
                    status,
                    finished_at,
                    spent_usd,
                    reserved_usd,
                    uncertain_cost as i64,
                    run_id
                ],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn add_result(&self, r: &ResultRecord) -> SResult<i64> {
        let details = r
            .grade_details
            .as_ref()
            .map(|d| serde_json::to_string(&Value::Object(d.clone())).unwrap_or_default());
        self.conn
            .execute(
                "INSERT INTO results (run_id, task_id, task_version, task_hash, category, \
                 runner, runner_kind, seed, repeat_index, status, output, error, latency_s, \
                 input_tokens, output_tokens, cost_usd, cost_inr, steps, tool_calls, grader, \
                 grade_status, passed, score, grade_details, human_score, human_note, \
                 artifact_path, created_at) VALUES \
                 (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,\
                  ?21,?22,?23,?24,?25,?26,?27,?28)",
                params![
                    r.run_id,
                    r.task_id,
                    r.task_version,
                    r.task_hash,
                    r.category,
                    r.runner,
                    r.runner_kind,
                    r.seed,
                    r.repeat_index,
                    r.status,
                    r.output,
                    r.error,
                    r.latency_s,
                    r.input_tokens,
                    r.output_tokens,
                    r.cost_usd,
                    r.cost_inr,
                    r.steps,
                    r.tool_calls,
                    r.grader,
                    r.grade_status,
                    r.passed.map(|b| b as i64),
                    r.score,
                    details,
                    r.human_score,
                    r.human_note,
                    r.artifact_path,
                    r.created_at,
                ],
            )
            .map_err(db_err)?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_run(&self, run_id: &str) -> SResult<Option<RunRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM runs WHERE id = ?")
            .map_err(db_err)?;
        let mut rows = stmt.query(params![run_id]).map_err(db_err)?;
        match rows.next().map_err(db_err)? {
            Some(row) => Ok(Some(run_from_row(row)?)),
            None => Ok(None),
        }
    }

    pub fn latest_run_id(&self) -> SResult<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM runs ORDER BY started_at DESC LIMIT 1")
            .map_err(db_err)?;
        let mut rows = stmt.query([]).map_err(db_err)?;
        match rows.next().map_err(db_err)? {
            Some(row) => Ok(Some(row.get(0).map_err(db_err)?)),
            None => Ok(None),
        }
    }

    pub fn list_runs(&self, limit: i64) -> SResult<Vec<RunRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM runs ORDER BY started_at DESC LIMIT ?")
            .map_err(db_err)?;
        let mut rows = stmt.query(params![limit]).map_err(db_err)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(db_err)? {
            out.push(run_from_row(row)?);
        }
        Ok(out)
    }

    pub fn results_for_run(&self, run_id: &str) -> SResult<Vec<ResultRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM results WHERE run_id = ? ORDER BY id")
            .map_err(db_err)?;
        let mut rows = stmt.query(params![run_id]).map_err(db_err)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(db_err)? {
            out.push(result_from_row(row)?);
        }
        Ok(out)
    }
}

fn run_from_row(row: &rusqlite::Row) -> SResult<RunRecord> {
    let g = |name: &str| row.get::<_, rusqlite::types::Value>(name).map_err(db_err);
    let runners_raw: String = row.get("runners").map_err(db_err)?;
    let uncertain: i64 = row.get("uncertain_cost").unwrap_or(0);
    let _ = g;
    Ok(RunRecord {
        id: row.get("id").map_err(db_err)?,
        started_at: row.get("started_at").map_err(db_err)?,
        finished_at: row.get("finished_at").ok(),
        status: row.get("status").map_err(db_err)?,
        suite_name: row.get("suite_name").map_err(db_err)?,
        suite_version: row.get("suite_version").map_err(db_err)?,
        seed: row.get("seed").map_err(db_err)?,
        repeats: row.get("repeats").unwrap_or(1),
        runners: serde_json::from_str(&runners_raw).unwrap_or_default(),
        max_usd: row.get("max_usd").map_err(db_err)?,
        spent_usd: row.get("spent_usd").unwrap_or(0.0),
        reserved_usd: row.get("reserved_usd").unwrap_or(0.0),
        uncertain_cost: uncertain != 0,
        usd_to_inr: row.get("usd_to_inr").map_err(db_err)?,
        artifacts_dir: row.get("artifacts_dir").map_err(db_err)?,
        mj_version: row.get("mj_version").map_err(db_err)?,
    })
}

fn result_from_row(row: &rusqlite::Row) -> SResult<ResultRecord> {
    let passed_raw: Option<i64> = row.get("passed").ok();
    let details_raw: Option<String> = row.get("grade_details").ok();
    Ok(ResultRecord {
        run_id: row.get("run_id").map_err(db_err)?,
        task_id: row.get("task_id").map_err(db_err)?,
        task_version: row.get("task_version").map_err(db_err)?,
        task_hash: row.get("task_hash").map_err(db_err)?,
        category: row.get("category").map_err(db_err)?,
        runner: row.get("runner").map_err(db_err)?,
        runner_kind: row.get("runner_kind").map_err(db_err)?,
        seed: row.get("seed").map_err(db_err)?,
        repeat_index: row.get("repeat_index").unwrap_or(0),
        status: row.get("status").map_err(db_err)?,
        created_at: row.get("created_at").map_err(db_err)?,
        output: row.get("output").ok(),
        error: row.get("error").ok(),
        latency_s: row.get("latency_s").ok(),
        input_tokens: row.get("input_tokens").ok(),
        output_tokens: row.get("output_tokens").ok(),
        cost_usd: row.get("cost_usd").ok(),
        cost_inr: row.get("cost_inr").ok(),
        steps: row.get("steps").ok(),
        tool_calls: row.get("tool_calls").ok(),
        grader: row.get("grader").ok(),
        grade_status: row.get("grade_status").ok(),
        passed: passed_raw.map(|v| v != 0),
        score: row.get("score").ok(),
        grade_details: details_raw.and_then(|s| match serde_json::from_str::<Value>(&s) {
            Ok(Value::Object(m)) => Some(m),
            _ => None,
        }),
        human_score: row.get("human_score").ok(),
        human_note: row.get("human_note").ok(),
        artifact_path: row.get("artifact_path").ok(),
    })
}
