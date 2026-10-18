//! km — Krutrim Mulyankan command line (Rust port of the Python Typer CLI).
//! Same commands, flags, suite YAML and artifacts; see RUNNER notes in the
//! evaluator crate for the parts that are not ported yet.

use krutrim_evaluator::outputs::write_outputs;
use krutrim_evaluator::report::{
    leaderboard_cells, verdict_cells, DEFINITIONS, LEADERBOARD_HEADERS, VERDICT_HEADERS,
};
use krutrim_evaluator::*;
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

mod keys;
mod tui;

static NO_ANIM: AtomicBool = AtomicBool::new(false);
static USE_COLOR: AtomicBool = AtomicBool::new(false);

/// Interactive terminal session (stdin and stdout on a real, non-dumb TTY).
/// Everything polished lives behind this; pipes/scripts keep classic output.
fn interactive() -> bool {
    std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(false)
}

fn color_enabled() -> bool {
    USE_COLOR.load(Ordering::Relaxed)
}

fn anim_enabled() -> bool {
    !NO_ANIM.load(Ordering::Relaxed)
}

fn main() -> ExitCode {
    load_dotenv();
    keys::load_keys_file();
    // --no-anim is a global presentation switch; it is accepted anywhere
    // and simply removed before the classic parser sees the arguments.
    if std::env::args().any(|a| a == "--no-anim") {
        NO_ANIM.store(true, Ordering::Relaxed);
    }
    if std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var("TERM").map(|t| t != "dumb").unwrap_or(false)
    {
        USE_COLOR.store(true, Ordering::Relaxed);
    }
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| a != "--no-anim")
        .collect();
    match dispatch(&args) {
        Ok(code) => ExitCode::from(code),
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::from(2)
        }
    }
}

fn dispatch(args: &[String]) -> Result<u8, String> {
    let Some(cmd) = args.first() else {
        if interactive() {
            let outcome = tui::run(
                tui::UiOptions {
                    animate: anim_enabled(),
                    color: color_enabled(),
                    help_text: main_help(),
                },
                settings(),
            );
            return match outcome {
                tui::Outcome::Quit => Ok(0),
                tui::Outcome::OpenUi => cmd_ui(&[]),
            };
        }
        println!("{}", boxed(&main_help()));
        return Ok(0);
    };
    match cmd.as_str() {
        "--help" | "-h" | "help" => {
            println!("{}", boxed(&main_help()));
            Ok(0)
        }
        "list" => cmd_list(&args[1..]),
        "history" => cmd_history(&args[1..]),
        "doctor" => cmd_doctor(&args[1..]),
        "micro-eval" => cmd_micro_eval(&args[1..]),
        "run" => cmd_run(&args[1..]),
        "import-suite" => cmd_import(&args[1..]),
        "ui" => cmd_ui(&args[1..]),
        "export" => cmd_export(&args[1..]),
        "report" => cmd_report(&args[1..]),
        other => Err(format!("No such command '{other}'.\n\n{}", main_help())),
    }
}

fn boxed(text: &str) -> String {
    // Cheap echo of Rich's rounded panels: one border around the help body.
    let lines: Vec<&str> = text.trim_end().lines().collect();
    let width = lines.iter().map(|l| display_width(l)).max().unwrap_or(0);
    let inner = width;
    let mut out = vec![format!("╭{}╮", "─".repeat(inner + 2))];
    for line in &lines {
        if display_width(line) <= inner {
            out.push(format!("│ {} │", pad(line, inner)));
        } else {
            for chunk in wrap_cell(line, inner) {
                out.push(format!("│ {} │", pad(&chunk, inner)));
            }
        }
    }
    out.push(format!("╰{}╯", "─".repeat(inner + 2)));
    out.join("\n")
}

fn main_help() -> String {
    "Usage: km [OPTIONS] COMMAND [ARGS]...\n\nLocal tests for AI models, harnesses and agents.\n\nOptions:\n  --install-completion  Install completion for the current shell.\n  --show-completion     Show completion for the current shell, to copy or customize the installation.\n  --help                Show this message and exit.\n\nCommands:\n  history       Inspect your ChatGPT/Claude/Claude Code history offline (counts only).\n  doctor        Check suites, prices, keys and the local judge path.\n  micro-eval    Mine prompts your AI answered wrongly into a personal suite.\n  list          List suites, runners, or past runs: km list suites|runners|runs\n  run           Run SUITE against the given runners, e.g. km run suites/example.yaml -r oracle\n  import-suite  Import a foreign suite directory into Mulyankan YAML.\n  ui            Open a local pilot workbench with task selection and free mock checks.\n  export        Write the schema-1.1 JSON export (default: latest run). Outputs are omitted unless --include-outputs is explicit.\n  report        Print the leaderboard for RUN_ID (default: latest run).\n".into()
}

fn load_dotenv() {
    let Ok(text) = std::fs::read_to_string(".env") else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let (k, v) = (k.trim(), v.trim().trim_matches(['"', '\'']));
            if std::env::var_os(k).is_none() {
                std::env::set_var(k, v);
            }
        }
    }
}

fn settings() -> Settings {
    Settings::from_env()
}

fn suites_dir(s: &Settings) -> PathBuf {
    effective_suites_dir(s)
}

fn err1(msg: impl std::fmt::Display) -> u8 {
    eprintln!("error: {msg}");
    1
}

// ---------------- list ----------------

fn cmd_list(args: &[String]) -> Result<u8, String> {
    let what = match args.first().map(|s| s.as_str()) {
        None | Some("--help") => {
            println!("Usage: km list [OPTIONS] WHAT\n\nList suites, runners, or past runs: km list suites|runners|runs\n\nArguments:\n  WHAT  [required]\n");
            return Ok(if args.is_empty() { 2 } else { 0 });
        }
        Some(w) => w,
    };
    let s = settings();
    match what {
        "suites" => {
            let dir = suites_dir(&s);
            let paths = discover_suites(&dir);
            if paths.is_empty() {
                let shown = repr_path(&dir);
                println!("No suites found in {shown}.");
                return Ok(0);
            }
            let mut rows = Vec::new();
            for p in paths {
                match load_suite(&p) {
                    Ok(suite) => rows.push(vec![
                        suite.name,
                        suite.version,
                        suite.validation_status,
                        suite.tasks.len().to_string(),
                        p.to_string_lossy().into_owned(),
                    ]),
                    Err(e) => {
                        let stem = p.file_stem().and_then(|x| x.to_str()).unwrap_or("?");
                        rows.push(vec![
                            stem.to_string(),
                            "?".into(),
                            "error".into(),
                            "?".into(),
                            truncate(&e, 80),
                        ]);
                    }
                }
            }
            println!(
                "{}",
                render_table(
                    Some("Suites"),
                    &["Name", "Version", "Status", "Tasks", "Path"],
                    &rows
                )
            );
            Ok(0)
        }
        "runners" => {
            let rows: Vec<Vec<String>> = RUNNER_CATALOG
                .iter()
                .map(|(spec, kind, done, desc)| {
                    let desc = desc.replace("\\\n", " ");
                    vec![
                        spec.to_string(),
                        kind.to_string(),
                        if *done {
                            "yes".into()
                        } else {
                            "planned".into()
                        },
                        desc,
                    ]
                })
                .collect();
            println!(
                "{}",
                render_table(
                    Some("Runners"),
                    &["Runner spec", "Kind", "Ready", "What it is"],
                    &rows
                )
            );
            Ok(0)
        }
        "runs" => {
            let store = match Store::open(&s.db_path) {
                Ok(st) => st,
                Err(e) => return Ok(err1(e)),
            };
            match store.list_runs(20) {
                Ok(runs) if runs.is_empty() => {
                    println!("No runs yet.");
                    Ok(0)
                }
                Ok(runs) => {
                    let rows: Vec<Vec<String>> = runs
                        .iter()
                        .map(|r| {
                            vec![
                                r.id.clone(),
                                r.suite_name.clone(),
                                r.status.clone(),
                                r.runners.join(", "),
                                format!("${:.4}", r.spent_usd),
                                r.started_at.clone(),
                            ]
                        })
                        .collect();
                    println!(
                        "{}",
                        render_table(
                            Some("Runs"),
                            &["Run", "Suite", "Status", "Runners", "Cost ↓", "Started"],
                            &rows
                        )
                    );
                    Ok(0)
                }
                Err(e) => Ok(err1(e)),
            }
        }
        other => Ok(err1(format!(
            "list what? Try: suites, runners, runs (not '{other}')"
        ))),
    }
}

