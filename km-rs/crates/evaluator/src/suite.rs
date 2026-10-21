//! Task suite schema and YAML loader, mirroring suite.py (Pydantic).
//! Unknown fields are refused, defaults match, and `content_hash` hashes the
//! same canonical JSON the Python model produces.

use crate::pypy::{py_json_dumps, sha256_hex};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq)]
pub enum GraderSpec {
    /// The checks behind tests people write in the desktop app. Unlike the
    /// strict pilot graders, these read a reply the way a person would:
    /// the final number in "The total is ₹2,500/-", "Tuesday." for
    /// "Tuesday", and whole words only, so "15" never counts as "5".
    Answer {
        check: AnswerCheck,
        expected: Vec<String>,
        tolerance: Option<(Decimal, String)>,
    },
    Exact {
        expected: Vec<String>,
        case_sensitive: bool,
        normalize_whitespace: bool,
    },
    LlmJudge {
        mode: String,
        judge_model: Option<String>,
        pass_threshold: i64,
    },
    Json {
        expected: Value,
    },
    Numeric {
        expected: Decimal,
        expected_raw: String,
        tolerance: Decimal,
        tolerance_raw: String,
    },
    Regex {
        pattern: String,
        case_sensitive: bool,
    },
    Workflow {
        fixture: PathBuf,
        expected: Map<String, Value>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerCheck {
    Number,
    Exact,
    Mentions,
}

impl AnswerCheck {
    pub fn as_str(&self) -> &'static str {
        match self {
            AnswerCheck::Number => "number",
            AnswerCheck::Exact => "exact",
            AnswerCheck::Mentions => "mentions",
        }
    }

    pub fn parse(value: &str) -> Option<AnswerCheck> {
        match value {
            "number" => Some(AnswerCheck::Number),
            "exact" => Some(AnswerCheck::Exact),
            "mentions" => Some(AnswerCheck::Mentions),
            _ => None,
        }
    }
}

impl GraderSpec {
    pub fn kind(&self) -> &'static str {
        match self {
            GraderSpec::Answer { .. } => "answer",
            GraderSpec::Exact { .. } => "exact",
            GraderSpec::LlmJudge { .. } => "llm_judge",
            GraderSpec::Json { .. } => "json",
            GraderSpec::Numeric { .. } => "numeric",
            GraderSpec::Regex { .. } => "regex",
            GraderSpec::Workflow { .. } => "workflow",
        }
    }

    fn dump_json(&self) -> Value {
        match self {
            GraderSpec::Answer {
                check,
                expected,
                tolerance,
            } => json!({
                "check": check.as_str(), "expected": expected,
                "tolerance": tolerance.as_ref().map(|(_, raw)| raw), "type": "answer"}),
            GraderSpec::Exact {
                expected,
                case_sensitive,
                normalize_whitespace,
            } => json!({
                "case_sensitive": case_sensitive, "expected": expected,
                "normalize_whitespace": normalize_whitespace, "type": "exact"}),
            GraderSpec::LlmJudge {
                mode,
                judge_model,
                pass_threshold,
            } => json!({
                "judge_model": judge_model, "mode": mode,
                "pass_threshold": pass_threshold, "type": "llm_judge"}),
            GraderSpec::Json { expected } => json!({
                "expected": expected, "mode": "exact", "type": "json"}),
            GraderSpec::Numeric {
                expected_raw,
                tolerance_raw,
                ..
            } => json!({
                "expected": expected_raw, "tolerance": tolerance_raw, "type": "numeric"}),
            GraderSpec::Regex {
                pattern,
                case_sensitive,
            } => json!({
                "case_sensitive": case_sensitive, "pattern": pattern, "type": "regex"}),
            GraderSpec::Workflow { fixture, expected } => json!({
                "expected": expected, "fixture": path_str(fixture), "type": "workflow"}),
        }
    }
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub version: i64,
    pub category: String,
    pub capability_track_version: String,
    pub capability_track: String,
    pub language: String,
    pub script: Option<String>,
    pub input_style: Option<String>,
    pub domain: Option<String>,
    pub as_of: Option<String>,
    pub provenance: Vec<Value>,
    pub risk_flags: Vec<String>,
    pub context: Value,
    pub prompt: String,
    pub inputs: BTreeMap<String, String>,
    pub fixtures: Vec<PathBuf>,
    pub grader: GraderSpec,
    pub rubric: Option<String>,
    pub reference: Option<String>,
    pub max_output_tokens: i64,
    pub timeout_s: f64,
    pub budget_usd: Option<f64>,
}

