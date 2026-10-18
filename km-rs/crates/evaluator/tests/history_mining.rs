//! Rust port parity for history parsing, mining and micro-eval building.
//! Uses the same synthetic fixtures as tests/test_history_mining.py.

use krutrim_evaluator::graders::grade_with_judge;
use krutrim_evaluator::history::{load_history, PromptPair};
use krutrim_evaluator::microeval::{build_suite, write_micro_eval};
use krutrim_evaluator::miner::{mine_failures, parse_verdict};
use krutrim_evaluator::suite::load_suite;
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/history")
}

fn all_pairs() -> Vec<PromptPair> {
    let f = fixtures();
    load_history(
        &[
            f.join("chatgpt-conversations.json"),
            f.join("claude-conversations.json"),
            f.join("claude-code-session.jsonl"),
        ],
        "auto",
    )
    .expect("fixtures parse")
    .pairs()
}

fn fake_judge(prompt: &str) -> Result<String, String> {
    let reply = |v: &str, c: f64, r: &str, e: &str, ru: &str| {
        serde_json::json!({"verdict": v, "confidence": c, "reason": r, "expected": e, "rubric": ru})
            .to_string()
    };
    Ok(if prompt.contains("GST") {
        reply(
            "wrong",
            0.95,
            "GST applied twice",
            "50150",
            "Must equal 50150.",
        )
    } else if prompt.contains("RLWL") {
        reply(
            "wrong",
            0.9,
            "Waitlisted tickets cannot board",
            "No. A waitlisted RLWL e-ticket holder cannot board.",
            "Must say the passenger cannot board.",
        )
    } else if prompt.contains("sum(x*x") {
        reply("wrong", 0.6, "Wrong value", "14", "Must equal 14.")
    } else if prompt.contains("poem") {
        reply("unclear", 0.9, "No checkable answer", "", "")
    } else {
        reply("correct", 0.9, "", "", "")
    })
}

#[test]
fn parses_all_three_formats() {
    let pairs = all_pairs();
    assert_eq!(pairs.len(), 5);
    assert_eq!(pairs[0].answer, "59177"); // current ChatGPT branch, not regeneration
    assert_eq!(pairs[0].model.as_deref(), Some("gpt-test"));
    assert!(pairs
        .iter()
        .any(|p| p.source == "claude_code" && p.answer == "30"));
}

#[test]
fn mine_keeps_only_confident_wrong() {
    let result = mine_failures(
        &all_pairs(),
        &fake_judge,
        "ollama/fake",
        0.7,
        None,
        None,
        None,
    )
    .expect("mining runs");
    assert_eq!(result.checked, 5);
    assert_eq!(result.correct, 1);
    assert_eq!(result.unclear, 1);
    assert_eq!(result.below_threshold, 1);
    assert_eq!(result.failures.len(), 2);
    assert_eq!(result.failures[0].verdict.expected, "50150");
}