fn repr_path(p: &Path) -> String {
    format!("'{}'", p.display())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + "…"
    }
}

// ---------------- history / micro-eval ----------------

type HistoryArgs = Option<(Vec<PathBuf>, String, Vec<String>)>;

fn parse_history_args(args: &[String], help: &str) -> Result<HistoryArgs, u8> {
    // Split positionals from --source; the rest belongs to the caller's parser.
    let mut paths = Vec::new();
    let mut source = "auto".to_string();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" => {
                println!("{help}");
                return Ok(None);
            }
            "--source" => {
                i += 1;
                source = args
                    .get(i)
                    .cloned()
                    .ok_or_else(|| err1("expected a value after --source"))?;
            }
            other if other.starts_with("--source=") => {
                source = other["--source=".len()..].to_string()
            }
            other if !other.starts_with('-') && rest.is_empty() && !other.contains('=') => {
                paths.push(PathBuf::from(other))
            }
            other => rest.push(other.to_string()),
        }
        i += 1;
    }
    if paths.is_empty() {
        eprintln!("error: Missing argument 'PATHS...'.");
        return Err(2);
    }
    Ok(Some((paths, source, rest)))
}

fn cmd_history(args: &[String]) -> Result<u8, String> {
    let help = "Usage: km history [--source auto|chatgpt|claude|claude-code] PATHS...\n\n\
        Inspect your own chat history exports offline. Counts only; prints no content.\n";
    let Some((paths, source, rest)) = (match parse_history_args(args, help) {
        Ok(v) => v,
        Err(code) => return Ok(code),
    }) else {
        return Ok(0);
    };
    if !rest.is_empty() {
        return Ok(err1(format!("No such option: {}", rest[0])));
    }
    let history = match krutrim_evaluator::history::load_history(&paths, &source) {
        Ok(h) => h,
        Err(e) => return Ok(err1(e)),
    };
    let pairs = history.pairs();
    let mut rows = Vec::new();
    for (src, count) in history.by_source() {
        let pair_count = pairs.iter().filter(|p| p.source == src).count();
        rows.push(vec![src, count.to_string(), pair_count.to_string()]);
    }
    println!(
        "{}",
        render_table(
            None,
            &["Source", "Conversations", "Prompts with an answer"],
            &rows
        )
    );
    println!(
        "Total: {} conversations, {} prompts. Nothing was uploaded or called.",
        history.conversations.len(),
        pairs.len()
    );
    Ok(0)
}

fn print_failure_breakdown(failures: &[krutrim_evaluator::miner::MinedFailure]) {
    if failures.is_empty() {
        return;
    }
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut add =
        |label: String, key: &dyn Fn(&krutrim_evaluator::miner::MinedFailure) -> String| {
            let mut counts: Vec<(String, usize)> = Vec::new();
            for f in failures {
                let k = key(f);
                match counts.iter_mut().find(|(s, _)| *s == k) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((k, 1)),
                }
            }
            counts.sort_by_key(|x| std::cmp::Reverse(x.1));
            for (k, n) in counts {
                rows.push(vec![format!("{label}: {k}"), n.to_string()]);
            }
        };
    add("source".into(), &|f| f.pair.source.clone());
    add("original model".into(), &|f| {
        f.pair
            .model
            .clone()
            .unwrap_or_else(|| "unknown model".into())
    });
    println!(
        "{}",
        render_table(
            Some("Where your AI went wrong"),
            &["Failures by", "Count"],
            &rows
        )
    );
}

