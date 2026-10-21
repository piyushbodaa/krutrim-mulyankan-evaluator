//! Reading a reply the way a person would, for the checks behind tests people
//! write in the desktop app (`grader: {type: answer}`).
//!
//! The pilot graders stay strict on purpose: their suites are frozen and must
//! score the same forever. A person's own test needs the opposite. An AI that
//! answers "The total is ₹2,500/-." to "how much?" got it right, "Tuesday." is
//! Tuesday, and a reply of "15" does not mention "5".

use crate::graders::{indic_number, nd_digit, Grade};
use crate::suite::AnswerCheck;
use regex::Regex;
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Map, Value};
use std::str::FromStr;
use std::sync::OnceLock;
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// How much of a reply the "found" detail keeps for the results screen.
const FOUND_CHARS: usize = 120;

/// What the check read from the reply, and which right answer it matched.
struct Reading {
    found: Option<String>,
    matched: Option<String>,
}

pub fn grade(
    check: AnswerCheck,
    expected: &[String],
    tolerance: Option<Decimal>,
    output: &str,
) -> Grade {
    let reading = match check {
        AnswerCheck::Number => read_number(output, expected, tolerance),
        AnswerCheck::Exact => read_exact(output, expected),
        AnswerCheck::Mentions => read_mentions(output, expected),
    };
    let passed = reading.matched.is_some();
    let mut details = Map::new();
    details.insert("check".into(), Value::String(check.as_str().into()));
    details.insert(
        "found".into(),
        reading.found.map(Value::String).unwrap_or(Value::Null),
    );
    details.insert(
        "matched".into(),
        reading.matched.map(Value::String).unwrap_or(Value::Null),
    );
    Grade::graded("answer", passed, details)
}

// ---- text, the way a person reads it ----

/// NFKC, lower case, every Unicode digit as ASCII, and no markdown emphasis
/// or code marks (`**2500**`, `_Tuesday_`, `` `x` ``, `~~y~~`).
fn plain(text: &str) -> String {
    text.nfkc()
        .flat_map(|c| c.to_lowercase())
        .map(|c| nd_digit(c).unwrap_or(c))
        .filter(|c| !matches!(c, '*' | '_' | '`' | '~'))
        .collect()
}

fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

const WRAPPERS: &[char] = &[
    '"', '\'', '“', '”', '‘', '’', '«', '»', '(', ')', '[', ']', '{', '}', '<', '>',
];
const ENDINGS: &[char] = &['.', ',', '!', '?', ';', ':', '।', '॥', '…'];

/// An answer without the quotes, brackets, bullets and full stop around it.
fn bare(text: &str) -> String {
    let mut s = text.trim();
    for bullet in ["- ", "• "] {
        s = s.strip_prefix(bullet).unwrap_or(s);
    }
    loop {
        let before = s.len();
        s = s.trim_end_matches(|c: char| ENDINGS.contains(&c) || c.is_whitespace());
        s = s.trim_matches(|c: char| WRAPPERS.contains(&c) || c.is_whitespace());
        if s.len() == before {
            return s.to_string();
        }
    }
}

fn clip(text: &str) -> String {
    let mut out: String = text.chars().take(FOUND_CHARS).collect();
    if text.chars().count() > FOUND_CHARS {
        out.push('…');
    }
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || is_combining_mark(c)
}

/// Words that introduce a final answer, in English and Hindi.
fn cue_re() -> &'static Regex {
    static CUE: OnceLock<Regex> = OnceLock::new();
    CUE.get_or_init(|| {
        Regex::new(
            r"(?:\b(?:final answer|answer|total|result|equals|comes to|उत्तर|जवाब|कुल)\b|=|≈)\s*(?:is|would be|will be|:|-|–|—|=|है|होगा)?\s*",
        )
        .unwrap()
    })
}

// ---- number ----

struct Number {
    start: usize,
    end: usize,
    value: Decimal,
    text: String,
}

fn number_re() -> &'static Regex {
    static NUMBER: OnceLock<Regex> = OnceLock::new();
    NUMBER.get_or_init(|| {
        Regex::new(
            r"(?P<sign>[+-])?(?P<num>[0-9](?:[0-9,]*[0-9])?(?:\.[0-9]+)?)(?:\s*%|\s*(?P<unit>lakhs?|lacs?|crores?|लाख|करोड़|करोड|thousand|million|billion|cr|mn|bn)\b)?",
        )
        .unwrap()
    })
}

fn grouping_ok(digits: &str) -> bool {
    static GROUPING: OnceLock<Regex> = OnceLock::new();
    GROUPING
        .get_or_init(|| {
            Regex::new(
                r"^(?:[0-9]{1,3}(?:,[0-9]{3})+|[0-9]{1,2}(?:,[0-9]{2})*,[0-9]{3})(?:\.[0-9]+)?$",
            )
            .unwrap()
        })
        .is_match(digits)
}

