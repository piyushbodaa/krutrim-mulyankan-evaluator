//! Live side-by-side answers through OpenRouter: one prompt to a few models
//! at once, streamed, with the numbers people compare. It records when the
//! request was sent, when OpenRouter answered, the first thought and first
//! word, every pause, the token counts, the cost, which provider served the
//! model and why it stopped, plus OpenRouter's own server-side timings.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";

/// Where OpenRouter lives. KM_OPENROUTER_BASE points tests at a stand-in.
pub fn openrouter_base() -> String {
    std::env::var("KM_OPENROUTER_BASE")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OPENROUTER_BASE.to_string())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(300))
        .build()
}

fn identified(request: ureq::Request, key: Option<&str>) -> ureq::Request {
    let request = request
        .set("HTTP-Referer", "https://krutrimmulyankan.in")
        .set("X-Title", "Krutrim Mulyankan");
    match key {
        Some(key) => request.set("Authorization", &format!("Bearer {key}")),
        None => request,
    }
}

/// What went wrong, in words, with OpenRouter's own detail when it sent one.
pub fn status_message(code: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|d| !d.trim().is_empty());
    let plain = match code {
        401 => "OpenRouter did not accept this key",
        402 => {
            "Your OpenRouter account is out of credit. Add credit at openrouter.ai and try again"
        }
        403 => "OpenRouter refused this request",
        404 => "OpenRouter does not offer this model right now",
        408 | 504 => "The model took too long to answer",
        429 => "Too many requests right now. Wait a moment and try again",
        502 | 503 => "The model is not available right now. Try again or pick another",
        _ => "OpenRouter reported a problem",
    };
    match detail {
        Some(detail) => format!("{plain}. {detail}"),
        None => format!("{plain} (HTTP {code})."),
    }
}

/// Everything measured for one model's answer. Times are seconds from the
/// moment the request left this computer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveMetrics {
    pub model: String,
    pub http_status: Option<u16>,
    pub connected_s: Option<f64>,
    pub first_thinking_s: Option<f64>,
    pub first_answer_s: Option<f64>,
    pub total_s: f64,
    pub chunks: usize,
    pub longest_pause_s: f64,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub cached_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub provider: Option<String>,
    pub generation_id: Option<String>,
    pub finish_reason: Option<String>,
    pub answer_chars: usize,
    pub answer_words: usize,
    pub thinking_chars: usize,
    pub error: Option<String>,
    pub stopped: bool,
}

impl LiveMetrics {
    /// The first token of any kind, thinking or answer.
    pub fn first_token_s(&self) -> Option<f64> {
        match (self.first_thinking_s, self.first_answer_s) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Output tokens per second while writing: from the first token to the end.
    pub fn tokens_per_s(&self) -> Option<f64> {
        let tokens = self.output_tokens? as f64;
        let writing = self.total_s - self.first_token_s()?;
        (tokens > 0.0 && writing > 0.05).then(|| tokens / writing)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "model": self.model,
            "http_status": self.http_status,
            "connected_s": self.connected_s,
            "first_thinking_s": self.first_thinking_s,
            "first_answer_s": self.first_answer_s,
            "first_token_s": self.first_token_s(),
            "total_s": self.total_s,
            "tokens_per_s": self.tokens_per_s(),
            "chunks": self.chunks,
            "longest_pause_s": self.longest_pause_s,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
            "reasoning_tokens": self.reasoning_tokens,
            "cached_tokens": self.cached_tokens,
            "cost_usd": self.cost_usd,
            "provider": self.provider,
            "generation_id": self.generation_id,
            "finish_reason": self.finish_reason,
            "answer_chars": self.answer_chars,
            "answer_words": self.answer_words,
            "thinking_chars": self.thinking_chars,
            "error": self.error,
            "stopped": self.stopped,
        })
    }
}

