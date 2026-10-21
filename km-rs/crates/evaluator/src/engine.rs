//! Run engine, mirroring engine.py: preflight, per-attempt accounting in
//! the same order, manifests, artifacts, and the SQLite records.

use crate::graders::{grade_with_judge as run_grader, Grade};
use crate::judge::JudgeFn;
use crate::provenance::{redact, redact_str, source_metadata};
use crate::pypy::sha256_hex;
use crate::runners::{RunContext, RunOutput, Runner, Usage, VERSION};
use crate::settings::Settings;
use crate::store::{ResultRecord, RunRecord, Store};
use crate::suite::{Suite, Task};
use chrono::Local;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const MAX_REPEATS: i64 = 10;

#[derive(Debug, Clone)]
pub struct Estimate {
    pub task_id: String,
    pub runner: String,
    pub cost_usd: Option<f64>,
}

#[derive(Debug)]
pub struct Preflight {
    pub estimates: Vec<Estimate>,
    pub worst_case_usd: f64,
    pub problems: Vec<String>,
    pub warnings: Vec<String>,
}

impl Preflight {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }
}

pub struct PreflightError(pub Vec<String>);

impl std::fmt::Display for PreflightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.join("; "))
    }
}

pub fn estimate(tasks: &[Task], runners: &[Runner]) -> Vec<Estimate> {
    let mut out = Vec::new();
    for t in tasks {
        let prompt = t.render_prompt();
        for r in runners {
            out.push(Estimate {
                task_id: t.id.clone(),
                runner: r.name.clone(),
                cost_usd: r.estimate_cost_usd(t, &prompt),
            });
        }
    }
    out
}

