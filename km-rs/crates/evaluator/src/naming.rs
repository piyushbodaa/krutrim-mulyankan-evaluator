//! Plain names and amounts shared by every km surface, so the terminal and
//! the desktop app say the same thing: what an AI is called, what a
//! ready-made test is called, how rupees and durations are written, and how
//! a comparison between two AIs reads.

use crate::suite::Suite;
use crate::verdicts::Verdict;

/// "claude-haiku-4-5" -> "Claude Haiku 4.5"; other ids stay as they are.
pub fn pretty_model(model: &str) -> String {
    let parts: Vec<&str> = model.split('-').collect();
    let numeric = |s: &&str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    match parts.as_slice() {
        ["claude", family, major, rest @ ..]
            if family.chars().all(|c| c.is_ascii_lowercase())
                && numeric(major)
                && rest.len() <= 1
                && rest.iter().all(numeric) =>
        {
            let mut name = format!(
                "Claude {}{} {major}",
                family[..1].to_uppercase(),
                &family[1..]
            );
            if let Some(minor) = rest.first() {
                name.push('.');
                name.push_str(minor);
            }
            name
        }
        _ => model.to_string(),
    }
}

pub fn model_blurb(model: &str) -> &'static str {
    match model {
        "claude-haiku-4-5" | "claude-haiku-5-5" => "A smaller Claude model. Lower price per token.",
        "claude-sonnet-4-6" | "claude-sonnet-5" | "claude-sonnet-5-5" => "A mid-size Claude model.",
        "claude-opus-4-6" | "claude-opus-4-7" | "claude-opus-4-8" | "claude-opus-5"
        | "claude-opus-5-5" => "A larger Claude model. Higher price per token.",
        _ => "Priced from the checked price table.",
    }
}

/// Name, Hindi name, badge, summary and Hindi summary of a ready-made test.
pub fn pack_copy(suite: &Suite) -> (String, String, String, String, String) {
    match suite.name.as_str() {
        "india-pilot" => (
            "India quick check".into(), "भारत झटपट जाँच".into(), "Pilot set".into(),
            "Adding rupees, removing a repeated entry, and noticing a missing date.".into(),
            "रुपये जोड़ना, दोहराई गई एंट्री हटाना और गायब तारीख पहचानना।".into(),
        ),
        "railway-pilot" => (
            "Railway booking check".into(), "रेलवे बुकिंग जाँच".into(), "Extra fixture".into(),
            "Fill in a train journey plan correctly.".into(),
            "ट्रेन यात्रा की योजना सही ढंग से भरना।".into(),
        ),
        "example" => (
            "Mixed starter questions".into(), "मिले-जुले शुरुआती सवाल".into(), "Pipeline check".into(),
            "Maths, a line of code, picking the right tool, a bill date, and a short reply that a person marks.".into(),
            "गणित, कोड की एक लाइन, सही टूल चुनना, बिल की तारीख, और एक छोटा जवाब जिसे कोई व्यक्ति जाँचता है।".into(),
        ),
        _ => (
            suite.name.clone(), suite.name.clone(), suite.validation_status.clone(),
            suite.description.clone(), suite.description.clone(),
        ),
    }
}

/// What people call a runner: the two free demo bots by their app names,
/// priced models by their product names, anything else as written.
pub fn runner_label(spec: &str) -> String {
    match spec {
        "oracle" => "Answer-key bot".into(),
        "echo" => "Parrot bot".into(),
        _ => spec
            .strip_prefix("model:")
            .and_then(|rest| rest.split(',').next())
            .and_then(|target| target.split_once('/'))
            .map(|(_, model)| pretty_model(model))
            .unwrap_or_else(|| spec.to_string()),
    }
}

