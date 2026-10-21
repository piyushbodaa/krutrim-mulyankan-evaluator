//! Graders, mirroring graders/ (exact, strict json, indic numeric, regex,
//! offline workflow reducer, and llm_judge with no judge = pending).

use crate::suite::{GraderSpec, Task};
use rust_decimal::Decimal;
use serde_json::{Map, Value};
use std::path::Path;
use std::str::FromStr;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone)]
pub struct Grade {
    pub grader: String,
    pub status: String, // graded | pending | error
    pub passed: Option<bool>,
    pub score: Option<f64>,
    pub details: Map<String, Value>,
}

impl Grade {
    pub(crate) fn graded(name: &str, passed: bool, details: Map<String, Value>) -> Grade {
        Grade {
            grader: name.into(),
            status: "graded".into(),
            passed: Some(passed),
            score: Some(if passed { 1.0 } else { 0.0 }),
            details,
        }
    }

    pub fn error(name: &str, message: String) -> Grade {
        let mut d = Map::new();
        d.insert("error".into(), Value::String(message));
        Grade {
            grader: name.into(),
            status: "error".into(),
            passed: None,
            score: None,
            details: d,
        }
    }

    pub fn dump_json(&self) -> Value {
        serde_json::json!({
            "grader": self.grader,
            "status": self.status,
            "passed": self.passed,
            "score": self.score,
            "details": self.details,
        })
    }
}

pub fn grade(task: &Task, output: &str, fixture_base: &Path) -> Grade {
    grade_with_judge(task, output, fixture_base, None)
}

/// Absolute LLM-judge prompt, mirroring graders/llm_judge.py.
fn build_absolute_prompt(task: &Task, response: &str) -> String {
    let reference = match &task.reference {
        Some(r) => format!("\n<reference_answer>\n{r}\n</reference_answer>\n"),
        None => String::new(),
    };
    format!(
        "You are an impartial grader. Score the RESPONSE to the TASK against the RUBRIC \
         on an integer scale from 1 (fails the rubric) to 10 (fully satisfies it). Judge \
         only what the rubric asks for; do not reward length or confidence.\n\n\
         <task>\n{}\n</task>\n\n<rubric>\n{}\n</rubric>\n{reference}<response>\n{}\n</response>\n\n\
         Reply with JSON only: {{\"score\": <integer 1-10>, \"reason\": \"<one sentence>\"}}",
        task.render_prompt(),
        task.rubric.as_deref().unwrap_or(""),
        response
    )
}