fn cmd_doctor(_args: &[String]) -> Result<u8, String> {
    let s = settings();
    let env_map: BTreeMap<String, String> = std::env::vars().collect();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut hard_fail = false;
    let dir = suites_dir(&s);
    let paths = krutrim_evaluator::discover_suites(&dir);
    let mut loaded = 0;
    for path in &paths {
        match load_suite(path) {
            Ok(_) => loaded += 1,
            Err(e) => {
                rows.push(vec![
                    format!(
                        "suite {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    format!("BROKEN: {e}"),
                ]);
                hard_fail = true;
            }
        }
    }
    rows.push(vec![
        "suites".into(),
        format!("{loaded}/{} load from {}/", paths.len(), dir.display()),
    ]);
    match load_prices(Some(&s.prices_path)) {
        Ok(prices) => rows.push(vec![
            "prices".into(),
            format!("{} priced models", prices.prices.len()),
        ]),
        Err(e) => {
            rows.push(vec!["prices".into(), format!("BROKEN: {e}")]);
            hard_fail = true;
        }
    }
    let (mut ready, mut missing) = (Vec::new(), Vec::new());
    for provider in [
        "anthropic",
        "openai",
        "xai",
        "gemini",
        "openrouter",
        "ollama",
        "vllm",
        "compatible",
    ] {
        if krutrim_evaluator::providers::credentials_problem(provider, &env_map).is_some() {
            missing.push(provider);
        } else {
            ready.push(provider);
        }
    }
    rows.push(vec![
        "providers ready".into(),
        if ready.is_empty() {
            "none".into()
        } else {
            ready.join(", ")
        },
    ]);
    rows.push(vec![
        "providers missing keys".into(),
        if missing.is_empty() {
            "none".into()
        } else {
            missing.join(", ")
        },
    ]);
    match env_map.get("MJ_JUDGE") {
        Some(spec) => {
            let problems = krutrim_evaluator::judge::judge_problems(spec, &env_map);
            rows.push(vec![
                "MJ_JUDGE".into(),
                if problems.is_empty() {
                    spec.clone()
                } else {
                    format!("{spec} — {}", problems.join("; "))
                },
            ]);
        }
        None => rows.push(vec![
            "MJ_JUDGE".into(),
            "not set (pass --judge when mining/running)".into(),
        ]),
    }
    let ollama = std::net::TcpStream::connect_timeout(
        &"127.0.0.1:11434".parse().expect("valid address"),
        std::time::Duration::from_secs(2),
    )
    .is_ok();
    rows.push(vec![
        "ollama".into(),
        if ollama {
            "reachable at localhost:11434 — free local judge".into()
        } else {
            "not reachable at localhost:11434 (fine if unused)".into()
        },
    ]);
    println!(
        "{}",
        render_table(Some("km doctor"), &["Check", "Status"], &rows)
    );
    Ok(if hard_fail {
        err1("doctor found broken suites/prices above")
    } else {
        0
    })
}

fn cmd_micro_eval(args: &[String]) -> Result<u8, String> {
    let help = "Usage: km micro-eval [OPTIONS] PATHS...\n\n\
        Mine prompts your AI answered wrongly and build them into a personal suite.\n\n\
        Options:\n  \
        --judge TEXT            Mining judge as <provider>/<model> (default MJ_JUDGE).\n  \
        --out, -o PATH          Suite YAML to create  [default: suites/my-micro-eval.yaml]\n  \
        --name TEXT             Suite name  [default: my-micro-eval]\n  \
        --version TEXT          Suite version (default: today)\n  \
        --source TEXT           auto|chatgpt|claude|claude-code  [default: auto]\n  \
        --limit INT             Judge at most this many prompts.\n  \
        --min-confidence FLOAT  Keep wrong verdicts >= this  [default: 0.7]\n  \
        --max-usd FLOAT         Hard budget cap for judging (default MJ_MAX_USD)\n  \
        --dry-run               Parse and price judging; calls nothing.\n  \
        --allow-paid            Explicitly permit paid judge calls within --max-usd.\n";
    if args.iter().any(|a| a == "--help") {
        println!("{help}");
        return Ok(0);
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut judge_spec: Option<String> = None;
    let mut out = PathBuf::from("suites/my-micro-eval.yaml");
    let mut name = "my-micro-eval".to_string();
    let mut version: Option<String> = None;
    let mut source = "auto".to_string();
    let mut limit: Option<usize> = None;
    let mut min_confidence = 0.7f64;
    let mut max_usd: Option<f64> = None;
    let (mut dry_run, mut allow_paid) = (false, false);
    let mut cache_path = PathBuf::from("runs/judge-cache.jsonl");
    let mut no_cache = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].clone();
        let take = |i: &mut usize| -> Result<String, u8> {
            *i += 1;
            args.get(*i)
                .cloned()
                .ok_or_else(|| err1("expected a value after the flag"))
        };
        macro_rules! take {
            () => {
                match take(&mut i) {
                    Ok(v) => v,
                    Err(code) => return Ok(code),
                }
            };
        }
        macro_rules! parsed {
            ($e:expr, $msg:expr) => {
                match $e {
                    Ok(v) => v,
                    Err(_) => return Ok(err1($msg)),
                }
            };
        }
        match a.as_str() {
            "--dry-run" => dry_run = true,
            "--allow-paid" => allow_paid = true,
            "--no-cache" => no_cache = true,
            "--cache" => cache_path = PathBuf::from(take!()),
            "--judge" => judge_spec = Some(take!()),
            "-o" | "--out" => out = PathBuf::from(take!()),
            "--name" => name = take!(),
            "--version" => version = Some(take!()),
            "--source" => source = take!(),
            "--limit" => limit = Some(parsed!(take!().parse(), "--limit needs an integer")),
            "--min-confidence" => {
                min_confidence = parsed!(take!().parse(), "--min-confidence needs a number")
            }
            "--max-usd" => max_usd = Some(parsed!(take!().parse(), "--max-usd needs a number")),
            other if other.starts_with("--judge=") => judge_spec = Some(other[8..].to_string()),
            other if other.starts_with("--cache=") => cache_path = PathBuf::from(&other[8..]),
            other if other.starts_with("--out=") => out = PathBuf::from(&other[6..]),
            other if other.starts_with("--name=") => name = other[7..].to_string(),
            other if other.starts_with("--version=") => version = Some(other[10..].to_string()),
            other if other.starts_with("--source=") => source = other[9..].to_string(),
            other if other.starts_with("--limit=") => {
                limit = Some(parsed!(other[8..].parse(), "--limit needs an integer"))
            }
            other if other.starts_with("--min-confidence=") => {
                min_confidence = parsed!(other[17..].parse(), "--min-confidence needs a number")
            }
            other if other.starts_with("--max-usd=") => {
                max_usd = Some(parsed!(other[10..].parse(), "--max-usd needs a number"))
            }
            other if other.starts_with('-') => return Ok(err1(format!("No such option: {other}"))),
            other => paths.push(PathBuf::from(other)),
        }
        i += 1;
    }
    if paths.is_empty() {
        eprintln!("error: Missing argument 'PATHS...'.");
        return Ok(2);
    }
    let s = settings();
    let env_map: BTreeMap<String, String> = std::env::vars().collect();
    let history = match krutrim_evaluator::history::load_history(&paths, &source) {
        Ok(h) => h,
        Err(e) => return Ok(err1(e)),
    };
    let pairs = history.pairs();
    let selection = krutrim_evaluator::miner::select_pairs(&pairs, limit);
    let selected = &selection.pairs;
    if selected.is_empty() {
        return Ok(err1("no prompt/answer pairs found in that history"));
    }
    println!(
        "{} conversations, {} prompts; judging {} ({} duplicates and {} oversized skipped).",
        history.conversations.len(),
        pairs.len(),
        selected.len(),
        selection.duplicates,
        selection.oversized
    );
    let resolved_judge = judge_spec.or_else(|| env_map.get("MJ_JUDGE").cloned());
    let Some(resolved_judge) = resolved_judge else {
        return Ok(err1(
            "a mining judge is required: pass --judge <provider>/<model> or set MJ_JUDGE \
             (free local example: --judge ollama/llama3.2:3b)",
        ));
    };
    let mut ready = true;
    for problem in krutrim_evaluator::judge::judge_problems(&resolved_judge, &env_map) {
        eprintln!("problem: judge: {problem}");
        ready = false;
    }
    let prices = match load_prices(Some(&s.prices_path)) {
        Ok(p) => p,
        Err(e) => return Ok(err1(e)),
    };
    let mut cache = if no_cache {
        None
    } else {
        Some(krutrim_evaluator::miner::VerdictCache::load(
            cache_path.clone(),
        ))
    };
    let estimate = match krutrim_evaluator::miner::estimate_judge_cost_usd(
        &pairs,
        &resolved_judge,
        &prices,
        limit,
        cache.as_ref(),
    ) {
        Ok(e) => e,
        Err(e) => return Ok(err1(e)),
    };
    let cap = max_usd.unwrap_or(s.max_usd);
    match estimate {
        None => {
            eprintln!(
                "problem: no price for judge {resolved_judge}; add it to prices.yaml \
                 (see `km list prices`) so judging cost can be bounded."
            );
            ready = false;
        }
        Some(est) => {
            println!(
                "Judge {resolved_judge}: worst-case judging cost ${est:.4} (₹{:.2}) \
                 against a cap of ${cap:.2} for {} prompts.",
                est * s.usd_to_inr,
                selected.len()
            );
            if est > cap {
                eprintln!(
                    "problem: worst-case judging cost exceeds the cap; \
                     raise --max-usd or use --limit."
                );
                ready = false;
            }
        }
    }
    if dry_run {
        println!(
            "Dry run: nothing was called or spent; {}.",
            if ready {
                "a real mining would start"
            } else {
                "a real mining would be refused"
            }
        );
        return Ok(if ready { 0 } else { 1 });
    }
    if !ready {
        return Ok(err1(
            "refusing to mine; fix the problems above (nothing was called or spent)",
        ));
    }
    let estimate = estimate.expect("ready implies priced");
    if estimate > 0.0 && !allow_paid {
        return Ok(err1(
            "paid judging requires --allow-paid and an approved --max-usd budget",
        ));
    }
    let judge = match krutrim_evaluator::judge::make_judge(&resolved_judge, &env_map) {
        Ok(j) => j,
        Err(e) => return Ok(err1(e)),
    };
    let progress = |done: usize, total: usize, _pair: &krutrim_evaluator::history::PromptPair| {
        if done == 1 || done == total || done % 25 == 0 {
            println!(" judging {done}/{total}…");
        }
    };
    let result = match krutrim_evaluator::miner::mine_failures(
        &pairs,
        &*judge,
        &resolved_judge,
        min_confidence,
        limit,
        Some(&progress),
        cache.as_mut(),
    ) {
        Ok(r) => r,
        Err(e) => return Ok(err1(e)),
    };
    println!(
        "Checked {}: {} failures kept, {} right, {} unclear, {} wrong below \
         {min_confidence} confidence, {} judge errors skipped, {} from cache, \
         {} pairs with secrets redacted before judging.",
        result.checked,
        result.failures.len(),
        result.correct,
        result.unclear,
        result.below_threshold,
        result.errors,
        result.cache_hits,
        result.redacted_pairs
    );
    print_failure_breakdown(&result.failures);
    if result.failures.is_empty() {
        return Ok(err1(
            "no confident failures found; no suite written. Try more history or --limit off.",
        ));
    }
    let version = version.unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string());
    match krutrim_evaluator::microeval::write_micro_eval(
        &result.failures,
        &out,
        &name,
        &version,
        &resolved_judge,
    ) {
        Ok((suite, evidence_path)) => {
            println!(
                "Micro-eval: {} ({} tasks). Evidence: {}.",
                out.display(),
                suite.tasks.len(),
                evidence_path.display()
            );
            println!("Keep the evidence file private: it contains your prompts and answers.");
            println!(
                "Run it: km run {name} -r model:<provider>/<model> --judge {resolved_judge} \
                 --allow-paid --max-usd <budget> (free local check first: -r oracle -r echo)"
            );
            Ok(0)
        }
        Err(e) => Ok(err1(e)),
    }
}

