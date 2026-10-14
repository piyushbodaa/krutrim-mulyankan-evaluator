//! Loopback pilot workbench (`km ui`, Python `local_ui.py`, `jobs.py`,
//! `job_routes.py`, `comparisons.py`, `evidence_archive.py`). Manual app
//! observations and fixed free mock controls; paid APIs stay disabled.

use crate::claude_draft::ClaudeDraft;
use crate::observations::{AppObservation, AppObservationRunner};
use crate::pypy::sha256_hex;
use crate::store::Store;
use crate::suite::{load_suite, Suite, Task};
use crate::{make_runner, run_suite, Runner};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

fn utc_now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.6f%:z")
        .to_string()
}

fn workbench_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 15 || bytes[8] != b'-' {
        return false;
    }
    value[..8].chars().all(|c| c.is_ascii_digit())
        && value[9..15].chars().all(|c| c.is_ascii_digit())
        && (bytes.len() == 15
            || (bytes[15] == b'-' && value[16..].chars().all(|c| c.is_ascii_digit())))
}

fn str_field<'a>(value: Option<&'a Value>, label: &str) -> Result<&'a str, String> {
    value
        .and_then(Value::as_str)
        .ok_or_else(|| format!("field {label} must be a string"))
}

fn str_list(value: Option<&Value>, label: &str) -> Result<Vec<String>, String> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| format!("field {label} must be a list"))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("field {label} must contain strings"))
        })
        .collect()
}

fn hex64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn key_pattern(value: &str) -> bool {
    (8..=100).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn uuid_hex() -> String {
    let material = format!(
        "{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    sha256_hex(material.as_bytes())[..32].to_string()
}

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// ---------------- configuration registry ----------------

const ORIGINS: [&str; 3] = ["template", "user_created", "imported"];

#[derive(Clone, PartialEq)]
struct NamedConfiguration {
    id: String,
    name: String,
    app_context: Value,
    origin: String,
}

impl NamedConfiguration {
    fn from_value(value: &Value) -> Result<Self, String> {
        let obj = value
            .as_object()
            .ok_or_else(|| "configuration must be an object".to_string())?;
        for field in obj.keys() {
            if !matches!(field.as_str(), "id" | "name" | "app_context" | "origin") {
                return Err(format!("extra input is not permitted: {field}"));
            }
        }
        let id = str_field(obj.get("id"), "id")?.to_string();
        if id.is_empty()
            || id.len() > 80
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return Err("configuration id does not match the required pattern".into());
        }
        let name = str_field(obj.get("name"), "name")?.to_string();
        if name.is_empty() || name.chars().count() > 120 {
            return Err("configuration name must not be empty".into());
        }
        let app_context = obj
            .get("app_context")
            .ok_or_else(|| "configuration needs app_context".to_string())?
            .clone();
        crate::observations::validate_app_context(&app_context)?;
        let origin = obj
            .get("origin")
            .and_then(Value::as_str)
            .unwrap_or("user_created");
        if !ORIGINS.contains(&origin) {
            return Err("configuration origin must be template/user_created/imported".into());
        }
        Ok(Self {
            id,
            name,
            app_context,
            origin: origin.to_string(),
        })
    }

    fn to_value(&self) -> Value {
        json!({"id": self.id, "name": self.name, "app_context": self.app_context, "origin": self.origin})
    }
}

struct ConfigurationRegistry {
    path: PathBuf,
    lock: Mutex<()>,
}

impl ConfigurationRegistry {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    fn read_entries(&self) -> Result<Vec<NamedConfiguration>, String> {
        if self.path.is_file() {
            let text = std::fs::read_to_string(&self.path).map_err(|e| e.to_string())?;
            let data: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            if data.get("schema_version").and_then(Value::as_str) != Some("1.0") {
                return Err("unsupported configuration registry version".into());
            }
            let entries: Vec<NamedConfiguration> = data
                .get("configurations")
                .and_then(Value::as_array)
                .ok_or_else(|| "registry configurations must be a list".to_string())?
                .iter()
                .map(NamedConfiguration::from_value)
                .collect::<Result<_, _>>()?;
            if entries
                .iter()
                .map(|e| e.id.clone())
                .collect::<HashSet<_>>()
                .len()
                != entries.len()
            {
                return Err("duplicate configuration IDs in registry".into());
            }
            Ok(entries)
        } else {
            Ok([
                ("chatgpt-web", "ChatGPT"),
                ("claude-web", "Claude"),
                ("gemini-web", "Gemini"),
            ]
            .iter()
            .map(|(slug, app)| NamedConfiguration {
                id: slug.to_string(),
                name: format!("{app} · Web · plan unknown"),
                app_context: json!({"app": app, "client": "Web browser", "plan": "Unknown",
                    "app_url": null, "client_version": null, "model_label": null,
                    "observed_at": null, "observed_latency_s": null, "session": "unknown",
                    "observation_kind": "contributor_reported"}),
                origin: "template".into(),
            })
            .collect())
        }
    }

    fn write_entries(&self, entries: &[NamedConfiguration]) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let payload = json!({"schema_version": "1.0",
            "configurations": entries.iter().map(NamedConfiguration::to_value).collect::<Vec<_>>()});
        let text = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
        let parent = self.path.parent().unwrap_or(Path::new("."));
        let temp = parent.join(format!(".configurations-{}", uuid_hex()));
        std::fs::write(&temp, text).map_err(|e| e.to_string())?;
        if let Err(err) = std::fs::rename(&temp, &self.path) {
            let _ = std::fs::remove_file(&temp);
            return Err(err.to_string());
        }
        Ok(())
    }

    fn list(&self) -> Result<Vec<NamedConfiguration>, String> {
        let _guard = self.lock.lock().unwrap();
        let entries = self.read_entries()?;
        if !self.path.exists() {
            self.write_entries(&entries)?;
        }
        Ok(entries)
    }

    fn create(&self, name: &str, app_context: Value) -> Result<NamedConfiguration, String> {
        let config = NamedConfiguration {
            id: format!("config-{}", &uuid_hex()[..16]),
            name: name.to_string(),
            app_context,
            origin: "user_created".into(),
        };
        self.ensure(&[config.clone()], true)?;
        Ok(config)
    }

    fn ensure(&self, requested: &[NamedConfiguration], allow_new: bool) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap();
        let mut entries = self.read_entries()?;
        let mut by_id: HashMap<String, NamedConfiguration> =
            entries.iter().map(|e| (e.id.clone(), e.clone())).collect();
        for config in requested {
            if let Some(existing) = by_id.get(&config.id) {
                if config != existing {
                    return Err("configuration snapshot differs from saved configuration".into());
                }
            } else if !allow_new {
                return Err("unknown configuration; save it or import a comparison first".into());
            } else {
                entries.push(config.clone());
                by_id.insert(config.id.clone(), config.clone());
            }
        }
        self.write_entries(&entries)
    }
}

// ---------------- jobs ----------------

#[derive(Clone, PartialEq)]
struct JobRequest {
    api_version: String,
    suite: String,
    task_ids: Vec<String>,
    adapter_ids: Vec<String>,
    seed: u64,
    repeats: usize,
    idempotency_key: String,
}

impl JobRequest {
    fn from_value(value: &Value) -> Result<Self, String> {
        let obj = value
            .as_object()
            .ok_or_else(|| "job request must be an object".to_string())?;
        for field in obj.keys() {
            if !matches!(
                field.as_str(),
                "api_version"
                    | "suite"
                    | "task_ids"
                    | "adapter_ids"
                    | "seed"
                    | "repeats"
                    | "idempotency_key"
            ) {
                return Err(format!("extra input is not permitted: {field}"));
            }
        }
        let api_version = obj
            .get("api_version")
            .and_then(Value::as_str)
            .unwrap_or("1.0")
            .to_string();
        if api_version != "1.0" {
            return Err("input should be '1.0'".into());
        }
        let suite = obj
            .get("suite")
            .and_then(Value::as_str)
            .unwrap_or("india-pilot")
            .to_string();
        if !matches!(suite.as_str(), "india-pilot" | "railway-pilot") {
            return Err("suite must be india-pilot or railway-pilot".into());
        }
        let task_ids = str_list(obj.get("task_ids"), "task_ids")?;
        if task_ids.is_empty() || task_ids.len() > 24 {
            return Err("task selection must contain 1 to 24 tasks".into());
        }
        let adapter_ids = str_list(obj.get("adapter_ids"), "adapter_ids")?;
        if adapter_ids.is_empty() || adapter_ids.len() > 4 {
            return Err("select 1 to 4 adapters".into());
        }
        let seed = obj.get("seed").and_then(Value::as_u64).unwrap_or(12);
        if obj.get("seed").is_some_and(|s| !s.is_u64()) || seed > 2_147_483_647 {
            return Err("seed must be between 0 and 2147483647".into());
        }
        let repeats = obj.get("repeats").and_then(Value::as_u64).unwrap_or(1);
        if !(1..=5).contains(&repeats) {
            return Err("repeats must be between 1 and 5".into());
        }
        let idempotency_key = str_field(obj.get("idempotency_key"), "idempotency_key")?.to_string();
        if !key_pattern(&idempotency_key) {
            return Err("idempotency key does not match the required pattern".into());
        }
        if task_ids.iter().collect::<HashSet<_>>().len() != task_ids.len()
            || adapter_ids.iter().collect::<HashSet<_>>().len() != adapter_ids.len()
        {
            return Err("task and adapter selections must be unique".into());
        }
        Ok(Self {
            api_version,
            suite,
            task_ids,
            adapter_ids,
            seed,
            repeats: repeats as usize,
            idempotency_key,
        })
    }

