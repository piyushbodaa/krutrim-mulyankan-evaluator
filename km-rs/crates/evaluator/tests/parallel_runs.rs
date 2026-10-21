//! Parallel runs against a threaded loopback mock: the same stored results,
//! in the same order, as a one-at-a-time run; real overlap within per-model
//! limits; cancel; and rate limits that are waited out at a known cost.

use krutrim_evaluator::pricing::{Price, PriceTable};
use krutrim_evaluator::{
    load_suite, make_runner, run_suite_with, RunCallbacks, Runner, Settings, Store, Suite,
};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Mock {
    base: String,
    requests: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    peak_per_model: Arc<Mutex<BTreeMap<String, usize>>>,
}

/// An OpenAI-style chat API. "good" answers 7×n in a sentence, "bad" is off
/// by one, and each answer takes a different time so they finish out of
/// order. With `rate_limit_first`, the very first request gets a 429.
fn start_mock(rate_limit_first: bool) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(AtomicUsize::new(0));
    let busy = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let busy_per_model: Arc<Mutex<BTreeMap<String, usize>>> = Arc::default();
    let peak_per_model: Arc<Mutex<BTreeMap<String, usize>>> = Arc::default();
    let (requests2, peak2, peak_per_model2) =
        (requests.clone(), peak.clone(), peak_per_model.clone());
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut stream) = conn else { continue };
            let (requests, busy, peak) = (requests2.clone(), busy.clone(), peak2.clone());
            let (busy_per_model, peak_per_model) =
                (busy_per_model.clone(), peak_per_model2.clone());
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let mut length = 0usize;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = header.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("content-length") {
                            length = v.trim().parse().unwrap_or(0);
                        }
                    }
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let model = request["model"].as_str().unwrap_or_default().to_string();
                let prompt = request["messages"][0]["content"]
                    .as_str()
                    .unwrap_or_default();
                let n: i64 = prompt
                    .split(|c: char| !c.is_ascii_digit())
                    .rfind(|s| !s.is_empty())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let first = requests.fetch_add(1, Ordering::SeqCst) == 0;
                let now = busy.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                {
                    let mut per = busy_per_model.lock().unwrap();
                    let count = per.entry(model.clone()).or_default();
                    *count += 1;
                    let mut peaks = peak_per_model.lock().unwrap();
                    let top = peaks.entry(model.clone()).or_default();
                    *top = (*top).max(*count);
                }
                std::thread::sleep(Duration::from_millis(40 + (n as u64 * 37) % 110));
                busy.fetch_sub(1, Ordering::SeqCst);
                *busy_per_model.lock().unwrap().get_mut(&model).unwrap() -= 1;
                let (status, text) = if rate_limit_first && first {
                    (429, r#"{"error":{"message":"slow down"}}"#.to_string())
                } else {
                    let content = if model.starts_with("good") {
                        format!("Let me work it out. The answer is **{}**.", 7 * n)
                    } else {
                        format!("I think it is {}", 7 * n + 1)
                    };
                    (
                        200,
                        serde_json::json!({
                            "id": "cmpl", "model": model,
                            "choices": [{"index": 0, "finish_reason": "stop",
                                         "message": {"role": "assistant", "content": content}}],
                            "usage": {"prompt_tokens": 12, "completion_tokens": 9}
                        })
                        .to_string(),
                    )
                };
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nRetry-After: 0\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                    text.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(text.as_bytes());
            });
        }
    });
    Mock {
        base,
        requests,
        peak,
        peak_per_model,
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("km-parallel-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn suite(dir: &Path, questions: i64) -> Suite {
    let mut yaml =
        String::from("name: parallel-probe\nversion: '1'\ndescription: Parallel probe\ntasks:\n");
    for n in 1..=questions {
        yaml += &format!(
            "  - {{id: Q{n:02}, version: 1, category: c, prompt: 'Question {n}: what is 7 times {n}?', \
             max_output_tokens: 64, grader: {{type: answer, check: number, expected: '{}'}}}}\n",
            7 * n
        );
    }
    let path = dir.join("parallel-probe.yaml");
    std::fs::write(&path, yaml).unwrap();
    load_suite(&path).unwrap()
}

fn runners(mock: &Mock, specs: &[&str]) -> Vec<Runner> {
    let price = Price {
        input: 1.0,
        output: 1.0,
        cache_read: None,
        cache_write: None,
        long_context_threshold: None,
        long_input: None,
        long_output: None,
        source: None,
        as_of: None,
    };
    let mut prices = PriceTable::default();
    for model in ["good", "bad", "good:free"] {
        prices
            .prices
            .insert(format!("compatible/{model}"), price.clone());
    }
    let env: BTreeMap<String, String> = [
        ("COMPATIBLE_BASE_URL".to_string(), mock.base.clone()),
        ("COMPATIBLE_API_KEY".to_string(), "test-key".to_string()),
    ]
    .into_iter()
    .collect();
    specs
        .iter()
        .map(|spec| make_runner(spec, &prices, &env).unwrap())
        .collect()
}

fn settings(dir: &Path) -> Settings {
    Settings {
        db_path: dir.join("results.db"),
        suites_dir: dir.to_path_buf(),
        prices_path: dir.join("no-prices.yaml"),
        max_usd: 5.0,
        usd_to_inr: 88.0,
        runs_dir: dir.join("runs"),
    }
}

type Row = (String, String, i64, String, Option<bool>, Option<String>);

fn rows(store: &Store, run_id: &str) -> Vec<Row> {
    store
        .results_for_run(run_id)
        .unwrap()
        .into_iter()
        .map(|r| {
            (
                r.task_id,
                r.runner,
                r.repeat_index,
                r.status,
                r.passed,
                r.output,
            )
        })
        .collect()
}

fn run(
    dir: &Path,
    concurrency: usize,
    specs: &[&str],
    questions: i64,
) -> (Vec<Row>, Duration, Mock) {
    let mock = start_mock(false);
    let suite = suite(dir, questions);
    let runners = runners(&mock, specs);
    let settings = settings(dir);
    let store = Store::open(&settings.db_path).unwrap();
    let started = Instant::now();
    let summary = run_suite_with(
        &suite,
        &runners,
        &store,
        &settings,
        0,
        1,
        Some(5.0),
        None,
        &RunCallbacks::default(),
        None,
        concurrency,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let took = started.elapsed();
    assert_eq!(summary.status, "completed");
    (rows(&store, &summary.run_id), took, mock)
}

#[test]
fn parallel_runs_store_what_one_at_a_time_runs_store_in_the_same_order() {
    let specs = ["oracle", "model:compatible/good", "model:compatible/bad"];
    let (one, slow, _) = run(&scratch("one"), 1, &specs, 12);
    let (many, fast, mock) = run(&scratch("many"), 8, &specs, 12);
    eprintln!("12 questions x 3 AIs: one at a time {slow:?}, parallel {fast:?}");
    assert_eq!(one.len(), 36);
    assert_eq!(one, many);
    // Every "good" answer is marked right and every "bad" one wrong.
    for (_, runner, _, status, passed, _) in &many {
        assert_eq!(status, "ok");
        let right = runner != "model:compatible/bad";
        assert_eq!(*passed, Some(right), "{runner}");
    }
    assert!(
        fast * 2 < slow,
        "parallel {fast:?} should be well under half of one-at-a-time {slow:?}"
    );
    assert!(mock.peak.load(Ordering::SeqCst) >= 4, "calls overlapped");
    assert!(mock.peak.load(Ordering::SeqCst) <= 8);
    for (model, top) in mock.peak_per_model.lock().unwrap().iter() {
        assert!(*top <= 4, "{model} had {top} calls at once");
    }
}

#[test]
fn free_models_get_at_most_two_calls_at_once() {
    let (rows, _, mock) = run(&scratch("free"), 8, &["model:compatible/good:free"], 8);
    assert_eq!(rows.len(), 8);
    assert!(rows.iter().all(|r| r.4 == Some(true)));
    assert_eq!(mock.peak_per_model.lock().unwrap()["good:free"], 2);
}

#[test]
fn cancelling_a_parallel_run_records_the_rest_as_skipped_in_order() {
    let dir = scratch("cancel");
    let mock = start_mock(false);
    let suite = suite(&dir, 10);
    let runners = runners(&mock, &["model:compatible/good", "model:compatible/bad"]);
    let settings = settings(&dir);
    let store = Store::open(&settings.db_path).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let seen = AtomicUsize::new(0);
    let stop = cancel.clone();
    let on_result = move |_: &krutrim_evaluator::ResultRecord| {
        if seen.fetch_add(1, Ordering::SeqCst) + 1 == 3 {
            stop.store(true, Ordering::SeqCst);
        }
    };
    let callbacks = RunCallbacks {
        on_result: Some(&on_result),
        ..Default::default()
    };
    let summary = run_suite_with(
        &suite,
        &runners,
        &store,
        &settings,
        0,
        1,
        Some(5.0),
        None,
        &callbacks,
        Some(cancel),
        8,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(summary.status, "cancelled");
    let stored = rows(&store, &summary.run_id);
    assert_eq!(stored.len(), 20, "every attempt is recorded");
    let order: Vec<(String, String)> = stored.iter().map(|r| (r.0.clone(), r.1.clone())).collect();
    let mut expected = Vec::new();
    for n in 1..=10 {
        for model in ["good", "bad"] {
            expected.push((format!("Q{n:02}"), format!("model:compatible/{model}")));
        }
    }
    assert_eq!(order, expected);
    let skipped = stored.iter().filter(|r| r.3 == "skipped").count();
    assert!(skipped > 0, "attempts after the cancel were not called");
    assert_eq!(
        mock.requests.load(Ordering::SeqCst),
        20 - skipped,
        "only attempts that ran reached the API"
    );
}

#[test]
fn rate_limits_are_waited_out_and_the_cost_stays_known() {
    let dir = scratch("ratelimit");
    let mock = start_mock(true);
    let suite = suite(&dir, 1);
    let runners = runners(&mock, &["model:compatible/good"]);
    let settings = settings(&dir);
    let store = Store::open(&settings.db_path).unwrap();
    let summary = run_suite_with(
        &suite,
        &runners,
        &store,
        &settings,
        0,
        1,
        Some(5.0),
        None,
        &RunCallbacks::default(),
        None,
        8,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let record = &store.results_for_run(&summary.run_id).unwrap()[0];
    assert_eq!(record.status, "ok");
    assert_eq!(record.passed, Some(true));
    assert_eq!(mock.requests.load(Ordering::SeqCst), 2);
    assert!(record.cost_usd.is_some(), "a refused 429 was not billed");
}
