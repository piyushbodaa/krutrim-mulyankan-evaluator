//! Provider adapters, mirroring runners/providers.py. Anthropic speaks its
//! Messages API; every other provider speaks OpenAI Chat Completions with a
//! per-provider base URL. Retries cover only HTTP 429/5xx (never timeouts or
//! dropped connections, which may already have been billed).

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub protocol: &'static str, // "anthropic" | "openai"
    pub key_env: Option<&'static str>,
    pub base_url: Option<&'static str>,
    pub base_url_env: Option<&'static str>,
    pub max_tokens_param: &'static str,
    pub supports_seed: bool,
    pub local: bool,
}

impl ProviderConfig {
    pub fn resolve_base_url(&self, env: &BTreeMap<String, String>) -> Option<String> {
        match self.base_url_env {
            Some(name) => env
                .get(name)
                .cloned()
                .or_else(|| self.base_url.map(|s| s.to_string())),
            None => self.base_url.map(|s| s.to_string()),
        }
    }
}

pub fn provider_config(provider: &str) -> Option<ProviderConfig> {
    Some(match provider {
        "anthropic" => ProviderConfig {
            protocol: "anthropic",
            key_env: Some("ANTHROPIC_API_KEY"),
            base_url: None,
            base_url_env: None,
            max_tokens_param: "max_tokens",
            supports_seed: false,
            local: false,
        },
        "openai" => ProviderConfig {
            protocol: "openai",
            key_env: Some("OPENAI_API_KEY"),
            base_url: Some("https://api.openai.com/v1"),
            base_url_env: None,
            max_tokens_param: "max_completion_tokens",
            supports_seed: true,
            local: false,
        },
        "xai" => ProviderConfig {
            protocol: "openai",
            key_env: Some("XAI_API_KEY"),
            base_url: Some("https://api.x.ai/v1"),
            base_url_env: None,
            max_tokens_param: "max_tokens",
            supports_seed: false,
            local: false,
        },
        "gemini" => ProviderConfig {
            protocol: "openai",
            key_env: Some("GEMINI_API_KEY"),
            base_url: Some("https://generativelanguage.googleapis.com/v1beta/openai/"),
            base_url_env: None,
            max_tokens_param: "max_tokens",
            supports_seed: false,
            local: false,
        },
        "openrouter" => ProviderConfig {
            protocol: "openai",
            key_env: Some("OPENROUTER_API_KEY"),
            base_url: Some("https://openrouter.ai/api/v1"),
            base_url_env: None,
            max_tokens_param: "max_tokens",
            supports_seed: false,
            local: false,
        },
        "ollama" => ProviderConfig {
            protocol: "openai",
            key_env: None,
            base_url: Some("http://localhost:11434/v1"),
            base_url_env: Some("OLLAMA_BASE_URL"),
            max_tokens_param: "max_tokens",
            supports_seed: true,
            local: true,
        },
        "vllm" => ProviderConfig {
            protocol: "openai",
            key_env: None,
            base_url: Some("http://localhost:8000/v1"),
            base_url_env: Some("VLLM_BASE_URL"),
            max_tokens_param: "max_tokens",
            supports_seed: true,
            local: true,
        },
        "compatible" => ProviderConfig {
            protocol: "openai",
            key_env: Some("COMPATIBLE_API_KEY"),
            base_url: None,
            base_url_env: Some("COMPATIBLE_BASE_URL"),
            max_tokens_param: "max_tokens",
            supports_seed: false,
            local: false,
        },
        _ => return None,
    })
}

pub const PROVIDER_NAMES: &[&str] = &[
    "anthropic",
    "openai",
    "xai",
    "gemini",
    "openrouter",
    "ollama",
    "vllm",
    "compatible",
];