// ---------------- run ----------------

struct RunArgs {
    suite: String,
    runners: Vec<String>,
    reps: i64,
    max_usd: Option<f64>,
    seed: i64,
    tasks: Vec<String>,
    dry_run: bool,
    allow_paid: bool,
    judge_spec: Option<String>,
}

fn parse_run_args(args: &[String]) -> Result<Option<RunArgs>, u8> {
    let mut suite: Option<String> = None;
    let mut runners = Vec::new();
    let mut tasks = Vec::new();
    let (mut reps, mut seed, mut max_usd) = (1i64, 7i64, None);
    let (mut dry_run, mut allow_paid) = (false, false);
    let mut judge_spec: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let take = |i: &mut usize| -> Result<String, u8> {
            *i += 1;
            args.get(*i)
                .cloned()
                .ok_or_else(|| err1("expected a value after the flag"))
        };
        match a.as_str() {
            "--help" => {
                println!("Usage: km run [OPTIONS] SUITE\n\nRun SUITE against the given runners, e.g. km run suites/example.yaml -r oracle\n\nArguments:\n  SUITE  Suite name or path  [required]\n\nOptions:\n  -r, --runner TEXT       Runner spec (repeatable)  [default: echo, oracle]\n  --repeats INT   Tries per task, 1-10  [default: 1]\n  --max-usd FLOAT         Spend cap in USD (default MJ_MAX_USD or 1.0)\n  --seed INT              Run seed  [default: 7]\n  --task TEXT             Only these task ids (repeatable)\n  --dry-run               Estimate cost and stop; runs nothing.\n  --allow-paid            Explicitly permit paid API calls after preflight.\n  --judge TEXT            Grading judge as <provider>/<model> (default MJ_JUDGE or the suite's judge).\n  --help                  Show this message and exit.\n");
                return Ok(None);
            }
            "--dry-run" => dry_run = true,
            "--allow-paid" => allow_paid = true,
            "-r" | "--runner" => runners.push(take(&mut i)?),
            "--repeats" => {
                reps = take(&mut i)?
                    .parse()
                    .map_err(|_| err1("--repeats needs an integer"))?
            }
            "--seed" => {
                seed = take(&mut i)?
                    .parse()
                    .map_err(|_| err1("--seed needs an integer"))?
            }
            "--max-usd" => {
                max_usd = Some(
                    take(&mut i)?
                        .parse()
                        .map_err(|_| err1("--max-usd needs a number"))?,
                )
            }
            "--task" => tasks.push(take(&mut i)?),
            "--judge" => judge_spec = Some(take(&mut i)?),
            other if other.starts_with("--judge=") => judge_spec = Some(other[8..].to_string()),
            other if other.starts_with("--runner=") => runners.push(other[9..].to_string()),
            other if other.starts_with("--task=") => tasks.push(other[7..].to_string()),
            other if other.starts_with("--seed=") => {
                seed = other[7..]
                    .parse()
                    .map_err(|_| err1("--seed needs an integer"))?
            }
            other if other.starts_with("--max-usd=") => {
                max_usd = Some(
                    other[10..]
                        .parse()
                        .map_err(|_| err1("--max-usd needs a number"))?,
                )
            }
            other if other.starts_with("--repeats=") => {
                reps = other[10..]
                    .parse()
                    .map_err(|_| err1("--repeats needs an integer"))?
            }
            other if other.starts_with('-') => {
                return Err(err1(format!("No such option: {other}")))
            }
            other => {
                if suite.is_some() {
                    return Err(err1("unexpected extra argument"));
                }
                suite = Some(other.to_string());
            }
        }
        i += 1;
    }
    let Some(suite) = suite else {
        eprintln!("error: Missing argument 'SUITE'.");
        return Err(2);
    };
    if runners.is_empty() {
        runners = vec!["echo".into(), "oracle".into()];
    }
    Ok(Some(RunArgs {
        suite,
        runners,
        reps,
        max_usd,
        seed,
        tasks,
        dry_run,
        allow_paid,
        judge_spec,
    }))
}

