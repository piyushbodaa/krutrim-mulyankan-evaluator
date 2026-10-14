//! Reference values captured from the Python implementation; these pin the
//! CPython-compatible pieces (random, hashing, seeds, bootstrap).

use krutrim_evaluator::engine::derive_seed;
use krutrim_evaluator::pypy::PyRandom;
use krutrim_evaluator::suite::load_suite;
use krutrim_evaluator::verdicts::bootstrap_ci_mean;
use std::path::PathBuf;

#[test]
fn cpython_random_sequence_matches() {
    let mut r = PyRandom::new(20261014);
    let got: Vec<f64> = (0..5).map(|_| r.random()).collect();
    let want = [
        0.7275074686529501,
        0.17484386025003607,
        0.7681855048949298,
        0.8073503112910572,
        0.5910386779004542,
    ];
    for (g, w) in got.iter().zip(want) {
        assert_eq!(*g, w, "random() sequence drifted from CPython");
    }
    let mut r = PyRandom::new(20261014);
    let picked: Vec<i64> = r.choices(&[10, 20, 30], 6).iter().map(|v| **v).collect();
    assert_eq!(picked, vec![30, 10, 30, 30, 20, 10]);
}

#[test]
fn derived_seeds_match_python() {
    assert_eq!(derive_seed(7, "reasoning.bat-and-ball", 0), 1690565186);
    assert_eq!(derive_seed(7, "reasoning.bat-and-ball", 1), 351826960);
}

#[test]
fn bootstrap_ci_matches_python() {
    let ci = bootstrap_ci_mean(&[1.0, 1.0, 0.0, 1.0, 0.0]).expect("ci");
    assert_eq!(ci, (0.2, 1.0));
}

#[test]
fn example_content_hashes_match_python() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let suite = load_suite(&manifest_dir.join("../../../suites/example.yaml")).expect("load");
    let got: Vec<String> = suite
        .tasks
        .iter()
        .map(|t| t.content_hash()[..12].to_string())
        .collect();
    assert_eq!(
        got,
        vec![
            "0d959eecfb82",
            "3e10ee191cb3",
            "4bd2df02d560",
            "91abbb4c7aa3",
            "19ec135a360e"
        ]
    );
}

#[test]
fn indic_numbers_parse_like_python() {
    use krutrim_evaluator::graders::indic_number;
    assert_eq!(indic_number("₹1,23,456").unwrap().to_string(), "123456");
    assert_eq!(indic_number("2.5 lakh").unwrap().to_string(), "250000.0");
    assert_eq!(indic_number("१२३").unwrap().to_string(), "123");
    assert_eq!(indic_number("Rs. 40").unwrap().to_string(), "40");
    assert_eq!(indic_number("1 crore").unwrap().to_string(), "10000000");
    assert!(indic_number("about 40").is_err());
    assert!(indic_number("1,2,34").is_err());
}