pub fn preflight(tasks: &[Task], runners: &[Runner], cap: f64, repeats: i64) -> Preflight {
    let estimates = estimate(tasks, runners);
    let mut problems: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    if !cap.is_finite() || cap < 0.0 {
        problems.push("budget cap must be finite and nonnegative".into());
    }
    if !(1..=MAX_REPEATS).contains(&repeats) {
        problems.push(format!("repeats must be between 1 and {MAX_REPEATS}"));
    }
    for runner in runners {
        let own = runner.problems();
        let unknown = estimates
            .iter()
            .filter(|e| e.runner == runner.name && e.cost_usd.is_none())
            .count();
        problems.extend(own.iter().map(|p| format!("{}: {p}", runner.name)));
        if unknown > 0 && own.is_empty() {
            problems.push(format!(
                "{}: no cost estimate for {unknown} attempt(s)",
                runner.name
            ));
        }
    }
    if estimates
        .iter()
        .filter_map(|e| e.cost_usd)
        .any(|c| !c.is_finite() || c < 0.0)
    {
        problems.push("cost estimates must be finite and nonnegative".into());
    }
    let mut unsupported: Vec<&str> = tasks
        .iter()
        .map(|t| t.capability_track.as_str())
        .filter(|t| *t != "text" && *t != "browser_workflow")
        .collect();
    unsupported.sort();
    unsupported.dedup();
    if !unsupported.is_empty() {
        problems.push(format!(
            "unsupported capability tracks: [{}]",
            unsupported
                .iter()
                .map(|s| format!("'{s}'"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let over: Vec<&Estimate> = estimates
        .iter()
        .filter(|e| {
            e.cost_usd
                .and_then(|c| {
                    tasks
                        .iter()
                        .find(|t| t.id == e.task_id)
                        .and_then(|t| t.budget_usd)
                        .map(|b| c > b)
                })
                .unwrap_or(false)
        })
        .collect();
    if !over.is_empty() {
        warnings.push(format!(
            "{} attempt(s) exceed their task's budget_usd at worst case and will be skipped \
             (lower max_output_tokens or raise budget_usd)",
            over.len()
        ));
    }
    let over_keys: Vec<(&str, &str)> = over
        .iter()
        .map(|e| (e.task_id.as_str(), e.runner.as_str()))
        .collect();
    let worst = repeats as f64
        * estimates
            .iter()
            .filter(|e| !over_keys.contains(&(e.task_id.as_str(), e.runner.as_str())))
            .filter_map(|e| e.cost_usd)
            .sum::<f64>();
    if worst > cap {
        problems.push(format!(
            "worst-case cost ${worst:.4} exceeds the ${cap:.2} cap; raise --max-usd, \
             lower max_output_tokens, or run fewer tasks, runners or repeats"
        ));
    }
    Preflight {
        estimates,
        worst_case_usd: worst,
        problems,
        warnings,
    }
}

pub fn derive_seed(run_seed: i64, task_id: &str, repeat_index: i64) -> i64 {
    let key = if repeat_index == 0 {
        format!("{run_seed}:{task_id}")
    } else {
        format!("{run_seed}:{task_id}:{repeat_index}")
    };
    let hex = sha256_hex(key.as_bytes());
    let bytes = (0..4)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0))
        .collect::<Vec<_>>();
    i64::from(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

pub fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut in_bad_run = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
            in_bad_run = false;
        } else if !in_bad_run {
            out.push('_');
            in_bad_run = true;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "_".into()
    } else {
        trimmed.to_string()
    }
}

pub fn now_iso() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S%.6f%:z").to_string()
}

pub fn new_run_dir(runs_dir: &Path) -> std::io::Result<(String, PathBuf)> {
    let base = Local::now().format("%Y%m%d-%H%M%S").to_string();
    std::fs::create_dir_all(runs_dir)?;
    let (mut run_id, mut n) = (base.clone(), 1i64);
    loop {
        let dir = runs_dir.join(&run_id);
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok((run_id, dir)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                run_id = format!("{base}-{n}");
                n += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

#[derive(Default)]
#[allow(clippy::type_complexity)]
pub struct RunCallbacks<'a> {
    pub on_start: Option<&'a dyn Fn(&str)>,
    pub on_attempt: Option<&'a dyn Fn(&str, &str)>,
    pub on_result: Option<&'a dyn Fn(&ResultRecord)>,
    /// Grading judge for `llm_judge` tasks; None leaves them pending.
    pub judge: Option<&'a JudgeFn>,
}

pub struct RunSummary {
    pub run_id: String,
    pub run_dir: PathBuf,
    pub status: String,
    pub spent_usd: f64,
    pub results: Vec<ResultRecord>,
}

/// One attempt per (repeat, task, runner), in the order results are stored.
struct Planned<'a> {
    repeat_index: i64,
    task: &'a Task,
    prompt: &'a str,
    runner: &'a Runner,
    runner_index: usize,
}

/// What one attempt produced: output, latency, run status, grade.
type Outcome = (RunOutput, Option<f64>, &'static str, Option<Grade>);

/// Ask the runner and grade the reply. Runs on a worker thread when the run
/// is parallel, so it touches nothing shared but its arguments.
fn execute(
    runner: &Runner,
    task: &Task,
    prompt: &str,
    ctx: &RunContext,
    fixture_base: &Path,
    judge: Option<&JudgeFn>,
) -> Outcome {
    let (out, latency) = attempt(runner, task, prompt, ctx);
    if ctx.workdir != fixture_base {
        let _ = std::fs::remove_dir_all(&ctx.workdir);
    }
    let run_status = if out.error.is_some() { "error" } else { "ok" };
    let grade = if run_status == "ok" {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_grader(task, &out.output, fixture_base, judge)
        })) {
            Ok(g) => Some(g),
            Err(_) => Some(Grade::error(
                task.grader.kind(),
                "Panic: grader panicked".into(),
            )),
        }
    } else {
        None
    };
    (out, latency, run_status, grade)
}

fn skipped(reason: String) -> Outcome {
    let out = RunOutput {
        error: Some(reason),
        usage: Usage {
            cost_usd: Some(0.0),
            ..Default::default()
        },
        ..Default::default()
    };
    (out, None, "skipped", None)
}

/// How many of one runner's answers may be in flight at once. Free
/// OpenRouter models share tight rate limits; local harnesses and recorded
/// app observations run one at a time.
fn runner_parallelism(runner: &Runner) -> usize {
    match runner.model_state() {
        Some(m) if m.model.ends_with(":free") => 2,
        Some(_) => 4,
        None if runner.kind == crate::runners::RunnerKind::Fake => usize::MAX,
        None => 1,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_suite(
    suite: &Suite,
    runners: &[Runner],
    store: &Store,
    settings: &Settings,
    seed: i64,
    repeats: i64,
    max_usd: Option<f64>,
    task_ids: Option<&[String]>,
    callbacks: &RunCallbacks,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<RunSummary, PreflightError> {
    run_suite_with(
        suite, runners, store, settings, seed, repeats, max_usd, task_ids, callbacks, cancel, 1,
    )
}

/// `run_suite`, asking up to `concurrency` questions at once. Results are
/// still stored in (repeat, task, runner) order, so a parallel run exports
/// the same file a one-at-a-time run would. The spending cap is checked
/// before each call against what is spent plus every call still in flight
/// at its worst case, so parallel calls can never overshoot it together.
/// `concurrency` 1 is exactly the one-at-a-time run.
#[allow(clippy::too_many_arguments)]
pub fn run_suite_with(
    suite: &Suite,
    runners: &[Runner],
    store: &Store,
    settings: &Settings,
    seed: i64,
    repeats: i64,
    max_usd: Option<f64>,
    task_ids: Option<&[String]>,
    callbacks: &RunCallbacks,
    cancel: Option<Arc<AtomicBool>>,
    concurrency: usize,
) -> Result<RunSummary, PreflightError> {
    let tasks = suite
        .select(task_ids)
        .map_err(|e| PreflightError(vec![e]))?;
    let unique: std::collections::HashSet<&str> = runners.iter().map(|r| r.name.as_str()).collect();
    if unique.len() != runners.len() || runners.is_empty() {
        return Err(PreflightError(vec![
            "runners must be nonempty and unique".into()
        ]));
    }
    let cap = max_usd.unwrap_or(settings.max_usd);
    let check = preflight(&tasks, runners, cap, repeats);
    if !check.ok() {
        return Err(PreflightError(check.problems));
    }
    let (run_id, run_dir) =
        new_run_dir(&settings.runs_dir).map_err(|e| PreflightError(vec![e.to_string()]))?;

    if let Some(suite_path) = &suite.path {
        let _ = std::fs::copy(suite_path, run_dir.join("suite.yaml"));
        let base = suite_path.parent().unwrap_or(Path::new("."));
        let mut fixtures: Vec<&PathBuf> = tasks.iter().flat_map(|t| &t.fixtures).collect();
        fixtures.sort();
        fixtures.dedup();
        for f in fixtures {
            let dest = run_dir.join(f);
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(base.join(f), &dest);
        }
    }
    let started_at = now_iso();
    let suite_sha = match &suite.path {
        Some(p) => crate::pypy::sha256_file(p).unwrap_or_default(),
        None => sha256_hex(
            serde_json::to_string(&suite.dump_json())
                .unwrap_or_default()
                .as_bytes(),
        ),
    };
    let mut manifest = build_manifest(
        suite,
        &tasks,
        runners,
        &run_id,
        &started_at,
        seed,
        repeats,
        cap,
        settings,
        &suite_sha,
    );
    write_json_pretty(&run_dir.join("manifest.json"), &manifest);

    let run_record = RunRecord {
        id: run_id.clone(),
        started_at: started_at.clone(),
        status: "running".into(),
        suite_name: suite.name.clone(),
        suite_version: suite.version.clone(),
        seed,
        runners: runners.iter().map(|r| r.name.clone()).collect(),
        max_usd: cap,
        usd_to_inr: settings.usd_to_inr,
        artifacts_dir: run_dir.to_string_lossy().into_owned(),
        mj_version: VERSION.into(),
        finished_at: None,
        spent_usd: 0.0,
        reserved_usd: 0.0,
        uncertain_cost: false,
        repeats,
    };
    let _ = store.create_run(&run_record);

    let fixture_base: PathBuf = suite
        .path
        .as_ref()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let (mut spent, mut reserved) = (0.0f64, 0.0f64);
    let mut uncertain = false;
    let mut status = "completed".to_string();
    let mut results: Vec<ResultRecord> = Vec::new();
    if let Some(cb) = &callbacks.on_start {
        cb(&run_id);
    }
    let prompts: Vec<String> = tasks.iter().map(|t| t.render_prompt()).collect();
    let mut planned: Vec<Planned> = Vec::new();
    for repeat_index in 0..repeats {
        for (task, prompt) in tasks.iter().zip(&prompts) {
            for (runner_index, runner) in runners.iter().enumerate() {
                planned.push(Planned {
                    repeat_index,
                    task,
                    prompt,
                    runner,
                    runner_index,
                });
            }
        }
    }
    let cancelled = || {
        cancel
            .as_ref()
            .map(|c| c.load(Ordering::SeqCst))
            .unwrap_or(false)
    };
    // Why an attempt must not call anything, checked just before it would:
    // a cancel, the task's own budget, or the run cap with every call still
    // in flight counted at its worst case.
    let skip_reason = |task: &Task,
                       est: Option<f64>,
                       committed: f64,
                       spent: f64,
                       status: &mut String|
     -> Option<String> {
        if cancelled() {
            *status = "cancelled".into();
            Some("Cancelled before this attempt; no adapter call made".into())
        } else if task
            .budget_usd
            .zip(est)
            .map(|(b, e)| e > b)
            .unwrap_or(false)
        {
            Some(format!(
                "estimate ${:.4} exceeds task budget ${:.4}",
                est.unwrap_or(0.0),
                task.budget_usd.unwrap_or(0.0)
            ))
        } else if committed + est.unwrap_or(0.0) > cap {
            *status = "budget_exceeded".into();
            Some(format!(
                "run budget cap ${cap:.2} reached (spent ${spent:.4})"
            ))
        } else {
            None
        }
    };
    let context = |p: &Planned| RunContext {
        workdir: make_workdir(&fixture_base, p.task),
        seed: derive_seed(seed, &p.task.id, p.repeat_index),
        timeout_s: p.task.timeout_s,
        cancel: cancel.clone(),
    };
    // Account for one finished attempt and build its record. Storing it is
    // left to the caller, which keeps the fixed order.
    let finish = |p: &Planned,
                  est: Option<f64>,
                  outcome: Outcome,
                  spent: &mut f64,
                  reserved: &mut f64,
                  uncertain: &mut bool,
                  status: &mut String|
     -> ResultRecord {
        let (mut out, latency, run_status, grade) = outcome;
        if out.usage.cost_usd.is_none()
            && est == Some(0.0)
            && p.runner.kind != crate::runners::RunnerKind::Harness
        {
            out.usage.cost_usd = Some(0.0);
        }
        *spent += out.usage.cost_usd.unwrap_or(0.0);
        *reserved += out.usage.cost_usd.or(est).unwrap_or(0.0);
        *uncertain |= out.usage.cost_usd.is_none();
        if *reserved > cap {
            *status = "budget_exceeded".into();
        }
        let mut record = make_record(
            &run_id,
            p.task,
            p.runner,
            derive_seed(seed, &p.task.id, p.repeat_index),
            run_status,
            &out,
            latency,
            grade.as_ref(),
            settings,
        );
        record.repeat_index = p.repeat_index;
        let artifact = save_artifact(
            &run_dir,
            p.task,
            p.runner,
            p.prompt,
            &out,
            &record,
            grade.as_ref(),
        );
        record.artifact_path = Some(artifact.to_string_lossy().into_owned());
        record
    };

    if concurrency <= 1 {
        for p in &planned {
            let est = p.runner.estimate_cost_usd(p.task, p.prompt);
            let outcome = match skip_reason(p.task, est, reserved, spent, &mut status) {
                Some(reason) => skipped(reason),
                None => {
                    if let Some(cb) = &callbacks.on_attempt {
                        cb(&p.task.id, &p.runner.name);
                    }
                    let outcome = execute(
                        p.runner,
                        p.task,
                        p.prompt,
                        &context(p),
                        &fixture_base,
                        callbacks.judge,
                    );
                    if cancelled() {
                        status = "cancelled".into();
                    }
                    outcome
                }
            };
            let record = finish(
                p,
                est,
                outcome,
                &mut spent,
                &mut reserved,
                &mut uncertain,
                &mut status,
            );
            let _ = store.add_result(&record);
            if let Some(cb) = &callbacks.on_result {
                cb(&record);
            }
            results.push(record);
            // On cancel, remaining attempts are still recorded as skipped
            // by the check at the top of the loop (like Python).
        }
    } else {
        // Workers only ask and grade. Callbacks, accounting and storage stay
        // on this thread; finished records wait in `done` until every earlier
        // attempt is stored, so the export order never depends on timing.
        let total = planned.len();
        let mut launched = vec![false; total];
        let mut estimates: Vec<Option<f64>> = vec![None; total];
        let mut done: Vec<Option<ResultRecord>> = (0..total).map(|_| None).collect();
        let mut busy_per_runner = vec![0usize; runners.len()];
        let (mut busy, mut busy_usd) = (0usize, 0.0f64);
        let (mut cursor, mut stored) = (0usize, 0usize);
        std::thread::scope(|scope| {
            let (tx, rx) = std::sync::mpsc::channel::<(usize, Outcome)>();
            loop {
                let stop = cancelled();
                let mut index = cursor;
                while index < total && (stop || busy < concurrency) {
                    let p = &planned[index];
                    let full = busy_per_runner[p.runner_index] >= runner_parallelism(p.runner);
                    if launched[index] || (!stop && full) {
                        index += 1;
                        continue;
                    }
                    launched[index] = true;
                    let est = p.runner.estimate_cost_usd(p.task, p.prompt);
                    estimates[index] = est;
                    match skip_reason(p.task, est, reserved + busy_usd, spent, &mut status) {
                        Some(reason) => {
                            let record = finish(
                                p,
                                est,
                                skipped(reason),
                                &mut spent,
                                &mut reserved,
                                &mut uncertain,
                                &mut status,
                            );
                            if let Some(cb) = &callbacks.on_result {
                                cb(&record);
                            }
                            done[index] = Some(record);
                        }
                        None => {
                            if let Some(cb) = &callbacks.on_attempt {
                                cb(&p.task.id, &p.runner.name);
                            }
                            busy += 1;
                            busy_per_runner[p.runner_index] += 1;
                            busy_usd += est.unwrap_or(0.0);
                            let ctx = context(p);
                            let (tx, base, judge) =
                                (tx.clone(), fixture_base.as_path(), callbacks.judge);
                            let (runner, task, prompt) = (p.runner, p.task, p.prompt);
                            scope.spawn(move || {
                                let outcome =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        execute(runner, task, prompt, &ctx, base, judge)
                                    }))
                                    .unwrap_or_else(|_| {
                                        (
                                            RunOutput::error("Panic: attempt panicked".into()),
                                            None,
                                            "error",
                                            None,
                                        )
                                    });
                                let _ = tx.send((index, outcome));
                            });
                        }
                    }
                    index += 1;
                }
                while cursor < total && launched[cursor] {
                    cursor += 1;
                }
                while stored < total {
                    let Some(record) = done[stored].take() else {
                        break;
                    };
                    let _ = store.add_result(&record);
                    results.push(record);
                    stored += 1;
                }
                if stored == total {
                    break;
                }
                if busy == 0 {
                    continue;
                }
                let Ok((index, outcome)) = rx.recv() else {
                    break;
                };
                let p = &planned[index];
                busy -= 1;
                busy_per_runner[p.runner_index] -= 1;
                busy_usd -= estimates[index].unwrap_or(0.0);
                if cancelled() {
                    status = "cancelled".into();
                }
                let record = finish(
                    p,
                    estimates[index],
                    outcome,
                    &mut spent,
                    &mut reserved,
                    &mut uncertain,
                    &mut status,
                );
                if let Some(cb) = &callbacks.on_result {
                    cb(&record);
                }
                done[index] = Some(record);
            }
        });
    }
    let finished_at = now_iso();
    let _ = store.finish_run(&run_id, &status, &finished_at, spent, reserved, uncertain);
    if let Value::Object(ref mut m) = manifest {
        m.insert("status".into(), json!(status));
        m.insert("finished_at".into(), json!(finished_at));
        m.insert("known_spent_usd".into(), json!(spent));
        m.insert("reserved_usd".into(), json!(reserved));
        m.insert("uncertain_cost".into(), json!(uncertain));
    }
    write_json_pretty(&run_dir.join("manifest.json"), &manifest);
    Ok(RunSummary {
        run_id,
        run_dir,
        status,
        spent_usd: spent,
        results,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_manifest(
    suite: &Suite,
    tasks: &[Task],
    runners: &[Runner],
    run_id: &str,
    started_at: &str,
    seed: i64,
    repeats: i64,
    cap: f64,
    settings: &Settings,
    suite_sha: &str,
) -> Value {
    let task_entries: Vec<Value> = tasks
        .iter()
        .map(|t| {
            let mut fixtures = Map::new();
            if let Some(p) = &suite.path {
                let base = p.parent().unwrap_or(Path::new("."));
                for f in &t.fixtures {
                    if let Ok(h) = crate::pypy::sha256_file(&base.join(f)) {
                        fixtures.insert(f.to_string_lossy().replace('\\', "/"), json!(h));
                    }
                }
            }
            json!({
                "id": t.id, "version": t.version, "hash": t.content_hash(),
                "metadata": t.metadata_json(), "fixtures": fixtures})
        })
        .collect();
    let runner_entries: Vec<Value> = runners
        .iter()
        .map(|r| {
            let mut m = Map::new();
            m.insert("spec".into(), json!(r.name));
            m.extend(r.configuration());
            Value::Object(m)
        })
        .collect();
    json!({
        "schema_version": "1.0",
        "run_id": run_id,
        "started_at": started_at,
        "suite": {"name": suite.name, "version": suite.version,
                  "validation_status": suite.validation_status, "sha256": suite_sha},
        "tasks": task_entries,
        "runners": runner_entries,
        "seed": seed,
        "repeats": repeats,
        "max_usd": cap,
        "usd_to_inr": settings.usd_to_inr,
        "mj_version": VERSION,
        "python": null,
        "source": source_metadata(),
        "budget_policy": "Reserve worst-case cost when billing is unknown; paid retries disabled.",
    })
}

fn make_workdir(base: &std::path::Path, task: &Task) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("km-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    for fixture in &task.fixtures {
        let dest = dir.join(fixture);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::copy(base.join(fixture), &dest);
    }
    dir
}

fn attempt(
    runner: &Runner,
    task: &Task,
    prompt: &str,
    ctx: &RunContext,
) -> (RunOutput, Option<f64>) {
    let started_at = now_iso();
    let start = std::time::Instant::now();
    let mut out = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        runner.run(task, prompt, ctx)
    })) {
        Ok(o) => o,
        Err(_) => RunOutput::error(redact_str("Panic: runner panicked", &[])),
    };
    if let Some(err) = &out.error {
        out.error = Some(redact_str(err, &[]));
    }
    let latency = start.elapsed().as_secs_f64();
    out.raw.insert(
        "attempt_timing".into(),
        json!({
            "started_at": started_at,
            "finished_at": now_iso(),
            "wall_elapsed_s": latency,
            "scope": "adapter invocation including local overhead",
        }),
    );
    // Import latency is not app response time for manual observations.
    let config = runner.configuration();
    let latency = if config.get("evaluation_mode").and_then(|v| v.as_str()) == Some("manual_app") {
        config
            .get("app_context")
            .and_then(|c| c.get("observed_latency_s"))
            .and_then(|v| v.as_f64())
    } else {
        Some(latency)
    };
    (out, latency)
}

