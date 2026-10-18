//! Turn mined failures into a frozen suite plus an evidence sidecar.
//! Mirrors `microeval.py`: suite = your prompts verbatim graded by an LLM
//! judge; sidecar `<suite>.evidence.json` holds the private original Q&A.

use crate::miner::{judge_prompt_hash, MinedFailure};
use crate::pypy::sha256_hex;
use crate::suite::{GraderSpec, Suite, Task};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

fn literal_inputs(prompt: &str) -> BTreeMap<String, String> {
    let re = regex::Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*)\s*\}\}").expect("valid regex");
    re.captures_iter(prompt)
        .map(|c| (c[1].to_string(), format!("{{{{{}}}}}", &c[1])))
        .collect()
}

pub fn build_suite(
    failures: &[MinedFailure],
    name: &str,
    version: &str,
    judge_spec: &str,
) -> Result<Suite, String> {
    if failures.is_empty() {
        return Err("no mined failures to build a micro-eval from".into());
    }
    let name_ok = !name.is_empty()
        && name
            .chars()
            .next()
            .map(|c| c.is_ascii_alphanumeric())
            .unwrap_or(false)
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if !name_ok {
        return Err(format!(
            "suite name '{name}' must use letters, digits, '.', '-' or '_'"
        ));
    }
    let mut tasks = Vec::new();
    for (index, failure) in failures.iter().enumerate() {
        let pair = &failure.pair;
        let short = &sha256_hex(format!("{}\x00{}", pair.prompt, pair.answer).as_bytes())[..8];
        let rubric = if failure.verdict.rubric.trim().is_empty() {
            "The answer is correct only if it matches the reference answer's facts \
             and follows every constraint in the prompt."
                .to_string()
        } else {
            failure.verdict.rubric.trim().to_string()
        };
        tasks.push(Task {
            id: format!("mine.{}.{:04}-{short}", pair.source, index + 1),
            version: 1,
            category: "personal".into(),
            capability_track_version: "1.0".into(),
            capability_track: "text".into(),
            language: "und".into(),
            script: None,
            input_style: None,
            domain: Some(pair.source.clone()),
            as_of: None,
            provenance: vec![json!({
                "source": pair.source,
                "conversation_id": pair.conversation_id,
                "conversation_title": pair.conversation_title,
                "turn_index": pair.turn_index,
                "original_model": pair.model,
                "judge": failure.judge_spec,
                "judge_prompt_hash": judge_prompt_hash(),
                "judge_confidence": failure.verdict.confidence,
                "judge_reason": failure.verdict.reason,
                "original_answer_sha256": failure.answer_sha256(),
            })],
            risk_flags: Vec::new(),
            context: Value::Object(Map::new()),
            prompt: pair.prompt.clone(),
            inputs: literal_inputs(&pair.prompt),
            fixtures: Vec::new(),
            grader: GraderSpec::LlmJudge {
                mode: "absolute".into(),
                judge_model: Some(judge_spec.to_string()),
                pass_threshold: 7,
            },
            rubric: Some(rubric),
            reference: Some(failure.verdict.expected.clone()),
            max_output_tokens: 4096,
            timeout_s: 120.0,
            budget_usd: None,
        });
    }
    Ok(Suite {
        name: name.to_string(),
        version: version.to_string(),
        description: "Personal micro-eval mined automatically from the owner's own chat \
                      history: prompts their previous model answered wrongly, graded by \
                      an LLM judge against mined expected answers. Judge-graded, not \
                      ground truth."
            .to_string(),
        validation_status: "unvalidated_pilot".into(),
        tasks,
        path: None,
    })
}

pub fn evidence_payload(suite: &Suite, failures: &[MinedFailure], judge_spec: &str) -> Value {
    json!({
        "schema_version": "1.0",
        "suite": suite.name,
        "version": suite.version,
        "judge": judge_spec,
        "judge_prompt_hash": judge_prompt_hash(),
        "private": "Contains the owner's original prompts and answers. \
                    Keep private; never commit or publish.",
        "items": suite.tasks.iter().zip(failures).map(|(task, failure)| json!({
            "task_id": task.id,
            "source": failure.pair.source,
            "conversation_id": failure.pair.conversation_id,
            "conversation_title": failure.pair.conversation_title,
            "turn_index": failure.pair.turn_index,
            "created_at": failure.pair.created_at,
            "original_model": failure.pair.model,
            "prompt": failure.pair.prompt,
            "original_answer": failure.pair.answer,
            "verdict": {
                "verdict": failure.verdict.verdict,
                "confidence": failure.verdict.confidence,
                "reason": failure.verdict.reason,
                "expected": failure.verdict.expected,
                "rubric": failure.verdict.rubric,
            },
        })).collect::<Vec<_>>(),
    })
}

/// Write suite YAML and evidence sidecar; never overwrites either.
pub fn write_micro_eval(
    failures: &[MinedFailure],
    destination: &Path,
    name: &str,
    version: &str,
    judge_spec: &str,
) -> Result<(Suite, PathBuf), String> {
    let suite = build_suite(failures, name, version, judge_spec)?;
    let evidence_path = destination
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(format!(
            "{}.evidence.json",
            destination
                .file_stem()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default()
        ));
    if evidence_path.exists() {
        return Err(format!("refusing to overwrite {}", evidence_path.display()));
    }
    let yaml = serde_yaml::to_string(&suite.dump_json()).map_err(|e| e.to_string())?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|e| format!("{}: {e}", destination.display()))?;
    file.write_all(yaml.as_bytes()).map_err(|e| e.to_string())?;
    let evidence = serde_json::to_string_pretty(&evidence_payload(&suite, failures, judge_spec))
        .map_err(|e| e.to_string())?;
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&evidence_path)
    {
        Ok(mut f) => {
            if let Err(e) = f.write_all(evidence.as_bytes()) {
                let _ = std::fs::remove_file(destination);
                return Err(e.to_string());
            }
        }
        Err(e) => {
            let _ = std::fs::remove_file(destination); // never leave a suite without evidence
            return Err(format!("{}: {e}", evidence_path.display()));
        }
    }
    Ok((suite, evidence_path))
}