impl Task {
    pub fn render_prompt(&self) -> String {
        render_template(&self.prompt, &self.inputs)
    }

    /// model_dump(mode="json"): every field, defaults included.
    pub fn dump_json(&self) -> Value {
        json!({
            "as_of": self.as_of,
            "budget_usd": self.budget_usd,
            "capability_track": self.capability_track,
            "capability_track_version": self.capability_track_version,
            "category": self.category,
            "context": self.context,
            "domain": self.domain,
            "fixtures": self.fixtures.iter().map(|p| path_str(p)).collect::<Vec<_>>(),
            "grader": self.grader.dump_json(),
            "id": self.id,
            "input_style": self.input_style,
            "inputs": self.inputs,
            "language": self.language,
            "max_output_tokens": self.max_output_tokens,
            "prompt": self.prompt,
            "provenance": self.provenance,
            "reference": self.reference,
            "risk_flags": self.risk_flags,
            "rubric": self.rubric,
            "script": self.script,
            "timeout_s": self.timeout_s,
            "version": self.version,
        })
    }

    pub fn content_hash(&self) -> String {
        sha256_hex(py_json_dumps(&self.dump_json()).as_bytes())
    }

    /// The metadata subset the manifest records, in Pydantic field order.
    pub fn metadata_json(&self) -> Value {
        let all = self.dump_json();
        let mut m = Map::new();
        for k in [
            "category",
            "capability_track",
            "capability_track_version",
            "language",
            "script",
            "input_style",
            "domain",
            "as_of",
            "provenance",
            "risk_flags",
        ] {
            m.insert(k.to_string(), all[k].clone());
        }
        Value::Object(m)
    }
}

/// {{ name }} substitution, like suite.py's regex over the prompt.
pub fn render_template(prompt: &str, inputs: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(prompt.len());
    let mut rest = prompt;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let name = after[..end].trim();
                if is_ident(name) {
                    if let Some(v) = inputs.get(name) {
                        out.push_str(v);
                    } else {
                        out.push_str(&rest[start..start + 2 + end + 2]);
                    }
                    rest = &after[end + 2..];
                } else {
                    out.push_str("{{");
                    rest = after;
                }
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    }
}

#[derive(Debug, Clone)]
pub struct Suite {
    pub name: String,
    pub version: String,
    pub description: String,
    pub validation_status: String,
    pub tasks: Vec<Task>,
    pub path: Option<PathBuf>,
}

impl Suite {
    pub fn select(&self, task_ids: Option<&[String]>) -> Result<Vec<Task>, String> {
        let Some(ids) = task_ids.filter(|v| !v.is_empty()) else {
            return Ok(self.tasks.clone());
        };
        if ids.len() != ids.iter().collect::<HashSet<_>>().len() {
            return Err("duplicate selected task ids".into());
        }
        let unknown: Vec<&String> = ids
            .iter()
            .filter(|id| !self.tasks.iter().any(|t| &t.id == *id))
            .collect();
        if !unknown.is_empty() {
            return Err(format!(
                "unknown task ids in suite '{}': [{}]",
                self.name,
                unknown
                    .iter()
                    .map(|s| format!("'{s}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Ok(ids
            .iter()
            .map(|id| self.tasks.iter().find(|t| &t.id == id).unwrap().clone())
            .collect())
    }

    pub fn dump_json(&self) -> Value {
        json!({
            "name": self.name,
            "version": self.version,
            "description": self.description,
            "validation_status": self.validation_status,
            "tasks": self.tasks.iter().map(|t| t.dump_json()).collect::<Vec<_>>(),
        })
    }
}

type Yaml = serde_yaml::Value;

fn ymap<'a>(v: &'a Yaml, what: &str) -> Result<&'a serde_yaml::Mapping, String> {
    v.as_mapping()
        .ok_or_else(|| format!("{what}: expected a mapping"))
}

fn yget<'a>(m: &'a serde_yaml::Mapping, key: &str) -> Option<&'a Yaml> {
    m.get(Yaml::String(key.to_string()))
}