#[allow(clippy::too_many_arguments)]
fn make_record(
    run_id: &str,
    task: &Task,
    runner: &Runner,
    seed: i64,
    status: &str,
    out: &RunOutput,
    latency: Option<f64>,
    grade: Option<&Grade>,
    settings: &Settings,
) -> ResultRecord {
    ResultRecord {
        run_id: run_id.into(),
        task_id: task.id.clone(),
        task_version: task.version,
        task_hash: task.content_hash(),
        category: task.category.clone(),
        runner: runner.name.clone(),
        runner_kind: runner.kind.as_str().into(),
        seed,
        status: status.into(),
        created_at: now_iso(),
        repeat_index: 0,
        output: if status != "skipped" {
            Some(out.output.clone())
        } else {
            None
        },
        error: out.error.clone(),
        latency_s: latency,
        input_tokens: out.usage.input_tokens,
        output_tokens: out.usage.output_tokens,
        cost_usd: out.usage.cost_usd,
        cost_inr: out.usage.cost_usd.map(|c| c * settings.usd_to_inr),
        steps: out.steps,
        tool_calls: out.tool_calls,
        grader: grade.map(|g| g.grader.clone()),
        grade_status: grade.map(|g| g.status.clone()),
        passed: grade.and_then(|g| g.passed),
        score: grade.and_then(|g| g.score),
        grade_details: grade.map(|g| g.details.clone()),
        human_score: None,
        human_note: None,
        artifact_path: None,
    }
}