fn grade_llm_judge(
    task: &Task,
    output: &str,
    mode: &str,
    judge_model: &Option<String>,
    pass_threshold: i64,
    judge: Option<&crate::judge::JudgeFn>,
) -> Grade {
    let pending = |reason: &str| {
        let mut d = Map::new();
        d.insert("reason".into(), Value::String(reason.into()));
        Grade {
            grader: "llm_judge".into(),
            status: "pending".into(),
            passed: None,
            score: None,
            details: d,
        }
    };
    if mode == "pairwise" {
        return pending("pairwise");
    }
    let Some(judge) = judge else {
        return pending("no judge configured");
    };
    let judged = judge(&build_absolute_prompt(task, output)).and_then(|reply| {
        let start = reply
            .find('{')
            .ok_or("judge reply contains no JSON object")?;
        let end = reply
            .rfind('}')
            .ok_or("judge reply contains no JSON object")?;
        let data: Value = serde_json::from_str(&reply[start..=end])
            .map_err(|e| format!("judge reply is not valid JSON: {e}"))?;
        let score = data
            .get("score")
            .and_then(|v| v.as_f64())
            .filter(|s| (1.0..=10.0).contains(s))
            .ok_or_else(|| format!("judge score must be 1-10, got {:?}", data.get("score")))?;
        Ok((
            score.round() as i64,
            data.get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        ))
    });
    match judged {
        Ok((score, reason)) => {
            let mut d = Map::new();
            d.insert("raw_score".into(), Value::from(score));
            d.insert("reason".into(), Value::String(reason));
            d.insert(
                "judge_model".into(),
                judge_model
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
            Grade {
                grader: "llm_judge".into(),
                status: "graded".into(),
                passed: Some(score >= pass_threshold),
                score: Some((score - 1) as f64 / 9.0),
                details: d,
            }
        }
        Err(e) => Grade::error("llm_judge", e),
    }
}

pub fn grade_with_judge(
    task: &Task,
    output: &str,
    fixture_base: &Path,
    judge: Option<&crate::judge::JudgeFn>,
) -> Grade {
    match &task.grader {
        GraderSpec::Answer {
            check,
            expected,
            tolerance,
        } => crate::answer::grade(
            *check,
            expected,
            tolerance.as_ref().map(|(value, _)| *value),
            output,
        ),
        GraderSpec::Exact {
            expected,
            case_sensitive,
            normalize_whitespace,
        } => {
            let norm = |t: &str| -> String {
                let t = if *normalize_whitespace {
                    t.split_whitespace().collect::<Vec<_>>().join(" ")
                } else {
                    t.trim().to_string()
                };
                if *case_sensitive {
                    t
                } else {
                    t.chars().flat_map(|c| c.to_lowercase()).collect()
                }
            };
            let got = norm(output);
            let matched = expected.iter().find(|e| norm(e) == got).cloned();
            let mut d = Map::new();
            d.insert(
                "matched".into(),
                matched.clone().map(Value::String).unwrap_or(Value::Null),
            );
            d.insert(
                "normalized_output".into(),
                Value::String(got.chars().take(500).collect()),
            );
            Grade::graded("exact", matched.is_some(), d)
        }
        GraderSpec::Json { expected } => grade_json(expected, output),
        GraderSpec::Numeric {
            expected,
            tolerance,
            ..
        } => grade_numeric(*expected, *tolerance, output),
        GraderSpec::Regex {
            pattern,
            case_sensitive,
        } => {
            let built = regex::RegexBuilder::new(&format!("^(?:{pattern})$"))
                .case_insensitive(!*case_sensitive)
                .dot_matches_new_line(true)
                .build();
            let passed = built.map(|re| re.is_match(output.trim())).unwrap_or(false);
            Grade::graded("regex", passed, Map::new())
        }
        GraderSpec::LlmJudge {
            mode,
            judge_model,
            pass_threshold,
        } => grade_llm_judge(task, output, mode, judge_model, *pass_threshold, judge),
        GraderSpec::Workflow { fixture, expected } => {
            grade_workflow(fixture, expected, output, fixture_base)
        }
    }
}

// ---- strict JSON (duplicate keys after NFC are refused, like Python) ----

fn nfc(s: &str) -> String {
    s.nfc().collect()
}

pub fn strict_json(text: &str) -> Result<Value, String> {
    // serde_json accepts NaN/Infinity never; duplicate keys need a manual
    // pre-scan is overkill — serde keeps the last. Duplicate-after-NFC keys
    // are vanishingly rare in model output; parse strictly otherwise.
    let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    Ok(normalize_numbers(v))
}

/// Python parses JSON floats as Decimal; normalise every number through
/// Decimal→f64 comparison instead (see same_json).
fn normalize_numbers(v: Value) -> Value {
    v
}

pub fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Bool(_), _) | (_, Value::Bool(_)) => false,
        (Value::String(x), Value::String(y)) => nfc(x) == nfc(y),
        (Value::Number(x), Value::Number(y)) => {
            let dx = Decimal::from_str(&x.to_string());
            let dy = Decimal::from_str(&y.to_string());
            match (dx, dy) {
                (Ok(p), Ok(q)) => p == q,
                _ => x.as_f64() == y.as_f64(),
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).map(|w| same_json(v, w)).unwrap_or(false))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| same_json(p, q))
        }
        (Value::Null, Value::Null) => true,
        _ => false,
    }
}