    fn to_value(&self) -> Value {
        json!({"api_version": self.api_version, "suite": self.suite,
            "task_ids": self.task_ids, "adapter_ids": self.adapter_ids,
            "seed": self.seed, "repeats": self.repeats,
            "idempotency_key": self.idempotency_key})
    }
}

const TERMINAL: [&str; 4] = ["completed", "failed", "cancelled", "interrupted"];

fn state_request(state: &Value) -> Result<JobRequest, String> {
    JobRequest::from_value(state.get("request").unwrap_or(&Value::Null))
}

pub struct JobManager {
    parent: Arc<PilotService>,
    directory: PathBuf,
    states: Mutex<BTreeMap<String, Value>>,
    cancellations: Mutex<HashMap<String, Arc<AtomicBool>>>,
    sender: Mutex<Option<mpsc::Sender<(String, Vec<Runner>)>>>,
}

impl JobManager {
    fn new(parent: Arc<PilotService>) -> Result<Arc<Self>, String> {
        let directory = parent.settings.runs_dir.join("jobs");
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let manager = Arc::new(Self {
            parent,
            directory: directory.clone(),
            states: Mutex::new(BTreeMap::new()),
            cancellations: Mutex::new(HashMap::new()),
            sender: Mutex::new(None),
        });
        let mut loaded = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&directory) {
            for entry in entries.flatten() {
                let file = entry.path().join("job.json");
                if let Ok(text) = std::fs::read_to_string(&file) {
                    if let Ok(mut state) = serde_json::from_str::<Value>(&text) {
                        let id = state
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        if !id.is_empty() {
                            let active = !TERMINAL.contains(
                                &state.get("status").and_then(Value::as_str).unwrap_or(""),
                            );
                            if active {
                                state["status"] = json!("interrupted");
                                state["error"] = json!(
                                    "Server stopped before completion. No automatic retry was made."
                                );
                                state["finished_at"] = json!(utc_now());
                            }
                            loaded.push((id, state));
                        }
                    }
                }
            }
        }
        {
            let mut states = manager.states.lock().unwrap();
            let mut cancels = manager.cancellations.lock().unwrap();
            for (id, state) in loaded {
                if state.get("status").and_then(Value::as_str) == Some("interrupted") {
                    let _ = manager.save_value(&state);
                }
                cancels.insert(id.clone(), Arc::new(AtomicBool::new(false)));
                states.insert(id, state);
            }
        }
        let (tx, rx) = mpsc::channel::<(String, Vec<Runner>)>();
        *manager.sender.lock().unwrap() = Some(tx);
        let worker_manager = manager.clone();
        std::thread::spawn(move || {
            while let Ok((identity, runners)) = rx.recv() {
                worker_manager.execute(&identity, &runners);
            }
        });
        Ok(manager)
    }

    fn factory(&self, adapter_id: &str) -> Option<Runner> {
        let settings = &self.parent.settings;
        match adapter_id {
            "mock:oracle" => make_runner(
                "oracle",
                &crate::pricing::PriceTable::default(),
                &workbench_env(),
            )
            .ok(),
            "mock:echo" => make_runner(
                "echo",
                &crate::pricing::PriceTable::default(),
                &workbench_env(),
            )
            .ok(),
            "harness:claude-draft" => {
                Some(ClaudeDraft::new(settings.runs_dir.join("automation-access.json")).as_runner())
            }
            _ => None,
        }
    }

    fn save_value(&self, state: &Value) -> Result<(), String> {
        let id = state.get("id").and_then(Value::as_str).unwrap_or("");
        let directory = self.directory.join(id);
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let target = directory.join("job.json");
        let temp = directory.join("job.tmp");
        let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())? + "\n";
        std::fs::write(&temp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&temp, &target).map_err(|e| e.to_string())
    }

    fn save_state(&self, identity: &str) {
        let mut states = self.states.lock().unwrap();
        if let Some(state) = states.get_mut(identity) {
            state["updated_at"] = json!(utc_now());
            let _ = self.save_value(state);
        }
    }

    fn capabilities(&self) -> Value {
        let mut adapters = Vec::new();
        for (identity, title, track) in [
            ("mock:oracle", "Correct-answer control", "mock_validation"),
            ("mock:echo", "Echo control", "mock_validation"),
            (
                "harness:claude-draft",
                "Claude Code · tool-free draft",
                "harness",
            ),
        ] {
            let Some(runner) = self.factory(identity) else {
                continue;
            };
            let problems = runner.problems();
            adapters.push(json!({
                "id": identity, "name": title, "track": track,
                "ready": problems.is_empty(),
                "mode": if track == "mock_validation" { "mock" } else { "live" },
                "reason": if problems.is_empty() { "Ready".to_string() } else { problems.join("; ") },
                "max_tasks": if track == "harness" { 3 } else { 24 },
                "cost": if track == "mock_validation" { "Known zero API cost" } else { "Included-plan verified; actual charge unknown" },
            }));
        }
        for provider in [
            "anthropic",
            "openai",
            "gemini",
            "xai",
            "openrouter",
            "compatible",
        ] {
            adapters.push(json!({
                "id": format!("provider:{provider}"), "name": format!("{provider} API"),
                "track": "model_api", "ready": false, "mode": "live",
                "reason": "No approved paid budget; configured access/model/pricing unverified",
                "cost": "Unknown",
            }));
        }
        for runtime in ["ollama", "vllm"] {
            adapters.push(json!({
                "id": format!("local:{runtime}"), "name": format!("{runtime} local weights"),
                "track": "model_api", "ready": false, "mode": "live",
                "reason": "No running local server/loaded model verified",
                "cost": "Hardware/electricity excluded",
            }));
        }
        for cli in ["codex", "antigravity"] {
            adapters.push(json!({
                "id": format!("harness:{cli}"), "name": format!("{cli} CLI"),
                "track": "harness", "installed": which(cli), "ready": false, "mode": "live",
                "reason": "No-spend and tool boundaries need verification before enablement",
                "cost": "Unknown",
            }));
        }
        adapters.push(json!({
            "id": "consumer:manual", "name": "Consumer apps · recorded answers",
            "track": "consumer_app", "ready": false, "mode": "manual_app",
            "reason": "Record answers manually; browser automation is not connected",
            "cost": "Unknown",
        }));
        json!({
            "api_version": "1.0", "adapters": adapters,
            "execution_policy": "One active job; explicit retries; paid APIs disabled",
            "request_schema_url": "/api/v1/schemas/job-request",
            "state_schema_url": "/api/v1/schemas/job-state",
        })
    }

    fn preview(&self, request: &JobRequest) -> Result<(Value, Vec<Runner>), String> {
        let suite = self.parent.suite(&request.suite)?.clone();
        let tasks = suite.select(Some(&request.task_ids))?;
        let runners: Vec<Runner> = request
            .adapter_ids
            .iter()
            .map(|id| self.factory(id))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                "Unsupported or gated adapter; inspect /api/v1/capabilities".to_string()
            })?;
        if request
            .adapter_ids
            .iter()
            .any(|a| a == "harness:claude-draft")
        {
            if request.suite != "india-pilot" || tasks.len() > 3 {
                return Err("Claude draft is limited to 1–3 frozen synthetic text tasks".into());
            }
            let cutoff = utc_now();
            let submitted: usize = self
                .states
                .lock()
                .unwrap()
                .values()
                .filter_map(|state| {
                    let created = state.get("created_at").and_then(Value::as_str)?;
                    let recent = within_hours(created, &cutoff, 3600);
                    let req = state_request(state).ok()?;
                    (recent && req.adapter_ids.iter().any(|a| a == "harness:claude-draft"))
                        .then_some(req.task_ids.len() * req.repeats)
                })
                .sum();
            if submitted + tasks.len() * request.repeats > 8 {
                return Err(
                    "Bounded CLI allowance reached: maximum 8 submitted attempts/hour".into(),
                );
            }
        }
        let check = crate::preflight(&tasks, &runners, 0.0, request.repeats as i64);
        if !check.ok() {
            return Err(check.problems.join("; "));
        }
        let mut modes: Vec<String> = runners
            .iter()
            .map(|runner| {
                runner
                    .configuration()
                    .get("evaluation_mode")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            })
            .collect();
        modes.sort();
        modes.dedup();
        Ok((
            json!({
                "api_version": "1.0",
                "attempts": tasks.len() * runners.len() * request.repeats,
                "repeats": request.repeats,
                "paid_api_budget_usd": 0,
                "actual_cost_known": runners.iter().all(|r| matches!(r.kind, crate::RunnerKind::Fake)),
                "modes": modes,
                "warnings": check.warnings,
                "tasks": tasks.iter().map(|t| json!({"id": t.id, "sha256": t.content_hash()})).collect::<Vec<_>>(),
                "configurations": runners.iter().map(|r| Value::Object(r.configuration())).collect::<Vec<_>>(),
            }),
            runners,
        ))
    }

    fn create(&self, request: JobRequest, parent_job_id: Option<&str>) -> Result<Value, WorkError> {
        {
            let states = self.states.lock().unwrap();
            for state in states.values() {
                if let Ok(existing) = state_request(state) {
                    if existing.idempotency_key == request.idempotency_key {
                        if existing != request {
                            return Err(WorkError::Bad(
                                "Idempotency key already has a different request".into(),
                            ));
                        }
                        return Ok(state.clone());
                    }
                }
            }
            if states
                .values()
                .filter(|s| {
                    !TERMINAL.contains(&s.get("status").and_then(Value::as_str).unwrap_or(""))
                })
                .count()
                >= 4
            {
                return Err(WorkError::Conflict(
                    "Local queue is full; wait for a job or cancel one".into(),
                ));
            }
        }
        let (_, runners) = self.preview(&request).map_err(WorkError::Bad)?;
        let identity = uuid_hex();
        let total = request.task_ids.len() * runners.len() * request.repeats;
        let now = utc_now();
        let state = json!({
            "api_version": "1.0", "id": identity, "request": request.to_value(),
            "status": "queued", "created_at": now, "updated_at": now,
            "started_at": null, "finished_at": null, "run_id": null,
            "parent_job_id": parent_job_id, "total": total, "processed": 0,
            "execution_errors": 0, "grader_errors": 0, "pending": 0,
            "graded": 0, "passed": 0, "skipped": 0, "error": null,
            "events": [], "result_url": null, "private_result_url": null,
        });
        self.states
            .lock()
            .unwrap()
            .insert(identity.clone(), state.clone());
        self.cancellations
            .lock()
            .unwrap()
            .insert(identity.clone(), Arc::new(AtomicBool::new(false)));
        self.save_value(&state).map_err(WorkError::Bad)?;
        if let Some(sender) = self.sender.lock().unwrap().as_ref() {
            let _ = sender.send((identity, runners));
        }
        Ok(state)
    }

    fn get(&self, identity: &str) -> Result<Value, String> {
        if identity.len() != 32
            || !identity
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("unknown job".into());
        }
        self.states
            .lock()
            .unwrap()
            .get(identity)
            .cloned()
            .ok_or_else(|| "unknown job".to_string())
    }

    fn list(&self) -> Value {
        let mut values: Vec<Value> = self.states.lock().unwrap().values().cloned().collect();
        values.sort_by(|a, b| {
            b.get("created_at")
                .and_then(Value::as_str)
                .unwrap_or("")
                .cmp(a.get("created_at").and_then(Value::as_str).unwrap_or(""))
        });
        values.truncate(50);
        Value::Array(values)
    }

    fn cancel(&self, identity: &str) -> Result<Value, String> {
        let _ = self.get(identity)?;
        if let Some(flag) = self.cancellations.lock().unwrap().get(identity) {
            let active = {
                let states = self.states.lock().unwrap();
                states.get(identity).is_some_and(|s| {
                    !TERMINAL.contains(&s.get("status").and_then(Value::as_str).unwrap_or(""))
                })
            };
            if active {
                flag.store(true, Ordering::SeqCst);
                {
                    let mut states = self.states.lock().unwrap();
                    if let Some(state) = states.get_mut(identity) {
                        state["status"] = json!("cancelling");
                    }
                }
                self.save_state(identity);
            }
        }
        self.get(identity)
    }

    fn retry(&self, identity: &str, key: &str) -> Result<Value, WorkError> {
        let state = self.get(identity).map_err(WorkError::Bad)?;
        if !TERMINAL.contains(&state.get("status").and_then(Value::as_str).unwrap_or("")) {
            return Err(WorkError::Bad(
                "Wait for terminal status before retrying".into(),
            ));
        }
        let mut request = state_request(&state).map_err(WorkError::Bad)?;
        request.idempotency_key = key.to_string();
        if !key_pattern(key) {
            return Err(WorkError::Bad(
                "idempotency key does not match the required pattern".into(),
            ));
        }
        self.create(request, Some(identity))
    }

    fn result(&self, identity: &str, outputs: bool) -> Result<Value, String> {
        let state = self.get(identity)?;
        let run_id = state
            .get("run_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "No evaluator run exists yet".to_string())?;
        let store = Store::open(&self.parent.settings.db_path).map_err(|e| e)?;
        let mut payload = crate::export::result_export(&store, run_id, outputs)?.value;
        if outputs {
            let mut explanations = Map::new();
            for record in store.results_for_run(run_id).unwrap_or_default() {
                explanations.insert(
                    crate::export::explanation_key(
                        &record.task_id,
                        &record.runner,
                        record.repeat_index,
                    ),
                    record
                        .grade_details
                        .map(Value::Object)
                        .unwrap_or(Value::Null),
                );
            }
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("explanations".into(), Value::Object(explanations));
            }
        }
        Ok(payload)
    }

    fn execute(self: &Arc<Self>, identity: &str, runners: &[Runner]) {
        let cancel = self
            .cancellations
            .lock()
            .unwrap()
            .get(identity)
            .cloned()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let request = match self.get(identity).and_then(|state| state_request(&state)) {
            Ok(request) => request,
            Err(_) => return,
        };
        {
            if cancel.load(Ordering::SeqCst) {
                let mut states = self.states.lock().unwrap();
                if let Some(state) = states.get_mut(identity) {
                    state["status"] = json!("cancelled");
                    state["finished_at"] = json!(utc_now());
                }
                drop(states);
                self.save_state(identity);
                return;
            }
            let mut states = self.states.lock().unwrap();
            if let Some(state) = states.get_mut(identity) {
                state["status"] = json!("running");
                state["started_at"] = json!(utc_now());
            }
            drop(states);
            self.save_state(identity);
        }
        let manager = self.clone();
        let id_started = identity.to_string();
        let id_attempt = identity.to_string();
        let id_result = identity.to_string();
        let on_start = move |run_id: &str| {
            let mut states = manager.states.lock().unwrap();
            if let Some(state) = states.get_mut(&id_started) {
                state["run_id"] = json!(run_id);
                state["result_url"] = json!(format!("/api/v1/jobs/{id_started}/export"));
                state["private_result_url"] = json!(format!("/api/v1/jobs/{id_started}/results"));
            }
            drop(states);
            manager.save_state(&id_started);
        };
        let manager = self.clone();
        let on_attempt = move |task_id: &str, runner_id: &str| {
            let mut states = manager.states.lock().unwrap();
            if let Some(state) = states.get_mut(&id_attempt) {
                if let Some(events) = state.get_mut("events").and_then(Value::as_array_mut) {
                    events.push(json!({"at": utc_now(), "stage": "executing",
                        "task_id": task_id, "runner_id": runner_id,
                        "repeat_index": null, "status": null,
                        "grade_status": null, "passed": null}));
                }
            }
            drop(states);
            manager.save_state(&id_attempt);
        };
        let manager = self.clone();
        let on_result = move |record: &crate::ResultRecord| {
            let mut states = manager.states.lock().unwrap();
            if let Some(state) = states.get_mut(&id_result) {
                let bump = |state: &mut Value, field: &str| {
                    let next = state.get(field).and_then(Value::as_u64).unwrap_or(0) + 1;
                    state[field] = json!(next);
                };
                bump(state, "processed");
                if record.status == "error" {
                    bump(state, "execution_errors");
                }
                if record.grade_status.as_deref() == Some("error") {
                    bump(state, "grader_errors");
                }
                if record.grade_status.as_deref() == Some("pending") {
                    bump(state, "pending");
                }
                if record.status == "skipped" {
                    bump(state, "skipped");
                }
                if record.grade_status.as_deref() == Some("graded") {
                    bump(state, "graded");
                }
                if record.passed == Some(true) {
                    bump(state, "passed");
                }
                if let Some(events) = state.get_mut("events").and_then(Value::as_array_mut) {
                    events.push(json!({"at": utc_now(), "stage": "persisted",
                        "task_id": record.task_id, "runner_id": record.runner,
                        "repeat_index": record.repeat_index, "status": record.status,
                        "grade_status": record.grade_status, "passed": record.passed}));
                }
            }
            drop(states);
            manager.save_state(&id_result);
        };
        let callbacks = crate::RunCallbacks {
            on_start: Some(&on_start),
            on_attempt: Some(&on_attempt),
            on_result: Some(&on_result),
        };
        let settings = self.parent.settings.clone();
        let outcome = Store::open(&settings.db_path)
            .map_err(|e| e)
            .and_then(|store| {
                let suite = self.parent.suite(&request.suite)?.clone();
                let summary = run_suite(
                    &suite,
                    runners,
                    &store,
                    &settings,
                    request.seed as i64,
                    request.repeats as i64,
                    Some(0.0),
                    Some(&request.task_ids),
                    &callbacks,
                    Some(cancel.clone()),
                )
                .map_err(|e| e.to_string())?;
                let _ = crate::export::write_export(
                    &store,
                    &summary.run_id,
                    &summary.run_dir.join("results.json"),
                    false,
                );
                Ok(summary)
            });
        let mut states = self.states.lock().unwrap();
        if let Some(state) = states.get_mut(identity) {
            match outcome {
                Ok(summary) => {
                    let execution_errors = state
                        .get("execution_errors")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    state["status"] = json!(if summary.status == "cancelled" {
                        "cancelled"
                    } else if execution_errors > 0 {
                        "failed"
                    } else {
                        "completed"
                    });
                }
                Err(error) => {
                    state["status"] = json!("failed");
                    let cleaned = self.parent.scrub_text(&error);
                    state["error"] = json!(cleaned.chars().take(250).collect::<String>());
                }
            }
            state["finished_at"] = json!(utc_now());
        }
        drop(states);
        self.save_state(identity);
    }
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let candidate = dir.join(name);
                candidate.is_file()
            })
        })
        .unwrap_or(false)
}