fn ystr(v: &Yaml, what: &str) -> Result<String, String> {
    match v {
        Yaml::String(s) => Ok(s.clone()),
        Yaml::Number(n) => Ok(n.to_string()),
        Yaml::Bool(b) => Ok(b.to_string()),
        _ => Err(format!("{what}: expected a string")),
    }
}

fn yopt_str(m: &serde_yaml::Mapping, key: &str) -> Result<Option<String>, String> {
    match yget(m, key) {
        None | Some(Yaml::Null) => Ok(None),
        Some(v) => Ok(Some(ystr(v, key)?)),
    }
}

fn ybool(v: Option<&Yaml>, default: bool, what: &str) -> Result<bool, String> {
    match v {
        None | Some(Yaml::Null) => Ok(default),
        Some(Yaml::Bool(b)) => Ok(*b),
        Some(_) => Err(format!("{what}: expected true or false")),
    }
}

fn yint(v: Option<&Yaml>, default: i64, what: &str) -> Result<i64, String> {
    match v {
        None | Some(Yaml::Null) => Ok(default),
        Some(Yaml::Number(n)) => n
            .as_i64()
            .ok_or_else(|| format!("{what}: expected an integer")),
        Some(_) => Err(format!("{what}: expected an integer")),
    }
}

fn yfloat(v: &Yaml, what: &str) -> Result<f64, String> {
    v.as_f64()
        .ok_or_else(|| format!("{what}: expected a number"))
}

fn ylist_str(m: &serde_yaml::Mapping, key: &str) -> Result<Vec<String>, String> {
    match yget(m, key) {
        None | Some(Yaml::Null) => Ok(Vec::new()),
        Some(Yaml::Sequence(items)) => items.iter().map(|x| ystr(x, key)).collect(),
        Some(_) => Err(format!("{key}: expected a list")),
    }
}

/// YAML value to JSON value (numbers keep ints as ints; decimals as f64).
fn y2j(v: &Yaml) -> Value {
    match v {
        Yaml::Null => Value::Null,
        Yaml::Bool(b) => Value::Bool(*b),
        Yaml::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Number(i.into())
            } else if let Some(u) = n.as_u64() {
                Value::Number(u.into())
            } else {
                serde_json::Number::from_f64(n.as_f64().unwrap_or(0.0))
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
        }
        Yaml::String(s) => Value::String(s.clone()),
        Yaml::Sequence(items) => Value::Array(items.iter().map(y2j).collect()),
        Yaml::Mapping(m) => {
            let mut out = Map::new();
            for (k, val) in m {
                if let Yaml::String(ks) = k {
                    out.insert(ks.clone(), y2j(val));
                }
            }
            Value::Object(out)
        }
        Yaml::Tagged(t) => y2j(&t.value),
    }
}

fn forbid_unknown(m: &serde_yaml::Mapping, allowed: &[&str], what: &str) -> Result<(), String> {
    for k in m.keys() {
        if let Yaml::String(s) = k {
            if !allowed.contains(&s.as_str()) {
                return Err(format!("{what}: unexpected field '{s}'"));
            }
        }
    }
    Ok(())
}

pub fn placeholder_names(prompt: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = prompt;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let name = after[..end].trim();
                if is_ident(name) {
                    names.push(name.to_string());
                }
                rest = &after[end + 2..];
            }
            None => break,
        }
    }
    names
}

fn decimal_of(v: &Yaml, what: &str) -> Result<(Decimal, String), String> {
    let raw = match v {
        Yaml::Number(n) => n.to_string(),
        Yaml::String(s) => s.clone(),
        _ => return Err(format!("{what}: expected a number")),
    };
    let dec = Decimal::from_str(&raw).map_err(|_| format!("{what}: expected a number"))?;
    Ok((dec, dec.to_string()))
}