pub enum LiveEvent<'a> {
    /// OpenRouter accepted the request and started streaming.
    Connected(f64),
    /// The first token of this kind arrived.
    First { at: f64, thinking: bool },
    /// More text: the answer, or the model's visible thinking.
    Text { text: &'a str, thinking: bool },
}

/// Send one prompt to one model and stream the answer, calling `on_event`
/// as it arrives. Stops early, keeping what arrived, when `stop` is set.
pub fn stream_chat(
    base: &str,
    key: &str,
    model: &str,
    prompt: &str,
    stop: &AtomicBool,
    on_event: &mut dyn FnMut(LiveEvent<'_>),
) -> LiveMetrics {
    let started = Instant::now();
    let since = || started.elapsed().as_secs_f64();
    let mut m = LiveMetrics {
        model: model.to_string(),
        ..LiveMetrics::default()
    };
    let body = json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "stream": true,
        "usage": {"include": true},
    });
    let request = identified(agent().post(&format!("{base}/chat/completions")), Some(key));
    let response = match request.send_json(body) {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            m.http_status = Some(code);
            m.error = Some(status_message(
                code,
                &response.into_string().unwrap_or_default(),
            ));
            m.total_s = since();
            return m;
        }
        Err(error) => {
            m.error = Some(format!("Could not reach OpenRouter: {error}"));
            m.total_s = since();
            return m;
        }
    };
    m.http_status = Some(response.status());
    let connected = since();
    m.connected_s = Some(connected);
    on_event(LiveEvent::Connected(connected));
    let mut last_text: Option<f64> = None;
    let mut answer = String::new();
    for line in BufReader::new(response.into_reader()).lines() {
        if stop.load(Ordering::Relaxed) {
            m.stopped = true;
            break;
        }
        let Ok(line) = line else {
            m.error =
                Some("The connection to OpenRouter dropped before the answer finished.".into());
            break;
        };
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(error) = chunk.get("error") {
            m.error = Some(
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("OpenRouter reported a problem.")
                    .to_string(),
            );
            break;
        }
        if m.provider.is_none() {
            m.provider = chunk
                .get("provider")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        if m.generation_id.is_none() {
            m.generation_id = chunk.get("id").and_then(Value::as_str).map(str::to_string);
        }
        if let Some(choice) = chunk.pointer("/choices/0") {
            for (field, thinking) in [("reasoning", true), ("content", false)] {
                let Some(text) = choice
                    .pointer(&format!("/delta/{field}"))
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                else {
                    continue;
                };
                let at = since();
                let first = if thinking {
                    &mut m.first_thinking_s
                } else {
                    &mut m.first_answer_s
                };
                if first.is_none() {
                    *first = Some(at);
                    on_event(LiveEvent::First { at, thinking });
                }
                if let Some(previous) = last_text {
                    m.longest_pause_s = m.longest_pause_s.max(at - previous);
                }
                last_text = Some(at);
                m.chunks += 1;
                if thinking {
                    m.thinking_chars += text.chars().count();
                } else {
                    m.answer_chars += text.chars().count();
                    answer.push_str(text);
                }
                on_event(LiveEvent::Text { text, thinking });
            }
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                m.finish_reason = Some(reason.to_string());
            }
        }
        if let Some(usage) = chunk.get("usage").filter(|usage| usage.is_object()) {
            m.input_tokens = usage.get("prompt_tokens").and_then(Value::as_i64);
            m.output_tokens = usage.get("completion_tokens").and_then(Value::as_i64);
            m.reasoning_tokens = usage
                .pointer("/completion_tokens_details/reasoning_tokens")
                .and_then(Value::as_i64);
            m.cached_tokens = usage
                .pointer("/prompt_tokens_details/cached_tokens")
                .and_then(Value::as_i64);
            m.cost_usd = usage.get("cost").and_then(Value::as_f64);
        }
    }
    m.total_s = since();
    m.answer_words = count_words(&answer);
    m
}

