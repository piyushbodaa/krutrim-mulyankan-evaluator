//! Statistics: Wilson/mean intervals and paired verdicts, mirroring
//! metrics.py and verdicts.py — including CPython-seeded bootstrap resampling
//! so 95% ranges match the Python implementation exactly.

use crate::pypy::PyRandom;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

pub const Z95: f64 = 1.959964;

pub fn wilson_interval(successes: i64, n: i64) -> Option<(f64, f64)> {
    wilson_interval_z(successes, n, Z95)
}

pub fn wilson_interval_z(successes: i64, n: i64, z: f64) -> Option<(f64, f64)> {
    if n <= 0 {
        return None;
    }
    let n = n as f64;
    let p = successes as f64 / n;
    let denom = 1.0 + z * z / n;
    let centre = (p + z * z / (2.0 * n)) / denom;
    let half = z * (p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt() / denom;
    Some(((centre - half).max(0.0), (centre + half).min(1.0)))
}

pub fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

pub fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

/// (mean, low, high) normal-approximation interval; Python statistics.stdev
/// is the sample standard deviation (n-1).
pub fn mean_ci(values: &[f64]) -> Option<(f64, f64, f64)> {
    if values.is_empty() {
        return None;
    }
    let m = mean(values);
    let half = if values.len() > 1 {
        let var: f64 =
            values.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / (values.len() - 1) as f64;
        Z95 * var.sqrt() / (values.len() as f64).sqrt()
    } else {
        0.0
    };
    Some((m, m - half, m + half))
}

pub const ALPHA: f64 = 0.05;
pub const RESAMPLES: usize = 4000;
pub const SEED: u64 = 20261014;
pub const EPSILON: f64 = 1e-12;
pub const METHOD: &str =
    "Exact two-sided sign test over paired tasks; 95% paired bootstrap range (4000 resamples).";

#[derive(Debug, Clone, Serialize)]
pub struct Verdict {
    pub metric: String,
    pub capability_track: String,
    pub runner_a: String,
    pub runner_b: String,
    pub verdict: String,
    pub favours: Option<String>,
    pub estimate: f64,
    pub estimate_unit: String,
    pub ci_95: Option<(f64, f64)>,
    pub p_value: f64,
    pub p_value_adjusted: f64,
    pub tasks: i64,
    pub a_better: i64,
    pub b_better: i64,
    pub tied: i64,
    pub tries_per_task: i64,
    pub alpha: f64,
    pub method: String,
    pub explanation: String,
}

fn comb(n: i64, k: i64) -> f64 {
    // Small n (task counts); compute with multiplicative formula in f64.
    if k < 0 || k > n {
        return 0.0;
    }
    let k = k.min(n - k);
    let mut acc = 1.0f64;
    for i in 0..k {
        acc = acc * (n - i) as f64 / (i + 1) as f64;
    }
    acc
}

pub fn sign_test(wins: i64, losses: i64) -> f64 {
    let n = wins + losses;
    if n == 0 {
        return 1.0;
    }
    let tail: f64 = (0..=wins.min(losses)).map(|k| comb(n, k)).sum();
    (2.0 * tail / 2f64.powi(n as i32)).min(1.0)
}

pub fn holm(p_values: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..p_values.len()).collect();
    order.sort_by(|&a, &b| p_values[a].partial_cmp(&p_values[b]).unwrap());
    let mut adjusted = vec![1.0; p_values.len()];
    let mut running = 0.0f64;
    for (rank, &index) in order.iter().enumerate() {
        running = running.max(((p_values.len() - rank) as f64 * p_values[index]).min(1.0));
        adjusted[index] = running;
    }
    adjusted
}

fn quantile(ordered: &[f64], q: f64) -> f64 {
    let position = q * (ordered.len() - 1) as f64;
    let (low, high) = (position.floor() as usize, position.ceil() as usize);
    ordered[low] + (ordered[high] - ordered[low]) * (position - low as f64)
}

/// 95% percentile interval from resampling whole tasks with CPython's RNG.
pub fn bootstrap_ci(values: &[f64]) -> Option<(f64, f64)> {
    bootstrap_ci_mean(values)
}

pub fn bootstrap_ci_mean(values: &[f64]) -> Option<(f64, f64)> {
    if values.len() < 2 {
        return None;
    }
    let mut rng = PyRandom::new(SEED);
    let mut samples: Vec<f64> = (0..RESAMPLES)
        .map(|_| {
            let picked = rng.choices(values, values.len());
            picked.iter().map(|v| **v).sum::<f64>() / values.len() as f64
        })
        .collect();
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some((quantile(&samples, 0.025), quantile(&samples, 0.975)))
}

/// Rows are export-shaped dictionaries: status, grade_status, passed, task_id,
/// runner_id, latency_s, capability_track. Per task: (right, counted).
pub fn task_tries(rows: &[Value]) -> BTreeMap<String, (i64, i64)> {
    let mut counts: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for row in rows {
        let task = row["task_id"].as_str().unwrap_or_default().to_string();
        let entry = counts.entry(task).or_insert((0, 0));
        if row["status"] == "error" {
            entry.1 += 1;
        } else if row["status"] == "ok"
            && row.get("grade_status") == Some(&Value::String("graded".into()))
        {
            if row.get("passed") == Some(&Value::Bool(true)) {
                entry.0 += 1;
            }
            entry.1 += 1;
        }
    }
    counts.retain(|_, (_, tries)| *tries > 0);
    counts
}