fn parse_grader(v: &Yaml) -> Result<GraderSpec, String> {
    let m = ymap(v, "grader")?;
    let kind = yget(m, "type")
        .map(|x| ystr(x, "grader.type"))
        .transpose()?
        .unwrap_or_default();
    match kind.as_str() {
        "answer" => {
            forbid_unknown(m, &["type", "check", "expected", "tolerance"], "grader")?;
            let check = yopt_str(m, "check")?.ok_or("grader.check: field required")?;
            let check = AnswerCheck::parse(&check)
                .ok_or("grader.check: expected number, exact or mentions")?;
            let expected: Vec<String> = match yget(m, "expected") {
                Some(Yaml::Sequence(items)) => items
                    .iter()
                    .map(|x| ystr(x, "grader.expected"))
                    .collect::<Result<_, _>>()?,
                Some(other) => vec![ystr(other, "grader.expected")?],
                None => return Err("grader.expected: field required".into()),
            };
            if expected.is_empty() || expected.iter().any(|e| e.trim().is_empty()) {
                return Err("grader.expected: every answer must have text".into());
            }
            if check == AnswerCheck::Number {
                for value in &expected {
                    crate::graders::indic_number(value)
                        .map_err(|e| format!("grader.expected: {value}: {e}"))?;
                }
            }
            let tolerance = match yget(m, "tolerance") {
                None | Some(Yaml::Null) => None,
                Some(_) if check != AnswerCheck::Number => {
                    return Err("grader.tolerance: only number checks take a tolerance".into())
                }
                Some(v) => Some(decimal_of(v, "grader.tolerance")?),
            };
            Ok(GraderSpec::Answer {
                check,
                expected,
                tolerance,
            })
        }
        "exact" => {
            forbid_unknown(
                m,
                &["type", "expected", "case_sensitive", "normalize_whitespace"],
                "grader",
            )?;
            let expected = match yget(m, "expected") {
                Some(Yaml::String(s)) => vec![s.clone()],
                Some(Yaml::Sequence(items)) => items
                    .iter()
                    .map(|x| ystr(x, "grader.expected"))
                    .collect::<Result<_, _>>()?,
                Some(other) => vec![ystr(other, "grader.expected")?],
                None => return Err("grader.expected: field required".into()),
            };
            if expected.is_empty() {
                return Err("grader.expected: expected at least one answer".into());
            }
            Ok(GraderSpec::Exact {
                expected,
                case_sensitive: ybool(yget(m, "case_sensitive"), false, "case_sensitive")?,
                normalize_whitespace: ybool(
                    yget(m, "normalize_whitespace"),
                    true,
                    "normalize_whitespace",
                )?,
            })
        }
        "llm_judge" => {
            forbid_unknown(
                m,
                &["type", "mode", "judge_model", "pass_threshold"],
                "grader",
            )?;
            let mode = yopt_str(m, "mode")?.unwrap_or_else(|| "absolute".into());
            if mode != "absolute" && mode != "pairwise" {
                return Err("grader.mode: expected absolute or pairwise".into());
            }
            Ok(GraderSpec::LlmJudge {
                mode,
                judge_model: yopt_str(m, "judge_model")?,
                pass_threshold: yint(yget(m, "pass_threshold"), 7, "pass_threshold")?,
            })
        }
        "json" => {
            forbid_unknown(m, &["type", "expected", "mode"], "grader")?;
            let expected = yget(m, "expected").ok_or("grader.expected: field required")?;
            Ok(GraderSpec::Json {
                expected: y2j(expected),
            })
        }
        "numeric" => {
            forbid_unknown(m, &["type", "expected", "tolerance"], "grader")?;
            let (expected, expected_raw) = decimal_of(
                yget(m, "expected").ok_or("grader.expected: field required")?,
                "grader.expected",
            )?;
            let (tolerance, tolerance_raw) = match yget(m, "tolerance") {
                None => (Decimal::ZERO, "0".to_string()),
                Some(v) => decimal_of(v, "grader.tolerance")?,
            };
            Ok(GraderSpec::Numeric {
                expected,
                expected_raw,
                tolerance,
                tolerance_raw,
            })
        }
        "regex" => {
            forbid_unknown(m, &["type", "pattern", "case_sensitive"], "grader")?;
            let pattern = yopt_str(m, "pattern")?.ok_or("grader.pattern: field required")?;
            regex::Regex::new(&pattern).map_err(|e| format!("grader.pattern: {e}"))?;
            Ok(GraderSpec::Regex {
                pattern,
                case_sensitive: ybool(yget(m, "case_sensitive"), true, "case_sensitive")?,
            })
        }
        "workflow" => {
            forbid_unknown(m, &["type", "fixture", "expected"], "grader")?;
            let fixture = yopt_str(m, "fixture")?.ok_or("grader.fixture: field required")?;
            let expected = match y2j(yget(m, "expected").ok_or("grader.expected: field required")?)
            {
                Value::Object(o) => o,
                _ => return Err("grader.expected: expected a mapping".into()),
            };
            Ok(GraderSpec::Workflow {
                fixture: PathBuf::from(fixture),
                expected,
            })
        }
        other => Err(format!("grader: unknown type '{other}'")),
    }
}

