//! Runtime settings from environment variables, mirroring settings.py.
//! Rust binaries also embed the shipped suites as a last resort so a bare
//! `km` works anywhere; explicit dirs and on-disk suites always win.

use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Settings {
    pub runs_dir: PathBuf,
    pub db_path: PathBuf,
    pub suites_dir: PathBuf,
    pub prices_path: PathBuf,
    pub max_usd: f64,
    pub usd_to_inr: f64,
}

impl Settings {
    pub fn from_env() -> Settings {
        Self::from_map(&std::env::vars().collect())
    }

    pub fn from_map(env: &BTreeMap<String, String>) -> Settings {
        let get = |k: &str| env.get(k).cloned();
        let runs_dir = PathBuf::from(get("MJ_RUNS_DIR").unwrap_or_else(|| "runs".into()));
        Settings {
            db_path: get("MJ_DB")
                .map(PathBuf::from)
                .unwrap_or_else(|| runs_dir.join("results.db")),
            suites_dir: default_suites_dir(env),
            prices_path: PathBuf::from(get("MJ_PRICES").unwrap_or_else(|| "prices.yaml".into())),
            max_usd: get("MJ_MAX_USD")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.0),
            usd_to_inr: get("MJ_USD_INR")
                .and_then(|v| v.parse().ok())
                .unwrap_or(88.0),
            runs_dir,
        }
    }
}

/// Where `km` looks for suites. `MJ_SUITES_DIR` wins; then `suites/` beside
/// the binary (the tarball layout); then `./suites`; the caller falls back
/// to the embedded copies when none of those exist.
pub fn default_suites_dir(env: &BTreeMap<String, String>) -> PathBuf {
    if let Some(v) = env.get("MJ_SUITES_DIR") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Ok(real) = exe.canonicalize() {
            if let Some(dir) = real.parent() {
                let beside = dir.join("suites");
                if beside.is_dir() {
                    return beside;
                }
            }
        }
    }
    PathBuf::from("suites")
}

pub const EMBEDDED_SUITES: &[(&str, &str)] = &[
    (
        "example.yaml",
        include_str!("../../../../suites/example.yaml"),
    ),
    (
        "india-pilot.yaml",
        include_str!("../../../../suites/india-pilot.yaml"),
    ),
    (
        "railway-pilot.yaml",
        include_str!("../../../../suites/railway-pilot.yaml"),
    ),
];

/// Materialize the embedded suites into a per-process temp dir (idempotent).
pub fn embedded_suites_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("km-suites-{}", std::process::id()));
    for (name, text) in EMBEDDED_SUITES {
        let target = dir.join(name);
        if !target.is_file() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&target, text);
        }
    }
    dir
}

/// The suites dir to use, falling back to the embedded copies when the
/// configured directory holds no suite files.
pub fn effective_suites_dir(settings: &Settings) -> PathBuf {
    if crate::suite::discover_suites(&settings.suites_dir).is_empty() {
        embedded_suites_dir()
    } else {
        settings.suites_dir.clone()
    }
}