fn multiplier(unit: Option<&str>) -> Decimal {
    match unit {
        Some("lakh" | "lakhs" | "lac" | "lacs" | "लाख") => Decimal::from(100_000),
        Some("crore" | "crores" | "cr" | "करोड़" | "करोड") => {
            Decimal::from(10_000_000)
        }
        Some("thousand") => Decimal::from(1_000),
        Some("million" | "mn") => Decimal::from(1_000_000),
        Some("billion" | "bn") => Decimal::from(1_000_000_000),
        _ => Decimal::ONE,
    }
}

/// Every number in already-plain text: "₹2,500/-" is 2500, "2.5 lakh" is
/// 250000, and the hyphen in "COVID-19" is not a minus sign.
fn numbers(text: &str) -> Vec<Number> {
    let mut found = Vec::new();
    for caps in number_re().captures_iter(text) {
        let whole = caps.get(0).unwrap();
        let num = caps.name("num").unwrap();
        let negative = caps.name("sign").is_some_and(|sign| {
            sign.as_str() == "-"
                && !text[..sign.start()]
                    .chars()
                    .next_back()
                    .is_some_and(is_word_char)
        });
        let unit = caps.name("unit").map(|u| u.as_str());
        let raw = num.as_str();
        // "1,2,3" is a list, not a number with odd grouping.
        let parts: Vec<(usize, &str)> = if raw.contains(',') && !grouping_ok(raw) {
            let mut at = num.start();
            raw.split(',')
                .map(|part| {
                    let start = at;
                    at += part.len() + 1;
                    (start, part)
                })
                .filter(|(_, part)| !part.is_empty())
                .collect()
        } else {
            vec![(num.start(), raw)]
        };
        let last = parts.len().saturating_sub(1);
        for (index, (start, part)) in parts.into_iter().enumerate() {
            let Ok(mut value) = Decimal::from_str(&part.replace(',', "")) else {
                continue;
            };
            if index == last {
                value *= multiplier(unit);
            }
            if negative && index == 0 {
                value = -value;
            }
            let end = if index == last {
                whole.end()
            } else {
                start + part.len()
            };
            found.push(Number {
                start,
                end,
                value,
                text: text[start..end].trim().to_string(),
            });
        }
    }
    found
}

/// The number a reader would take as the answer: inside \boxed{}, else the
/// first number after the last "answer is" / "total" / "=", else the last
/// number in the reply.
fn final_number(text: &str) -> Option<Number> {
    let mut all = numbers(text);
    if all.is_empty() {
        return None;
    }
    static BOXED: OnceLock<Regex> = OnceLock::new();
    let boxed = BOXED.get_or_init(|| Regex::new(r"\\boxed\{([^{}]*)\}").unwrap());
    if let Some(inside) = boxed.captures_iter(text).last().and_then(|c| c.get(1)) {
        if let Some(index) = all
            .iter()
            .rposition(|n| n.start >= inside.start() && n.end <= inside.end() + 12)
        {
            return Some(all.swap_remove(index));
        }
    }
    let mut cued = None;
    for cue in cue_re().find_iter(text) {
        let line_end = text[cue.end()..]
            .find('\n')
            .map(|i| cue.end() + i)
            .unwrap_or(text.len());
        if let Some(index) = all.iter().position(|n| {
            n.start >= cue.end()
                && n.start <= line_end
                && text[cue.end()..n.start].chars().count() <= 40
        }) {
            cued = Some(index);
        }
    }
    let index = cued.unwrap_or(all.len() - 1);
    Some(all.swap_remove(index))
}

/// Equal at the precision the right answer was written with: "33.33" accepts
/// 33.333, but a whole-number answer like 100 needs exactly 100.
fn same(found: Decimal, want: Decimal, tolerance: Option<Decimal>) -> bool {
    if let Some(tolerance) = tolerance {
        return (found - want).abs() <= tolerance;
    }
    let places = want.normalize().scale();
    if places == 0 {
        return found == want;
    }
    found.round_dp_with_strategy(places, RoundingStrategy::MidpointAwayFromZero) == want
}

fn read_number(output: &str, expected: &[String], tolerance: Option<Decimal>) -> Reading {
    let text = plain(output);
    let Some(found) = final_number(&text) else {
        return Reading {
            found: None,
            matched: None,
        };
    };
    let matched = expected.iter().find(|want| {
        indic_number(want)
            .map(|value| same(found.value, value, tolerance))
            .unwrap_or(false)
    });
    Reading {
        found: Some(clip(&found.text)),
        matched: matched.cloned(),
    }
}