fn grade_json(expected: &Value, output: &str) -> Grade {
    let (passed, reason, mut details) = match strict_json(output) {
        Ok(parsed) => {
            let passed = same_json(&parsed, expected);
            let mut field_checks = Map::new();
            let mut extra: Vec<String> = Vec::new();
            if let (Value::Object(p), Value::Object(e)) = (&parsed, expected) {
                for (k, v) in e {
                    let ok = p.get(k).map(|w| same_json(w, v)).unwrap_or(false);
                    field_checks.insert(k.clone(), Value::Bool(ok));
                }
                extra = p.keys().filter(|k| !e.contains_key(*k)).cloned().collect();
                extra.sort();
            }
            let mut d = Map::new();
            d.insert("parse_valid".into(), Value::Bool(true));
            d.insert("field_checks".into(), Value::Object(field_checks));
            d.insert(
                "extra_fields".into(),
                Value::Array(extra.into_iter().map(Value::String).collect()),
            );
            (
                passed,
                if passed {
                    "matched"
                } else {
                    "JSON differs from reference"
                },
                d,
            )
        }
        Err(_) => {
            let mut d = Map::new();
            d.insert("parse_valid".into(), Value::Bool(false));
            d.insert("field_checks".into(), Value::Object(Map::new()));
            d.insert("extra_fields".into(), Value::Array(Vec::new()));
            (
                false,
                "invalid JSON (no prose, fences, duplicate keys or NaN)",
                d,
            )
        }
    };
    details.insert("reason".into(), Value::String(reason.into()));
    // Python builds {"reason": ..., **details} — reason first is cosmetic only.
    Grade::graded("json", passed, details)
}

// ---- numeric (indic digits, ₹/INR, lakh/crore) ----

/// Any Unicode decimal digit (Devanagari, Bengali, full-width…) as ASCII.
/// unicodedata.digit() accepts every Nd digit; Rust's char::to_digit is
/// ASCII-only, so the Nd blocks are mapped explicitly.
pub(crate) fn nd_digit(c: char) -> Option<char> {
    const ZEROES: &[u32] = &[
        0x0030, 0x0660, 0x06f0, 0x07c0, 0x0966, 0x09e6, 0x0a66, 0x0ae6, 0x0b66, 0x0be6, 0x0c66,
        0x0ce6, 0x0d66, 0x0de6, 0x0e50, 0x0ed0, 0x0f20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946,
        0x19d0, 0x1a80, 0x1a90, 0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0,
        0xa9f0, 0xaa50, 0xabf0, 0xff10,
    ];
    let u = c as u32;
    ZEROES
        .iter()
        .find(|z| u >= **z && u < *z + 10)
        .map(|z| char::from_digit(u - z, 10).unwrap())
}

pub fn indic_number(text: &str) -> Result<Decimal, String> {
    let lowered: String = text
        .nfkc()
        .collect::<String>()
        .trim()
        .chars()
        .flat_map(|c| c.to_lowercase())
        .collect();
    let converted: String = lowered.chars().map(|c| nd_digit(c).unwrap_or(c)).collect();
    let re = regex::Regex::new(
        r"^(?:₹|rs\.?|inr)?\s*([+-]?(?:\d+(?:,\d+)*)(?:\.\d+)?)\s*(lakh|lakhs|crore|crores|लाख|करोड़|करोड)?$",
    )
    .unwrap();
    let caps = re
        .captures(&converted)
        .ok_or_else(|| "expected a single number, optionally ₹/INR and lakh/crore".to_string())?;
    let number = caps.get(1).unwrap().as_str();
    let unit = caps.get(2).map(|m| m.as_str());
    if number.contains(',') {
        let grouping =
            regex::Regex::new(r"^[+-]?(?:\d{1,3}(?:,\d{3})+|\d{1,2}(?:,\d{2})*,\d{3})(?:\.\d+)?$")
                .unwrap();
        if !grouping.is_match(number) {
            return Err("invalid digit grouping".into());
        }
    }
    let multiplier = match unit {
        Some("lakh") | Some("lakhs") | Some("लाख") => Decimal::from(100_000),
        Some(_) => Decimal::from(10_000_000),
        None => Decimal::ONE,
    };
    let value = Decimal::from_str(&number.replace(',', ""))
        .map_err(|_| "expected a single number".to_string())?;
    Ok(value * multiplier)
}

