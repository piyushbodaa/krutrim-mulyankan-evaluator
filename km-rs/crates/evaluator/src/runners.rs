//! Runners, mirroring runners/. Fake echo/oracle, the direct-API
//! ModelRunner for all eight providers, the harness/agent stubs, and the
//! Claude Code draft + recorded app observations used by the workbench.

use crate::pricing::{max_input_tokens, Price, PriceTable};
use crate::providers::{
    call_with_retries, credentials_problem, make_adapter, provider_config, Adapter, ApiError,
};
use crate::suite::{GraderSpec, Task};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct RunOutput {
    pub output: String,
    pub usage: Usage,
    pub steps: Option<i64>,
    pub tool_calls: Option<i64>,
    pub error: Option<String>,
    pub raw: Map<String, Value>,
}

impl RunOutput {
    pub fn error(message: String) -> RunOutput {
        RunOutput {
            error: Some(message),
            ..Default::default()
        }
    }

    pub fn dump_json(&self) -> Value {
        json!({
            "output": self.output,
            "usage": {
                "input_tokens": self.usage.input_tokens,
                "output_tokens": self.usage.output_tokens,
                "cost_usd": self.usage.cost_usd,
            },
            "steps": self.steps,
            "tool_calls": self.tool_calls,
            "error": self.error,
            "raw": self.raw,
        })
    }
}

/// One attempt's context, mirroring base.RunContext.
#[derive(Debug, Clone)]
pub struct RunContext {
    pub workdir: PathBuf,
    pub seed: i64,
    pub timeout_s: f64,
    pub cancel: Option<Arc<AtomicBool>>,
}