fn save_artifact(
    run_dir: &Path,
    task: &Task,
    runner: &Runner,
    prompt: &str,
    out: &RunOutput,
    record: &ResultRecord,
    grade: Option<&Grade>,
) -> PathBuf {
    let suffix = &sha256_hex(runner.name.as_bytes())[..10];
    let directory = if runner.kind == crate::runners::RunnerKind::Fake {
        slug(&runner.name)
    } else {
        format!("{}-{}", slug(&runner.name), suffix)
    };
    let mut name = slug(&task.id);
    if record.repeat_index != 0 {
        name += &format!(".repeat-{}", record.repeat_index);
    }
    let path = run_dir.join(&directory).join(format!("{name}.json"));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let payload = json!({
        "task": {"id": task.id, "version": task.version, "hash": record.task_hash},
        "runner": {"spec": runner.name, "kind": runner.kind.as_str()},
        "seed": record.seed,
        "repeat_index": record.repeat_index,
        "status": record.status,
        "latency_s": record.latency_s,
        "prompt": prompt,
        "result": out.dump_json(),
        "grade": grade.map(|g| g.dump_json()),
    });
    let redacted = redact(&payload);
    // Python: json.dumps(payload, indent=2, ensure_ascii=False) + "\n".
    let text = serde_json::to_string_pretty(&redacted).unwrap_or_default() + "\n";
    let _ = std::fs::write(&path, text);
    path
}

pub fn write_json_pretty(path: &Path, value: &Value) {
    let text = serde_json::to_string_pretty(value).unwrap_or_default() + "\n";
    let _ = std::fs::write(path, text);
}