fn within_hours(created: &str, _now: &str, seconds: i64) -> bool {
    // Compare against the current UTC instant via chrono.
    let created =
        chrono::DateTime::parse_from_rfc3339(created).map(|dt| dt.with_timezone(&chrono::Utc));
    match created {
        Ok(when) => (chrono::Utc::now() - when).num_seconds() < seconds,
        Err(_) => false,
    }
}

fn workbench_env() -> BTreeMap<String, String> {
    std::env::vars().collect()
}

#[derive(Debug)]
pub enum WorkError {
    Bad(String),
    Conflict(String),
    NotFound(String),
}

// ---------------- pilot service ----------------

const CATEGORIES: [(&str, &str); 7] = [
    ("rupee_numeracy", "Money & numbers"),
    ("india_scheduling", "Dates & schedules"),
    ("record_reconciliation", "Office records"),
    ("hindi_hinglish_instructions", "Hindi & Hinglish"),
    ("evidence_and_freshness", "Checking evidence"),
    ("robustness_and_abstention", "Avoiding mistakes"),
    ("tool-use", "Travel planning"),
];

const TITLES: [(&str, &str); 25] = [
    ("NUM01", "Add up an office budget"),
    ("NUM02", "Check an invoice total"),
    ("NUM03", "Calculate a balance in Hindi"),
    ("NUM04", "Choose a supplier within budget"),
    ("DATE01", "Convert an Indian meeting time to UTC"),
    ("DATE02", "Schedule a meeting in India"),
    ("DATE03", "Find a payment due date"),
    ("DATE04", "Spot an unclear date"),
    ("DOC01", "Remove duplicate invoices"),
    ("DOC02", "Reconcile an office delivery"),
    ("DOC03", "Read a Hindi meeting note"),
    ("DOC04", "Compare two supplier quotes"),
    ("LANG01", "Prioritise open tickets in Hindi"),
    ("LANG02", "Read a Hinglish deadline"),
    ("LANG03", "Check which files can be sent"),
    ("LANG04", "Follow the latest dispatch update"),
    ("EVID01", "Find the approved budget"),
    ("EVID02", "Check a time-zone reference"),
    ("EVID03", "Know when the evidence is missing"),
    ("EVID04", "Compare forecast and actual spend"),
    ("ROB01", "Ignore instructions hidden in an invoice"),
    ("ROB02", "Summarise without personal details"),
    ("ROB03", "Ask for missing information"),
    ("ROB04", "Spot an unreadable amount"),
    ("RAIL01", "Save a railway itinerary draft"),
];