/// Word count of an answer, for the metrics sheet.
pub fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}

/// OpenRouter's own record of a finished answer: its server-side latency
/// and generation time, native token counts and the upstream provider. It
/// appears a moment after the stream ends, so this asks a few times.
pub fn generation_stats(base: &str, key: &str, id: &str, attempts: usize) -> Option<Value> {
    for attempt in 0..attempts.max(1) {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(600 * attempt as u64));
        }
        let response = identified(agent().get(&format!("{base}/generation")), Some(key))
            .query("id", id)
            .call();
        if let Ok(response) = response {
            if let Ok(doc) = response.into_json::<Value>() {
                let data = &doc["data"];
                if data.is_object() {
                    return Some(json!({
                        "latency_ms": data["latency"],
                        "generation_time_ms": data["generation_time"],
                        "moderation_latency_ms": data["moderation_latency"],
                        "native_input_tokens": data["native_tokens_prompt"],
                        "native_output_tokens": data["native_tokens_completion"],
                        "native_reasoning_tokens": data["native_tokens_reasoning"],
                        "native_cached_tokens": data["native_tokens_cached"],
                        "provider": data["provider_name"],
                        "total_cost_usd": data["total_cost"],
                        "native_finish_reason": data["native_finish_reason"],
                    }));
                }
            }
        }
    }
    None
}

fn price(model: &Value, field: &str) -> Option<f64> {
    let value = model.pointer(&format!("/pricing/{field}"))?;
    let number = value
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| value.as_f64())?;
    (number >= 0.0).then_some(number)
}

/// The text-writing models OpenRouter offers, with prices per token in
/// dollars, the context window and the longest reply each allows.
pub fn openrouter_models(base: &str) -> Result<Vec<Value>, String> {
    let response = identified(agent().get(&format!("{base}/models")), None)
        .call()
        .map_err(|error| format!("Could not load the list of AIs from OpenRouter: {error}"))?;
    let doc: Value = response
        .into_json()
        .map_err(|error| format!("OpenRouter sent a list that could not be read: {error}"))?;
    let mut models = Vec::new();
    for model in doc["data"].as_array().cloned().unwrap_or_default() {
        let Some(id) = model["id"].as_str() else {
            continue;
        };
        if let Some(outputs) = model
            .pointer("/architecture/output_modalities")
            .and_then(Value::as_array)
        {
            if !outputs.iter().any(|o| o == "text") {
                continue;
            }
        }
        let input = price(&model, "prompt");
        let output = price(&model, "completion");
        models.push(json!({
            "id": id,
            "name": model["name"].as_str().unwrap_or(id),
            "input_usd": input,
            "output_usd": output,
            "reasoning_usd": price(&model, "internal_reasoning"),
            "context": model["context_length"],
            "max_output": model.pointer("/top_provider/max_completion_tokens").cloned().unwrap_or(Value::Null),
            "created": model["created"],
            "free": id.ends_with(":free") || (input == Some(0.0) && output == Some(0.0)),
            "inputs": model.pointer("/architecture/input_modalities").cloned().unwrap_or(Value::Null),
            "outputs": model.pointer("/architecture/output_modalities").cloned().unwrap_or(Value::Null),
        }));
    }
    Ok(models)
}

