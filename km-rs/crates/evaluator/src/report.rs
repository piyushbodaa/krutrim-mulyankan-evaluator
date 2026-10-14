//! Report aggregation and Markdown/HTML rendering, mirroring report.py.

use crate::export::result_export;
use crate::store::{ResultRecord, RunRecord, Store};
use crate::verdicts::{bootstrap_ci_mean, mean, mean_ci, task_tries, wilson_interval, Verdict};
use serde_json::Value;
use std::collections::BTreeMap;

pub const DEFINITIONS: &str = "Success = passed / (graded + errored); pending (awaiting a judge) and skipped attempts are excluded. Score is normalised to 0-1, and a human score override wins when set. Intervals are 95% (Wilson for success with one try per task; resampled by task with repeats; normal approximation for score). Fake runners report approximate tokens (characters / 4).";

const RELATED_BENCHMARKS: &[(&str, &str, &str)] = &[
    (
        "Samiksha",
        "Microsoft Research India and Karya",
        "https://evals.karya.in/samiksha/",
    ),
    (
        "BharatBench",
        "Ola Krutrim",
        "https://tech.olakrutrim.com/bharat-bench/",
    ),
];

#[derive(Debug, Clone)]
pub struct RunnerRow {
    pub runner: String,
    pub kind: String,
    pub passed: i64,
    pub judged: i64,
    pub success: Option<f64>,
    pub success_ci: Option<(f64, f64)>,
    pub score: Option<(f64, f64, f64)>,
    pub pending: i64,
    pub errors: i64,
    pub skipped: i64,
    pub latency_s: Option<f64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub cost_inr: Option<f64>,
    pub tries_per_task: i64,
    pub always: i64,
    pub sometimes: i64,
    pub never: i64,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub run: RunRecord,
    pub rows: Vec<RunnerRow>,
    pub task_ids: Vec<String>,
    pub categories: BTreeMap<String, String>,
    pub runners: Vec<String>,
    pub cells: BTreeMap<(String, String), String>,
    pub verdicts: Vec<Verdict>,
    pub repeats: i64,
}

fn effective_score(r: &ResultRecord) -> Option<f64> {
    r.human_score.or(r.score)
}