pub fn task_waits(rows: &[Value]) -> BTreeMap<String, f64> {
    let mut waits: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for row in rows {
        if row["status"] == "ok" {
            if let Some(lat) = row.get("latency_s").and_then(|v| v.as_f64()) {
                if lat > 0.0 {
                    waits
                        .entry(row["task_id"].as_str().unwrap_or_default().to_string())
                        .or_default()
                        .push(lat);
                }
            }
        }
    }
    waits.into_iter().map(|(k, v)| (k, median(&v))).collect()
}

struct Draft {
    metric: String,
    capability_track: String,
    runner_a: String,
    runner_b: String,
    favours: Option<String>,
    estimate: f64,
    estimate_unit: String,
    ci_95: Option<(f64, f64)>,
    p_value: f64,
    tasks: i64,
    a_better: i64,
    b_better: i64,
    tied: i64,
    tries_per_task: i64,
}

fn accuracy_draft(
    cap: &str,
    a: &str,
    b: &str,
    rows_a: &[Value],
    rows_b: &[Value],
) -> Option<Draft> {
    let (ta, tb) = (task_tries(rows_a), task_tries(rows_b));
    let shared: Vec<&String> = ta.keys().filter(|k| tb.contains_key(*k)).collect();
    if shared.is_empty() {
        return None;
    }
    let gaps: Vec<f64> = shared
        .iter()
        .map(|t| ta[*t].0 as f64 / ta[*t].1 as f64 - tb[*t].0 as f64 / tb[*t].1 as f64)
        .collect();
    let wins = gaps.iter().filter(|g| **g > EPSILON).count() as i64;
    let losses = gaps.iter().filter(|g| **g < -EPSILON).count() as i64;
    let centre = mean(&gaps);
    let interval = bootstrap_ci_mean(&gaps);
    Some(Draft {
        metric: "accuracy".into(),
        capability_track: cap.into(),
        runner_a: a.into(),
        runner_b: b.into(),
        favours: if centre > EPSILON {
            Some(a.into())
        } else if centre < -EPSILON {
            Some(b.into())
        } else {
            None
        },
        estimate: centre * 100.0,
        estimate_unit: "percentage_points".into(),
        ci_95: interval.map(|(lo, hi)| (lo * 100.0, hi * 100.0)),
        p_value: sign_test(wins, losses),
        tasks: shared.len() as i64,
        a_better: wins,
        b_better: losses,
        tied: shared.len() as i64 - wins - losses,
        tries_per_task: shared
            .iter()
            .map(|t| ta[*t].1.max(tb[*t].1))
            .max()
            .unwrap_or(0),
    })
}

fn speed_draft(cap: &str, a: &str, b: &str, rows_a: &[Value], rows_b: &[Value]) -> Option<Draft> {
    let (wa, wb) = (task_waits(rows_a), task_waits(rows_b));
    let shared: Vec<&String> = wa.keys().filter(|k| wb.contains_key(*k)).collect();
    if shared.is_empty() {
        return None;
    }
    let logs: Vec<f64> = shared.iter().map(|t| (wb[*t] / wa[*t]).ln()).collect();
    let wins = logs.iter().filter(|v| **v > EPSILON).count() as i64;
    let losses = logs.iter().filter(|v| **v < -EPSILON).count() as i64;
    let centre = mean(&logs);
    let interval = bootstrap_ci_mean(&logs);
    let (favours, estimate, ci) = if centre >= 0.0 {
        (
            if centre > EPSILON {
                Some(a.to_string())
            } else {
                None
            },
            centre.exp(),
            interval.map(|(lo, hi)| (lo.exp(), hi.exp())),
        )
    } else {
        (
            Some(b.to_string()),
            (-centre).exp(),
            interval.map(|(lo, hi)| ((-hi).exp(), (-lo).exp())),
        )
    };
    let mut tries: BTreeMap<(String, String), i64> = BTreeMap::new();
    for row in rows_a.iter().chain(rows_b.iter()) {
        *tries
            .entry((
                row["runner_id"].as_str().unwrap_or_default().into(),
                row["task_id"].as_str().unwrap_or_default().into(),
            ))
            .or_insert(0) += 1;
    }
    Some(Draft {
        metric: "speed".into(),
        capability_track: cap.into(),
        runner_a: a.into(),
        runner_b: b.into(),
        favours,
        estimate,
        estimate_unit: "times_as_long".into(),
        ci_95: ci,
        p_value: sign_test(wins, losses),
        tasks: shared.len() as i64,
        a_better: wins,
        b_better: losses,
        tied: shared.len() as i64 - wins - losses,
        tries_per_task: tries.values().copied().max().unwrap_or(0),
    })
}