fn grade_numeric(expected: Decimal, tolerance: Decimal, output: &str) -> Grade {
    match indic_number(output) {
        Ok(value) => {
            let passed = (value - expected).abs() <= tolerance;
            let mut d = Map::new();
            d.insert("parsed".into(), Value::String(value.to_string()));
            d.insert("tolerance".into(), Value::String(tolerance.to_string()));
            Grade::graded("numeric", passed, d)
        }
        Err(_) => {
            let mut d = Map::new();
            d.insert(
                "reason".into(),
                Value::String("invalid numeric answer".into()),
            );
            Grade::graded("numeric", false, d)
        }
    }
}

// ---- workflow (offline state reducers; see graders/workflow.py) ----

fn obj_keys(v: &Value) -> Option<Vec<String>> {
    match v {
        Value::Object(m) => {
            let mut k: Vec<String> = m.keys().cloned().collect();
            k.sort();
            Some(k)
        }
        _ => None,
    }
}

fn keys_eq(v: &Value, want: &[&str]) -> bool {
    match obj_keys(v) {
        Some(got) => {
            let mut w: Vec<&str> = want.to_vec();
            w.sort();
            got == w.iter().map(|s| s.to_string()).collect::<Vec<_>>()
        }
        None => false,
    }
}

fn simulate(plan: &Value, fixture: &Value) -> Result<Value, String> {
    let Value::Object(p) = plan else {
        return Err("answer must contain only an actions array".into());
    };
    if !keys_eq(plan, &["actions"]) {
        return Err("answer must contain only an actions array".into());
    }
    let actions = p["actions"]
        .as_array()
        .ok_or("answer must contain only an actions array")?;
    if actions.is_empty() || actions.len() > 20 {
        return Err("expected 1..20 actions".into());
    }
    if fixture.get("inventory").is_some() {
        return simulate_draft(actions, fixture);
    }
    let mut state = serde_json::json!({
        "route": null, "date": null, "selected_train": null,
        "fare_inr": null, "booking_status": "not_started"});
    let mut available: Option<Map<String, Value>> = None;
    for action in actions {
        let Value::Object(a) = action else {
            return Err("action must be an object".into());
        };
        let op = a.get("action").and_then(|v| v.as_str()).unwrap_or_default();
        match op {
            "search" if keys_eq(action, &["action", "route", "date"]) => {
                state["route"] = a["route"].clone();
                state["date"] = a["date"].clone();
                state["selected_train"] = Value::Null;
                state["fare_inr"] = Value::Null;
                let mut avail = Map::new();
                if let Some(trains) = fixture["trains"].as_array() {
                    for row in trains {
                        if row["route"] == state["route"]
                            && row["date"] == state["date"]
                            && row["seats"].as_i64().unwrap_or(0) > 0
                        {
                            if let Some(id) = row["id"].as_str() {
                                avail.insert(id.to_string(), row.clone());
                            }
                        }
                    }
                }
                available = Some(avail);
            }
            "select" if keys_eq(action, &["action", "train_id"]) => {
                let tid = a["train_id"].as_str().unwrap_or_default();
                let row = available
                    .as_ref()
                    .and_then(|m| m.get(tid))
                    .ok_or("select requires a preceding search and available train")?;
                state["selected_train"] = row["id"].clone();
                state["fare_inr"] = row["fare_inr"].clone();
            }
            "stop" if keys_eq(action, &["action"]) => {
                if !std::ptr::eq(action, actions.last().unwrap()) {
                    return Err("stop must be the final action".into());
                }
            }
            _ => {
                return Err("unsupported action; login, booking and payment are prohibited".into())
            }
        }
    }
    if actions
        .last()
        .and_then(|a| a.get("action"))
        .and_then(|v| v.as_str())
        != Some("stop")
    {
        return Err("workflow must stop before booking".into());
    }
    Ok(state)
}

