//! API keys managed by the interactive menu. Stored in
//! `$XDG_CONFIG_HOME/krutrim-mulyankan/keys.env` (default `~/.config/...`),
//! one `NAME=value` per line, file mode 0600 on Unix. Real environment
//! variables and a project `.env` always win; this file only fills gaps.
//! The Python km does not read this file; keys saved here apply to the
//! Rust km (CLI and TUI runs).

use std::collections::BTreeMap;
use std::path::PathBuf;

pub const PROVIDER_KEYS: [(&str, &str, bool); 7] = [
    ("Anthropic", "ANTHROPIC_API_KEY", true),
    ("OpenAI", "OPENAI_API_KEY", true),
    ("xAI", "XAI_API_KEY", true),
    ("Gemini", "GEMINI_API_KEY", true),
    ("OpenRouter", "OPENROUTER_API_KEY", true),
    ("Compatible endpoint key", "COMPATIBLE_API_KEY", true),
    ("Compatible endpoint URL", "COMPATIBLE_BASE_URL", false),
];

pub fn keys_path() -> PathBuf {
    if let Some(path) = std::env::var_os("KM_KEYS_FILE") {
        return PathBuf::from(path);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".config"))
                .unwrap_or_else(|| PathBuf::from(".config"))
        });
    base.join("krutrim-mulyankan").join("keys.env")
}

pub fn read_keys_file(path: &std::path::Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                out.insert(
                    key.trim().to_string(),
                    value.trim().trim_matches(['"', '\'']).to_string(),
                );
            }
        }
    }
    out
}

/// Fill unset process env vars from the keys file (never overrides).
pub fn load_keys_file() {
    for (key, value) in read_keys_file(&keys_path()) {
        if std::env::var_os(&key).is_none() {
            // Single-threaded startup: main calls this before any threads.
            unsafe { std::env::set_var(&key, &value) };
        }
    }
}

pub fn save_key(path: &std::path::Path, name: &str, value: Option<&str>) -> Result<(), String> {
    let mut entries = read_keys_file(path);
    match value {
        Some(value) => {
            entries.insert(name.to_string(), value.to_string());
        }
        None => {
            entries.remove(name);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut text =
        String::from("# Krutrim Mulyankan API keys (managed by km). Keep this file private.\n");
    for (key, value) in &entries {
        text.push_str(&format!("{key}={value}\n"));
    }
    std::fs::write(path, text).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    // Reflect the change in this process so a run started from the menu
    // sees the key that was just saved (or no longer sees a deleted one).
    match value {
        Some(value) => unsafe { std::env::set_var(name, value) },
        None => unsafe { std::env::remove_var(name) },
    }
    Ok(())
}

/// Where a key currently comes from: "environment", "keys file", or unset.
pub fn key_source(name: &str, file_entries: &BTreeMap<String, String>) -> Option<&'static str> {
    match std::env::var(name) {
        Ok(current) if !current.is_empty() => {
            if file_entries.get(name) == Some(&current) {
                Some("keys file")
            } else {
                Some("environment")
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_permissions() {
        let dir = std::env::temp_dir().join(format!("km-keys-{}", std::process::id()));
        let path = dir.join("keys.env");
        save_key(&path, "OPENROUTER_API_KEY", Some("sk-test-123")).unwrap();
        assert_eq!(
            read_keys_file(&path).get("OPENROUTER_API_KEY"),
            Some(&"sk-test-123".to_string())
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        save_key(&path, "OPENROUTER_API_KEY", None).unwrap();
        assert!(read_keys_file(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