pub fn credentials_problem(provider: &str, env: &BTreeMap<String, String>) -> Option<String> {
    let cfg = provider_config(provider)?;
    if provider == "compatible" && cfg.resolve_base_url(env).is_none() {
        return Some("COMPATIBLE_BASE_URL is not set".into());
    }
    if cfg.local {
        return None;
    }
    let key_env = cfg.key_env?;
    if env.get(key_env).map(|v| !v.is_empty()).unwrap_or(false) {
        return None;
    }
    if provider == "anthropic" {
        if env
            .get("ANTHROPIC_AUTH_TOKEN")
            .map(|v| !v.is_empty())
            .unwrap_or(false)
            || env
                .get("ANTHROPIC_PROFILE")
                .map(|v| !v.is_empty())
                .unwrap_or(false)
        {
            return None;
        }
        let config_home = env
            .get("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env.get("HOME").map(|h| PathBuf::from(h).join(".config")));
        if let Some(home) = config_home {
            if home.join("anthropic").is_dir() {
                return None;
            }
        }
        return Some(
            "no Anthropic credentials (set ANTHROPIC_API_KEY or run `ant auth login`)".into(),
        );
    }
    Some(format!("{key_env} is not set"))
}

#[derive(Debug, Clone, Default)]
pub struct Completion {
    pub text: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub stop_reason: Option<String>,
    pub request: Map<String, Value>,
    pub response: Map<String, Value>,
    pub resolved_model: Option<String>,
}

#[derive(Debug)]
pub enum ApiError {
    /// HTTP status error with response headers (retryable only on 429/5xx).
    Status {
        code: u16,
        retry_after: Option<String>,
        body: String,
    },
    /// Transport failure (including timeouts): never retried.
    Transport(String),
    Other(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Status { code, body, .. } => {
                write!(
                    f,
                    "APIStatusError: HTTP {code}: {}",
                    body.chars().take(300).collect::<String>()
                )
            }
            ApiError::Transport(msg) => write!(f, "APIConnectionError: {msg}"),
            ApiError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

pub fn is_retryable(err: &ApiError) -> bool {
    matches!(err, ApiError::Status { code, .. } if *code == 429 || *code >= 500)
}

pub fn retry_delay(err: &ApiError, attempt: u32) -> f64 {
    retry_delay_cap(err, attempt, 30.0)
}

pub fn retry_delay_cap(err: &ApiError, attempt: u32, cap: f64) -> f64 {
    if let ApiError::Status {
        retry_after: Some(h),
        ..
    } = err
    {
        if let Ok(v) = h.parse::<f64>() {
            return v.max(0.0).min(cap);
        }
    }
    (2f64).powi(attempt as i32).min(cap)
}

pub type SleepFn = Box<dyn Fn(f64) + Send + Sync>;

pub fn call_with_retries<T>(
    max_retries: i64,
    sleep: &dyn Fn(f64),
    mut f: impl FnMut() -> Result<T, ApiError>,
) -> Result<(T, i64), ApiError> {
    for attempt in 0..=max_retries {
        match f() {
            Ok(v) => return Ok((v, attempt)),
            Err(e) => {
                if attempt == max_retries || !is_retryable(&e) {
                    return Err(e);
                }
                sleep(retry_delay(&e, attempt as u32));
            }
        }
    }
    unreachable!()
}

fn agent(timeout_s: f64) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs_f64(timeout_s.max(0.05)))
        .redirects(0)
        .build()
}

fn send_json(
    url: &str,
    headers: &[(&str, String)],
    body: &Map<String, Value>,
    timeout_s: f64,
) -> Result<Value, ApiError> {
    let mut req = agent(timeout_s)
        .post(url)
        .set("Content-Type", "application/json");
    for (k, v) in headers {
        req = req.set(k, v);
    }
    match req.send_json(Value::Object(body.clone())) {
        Ok(resp) => resp.into_json::<Value>().map_err(|e| {
            ApiError::Other(format!("ValueError: provider response was not JSON: {e}"))
        }),
        Err(ureq::Error::Status(code, resp)) => {
            let retry_after = resp.header("retry-after").map(|s| s.to_string());
            let text = resp.into_string().unwrap_or_default();
            Err(ApiError::Status {
                code,
                retry_after,
                body: text,
            })
        }
        Err(ureq::Error::Transport(t)) => Err(ApiError::Transport(t.to_string())),
    }
}

fn obj(v: &Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    }
}

pub struct Adapter {
    pub provider: String,
    cfg: ProviderConfig,
    api_key: Option<String>,
    env: BTreeMap<String, String>,
}

pub fn make_adapter(
    provider: &str,
    api_key: Option<String>,
    env: &BTreeMap<String, String>,
) -> Adapter {
    let cfg = provider_config(provider).expect("known provider");
    let key = api_key.or_else(|| {
        cfg.key_env
            .and_then(|name| env.get(name).cloned())
            .filter(|v| !v.is_empty())
    });
    Adapter {
        provider: provider.to_string(),
        cfg,
        api_key: key,
        env: env.clone(),
    }
}

impl Adapter {
    /// KM_<PROVIDER>_BASE_URL overrides for tests/mock servers; unset in
    /// normal use, where this returns exactly what the Python adapters use
    /// (plus ANTHROPIC_BASE_URL, which the Anthropic SDK also honours).
    fn base_url(&self) -> Option<String> {
        let hook = format!("KM_{}_BASE_URL", self.provider.to_uppercase());
        if let Some(v) = self.env.get(&hook) {
            if !v.is_empty() {
                return Some(v.trim_end_matches('/').to_string());
            }
        }
        if self.provider == "anthropic" {
            if let Some(v) = self.env.get("ANTHROPIC_BASE_URL") {
                if !v.is_empty() {
                    return Some(v.trim_end_matches('/').to_string());
                }
            }
            return Some("https://api.anthropic.com".to_string());
        }
        self.cfg
            .resolve_base_url(&self.env)
            .map(|u| u.trim_end_matches('/').to_string())
    }