pub fn build_report(store: &Store, run_id: &str) -> Result<Report, String> {
    let run = store
        .get_run(run_id)?
        .ok_or_else(|| format!("no run '{run_id}'"))?;
    let results = store.results_for_run(run_id)?;
    let mut rows = Vec::new();
    for runner in &run.runners {
        let rs: Vec<&ResultRecord> = results.iter().filter(|r| &r.runner == runner).collect();
        let graded: Vec<&ResultRecord> = rs
            .iter()
            .filter(|r| r.status == "ok" && r.grade_status.as_deref() == Some("graded"))
            .copied()
            .collect();
        let errors = rs.iter().filter(|r| r.status == "error").count() as i64;
        let passed = graded.iter().filter(|r| r.passed == Some(true)).count() as i64;
        let judged = graded.len() as i64 + errors;
        let scores: Vec<f64> = graded.iter().filter_map(|r| effective_score(r)).collect();
        let latencies: Vec<f64> = rs
            .iter()
            .filter(|r| r.status == "ok")
            .filter_map(|r| r.latency_s)
            .collect();
        // task_tries over export-shaped dicts (runner_id key).
        let dicts: Vec<Value> = rs
            .iter()
            .map(|r| {
                serde_json::json!({
                    "task_id": r.task_id, "runner_id": r.runner, "status": r.status,
                    "grade_status": r.grade_status, "passed": r.passed,
                    "latency_s": r.latency_s})
            })
            .collect();
        let tries = task_tries(&dicts);
        let repeated = tries.values().any(|(_, c)| *c > 1);
        let all_or_none = |pick: &dyn Fn(&ResultRecord) -> Option<f64>| -> Option<f64> {
            if rs.iter().all(|r| pick(r).is_some()) {
                Some(rs.iter().map(|r| pick(r).unwrap()).sum())
            } else {
                None
            }
        };
        let tokens = |pick: &dyn Fn(&ResultRecord) -> Option<i64>| -> Option<i64> {
            if rs
                .iter()
                .all(|r| pick(r).is_some() || r.status == "skipped")
            {
                Some(rs.iter().map(|r| pick(r).unwrap_or(0)).sum())
            } else {
                None
            }
        };
        rows.push(RunnerRow {
            runner: runner.clone(),
            kind: rs
                .first()
                .map(|r| r.runner_kind.clone())
                .unwrap_or_else(|| "?".into()),
            passed,
            judged,
            success: if judged > 0 {
                Some(passed as f64 / judged as f64)
            } else {
                None
            },
            success_ci: if repeated {
                let rates: Vec<f64> = tries.values().map(|(r, c)| *r as f64 / *c as f64).collect();
                bootstrap_ci_mean(&rates)
            } else {
                wilson_interval(passed, judged)
            },
            score: mean_ci(&scores),
            pending: rs
                .iter()
                .filter(|r| r.status == "ok" && r.grade_status.as_deref() != Some("graded"))
                .count() as i64,
            errors,
            skipped: rs.iter().filter(|r| r.status == "skipped").count() as i64,
            latency_s: if latencies.is_empty() {
                None
            } else {
                Some(mean(&latencies))
            },
            input_tokens: tokens(&|r| r.input_tokens),
            output_tokens: tokens(&|r| r.output_tokens),
            cost_usd: all_or_none(&|r| r.cost_usd),
            cost_inr: all_or_none(&|r| r.cost_inr),
            tries_per_task: tries.values().map(|(_, c)| *c).max().unwrap_or(1),
            always: tries.values().filter(|(r, c)| r == c).count() as i64,
            sometimes: tries.values().filter(|(r, c)| *r > 0 && r < c).count() as i64,
            never: tries.values().filter(|(r, _)| *r == 0).count() as i64,
        });
    }
    rows.sort_by(|a, b| {
        let ka = (
            a.success.unwrap_or(-1.0),
            a.score.map(|s| s.0).unwrap_or(-1.0),
        );
        let kb = (
            b.success.unwrap_or(-1.0),
            b.score.map(|s| s.0).unwrap_or(-1.0),
        );
        kb.0.partial_cmp(&ka.0)
            .unwrap()
            .then(kb.1.partial_cmp(&ka.1).unwrap())
    });
    let mut task_ids: Vec<String> = Vec::new();
    for r in &results {
        if !task_ids.contains(&r.task_id) {
            task_ids.push(r.task_id.clone());
        }
    }
    let mut by_cell: BTreeMap<(String, String), Vec<&ResultRecord>> = BTreeMap::new();
    for r in &results {
        by_cell
            .entry((r.task_id.clone(), r.runner.clone()))
            .or_default()
            .push(r);
    }
    let cells = by_cell
        .into_iter()
        .map(|(k, tries)| {
            let v = if tries.len() == 1 {
                cell(tries[0])
            } else {
                tries_cell(&tries)
            };
            (k, v)
        })
        .collect();
    let verdicts = result_export(store, run_id, false)?.verdicts;
    Ok(Report {
        run: run.clone(),
        rows,
        task_ids,
        categories: results
            .iter()
            .map(|r| (r.task_id.clone(), r.category.clone()))
            .collect(),
        runners: run.runners.clone(),
        cells,
        verdicts,
        repeats: run.repeats,
    })
}

pub fn cell(r: &ResultRecord) -> String {
    if r.status != "ok" {
        return r.status.clone();
    }
    if r.grade_status.as_deref() == Some("graded") {
        let mark = if r.passed == Some(true) { "✓" } else { "✗" };
        match effective_score(r) {
            Some(s) => format!("{mark} {s:.2}"),
            None => mark.to_string(),
        }
    } else if r.grade_status.as_deref() == Some("error") {
        "judge error".into()
    } else {
        "pending".into()
    }
}