#[test]
fn parse_verdict_rejects_garbage() {
    assert!(parse_verdict("no json").is_err());
    assert!(parse_verdict(r#"{"verdict":"maybe","confidence":0.5}"#).is_err());
    assert!(parse_verdict(r#"{"verdict":"wrong","confidence":2}"#).is_err());
}

#[test]
fn micro_eval_roundtrip_and_grading() {
    let failures = mine_failures(
        &all_pairs(),
        &fake_judge,
        "ollama/fake",
        0.7,
        None,
        None,
        None,
    )
    .expect("mining runs")
    .failures;
    let suite =
        build_suite(&failures, "mine-test", "2026-10-18", "ollama/fake").expect("suite builds");
    assert_eq!(suite.tasks.len(), 2);
    assert_eq!(
        suite.tasks[0].render_prompt(),
        "Add 18% GST to 42500. Number only."
    );
    assert_eq!(suite.tasks[0].reference.as_deref(), Some("50150"));

    // The built suite is gradeable: reference answer passes, echo fails.
    let grading = |prompt: &str| -> Result<String, String> {
        let score = if prompt.contains("<response>\n50150\n</response>") {
            10
        } else {
            1
        };
        Ok(serde_json::json!({"score": score, "reason": "fake"}).to_string())
    };
    let dir = std::env::temp_dir();
    let good = grade_with_judge(&suite.tasks[0], "50150", &dir, Some(&grading));
    assert_eq!(good.passed, Some(true));
    let echo = grade_with_judge(
        &suite.tasks[0],
        "Add 18% GST to 42500. Number only.",
        &dir,
        Some(&grading),
    );
    assert_eq!(echo.passed, Some(false));
    // And with no judge it stays pending, exactly like before.
    let pending = krutrim_evaluator::graders::grade(&suite.tasks[0], "50150", &dir);
    assert_eq!(pending.status, "pending");
}

#[test]
fn write_micro_eval_roundtrip() {
    let failures = mine_failures(
        &all_pairs(),
        &fake_judge,
        "ollama/fake",
        0.7,
        None,
        None,
        None,
    )
    .expect("mining runs")
    .failures;
    let dir = std::env::temp_dir().join(format!("km-micro-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("mine.yaml");
    let (_, evidence) =
        write_micro_eval(&failures, &dest, "mine-test", "1", "ollama/fake").expect("writes");
    assert!(evidence.is_file());
    assert_eq!(load_suite(&dest).expect("suite loads").tasks.len(), 2);
    assert!(
        write_micro_eval(&failures, &dest, "mine-test", "1", "ollama/fake").is_err(),
        "must refuse to overwrite"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn golden_micro_eval_matches() {
    use krutrim_evaluator::miner::judge_prompt_hash;
    let golden: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("expected-micro-eval.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        judge_prompt_hash(),
        golden["judge_prompt_hash"].as_str().unwrap()
    );
    let failures = mine_failures(
        &all_pairs(),
        &fake_judge,
        "ollama/fake",
        0.7,
        None,
        None,
        None,
    )
    .expect("mining runs")
    .failures;
    let suite = build_suite(&failures, "mine-test", "1", "ollama/fake").unwrap();
    let tasks = golden["tasks"].as_array().unwrap();
    assert_eq!(suite.tasks.len(), tasks.len());
    for (task, want) in suite.tasks.iter().zip(tasks) {
        assert_eq!(task.id, want["id"].as_str().unwrap());
        assert_eq!(task.render_prompt(), want["prompt"].as_str().unwrap());
        assert_eq!(task.reference.as_deref(), want["reference"].as_str());
        assert_eq!(task.rubric.as_deref(), want["rubric"].as_str());
    }
}

#[test]
fn secrets_redacted_before_judging() {
    use krutrim_evaluator::miner::redact_secrets;
    let fake_key = ["sk", "-ant-", "abcdef12345678"].concat();
    assert!(!redact_secrets(&format!("key {fake_key} end")).contains(&fake_key));
    assert!(!redact_secrets("api_key=supersecretvalue99 x").contains("supersecretvalue99"));
}

#[test]
fn select_dedupes_oversizes_and_samples_evenly() {
    use krutrim_evaluator::miner::{select_pairs, MAX_PAIR_CHARS};
    let base = all_pairs();
    let mut many = base.clone();
    many.push(base[0].clone());
    let mut big = base[0].clone();
    big.prompt = "x".repeat(MAX_PAIR_CHARS + 1);
    many.push(big);
    let selection = select_pairs(&many, Some(2));
    assert_eq!(selection.duplicates, 1);
    assert_eq!(selection.oversized, 1);
    assert_eq!(selection.pairs.len(), 2);
    assert_eq!(selection.pairs[0].prompt, base[0].prompt);
    assert_ne!(selection.pairs[1].prompt, base[1].prompt);
}

#[test]
fn verdict_cache_avoids_second_judge_call() {
    use krutrim_evaluator::miner::VerdictCache;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let counting = |prompt: &str| -> Result<String, String> {
        CALLS.fetch_add(1, Ordering::SeqCst);
        fake_judge(prompt)
    };
    let path = std::env::temp_dir().join(format!("km-cache-{}.jsonl", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut cache = VerdictCache::load(path.clone());
    let first = mine_failures(
        &all_pairs(),
        &counting,
        "ollama/fake",
        0.7,
        None,
        None,
        Some(&mut cache),
    )
    .unwrap();
    assert_eq!(first.cache_hits, 0);
    let after_first = CALLS.load(Ordering::SeqCst);
    let mut reloaded = VerdictCache::load(path.clone());
    let second = mine_failures(
        &all_pairs(),
        &counting,
        "ollama/fake",
        0.7,
        None,
        None,
        Some(&mut reloaded),
    )
    .unwrap();
    assert_eq!(CALLS.load(Ordering::SeqCst), after_first);
    assert_eq!(second.cache_hits, 5);
    assert_eq!(second.failures.len(), 2);
    let _ = std::fs::remove_file(&path);
}
