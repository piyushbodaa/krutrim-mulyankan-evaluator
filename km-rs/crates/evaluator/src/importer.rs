//! `km import-suite`: convert a foreign suite directory to Mulyankan YAML,
//! mirroring importer.py. Nothing is executed; unrecognised files are listed.

use crate::suite::Suite;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub struct ImportedSuite {
    pub suite: Suite,
    pub unhandled: Vec<String>,
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir()
                && !p
                    .file_name()
                    .map(|n| n.to_string_lossy().starts_with('.'))
                    .unwrap_or(false)
            {
                collect(&p, out);
            } else if p.is_file() {
                out.push(p);
            }
        }
    }
}

pub fn import_suite(source: &Path, name: Option<&str>) -> Result<ImportedSuite, String> {
    let mut files = Vec::new();
    collect(source, &mut files);
    files.sort();
    let mut tasks: Vec<Map<String, Value>> = Vec::new();
    let mut payload_count = 0;
    let mut unhandled = Vec::new();
    for path in &files {
        let rel = path
            .strip_prefix(source)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        match ext.as_str() {
            "yaml" | "yml" => {
                let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
                match serde_yaml::from_str::<serde_yaml::Value>(&text) {
                    Ok(doc) if is_our_suite(&doc) => {
                        let suite = crate::suite::parse_suite(&doc, path)?;
                        for t in &suite.tasks {
                            if let Value::Object(m) = t.dump_json() {
                                tasks.push(m);
                            }
                        }
                        payload_count += 1;
                    }
                    Ok(doc) => {
                        let imported = convert_records(&doc);
                        if imported.is_empty() {
                            unhandled.push(rel);
                        } else {
                            tasks.extend(imported);
                            payload_count += 1;
                        }
                    }
                    Err(_) => unhandled.push(rel),
                }
            }
            "json" => {
                let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
                match serde_json::from_str::<Value>(&text) {
                    Ok(doc) => {
                        let imported = convert_json_records(&doc);
                        if imported.is_empty() {
                            unhandled.push(rel);
                        } else {
                            tasks.extend(imported);
                            payload_count += 1;
                        }
                    }
                    Err(_) => unhandled.push(rel),
                }
            }
            "md" | "txt" => {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("task");
                let text = std::fs::read_to_string(path).unwrap_or_default();
                tasks.push(draft(stem, text.trim(), "llm_judge"));
                payload_count += 1;
            }
            _ => unhandled.push(rel),
        }
    }
    if payload_count == 0 {
        return Err(format!(
            "no recognised suite files under {}; expected Mulyankan YAML, JSON records, or .md/.txt prompts",
            source.display()
        ));
    }
    if tasks.is_empty() {
        return Err("recognized files did not contain valid tasks/projects".into());
    }
    let mut seen: Vec<String> = Vec::new();
    for task in &mut tasks {
        let mut id = sanitise(task["id"].as_str().unwrap_or("task"));
        let base = id.clone();
        let mut n = 2;
        while seen.contains(&id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        task.insert("id".into(), json!(id.clone()));
        seen.push(id);
        task.remove("path");
    }
    let suite_name = name.map(|n| n.to_string()).unwrap_or_else(|| {
        source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "imported-suite".into())
    });
    let doc = json!({
        "name": suite_name, "version": "0.1.0",
        "description": format!("Imported from {}; validate before use.", source.display()),
        "validation_status": "unvalidated_pilot",
        "tasks": tasks,
    });
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(&serde_json::to_string(&doc).unwrap_or_default())
            .map_err(|e| e.to_string())?;
    let suite = crate::suite::parse_suite(&yaml, Path::new(&format!("{suite_name}.yaml")))?;
    Ok(ImportedSuite { suite, unhandled })
}

fn is_our_suite(doc: &serde_yaml::Value) -> bool {
    doc.as_mapping()
        .map(|m| {
            m.contains_key(&serde_yaml::Value::String("tasks".into()))
                || m.contains_key(&serde_yaml::Value::String("version".into()))
        })
        .unwrap_or(false)
}

fn draft(task_id: &str, prompt: &str, grader: &str) -> Map<String, Value> {
    let grader = if grader == "llm_judge" {
        json!({"type": "llm_judge"})
    } else {
        json!({"type": "exact", "expected": grader})
    };
    let mut m = Map::new();
    m.insert("id".into(), json!(sanitise(task_id)));
    m.insert("category".into(), json!("imported"));
    m.insert("prompt".into(), json!(prompt));
    m.insert("grader".into(), grader);
    if m["grader"]["type"] == "llm_judge" {
        m.insert(
            "rubric".into(),
            json!("Score whether the answer directly and correctly fulfils the task prompt."),
        );
    }
    m
}

fn clean_extras(mut record: Map<String, Value>) -> Map<String, Value> {
    const ALLOWED: &[&str] = &[
        "id",
        "version",
        "category",
        "capability_track_version",
        "capability_track",
        "language",
        "script",
        "input_style",
        "domain",
        "as_of",
        "provenance",
        "risk_flags",
        "context",
        "prompt",
        "grader",
        "rubric",
        "reference",
    ];
    record.retain(|k, _| ALLOWED.contains(&k.as_str()));
    record
}