fn title_for(id: &str) -> &str {
    TITLES
        .iter()
        .find(|(k, _)| *k == id)
        .map(|(_, v)| *v)
        .unwrap_or(id)
}

fn category_for(id: &str) -> &str {
    CATEGORIES
        .iter()
        .find(|(k, _)| *k == id)
        .map(|(_, v)| *v)
        .unwrap_or(id)
}

pub struct PilotService {
    pub project: PathBuf,
    pub settings: crate::Settings,
    suites: BTreeMap<String, Suite>,
    lock: Mutex<()>,
    registry: ConfigurationRegistry,
}

impl PilotService {
    pub fn new(project: &Path) -> Result<Self, String> {
        let project = project
            .canonicalize()
            .unwrap_or_else(|_| project.to_path_buf());
        let settings = crate::Settings {
            suites_dir: project.join("suites"),
            runs_dir: project.join("runs"),
            db_path: project.join("runs").join("results.db"),
            prices_path: project.join("prices.yaml"),
            max_usd: 0.0,
            usd_to_inr: 95.0,
        };
        let mut suites = BTreeMap::new();
        for name in ["india-pilot", "railway-pilot"] {
            let suite = load_suite(&settings.suites_dir.join(format!("{name}.yaml")))?;
            suites.insert(name.to_string(), suite);
        }
        let registry = ConfigurationRegistry::new(settings.runs_dir.join("configurations.json"));
        Ok(Self {
            project,
            settings,
            suites,
            lock: Mutex::new(()),
            registry,
        })
    }

    pub fn scrub_text(&self, text: &str) -> String {
        crate::provenance::redact_str(text, &[])
    }

    pub fn store(&self) -> Result<Store, String> {
        Store::open(&self.settings.db_path)
    }

    pub fn suite(&self, name: &str) -> Result<&Suite, String> {
        self.suites
            .get(name)
            .ok_or_else(|| format!("unknown suite: {name}"))
    }

    fn task_in(&self, suite: &str, task_id: &str) -> Result<Task, String> {
        self.suite(suite)?
            .tasks
            .iter()
            .find(|t| t.id == task_id)
            .cloned()
            .ok_or_else(|| format!("unknown task: {task_id}"))
    }

    pub fn tasks(&self) -> Vec<Value> {
        let mut result = Vec::new();
        for (name, suite) in &self.suites {
            for item in &suite.tasks {
                let spec = &item.grader;
                let expected = match spec {
                    crate::GraderSpec::Json { expected } => expected.clone(),
                    crate::GraderSpec::Workflow { expected, .. } => Value::Object(expected.clone()),
                    _ => Value::Null,
                };
                let marker =
                    "Use only the supplied context; do not browse or take external actions.";
                let mut instruction = item
                    .prompt
                    .rsplit(marker)
                    .next()
                    .unwrap_or(&item.prompt)
                    .trim()
                    .to_string();
                if item.id == "NUM01" {
                    instruction = "Work out the office budget after the one-time discount. \
                        Give the total amount and write it using Indian comma grouping."
                        .to_string();
                }
                let context_empty = matches!(&item.context, Value::Null)
                    || item.context.as_str().is_some_and(|s| s.is_empty())
                    || item.context.as_object().is_some_and(|o| o.is_empty());
                let context = if context_empty {
                    json!("The fictional train inventory is in the exact task prompt below.")
                } else {
                    item.context.clone()
                };
                let full_prompt = item.render_prompt();
                result.push(json!({
                    "id": item.id, "suite": name, "title": title_for(&item.id),
                    "category": category_for(&item.category),
                    "language": match item.language.as_str() {
                        "en-IN" => "English", "hi" => "Hindi", "hi-Latn" => "Hinglish", other => other,
                    },
                    "track": item.capability_track, "instruction": instruction,
                    "context": context, "full_prompt": full_prompt,
                    "prompt_sha256": sha256_hex(full_prompt.as_bytes()),
                    "task_sha256": item.content_hash(), "task_version": item.version,
                    "expected": expected,
                    "grading": if spec.kind() == "json" {
                        "Every answer field must match; no missing or extra fields."
                    } else {
                        "Save the cheapest eligible draft. No booking, payment or login."
                    },
                }));
            }
        }
        result
    }

