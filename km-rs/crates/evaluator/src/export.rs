//! Schema v1.1 export (results.json), mirroring export.py field order,
//! aggregates and verdicts.

use crate::provenance::{redact, redact_str};
use crate::pypy::{py_json_dumps, sha256_file, sha256_hex};
use crate::store::Store;
use crate::verdicts::{
    bootstrap_ci_mean, mean, median, paired_verdicts, task_tries, wilson_interval, Verdict,
};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn ordered(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

pub struct Export {
    pub value: Value,
    pub verdicts: Vec<Verdict>,
}

pub fn result_export(store: &Store, run_id: &str, include_outputs: bool) -> Result<Export, String> {
    let run = store
        .get_run(run_id)?
        .ok_or_else(|| format!("no run '{run_id}'"))?;
    let directory = PathBuf::from(&run.artifacts_dir);
    let manifest: Value = std::fs::read_to_string(directory.join("manifest.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}));
    let empty = Vec::new();
    let manifest_tasks = manifest
        .get("tasks")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    let mut tasks_meta: BTreeMap<String, Value> = BTreeMap::new();
    for item in manifest_tasks {
        if let Some(id) = item["id"].as_str() {
            tasks_meta.insert(id.to_string(), item.clone());
        }
    }
    let manifest_runners = manifest
        .get("runners")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    let mut configurations: BTreeMap<String, Value> = BTreeMap::new();
    for item in manifest_runners {
        if let Some(spec) = item["spec"].as_str() {
            configurations.insert(spec.to_string(), item.clone());
        }
    }

    // models: manifest runner config -> TestedConfiguration shape + id hash.
    let mut models: Vec<Value> = Vec::new();
    let mut model_raw: Vec<Value> = Vec::new();
    for runner_id in &run.runners {
        let mut config = match configurations.get(runner_id) {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        config.remove("spec");
        config
            .entry("runner_id".to_string())
            .or_insert_with(|| Value::String(runner_id.clone()));
        config
            .entry("evaluation_mode".to_string())
            .or_insert_with(|| Value::String("unverified".into()));
        let hash_input = json!({
            "configuration": Value::Object(config.clone()),
            "source": manifest.get("source").cloned().unwrap_or(Value::Null),
            "budget_usd": run.max_usd,
        });
        // Python: json.dumps(..., sort_keys=True) with default separators.
        let configuration_id = sha256_hex(py_json_dumps(&hash_input).as_bytes());
        let model = tested_configuration(&config, &configuration_id);
        model_raw.push(model.clone());
        models.push(model);
    }
    let by_runner: BTreeMap<String, Value> = model_raw
        .iter()
        .map(|m| {
            (
                m["runner_id"].as_str().unwrap_or_default().to_string(),
                m.clone(),
            )
        })
        .collect();

    let records = store.results_for_run(run_id)?;
    let mut results: Vec<Value> = Vec::new();
    for record in &records {
        let metadata = tasks_meta
            .get(&record.task_id)
            .and_then(|t| t.get("metadata"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let config = &by_runner[&record.runner];
        let (reference, raw) = artifact_reference(record.artifact_path.as_deref(), &directory);
        let raw_obj = raw.as_object().cloned().unwrap_or_default();
        let timing = raw_obj
            .get("attempt_timing")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let observation = raw_obj
            .get("observation")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let response = raw_obj
            .get("response")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let track =
            |k: &str, d: &str| -> Value { metadata.get(k).cloned().unwrap_or_else(|| json!(d)) };
        let score = record.human_score.or(record.score);
        let mut usage = Map::new();
        usage.insert("input_tokens".into(), json!(record.input_tokens));
        usage.insert("output_tokens".into(), json!(record.output_tokens));
        if config.get("evaluation_track").and_then(|v| v.as_str()) == Some("harness") {
            let u = response.get("usage").cloned().unwrap_or_else(|| json!({}));
            usage.insert(
                "cache_read_input_tokens".into(),
                u.get("cache_read_input_tokens")
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            usage.insert(
                "cache_creation_input_tokens".into(),
                u.get("cache_creation_input_tokens")
                    .cloned()
                    .unwrap_or(Value::Null),
            );
        }
        let mode = config["evaluation_mode"].as_str().unwrap_or("unverified");
        let cost_method = if mode == "manual_app" {
            "manual_app_observation"
        } else if mode == "mock" {
            "mock_execution"
        } else if config.get("evaluation_track").and_then(|v| v.as_str()) == Some("harness") {
            "subscription_cli_charge_unverified"
        } else {
            "configured_rate_times_provider_usage"
        };
        let reported_price = raw_obj
            .get("reported_token_price_usd")
            .cloned()
            .unwrap_or(Value::Null);
        let result = ordered(vec![
            ("task_id", json!(record.task_id)),
            ("task_version", json!(record.task_version)),
            ("task_sha256", json!(record.task_hash)),
            ("runner_id", json!(record.runner)),
            ("repeat_index", json!(record.repeat_index)),
            ("configuration_id", config["configuration_id"].clone()),
            ("capability_track", track("capability_track", "text")),
            (
                "capability_track_version",
                track("capability_track_version", "1.0"),
            ),
            ("category", json!(record.category)),
            (
                "language",
                metadata
                    .get("language")
                    .cloned()
                    .unwrap_or_else(|| json!("und")),
            ),
            (
                "domain",
                metadata.get("domain").cloned().unwrap_or(Value::Null),
            ),
            ("evaluation_mode", json!(mode)),
            (
                "evaluation_track",
                config
                    .get("evaluation_track")
                    .cloned()
                    .unwrap_or_else(|| json!("unverified")),
            ),
            (
                "app_context",
                config.get("app_context").cloned().unwrap_or(Value::Null),
            ),
            (
                "verification_status",
                config
                    .get("verification_status")
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
            (
                "exact_prompt_used",
                config
                    .get("exact_prompt_used")
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
            (
                "evidence_note",
                if include_outputs {
                    observation
                        .get("evidence_note")
                        .cloned()
                        .unwrap_or(Value::Null)
                } else {
                    Value::Null
                },
            ),
            (
                "evidence_references",
                if include_outputs {
                    observation
                        .get("evidence_references")
                        .cloned()
                        .unwrap_or(Value::Null)
                } else {
                    Value::Null
                },
            ),
            (
                "completion_status",
                observation
                    .get("completion_status")
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
            (
                "configuration_name",
                config
                    .get("configuration_name")
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
            (
                "configuration_registry_id",
                config
                    .get("configuration_registry_id")
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
            (
                "resolved_model",
                raw_obj
                    .get("resolved_model")
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
            (
                "system_fingerprint",
                response
                    .get("system_fingerprint")
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
            ("status", json!(record.status)),
            ("grade_status", json!(record.grade_status)),
            ("passed", json!(record.passed)),
            ("score", json!(score)),
            ("latency_s", json!(record.latency_s)),
            (
                "attempt_started_at",
                timing.get("started_at").cloned().unwrap_or(Value::Null),
            ),
            (
                "attempt_finished_at",
                timing.get("finished_at").cloned().unwrap_or(Value::Null),
            ),
            (
                "latency_scope",
                timing.get("scope").cloned().unwrap_or(Value::Null),
            ),
            ("usage", Value::Object(usage)),
            (
                "cost",
                ordered(vec![
                    (
                        "reported_amount",
                        observation
                            .get("observed_cost_amount")
                            .cloned()
                            .unwrap_or(Value::Null),
                    ),
                    (
                        "reported_currency",
                        observation
                            .get("observed_cost_currency")
                            .cloned()
                            .unwrap_or(Value::Null),
                    ),
                    (
                        "reported_source",
                        observation
                            .get("observed_cost_source")
                            .cloned()
                            .unwrap_or(Value::Null),
                    ),
                    ("reported_cost_verified", json!(false)),
                    ("estimated_token_price_usd", reported_price.clone()),
                    (
                        "estimated_token_price_source",
                        if reported_price.is_null() {
                            Value::Null
                        } else {
                            json!("CLI list-price estimate; not actual charge")
                        },
                    ),
                    ("usd", json!(record.cost_usd)),
                    ("inr", json!(record.cost_inr)),
                    ("currency", json!("USD")),
                    ("usd_to_inr", json!(run.usd_to_inr)),
                    ("known", json!(record.cost_usd.is_some())),
                    ("method", json!(cost_method)),
                    ("verified_provider_charge_usd", Value::Null),
                    (
                        "fx_source",
                        json!("configured MJ_USD_INR; no live FX lookup"),
                    ),
                    ("price", config.get("price").cloned().unwrap_or(Value::Null)),
                    (
                        "scope",
                        config
                            .get("cost_scope")
                            .cloned()
                            .unwrap_or_else(|| json!("mock execution")),
                    ),
                ]),
            ),
            (
                "error",
                record
                    .error
                    .as_ref()
                    .map(|e| json!(redact_str(e, &[])))
                    .unwrap_or(Value::Null),
            ),
            ("artifact_reference", reference),
            (
                "output",
                if include_outputs {
                    record
                        .output
                        .as_ref()
                        .map(|o| json!(redact_str(o, &[])))
                        .unwrap_or(Value::Null)
                } else {
                    Value::Null
                },
            ),
        ]);
        results.push(result);
    }

    let aggregates = build_aggregates(&results);
    let verdicts = paired_verdicts(&results, &model_raw);
    let verdict_values: Vec<Value> = verdicts
        .iter()
        .map(|v| serde_json::to_value(v).unwrap_or(Value::Null))
        .collect();
    let modes: std::collections::HashSet<&str> = model_raw
        .iter()
        .filter_map(|m| m["evaluation_mode"].as_str())
        .collect();
    let eval_mode = if modes.len() == 1 {
        modes.iter().next().unwrap().to_string()
    } else {
        "mixed".to_string()
    };
    let value = ordered(vec![
        ("schema_version", json!("1.1")),
        (
            "benchmark",
            ordered(vec![
                ("name", json!(run.suite_name)),
                ("release", json!(run.suite_version)),
                (
                    "validation_status",
                    manifest
                        .pointer("/suite/validation_status")
                        .cloned()
                        .unwrap_or_else(|| json!("unvalidated_pilot")),
                ),
                (
                    "suite_sha256",
                    manifest
                        .pointer("/suite/sha256")
                        .cloned()
                        .unwrap_or(Value::Null),
                ),
            ]),
        ),
        (
            "run",
            ordered(vec![
                ("id", json!(run.id)),
                ("started_at", json!(run.started_at)),
                ("finished_at", json!(run.finished_at)),
                ("status", json!(run.status)),
                ("evaluation_mode", json!(eval_mode)),
                ("seed", json!(run.seed)),
                ("repeats", json!(run.repeats)),
                ("software_version", json!(run.mj_version)),
                (
                    "source",
                    manifest.get("source").cloned().unwrap_or(Value::Null),
                ),
                ("max_usd", json!(run.max_usd)),
                (
                    "cost_usd",
                    if run.uncertain_cost {
                        Value::Null
                    } else {
                        json!(run.spent_usd)
                    },
                ),
                ("known_spent_usd", json!(run.spent_usd)),
                ("reserved_usd", json!(run.reserved_usd)),
            ]),
        ),
        ("models", Value::Array(models)),
        ("results", Value::Array(results)),
        ("aggregates", Value::Array(aggregates)),
        ("verdicts", Value::Array(verdict_values)),
        ("supported_tracks", supported_tracks()),
        ("limitations", limitations()),
    ]);
    Ok(Export { value, verdicts })
}

fn tested_configuration(config: &Map<String, Value>, configuration_id: &str) -> Value {
    let get = |k: &str| config.get(k).cloned().unwrap_or(Value::Null);
    let mut m = Map::new();
    m.insert("runner_id".into(), get("runner_id"));
    m.insert("configuration_id".into(), json!(configuration_id));
    m.insert("kind".into(), get("kind"));
    m.insert("evaluation_mode".into(), get("evaluation_mode"));
    m.insert(
        "evaluation_track".into(),
        config
            .get("evaluation_track")
            .cloned()
            .unwrap_or_else(|| json!("unverified")),
    );
    for k in [
        "app_context",
        "verification_status",
        "configuration_name",
        "configuration_registry_id",
        "provider",
        "requested_model",
        "framework",
        "framework_version",
    ] {
        m.insert(k.into(), get(k));
    }
    m.insert(
        "tools".into(),
        config.get("tools").cloned().unwrap_or_else(|| json!([])),
    );
    m.insert("system_prompt".into(), get("system_prompt"));
    m.insert(
        "options".into(),
        config.get("options").cloned().unwrap_or_else(|| json!({})),
    );
    for k in [
        "weights_revision",
        "tokenizer_revision",
        "weights_license",
        "quantization",
        "hardware",
        "chat_template_sha256",
    ] {
        m.insert(k.into(), get(k));
    }
    // Pydantic extra="allow": keep any further manifest keys after the fields.
    for (k, v) in config {
        if !m.contains_key(k) && k != "configuration_id" {
            m.insert(k.clone(), v.clone());
        }
    }
    Value::Object(m)
}

fn artifact_reference(path: Option<&str>, directory: &Path) -> (Value, Value) {
    let Some(p) = path else {
        return (Value::Null, json!({}));
    };
    let artifact = Path::new(p);
    let Ok(real) = artifact.canonicalize() else {
        return (Value::Null, json!({}));
    };
    let Ok(root) = directory.canonicalize() else {
        return (Value::Null, json!({}));
    };
    if !real.starts_with(&root) || !real.is_file() {
        return (Value::Null, json!({}));
    }
    let rel = real
        .strip_prefix(&root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let sha = sha256_file(&real).unwrap_or_default();
    let raw = std::fs::read_to_string(&real)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("result").and_then(|r| r.get("raw")).cloned())
        .unwrap_or_else(|| json!({}));
    (json!({"path": rel, "sha256": sha}), raw)
}

fn build_aggregates(results: &[Value]) -> Vec<Value> {
    let mut groups: BTreeMap<(String, String, String, String), Vec<Value>> = BTreeMap::new();
    for r in results {
        let key = (
            r["runner_id"].as_str().unwrap_or_default().to_string(),
            r["capability_track"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            r["language"].as_str().unwrap_or_default().to_string(),
            r["domain"].as_str().unwrap_or("\u{1}null").to_string(),
        );
        groups.entry(key).or_default().push(r.clone());
    }
    // Python sorts groups by str(tuple) — domain None renders as None. Keep a
    // stable approximation: sort by the same tuple text.
    let mut keys: Vec<_> = groups.keys().cloned().collect();
    keys.sort_by_key(|(a, b, c, d)| {
        let domain = if d == "\u{1}null" {
            "None".to_string()
        } else {
            format!("'{d}'")
        };
        format!("('{a}', '{b}', '{c}', {domain})")
    });
    let mut out = Vec::new();
    for key in keys {
        let rows = &groups[&key];
        let (runner_id, track, language, domain_raw) = &key;
        let graded: Vec<&Value> = rows
            .iter()
            .filter(|r| r["grade_status"] == "graded" && r["status"] == "ok")
            .collect();
        let errors = rows.iter().filter(|r| r["status"] == "error").count() as i64;
        let passed = graded
            .iter()
            .filter(|r| r["passed"] == Value::Bool(true))
            .count() as i64;
        let denominator = graded.len() as i64 + errors;
        let costs: Vec<Option<f64>> = rows.iter().map(|r| r["cost"]["usd"].as_f64()).collect();
        let latencies: Vec<f64> = rows
            .iter()
            .filter_map(|r| r["latency_s"].as_f64())
            .collect();
        let scores: Vec<f64> = graded.iter().filter_map(|r| r["score"].as_f64()).collect();
        let tries = task_tries(rows);
        let rates: Vec<f64> = tries.values().map(|(r, c)| *r as f64 / *c as f64).collect();
        let repeated = tries.values().any(|(_, c)| *c > 1);
        let task_ci = if repeated {
            bootstrap_ci_mean(&rates)
        } else {
            None
        };
        let mode = rows[0]["evaluation_mode"].as_str().unwrap_or_default();
        out.push(ordered(vec![
            ("runner_id", json!(runner_id)),
            ("configuration_id", rows[0]["configuration_id"].clone()),
            ("capability_track", json!(track)),
            ("language", json!(language)),
            (
                "domain",
                if domain_raw == "\u{1}null" {
                    Value::Null
                } else {
                    json!(domain_raw)
                },
            ),
            ("evaluation_mode", json!(mode)),
            (
                "eligible_for_model_ranking",
                json!(mode == "live" && rows[0]["evaluation_track"] == "model_api"),
            ),
            ("evaluation_track", rows[0]["evaluation_track"].clone()),
            ("eligible_for_app_ranking", json!(false)),
            ("sample_count", json!(rows.len())),
            ("graded_count", json!(graded.len())),
            ("passed_count", json!(passed)),
            ("error_count", json!(errors)),
            (
                "skipped_count",
                json!(rows.iter().filter(|r| r["status"] == "skipped").count()),
            ),
            (
                "pending_count",
                json!(rows
                    .iter()
                    .filter(|r| r["grade_status"] == "pending")
                    .count()),
            ),
            (
                "grader_error_count",
                json!(rows.iter().filter(|r| r["grade_status"] == "error").count()),
            ),
            (
                "success_rate",
                if denominator > 0 {
                    json!(passed as f64 / denominator as f64)
                } else {
                    Value::Null
                },
            ),
            (
                "success_ci_95",
                match (repeated, task_ci) {
                    (true, ci) => ci.map(|c| json!([c.0, c.1])).unwrap_or(Value::Null),
                    _ => wilson_interval(passed, denominator)
                        .map(|c| json!([c.0, c.1]))
                        .unwrap_or(Value::Null),
                },
            ),
            (
                "mean_score",
                if scores.is_empty() {
                    Value::Null
                } else {
                    json!(mean(&scores))
                },
            ),
            (
                "mean_latency_s",
                if latencies.is_empty() {
                    Value::Null
                } else {
                    json!(mean(&latencies))
                },
            ),
            (
                "cost_usd",
                if costs.iter().all(|c| c.is_some()) {
                    json!(costs.iter().map(|c| c.unwrap()).sum::<f64>())
                } else {
                    Value::Null
                },
            ),
            ("small_sample", json!(tries.len() < 30)),
            (
                "median_latency_s",
                if latencies.is_empty() {
                    Value::Null
                } else {
                    json!(median(&latencies))
                },
            ),
            ("task_count", json!(tries.len())),
            (
                "tries_per_task",
                json!(tries.values().map(|(_, c)| *c).max().unwrap_or(0)),
            ),
            (
                "always_correct",
                json!(tries.values().filter(|(r, c)| r == c).count()),
            ),
            (
                "sometimes_correct",
                json!(tries.values().filter(|(r, c)| *r > 0 && r < c).count()),
            ),
            (
                "never_correct",
                json!(tries.values().filter(|(r, _)| *r == 0).count()),
            ),
            (
                "success_ci_method",
                json!(if repeated { "task_bootstrap" } else { "wilson" }),
            ),
        ]));
    }
    out
}

fn supported_tracks() -> Value {
    ordered(vec![
        (
            "consumer_app",
            json!("manual evidence imports; contributor-reported; review pending"),
        ),
        (
            "model_api",
            json!("direct API adapters; separate from app/subscription plans"),
        ),
        ("text", json!("supported")),
        (
            "browser_workflow",
            json!("offline action-plan state checks"),
        ),
        (
            "harness",
            json!("Claude tool-free synthetic drafts; other harnesses are stubs"),
        ),
        ("personal_agent", json!("stub")),
        ("image_understanding", json!("unsupported")),
        ("image_generation", json!("unsupported")),
        ("asr", json!("unsupported")),
        ("tts", json!("unsupported")),
        ("video_analysis", json!("unsupported")),
        ("video_generation", json!("unsupported")),
    ])
}

fn limitations() -> Value {
    json!([
        "Auditable pilot; no India-wide validated ranking.",
        "Mock checks are not model evaluations; live labels identify adapter execution, not validation.",
        "Success denominator is graded attempts plus execution errors; pending/skipped excluded.",
        "Verdicts compare systems on the same track and tasks; 'too close to call' means the gap could be chance, not that the systems are equal.",
        "Browser workflow executes an offline state reducer, not a real browser agent.",
        "Costs cover API usage; subscriptions, hardware and electricity are excluded.",
        "Raw artifacts are private; default export omits prompts and output text.",
        "Manual observations grade recorded responses; app/plan context is contributor-reported. An API model result never evaluates an app subscription tier.",
    ])
}

pub fn write_export(
    store: &Store,
    run_id: &str,
    destination: &Path,
    include_outputs: bool,
) -> Result<PathBuf, String> {
    let payload = result_export(store, run_id, include_outputs)?;
    // Python: model_dump_json(indent=2) — ensure_ascii escapes non-ASCII.
    let text = dump_json_ascii(&payload.value);
    std::fs::write(destination, text + "\n").map_err(|e| e.to_string())?;
    Ok(destination.to_path_buf())
}

/// Pydantic's model_dump_json escapes non-ASCII (₹ as \u20b9). Mirror it so
/// results.json matches the Python bytes apart from volatile fields.
pub fn dump_json_ascii(v: &Value) -> String {
    let pretty = serde_json::to_string_pretty(v).unwrap_or_default();
    let mut out = String::with_capacity(pretty.len());
    for c in pretty.chars() {
        if (c as u32) < 0x80 {
            out.push(c);
        } else {
            let mut buf = [0u16; 2];
            for u in c.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{:04x}", u));
            }
        }
    }
    out
}

pub fn explanation_key(task_id: &str, runner: &str, repeat_index: i64) -> String {
    if repeat_index != 0 {
        format!("{task_id}:{runner}#{repeat_index}")
    } else {
        format!("{task_id}:{runner}")
    }
}

pub fn redact_value(v: &Value) -> Value {
    redact(v)
}