// ---- exact ----

/// The reply as a short answer: the whole reply, its first and last lines,
/// and whatever follows "answer:" all count.
fn exact_candidates(output: &str) -> Vec<String> {
    let text = plain(output);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut candidates = vec![text.clone()];
    if let Some(first) = lines.first() {
        candidates.push(first.to_string());
    }
    if let Some(last) = lines.last() {
        candidates.push(last.to_string());
    }
    for line in &lines {
        if let Some(cue) = cue_re().find_iter(line).last() {
            candidates.push(line[cue.end()..].to_string());
        }
    }
    let mut out: Vec<String> = Vec::new();
    for candidate in candidates {
        let candidate = bare(&squash(&candidate));
        if !candidate.is_empty() && !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
}

fn read_exact(output: &str, expected: &[String]) -> Reading {
    let candidates = exact_candidates(output);
    let matched = expected.iter().find(|want| {
        let want = bare(&squash(&plain(want)));
        candidates.contains(&want)
    });
    Reading {
        found: candidates.first().map(|c| clip(c)),
        matched: matched.cloned(),
    }
}

// ---- mentions ----

/// Whole words only: "Delhi" is in "New Delhi." but not in "Delhites".
fn has_phrase(text: &str, phrase: &str) -> bool {
    if phrase.is_empty() {
        return false;
    }
    let starts_word = phrase.chars().next().is_some_and(is_word_char);
    let ends_word = phrase.chars().next_back().is_some_and(is_word_char);
    text.match_indices(phrase).any(|(at, _)| {
        let before_ok = !starts_word || !text[..at].chars().next_back().is_some_and(is_word_char);
        let after_ok = !ends_word
            || !text[at + phrase.len()..]
                .chars()
                .next()
                .is_some_and(is_word_char);
        before_ok && after_ok
    })
}

fn read_mentions(output: &str, expected: &[String]) -> Reading {
    let text = squash(&plain(output));
    let all = numbers(&text);
    for want in expected {
        // A number is mentioned when its value is: "₹2,500" mentions 2500,
        // and "15" does not mention 5.
        if let Ok(value) = indic_number(want) {
            if let Some(hit) = all.iter().find(|n| same(n.value, value, None)) {
                return Reading {
                    found: Some(clip(&hit.text)),
                    matched: Some(want.clone()),
                };
            }
            continue;
        }
        let phrase = squash(&plain(want));
        let phrase = phrase.trim_end_matches(|c: char| ENDINGS.contains(&c) || c.is_whitespace());
        if has_phrase(&text, phrase) {
            return Reading {
                found: Some(clip(phrase)),
                matched: Some(want.clone()),
            };
        }
    }
    Reading {
        found: None,
        matched: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(reply: &str, want: &str) -> bool {
        grade(AnswerCheck::Number, &[want.into()], None, reply).passed == Some(true)
    }
    fn exact(reply: &str, want: &str) -> bool {
        grade(AnswerCheck::Exact, &[want.into()], None, reply).passed == Some(true)
    }
    fn mentions(reply: &str, want: &str) -> bool {
        grade(AnswerCheck::Mentions, &[want.into()], None, reply).passed == Some(true)
    }

    #[test]
    fn numbers_read_like_a_shopkeeper_reads_them() {
        for reply in [
            "2500",
            "₹2,500",
            "2500.",
            "**2500**",
            "₹2,500/-",
            "Rs. 2500",
            "Rs.2500/-",
            "The total is ₹2,500.",
            "2,500 rupees",
            "₹2500.00",
            "२५००",
            "कुल ₹२,५०० होगा।",
            "40 × 62.5 = 2500",
            "The answer is 2500, because 40 × 62.5 = 2500.",
            "Answer: 2500\n\n(2.5 kg at ₹1,000 a kg)",
            "$\\boxed{2500}$",
            "₹0.025 lakh",
        ] {
            assert!(number(reply, "2500"), "{reply}");
        }
        assert!(number("₹2.5 lakh", "250000"));
        assert!(number("2,50,000", "₹2.5 lakh"));
        assert!(number("It will be -5 degrees", "-5"));
        assert!(number("33.333", "33.33"));
    }

    #[test]
    fn wrong_numbers_stay_wrong() {
        for reply in [
            "2400",
            "₹25,000",
            "The answer is 2400. Some say 2500.",
            "40 × 2.5 = 100",
            "No idea.",
            "",
            "COVID-19 changed prices",
        ] {
            assert!(!number(reply, "2500"), "{reply}");
        }
        assert!(
            !number("100.4", "100"),
            "whole-number answers need the whole number"
        );
        assert!(!number("33.3", "33.33"), "less precise than the answer key");
        assert!(!number("1,2,3", "123"), "a list is not a number");
        assert!(!number("COVID-19", "-19"));
    }

    #[test]
    fn the_found_number_is_reported() {
        let g = grade(
            AnswerCheck::Number,
            &["2500".into()],
            None,
            "So the total comes to ₹2,400/-",
        );
        assert_eq!(g.passed, Some(false));
        assert_eq!(g.details["found"], "2,400");
        assert_eq!(g.details["matched"], Value::Null);
    }

    #[test]
    fn tolerance_widens_the_match() {
        let g = grade(
            AnswerCheck::Number,
            &["100".into()],
            Some(Decimal::from(2)),
            "about 98",
        );
        assert_eq!(g.passed, Some(true));
    }

    #[test]
    fn exact_ignores_the_wrapping_but_not_the_word() {
        for reply in [
            "Tuesday",
            "Tuesday.",
            "**Tuesday**",
            "\"Tuesday\"",
            "  tuesday ",
            "Answer: Tuesday",
            "Tuesday\n\nIt is two days after Sunday.",
            "Let me count the days.\nTuesday",
        ] {
            assert!(exact(reply, "Tuesday"), "{reply}");
        }
        for reply in ["Monday", "Tuesday or Wednesday", "Not Tuesday", "Tuesdays"] {
            assert!(!exact(reply, "Tuesday"), "{reply}");
        }
        assert!(exact("मंगलवार।", "मंगलवार"));
    }

    #[test]
    fn mentions_means_whole_words() {
        assert!(mentions("The capital is New Delhi.", "Delhi"));
        assert!(mentions("**Jaipur** is the Pink City", "jaipur"));
        assert!(mentions("दिल्ली भारत की राजधानी है", "दिल्ली"));
        assert!(mentions("It costs ₹2,500 in total", "2500"));
        assert!(mentions("Use C++ for this", "C++"));
        assert!(!mentions("The answer is 15", "5"));
        assert!(!mentions("50", "5"));
        assert!(!mentions("Delhites love food", "Delhi"));
        assert!(!mentions("दिल्लीवाले", "दिल्ली"));
        assert!(!mentions("", "Delhi"));
    }

    #[test]
    fn suites_load_answer_checks_and_the_answer_key_bot_passes_them() {
        let doc: serde_yaml::Value = serde_yaml::from_str(
            "name: my-shop\nversion: '1'\ndescription: Shop maths\ntasks:\n\
             - {id: Q01, version: 1, category: c, prompt: 'How much?', grader: {type: answer, check: number, expected: '₹2,500'}}\n\
             - {id: Q02, version: 1, category: c, prompt: 'Which day?', grader: {type: answer, check: exact, expected: [Tuesday, Tue]}}\n\
             - {id: Q03, version: 1, category: c, prompt: 'Capital?', grader: {type: answer, check: mentions, expected: Delhi}}\n",
        )
        .unwrap();
        let suite = crate::suite::parse_suite(&doc, std::path::Path::new("my-shop.yaml")).unwrap();
        let oracle =
            crate::make_runner("oracle", &crate::PriceTable::default(), &Default::default())
                .unwrap();
        for task in &suite.tasks {
            let reply = oracle.run(task, &task.render_prompt(), &Default::default());
            let grade = crate::graders::grade(task, &reply.output, std::path::Path::new("."));
            assert_eq!(grade.passed, Some(true), "{}", task.id);
            assert_eq!(grade.grader, "answer");
        }
        assert_eq!(
            suite.tasks[1].dump_json()["grader"],
            serde_json::json!({"type": "answer", "check": "exact",
                               "expected": ["Tuesday", "Tue"], "tolerance": null})
        );
        for bad in [
            "{type: answer, check: vibes, expected: x}",
            "{type: answer, check: number, expected: 'two'}",
            "{type: answer, check: exact, expected: x, tolerance: 1}",
            "{type: answer, check: mentions, expected: ''}",
        ] {
            let doc: serde_yaml::Value = serde_yaml::from_str(&format!(
                "name: x\nversion: '1'\ndescription: x\ntasks:\n- {{id: Q01, version: 1, category: c, prompt: p, grader: {bad}}}\n"
            ))
            .unwrap();
            assert!(
                crate::suite::parse_suite(&doc, std::path::Path::new("x.yaml")).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn any_listed_answer_counts() {
        let answers = ["New Delhi".to_string(), "Delhi".to_string()];
        let g = grade(AnswerCheck::Mentions, &answers, None, "It is Delhi.");
        assert_eq!(g.passed, Some(true));
        assert_eq!(g.details["matched"], "Delhi");
    }
}