    pub fn proposals(&self) -> Value {
        let path = self
            .project
            .join("research/consumer100-proposals/krutrim_mulyankan_consumer100/consumer_100.json");
        if !path.is_file() {
            return json!({"status": "unavailable", "items": []});
        }
        let data: Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| json!({"items": []}));
        let fields = [
            "id",
            "title",
            "category",
            "service_provider",
            "user_prompt",
            "safe_execution_boundary",
        ];
        let items: Vec<Value> = data
            .get("items")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        let mut out = Map::new();
                        if let Some(obj) = item.as_object() {
                            for key in fields {
                                if let Some(value) = obj.get(key) {
                                    out.insert(key.to_string(), value.clone());
                                }
                            }
                        }
                        Value::Object(out)
                    })
                    .collect()
            })
            .unwrap_or_default();
        json!({"status": "proposed_not_executable", "items": items})
    }

    pub fn runs(&self) -> Vec<Value> {
        if !self.settings.db_path.exists() {
            return Vec::new();
        }
        let Ok(store) = self.store() else {
            return Vec::new();
        };
        let Ok(all) = store.list_runs(50) else {
            return Vec::new();
        };
        let mut records = Vec::new();
        for run in all {
            if !self.suites.contains_key(&run.suite_name) {
                continue;
            }
            let controls_ok = run
                .runners
                .iter()
                .all(|r| matches!(r.as_str(), "echo" | "oracle"));
            let observations_ok = !run.runners.is_empty()
                && run
                    .runners
                    .iter()
                    .all(|r| r.starts_with("app-observation:"));
            if !controls_ok && !observations_ok {
                continue;
            }
            let task_count = store
                .results_for_run(&run.id)
                .unwrap_or_default()
                .iter()
                .map(|row| row.task_id.clone())
                .collect::<HashSet<_>>()
                .len();
            let mut record = json!({
                "id": run.id, "suite": run.suite_name, "started_at": run.started_at,
                "status": run.status,
                "cost_usd": if run.uncertain_cost { Value::Null } else { json!(run.spent_usd) },
                "task_count": task_count, "controls": run.runners,
                "mode": if observations_ok { "manual_app" } else { "mock" },
            });
            if observations_ok {
                let manifest: Value =
                    std::fs::read_to_string(Path::new(&run.artifacts_dir).join("manifest.json"))
                        .ok()
                        .and_then(|t| serde_json::from_str(&t).ok())
                        .unwrap_or_else(|| json!({}));
                let configuration = manifest
                    .get("runners")
                    .and_then(Value::as_array)
                    .and_then(|rows| rows.first())
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                record["verification_status"] = configuration
                    .get("verification_status")
                    .cloned()
                    .unwrap_or_else(|| json!("unverified"));
                record["app_context"] = configuration
                    .get("app_context")
                    .cloned()
                    .unwrap_or(Value::Null);
            }
            records.push(record);
        }
        records
    }

    pub fn run_detail(&self, run_id: &str, outputs: bool) -> Result<Value, String> {
        if !workbench_id(run_id) {
            return Err("unknown run".into());
        }
        let store = self.store()?;
        let run = store.get_run(run_id)?.ok_or_else(|| {
            "only local mock/app-observation pilot results are available here".to_string()
        })?;
        let controls_ok = run
            .runners
            .iter()
            .all(|r| matches!(r.as_str(), "echo" | "oracle"));
        let observations_ok = !run.runners.is_empty()
            && run
                .runners
                .iter()
                .all(|r| r.starts_with("app-observation:"));
        if !self.suites.contains_key(&run.suite_name) || !(controls_ok || observations_ok) {
            return Err("only local mock/app-observation pilot results are available here".into());
        }
        let mut payload = crate::export::result_export(&store, run_id, outputs)?.value;
        if outputs {
            let mut details = Map::new();
            for record in store.results_for_run(run_id).unwrap_or_default() {
                details.insert(
                    crate::export::explanation_key(
                        &record.task_id,
                        &record.runner,
                        record.repeat_index,
                    ),
                    record
                        .grade_details
                        .map(Value::Object)
                        .unwrap_or(Value::Null),
                );
            }
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("explanations".into(), Value::Object(details));
                let comparison_path = Path::new(&run.artifacts_dir).join("comparison.json");
                if comparison_path.is_file() {
                    if let Ok(text) = std::fs::read_to_string(&comparison_path) {
                        if let Ok(bundle) = serde_json::from_str::<Value>(&text) {
                            obj.insert("comparison_bundle".into(), bundle);
                        }
                    }
                }
            }
        }
        Ok(payload)
    }

    fn run_single_task(
        &self,
        suite: &Suite,
        runners: &[Runner],
        task_id: &str,
    ) -> Result<Value, WorkError> {
        let Ok(_guard) = self.lock.try_lock() else {
            return Err(WorkError::Conflict(
                "A check is already running. Please try again shortly.".into(),
            ));
        };
        let store = self.store().map_err(WorkError::Bad)?;
        let summary = run_suite(
            suite,
            runners,
            &store,
            &self.settings,
            12,
            1,
            Some(0.0),
            Some(&[task_id.to_string()]),
            &crate::RunCallbacks::default(),
            None,
        )
        .map_err(|e| WorkError::Bad(e.to_string()))?;
        let _ = crate::export::write_export(
            &store,
            &summary.run_id,
            &summary.run_dir.join("results.json"),
            false,
        );
        self.run_detail(&summary.run_id, true)
            .map_err(WorkError::Bad)
    }

    pub fn validate(&self, body: &Value) -> Result<Value, WorkError> {
        let obj = body
            .as_object()
            .ok_or_else(|| WorkError::Bad("request must be an object".into()))?;
        for field in obj.keys() {
            if !matches!(field.as_str(), "suite" | "task_id" | "control") {
                return Err(WorkError::Bad(format!(
                    "extra input is not permitted: {field}"
                )));
            }
        }
        let suite_name = str_field(obj.get("suite"), "suite").map_err(WorkError::Bad)?;
        let suite = self.suite(suite_name).map_err(WorkError::Bad)?.clone();
        let task_id = str_field(obj.get("task_id"), "task_id").map_err(WorkError::Bad)?;
        suite
            .select(Some(&[task_id.to_string()]))
            .map_err(WorkError::Bad)?;
        let control = obj.get("control").and_then(Value::as_str).unwrap_or("both");
        let env = workbench_env();
        let empty = crate::pricing::PriceTable::default();
        let runners: Vec<Runner> = match control {
            "oracle" => vec![make_runner("oracle", &empty, &env).map_err(WorkError::Bad)?],
            "echo" => vec![make_runner("echo", &empty, &env).map_err(WorkError::Bad)?],
            "both" => vec![
                make_runner("oracle", &empty, &env).map_err(WorkError::Bad)?,
                make_runner("echo", &empty, &env).map_err(WorkError::Bad)?,
            ],
            _ => {
                return Err(WorkError::Bad(
                    "control must be both, oracle or echo".into(),
                ))
            }
        };
        self.run_single_task(&suite, &runners, task_id)
    }

    pub fn record_observation(&self, body: &Value) -> Result<Value, WorkError> {
        let observation = AppObservation::validate(body).map_err(WorkError::Bad)?;
        let suite_name = observation
            .field("suite")
            .and_then(Value::as_str)
            .ok_or_else(|| WorkError::Bad("observation suite is required".into()))?
            .to_string();
        let task_id = observation
            .field("task_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let item = self
            .task_in(&suite_name, &task_id)
            .map_err(WorkError::Bad)?;
        if item.capability_track != "text" {
            return Err(WorkError::Bad(
                "manual text responses cannot establish browser-workflow success".into(),
            ));
        }
        let expected_hash = sha256_hex(item.render_prompt().as_bytes());
        if observation.field("prompt_sha256").and_then(Value::as_str) != Some(&expected_hash) {
            return Err(WorkError::Bad(
                "the submitted prompt does not match this frozen task".into(),
            ));
        }
        let runner = AppObservationRunner::new(observation).as_runner();
        let suite = self.suite(&suite_name).map_err(WorkError::Bad)?.clone();
        self.run_single_task(&suite, &[runner], &task_id)
    }

    pub fn configurations(&self) -> Vec<Value> {
        let mut status: HashMap<String, String> = HashMap::new();
        if self.settings.db_path.is_file() {
            if let Ok(store) = self.store() {
                if let Ok(runs) = store.list_runs(200) {
                    for run in runs {
                        let manifest: Value = std::fs::read_to_string(
                            Path::new(&run.artifacts_dir).join("manifest.json"),
                        )
                        .ok()
                        .and_then(|t| serde_json::from_str(&t).ok())
                        .unwrap_or_else(|| json!({}));
                        if let Some(configs) = manifest.get("runners").and_then(Value::as_array) {
                            for config in configs {
                                if let Some(registry_id) = config
                                    .get("configuration_registry_id")
                                    .and_then(Value::as_str)
                                {
                                    let current = if config
                                        .get("verification_status")
                                        .and_then(Value::as_str)
                                        == Some("test_fixture")
                                    {
                                        "sample_only"
                                    } else {
                                        "recorded_unverified"
                                    };
                                    if status.get(registry_id).map(String::as_str)
                                        != Some("recorded_unverified")
                                    {
                                        status.insert(registry_id.to_string(), current.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        self.registry
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|config| {
                json!({"configuration": config.to_value(),
                    "status": status.get(&config.id).cloned().unwrap_or_else(|| "unrun".to_string())})
            })
            .collect()
    }

    pub fn compare(&self, body: &Value, imported: bool) -> Result<Value, WorkError> {
        let bundle = parse_comparison_bundle(body).map_err(WorkError::Bad)?;
        let suite = self.suite(&bundle.suite).map_err(WorkError::Bad)?.clone();
        let item = suite
            .select(Some(&[bundle.task_id.clone()]))
            .map_err(WorkError::Bad)?
            .into_iter()
            .next()
            .ok_or_else(|| WorkError::Bad("unknown task".into()))?;
        if item.content_hash() != bundle.task_sha256 || item.version != bundle.task_version {
            return Err(WorkError::Bad(
                "task definition/version changed; use the same frozen task".into(),
            ));
        }
        if sha256_hex(item.render_prompt().as_bytes()) != bundle.prompt_sha256 {
            return Err(WorkError::Bad(
                "prompt hash differs from the selected task".into(),
            ));
        }
        let Ok(_guard) = self.lock.try_lock() else {
            return Err(WorkError::Conflict(
                "A check is already running. Please try again shortly.".into(),
            ));
        };
        self.registry
            .ensure(
                &bundle
                    .entries
                    .iter()
                    .map(|e| e.configuration.clone())
                    .collect::<Vec<_>>(),
                imported,
            )
            .map_err(WorkError::Bad)?;
        let runners: Vec<Runner> = bundle
            .entries
            .iter()
            .map(|entry| {
                entry
                    .observation(&bundle)
                    .map(|obs| AppObservationRunner::new(obs).as_runner())
            })
            .collect::<Result<_, _>>()
            .map_err(WorkError::Bad)?;
        let store = self.store().map_err(WorkError::Bad)?;
        let summary = run_suite(
            &suite,
            &runners,
            &store,
            &self.settings,
            12,
            1,
            Some(0.0),
            Some(&[item.id.clone()]),
            &crate::RunCallbacks::default(),
            None,
        )
        .map_err(|e| WorkError::Bad(e.to_string()))?;
        let mut captured = bundle.raw.clone();
        captured["recorded_run_id"] = json!(summary.run_id);
        let text =
            serde_json::to_string_pretty(&captured).map_err(|e| WorkError::Bad(e.to_string()))?;
        std::fs::write(summary.run_dir.join("comparison.json"), text + "\n")
            .map_err(|e| WorkError::Bad(e.to_string()))?;
        let _ = crate::export::write_export(
            &store,
            &summary.run_id,
            &summary.run_dir.join("results.json"),
            false,
        );
        let mut payload = self
            .run_detail(&summary.run_id, true)
            .map_err(WorkError::Bad)?;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("comparison_bundle".into(), captured);
        }
        Ok(payload)
    }

    pub fn import_comparison(&self, document: &str) -> Result<Value, WorkError> {
        let data: Value = serde_json::from_str(document)
            .map_err(|e| WorkError::Bad(format!("invalid comparison document: {e}")))?;
        self.compare(&data, true)
    }
}

// ---------------- comparisons ----------------

struct ComparisonEntryData {
    configuration: NamedConfiguration,
    answer: Value,
}

struct ComparisonBundleData {
    raw: Value,
    suite: String,
    task_id: String,
    task_version: i64,
    task_sha256: String,
    prompt_sha256: String,
    entries: Vec<ComparisonEntryData>,
}

fn parse_comparison_bundle(body: &Value) -> Result<ComparisonBundleData, String> {
    let obj = body
        .as_object()
        .ok_or_else(|| "comparison bundle must be an object".to_string())?;
    for field in obj.keys() {
        if !matches!(
            field.as_str(),
            "schema_version"
                | "kind"
                | "suite"
                | "task_id"
                | "task_version"
                | "task_sha256"
                | "prompt_sha256"
                | "entries"
                | "recorded_run_id"
        ) {
            return Err(format!("extra input is not permitted: {field}"));
        }
    }
    if obj
        .get("schema_version")
        .and_then(Value::as_str)
        .unwrap_or("1.0")
        != "1.0"
    {
        return Err("input should be '1.0'".into());
    }
    if obj
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("manual_comparison")
        != "manual_comparison"
    {
        return Err("input should be 'manual_comparison'".into());
    }
    let suite = obj
        .get("suite")
        .and_then(Value::as_str)
        .unwrap_or("india-pilot")
        .to_string();
    if suite != "india-pilot" {
        return Err("input should be 'india-pilot'".into());
    }
    let task_id = str_field(obj.get("task_id"), "task_id")?.to_string();
    let task_version = obj.get("task_version").and_then(Value::as_i64).unwrap_or(0);
    if task_version < 1 {
        return Err("task_version must be at least 1".into());
    }
    let task_sha256 = str_field(obj.get("task_sha256"), "task_sha256")?.to_string();
    let prompt_sha256 = str_field(obj.get("prompt_sha256"), "prompt_sha256")?.to_string();
    if !hex64(&task_sha256) || !hex64(&prompt_sha256) {
        return Err("task and prompt hashes must be lowercase SHA-256 hex".into());
    }
    let raw_entries = obj
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| "entries must be a list".to_string())?;
    if !(2..=8).contains(&raw_entries.len()) {
        return Err("compare between 2 and 8 configurations".into());
    }
    let mut entries = Vec::new();
    for entry in raw_entries {
        let entry_obj = entry
            .as_object()
            .ok_or_else(|| "comparison entry must be an object".to_string())?;
        let configuration =
            NamedConfiguration::from_value(entry_obj.get("configuration").unwrap_or(&Value::Null))?;
        let answer = entry_obj.get("answer").cloned().unwrap_or(Value::Null);
        validate_recorded_answer(&answer)?;
        entries.push(ComparisonEntryData {
            configuration,
            answer,
        });
    }
    let ids: Vec<&str> = entries
        .iter()
        .map(|e| e.configuration.id.as_str())
        .collect();
    if ids.iter().collect::<HashSet<_>>().len() != ids.len() {
        return Err("select each configuration only once".into());
    }
    Ok(ComparisonBundleData {
        raw: body.clone(),
        suite,
        task_id,
        task_version,
        task_sha256,
        prompt_sha256,
        entries,
    })
}

fn validate_recorded_answer(answer: &Value) -> Result<(), String> {
    let obj = answer
        .as_object()
        .ok_or_else(|| "answer must be an object".to_string())?;
    for field in obj.keys() {
        if !matches!(
            field.as_str(),
            "response"
                | "evidence_note"
                | "evidence_references"
                | "completion_status"
                | "elapsed_s"
                | "observed_at"
                | "cost_amount"
                | "cost_currency"
                | "cost_source"
                | "kind"
                | "exact_prompt_used"
                | "attested"
        ) {
            return Err(format!("extra input is not permitted: {field}"));
        }
    }
    let response = str_field(obj.get("response"), "response")?;
    if response.chars().count() > 30000 {
        return Err("response is too long".into());
    }
    let note = str_field(obj.get("evidence_note"), "evidence_note")?;
    if note.chars().count() < 8 || note.chars().count() > 4000 {
        return Err("evidence note length is outside 8 to 4000 characters".into());
    }
    if obj
        .get("evidence_references")
        .and_then(Value::as_array)
        .is_some_and(|refs| refs.len() > 10)
    {
        return Err("at most 10 evidence references".into());
    }
    if let Some(status) = obj.get("completion_status") {
        if !matches!(
            status.as_str(),
            Some("completed" | "partial" | "not_completed" | "unknown")
        ) {
            return Err("unknown completion status".into());
        }
    }
    if let Some(elapsed) = obj.get("elapsed_s").and_then(Value::as_f64) {
        if elapsed < 0.0 {
            return Err("elapsed_s must not be negative".into());
        }
    }
    if let Some(cost) = obj.get("cost_amount").and_then(Value::as_f64) {
        if cost < 0.0 {
            return Err("cost_amount must not be negative".into());
        }
    }
    if let Some(currency) = obj.get("cost_currency").and_then(Value::as_str) {
        if currency.len() != 3 || !currency.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err("cost_currency must be a three-letter code".into());
        }
    }
    if let Some(kind) = obj.get("kind") {
        if !matches!(kind.as_str(), Some("contributor_reported" | "test_fixture")) {
            return Err("unknown answer kind".into());
        }
    }
    Ok(())
}

impl ComparisonEntryData {
    fn observation(&self, bundle: &ComparisonBundleData) -> Result<AppObservation, String> {
        let answer = &self.answer;
        let mut context = self
            .configuration
            .app_context
            .as_object()
            .cloned()
            .unwrap_or_default();
        context.insert(
            "observed_latency_s".into(),
            answer.get("elapsed_s").cloned().unwrap_or(Value::Null),
        );
        context.insert(
            "observed_at".into(),
            answer.get("observed_at").cloned().unwrap_or(Value::Null),
        );
        context.insert(
            "observation_kind".into(),
            answer
                .get("kind")
                .cloned()
                .unwrap_or_else(|| json!("contributor_reported")),
        );
        let observation = json!({
            "suite": bundle.suite,
            "task_id": bundle.task_id,
            "prompt_sha256": bundle.prompt_sha256,
            "app_context": Value::Object(context),
            "response": answer.get("response").cloned().unwrap_or(json!("")),
            "evidence_note": answer.get("evidence_note").cloned().unwrap_or(json!("")),
            "evidence_references": answer.get("evidence_references").cloned().unwrap_or(json!([])),
            "configuration_name": self.configuration.name,
            "configuration_registry_id": self.configuration.id,
            "completion_status": answer.get("completion_status").cloned().unwrap_or_else(|| json!("unknown")),
            "observed_cost_amount": answer.get("cost_amount").cloned().unwrap_or(Value::Null),
            "observed_cost_currency": answer.get("cost_currency").cloned().unwrap_or(Value::Null),
            "observed_cost_source": answer.get("cost_source").cloned().unwrap_or(Value::Null),
            "exact_prompt_used": answer.get("exact_prompt_used").cloned().unwrap_or(json!(false)),
            "attested": answer.get("attested").cloned().unwrap_or(json!(false)),
        });
        AppObservation::validate(&observation)
    }
}

// ---------------- evidence archive ----------------

pub fn list_evidence(destination: &Path) -> Vec<Value> {
    if !destination.is_dir() {
        return Vec::new();
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(destination) {
        for entry in entries.flatten() {
            let summary = entry.path().join("summary.json");
            if summary.is_file() {
                paths.push(summary);
            }
        }
    }
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .collect()
}

pub fn evidence_detail(destination: &Path, identifier: &str) -> Result<Value, String> {
    if !hex64(identifier) {
        return Err("unknown evidence archive".into());
    }
    let directory = destination.join(identifier);
    let summary: Value = std::fs::read_to_string(directory.join("summary.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .ok_or_else(|| "unknown evidence archive".to_string())?;
    let data: Value = std::fs::read_to_string(directory.join("source.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .ok_or_else(|| "unknown evidence archive".to_string())?;
    let mut rows = Vec::new();
    if let Some(attempts) = data.get("attempts").and_then(Value::as_array) {
        for (index, attempt) in attempts.iter().enumerate() {
            let variants: Vec<Value> = attempt
                .get("response_variants")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter(|v| v.is_object())
                        .map(|v| {
                            json!({"label": v.get("label").cloned().unwrap_or(Value::Null),
                            "response": v.get("response").cloned().unwrap_or(Value::Null)})
                        })
                        .collect()
                })
                .unwrap_or_default();
            let visible = attempt
                .get("visible_model")
                .cloned()
                .filter(|v| !matches!(v, Value::Null))
                .or_else(|| attempt.get("visible_model_mode").cloned())
                .or_else(|| attempt.get("visible_mode").cloned())
                .unwrap_or(Value::Null);
            rows.push(json!({
                "index": index + 1,
                "app": attempt.get("app").cloned().unwrap_or(Value::Null),
                "task": attempt.get("task").cloned().unwrap_or(Value::Null),
                "response": attempt.get("response").cloned().unwrap_or(Value::Null),
                "response_variants": variants,
                "submission_utc": attempt.get("submission_call_utc").cloned().unwrap_or(Value::Null),
                "capture_utc": attempt.get("evidence_capture_utc").cloned().unwrap_or(Value::Null),
                "visible_model_label": visible,
                "scoring_status": "unscored_task_mapping_missing",
                "score": null, "cost": null, "measured_elapsed_s": null,
                "source_grade_claim": attempt.get("grade").cloned().unwrap_or(Value::Null),
            }));
        }
    }
    Ok(json!({"summary": summary, "attempts": rows}))
}

// ---------------- HTTP server ----------------

pub struct Workbench {
    pub service: Arc<PilotService>,
    pub jobs: Arc<JobManager>,
}

pub fn create_workbench(project: &Path) -> Result<Arc<Workbench>, String> {
    let service = Arc::new(PilotService::new(project)?);
    let jobs = JobManager::new(service.clone())?;
    Ok(Arc::new(Workbench { service, jobs }))
}

fn work_error_response(error: WorkError, service: &PilotService) -> (u16, Value) {
    match error {
        WorkError::Bad(message) => (
            400,
            json!({"error": format!("Input rejected: {}", service.scrub_text(&message).chars().take(220).collect::<String>())}),
        ),
        WorkError::Conflict(message) => (409, json!({"error": service.scrub_text(&message)})),
        WorkError::NotFound(message) => (404, json!({"error": service.scrub_text(&message)})),
    }
}

fn get_v1_route(bench: &Workbench, path: &str) -> Result<Value, String> {
    let manager = &bench.jobs;
    match path {
        "/api/v1" => Ok(json!({
            "api_version": "1.0",
            "capabilities": "/api/v1/capabilities",
            "tasks": "/api/v1/tasks",
            "jobs": "/api/v1/jobs",
            "request_schema": "/api/v1/schemas/job-request",
            "state_schema": "/api/v1/schemas/job-state",
            "auth": "Loopback Host; same-origin writes; X-Pilot-Token from page meta pilot-token",
            "privacy": "Results endpoint includes private output; export omits raw output",
        })),
        "/api/v1/capabilities" => Ok(manager.capabilities()),
        "/api/v1/tasks" => Ok(json!({"api_version": "1.0", "tasks": bench.service.tasks()})),
        "/api/v1/jobs" => Ok(json!({"api_version": "1.0", "jobs": manager.list()})),
        "/api/v1/schemas/job-request" => Ok(json!({
            "title": "JobRequest", "type": "object",
            "additionalProperties": false,
            "required": ["task_ids", "adapter_ids", "idempotency_key"],
            "properties": {
                "api_version": {"const": "1.0", "default": "1.0", "type": "string"},
                "suite": {"enum": ["india-pilot", "railway-pilot"], "default": "india-pilot", "type": "string"},
                "task_ids": {"items": {"type": "string"}, "maxItems": 24, "minItems": 1, "type": "array"},
                "adapter_ids": {"items": {"type": "string"}, "maxItems": 4, "minItems": 1, "type": "array"},
                "seed": {"default": 12, "maximum": 2147483647, "minimum": 0, "type": "integer"},
                "repeats": {"default": 1, "maximum": 5, "minimum": 1, "type": "integer",
                    "description": "Tries per task and adapter"},
                "idempotency_key": {"pattern": "^[A-Za-z0-9_-]{8,100}$", "type": "string"},
            },
        })),
        "/api/v1/schemas/job-state" => Ok(json!({
            "title": "JobState", "type": "object",
            "additionalProperties": false,
            "required": ["id", "request", "status", "created_at", "updated_at", "total"],
            "properties": {
                "api_version": {"const": "1.0", "default": "1.0", "type": "string"},
                "id": {"type": "string"},
                "status": {"enum": ["queued", "running", "cancelling", "completed", "failed", "cancelled", "interrupted"], "type": "string"},
                "created_at": {"type": "string"}, "updated_at": {"type": "string"},
                "started_at": {"type": ["string", "null"]},
                "finished_at": {"type": ["string", "null"]},
                "run_id": {"type": ["string", "null"]},
                "parent_job_id": {"type": ["string", "null"]},
                "total": {"type": "integer"},
            },
        })),
        _ => {
            if let Some(rest) = path.strip_prefix("/api/v1/jobs/") {
                let (identity, suffix) = match rest.split_once('/') {
                    None => (rest, None),
                    Some((id, suffix)) => (id, Some(suffix)),
                };
                if identity.len() == 32
                    && identity
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                {
                    return match suffix {
                        None => manager.get(identity),
                        Some("results") => manager.result(identity, true),
                        Some("export") => manager.result(identity, false),
                        _ => Err("unknown API route".into()),
                    };
                }
            }
            Err("unknown API route".into())
        }
    }
}

fn post_v1_route(bench: &Workbench, path: &str, body: &Value) -> Result<(Value, u16), WorkError> {
    let manager = &bench.jobs;
    match path {
        "/api/v1/jobs/preview" => {
            let request = JobRequest::from_value(body).map_err(WorkError::Bad)?;
            let (preview, _) = manager.preview(&request).map_err(WorkError::Bad)?;
            Ok((preview, 200))
        }
        "/api/v1/jobs" => {
            let request = JobRequest::from_value(body).map_err(WorkError::Bad)?;
            Ok((manager.create(request, None)?, 202))
        }
        _ => {
            if let Some(rest) = path.strip_prefix("/api/v1/jobs/") {
                if let Some((identity, action)) = rest.split_once('/') {
                    if identity.len() == 32
                        && identity
                            .bytes()
                            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                        && matches!(action, "cancel" | "retry")
                    {
                        if action == "cancel" {
                            return Ok((manager.cancel(identity).map_err(WorkError::Bad)?, 200));
                        }
                        let key = body
                            .get("idempotency_key")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                WorkError::Bad("Retry requires a new idempotency key".into())
                            })?;
                        return Ok((manager.retry(identity, key)?, 202));
                    }
                }
            }
            Err(WorkError::Bad("unknown API route".into()))
        }
    }
}

struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

struct HttpResponse {
    status: u16,
    content_type: String,
    body: Vec<u8>,
    extra_headers: Vec<(String, String)>,
}

fn json_response(status: u16, value: &Value) -> HttpResponse {
    HttpResponse {
        status,
        content_type: "application/json; charset=utf-8".into(),
        body: serde_json::to_vec(value).unwrap_or_default(),
        extra_headers: Vec::new(),
    }
}

fn read_http_request(stream: &mut TcpStream) -> Option<HttpRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 16384];
    let header_end = loop {
        if let Some(pos) = find_subslice(&buffer, b"\r\n\r\n") {
            break pos;
        }
        match stream.read(&mut chunk) {
            Ok(0) => return None,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            Err(_) => return None,
        }
        if buffer.len() > 2 * 1024 * 1024 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.lines();
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let path = target.split('?').next().unwrap_or("/").to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buffer[header_end + 4..].to_vec();
    while body.len() < length {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    body.truncate(length);
    Some(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        302 => "Found",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    }
}

fn write_http_response(stream: &mut TcpStream, response: &HttpResponse) {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        reason_phrase(response.status),
        response.content_type,
        response.body.len()
    );
    for (name, value) in &response.extra_headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&response.body);
    let _ = stream.flush();
}

const ASSETS: [(&str, &[u8], &str); 15] = [
    (
        "/",
        include_bytes!("../../../../src/kritrim_mulyankan/ui_assets/index.html"),
        "text/html; charset=utf-8",
    ),
    (
        "/app.js",
        include_bytes!("../../../../src/kritrim_mulyankan/ui_assets/app.js"),
        "text/javascript; charset=utf-8",
    ),
    (
        "/showcase.js",
        include_bytes!("../../../../src/kritrim_mulyankan/ui_assets/showcase.js"),
        "text/javascript; charset=utf-8",
    ),
    (
        "/compare.js",
        include_bytes!("../../../../src/kritrim_mulyankan/ui_assets/compare.js"),
        "text/javascript; charset=utf-8",
    ),
    (
        "/jobs.js",
        include_bytes!("../../../../src/kritrim_mulyankan/ui_assets/jobs.js"),
        "text/javascript; charset=utf-8",
    ),
    (
        "/theme.js",
        include_bytes!("../../../../src/kritrim_mulyankan/ui_assets/theme.js"),
        "text/javascript; charset=utf-8",
    ),
    (
        "/style.css",
        include_bytes!("../../../../src/kritrim_mulyankan/ui_assets/style.css"),
        "text/css; charset=utf-8",
    ),
    (
        "/favicon.ico",
        include_bytes!("../../../../src/kritrim_mulyankan/ui_assets/brand/favicon.ico"),
        "image/x-icon",
    ),
    (
        "/brand/favicon.ico",
        include_bytes!("../../../../src/kritrim_mulyankan/ui_assets/brand/favicon.ico"),
        "image/x-icon",
    ),
    (
        "/brand/favicon-16.png",
        include_bytes!("../../../../src/kritrim_mulyankan/ui_assets/brand/favicon-16.png"),
        "image/png",
    ),
    (
        "/brand/favicon-32.png",
        include_bytes!("../../../../src/kritrim_mulyankan/ui_assets/brand/favicon-32.png"),
        "image/png",
    ),
    (
        "/brand/favicon-180.png",
        include_bytes!("../../../../src/kritrim_mulyankan/ui_assets/brand/favicon-180.png"),
        "image/png",
    ),
    (
        "/brand/km-chakra-wordmark.svg",
        include_bytes!("../../../../src/kritrim_mulyankan/ui_assets/brand/km-chakra-wordmark.svg"),
        "image/svg+xml",
    ),
    (
        "/brand/km-chakra-wordmark-white.svg",
        include_bytes!(
            "../../../../src/kritrim_mulyankan/ui_assets/brand/km-chakra-wordmark-white.svg"
        ),
        "image/svg+xml",
    ),
    (
        "/brand/fonts/Sora.woff2",
        include_bytes!("../../../../src/kritrim_mulyankan/ui_assets/brand/fonts/Sora.woff2"),
        "font/woff2",
    ),
];

fn asset_for(path: &str) -> Option<(&'static [u8], &'static str)> {
    ASSETS
        .iter()
        .find(|(route, _, _)| *route == path)
        .map(|(_, body, content_type)| (*body, *content_type))
        .or_else(|| {
            (path == "/brand/fonts/NotoSansDevanagari-Regular.woff2").then_some((
                include_bytes!("../../../../src/kritrim_mulyankan/ui_assets/brand/fonts/NotoSansDevanagari-Regular.woff2") as &[u8],
                "font/woff2",
            ))
        })
}

fn handle_get(bench: &Workbench, path: &str, token: &str) -> HttpResponse {
    let service = &bench.service;
    if path == "/api/v1" || path.starts_with("/api/v1/") {
        return match get_v1_route(bench, path) {
            Ok(value) => json_response(200, &value),
            Err(message) => json_response(
                404,
                &json!({"error": message.chars().take(220).collect::<String>()}),
            ),
        };
    }
    if matches!(path, "/overview.html" | "/index.html") {
        return HttpResponse {
            status: 302,
            content_type: "application/json; charset=utf-8".into(),
            body: Vec::new(),
            extra_headers: vec![("Location".into(), "/".into())],
        };
    }
    match path {
        "/api/tasks" => json_response(200, &json!({"tasks": service.tasks()})),
        "/api/proposals" => json_response(200, &service.proposals()),
        "/api/configurations" => {
            json_response(200, &json!({"configurations": service.configurations()}))
        }
        "/api/evidence" => json_response(
            200,
            &json!({"evidence": list_evidence(&service.settings.runs_dir.join("evidence"))}),
        ),
        "/api/runs" => json_response(200, &json!({"runs": service.runs()})),
        _ => {
            if let Some(identity) = path.strip_prefix("/api/evidence/") {
                return match evidence_detail(&service.settings.runs_dir.join("evidence"), identity)
                {
                    Ok(value) => json_response(200, &value),
                    Err(_) => json_response(404, &json!({"error": "evidence not found"})),
                };
            }
            if let Some(rest) = path.strip_prefix("/api/runs/") {
                let (run_id, export) = match rest.strip_suffix("/export") {
                    Some(id) => (id, true),
                    None => (rest, false),
                };
                if workbench_id(run_id) || run_id.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
                    return match service.run_detail(run_id, !export) {
                        Ok(value) => json_response(200, &value),
                        Err(message) => json_response(404, &json!({"error": message})),
                    };
                }
            }
            if let Some(rest) = path.strip_prefix("/api/comparisons/") {
                if let Some(run_id) = rest.strip_suffix("/export") {
                    return match service.run_detail(run_id, false) {
                        Ok(_) => {
                            let found = service
                                .store()
                                .ok()
                                .and_then(|store| store.get_run(run_id).ok().flatten())
                                .and_then(|run| {
                                    std::fs::read_to_string(
                                        Path::new(&run.artifacts_dir).join("comparison.json"),
                                    )
                                    .ok()
                                })
                                .and_then(|text| serde_json::from_str::<Value>(&text).ok());
                            match found {
                                Some(bundle) => json_response(200, &bundle),
                                None => {
                                    json_response(404, &json!({"error": "comparison not found"}))
                                }
                            }
                        }
                        Err(_) => json_response(404, &json!({"error": "comparison not found"})),
                    };
                }
            }
            match asset_for(path) {
                Some((body, content_type)) => {
                    let body = if path == "/" {
                        String::from_utf8_lossy(body)
                            .replace("REQUEST_TOKEN", token)
                            .into_bytes()
                    } else {
                        body.to_vec()
                    };
                    HttpResponse {
                        status: 200,
                        content_type: content_type.into(),
                        body,
                        extra_headers: vec![
                            ("Content-Security-Policy".into(), "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; form-action 'none'; base-uri 'none'".into()),
                            ("X-Content-Type-Options".into(), "nosniff".into()),
                        ],
                    }
                }
                None => json_response(404, &json!({"error": "Page not found."})),
            }
        }
    }
}

fn handle_post(bench: &Workbench, request: &HttpRequest, port: u16, token: &str) -> HttpResponse {
    let service = &bench.service;
    let allowed_origins = [
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
    ];
    let origin_ok = request
        .headers
        .get("origin")
        .is_some_and(|origin| allowed_origins.contains(origin));
    let token_ok = request.headers.get("x-pilot-token").map(String::as_str) == Some(token);
    if !origin_ok || !token_ok {
        return json_response(
            403,
            &json!({"error": "Use the local workbench to run a check."}),
        );
    }
    let path = request.path.as_str();
    if !path.starts_with("/api/v1/")
        && !matches!(
            path,
            "/api/validate"
                | "/api/observations"
                | "/api/configurations"
                | "/api/comparisons"
                | "/api/comparisons/import"
        )
    {
        return json_response(404, &json!({"error": "Unsupported action."}));
    }
    if request.body.is_empty()
        || request.body.len() > 1024 * 1024
        || request.headers.get("content-type").map(String::as_str) != Some("application/json")
    {
        return json_response(400, &json!({"error": "Input rejected: invalid request"}));
    }
    let body: Value = match serde_json::from_slice(&request.body) {
        Ok(value) => value,
        Err(error) => {
            return json_response(400, &json!({"error": format!("Input rejected: {}", error)}));
        }
    };
    let result: Result<(Value, u16), WorkError> = if path.starts_with("/api/v1/") {
        post_v1_route(bench, path, &body)
    } else {
        match path {
            "/api/configurations" => (|| {
                let name = body
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty() && name.chars().count() <= 120)
                    .ok_or_else(|| WorkError::Bad("configuration name is required".into()))?;
                let context_value = body
                    .get("app_context")
                    .ok_or_else(|| WorkError::Bad("app_context is required".into()))?;
                let context = crate::observations::validate_app_context(context_value)
                    .map_err(WorkError::Bad)?;
                let created = service
                    .registry
                    .create(name, Value::Object(context))
                    .map_err(WorkError::Bad)?;
                Ok((
                    json!({"configuration": created.to_value(), "status": "unrun"}),
                    200,
                ))
            })(),
            "/api/comparisons" => service.compare(&body, false).map(|value| (value, 200)),
            "/api/comparisons/import" => (|| {
                let document = body
                    .as_object()
                    .filter(|obj| obj.len() == 1)
                    .and_then(|obj| obj.get("document"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| WorkError::Bad("expected comparison document text".into()))?;
                service
                    .import_comparison(document)
                    .map(|value| (value, 200))
            })(),
            "/api/observations" => service.record_observation(&body).map(|value| (value, 200)),
            _ => service.validate(&body).map(|value| (value, 200)),
        }
    };
    match result {
        Ok((value, status)) => json_response(status, &value),
        Err(error) => {
            let (status, value) = work_error_response(error, service);
            json_response(status, &value)
        }
    }
}

fn random_token() -> String {
    let material = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    sha256_hex(material.as_bytes())
}

/// Serve the workbench forever; mirrors `local_ui.serve`.
pub fn serve_workbench(project: &Path, port: u16) -> Result<(), String> {
    let bench = create_workbench(project)?;
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("cannot start workbench: {e}"))?;
    let bound = listener
        .local_addr()
        .map(|addr| addr.port())
        .unwrap_or(port);
    println!("Krutrim Mulyankan evaluator: http://127.0.0.1:{bound}/");
    println!("Recorded app evidence, mock controls and verified CLI drafts. Paid APIs disabled.");
    let token = random_token();
    serve_on(bench, listener, &token, bound)
}

fn serve_on(
    bench: Arc<Workbench>,
    listener: TcpListener,
    token: &str,
    port: u16,
) -> Result<(), String> {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let bench = bench.clone();
        let token = token.to_string();
        std::thread::spawn(move || {
            let Some(request) = read_http_request(&mut stream) else {
                return;
            };
            let host = request.headers.get("host").cloned().unwrap_or_default();
            let local = host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}");
            let response = if !local {
                json_response(403, &json!({"error": "This workbench is local only."}))
            } else if request.method == "GET" {
                handle_get(&bench, &request.path, &token)
            } else if request.method == "POST" {
                handle_post(&bench, &request, port, &token)
            } else {
                json_response(404, &json!({"error": "Page not found."}))
            };
            write_http_response(&mut stream, &response);
        });
    }
    Ok(())
}