const TASK_FIELDS: &[&str] = &[
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
    "inputs",
    "fixtures",
    "grader",
    "rubric",
    "reference",
    "max_output_tokens",
    "timeout_s",
    "budget_usd",
];

fn parse_task(v: &Yaml, path: &Path) -> Result<Task, String> {
    let m = ymap(v, "task")?;
    forbid_unknown(m, TASK_FIELDS, "task")?;
    let id = yopt_str(m, "id")?.ok_or("task.id: field required")?;
    if !regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
        .unwrap()
        .is_match(&id)
    {
        return Err(format!(
            "task id '{id}' does not match the required pattern"
        ));
    }
    let prompt = yopt_str(m, "prompt")?.ok_or("task.prompt: field required")?;
    if prompt.is_empty() {
        return Err("task.prompt: must not be empty".into());
    }
    let mut inputs = BTreeMap::new();
    if let Some(Yaml::Mapping(im)) = yget(m, "inputs") {
        for (k, val) in im {
            if let Yaml::String(ks) = k {
                inputs.insert(ks.clone(), ystr(val, "task.inputs")?);
            }
        }
    }
    let missing: Vec<String> = placeholder_names(&prompt)
        .into_iter()
        .filter(|n| !inputs.contains_key(n))
        .collect();
    if !missing.is_empty() {
        return Err(format!("prompt references unknown inputs: {missing:?}"));
    }
    let grader = parse_grader(yget(m, "grader").ok_or("task.grader: field required")?)?;
    let rubric = yopt_str(m, "rubric")?;
    if grader.kind() == "llm_judge" && rubric.as_deref().unwrap_or("").is_empty() {
        return Err("llm_judge tasks need a rubric".into());
    }
    let fixtures: Vec<PathBuf> = ylist_str(m, "fixtures")?
        .into_iter()
        .map(PathBuf::from)
        .collect();
    if fixtures
        .iter()
        .any(|p| p.is_absolute() || p.components().any(|c| c.as_os_str() == ".."))
    {
        return Err("fixtures must be relative paths inside the suite directory".into());
    }
    if let GraderSpec::Workflow { fixture, .. } = &grader {
        if !fixtures.contains(fixture) {
            return Err("workflow grader fixture must be listed in task fixtures".into());
        }
    }
    let _ = path;
    let track = yopt_str(m, "capability_track")?.unwrap_or_else(|| "text".into());
    const TRACKS: &[&str] = &[
        "text",
        "browser_workflow",
        "image_understanding",
        "image_generation",
        "asr",
        "tts",
        "video_analysis",
        "video_generation",
    ];
    if !TRACKS.contains(&track.as_str()) {
        return Err(format!("task.capability_track: unknown track '{track}'"));
    }
    let provenance = match yget(m, "provenance") {
        Some(Yaml::Sequence(items)) => items.iter().map(y2j).collect(),
        _ => Vec::new(),
    };
    Ok(Task {
        id,
        version: yint(yget(m, "version"), 1, "task.version")?,
        category: yopt_str(m, "category")?.ok_or("task.category: field required")?,
        capability_track_version: yopt_str(m, "capability_track_version")?
            .unwrap_or_else(|| "1.0".into()),
        capability_track: track,
        language: yopt_str(m, "language")?.unwrap_or_else(|| "und".into()),
        script: yopt_str(m, "script")?,
        input_style: yopt_str(m, "input_style")?,
        domain: yopt_str(m, "domain")?,
        as_of: yopt_str(m, "as_of")?,
        provenance,
        risk_flags: ylist_str(m, "risk_flags")?,
        context: yget(m, "context").map(y2j).unwrap_or_else(|| json!({})),
        prompt,
        inputs,
        fixtures,
        grader,
        rubric,
        reference: yopt_str(m, "reference")?,
        max_output_tokens: yint(yget(m, "max_output_tokens"), 4096, "max_output_tokens")?,
        timeout_s: match yget(m, "timeout_s") {
            None => 120.0,
            Some(v) => yfloat(v, "timeout_s")?,
        },
        budget_usd: match yget(m, "budget_usd") {
            None | Some(Yaml::Null) => None,
            Some(v) => Some(yfloat(v, "budget_usd")?),
        },
    })
}