impl Default for RunContext {
    fn default() -> Self {
        RunContext {
            workdir: PathBuf::from("."),
            seed: 0,
            timeout_s: 120.0,
            cancel: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerKind {
    Fake,
    Model,
    Harness,
    Agent,
    App,
}

impl RunnerKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunnerKind::Fake => "fake",
            RunnerKind::Model => "model",
            RunnerKind::Harness => "harness",
            RunnerKind::Agent => "agent",
            RunnerKind::App => "app",
        }
    }
    fn track(&self) -> &'static str {
        match self {
            RunnerKind::Fake => "mock_validation",
            RunnerKind::Model => "model_api",
            RunnerKind::Harness => "harness",
            RunnerKind::Agent => "personal_agent",
            RunnerKind::App => "consumer_app",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FakeKind {
    Echo,
    Oracle,
}

#[derive(Debug, Clone)]
pub struct ModelOptions {
    pub max_tokens: Option<i64>,
    pub effort: Option<String>,
    pub temperature: Option<f64>,
    pub retries: i64,
}

impl ModelOptions {
    fn suffix(&self) -> String {
        let mut set: BTreeMap<&str, String> = BTreeMap::new();
        if let Some(v) = self.max_tokens {
            set.insert("max_tokens", v.to_string());
        }
        if let Some(v) = &self.effort {
            set.insert("effort", v.clone());
        }
        if let Some(v) = self.temperature {
            set.insert("temperature", crate::pypy::py_float(v));
        }
        if self.retries != 0 {
            set.insert("retries", self.retries.to_string());
        }
        set.iter().map(|(k, v)| format!(",{k}={v}")).collect()
    }

    fn dump_json(&self) -> Value {
        json!({"max_tokens": self.max_tokens, "effort": self.effort,
               "temperature": self.temperature, "retries": self.retries})
    }
}

pub struct ModelState {
    pub provider: String,
    pub model: String,
    pub options: ModelOptions,
    pub price: Option<Price>,
    pub env: BTreeMap<String, String>,
    pub sleep: Box<dyn Fn(f64) + Send + Sync>,
    adapter: Mutex<Arc<Adapter>>,
}

impl ModelState {
    fn adapter(&self) -> Result<Arc<Adapter>, String> {
        let mut cached = self
            .adapter
            .lock()
            .map_err(|_| "Model adapter is unavailable".to_string())?;
        // Desktop runs normally get a fresh environment snapshot at runner
        // construction. Also honor explicit changes to this public state,
        // without retaining old credentials or connections after a change.
        if !cached.matches_configuration(&self.provider, &self.env) {
            *cached = Arc::new(make_adapter(&self.provider, None, &self.env));
        }
        // Share the adapter and connection pool. Never hold this lock while
        // doing network I/O; pricing/options are applied separately per task.
        Ok(Arc::clone(&cached))
    }
}

pub enum RunnerInner {
    Fake(FakeKind),
    Model(Box<ModelState>),
    Stub { message: String },
    ClaudeDraft(crate::claude_draft::ClaudeDraft),
    AppObservation(crate::observations::AppObservationRunner),
}

pub struct Runner {
    pub name: String,
    pub kind: RunnerKind,
    pub inner: RunnerInner,
}

pub fn approx_tokens(text: &str) -> i64 {
    if text.is_empty() {
        0
    } else {
        ((text.chars().count() / 4) as i64).max(1)
    }
}

impl Runner {
    fn fake(kind: FakeKind) -> Runner {
        Runner {
            name: match kind {
                FakeKind::Echo => "echo".into(),
                FakeKind::Oracle => "oracle".into(),
            },
            kind: RunnerKind::Fake,
            inner: RunnerInner::Fake(kind),
        }
    }

    pub fn model_state(&self) -> Option<&ModelState> {
        match &self.inner {
            RunnerInner::Model(m) => Some(m),
            _ => None,
        }
    }

    pub fn problems(&self) -> Vec<String> {
        match &self.inner {
            RunnerInner::Stub { message } => vec![message.clone()],
            RunnerInner::Model(m) => model_problems(m),
            RunnerInner::ClaudeDraft(d) => d.problems(),
            _ => Vec::new(),
        }
    }

    pub fn estimate_cost_usd(&self, task: &Task, prompt: &str) -> Option<f64> {
        match &self.inner {
            RunnerInner::Fake(_) | RunnerInner::AppObservation(_) | RunnerInner::ClaudeDraft(_) => {
                Some(0.0)
            }
            RunnerInner::Stub { .. } => None,
            RunnerInner::Model(m) => m.price.as_ref().map(|p| {
                p.cost(max_input_tokens(prompt), model_max_tokens(m, task))
                    * (1 + m.options.retries) as f64
            }),
        }
    }

    pub fn run(&self, task: &Task, prompt: &str, ctx: &RunContext) -> RunOutput {
        match self.run_inner(task, prompt, ctx) {
            Ok(out) => out,
            Err(e) => RunOutput::error(e),
        }
    }

    fn run_inner(&self, task: &Task, prompt: &str, ctx: &RunContext) -> Result<RunOutput, String> {
        match &self.inner {
            RunnerInner::Fake(FakeKind::Echo) => Ok(free_output(prompt, prompt)),
            RunnerInner::Fake(FakeKind::Oracle) => {
                let answer = match &task.grader {
                    GraderSpec::Exact { expected, .. } => expected[0].clone(),
                    GraderSpec::Json { expected } => crate::pypy::py_json_dumps(expected),
                    GraderSpec::Numeric { expected_raw, .. } => expected_raw.clone(),
                    _ => task.reference.clone().unwrap_or_default(),
                };
                Ok(free_output(prompt, &answer))
            }
            RunnerInner::Stub { message } => Err(format!("NotImplementedError: {message}")),
            RunnerInner::Model(m) => run_model(m, task, prompt, ctx),
            RunnerInner::ClaudeDraft(d) => d.run(task, prompt, ctx),
            RunnerInner::AppObservation(r) => r.run(task, prompt),
        }
    }

    pub fn configuration(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("runner_id".into(), json!(self.name));
        m.insert("kind".into(), json!(self.kind.as_str()));
        m.insert(
            "evaluation_mode".into(),
            json!(if self.kind == RunnerKind::Fake {
                "mock"
            } else {
                "live"
            }),
        );
        m.insert("framework".into(), json!("kritrim_mulyankan"));
        m.insert("framework_version".into(), json!(VERSION));
        m.insert("evaluation_track".into(), json!(self.kind.track()));
        m.insert("tools".into(), json!([]));
        m.insert("system_prompt".into(), Value::Null);
        match &self.inner {
            RunnerInner::Model(state) => {
                let cfg = provider_config(&state.provider).unwrap();
                m.insert("evaluation_mode".into(), json!("live"));
                m.insert("provider".into(), json!(state.provider));
                m.insert("requested_model".into(), json!(state.model));
                m.insert("options".into(), state.options.dump_json());
                m.insert(
                    "endpoint".into(),
                    cfg.resolve_base_url(&state.env)
                        .map(Value::String)
                        .unwrap_or(Value::Null),
                );
                m.insert("seed_supported".into(), json!(cfg.supports_seed));
                m.insert(
                    "price".into(),
                    state
                        .price
                        .as_ref()
                        .map(|p| price_dump(p, true))
                        .unwrap_or(Value::Null),
                );
                m.insert("pricing_currency".into(), json!("USD"));
                m.insert(
                    "cost_scope".into(),
                    json!("API charges; excludes hardware/subscription"),
                );
            }
            RunnerInner::ClaudeDraft(d) => {
                for (k, v) in d.configuration_extra() {
                    m.insert(k, v);
                }
            }
            RunnerInner::AppObservation(r) => {
                for (k, v) in r.configuration_extra() {
                    m.insert(k, v);
                }
            }
            _ => {}
        }
        m
    }
}

pub fn price_dump(price: &Price, include_none: bool) -> Value {
    let mut m = Map::new();
    let put = |m: &mut Map<String, Value>, key: &str, value: Value| {
        if include_none || !value.is_null() {
            m.insert(key.to_string(), value);
        }
    };
    put(&mut m, "input", json!(price.input));
    put(&mut m, "output", json!(price.output));
    put(&mut m, "cache_read", json!(price.cache_read));
    put(&mut m, "cache_write", json!(price.cache_write));
    put(
        &mut m,
        "long_context_threshold",
        json!(price.long_context_threshold),
    );
    put(&mut m, "long_input", json!(price.long_input));
    put(&mut m, "long_output", json!(price.long_output));
    put(&mut m, "source", json!(price.source));
    put(&mut m, "as_of", json!(price.as_of));
    Value::Object(m)
}

fn model_max_tokens(m: &ModelState, task: &Task) -> i64 {
    m.options.max_tokens.unwrap_or(task.max_output_tokens)
}

fn endpoint_problems(endpoint: &str) -> Vec<String> {
    let mut found = Vec::new();
    let Some((scheme, rest)) = endpoint.split_once("://") else {
        return vec!["endpoint must be an HTTP(S) URL".into()];
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("");
    if !["http", "https"].contains(&scheme) || host.is_empty() {
        found.push("endpoint must be an HTTP(S) URL".into());
    }
    if authority.contains('@') || endpoint.contains('?') || endpoint.contains('#') {
        found.push("endpoint must not contain credentials, query parameters or fragments".into());
    }
    found
}

fn model_problems(m: &ModelState) -> Vec<String> {
    let mut found = Vec::new();
    if m.price.is_none() {
        found.push(format!(
            "no price for {}/{}; add it to prices.yaml (see `km list prices`)",
            m.provider, m.model
        ));
    }
    if let Some(problem) = credentials_problem(&m.provider, &m.env) {
        found.push(problem);
    }
    if m.provider == "anthropic" && m.options.temperature.is_some() {
        found.push("Anthropic adapter does not support temperature".into());
    }
    let cfg = provider_config(&m.provider).unwrap();
    match cfg.resolve_base_url(&m.env) {
        Some(endpoint) => found.extend(endpoint_problems(&endpoint)),
        None if m.provider == "compatible" => found.push("COMPATIBLE_BASE_URL is not set".into()),
        None => {}
    }
    found
}

fn run_model(
    m: &ModelState,
    task: &Task,
    prompt: &str,
    ctx: &RunContext,
) -> Result<RunOutput, String> {
    let Some(price) = &m.price else {
        return Err(format!(
            "RuntimeError: refusing to call model:{}/{}: no price, so no budget check",
            m.provider, m.model
        ));
    };
    let adapter = m.adapter()?;
    let cfg = provider_config(&m.provider).unwrap();
    let max_tokens = model_max_tokens(m, task);
    let (result, attempts) = call_with_retries(m.options.retries, &*m.sleep, || {
        adapter.complete(
            &m.model,
            prompt,
            max_tokens,
            ctx.timeout_s,
            ctx.seed,
            m.options.effort.as_deref(),
            m.options.temperature,
        )
    })
    .map_err(|e: ApiError| e.to_string())?;
    let retries = attempts;
    let known = result.input_tokens.is_some() && result.output_tokens.is_some();
    let mut cost = if known {
        Some(price.cost_full(
            result.input_tokens.unwrap_or(0),
            result.output_tokens.unwrap_or(0),
            result.cache_read_tokens,
            result.cache_write_tokens,
        ))
    } else if cfg.local {
        Some(0.0)
    } else {
        None
    };
    if retries > 0 {
        cost = None; // Earlier 5xx requests may have been billed.
    }
    let error = if matches!(
        result.stop_reason.as_deref(),
        Some("length") | Some("max_tokens")
    ) {
        Some("output truncated by token limit".to_string())
    } else if result.text.is_empty() {
        Some("provider returned no answer".to_string())
    } else {
        None
    };
    let mut raw = Map::new();
    raw.insert("provider".into(), json!(m.provider));
    raw.insert("resolved_model".into(), json!(result.resolved_model));
    raw.insert("request".into(), Value::Object(result.request.clone()));
    raw.insert("stop_reason".into(), json!(result.stop_reason));
    raw.insert("retries".into(), json!(retries));
    raw.insert(
        "usage".into(),
        json!({"input_tokens": result.input_tokens, "output_tokens": result.output_tokens,
               "cache_read_tokens": result.cache_read_tokens,
               "cache_write_tokens": result.cache_write_tokens}),
    );
    raw.insert("price_per_mtok".into(), price_dump(price, false));
    raw.insert("response".into(), Value::Object(result.response.clone()));
    Ok(RunOutput {
        output: result.text.clone(),
        usage: Usage {
            input_tokens: result
                .input_tokens
                .map(|i| i + result.cache_read_tokens + result.cache_write_tokens),
            output_tokens: result.output_tokens,
            cost_usd: cost,
        },
        steps: Some(1),
        tool_calls: Some(0),
        error,
        raw,
    })
}

fn free_output(prompt: &str, answer: &str) -> RunOutput {
    RunOutput {
        output: answer.to_string(),
        usage: Usage {
            input_tokens: Some(approx_tokens(prompt)),
            output_tokens: Some(approx_tokens(answer)),
            cost_usd: Some(0.0),
        },
        steps: Some(1),
        tool_calls: Some(0),
        error: None,
        raw: Map::new(),
    }
}

pub const RUNNER_CATALOG: &[(&str, &str, bool, &str)] = &[
    ("echo", "fake", true, "Replies with the prompt. Free; a floor score."),
    ("oracle", "fake", true, "Replies with the expected/reference answer. Free; checks graders."),
    (
        "model:<provider>/<model>[,k=v]",
        "model",
        true,
        "Direct API call. Providers: anthropic, openai, xai, gemini, openrouter, ollama, vllm, compatible. Options: effort, max_tokens, temperature, retries.",
    ),
    ("harness:<cli>", "harness", false, "Headless coding/agent CLI in a temp workdir."),
    ("agent:<name>", "agent", false, "Personal agent via imported transcripts."),
];

fn parse_model_options(parts: &[&str], spec: &str) -> Result<ModelOptions, String> {
    let mut opts = ModelOptions {
        max_tokens: None,
        effort: None,
        temperature: None,
        retries: 0,
    };
    for part in parts {
        let Some((key, value)) = part.split_once('=') else {
            return Err(format!(
                "runner option must be key=value, got '{part}' in '{spec}'"
            ));
        };
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty() || value.is_empty() {
            return Err(format!(
                "runner option must be key=value, got '{part}' in '{spec}'"
            ));
        }
        let bad = |msg: &str| format!("bad options in '{spec}': {key}: {msg}");
        match key {
            "max_tokens" => {
                opts.max_tokens = Some(
                    value
                        .parse::<i64>()
                        .ok()
                        .filter(|v| *v >= 1)
                        .ok_or_else(|| bad("must be an integer >= 1"))?,
                )
            }
            "effort" => {
                if !["minimal", "low", "medium", "high", "xhigh", "max"].contains(&value) {
                    return Err(bad(
                        "expected one of minimal, low, medium, high, xhigh, max",
                    ));
                }
                opts.effort = Some(value.to_string());
            }
            "temperature" => {
                opts.temperature = Some(
                    value
                        .parse::<f64>()
                        .ok()
                        .filter(|v| (0.0..=2.0).contains(v))
                        .ok_or_else(|| bad("must be between 0 and 2"))?,
                )
            }
            "retries" => {
                opts.retries = value
                    .parse::<i64>()
                    .ok()
                    .filter(|v| (0..=2).contains(v))
                    .ok_or_else(|| bad("must be an integer between 0 and 2"))?
            }
            other => return Err(format!("bad options in '{spec}': {other}: unknown option")),
        }
    }
    Ok(opts)
}

pub fn make_model_runner(
    provider: &str,
    model: &str,
    options: ModelOptions,
    prices: &PriceTable,
    env: BTreeMap<String, String>,
) -> Result<Runner, String> {
    if provider_config(provider).is_none() {
        let names: Vec<&str> = crate::providers::PROVIDER_NAMES.to_vec();
        return Err(format!(
            "unknown provider '{provider}'; choose from {names:?}"
        ));
    }
    let name = format!("model:{provider}/{model}{}", options.suffix());
    Ok(Runner {
        name,
        kind: RunnerKind::Model,
        inner: RunnerInner::Model(Box::new(ModelState {
            adapter: Mutex::new(Arc::new(make_adapter(provider, None, &env))),
            price: prices.lookup(provider, model).cloned(),
            provider: provider.to_string(),
            model: model.to_string(),
            options,
            env,
            sleep: Box::new(|s| std::thread::sleep(std::time::Duration::from_secs_f64(s))),
        })),
    })
}

pub fn make_runner(
    spec: &str,
    prices: &PriceTable,
    env: &BTreeMap<String, String>,
) -> Result<Runner, String> {
    match spec.split_once(':') {
        None => match spec {
            "echo" => Ok(Runner::fake(FakeKind::Echo)),
            "oracle" => Ok(Runner::fake(FakeKind::Oracle)),
            _ => Err(format!("unknown runner '{spec}'; see `km list runners`")),
        },
        Some((kind, arg)) if !arg.is_empty() => match kind {
            "model" => {
                let parts: Vec<&str> = arg.split(',').collect();
                let (provider, sep, model) = match parts[0].split_once('/') {
                    Some((p, m)) if !p.is_empty() && !m.is_empty() => (p, true, m),
                    _ => ("", false, ""),
                };
                if !sep {
                    return Err(format!(
                        "model runner spec must be <provider>/<model>, got '{spec}'"
                    ));
                }
                let options = parse_model_options(&parts[1..], spec)?;
                make_model_runner(provider, model, options, prices, env.clone())
            }
            "harness" => Ok(Runner {
                name: format!("harness:{arg}"),
                kind: RunnerKind::Harness,
                inner: RunnerInner::Stub {
                    message: "HarnessRunner is not implemented yet".into(),
                },
            }),
            "agent" => Ok(Runner {
                name: format!("agent:{arg}"),
                kind: RunnerKind::Agent,
                inner: RunnerInner::Stub {
                    message: "AgentRunner is not implemented yet".into(),
                },
            }),
            _ => Err(format!("unknown runner '{spec}'; see `km list runners`")),
        },
        _ => Err(format!("unknown runner '{spec}'; see `km list runners`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_state_keeps_adapter_until_credentials_or_endpoint_change() {
        let mut env = BTreeMap::from([("OPENAI_API_KEY".into(), "initial-key".into())]);
        let mut runner = make_runner("model:openai/test", &PriceTable::default(), &env).unwrap();
        let RunnerInner::Model(state) = &mut runner.inner else {
            unreachable!()
        };
        let first = state.adapter().unwrap();
        env.insert("OPENAI_API_KEY".into(), "external-change".into());
        state.options.max_tokens = Some(13);
        state.options.temperature = Some(0.2);
        state.model = "another-model-on-the-same-provider".into();
        assert!(Arc::ptr_eq(&first, &state.adapter().unwrap()));

        state
            .env
            .insert("OPENAI_API_KEY".into(), "rotated-key".into());
        let rotated = state.adapter().unwrap();
        assert!(!Arc::ptr_eq(&first, &rotated));
        assert!(Arc::ptr_eq(&rotated, &state.adapter().unwrap()));

        state
            .env
            .insert("KM_OPENAI_BASE_URL".into(), "http://localhost:1234".into());
        let moved = state.adapter().unwrap();
        assert!(!Arc::ptr_eq(&rotated, &moved));
        assert!(Arc::ptr_eq(&moved, &state.adapter().unwrap()));
    }
}
