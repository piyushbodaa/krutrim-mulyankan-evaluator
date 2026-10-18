//! Model runners against a local mock HTTP server: no network beyond
//! loopback, no real keys, no spend. Mirrors tests/test_model_runner.py cases
//! (same canned payloads as tests/model_mocks.py).

use krutrim_evaluator::pricing::{bundled_prices, Price, PriceTable};
use krutrim_evaluator::runners::{make_runner, RunContext};
use krutrim_evaluator::suite::load_suite;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[allow(clippy::type_complexity)]
struct Mock {
    base: String,
    requests: Arc<Mutex<Vec<(String, Vec<(String, String)>, serde_json::Value)>>>,
}

#[allow(clippy::type_complexity)]
fn start_mock(responses: Vec<(u16, Vec<(String, String)>, String)>) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests: Arc<Mutex<Vec<_>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let queue = Arc::new(Mutex::new(responses));
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut stream) = conn else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            if reader.read_line(&mut first).unwrap_or(0) == 0 {
                continue;
            }
            let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
            let mut headers = Vec::new();
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                    break;
                }
                if let Some((k, v)) = line.split_once(':') {
                    if k.trim().eq_ignore_ascii_case("content-length") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    headers.push((k.trim().to_lowercase(), v.trim().to_string()));
                }
            }
            let mut body = vec![0u8; len];
            let _ = reader.read_exact(&mut body);
            let json: serde_json::Value =
                serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
            captured.lock().unwrap().push((path, headers, json));
            let mut q = queue.lock().unwrap();
            let (status, extra, text) = if q.len() > 1 {
                q.remove(0)
            } else {
                q.first().cloned().unwrap()
            };
            let mut head = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n",
                text.len()
            );
            for (k, v) in extra {
                head += &format!("{k}: {v}\r\n");
            }
            head += "\r\n";
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(text.as_bytes());
        }
    });
    Mock {
        base: format!("http://127.0.0.1:{port}"),
        requests,
    }
}

fn anthropic_message(text: &str, input: i64, output: i64) -> (u16, Vec<(String, String)>, String) {
    (
        200,
        vec![],
        serde_json::json!({
            "id": "msg_test", "type": "message", "role": "assistant", "model": "claude-test",
            "content": [{"type": "thinking", "thinking": "", "signature": "sig"},
                        {"type": "text", "text": text}],
            "stop_reason": "end_turn", "stop_sequence": null,
            "usage": {"input_tokens": input, "output_tokens": output,
                      "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0}
        })
        .to_string(),
    )
}

fn chat_completion(
    text: &str,
    prompt_tokens: i64,
    completion_tokens: i64,
    cached: i64,
) -> (u16, Vec<(String, String)>, String) {
    (
        200,
        vec![],
        serde_json::json!({
            "id": "chatcmpl-test", "object": "chat.completion", "created": 1,
            "model": "test-model",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": text},
                         "finish_reason": "stop"}],
            "usage": {"prompt_tokens": prompt_tokens, "completion_tokens": completion_tokens,
                      "total_tokens": prompt_tokens + completion_tokens,
                      "prompt_tokens_details": {"cached_tokens": cached}}
        })
        .to_string(),
    )
}

fn api_error(status: u16, retry_after: Option<&str>) -> (u16, Vec<(String, String)>, String) {
    (
        status,
        retry_after
            .map(|v| vec![("retry-after".into(), v.into())])
            .unwrap_or_default(),
        serde_json::json!({"type": "error", "error": {"type": "x", "message": "boom"}}).to_string(),
    )
}

fn prices() -> PriceTable {
    let mut t = bundled_prices();
    let p = |input: f64, output: f64| Price {
        input,
        output,
        cache_read: None,
        cache_write: None,
        long_context_threshold: None,
        long_input: None,
        long_output: None,
        source: None,
        as_of: None,
    };
    t.prices.insert("openai/gpt-test".into(), p(1.0, 4.0));
    t.prices.insert("xai/grok-test".into(), p(2.0, 8.0));
    t.prices.insert("gemini/gemini-test".into(), p(0.5, 2.0));
    t.prices
        .insert("openrouter/test-model-x".into(), p(3.0, 6.0));
    t
}

fn task() -> krutrim_evaluator::Task {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let suite = load_suite(&root.join("../../../suites/example.yaml")).unwrap();
    suite
        .tasks
        .into_iter()
        .find(|t| t.id == "coding.sum-of-squares")
        .unwrap()
}