pub fn load_suite(path: &Path) -> Result<Suite, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc: Yaml = serde_yaml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_suite(&doc, path)
}

pub fn parse_suite(doc: &Yaml, path: &Path) -> Result<Suite, String> {
    let m = ymap(doc, "suite").map_err(|_| {
        format!(
            "{}: expected a YAML mapping at the top level",
            path.display()
        )
    })?;
    forbid_unknown(
        m,
        &[
            "name",
            "version",
            "description",
            "validation_status",
            "tasks",
            "path",
        ],
        "suite",
    )?;
    let version = match yget(m, "version") {
        Some(Yaml::String(s)) => s.clone(),
        Some(Yaml::Number(n)) if n.is_i64() || n.is_u64() => n.to_string(),
        Some(Yaml::Number(n)) => {
            let f = n.as_f64().unwrap_or(0.0);
            if f == f.trunc() {
                format!("{f:.1}")
            } else {
                format!("{f}")
            }
        }
        _ => return Err("suite.version: field required".into()),
    };
    let task_items = match yget(m, "tasks") {
        Some(Yaml::Sequence(items)) if !items.is_empty() => items.clone(),
        _ => return Err("suite.tasks: expected a non-empty list".into()),
    };
    let mut tasks = Vec::new();
    for item in &task_items {
        let task = parse_task(item, path)?;
        for fixture in &task.fixtures {
            let base = path.parent().unwrap_or(Path::new("."));
            let resolved = base.join(fixture);
            if !resolved.is_file() {
                return Err(format!(
                    "{}: task '{}' fixture not found: {}",
                    path.display(),
                    task.id,
                    fixture.display()
                ));
            }
        }
        tasks.push(task);
    }
    let mut seen = HashSet::new();
    let mut dupes = Vec::new();
    for t in &tasks {
        if !seen.insert(t.id.clone()) {
            dupes.push(t.id.clone());
        }
    }
    if !dupes.is_empty() {
        return Err(format!("duplicate task ids: {dupes:?}"));
    }
    let status = yopt_str(m, "validation_status")?.unwrap_or_else(|| "unvalidated_pilot".into());
    if !["smoke", "unvalidated_pilot", "validated"].contains(&status.as_str()) {
        return Err(format!(
            "suite.validation_status: unknown status '{status}'"
        ));
    }
    Ok(Suite {
        name: yopt_str(m, "name")?.ok_or("suite.name: field required")?,
        version,
        description: yopt_str(m, "description")?.unwrap_or_default(),
        validation_status: status,
        tasks,
        path: Some(path.to_path_buf()),
    })
}

pub fn resolve_suite(name_or_path: &str, suites_dir: &Path) -> Result<PathBuf, String> {
    let candidate = Path::new(name_or_path);
    if candidate.is_file() {
        return Ok(candidate.to_path_buf());
    }
    for ext in ["yaml", "yml"] {
        let named = suites_dir.join(format!("{name_or_path}.{ext}"));
        if named.is_file() {
            return Ok(named);
        }
    }
    if let Some(name) = candidate.file_name() {
        let in_dir = suites_dir.join(name);
        if in_dir.is_file() {
            return Ok(in_dir);
        }
    }
    Err(format!(
        "no suite '{name_or_path}' (looked for a file and in {}/)",
        suites_dir.display()
    ))
}

pub fn discover_suites(suites_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(suites_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if matches!(
                p.extension().and_then(|s| s.to_str()),
                Some("yaml") | Some("yml")
            ) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}
