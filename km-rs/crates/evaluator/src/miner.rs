//! Mine failures from prompt/answer pairs with an automatic LLM judge.
//! Mirrors `miner.py`: only confident `wrong` verdicts with an expected
//! answer are kept; unclear and judge errors are dropped, never guessed.

use crate::history::PromptPair;
use crate::judge::{JudgeFn, DEFAULT_JUDGE_MAX_TOKENS};
use crate::pricing::{max_input_tokens, PriceTable};
use crate::pypy::sha256_hex;
use serde_json::Value;

pub const MINE_TEMPLATE: &str = "You are checking whether an AI assistant's ANSWER to a user's PROMPT was wrong.\nJudge factual correctness, arithmetic, logic, code correctness and whether the\nprompt's explicit constraints were followed. Do not mark an answer wrong for\nstyle, length or tone. If the prompt has no checkable right answer (chitchat,\ncreative writing with no constraints, missing context), say \"unclear\".\n\n<prompt>\n{prompt}\n</prompt>\n\n<answer>\n{answer}\n</answer>\n\nReply with JSON only:\n{\"verdict\": \"wrong\" | \"correct\" | \"unclear\",\n  \"confidence\": <number 0 to 1>,\n  \"reason\": \"<one sentence>\",\n  \"expected\": \"<the correct answer, or what a correct answer must contain; empty unless wrong>\",\n  \"rubric\": \"<how to grade a new answer, one or two sentences; empty unless wrong>\"}";

/// Pairs longer than this (chars, prompt + answer) are skipped, never judged.
pub const MAX_PAIR_CHARS: usize = 32_000;

fn secret_patterns() -> Vec<regex::Regex> {
    [
        r"sk-ant-[A-Za-z0-9_\-]{8,}",
        r"sk-[A-Za-z0-9_\-]{16,}",
        r"gh[pousr]_[A-Za-z0-9]{20,}",
        r"AKIA[0-9A-Z]{16}",
        r"xox[baprs]-[A-Za-z0-9\-]{10,}",
        r"(?i)(bearer\s+)[A-Za-z0-9._\-]{16,}",
        r#"(?i)((?:api[_-]?key|auth[_-]?token|secret|password)\s*[:=]\s*)[^\s"']{8,}"#,
    ]
    .iter()
    .map(|p| regex::Regex::new(p).expect("valid secret regex"))
    .collect()
}

/// Redact env secrets plus common pasted key shapes before anything is sent
/// to a (possibly different) judge provider. Mirrors `redact_secrets` in miner.py.
pub fn redact_secrets(text: &str) -> String {
    let mut out = crate::provenance::redact_str(text, &[]);
    for pattern in secret_patterns() {
        out = pattern
            .replace_all(&out, |caps: &regex::Captures| match caps.get(1) {
                Some(head) => format!("{}[REDACTED]", head.as_str()),
                None => "[REDACTED]".to_string(),
            })
            .to_string();
    }
    out
}

#[derive(Debug, Default)]
pub struct Selection {
    pub pairs: Vec<PromptPair>,
    pub duplicates: usize,
    pub oversized: usize,
}

/// Dedupe, drop oversized, then sample evenly (never just the oldest).
pub fn select_pairs(pairs: &[PromptPair], limit: Option<usize>) -> Selection {
    let mut seen = std::collections::HashSet::new();
    let mut picked: Vec<PromptPair> = Vec::new();
    let (mut duplicates, mut oversized) = (0, 0);
    for pair in pairs {
        let key = sha256_hex(format!("{}\x00{}", pair.prompt, pair.answer).as_bytes());
        if !seen.insert(key) {
            duplicates += 1;
            continue;
        }
        if pair.prompt.chars().count() + pair.answer.chars().count() > MAX_PAIR_CHARS {
            oversized += 1;
            continue;
        }
        picked.push(pair.clone());
    }
    if let Some(limit) = limit {
        if limit < picked.len() {
            picked = if limit == 0 {
                Vec::new()
            } else {
                (0..limit)
                    .map(|i| picked[i * picked.len() / limit].clone())
                    .collect()
            };
        }
    }
    Selection {
        pairs: picked,
        duplicates,
        oversized,
    }
}