fn cmd_run(args: &[String]) -> Result<u8, String> {
    let parsed = match parse_run_args(args) {
        Ok(Some(p)) => p,
        Ok(None) => return Ok(0),
        Err(code) => return Ok(code),
    };
    let s = settings();
    let dir = suites_dir(&s);
    let path = match resolve_suite(&parsed.suite, &dir) {
        Ok(p) => p,
        Err(e) => return Ok(err1(e)),
    };
    let suite = match load_suite(&path) {
        Ok(su) => su,
        Err(e) => return Ok(err1(e)),
    };
    let prices = match load_prices(Some(&s.prices_path)) {
        Ok(p) => p,
        Err(e) => return Ok(err1(e)),
    };
    let mut runners = Vec::new();
    for spec in &parsed.runners {
        match make_runner(spec, &prices, &std::env::vars().collect()) {
            Ok(r) => runners.push(r),
            Err(e) => return Ok(err1(e)),
        }
    }
    // Always check first: keys, prices and the worst case against the cap.
    let selected = match suite.select(if parsed.tasks.is_empty() {
        None
    } else {
        Some(&parsed.tasks)
    }) {
        Ok(t) => t,
        Err(e) => return Ok(err1(e)),
    };
    let cap = parsed.max_usd.unwrap_or(s.max_usd);
    // Resolve the grading judge like the Python CLI: --judge, then MJ_JUDGE,
    // then the suite's recorded judge_model. No judge = llm_judge stays pending.
    let env_map: BTreeMap<String, String> = std::env::vars().collect();
    let needs_judge: Vec<&Task> = selected
        .iter()
        .filter(|t| t.grader.kind() == "llm_judge")
        .collect();
    let mut resolved_judge = parsed
        .judge_spec
        .clone()
        .or_else(|| env_map.get("MJ_JUDGE").cloned());
    if resolved_judge.is_none() && !needs_judge.is_empty() {
        resolved_judge = needs_judge.iter().find_map(|t| match &t.grader {
            krutrim_evaluator::suite::GraderSpec::LlmJudge { judge_model, .. } => {
                judge_model.clone()
            }
            _ => None,
        });
    }
    let mut judge_box: Option<Box<krutrim_evaluator::judge::JudgeFn>> = None;
    let mut judge_paid = false;
    let mut judge_worst_usd: Option<f64> = Some(0.0);
    if let Some(spec) = &resolved_judge {
        if !needs_judge.is_empty() {
            let problems = krutrim_evaluator::judge::judge_problems(spec, &env_map);
            if !problems.is_empty() {
                return Ok(err1(format!(
                    "judge '{spec}' cannot run: {}",
                    problems.join("; ")
                )));
            }
            match krutrim_evaluator::judge::parse_judge_spec(spec) {
                Ok((provider, model)) => {
                    let price = prices.lookup(provider, model);
                    judge_paid = price
                        .map(|p| p.input > 0.0 || p.output > 0.0)
                        .unwrap_or(true);
                    judge_worst_usd = price.map(|p| {
                        let calls = (needs_judge.len() * runners.len()) as f64 * parsed.reps as f64;
                        let per_call_in = needs_judge
                            .iter()
                            .map(|t| {
                                krutrim_evaluator::pricing::max_input_tokens(&format!(
                                    "{}{}{}",
                                    t.render_prompt(),
                                    t.rubric.as_deref().unwrap_or(""),
                                    t.reference.as_deref().unwrap_or("")
                                )) + t.max_output_tokens
                            })
                            .max()
                            .unwrap_or(0);
                        calls
                            * p.cost(
                                per_call_in,
                                krutrim_evaluator::judge::DEFAULT_JUDGE_MAX_TOKENS,
                            )
                    });
                }
                Err(e) => return Ok(err1(e)),
            }
            match krutrim_evaluator::judge::make_judge(spec, &env_map) {
                Ok(j) => judge_box = Some(j),
                Err(e) => return Ok(err1(e)),
            }
        }
    } else if parsed.judge_spec.is_some() && needs_judge.is_empty() {
        println!("--judge given but no selected task uses an LLM judge; ignoring it.");
    }
    let check = krutrim_evaluator::preflight(&selected, &runners, cap, parsed.reps);
    let mut rows: Vec<Vec<String>> = Vec::new();
    for runner in &runners {
        let mine: Vec<_> = check
            .estimates
            .iter()
            .filter(|e| e.runner == runner.name)
            .collect();
        let unknown = mine.iter().any(|e| e.cost_usd.is_none());
        let known: f64 = parsed.reps as f64 * mine.iter().filter_map(|e| e.cost_usd).sum::<f64>();
        let usd = if unknown {
            "unknown".into()
        } else {
            format!("${known:.4}")
        };
        let inr = if unknown {
            "unknown".into()
        } else {
            format!("₹{:.2}", known * s.usd_to_inr)
        };
        let status = if !runner.problems().is_empty() || unknown {
            "not ready"
        } else {
            "ready"
        };
        rows.push(vec![
            runner.name.clone(),
            (mine.len() as i64 * parsed.reps).to_string(),
            usd,
            inr,
            status.into(),
        ]);
    }
    println!(
        "{}",
        render_table(
            Some("Cost estimate"),
            &[
                "Runner",
                "Attempts",
                "Worst case USD",
                "Worst case INR",
                "Status"
            ],
            &rows
        )
    );
    println!(
        "Worst case ${:.4} (₹{:.2}) against a cap of ${cap:.2}. Worst case = prompt bytes in, max_output_tokens out.",
        check.worst_case_usd,
        check.worst_case_usd * s.usd_to_inr
    );
    for warning in &check.warnings {
        eprintln!("warning: {warning}");
    }
    for problem in &check.problems {
        eprintln!("problem: {problem}");
    }
    if let Some(spec) = &resolved_judge {
        if judge_box.is_some() {
            match judge_worst_usd {
                None => eprintln!(
                    "problem: no price for judge {spec}; grading cost cannot be bounded \
                     (add it to prices.yaml)."
                ),
                Some(jw) => {
                    let total = check.worst_case_usd + jw;
                    println!(
                        "Grading judge: {spec} ({} task(s)), worst case ${jw:.4} (₹{:.2}); \
                         total worst case ${total:.4} against the ${cap:.2} cap.",
                        needs_judge.len(),
                        jw * s.usd_to_inr
                    );
                    if total > cap {
                        eprintln!(
                            "problem: runners + grading judge exceed the cap; \
                             raise --max-usd or run fewer tasks/runners."
                        );
                    }
                }
            }
        }
    } else if !needs_judge.is_empty() {
        eprintln!(
            "warning: {} task(s) need an LLM judge and none is configured; they will be \
             left 'pending'. Pass --judge <provider>/<model>.",
            needs_judge.len()
        );
    }
    let judge_ok = judge_worst_usd
        .map(|jw| check.worst_case_usd + jw <= cap)
        .unwrap_or(false);
    if parsed.dry_run {
        let verdict = if check.ok() && judge_ok {
            "a real run would start"
        } else {
            "a real run would be refused"
        };
        println!("Dry run: nothing was called or spent; {verdict}.");
        return Ok(if check.ok() && judge_ok { 0 } else { 1 });
    }
    if !check.ok() || !judge_ok {
        return Ok(err1(
            "refusing to run; fix the problems above (nothing was called or spent)",
        ));
    }
    if !parsed.allow_paid && (check.worst_case_usd > 0.0 || judge_paid) {
        return Ok(err1(
            "paid execution requires --allow-paid and an approved --max-usd budget",
        ));
    }
    let store = match Store::open(&s.db_path) {
        Ok(st) => st,
        Err(e) => return Ok(err1(e)),
    };
    let repeats_note = if parsed.reps > 1 {
        format!(", {} tries each", parsed.reps)
    } else {
        String::new()
    };
    let live = std::io::stdout().is_terminal() && anim_enabled();
    let progress = live.then(|| LiveProgress::new(&runners, selected.len() * parsed.reps as usize));
    let on_attempt = |task_id: &str, runner_name: &str| {
        if let Some(live) = &progress {
            live.attempt(task_id, runner_name);
            return;
        }
        print!("  {task_id} @ {runner_name} … ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
    };
    let on_result = |r: &ResultRecord| {
        if let Some(live) = &progress {
            live.result(r);
            return;
        }
        let mark = if r.status == "skipped" {
            "skipped"
        } else if r.status != "ok" {
            "error"
        } else if r.passed == Some(true) {
            "✓"
        } else if r.passed == Some(false) {
            "✗"
        } else {
            "…"
        };
        let score = r.score.map(|v| format!(" {v:.2}")).unwrap_or_default();
        let timed = r
            .latency_s
            .map(|s| {
                if s >= 10.0 {
                    format!(" ({s:.1}s)")
                } else {
                    format!(" ({s:.3}s)")
                }
            })
            .unwrap_or_default();
        println!(
            "\r{mark}{score} · {} @ {}{timed}      ",
            r.task_id, r.runner
        );
    };
    let on_start = |run_id: &str| {
        let line = format!(
            "Run {run_id} · {} v{}{}",
            suite.name, suite.version, repeats_note
        );
        match &progress {
            Some(live) => live.println(line),
            None => println!("{line}"),
        }
    };
    let callbacks = RunCallbacks {
        on_start: Some(&on_start),
        on_attempt: Some(&on_attempt),
        on_result: Some(&on_result),
        judge: judge_box.as_deref(),
    };
    match run_suite(
        &suite,
        &runners,
        &store,
        &s,
        parsed.seed,
        parsed.reps,
        parsed.max_usd,
        if parsed.tasks.is_empty() {
            None
        } else {
            Some(&parsed.tasks)
        },
        &callbacks,
        None,
    ) {
        Ok(summary) => {
            if let Some(live) = &progress {
                live.clear();
            }
            finish(&store, &summary.run_id, &summary.run_dir, true);
            Ok(0)
        }
        Err(PreflightError(problems)) => {
            for p in &problems {
                eprintln!("error: refused to run: {p}");
            }
            Ok(1)
        }
    }
}

fn finish(store: &Store, run_id: &str, run_dir: &Path, announce: bool) {
    let written = write_outputs(store, run_id, run_dir);
    let report = match build_report(store, run_id) {
        Ok(r) => r,
        Err(_) => return,
    };
    let headers: Vec<String> = LEADERBOARD_HEADERS.iter().map(|s| s.to_string()).collect();
    println!(
        "{}",
        render_table(
            Some("Leaderboard"),
            &headers,
            &report
                .rows
                .iter()
                .map(leaderboard_cells)
                .collect::<Vec<_>>(),
        )
    );
    if !report.verdicts.is_empty() {
        let headers: Vec<String> = VERDICT_HEADERS.iter().map(|s| s.to_string()).collect();
        println!(
            "{}",
            render_table(
                Some("Is the difference real?"),
                &headers,
                &report
                    .verdicts
                    .iter()
                    .map(verdict_cells)
                    .collect::<Vec<_>>(),
            )
        );
        for v in &report.verdicts {
            println!("  {}", v.explanation);
        }
        println!("Too close to call means the gap could be chance, not that they are equal.");
    }
    println!("{DEFINITIONS}");
    if announce {
        let raw = run_dir.display();
        println!("Run {run_id} · raw outputs in {raw}/");
        println!("Report: {}", written.markdown.display());
        println!("Report: {}", written.html.display());
        println!("Report: {}", written.results.display());
    }
}

// ---------------- export / report / import ----------------

fn positional(args: &[String]) -> (Vec<String>, BTreeMap<String, Option<String>>) {
    let mut pos = Vec::new();
    let mut opts: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if let Some((k, v)) = a.split_once('=') {
            if k.starts_with("--") {
                opts.insert(k.to_string(), Some(v.to_string()));
                i += 1;
                continue;
            }
        }
        if a.starts_with("--") {
            let next = args.get(i + 1);
            if matches!(a.as_str(), "--output" | "-o" | "--name" | "--port") {
                opts.insert(a.clone(), next.cloned());
                i += 2;
            } else {
                opts.insert(a.clone(), None);
                i += 1;
            }
        } else {
            pos.push(a.clone());
            i += 1;
        }
    }
    (pos, opts)
}