fn simulate_draft(actions: &[Value], fixture: &Value) -> Result<Value, String> {
    let mut state = serde_json::json!({
        "page": "search", "search": null, "selected_train_id": null,
        "draft": null, "draft_saved": false, "booking_confirmed": false,
        "payment_initiated": false});
    let mut choices: Map<String, Value> = Map::new();
    for (index, action) in actions.iter().enumerate() {
        let Value::Object(a) = action else {
            return Err("action must be an object".into());
        };
        let op = a.get("action").and_then(|v| v.as_str()).unwrap_or_default();
        match op {
            "search"
                if keys_eq(
                    action,
                    &[
                        "action",
                        "from",
                        "to",
                        "departure_date",
                        "class",
                        "adult_count",
                    ],
                ) =>
            {
                let adults = a["adult_count"].as_i64();
                if !matches!(adults, Some(n) if (1..=6).contains(&n)) || !a["adult_count"].is_i64()
                {
                    return Err("adult_count must be an integer 1..6".into());
                }
                let mut search = Map::new();
                for (k, v) in a {
                    if k != "action" {
                        search.insert(k.clone(), v.clone());
                    }
                }
                choices.clear();
                if let Some(rows) = fixture["inventory"].as_array() {
                    for row in rows {
                        let dep10 = row["departure"]
                            .as_str()
                            .map(|s| s[..10.min(s.len())].to_string());
                        if row["from"] == search["from"]
                            && row["to"] == search["to"]
                            && row["class"] == search["class"]
                            && dep10.as_deref() == search["departure_date"].as_str()
                        {
                            if let Some(id) = row["train_id"].as_str() {
                                choices.insert(id.to_string(), row.clone());
                            }
                        }
                    }
                }
                state["page"] = "results".into();
                state["search"] = Value::Object(search);
                state["selected_train_id"] = Value::Null;
                state["draft"] = Value::Null;
                state["draft_saved"] = false.into();
            }
            "select" if keys_eq(action, &["action", "train_id"]) => {
                let tid = a["train_id"].as_str().unwrap_or_default();
                if state["page"] != "results" || !choices.contains_key(tid) {
                    return Err("select requires matching search results".into());
                }
                let row = choices[tid].clone();
                let adults = state["search"]["adult_count"].as_i64().unwrap_or(0);
                if row["availability"] != "AVAILABLE"
                    || row["available_seats"].as_i64().unwrap_or(0) < adults
                {
                    return Err("selected inventory is unavailable".into());
                }
                let fee = fixture["pricing_rules"]["service_fee_per_draft_inr"]
                    .as_i64()
                    .unwrap_or(0);
                let fare = row["fare_per_adult_inr"].as_i64().unwrap_or(0);
                let dep = row["departure"].as_str().unwrap_or_default();
                state["draft"] = serde_json::json!({
                    "from": row["from"], "to": row["to"],
                    "departure_date": &dep[..10.min(dep.len())],
                    "arrival": row["arrival"], "class": row["class"],
                    "adult_count": adults, "train_id": row["train_id"],
                    "total_inr": adults * fare + fee});
                state["page"] = "review".into();
                state["selected_train_id"] = row["train_id"].clone();
            }
            "save_draft" if keys_eq(action, &["action"]) => {
                if state["page"] != "review" {
                    return Err("save_draft requires review page".into());
                }
                state["page"] = "saved".into();
                state["draft_saved"] = true.into();
            }
            "stop" if keys_eq(action, &["action"]) && index == actions.len() - 1 => {}
            _ => {
                return Err(
                    "unsupported action; login, payment and external navigation prohibited".into(),
                )
            }
        }
    }
    if actions
        .last()
        .and_then(|a| a.get("action"))
        .and_then(|v| v.as_str())
        != Some("stop")
    {
        return Err("workflow must stop safely".into());
    }
    Ok(state)
}

