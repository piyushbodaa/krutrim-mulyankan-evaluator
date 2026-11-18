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
    pub bundled_suites: std::sync::Arc<std::sync::OnceLock<Result<tempfile::TempDir, String>>>,
}

impl Settings {
    pub fn from_env() -> Settings {
        Self::from_map(&std::env::vars().collect())
    }

    pub fn from_map(env: &BTreeMap<String, String>) -> Settings {
        let get = |k: &str| env.get(k).cloned();
        let runs_dir = PathBuf::from(get("MJ_RUNS_DIR").unwrap_or_else(|| "runs".into()));
        Settings {
            bundled_suites: Default::default(),
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

/// Create a fresh, user-private directory even with a permissive umask.
pub(crate) fn private_tempdir() -> std::io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("km-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir()
}

fn embedded_suites_dir() -> std::io::Result<tempfile::TempDir> {
    let dir = private_tempdir()?;
    for (name, text) in EMBEDDED_SUITES {
        std::fs::write(dir.path().join(name), text)?;
    }
    Ok(dir)
}

/// Keep bundled files alive for the lifetime of the settings and its clones.
pub fn effective_suites_dir(settings: &Settings) -> Result<PathBuf, String> {
    if crate::suite::discover_suites(&settings.suites_dir).is_empty() {
        settings
            .bundled_suites
            .get_or_init(|| embedded_suites_dir().map_err(|e| e.to_string()))
            .as_ref()
            .map(|dir| dir.path().to_owned())
            .map_err(Clone::clone)
    } else {
        Ok(settings.suites_dir.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_suites_share_a_private_directory_until_settings_drop() {
        let mut settings = Settings::from_map(&BTreeMap::new());
        let empty = private_tempdir().unwrap();
        settings.suites_dir = empty.path().to_path_buf();
        let path = effective_suites_dir(&settings).unwrap();
        assert_eq!(effective_suites_dir(&settings.clone()).unwrap(), path);
        for (name, text) in EMBEDDED_SUITES {
            assert_eq!(std::fs::read_to_string(path.join(name)).unwrap(), *text);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(path.metadata().unwrap().permissions().mode() & 0o077, 0);
        }
        drop(settings);
        assert!(!path.exists());
    }
}