pub fn build_mine_prompt(pair: &PromptPair) -> String {
    // Split the template (never the substituted text) so a prompt containing
    // a literal "{answer}" cannot be rewritten by the second placeholder.
    let (head, rest) = MINE_TEMPLATE
        .split_once("{prompt}")
        .expect("template has {prompt}");
    let (mid, tail) = rest.split_once("{answer}").expect("template has {answer}");
    format!(
        "{head}{}{mid}{}{tail}",
        redact_secrets(&pair.prompt),
        redact_secrets(&pair.answer)
    )
}

pub fn judge_prompt_hash() -> String {
    sha256_hex(MINE_TEMPLATE.as_bytes())[..16].to_string()
}

#[derive(Debug, Clone)]
pub struct MineVerdict {
    pub verdict: String, // wrong | correct | unclear
    pub confidence: f64,
    pub reason: String,
    pub expected: String,
    pub rubric: String,
}

pub fn parse_verdict(reply: &str) -> Result<MineVerdict, String> {
    let start = reply
        .find('{')
        .ok_or("judge reply contains no JSON object")?;
    let end = reply
        .rfind('}')
        .ok_or("judge reply contains no JSON object")?;
    let data: Value = serde_json::from_str(&reply[start..=end])
        .map_err(|e| format!("judge reply is not valid JSON: {e}"))?;
    let verdict = data
        .get("verdict")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if !["wrong", "correct", "unclear"].contains(&verdict.as_str()) {
        return Err(format!(
            "verdict must be wrong/correct/unclear, got '{verdict}'"
        ));
    }
    let confidence = data
        .get("confidence")
        .and_then(|v| v.as_f64())
        .filter(|c| (0.0..=1.0).contains(c))
        .ok_or_else(|| {
            format!(
                "confidence must be a number between 0 and 1, got {:?}",
                data.get("confidence")
            )
        })?;
    let field = |name: &str| {
        data.get(name)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    Ok(MineVerdict {
        verdict,
        confidence,
        reason: field("reason"),
        expected: field("expected"),
        rubric: field("rubric"),
    })
}

#[derive(Debug, Clone)]
pub struct MinedFailure {
    pub pair: PromptPair,
    pub verdict: MineVerdict,
    pub judge_spec: String,
}

impl MinedFailure {
    pub fn answer_sha256(&self) -> String {
        sha256_hex(self.pair.answer.as_bytes())
    }
}

/// JSONL cache of raw judge replies, keyed by judge + pair content.
#[derive(Debug)]
pub struct VerdictCache {
    path: std::path::PathBuf,
    entries: std::collections::HashMap<String, String>,
}

impl VerdictCache {
    pub fn load(path: std::path::PathBuf) -> VerdictCache {
        let mut entries = std::collections::HashMap::new();
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                if let Ok(row) = serde_json::from_str::<Value>(line) {
                    if let (Some(k), Some(r)) = (
                        row.get("key").and_then(|v| v.as_str()),
                        row.get("reply").and_then(|v| v.as_str()),
                    ) {
                        entries.insert(k.to_string(), r.to_string());
                    }
                }
            }
        }
        VerdictCache { path, entries }
    }

    pub fn key(pair: &PromptPair, judge_spec: &str) -> String {
        sha256_hex(
            format!(
                "{judge_spec}\x00{}\x00{}\x00{}",
                judge_prompt_hash(),
                pair.prompt,
                pair.answer
            )
            .as_bytes(),
        )
    }

    pub fn get(&self, pair: &PromptPair, judge_spec: &str) -> Option<String> {
        self.entries.get(&Self::key(pair, judge_spec)).cloned()
    }

    pub fn put(&mut self, pair: &PromptPair, judge_spec: &str, reply: &str) {
        let key = Self::key(pair, judge_spec);
        if self.entries.contains_key(&key) {
            return;
        }
        self.entries.insert(key.clone(), reply.to_string());
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            use std::io::Write;
            let _ = writeln!(file, "{}", serde_json::json!({"key": key, "reply": reply}));
        }
    }
}