pub fn tries_cell(tries: &[&ResultRecord]) -> String {
    let counted: Vec<&&ResultRecord> = tries
        .iter()
        .filter(|r| r.status == "error" || r.grade_status.as_deref() == Some("graded"))
        .collect();
    if counted.is_empty() {
        if tries.iter().any(|r| r.status == "ok") {
            return "pending".into();
        }
        return tries[0].status.clone();
    }
    let right = counted
        .iter()
        .filter(|r| r.status == "ok" && r.passed == Some(true))
        .count();
    let mark = if right == counted.len() {
        "✓"
    } else if right == 0 {
        "✗"
    } else {
        "◐"
    };
    format!("{mark} {right}/{}", counted.len())
}

pub const VERDICT_HEADERS: &[&str] = &[
    "Pair",
    "Measure",
    "Verdict",
    "Gap",
    "95% range",
    "Split",
    "p (adjusted)",
];

pub fn verdict_cells(v: &Verdict) -> Vec<String> {
    let mut split = format!("{}–{}", v.a_better, v.b_better);
    if v.tied != 0 {
        split += &format!(", {} tied", v.tied);
    }
    let (gap, span) = if v.metric == "accuracy" {
        let gap = match &v.favours {
            None => "none".to_string(),
            Some(f) => format!("{f} +{:.1} pts", v.estimate.abs()),
        };
        let span = match v.ci_95 {
            None => "–".to_string(),
            Some((lo, hi)) => format!("{lo:+.1} to {hi:+.1} pts"),
        };
        (gap, span)
    } else {
        let slower = if v.favours.as_deref() == Some(v.runner_a.as_str()) {
            &v.runner_b
        } else {
            &v.runner_a
        };
        let gap = match &v.favours {
            None => "none".to_string(),
            Some(_) => format!("{slower} takes {:.1}× as long", v.estimate),
        };
        let span = match v.ci_95 {
            None => "–".to_string(),
            Some((lo, hi)) => format!("{lo:.1}×–{hi:.1}×"),
        };
        (gap, span)
    };
    vec![
        format!("{} vs {}", v.runner_a, v.runner_b),
        if v.metric == "accuracy" {
            "Accuracy".into()
        } else {
            "Speed".into()
        },
        if v.verdict == "clearly_different" {
            "Clearly different".into()
        } else {
            "Too close to call".into()
        },
        gap,
        span,
        split,
        py_g3(v.p_value_adjusted),
    ]
}