fn cmd_export(args: &[String]) -> Result<u8, String> {
    let (pos, opts) = positional(args);
    if opts.contains_key("--help") {
        println!("Usage: km export [OPTIONS] [RUN_ID]\n\nWrite the schema-1.1 JSON export (default: latest run). Outputs are omitted unless --include-outputs is explicit.\n\nOptions:\n  --include-outputs      Include (possibly sensitive) model output text\n  -o, --output PATH      Where to write (default: runs/<id>/results.json)\n  --help                 Show this message and exit.\n");
        return Ok(0);
    }
    let s = settings();
    let store = match Store::open(&s.db_path) {
        Ok(st) => st,
        Err(e) => return Ok(err1(e)),
    };
    let run_id = match pos
        .first()
        .cloned()
        .or_else(|| store.latest_run_id().ok().flatten())
    {
        Some(id) => id,
        None => return Ok(err1("no runs yet")),
    };
    let run = match store.get_run(&run_id) {
        Ok(Some(r)) => r,
        _ => return Ok(err1(format!("no run '{run_id}'"))),
    };
    let destination = opts
        .get("--output")
        .or_else(|| opts.get("-o"))
        .cloned()
        .flatten()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&run.artifacts_dir).join("results.json"));
    match krutrim_evaluator::export::write_export(
        &store,
        &run_id,
        &destination,
        opts.contains_key("--include-outputs"),
    ) {
        Ok(path) => {
            println!("Export: {}", path.display());
            Ok(0)
        }
        Err(e) => Ok(err1(e)),
    }
}