#[derive(Debug, Default)]
pub struct MiningResult {
    pub failures: Vec<MinedFailure>,
    pub checked: usize,
    pub correct: usize,
    pub unclear: usize,
    pub below_threshold: usize,
    pub errors: usize,
    pub duplicates: usize,
    pub oversized: usize,
    pub cache_hits: usize,
    pub redacted_pairs: usize,
}

pub type ProgressFn<'a> = &'a dyn Fn(usize, usize, &PromptPair);

#[allow(clippy::too_many_arguments)]
pub fn mine_failures(
    pairs: &[PromptPair],
    judge: &JudgeFn,
    judge_spec: &str,
    min_confidence: f64,
    limit: Option<usize>,
    on_progress: Option<ProgressFn<'_>>,
    mut cache: Option<&mut VerdictCache>,
) -> Result<MiningResult, String> {
    if !(0.0..=1.0).contains(&min_confidence) {
        return Err("min_confidence must be between 0 and 1".into());
    }
    let selection = select_pairs(pairs, limit);
    let selected = &selection.pairs;
    let mut result = MiningResult {
        duplicates: selection.duplicates,
        oversized: selection.oversized,
        ..MiningResult::default()
    };
    for (index, pair) in selected.iter().enumerate() {
        if let Some(progress) = on_progress {
            progress(index + 1, selected.len(), pair);
        }
        result.checked += 1;
        let mine_prompt = build_mine_prompt(pair);
        if mine_prompt.contains("[REDACTED]") {
            result.redacted_pairs += 1;
        }
        let reply = match cache.as_deref().and_then(|c| c.get(pair, judge_spec)) {
            Some(cached) => {
                result.cache_hits += 1;
                Ok(cached)
            }
            None => match judge(&mine_prompt) {
                Ok(text) => {
                    if let Some(c) = cache.as_deref_mut() {
                        c.put(pair, judge_spec, &text);
                    }
                    Ok(text)
                }
                Err(e) => Err(e),
            },
        };
        let verdict = match reply.and_then(|r| parse_verdict(&r)) {
            Ok(v) => v,
            Err(_) => {
                result.errors += 1;
                continue;
            }
        };
        match verdict.verdict.as_str() {
            "correct" => result.correct += 1,
            "unclear" => result.unclear += 1,
            _ if verdict.confidence < min_confidence => result.below_threshold += 1,
            _ if verdict.expected.trim().is_empty() => result.unclear += 1,
            _ => result.failures.push(MinedFailure {
                pair: pair.clone(),
                verdict,
                judge_spec: judge_spec.to_string(),
            }),
        }
    }
    Ok(result)
}

/// Worst-case mining cost, or None when the judge model has no price.
pub fn estimate_judge_cost_usd(
    pairs: &[PromptPair],
    judge_spec: &str,
    prices: &PriceTable,
    limit: Option<usize>,
    cache: Option<&VerdictCache>,
) -> Result<Option<f64>, String> {
    let (provider, model) = crate::judge::parse_judge_spec(judge_spec)?;
    let Some(price) = prices.lookup(provider, model) else {
        return Ok(None);
    };
    Ok(Some(
        select_pairs(pairs, limit)
            .pairs
            .iter()
            .filter(|pair| {
                cache
                    .map(|c| c.get(pair, judge_spec).is_none())
                    .unwrap_or(true)
            })
            .map(|pair| {
                price.cost(
                    max_input_tokens(&build_mine_prompt(pair)),
                    DEFAULT_JUDGE_MAX_TOKENS,
                )
            })
            .sum(),
    ))
}