/// Python format(x, ".3g").
pub fn py_g3(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    let exp = x.abs().log10().floor() as i32;
    if (-4..3).contains(&exp) {
        let decimals = (2 - exp).max(0) as usize;
        let s = format!("{x:.decimals$}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        let mantissa = x / 10f64.powi(exp);
        let m = format!("{mantissa:.2}");
        let m = m.trim_end_matches('0').trim_end_matches('.');
        format!("{m}e{exp:+03}")
    }
}

fn pct(x: Option<f64>) -> String {
    match x {
        None => "–".into(),
        Some(v) => format!("{:.0}%", v * 100.0),
    }
}

fn ci_text(ci: Option<(f64, f64)>) -> String {
    match ci {
        None => "–".into(),
        Some((lo, hi)) => format!("{:.0}%–{:.0}%", lo * 100.0, hi * 100.0),
    }
}

fn score_text(s: Option<(f64, f64, f64)>) -> String {
    match s {
        None => "–".into(),
        Some((m, lo, hi)) => format!("{m:.2} ({:.2}–{:.2})", lo.max(0.0), hi.min(1.0)),
    }
}

fn secs(x: Option<f64>) -> String {
    match x {
        None => "–".into(),
        Some(v) if v < 10.0 => format!("{v:.3}s"),
        Some(v) => format!("{v:.1}s"),
    }
}

pub const LEADERBOARD_HEADERS: &[&str] = &[
    "Runner",
    "Kind",
    "Success",
    "95% CI",
    "Mean score (95% CI)",
    "Pending",
    "Errors",
    "Skipped",
    "Mean latency",
    "Tokens in / out",
    "Cost USD",
    "Cost INR",
];

pub fn leaderboard_cells(row: &RunnerRow) -> Vec<String> {
    vec![
        row.runner.clone(),
        row.kind.clone(),
        format!("{} ({}/{})", pct(row.success), row.passed, row.judged),
        ci_text(row.success_ci),
        score_text(row.score),
        row.pending.to_string(),
        row.errors.to_string(),
        row.skipped.to_string(),
        secs(row.latency_s),
        format!(
            "{} / {}",
            row.input_tokens
                .map(thousands)
                .unwrap_or_else(|| "unknown".into()),
            row.output_tokens
                .map(thousands)
                .unwrap_or_else(|| "unknown".into())
        ),
        row.cost_usd
            .map(|c| format!("${c:.4}"))
            .unwrap_or_else(|| "unknown".into()),
        row.cost_inr
            .map(|c| format!("₹{c:.2}"))
            .unwrap_or_else(|| "unknown".into()),
    ]
}

fn thousands(n: i64) -> String {
    let neg = n < 0;
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    let s: String = out.chars().rev().collect();
    if neg {
        format!("-{s}")
    } else {
        s
    }
}

fn meta_line(run: &RunRecord) -> String {
    let spent_inr = run.spent_usd * run.usd_to_inr;
    let qualifier = if run.uncertain_cost {
        "known cost (total unknown)"
    } else {
        "token-priced cost"
    };
    format!(
        "Run `{}` · status **{}** · seed {} · {qualifier} ${:.4} (₹{spent_inr:.2}) of ${:.2} cap · km {}",
        run.id, run.status, run.seed, run.spent_usd, run.max_usd, run.mj_version
    )
}

fn md_table(headers: &[String], rows: &[Vec<String>]) -> Vec<String> {
    let mut lines = vec![
        format!("| {} |", headers.join(" | ")),
        format!(
            "|{}|",
            headers.iter().map(|_| "---").collect::<Vec<_>>().join("|")
        ),
    ];
    for r in rows {
        let esc: Vec<String> = r.iter().map(|c| c.replace('|', "\\|")).collect();
        lines.push(format!("| {} |", esc.join(" | ")));
    }
    lines
}

pub fn to_markdown(rep: &Report) -> String {
    let mut lines: Vec<String> = vec![
        format!(
            "# Krutrim Mulyankan report: {} v{}",
            rep.run.suite_name, rep.run.suite_version
        ),
        String::new(),
        meta_line(&rep.run),
        String::new(),
        "## Leaderboard".into(),
        String::new(),
    ];
    let headers: Vec<String> = LEADERBOARD_HEADERS.iter().map(|s| s.to_string()).collect();
    lines.extend(md_table(
        &headers,
        &rep.rows.iter().map(leaderboard_cells).collect::<Vec<_>>(),
    ));
    lines.push(String::new());
    lines.push(format!("_{DEFINITIONS}_"));
    lines.push(String::new());
    lines.extend(markdown_extras(rep));
    lines.push("## Per task".into());
    lines.push(String::new());
    let mut task_headers = vec!["Task".to_string(), "Category".to_string()];
    task_headers.extend(rep.runners.iter().cloned());
    let task_rows: Vec<Vec<String>> = rep
        .task_ids
        .iter()
        .map(|t| {
            let mut row = vec![t.clone(), rep.categories[t].clone()];
            for r in &rep.runners {
                row.push(
                    rep.cells
                        .get(&(t.clone(), r.clone()))
                        .cloned()
                        .unwrap_or_else(|| "–".into()),
                );
            }
            row
        })
        .collect();
    lines.extend(md_table(&task_headers, &task_rows));
    lines.join("\n") + "\n"
}

fn markdown_extras(rep: &Report) -> Vec<String> {
    let mut lines = Vec::new();
    if rep.repeats > 1 {
        lines.push(format!("## Consistency ({} tries per task)", rep.repeats));
        lines.push(String::new());
        lines.extend(md_table(
            &[
                "Runner".into(),
                "Right every time".into(),
                "Sometimes right".into(),
                "Never right".into(),
            ],
            &rep.rows
                .iter()
                .map(|r| {
                    vec![
                        r.runner.clone(),
                        r.always.to_string(),
                        r.sometimes.to_string(),
                        r.never.to_string(),
                    ]
                })
                .collect::<Vec<_>>(),
        ));
        lines.push(String::new());
    }
    if !rep.verdicts.is_empty() {
        lines.push("## Is the difference real?".into());
        lines.push(String::new());
        lines.extend(md_table(
            &VERDICT_HEADERS
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
            &rep.verdicts.iter().map(verdict_cells).collect::<Vec<_>>(),
        ));
        lines.push(String::new());
        lines.extend(rep.verdicts.iter().map(|v| format!("- {}", v.explanation)));
        lines.push(String::new());
        lines.push(
            "_Too close to call means the gap could be chance, not that the systems are equal._"
                .into(),
        );
        lines.push(String::new());
    }
    lines
}

const CSS: &str = r#"
:root { --bg:#fbfbfa; --fg:#1d1d1b; --muted:#6b6b66; --line:#e3e2de; --head:#f1f0ec;
        --pass:#1f7a3a; --fail:#b3261e; }
@media (prefers-color-scheme: dark) {
  :root { --bg:#161615; --fg:#ecebe7; --muted:#a19f98; --line:#33322f; --head:#21201e;
          --pass:#6fcf8a; --fail:#f2877e; } }
body { background:var(--bg); color:var(--fg); margin:0;
       font:15px/1.5 ui-sans-serif, system-ui, sans-serif; }
main { max-width:1100px; margin:0 auto; padding:32px 16px 64px; }
h1 { font-size:1.5rem; margin:0 0 .25rem; } h2 { font-size:1.1rem; margin:2rem 0 .5rem; }
.meta, .note { color:var(--muted); font-size:.9rem; }
.scroll { overflow-x:auto; }
table { border-collapse:collapse; width:100%; font-variant-numeric:tabular-nums; }
th, td { text-align:left; padding:6px 10px; border-bottom:1px solid var(--line);
         white-space:nowrap; }
th { background:var(--head); font-weight:600; }
code { font:0.9em ui-monospace, monospace; }
.pass { color:var(--pass); } .fail { color:var(--fail); } .other { color:var(--muted); }
"#;

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn html_table(headers: &[String], rows: &[Vec<String>], classes: Option<&[Vec<String>]>) -> String {
    let head: String = headers
        .iter()
        .map(|h| format!("<th>{}</th>", esc(h)))
        .collect();
    let mut body = String::new();
    for (i, r) in rows.iter().enumerate() {
        body.push_str("<tr>");
        for (j, c) in r.iter().enumerate() {
            match classes {
                Some(cl) => body.push_str(&format!("<td class=\"{}\">{}</td>", cl[i][j], esc(c))),
                None => body.push_str(&format!("<td>{}</td>", esc(c))),
            }
        }
        body.push_str("</tr>");
    }
    format!("<div class=\"scroll\"><table><tr>{head}</tr>{body}</table></div>")
}

pub fn to_html(rep: &Report) -> String {
    let run = &rep.run;
    let headers: Vec<String> = LEADERBOARD_HEADERS.iter().map(|s| s.to_string()).collect();
    let leaderboard = html_table(
        &headers,
        &rep.rows.iter().map(leaderboard_cells).collect::<Vec<_>>(),
        None,
    );
    let mut task_headers = vec!["Task".to_string(), "Category".to_string()];
    task_headers.extend(rep.runners.iter().cloned());
    let task_rows: Vec<Vec<String>> = rep
        .task_ids
        .iter()
        .map(|t| {
            let mut row = vec![t.clone(), rep.categories[t].clone()];
            for r in &rep.runners {
                row.push(
                    rep.cells
                        .get(&(t.clone(), r.clone()))
                        .cloned()
                        .unwrap_or_else(|| "–".into()),
                );
            }
            row
        })
        .collect();
    let task_classes: Vec<Vec<String>> = task_rows
        .iter()
        .map(|row| {
            let mut cl = vec![String::new(), String::new()];
            cl.extend(row[2..].iter().map(|c| {
                if c.starts_with('✓') {
                    "pass".to_string()
                } else if c.starts_with('✗') {
                    "fail".to_string()
                } else {
                    "other".to_string()
                }
            }));
            cl
        })
        .collect();
    let qualifier = if run.uncertain_cost {
        "known cost (total unknown)"
    } else {
        "token-priced cost"
    };
    let meta = format!(
        "Run <code>{}</code> · status <b>{}</b> · seed {} · {qualifier} ${:.4} (₹{:.2}) of ${:.2} cap · km {}",
        esc(&run.id),
        esc(&run.status),
        run.seed,
        run.spent_usd,
        run.spent_usd * run.usd_to_inr,
        run.max_usd,
        esc(&run.mj_version)
    );
    let title = format!(
        "Krutrim Mulyankan: {} v{}",
        run.suite_name, run.suite_version
    );
    let related: Vec<String> = RELATED_BENCHMARKS
        .iter()
        .map(|(name, org, url)| {
            format!("<a href=\"{}\">{}</a> ({})", esc(url), esc(name), esc(org))
        })
        .collect();
    let extras = html_extras(rep);
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{}</title>\n<style>{CSS}</style>\n</head>\n<body>\n<main>\n<h1>{}</h1>\n<p class=\"meta\">{meta}</p>\n<h2>Leaderboard</h2>\n{leaderboard}\n<p class=\"note\">{}</p>\n{extras}\n<h2>Per task</h2>\n{}\n<footer class=\"note\">Related benchmarks: {}</footer>\n</main>\n</body>\n</html>\n",
        esc(&title),
        esc(&title),
        esc(DEFINITIONS),
        html_table(&task_headers, &task_rows, Some(&task_classes)),
        related.join(" · "),
    )
}

fn html_extras(rep: &Report) -> String {
    let mut parts: Vec<String> = Vec::new();
    if rep.repeats > 1 {
        parts.push(format!(
            "<h2>Consistency ({} tries per task)</h2>",
            rep.repeats
        ));
        parts.push(html_table(
            &[
                "Runner".into(),
                "Right every time".into(),
                "Sometimes right".into(),
                "Never right".into(),
            ],
            &rep.rows
                .iter()
                .map(|r| {
                    vec![
                        r.runner.clone(),
                        r.always.to_string(),
                        r.sometimes.to_string(),
                        r.never.to_string(),
                    ]
                })
                .collect::<Vec<_>>(),
            None,
        ));
    }
    if !rep.verdicts.is_empty() {
        parts.push("<h2>Is the difference real?</h2>".into());
        parts.push(html_table(
            &VERDICT_HEADERS
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
            &rep.verdicts.iter().map(verdict_cells).collect::<Vec<_>>(),
            None,
        ));
        let notes: String = rep
            .verdicts
            .iter()
            .map(|v| format!("<li>{}</li>", esc(&v.explanation)))
            .collect();
        parts.push(format!("<ul class=\"note\">{notes}</ul>"));
        parts.push(
            "<p class=\"note\">Too close to call means the gap could be chance, not that the systems are equal.</p>".into(),
        );
    }
    parts.join("\n")
}