fn cmd_report(args: &[String]) -> Result<u8, String> {
    let (pos, opts) = positional(args);
    if opts.contains_key("--help") {
        println!("Usage: km report [OPTIONS] [RUN_ID]\n\nPrint the leaderboard for RUN_ID (default: latest run).\n");
        return Ok(0);
    }
    let s = settings();
    let store = match Store::open(&s.db_path) {
        Ok(st) => st,
        Err(e) => return Ok(err1(e)),
    };
    let run_id = match pos
        .first()
        .cloned()
        .or_else(|| store.latest_run_id().ok().flatten())
    {
        Some(id) => id,
        None => return Ok(err1("no runs yet")),
    };
    let run = match store.get_run(&run_id) {
        Ok(Some(r)) => r,
        _ => return Ok(err1(format!("no run '{run_id}'"))),
    };
    finish(&store, &run_id, Path::new(&run.artifacts_dir), false);
    Ok(0)
}

fn cmd_import(args: &[String]) -> Result<u8, String> {
    let (pos, opts) = positional(args);
    if opts.contains_key("--help") || pos.is_empty() {
        println!("Usage: km import-suite [OPTIONS] SOURCE [OUTPUT]\n\nImport a foreign suite directory into Mulyankan YAML.\n\nArguments:\n  SOURCE  Directory to import  [required]\n  OUTPUT  Where to write YAML (default: suites/<name>.yaml)\n\nOptions:\n  --name TEXT  Suite name (default: directory name)\n  --help       Show this message and exit.\n");
        return Ok(if pos.is_empty() && !opts.contains_key("--help") {
            2
        } else {
            0
        });
    }
    let source = PathBuf::from(&pos[0]);
    if !source.is_dir() {
        return Ok(err1(format!("not a directory: {}", source.display())));
    }
    let name = opts.get("--name").cloned().flatten();
    match krutrim_evaluator::importer::import_suite(&source, name.as_deref()) {
        Ok(imported) => {
            let out = pos.get(1).cloned().map(PathBuf::from).unwrap_or_else(|| {
                PathBuf::from("suites")
                    .join(format!("{}.yaml", importer_slug(&imported.suite.name)))
            });
            if let Some(parent) = out.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let text = krutrim_evaluator::importer::suite_to_yaml(&imported.suite);
            if let Err(e) = std::fs::write(&out, text) {
                return Ok(err1(e));
            }
            println!(
                "Imported {} task{} → {}",
                imported.suite.tasks.len(),
                if imported.suite.tasks.len() == 1 {
                    ""
                } else {
                    "s"
                },
                out.display()
            );
            if !imported.unhandled.is_empty() {
                println!("Could not import (left alone):");
                for f in &imported.unhandled {
                    println!("  - {f}");
                }
            }
            Ok(0)
        }
        Err(e) => Ok(err1(e)),
    }
}

fn importer_slug(name: &str) -> String {
    // Path(name).stem in Python
    let base = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name);
    base.to_string()
}

// ---------------- rich-style table ----------------

fn display_width(s: &str) -> usize {
    // Combining marks and zero-width chars count 0; most other code points 1;
    // wide CJK/fullwidth blocks 2. Good enough for the tables km prints.
    s.chars()
        .map(|c| {
            let u = c as u32;
            if (0x300..=0x36f).contains(&u)
                || (0x900..=0x97f).contains(&u)
                    && matches!(u, 0x901..=0x903 | 0x93a..=0x94f | 0x951..=0x957 | 0x962..=0x963)
                || u == 0x200d
            {
                0
            } else if (0x1100..=0x115f).contains(&u)
                || (0x2e80..=0x9fff).contains(&u)
                || (0xa000..=0xa4cf).contains(&u)
                || (0xac00..=0xd7a3).contains(&u)
                || (0xf900..=0xfaff).contains(&u)
                || (0xfe30..=0xfe6f).contains(&u)
                || (0xff00..=0xff60).contains(&u)
                || (0x1f300..=0x1f9ff).contains(&u)
            {
                2
            } else {
                1
            }
        })
        .sum()
}

fn pad(s: &str, width: usize) -> String {
    let w = display_width(s);
    if w >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - w))
    }
}

