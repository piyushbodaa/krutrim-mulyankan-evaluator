//! Writing report.md / report.html / results.json for a run (shared by the
//! CLI and the desktop engine, like cli._finish + desktop_bridge._write_outputs).

use crate::export::{dump_json_ascii, result_export};
use crate::report::{build_report, to_html, to_markdown};
use crate::store::{ResultRecord, Store};
use std::path::{Path, PathBuf};

pub struct WrittenOutputs {
    pub markdown: PathBuf,
    pub html: PathBuf,
    pub results: PathBuf,
}

pub fn write_outputs(store: &Store, run_id: &str, directory: &Path) -> WrittenOutputs {
    let report = build_report(store, run_id).expect("report for finished run");
    let markdown = directory.join("report.md");
    let html = directory.join("report.html");
    let results = directory.join("results.json");
    let _ = std::fs::write(&markdown, to_markdown(&report));
    let _ = std::fs::write(&html, to_html(&report));
    if let Ok(payload) = result_export(store, run_id, false) {
        let _ = std::fs::write(&results, dump_json_ascii(&payload.value) + "\n");
    }
    WrittenOutputs {
        markdown,
        html,
        results,
    }
}

/// Plain-language line per finished attempt (desktop explanations.json).
pub fn explain(record: &ResultRecord) -> String {
    crate::plain(record)
}