/// Rupees as the app writes them: two decimals under ₹100, at most two
/// above, Indian digit grouping, and "under ₹0.01" for tiny amounts.
pub fn rupees(usd: f64, usd_to_inr: f64) -> String {
    let inr = usd * usd_to_inr;
    if inr > 0.0 && inr < 0.01 {
        return "under ₹0.01".into();
    }
    if inr < 100.0 {
        return format!("₹{inr:.2}");
    }
    let fixed = format!("{inr:.2}");
    let (whole, fraction) = fixed.split_once('.').unwrap_or((&fixed, ""));
    let fraction = fraction.trim_end_matches('0');
    let mut groups: Vec<&str> = Vec::new();
    let (mut head, tail) = whole.split_at(whole.len().saturating_sub(3));
    while head.len() > 2 {
        let (rest, pair) = head.split_at(head.len() - 2);
        groups.insert(0, pair);
        head = rest;
    }
    if !head.is_empty() {
        groups.insert(0, head);
    }
    groups.push(tail);
    let grouped = groups.join(",");
    if fraction.is_empty() {
        format!("₹{grouped}")
    } else {
        format!("₹{grouped}.{fraction}")
    }
}

/// Seconds as the app writes them: "under 1 s", "2.8 s", "14 s", "2 min 5 s".
pub fn duration_label(seconds: f64) -> String {
    let s = seconds.max(0.0);
    if s < 1.0 {
        "under 1 s".into()
    } else if s < 10.0 {
        format!("{s:.1} s")
    } else if s < 60.0 {
        format!("{} s", s.round())
    } else {
        format!("{} min {} s", (s / 60.0).floor(), (s % 60.0).round())
    }
}

/// How one accuracy comparison reads, the same way on every surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlainVerdict {
    /// Both got exactly the same questions right.
    Same { a: String, b: String },
    /// The gap is too small to be sure; it could be chance.
    TooClose { a: String, b: String },
    /// One got clearly more right than the other.
    Clear { winner: String, loser: String },
}

pub fn plain_verdict(verdict: &Verdict) -> PlainVerdict {
    let a = verdict.runner_a.clone();
    let b = verdict.runner_b.clone();
    if verdict.a_better == 0 && verdict.b_better == 0 {
        PlainVerdict::Same { a, b }
    } else if verdict.verdict == "clearly_different" {
        if verdict.favours.as_deref() == Some(verdict.runner_a.as_str()) {
            PlainVerdict::Clear {
                winner: a,
                loser: b,
            }
        } else {
            PlainVerdict::Clear {
                winner: b,
                loser: a,
            }
        }
    } else {
        PlainVerdict::TooClose { a, b }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_the_app() {
        assert_eq!(pretty_model("claude-haiku-4-5"), "Claude Haiku 4.5");
        assert_eq!(pretty_model("claude-opus-5"), "Claude Opus 5");
        assert_eq!(pretty_model("claude-fable-5-1"), "Claude Fable 5.1");
        assert_eq!(pretty_model("gpt-4.1-mini"), "gpt-4.1-mini");
        assert_eq!(runner_label("oracle"), "Answer-key bot");
        assert_eq!(runner_label("echo"), "Parrot bot");
        assert_eq!(
            runner_label("model:anthropic/claude-sonnet-5-5"),
            "Claude Sonnet 5.5"
        );
        assert_eq!(
            runner_label("model:anthropic/claude-sonnet-5-5,temperature=0"),
            "Claude Sonnet 5.5"
        );
        assert_eq!(runner_label("harness:claude-draft"), "harness:claude-draft");
    }

    #[test]
    fn rupees_match_the_app() {
        assert_eq!(rupees(0.0, 88.0), "₹0.00");
        assert_eq!(rupees(0.00001, 88.0), "under ₹0.01");
        assert_eq!(rupees(0.00318, 88.0), "₹0.28");
        assert_eq!(rupees(1.0, 88.0), "₹88.00");
        assert_eq!(rupees(2.0, 88.0), "₹176");
        assert_eq!(rupees(1.5, 100.5), "₹150.75");
        assert_eq!(rupees(1234.5678, 100.0), "₹1,23,456.78");
        assert_eq!(rupees(10_000_000.0, 1.0), "₹1,00,00,000");
    }

    #[test]
    fn durations_match_the_app() {
        assert_eq!(duration_label(0.4), "under 1 s");
        assert_eq!(duration_label(2.84), "2.8 s");
        assert_eq!(duration_label(14.4), "14 s");
        assert_eq!(duration_label(125.0), "2 min 5 s");
    }
}