fn env_for(provider: &str, base: &str) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert(
        format!("KM_{}_BASE_URL", provider.to_uppercase()),
        base.to_string(),
    );
    let key_env = match provider {
        "anthropic" => "ANTHROPIC_API_KEY",
        "openai" => "OPENAI_API_KEY",
        "xai" => "XAI_API_KEY",
        "gemini" => "GEMINI_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "compatible" => "COMPATIBLE_API_KEY",
        _ => "",
    };
    if !key_env.is_empty() {
        env.insert(key_env.to_string(), "test-key".to_string());
    }
    if provider == "compatible" {
        env.insert("COMPATIBLE_BASE_URL".to_string(), base.to_string());
    }
    if provider == "ollama" {
        env.remove("KM_OLLAMA_BASE_URL");
        env.insert("OLLAMA_BASE_URL".to_string(), format!("{base}/v1"));
    }
    if provider == "vllm" {
        env.remove("KM_VLLM_BASE_URL");
        env.insert("VLLM_BASE_URL".to_string(), format!("{base}/v1"));
    }
    env
}

fn ctx() -> RunContext {
    RunContext {
        workdir: PathBuf::from("."),
        seed: 1234,
        timeout_s: 30.0,
        cancel: None,
    }
}

#[test]
fn anthropic_request_shape_and_cost() {
    let mock = start_mock(vec![anthropic_message("14", 100, 50)]);
    let env = env_for("anthropic", &mock.base);
    let runner = make_runner(
        "model:anthropic/claude-opus-5-5,effort=high",
        &prices(),
        &env,
    )
    .unwrap();
    let t = task();
    assert!(runner.problems().is_empty(), "{:?}", runner.problems());
    let out = runner.run(&t, &t.render_prompt(), &ctx());
    let reqs = mock.requests.lock().unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].0, "/v1/messages");
    assert!(reqs[0]
        .1
        .iter()
        .any(|(k, v)| k == "x-api-key" && v == "test-key"));
    let body = &reqs[0].2;
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["max_tokens"], t.max_output_tokens);
    assert_eq!(body["output_config"], serde_json::json!({"effort": "high"}));
    assert!(body.get("temperature").is_none());
    assert_eq!(out.output, "14");
    assert_eq!(out.usage.input_tokens, Some(100));
    assert_eq!(out.usage.output_tokens, Some(50));
    assert!((out.usage.cost_usd.unwrap() - (100.0 * 4.0 + 50.0 * 20.0) / 1e6).abs() < 1e-12);
    assert_eq!(out.raw["retries"], 0);
}

#[test]
fn openai_shape_seed_and_cached_pricing() {
    let mock = start_mock(vec![chat_completion("14", 120, 30, 20)]);
    let env = env_for("openai", &mock.base);
    let runner = make_runner(
        "model:openai/gpt-test,effort=low,max_tokens=777",
        &prices(),
        &env,
    )
    .unwrap();
    let t = task();
    let out = runner.run(&t, "q", &ctx());
    let reqs = mock.requests.lock().unwrap();
    assert_eq!(reqs[0].0, "/chat/completions");
    assert!(reqs[0]
        .1
        .iter()
        .any(|(k, v)| k == "authorization" && v == "Bearer test-key"));
    let body = &reqs[0].2;
    assert_eq!(body["max_completion_tokens"], 777);
    assert!(body.get("max_tokens").is_none());
    assert_eq!(body["seed"], 1234);
    assert_eq!(body["reasoning_effort"], "low");
    assert_eq!(out.usage.input_tokens, Some(120));
    assert!((out.usage.cost_usd.unwrap() - (120.0 * 1.0 + 30.0 * 4.0) / 1e6).abs() < 1e-12);
}

#[test]
fn openrouter_mock_endpoint() {
    let mock = start_mock(vec![chat_completion("42", 200, 40, 0)]);
    let env = env_for("openrouter", &mock.base);
    let runner = make_runner("model:openrouter/test-model-x", &prices(), &env).unwrap();
    let t = task();
    let prompt = t.render_prompt();
    let est = runner.estimate_cost_usd(&t, &prompt).unwrap();
    let out = runner.run(&t, &prompt, &ctx());
    assert_eq!(out.output, "42");
    assert!((out.usage.cost_usd.unwrap() - (200.0 * 3.0 + 40.0 * 6.0) / 1e6).abs() < 1e-12);
    assert!(est >= out.usage.cost_usd.unwrap());
    assert_eq!(out.raw["provider"], "openrouter");
}

