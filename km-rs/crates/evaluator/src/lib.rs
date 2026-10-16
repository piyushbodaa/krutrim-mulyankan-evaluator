//! Krutrim Mulyankan evaluator core, shared by the `km` CLI and the desktop
//! engine. Behaviour mirrors the Python `kritrim_mulyankan` package for
//! free/local runs; see the Rust PR notes for what is not ported yet.

pub mod claude_draft;
pub mod engine;
pub mod export;
pub mod graders;
pub mod importer;
pub mod naming;
pub mod observations;
pub mod outputs;
pub mod pricing;
pub mod provenance;
pub mod providers;
pub mod pypy;
pub mod report;
pub mod runners;
pub mod settings;
pub mod store;
pub mod suite;
pub mod tokens;
pub mod verdicts;
pub mod workbench;

pub use engine::{preflight, run_suite, Preflight, PreflightError, RunCallbacks, RunSummary};
pub use naming::{
    duration_label, model_blurb, pack_copy, plain_verdict, pretty_model, runner_label, rupees,
    PlainVerdict,
};
pub use observations::{AppObservation, AppObservationRunner};
pub use pricing::{load_prices, PriceTable};
pub use report::{build_report, to_html, to_markdown, Report, RunnerRow};
pub use runners::{
    make_model_runner, make_runner, ModelOptions, RunContext, RunOutput, Runner, RunnerKind,
    RUNNER_CATALOG,
};
pub use settings::{effective_suites_dir, Settings};
pub use store::{ResultRecord, RunRecord, Store};
pub use suite::{discover_suites, load_suite, resolve_suite, GraderSpec, Suite, Task};
pub use verdicts::Verdict;

/// Plain-language explanation for one finished attempt (mirrors
/// desktop_bridge._explain for the graded/error/skipped/pending cases).
pub fn plain(record: &ResultRecord) -> String {
    let details = record.grade_details.clone().unwrap_or_default();
    if record.status != "ok" {
        return record
            .error
            .clone()
            .unwrap_or_else(|| format!("This check did not run ({})", record.status));
    }
    if record.grade_status.as_deref() != Some("graded") {
        return "This answer is waiting for a human or judge score.".into();
    }
    if record.passed == Some(true) {
        if record.grader.as_deref() == Some("llm_judge") {
            return match record.score {
                Some(s) => format!("Matched the rubric (score {s:.2})."),
                None => "Matched the rubric.".into(),
            };
        }
        return "Matched the expected answer.".into();
    }
    if let Some(reason) = details.get("reason").and_then(|v| v.as_str()) {
        return reason.to_string();
    }
    "Answer differed from the expected output.".into()
}