/// Whether a key works, and how much credit is left when OpenRouter says.
pub fn openrouter_key_status(base: &str, key: &str) -> Result<Value, String> {
    match identified(agent().get(&format!("{base}/key")), Some(key)).call() {
        Ok(response) => {
            let doc: Value = response.into_json().unwrap_or(Value::Null);
            let data = &doc["data"];
            let credits = identified(agent().get(&format!("{base}/credits")), Some(key))
                .call()
                .ok()
                .and_then(|r| r.into_json::<Value>().ok());
            let left = credits.and_then(|c| {
                Some(
                    c.pointer("/data/total_credits")?.as_f64()?
                        - c.pointer("/data/total_usage")?.as_f64()?,
                )
            });
            Ok(json!({
                "ok": true,
                "label": data["label"],
                "limit_remaining_usd": data["limit_remaining"],
                "credit_left_usd": left,
                "free_tier": data["is_free_tier"],
            }))
        }
        Err(ureq::Error::Status(code, response)) => Ok(json!({
            "ok": false,
            "error": status_message(code, &response.into_string().unwrap_or_default()),
        })),
        Err(error) => Err(format!("Could not reach OpenRouter: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A one-request stand-in for OpenRouter that replies with `body` in
    /// pieces, `gap` apart, and returns the request it received.
    fn serve(
        status: &'static str,
        pieces: Vec<String>,
        gap: Duration,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let n = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(head) = text.find("\r\n\r\n") {
                    let length = text[..head]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= head + 4 + length {
                        break;
                    }
                }
            }
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
            );
            for piece in pieces {
                // The client may hang up early (a stop); that is fine here.
                if stream
                    .write_all(piece.as_bytes())
                    .and_then(|_| stream.flush())
                    .is_err()
                {
                    break;
                }
                std::thread::sleep(gap);
            }
            String::from_utf8_lossy(&request).into_owned()
        });
        (base, handle)
    }

    fn sse(value: Value) -> String {
        format!("data: {value}\n\n")
    }

    #[test]
    fn streams_thinking_then_answer_with_usage_and_timings() {
        let pieces = vec![
            ": OPENROUTER PROCESSING\n\n".to_string(),
            sse(
                json!({"id": "gen-1", "provider": "Anthropic", "choices": [{"delta": {"reasoning": "Let me think."}}]}),
            ),
            sse(json!({"id": "gen-1", "choices": [{"delta": {"content": "Hello "}}]})),
            sse(
                json!({"id": "gen-1", "choices": [{"delta": {"content": "world."}, "finish_reason": "stop"}]}),
            ),
            sse(
                json!({"id": "gen-1", "choices": [], "usage": {"prompt_tokens": 12, "completion_tokens": 40,
                "completion_tokens_details": {"reasoning_tokens": 30}, "prompt_tokens_details": {"cached_tokens": 0}, "cost": 0.00042}}),
            ),
            "data: [DONE]\n\n".to_string(),
        ];
        let (base, server) = serve("200 OK", pieces, Duration::from_millis(30));
        let stop = AtomicBool::new(false);
        let mut seen = Vec::new();
        let metrics = stream_chat(
            &base,
            "sk-test",
            "anthropic/claude-x",
            "Say hello",
            &stop,
            &mut |event| {
                seen.push(match event {
                    LiveEvent::Connected(_) => "connected".to_string(),
                    LiveEvent::First { thinking, .. } => format!("first:{thinking}"),
                    LiveEvent::Text { text, thinking } => format!("{thinking}:{text}"),
                })
            },
        );
        let request = server.join().unwrap();
        assert!(
            request.contains("authorization: Bearer sk-test")
                || request.contains("Authorization: Bearer sk-test")
        );
        assert!(request.contains("\"stream\":true") && request.contains("Say hello"));
        assert_eq!(
            seen,
            [
                "connected",
                "first:true",
                "true:Let me think.",
                "first:false",
                "false:Hello ",
                "false:world."
            ]
        );
        assert_eq!(metrics.provider.as_deref(), Some("Anthropic"));
        assert_eq!(metrics.generation_id.as_deref(), Some("gen-1"));
        assert_eq!(metrics.finish_reason.as_deref(), Some("stop"));
        assert_eq!(
            (
                metrics.input_tokens,
                metrics.output_tokens,
                metrics.reasoning_tokens
            ),
            (Some(12), Some(40), Some(30))
        );
        assert_eq!(metrics.cost_usd, Some(0.00042));
        assert_eq!(
            (metrics.chunks, metrics.answer_chars, metrics.thinking_chars),
            (3, 12, 13)
        );
        assert_eq!(metrics.answer_words, 2);
        let first = metrics.first_token_s().unwrap();
        assert!(
            first < metrics.first_answer_s.unwrap()
                && metrics.first_answer_s.unwrap() < metrics.total_s
        );
        assert!(
            metrics.longest_pause_s >= 0.02,
            "{}",
            metrics.longest_pause_s
        );
        assert!(metrics.tokens_per_s().unwrap() > 0.0);
        assert!(metrics.error.is_none() && !metrics.stopped);
    }

    #[test]
    fn errors_read_as_what_to_do() {
        let (base, _server) = serve(
            "402 Payment Required",
            vec![json!({"error": {"message": "Insufficient credits"}}).to_string()],
            Duration::ZERO,
        );
        let metrics = stream_chat(&base, "k", "m", "p", &AtomicBool::new(false), &mut |_| {});
        assert_eq!(metrics.http_status, Some(402));
        assert_eq!(
            metrics.error.as_deref(),
            Some("Your OpenRouter account is out of credit. Add credit at openrouter.ai and try again. Insufficient credits")
        );
        assert_eq!(
            status_message(404, ""),
            "OpenRouter does not offer this model right now (HTTP 404)."
        );
    }

    #[test]
    fn mid_stream_errors_and_stops_keep_what_arrived() {
        let pieces = vec![
            sse(json!({"choices": [{"delta": {"content": "Part"}}]})),
            sse(json!({"error": {"message": "Provider returned error"}})),
        ];
        let (base, _server) = serve("200 OK", pieces, Duration::from_millis(5));
        let metrics = stream_chat(&base, "k", "m", "p", &AtomicBool::new(false), &mut |_| {});
        assert_eq!(metrics.answer_chars, 4);
        assert_eq!(metrics.error.as_deref(), Some("Provider returned error"));

        let pieces = (0..50)
            .map(|i| sse(json!({"choices": [{"delta": {"content": format!("{i} ")}}]})))
            .collect();
        let (base, _server) = serve("200 OK", pieces, Duration::from_millis(10));
        let stop = AtomicBool::new(false);
        let mut count = 0;
        let metrics = stream_chat(&base, "k", "m", "p", &stop, &mut |event| {
            if matches!(event, LiveEvent::Text { .. }) {
                count += 1;
                if count == 3 {
                    stop.store(true, Ordering::Relaxed);
                }
            }
        });
        assert!(metrics.stopped && metrics.chunks == 3);
    }

    #[test]
    fn model_list_keeps_text_models_with_prices_and_context() {
        let doc = json!({"data": [
            {"id": "a/text", "name": "Text One", "context_length": 200000, "created": 5,
             "pricing": {"prompt": "0.000002", "completion": "0.00001"},
             "top_provider": {"max_completion_tokens": 64000},
             "architecture": {"input_modalities": ["text"], "output_modalities": ["text"]}},
            {"id": "b/image", "name": "Pictures", "architecture": {"output_modalities": ["image"]}},
            {"id": "c/free:free", "name": "Free One", "pricing": {"prompt": "0", "completion": "0"}},
            {"id": "openrouter/auto", "name": "Auto", "pricing": {"prompt": "-1", "completion": "-1"}},
        ]});
        let body = doc.to_string();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/api/v1", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 2048];
            let _ = stream.read(&mut buffer);
            write!(stream, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len()).unwrap();
        });
        let models = openrouter_models(&base).unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["a/text", "c/free:free", "openrouter/auto"]);
        assert_eq!(models[0]["input_usd"], json!(0.000002));
        assert_eq!(models[0]["context"], json!(200000));
        assert_eq!(models[0]["max_output"], json!(64000));
        assert_eq!(models[1]["free"], json!(true));
        assert_eq!(models[2]["input_usd"], Value::Null);
    }
}