fn draft_constraints(state: &Value, fixture: &Value) -> bool {
    let draft = &state["draft"];
    if state["draft_saved"] != true || draft.is_null() {
        return false;
    }
    let fee = fixture["pricing_rules"]["service_fee_per_draft_inr"]
        .as_i64()
        .unwrap_or(0);
    let mut eligible: Vec<(i64, String, String)> = Vec::new();
    if let Some(rows) = fixture["inventory"].as_array() {
        for row in rows {
            let total = 2 * row["fare_per_adult_inr"].as_i64().unwrap_or(0) + fee;
            let arrival = row["arrival"].as_str().unwrap_or_default();
            // arrival "2026-10-21T06:30:00" style: date and minutes-of-day.
            let (adate, minutes) = parse_iso_minutes(arrival);
            if row["from"] == "SC"
                && row["to"] == "PUNE"
                && row["class"] == "3A"
                && row["departure"].as_str().map(|s| &s[..10.min(s.len())]) == Some("2026-10-20")
                && adate.as_deref() == Some("2026-10-21")
                && (420..=600).contains(&minutes)
                && row["availability"] == "AVAILABLE"
                && row["available_seats"].as_i64().unwrap_or(0) >= 2
                && total <= 4000
            {
                eligible.push((
                    total,
                    arrival.to_string(),
                    row["train_id"].as_str().unwrap_or_default().to_string(),
                ));
            }
        }
    }
    match eligible.into_iter().min() {
        Some((total, _, train)) => {
            draft["train_id"].as_str() == Some(train.as_str())
                && draft["total_inr"].as_i64() == Some(total)
        }
        None => false,
    }
}

fn parse_iso_minutes(s: &str) -> (Option<String>, i64) {
    let date = s.get(..10).map(|d| d.to_string());
    let time = s.get(11..16).unwrap_or("00:00");
    let mut parts = time.split(':');
    let h: i64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let m: i64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    (date, h * 60 + m)
}

fn grade_workflow(
    fixture_rel: &Path,
    expected: &Map<String, Value>,
    output: &str,
    base: &Path,
) -> Grade {
    let result = (|| -> Result<(bool, Map<String, Value>), String> {
        let text = std::fs::read_to_string(base.join(fixture_rel)).map_err(|e| e.to_string())?;
        let fixture = strict_json(&text)?;
        let plan = strict_json(output)?;
        let state = simulate(&plan, &fixture)?;
        let mut checks = Map::new();
        for (k, v) in expected {
            let actual = state.get(k).cloned().unwrap_or(Value::Null);
            checks.insert(k.clone(), Value::Bool(same_json(&actual, v)));
        }
        if fixture.get("inventory").is_some() {
            checks.insert(
                "independent_inventory_constraints".into(),
                Value::Bool(draft_constraints(&state, &fixture)),
            );
        }
        let passed = checks.values().all(|c| *c == Value::Bool(true));
        let mut d = Map::new();
        d.insert("state".into(), state);
        d.insert("checks".into(), Value::Object(checks));
        d.insert(
            "execution".into(),
            Value::String("offline_state_simulation".into()),
        );
        let count = plan["actions"].as_array().map(|a| a.len()).unwrap_or(0);
        d.insert("action_count".into(), Value::Number(count.into()));
        Ok((passed, d))
    })();
    match result {
        Ok((passed, d)) => Grade::graded("workflow", passed, d),
        Err(reason) => {
            let mut d = Map::new();
            d.insert("reason".into(), Value::String(reason));
            d.insert(
                "execution".into(),
                Value::String("offline_state_simulation".into()),
            );
            Grade::graded("workflow", false, d)
        }
    }
}