#[test]
fn xai_and_gemini_shapes() {
    for (provider, model) in [("xai", "grok-test"), ("gemini", "gemini-test")] {
        let mock = start_mock(vec![chat_completion("ok", 10, 5, 0)]);
        let env = env_for(provider, &mock.base);
        let runner = make_runner(
            &format!("model:{provider}/{model},temperature=0.2"),
            &prices(),
            &env,
        )
        .unwrap();
        let t = task();
        let out = runner.run(&t, "q", &ctx());
        assert_eq!(out.output, "ok", "{provider}");
        let reqs = mock.requests.lock().unwrap();
        let body = &reqs[0].2;
        assert_eq!(body["max_tokens"], t.max_output_tokens, "{provider}");
        assert!(body.get("seed").is_none(), "{provider}");
        assert_eq!(body["temperature"], 0.2, "{provider}");
    }
}

#[test]
fn ollama_and_vllm_are_local_free_and_keyless() {
    for provider in ["ollama", "vllm"] {
        let mock = start_mock(vec![chat_completion("hi", 500, 500, 0)]);
        let env = env_for(provider, &mock.base);
        let runner =
            make_runner(&format!("model:{provider}/llama3.2:3b"), &prices(), &env).unwrap();
        assert!(
            runner.problems().is_empty(),
            "{provider}: {:?}",
            runner.problems()
        );
        let t = task();
        let out = runner.run(&t, "q", &ctx());
        assert_eq!(out.usage.cost_usd, Some(0.0), "{provider}");
        assert_eq!(runner.estimate_cost_usd(&t, "q"), Some(0.0), "{provider}");
        assert_eq!(mock.requests.lock().unwrap()[0].0, "/v1/chat/completions");
    }
}

#[test]
fn compatible_provider_uses_its_base_url() {
    let mock = start_mock(vec![chat_completion("ok", 10, 5, 0)]);
    let mut env = env_for("compatible", &mock.base);
    env.remove("KM_COMPATIBLE_BASE_URL");
    let mut table = prices();
    table.prices.insert(
        "compatible/local-model".into(),
        Price {
            input: 1.0,
            output: 1.0,
            cache_read: None,
            cache_write: None,
            long_context_threshold: None,
            long_input: None,
            long_output: None,
            source: None,
            as_of: None,
        },
    );
    let runner = make_runner("model:compatible/local-model", &table, &env).unwrap();
    let out = runner.run(&task(), "q", &ctx());
    assert_eq!(out.output, "ok");
}

#[test]
fn retries_only_429_and_5xx_with_retry_after() {
    let mock = start_mock(vec![
        api_error(429, Some("0")),
        api_error(529, None),
        anthropic_message("14", 100, 50),
    ]);
    let env = env_for("anthropic", &mock.base);
    let runner = make_runner("model:anthropic/claude-opus-5-5,retries=2", &prices(), &env).unwrap();
    let out = runner.run(&task(), "q", &ctx());
    assert_eq!(out.output, "14");
    assert_eq!(out.raw["retries"], 2);
    assert_eq!(mock.requests.lock().unwrap().len(), 3);
    // Retried attempts may have been billed: cost must be unknown.
    assert_eq!(out.usage.cost_usd, None);
}

#[test]
fn client_errors_are_not_retried_and_unpriced_is_refused() {
    let mock = start_mock(vec![api_error(400, None)]);
    let env = env_for("anthropic", &mock.base);
    let runner = make_runner("model:anthropic/claude-opus-5-5", &prices(), &env).unwrap();
    let out = runner.run(&task(), "q", &ctx());
    assert!(out.error.is_some());
    assert_eq!(mock.requests.lock().unwrap().len(), 1);

    let mock2 = start_mock(vec![chat_completion("x", 1, 1, 0)]);
    let env2 = env_for("openai", &mock2.base);
    let unpriced = make_runner("model:openai/gpt-unpriced", &prices(), &env2).unwrap();
    assert_eq!(unpriced.estimate_cost_usd(&task(), "q"), None);
    assert!(unpriced.problems().iter().any(|p| p.contains("no price")));
    let out = unpriced.run(&task(), "q", &ctx());
    assert!(out.error.unwrap_or_default().contains("no price"));
    assert_eq!(mock2.requests.lock().unwrap().len(), 0);
}