    pub fn complete(
        &self,
        model: &str,
        prompt: &str,
        max_tokens: i64,
        timeout_s: f64,
        seed: i64,
        effort: Option<&str>,
        temperature: Option<f64>,
    ) -> Result<Completion, ApiError> {
        if self.provider == "anthropic" {
            self.complete_anthropic(model, prompt, max_tokens, timeout_s, effort, temperature)
        } else {
            self.complete_openai(
                model,
                prompt,
                max_tokens,
                timeout_s,
                seed,
                effort,
                temperature,
            )
        }
    }

    fn complete_anthropic(
        &self,
        model: &str,
        prompt: &str,
        max_tokens: i64,
        timeout_s: f64,
        effort: Option<&str>,
        temperature: Option<f64>,
    ) -> Result<Completion, ApiError> {
        if temperature.is_some() {
            return Err(ApiError::Other(
                "ValueError: current Claude models do not accept temperature; drop the option"
                    .into(),
            ));
        }
        let mut request = Map::new();
        request.insert("model".into(), json!(model));
        request.insert("max_tokens".into(), json!(max_tokens));
        request.insert(
            "messages".into(),
            json!([{"role": "user", "content": prompt}]),
        );
        if let Some(e) = effort {
            request.insert("output_config".into(), json!({"effort": e}));
        }
        let mut headers: Vec<(&str, String)> =
            vec![("anthropic-version", "2023-06-01".to_string())];
        if let Some(key) = &self.api_key {
            headers.push(("x-api-key", key.clone()));
        } else if let Some(token) = self.env.get("ANTHROPIC_AUTH_TOKEN") {
            headers.push(("Authorization", format!("Bearer {token}")));
        }
        let url = format!("{}/v1/messages", self.base_url().unwrap_or_default());
        let body = send_json(&url, &headers, &request, timeout_s)?;
        let message = obj(&body);
        let text: String = message
            .get("content")
            .and_then(|v| v.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(|v| v.as_str()) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(|v| v.as_str()))
                    .collect::<String>()
            })
            .unwrap_or_default();
        let usage = message.get("usage").cloned().unwrap_or_else(|| json!({}));
        Ok(Completion {
            text,
            input_tokens: usage.get("input_tokens").and_then(|v| v.as_i64()),
            output_tokens: usage.get("output_tokens").and_then(|v| v.as_i64()),
            cache_read_tokens: usage
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            cache_write_tokens: usage
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            stop_reason: message
                .get("stop_reason")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            request,
            response: message.clone(),
            resolved_model: message
                .get("model")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        })
    }

    fn complete_openai(
        &self,
        model: &str,
        prompt: &str,
        max_tokens: i64,
        timeout_s: f64,
        seed: i64,
        effort: Option<&str>,
        temperature: Option<f64>,
    ) -> Result<Completion, ApiError> {
        let mut request = Map::new();
        request.insert("model".into(), json!(model));
        request.insert(self.cfg.max_tokens_param.to_string(), json!(max_tokens));
        request.insert(
            "messages".into(),
            json!([{"role": "user", "content": prompt}]),
        );
        if self.cfg.supports_seed {
            request.insert("seed".into(), json!(seed));
        }
        if let Some(e) = effort {
            request.insert("reasoning_effort".into(), json!(e));
        }
        if let Some(t) = temperature {
            request.insert("temperature".into(), json!(t));
        }
        let key = self
            .api_key
            .clone()
            .or_else(|| {
                if self.cfg.local {
                    Some("ollama".to_string())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        let url = format!("{}/chat/completions", self.base_url().unwrap_or_default());
        let body = send_json(
            &url,
            &[("Authorization", format!("Bearer {key}"))],
            &request,
            timeout_s,
        )?;
        let root = obj(&body);
        let choice = root
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first().cloned())
            .unwrap_or_else(|| json!({}));
        let usage = root.get("usage").cloned().unwrap_or(Value::Null);
        let cached = usage
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let prompt_tokens = usage.get("prompt_tokens").and_then(|v| v.as_i64());
        Ok(Completion {
            text: choice
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            input_tokens: prompt_tokens.map(|p| p - cached),
            output_tokens: usage.get("completion_tokens").and_then(|v| v.as_i64()),
            cache_read_tokens: cached,
            cache_write_tokens: 0,
            stop_reason: choice
                .get("finish_reason")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            request,
            response: root.clone(),
            resolved_model: root
                .get("model")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        })
    }
}