fn wrap_cell(text: &str, width: usize) -> Vec<String> {
    if display_width(text) <= width {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split(' ') {
        if cur.is_empty() {
            cur = word.to_string();
        } else if display_width(&cur) + 1 + display_width(word) <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(cur);
            cur = word.to_string();
        }
        while display_width(&cur) > width {
            let mut taken = String::new();
            let mut rest = String::new();
            let mut w = 0;
            for c in cur.chars() {
                let cw = display_width(&c.to_string());
                if w + cw <= width && rest.is_empty() {
                    taken.push(c);
                    w += cw;
                } else {
                    rest.push(c);
                }
            }
            lines.push(taken);
            cur = rest;
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

fn term_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|w: &usize| *w >= 40)
        .unwrap_or(120)
}

/// Indicatif-backed live progress for `km run` on a terminal: one bar per
/// runner plus running token/USD totals. Draws to stderr; the classic
/// stdout tables print after it clears.
struct LiveProgress {
    multi: indicatif::MultiProgress,
    bars: std::cell::RefCell<Vec<(String, indicatif::ProgressBar)>>,
    totals: indicatif::ProgressBar,
    input_tokens: std::cell::Cell<i64>,
    output_tokens: std::cell::Cell<i64>,
    cost_usd: std::cell::Cell<f64>,
    cost_known: std::cell::Cell<bool>,
    passed: std::cell::Cell<usize>,
    done: std::cell::Cell<usize>,
}

impl LiveProgress {
    fn new(runners: &[Runner], attempts_per_runner: usize) -> LiveProgress {
        let multi = indicatif::MultiProgress::new();
        let bar_style = if color_enabled() {
            "{spinner:.208} {msg:<28} [{bar:24.208/dim}] {pos}/{len}"
        } else {
            "{spinner} {msg:<28} [{bar:24}] {pos}/{len}"
        };
        let style = indicatif::ProgressStyle::with_template(bar_style)
            .unwrap_or_else(|_| indicatif::ProgressStyle::default_bar())
            .progress_chars("██░")
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "✓"]);
        let mut bars = Vec::new();
        for runner in runners {
            let bar = multi.add(indicatif::ProgressBar::new(attempts_per_runner as u64));
            bar.set_style(style.clone());
            bar.set_message(runner.name.clone());
            bar.enable_steady_tick(std::time::Duration::from_millis(90));
            bars.push((runner.name.clone(), bar));
        }
        let totals = multi.add(indicatif::ProgressBar::new_spinner());
        totals.set_message("tokens 0 in / 0 out · $0.0000 · 0 passed");
        totals.enable_steady_tick(std::time::Duration::from_millis(120));
        LiveProgress {
            multi,
            bars: std::cell::RefCell::new(bars),
            totals,
            input_tokens: std::cell::Cell::new(0),
            output_tokens: std::cell::Cell::new(0),
            cost_usd: std::cell::Cell::new(0.0),
            cost_known: std::cell::Cell::new(true),
            passed: std::cell::Cell::new(0),
            done: std::cell::Cell::new(0),
        }
    }

    fn println(&self, line: String) {
        let _ = self.multi.println(line);
    }

    fn attempt(&self, task_id: &str, runner_name: &str) {
        if let Some((_, bar)) = self
            .bars
            .borrow()
            .iter()
            .find(|(name, _)| name == runner_name)
        {
            bar.set_message(format!("{runner_name} · {task_id}"));
        }
    }

    fn result(&self, record: &ResultRecord) {
        if let Some((_, bar)) = self
            .bars
            .borrow()
            .iter()
            .find(|(name, _)| name == &record.runner)
        {
            bar.inc(1);
            bar.set_message(record.runner.clone());
        }
        self.done.set(self.done.get() + 1);
        if record.passed == Some(true) {
            self.passed.set(self.passed.get() + 1);
        }
        self.input_tokens
            .set(self.input_tokens.get() + record.input_tokens.unwrap_or(0));
        self.output_tokens
            .set(self.output_tokens.get() + record.output_tokens.unwrap_or(0));
        match record.cost_usd {
            Some(cost) => self.cost_usd.set(self.cost_usd.get() + cost),
            None => {
                if record.status == "ok" {
                    self.cost_known.set(false);
                }
            }
        }
        let cost = if self.cost_known.get() {
            format!("${:.4}", self.cost_usd.get())
        } else {
            "cost unknown".into()
        };
        self.totals.set_message(format!(
            "tokens {} in / {} out · {cost} · {} passed · {} done",
            self.input_tokens.get(),
            self.output_tokens.get(),
            self.passed.get(),
            self.done.get()
        ));
        let mark = if record.status == "skipped" {
            "skipped".to_string()
        } else if record.status != "ok" {
            "error".to_string()
        } else if record.passed == Some(true) {
            if color_enabled() {
                "\u{1b}[32m✓\u{1b}[0m".into()
            } else {
                "✓".into()
            }
        } else if record.passed == Some(false) {
            if color_enabled() {
                "\u{1b}[31m✗\u{1b}[0m".into()
            } else {
                "✗".into()
            }
        } else {
            "…".into()
        };
        let timed = record
            .latency_s
            .map(|v| format!(" ({v:.3}s)"))
            .unwrap_or_default();
        let _ = self.multi.println(format!(
            "{mark} · {} @ {}{timed}",
            record.task_id, record.runner
        ));
    }

    fn clear(&self) {
        for (_, bar) in self.bars.borrow().iter() {
            bar.finish_and_clear();
        }
        self.totals.finish_and_clear();
        let _ = self.multi.clear();
    }
}

pub fn render_table(
    title: Option<&str>,
    headers: &[impl AsRef<str>],
    rows: &[Vec<String>],
) -> String {
    let headers: Vec<String> = headers.iter().map(|h| h.as_ref().to_string()).collect();
    let rendered = render_table_owned(title, &headers, rows);
    if color_enabled() {
        // Post-colour the verdict marks; widths were computed on plain text.
        rendered
            .replace('✓', "\u{1b}[32m✓\u{1b}[0m")
            .replace('✗', "\u{1b}[31m✗\u{1b}[0m")
    } else {
        rendered
    }
}

fn render_table_owned(title: Option<&str>, headers: &[String], rows: &[Vec<String>]) -> String {
    let cols = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| display_width(h)).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(cols) {
            widths[i] = widths[i].max(display_width(cell));
        }
    }
    let budget = term_width();
    let total = |w: &[usize]| w.iter().sum::<usize>() + 3 * cols + 1;
    while total(&widths) > budget {
        let Some((idx, _)) = widths.iter().enumerate().max_by_key(|(_, w)| **w) else {
            break;
        };
        if widths[idx] <= 4 {
            break;
        }
        widths[idx] -= 1;
    }
    let line = |left: &str, mid: &str, right: &str, fill: &str| -> String {
        let parts: Vec<String> = widths.iter().map(|w| fill.repeat(w + 2)).collect();
        format!("{left}{}{right}", parts.join(mid))
    };
    let render_row = |cells: &[String], sep: &str| -> Vec<String> {
        let wrapped: Vec<Vec<String>> = (0..cols)
            .map(|i| wrap_cell(cells.get(i).map(|s| s.as_str()).unwrap_or(""), widths[i]))
            .collect();
        let height = wrapped.iter().map(|v| v.len()).max().unwrap_or(1);
        (0..height)
            .map(|line_no| {
                let parts: Vec<String> = (0..cols)
                    .map(|i| {
                        format!(
                            " {} ",
                            pad(
                                wrapped[i].get(line_no).map(|s| s.as_str()).unwrap_or(""),
                                widths[i]
                            )
                        )
                    })
                    .collect();
                format!("{sep}{}{sep}", parts.join(sep))
            })
            .collect()
    };
    let mut out: Vec<String> = Vec::new();
    if let Some(t) = title {
        out.push(t.to_string());
    }
    out.push(line("┏", "┳", "┓", "━"));
    out.extend(render_row(headers, "┃"));
    out.push(line("┡", "╇", "┩", "━"));
    for row in rows {
        out.extend(render_row(row, "│"));
    }
    out.push(line("└", "┴", "┘", "─"));
    out.join("\n")
}

// ---------------- ui ----------------

fn cmd_ui(args: &[String]) -> Result<u8, String> {
    let (pos, opts) = positional(args);
    if opts.contains_key("--help") {
        println!("Usage: km ui [OPTIONS] [PROJECT_DIR]\n\nOpen a local pilot workbench with task selection and free mock checks.\n\nArguments:\n  PROJECT_DIR  [default: .]\n\nOptions:\n  --port INT  [default: 8765]\n  --help      Show this message and exit.\n");
        return Ok(0);
    }
    let project = pos.first().cloned().unwrap_or_else(|| ".".to_string());
    let port: u16 = match opts
        .get("--port")
        .cloned()
        .flatten()
        .map(|v| v.parse())
        .transpose()
    {
        Ok(Some(p)) => p,
        Ok(None) => 8765,
        Err(_) => return Ok(err1("--port needs an integer")),
    };
    if !(1024..=65535).contains(&port) {
        return Ok(err1("choose a port between 1024 and 65535"));
    }
    match krutrim_evaluator::workbench::serve_workbench(Path::new(&project), port) {
        Ok(()) => Ok(0),
        Err(e) => Ok(err1(e)),
    }
}