fn translate(record: &Map<String, Value>, fallback_id: &str) -> Option<Map<String, Value>> {
    if record.contains_key("prompt") && record.contains_key("grader") {
        return Some(clean_extras(record.clone()));
    }
    if record.contains_key("prompt") {
        let mut m = Map::new();
        for (k, v) in record {
            m.insert(k.clone(), v.clone());
        }
        m.insert("grader".into(), json!({"type": "llm_judge"}));
        m.insert(
            "rubric".into(),
            json!("Score whether the answer directly and correctly fulfils the task prompt."),
        );
        return Some(clean_extras(m));
    }
    let get_str = |keys: &[&str]| -> Option<String> {
        keys.iter().find_map(|k| {
            record
                .get(*k)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
    };
    let prompt = get_str(&["question", "instruction", "input"])?;
    let answer = get_str(&["answer", "expected", "output", "label"])?;
    let id = record
        .get("id")
        .or_else(|| record.get("task_id"))
        .and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Null | _ => None,
        })
        .unwrap_or_else(|| fallback_id.to_string());
    let mut m = Map::new();
    m.insert("id".into(), json!(id));
    m.insert(
        "category".into(),
        json!(record
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("imported")),
    );
    m.insert("prompt".into(), json!(prompt));
    m.insert(
        "grader".into(),
        json!({"type": "exact", "expected": answer.trim()}),
    );
    Some(m)
}

fn convert_records(doc: &serde_yaml::Value) -> Vec<Map<String, Value>> {
    // Reuse the JSON path by stringifying through JSON text.
    let text = serde_yaml::to_string(doc).unwrap_or_default();
    let as_yaml: Result<serde_yaml::Value, _> = serde_yaml::from_str(&text);
    let _ = as_yaml;
    let json_text = yaml_to_json_text(doc);
    match serde_json::from_str::<Value>(&json_text) {
        Ok(v) => convert_json_records(&v),
        Err(_) => Vec::new(),
    }
}

fn yaml_to_json_text(doc: &serde_yaml::Value) -> String {
    fn conv(v: &serde_yaml::Value) -> Value {
        match v {
            serde_yaml::Value::Null => Value::Null,
            serde_yaml::Value::Bool(b) => Value::Bool(*b),
            serde_yaml::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    json!(i)
                } else {
                    json!(n.as_f64().unwrap_or(0.0))
                }
            }
            serde_yaml::Value::String(s) => json!(s),
            serde_yaml::Value::Sequence(items) => Value::Array(items.iter().map(conv).collect()),
            serde_yaml::Value::Mapping(m) => {
                let mut o = Map::new();
                for (k, val) in m {
                    if let serde_yaml::Value::String(s) = k {
                        o.insert(s.clone(), conv(val));
                    }
                }
                Value::Object(o)
            }
            serde_yaml::Value::Tagged(t) => conv(&t.value),
        }
    }
    conv(doc).to_string()
}

fn convert_json_records(doc: &Value) -> Vec<Map<String, Value>> {
    let mut out = Vec::new();
    match doc {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                if let Value::Object(m) = item {
                    if let Some(task) = translate(m, &format!("task-{}", i + 1)) {
                        out.push(task);
                    }
                }
            }
        }
        Value::Object(m) => {
            for key in ["tasks", "examples", "items"] {
                if let Some(Value::Array(items)) = m.get(key) {
                    for (i, item) in items.iter().enumerate() {
                        if let Value::Object(mm) = item {
                            if let Some(task) = translate(mm, &format!("task-{}", i + 1)) {
                                out.push(task);
                            }
                        }
                    }
                    return out;
                }
            }
            if let Some(task) = translate(m, "task-1") {
                out.push(task);
            }
        }
        _ => {}
    }
    out
}

fn sanitise(text: &str) -> String {
    let lowered: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = lowered.trim_matches('_');
    if trimmed.is_empty() || !trimmed.chars().next().unwrap().is_ascii_alphanumeric() {
        format!("task-{trimmed}").trim_end_matches('-').to_string()
    } else {
        trimmed.to_string()
    }
}

/// Render an imported suite in Pydantic dump field order (like
/// yaml.safe_dump(..., allow_unicode=True, sort_keys=False)).
pub fn suite_to_yaml(suite: &Suite) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "name: {}", yaml_scalar(&suite.name));
    let _ = writeln!(out, "version: {}", yaml_scalar(&suite.version));
    let _ = writeln!(out, "description: {}", yaml_scalar(&suite.description));
    let _ = writeln!(out, "validation_status: {}", suite.validation_status);
    out.push_str("tasks:\n");
    for task in &suite.tasks {
        let dump = task.dump_json();
        out.push_str(&yaml_value(&Value::Array(vec![dump]), 0));
        // yaml_value renders "- key: ..." already at this indent.
    }
    out
}

fn yaml_scalar(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn yaml_value(v: &Value, indent: usize) -> String {
    // Delegate to serde_yaml for correctness; field order follows the
    // insertion order of dump_json's intermediate Value (sorted keys there,
    // which differs slightly from Pydantic's dump order but loads identically).
    let _ = indent;
    serde_yaml::to_string(v).unwrap_or_default()
}

pub fn group_clean(map: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    map.clone()
}