fn plural(count: i64, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

fn p_text(p: f64) -> String {
    if p < 0.001 {
        "p < 0.001".into()
    } else {
        format!("p = {p:.3}")
    }
}

fn split_text(d: &Draft) -> String {
    let better = if d.metric == "accuracy" {
        "did better"
    } else {
        "was faster"
    };
    let tied = if d.tied != 0 {
        format!("; {} tied.", d.tied)
    } else {
        ".".to_string()
    };
    if d.a_better != 0 && d.b_better != 0 {
        return format!(
            "{} {better} on {} and {} on {}{tied}",
            d.runner_a,
            plural(d.a_better, "task"),
            d.runner_b,
            d.b_better
        );
    }
    let (leader, count) = if d.a_better != 0 {
        (&d.runner_a, d.a_better)
    } else {
        (&d.runner_b, d.b_better)
    };
    if d.tied == 0 {
        format!("{leader} {better} on all {}.", plural(count, "task"))
    } else {
        format!("{leader} {better} on {}{tied}", plural(count, "task"))
    }
}

fn explain(d: &Draft, verdict: &str, p_adjusted: f64) -> String {
    let differing = d.a_better + d.b_better;
    if differing == 0 {
        return format!(
            "{} and {} matched on all {}.",
            d.runner_a,
            d.runner_b,
            plural(d.tasks, "task")
        );
    }
    let split = split_text(d);
    if verdict == "clearly_different" {
        return format!(
            "{split} A split this one-sided is unlikely if they were equally good ({}).",
            p_text(p_adjusted)
        );
    }
    if 2.0 * 0.5f64.powi(differing as i32) >= ALPHA {
        return format!(
            "{split} Only {} differed, too few to rule out chance.",
            plural(differing, "task")
        );
    }
    format!(
        "{split} A split like this is common between equally good systems ({}).",
        p_text(p_adjusted)
    )
}

/// models: export model dicts (runner_id, evaluation_track).
pub fn paired_verdicts(results: &[Value], models: &[Value]) -> Vec<Verdict> {
    let order: Vec<String> = models
        .iter()
        .map(|m| m["runner_id"].as_str().unwrap_or_default().to_string())
        .collect();
    let track = |id: &str| -> String {
        models
            .iter()
            .find(|m| m["runner_id"].as_str() == Some(id))
            .and_then(|m| m.get("evaluation_track"))
            .and_then(|v| v.as_str())
            .unwrap_or("unverified")
            .to_string()
    };
    let mut rows: BTreeMap<(String, String), Vec<Value>> = BTreeMap::new();
    for row in results {
        let cap = row
            .get("capability_track")
            .and_then(|v| v.as_str())
            .unwrap_or("text")
            .to_string();
        let rid = row["runner_id"].as_str().unwrap_or_default().to_string();
        rows.entry((cap, rid)).or_default().push(row.clone());
    }
    let mut verdicts = Vec::new();
    let caps: Vec<String> = {
        let mut seen: Vec<String> = rows.keys().map(|(c, _)| c.clone()).collect();
        seen.sort();
        seen.dedup();
        seen
    };
    for cap in &caps {
        let present: Vec<&String> = order
            .iter()
            .filter(|r| rows.contains_key(&(cap.clone(), (*r).clone())))
            .collect();
        let mut pairs: Vec<(&String, &String)> = Vec::new();
        for i in 0..present.len() {
            for j in i + 1..present.len() {
                if track(present[i]) == track(present[j]) {
                    pairs.push((present[i], present[j]));
                }
            }
        }
        for build in [
            accuracy_draft as fn(&str, &str, &str, &[Value], &[Value]) -> Option<Draft>,
            speed_draft,
        ] {
            let drafts: Vec<Draft> = pairs
                .iter()
                .filter_map(|(a, b)| {
                    build(
                        cap,
                        a,
                        b,
                        &rows[&(cap.clone(), (*a).clone())],
                        &rows[&(cap.clone(), (*b).clone())],
                    )
                })
                .collect();
            let adjusted = holm(&drafts.iter().map(|d| d.p_value).collect::<Vec<_>>());
            for (draft, adj) in drafts.into_iter().zip(adjusted) {
                let outcome = if adj < ALPHA {
                    "clearly_different"
                } else {
                    "too_close_to_call"
                };
                let explanation = explain(&draft, outcome, adj);
                verdicts.push(Verdict {
                    metric: draft.metric,
                    capability_track: draft.capability_track,
                    runner_a: draft.runner_a,
                    runner_b: draft.runner_b,
                    verdict: outcome.into(),
                    favours: draft.favours,
                    estimate: draft.estimate,
                    estimate_unit: draft.estimate_unit,
                    ci_95: draft.ci_95,
                    p_value: draft.p_value,
                    p_value_adjusted: adj,
                    tasks: draft.tasks,
                    a_better: draft.a_better,
                    b_better: draft.b_better,
                    tied: draft.tied,
                    tries_per_task: draft.tries_per_task,
                    alpha: ALPHA,
                    method: METHOD.into(),
                    explanation,
                });
            }
        }
    }
    verdicts
}
