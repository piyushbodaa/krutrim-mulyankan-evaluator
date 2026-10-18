//! Build a judge callable from a `<provider>/<model>` spec. Mirrors `judge.py`;
//! used to mine failures and to grade `llm_judge` tasks during a run.

use crate::providers::{credentials_problem, make_adapter, provider_config};
use std::collections::BTreeMap;

pub type JudgeFn = dyn Fn(&str) -> Result<String, String>;

pub const DEFAULT_JUDGE_MAX_TOKENS: i64 = 1024;
pub const DEFAULT_JUDGE_TIMEOUT_S: f64 = 120.0;

pub fn parse_judge_spec(spec: &str) -> Result<(&str, &str), String> {
    let (provider, model) = spec
        .split_once('/')
        .filter(|(p, m)| !p.is_empty() && !m.is_empty())
        .ok_or_else(|| format!("judge spec must be <provider>/<model>, got '{spec}'"))?;
    if provider_config(provider).is_none() {
        return Err(format!("unknown provider '{provider}'"));
    }
    Ok((provider, model))
}

pub fn judge_problems(spec: &str, env: &BTreeMap<String, String>) -> Vec<String> {
    match parse_judge_spec(spec) {
        Ok((provider, _)) => credentials_problem(provider, env).into_iter().collect(),
        Err(e) => vec![e],
    }
}

pub fn make_judge(spec: &str, env: &BTreeMap<String, String>) -> Result<Box<JudgeFn>, String> {
    let (provider, model) = parse_judge_spec(spec)?;
    let provider = provider.to_string();
    let model = model.to_string();
    let adapter = make_adapter(&provider, None, env);
    let spec = spec.to_string();
    Ok(Box::new(move |prompt: &str| {
        let result = adapter
            .complete(
                &model,
                prompt,
                DEFAULT_JUDGE_MAX_TOKENS,
                DEFAULT_JUDGE_TIMEOUT_S,
                0,
                None,
                None,
            )
            .map_err(|e| e.to_string())?;
        if result.text.is_empty() {
            return Err(format!("judge '{spec}' returned no text"));
        }
        Ok(result.text)
    }))
}