#[test]
fn missing_key_is_a_problem_and_bad_specs_error() {
    let env: BTreeMap<String, String> = [(
        "XDG_CONFIG_HOME".to_string(),
        "/tmp/km-no-config".to_string(),
    )]
    .into_iter()
    .collect();
    let runner = make_runner("model:xai/grok-test", &prices(), &env).unwrap();
    assert_eq!(
        runner.problems(),
        vec!["XAI_API_KEY is not set".to_string()]
    );
    for spec in [
        "model:nope/x",
        "model:anthropic/x,effort=extreme",
        "model:anthropic/x,bogus=1",
        "model:anthropic/x,effort",
        "model:anthropic/x,max_tokens=0",
        "model:no-provider",
        "harness:",
    ] {
        assert!(make_runner(spec, &prices(), &env).is_err(), "{spec}");
    }
    let named = make_runner(
        "model:anthropic/claude-opus-5-5,max_tokens=900,effort=high",
        &prices(),
        &env,
    )
    .unwrap();
    assert_eq!(
        named.name,
        "model:anthropic/claude-opus-5-5,effort=high,max_tokens=900"
    );
}

#[test]
fn runner_reuses_connection_and_does_not_replay_a_failed_post() {
    use krutrim_evaluator::runners::RunnerInner;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        for attempt in 0..3 {
            let mut first = String::new();
            assert!(
                reader.read_line(&mut first).unwrap() > 0,
                "connection was not reused"
            );
            assert!(first.starts_with("POST "));
            let mut length = 0;
            loop {
                let mut header = String::new();
                assert!(reader.read_line(&mut header).unwrap() > 0);
                if header.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = header.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            assert!(length > 0);
            if attempt == 1 {
                let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(body["max_completion_tokens"], 13);
                assert_eq!(body["temperature"], 0.2);
            }
            if attempt < 2 {
                let (_, _, response) = chat_completion("pooled", 10, 2, 0);
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}", response.len(), response).unwrap();
                stream.flush().unwrap();
            }
        }
        // The third request was received, but its response was lost. Reusing
        // connections must never turn that into another billable POST.
        drop(reader);
        drop(stream);
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            match listener.accept() {
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                other => panic!("unexpected connection after lost POST response: {other:?}"),
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    let mut runner = make_runner(
        "model:openai/gpt-test,retries=2",
        &prices(),
        &env_for("openai", &base),
    )
    .unwrap();
    let context = RunContext {
        timeout_s: 2.0,
        ..ctx()
    };
    let first = runner.run(&task(), "hello", &context);
    assert_eq!(first.output, "pooled");
    assert!((first.usage.cost_usd.unwrap() - 0.000018).abs() < 1e-12);
    if let RunnerInner::Model(state) = &mut runner.inner {
        state.options.max_tokens = Some(13);
        state.options.temperature = Some(0.2);
        state.price.as_mut().unwrap().input = 2.0;
        state.sleep = Box::new(|_| panic!("A failed POST response must not be retried"));
    }
    let second = runner.run(&task(), "hello again", &context);
    assert_eq!(second.output, "pooled");
    assert!((second.usage.cost_usd.unwrap() - 0.000028).abs() < 1e-12);
    let failed = runner.run(&task(), "lost response", &context);
    assert!(failed
        .error
        .unwrap_or_default()
        .contains("APIConnectionError"));
    assert_eq!(failed.usage.cost_usd, None);
    server.join().unwrap();
}

#[test]
fn runner_refreshes_adapter_when_credentials_or_endpoint_change() {
    use krutrim_evaluator::runners::RunnerInner;

    let first = start_mock(vec![chat_completion("first endpoint", 10, 2, 0)]);
    let second = start_mock(vec![chat_completion("second endpoint", 10, 2, 0)]);
    let mut env = env_for("openai", &first.base);
    let mut runner = make_runner("model:openai/gpt-test", &prices(), &env).unwrap();
    assert_eq!(
        runner.run(&task(), "first", &ctx()).output,
        "first endpoint"
    );

    // The runner uses its captured environment until it is explicitly changed
    // (desktop creates new runners from the next run's current key snapshot).
    env.insert("OPENAI_API_KEY".into(), "outside-snapshot".into());
    assert_eq!(
        runner.run(&task(), "snapshot", &ctx()).output,
        "first endpoint"
    );
    if let RunnerInner::Model(state) = &mut runner.inner {
        state
            .env
            .insert("OPENAI_API_KEY".into(), "rotated-key".into());
        state
            .env
            .insert("KM_OPENAI_BASE_URL".into(), second.base.clone());
    }
    assert_eq!(
        runner.run(&task(), "changed", &ctx()).output,
        "second endpoint"
    );
    for (_, headers, _) in first.requests.lock().unwrap().iter() {
        assert!(headers
            .iter()
            .any(|(name, value)| name == "authorization" && value == "Bearer test-key"));
    }
    let captured = second.requests.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert!(captured[0]
        .1
        .iter()
        .any(|(name, value)| name == "authorization" && value == "Bearer rotated-key"));
}
