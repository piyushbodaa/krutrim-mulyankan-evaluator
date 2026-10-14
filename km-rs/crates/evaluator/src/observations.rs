//! Recorded consumer-app observations, mirroring app_observations.py.
//! Importing an already-observed response never makes an inference call.

use crate::pypy::{py_json_dumps, sha256_hex};
use crate::runners::{RunOutput, Usage};
use crate::suite::Task;
use serde_json::{json, Map, Value};

fn str_field(m: &Map<String, Value>, key: &str) -> Option<String> {
    m.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn check_len(
    m: &Map<String, Value>,
    key: &str,
    min: usize,
    max: usize,
    required: bool,
) -> Result<(), String> {
    match m.get(key) {
        None | Some(Value::Null) => {
            if required {
                Err(format!("{key}: field required"))
            } else {
                Ok(())
            }
        }
        Some(Value::String(s)) => {
            if s.chars().count() < min || s.chars().count() > max {
                Err(format!("{key}: length must be between {min} and {max}"))
            } else {
                Ok(())
            }
        }
        Some(_) => Err(format!("{key}: expected a string")),
    }
}

fn forbid_extra(m: &Map<String, Value>, allowed: &[&str], what: &str) -> Result<(), String> {
    for k in m.keys() {
        if !allowed.contains(&k.as_str()) {
            return Err(format!("{what}: unexpected field '{k}'"));
        }
    }
    Ok(())
}

const CONTEXT_FIELDS: &[&str] = &[
    "app",
    "app_url",
    "client",
    "plan",
    "client_version",
    "displayed_model",
    "session",
    "context_notes",
    "observed_at",
    "observed_latency_s",
    "subscription_amount",
    "subscription_currency",
    "subscription_period",
    "subscription_price_source",
    "observation_kind",
];

/// Validate an AppContext object and return it with defaults filled, in
/// Pydantic field order (JSON object; decimals keep their JSON form here —
/// configuration dumps use dump_context for hashing-critical shapes).
pub fn validate_app_context(input: &Value) -> Result<Map<String, Value>, String> {
    let m = input.as_object().ok_or("app_context: expected an object")?;
    forbid_extra(m, CONTEXT_FIELDS, "app_context")?;
    check_len(m, "app", 1, 100, true)?;
    check_len(m, "app_url", 0, 500, false)?;
    check_len(m, "client", 1, 100, true)?;
    check_len(m, "plan", 1, 100, true)?;
    check_len(m, "client_version", 0, 100, false)?;
    check_len(m, "displayed_model", 0, 100, false)?;
    check_len(m, "context_notes", 0, 4000, false)?;
    check_len(m, "subscription_price_source", 0, 500, false)?;
    if let Some(url) = str_field(m, "app_url") {
        let ok = (url.starts_with("https://") || url.starts_with("http://"))
            && !url.contains('@')
            && !url.contains('?')
            && !url.contains('#')
            && url
                .split("://")
                .nth(1)
                .map(|r| !r.is_empty())
                .unwrap_or(false);
        if !ok {
            return Err(
                "app_url must be an HTTP(S) app URL without credentials/query tokens".into(),
            );
        }
    }
    if let Some(session) = str_field(m, "session") {
        if !["fresh", "existing", "unknown"].contains(&session.as_str()) {
            return Err("session: expected fresh, existing or unknown".into());
        }
    }
    if let Some(obs) = m.get("observed_at") {
        if !obs.is_null() {
            let s = obs.as_str().ok_or("observed_at: expected a datetime")?;
            if !(s.ends_with('Z') || s.rfind(['+', '-']).map(|i| i > 10).unwrap_or(false)) {
                return Err("observed_at must include a timezone".into());
            }
        }
    }
    let has_amount = m
        .get("subscription_amount")
        .map(|v| !v.is_null())
        .unwrap_or(false);
    if has_amount
        && !(str_field(m, "subscription_currency").is_some()
            && str_field(m, "subscription_period").is_some()
            && str_field(m, "subscription_price_source").is_some())
    {
        return Err("subscription amount needs currency, period and source".into());
    }
    let mut out = Map::new();
    let defaults: Map<String, Value> = [
        ("app_url", Value::Null),
        ("client_version", Value::Null),
        ("displayed_model", Value::Null),
        ("session", json!("unknown")),
        ("context_notes", json!("")),
        ("observed_at", Value::Null),
        ("observed_latency_s", Value::Null),
        ("subscription_amount", Value::Null),
        ("subscription_currency", Value::Null),
        ("subscription_period", Value::Null),
        ("subscription_price_source", Value::Null),
        ("observation_kind", json!("contributor_reported")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    for key in CONTEXT_FIELDS {
        let value = m
            .get(*key)
            .cloned()
            .unwrap_or_else(|| defaults.get(*key).cloned().unwrap_or(Value::Null));
        out.insert(key.to_string(), value);
    }
    Ok(out)
}

const OBS_FIELDS: &[&str] = &[
    "schema_version",
    "suite",
    "task_id",
    "prompt_sha256",
    "app_context",
    "response",
    "evidence_note",
    "evidence_references",
    "configuration_name",
    "configuration_registry_id",
    "completion_status",
    "observed_cost_amount",
    "observed_cost_currency",
    "observed_cost_source",
    "exact_prompt_used",
    "attested",
];

#[derive(Debug, Clone)]
pub struct AppObservation {
    pub raw: Map<String, Value>,
    pub app_context: Map<String, Value>,
}

impl AppObservation {
    pub fn validate(input: &Value) -> Result<AppObservation, String> {
        let m = input.as_object().ok_or("observation: expected an object")?;
        forbid_extra(m, OBS_FIELDS, "observation")?;
        if let Some(v) = str_field(m, "schema_version") {
            if v != "1.0" {
                return Err("schema_version: expected '1.0'".into());
            }
        }
        if let Some(v) = str_field(m, "suite") {
            if v != "india-pilot" {
                return Err("suite: expected 'india-pilot'".into());
            }
        }
        let sha = str_field(m, "prompt_sha256").ok_or("prompt_sha256: field required")?;
        if sha.len() != 64
            || !sha
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err("prompt_sha256: expected 64 lowercase hex characters".into());
        }
        check_len(m, "response", 0, 30000, true)?;
        check_len(m, "evidence_note", 8, 4000, true)?;
        check_len(m, "configuration_name", 0, 120, false)?;
        let context =
            validate_app_context(m.get("app_context").ok_or("app_context: field required")?)?;
        let response = str_field(m, "response").unwrap_or_default();
        let completion = str_field(m, "completion_status").unwrap_or_else(|| "unknown".into());
        if response.trim().is_empty() && completion != "not_completed" {
            return Err("provide a response or explicitly record not_completed".into());
        }
        let has_cost = m
            .get("observed_cost_amount")
            .map(|v| !v.is_null())
            .unwrap_or(false);
        if has_cost
            && !(str_field(m, "observed_cost_currency").is_some()
                && str_field(m, "observed_cost_source").is_some())
        {
            return Err("reported per-task cost needs currency and source".into());
        }
        let attested = m.get("attested").and_then(|v| v.as_bool()).unwrap_or(false);
        if context["observation_kind"] == "contributor_reported" && !attested {
            return Err("contributor must attest that the response was observed in the app".into());
        }
        let mut raw = m.clone();
        raw.entry("schema_version".to_string())
            .or_insert_with(|| json!("1.0"));
        raw.entry("suite".to_string())
            .or_insert_with(|| json!("india-pilot"));
        raw.entry("evidence_references".to_string())
            .or_insert_with(|| json!([]));
        raw.entry("configuration_name".to_string())
            .or_insert(Value::Null);
        raw.entry("configuration_registry_id".to_string())
            .or_insert(Value::Null);
        raw.entry("completion_status".to_string())
            .or_insert_with(|| json!("unknown"));
        raw.entry("observed_cost_amount".to_string())
            .or_insert(Value::Null);
        raw.entry("observed_cost_currency".to_string())
            .or_insert(Value::Null);
        raw.entry("observed_cost_source".to_string())
            .or_insert(Value::Null);
        raw.entry("attested".to_string())
            .or_insert_with(|| json!(false));
        raw.insert("app_context".into(), Value::Object(context.clone()));
        Ok(AppObservation {
            raw,
            app_context: context,
        })
    }

    pub fn field(&self, key: &str) -> Option<&Value> {
        self.raw.get(key)
    }

    /// model_dump(mode="json") field order.
    pub fn dump_json(&self) -> Value {
        let mut out = Map::new();
        for key in OBS_FIELDS {
            out.insert(
                key.to_string(),
                self.raw.get(*key).cloned().unwrap_or(Value::Null),
            );
        }
        Value::Object(out)
    }
}

pub struct AppObservationRunner {
    pub observation: AppObservation,
    pub name: String,
}

impl AppObservationRunner {
    pub fn new(observation: AppObservation) -> AppObservationRunner {
        let context_doc = json!({
            "context": Value::Object(observation.app_context.clone()),
            "registry_id": observation.raw.get("configuration_registry_id").cloned().unwrap_or(Value::Null),
            "name": observation.raw.get("configuration_name").cloned().unwrap_or(Value::Null),
        });
        // Python: json.dumps(..., sort_keys=True) with default separators.
        let identifier = &sha256_hex(py_json_dumps(&context_doc).as_bytes())[..16];
        AppObservationRunner {
            name: format!("app-observation:{identifier}"),
            observation,
        }
    }

    pub fn as_runner(self) -> crate::runners::Runner {
        crate::runners::Runner {
            name: self.name.clone(),
            kind: crate::runners::RunnerKind::App,
            inner: crate::runners::RunnerInner::AppObservation(self),
        }
    }

    pub fn configuration_extra(&self) -> Map<String, Value> {
        let obs = &self.observation;
        let mut m = Map::new();
        m.insert("evaluation_mode".into(), json!("manual_app"));
        m.insert("evaluation_track".into(), json!("consumer_app"));
        m.insert("app_context".into(), Value::Object(obs.app_context.clone()));
        m.insert(
            "verification_status".into(),
            obs.app_context
                .get("observation_kind")
                .cloned()
                .unwrap_or(Value::Null),
        );
        m.insert(
            "configuration_name".into(),
            obs.raw
                .get("configuration_name")
                .cloned()
                .unwrap_or(Value::Null),
        );
        m.insert(
            "configuration_registry_id".into(),
            obs.raw
                .get("configuration_registry_id")
                .cloned()
                .unwrap_or(Value::Null),
        );
        m.insert(
            "exact_prompt_used".into(),
            obs.raw
                .get("exact_prompt_used")
                .cloned()
                .unwrap_or(Value::Null),
        );
        m.insert(
            "cost_scope".into(),
            json!("Recorded app usage; per-task charge unknown. Subscription recorded separately."),
        );
        m
    }

    pub fn run(&self, task: &Task, prompt: &str) -> Result<RunOutput, String> {
        let obs = &self.observation;
        let task_id = obs
            .raw
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let prompt_sha = obs
            .raw
            .get("prompt_sha256")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if task.id != task_id || sha256_hex(prompt.as_bytes()) != prompt_sha {
            return Err(
                "ValueError: recorded task/prompt does not match the selected frozen task".into(),
            );
        }
        let response = obs
            .raw
            .get("response")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let mut raw = Map::new();
        raw.insert("observation".into(), obs.dump_json());
        raw.insert("import_api_charge_usd".into(), json!(0));
        raw.insert(
            "response_sha256".into(),
            json!(sha256_hex(response.as_bytes())),
        );
        Ok(RunOutput {
            output: response.to_string(),
            usage: Usage {
                cost_usd: None,
                ..Default::default()
            },
            raw,
            ..Default::default()
        })
    }
}
