//! Source provenance and secret redaction, mirroring provenance.py.

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn env_secrets() -> Vec<String> {
    std::env::vars()
        .filter(|(k, v)| {
            v.len() >= 8
                && ["API_KEY", "AUTH_TOKEN", "SECRET", "PASSWORD"]
                    .iter()
                    .any(|m| k.to_uppercase().contains(m))
        })
        .map(|(_, v)| v)
        .collect()
}

pub fn redact_str(text: &str, extra: &[String]) -> String {
    let mut out = text.to_string();
    for secret in env_secrets().iter().chain(extra.iter()) {
        if secret.len() >= 8 {
            out = out.replace(secret.as_str(), "[REDACTED]");
        }
    }
    out
}

pub fn redact(value: &Value) -> Value {
    redact_with(value, &env_secrets())
}

pub fn redact_with(value: &Value, secrets: &[String]) -> Value {
    match value {
        Value::String(s) => Value::String(redact_str(s, secrets)),
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| redact_with(v, secrets)).collect())
        }
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), redact_with(v, secrets)))
                .collect::<Map<_, _>>(),
        ),
        other => other.clone(),
    }
}

/// Find a repo root (pyproject.toml + src/) at or above `start`.
fn repo_root(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        if d.join("pyproject.toml").is_file() && d.join("src").is_dir() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

fn collect_files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_files(&p, ext, out);
            } else if p.extension().and_then(|s| s.to_str()) == Some(ext) {
                out.push(p);
            }
        }
    }
}

/// Same shape as Python's source_metadata(). Outside a checkout the git
/// fields are null, exactly as the frozen Python build reports them.
pub fn source_metadata() -> Value {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let root = repo_root(&cwd);
    let (commit, dirty, source_sha) = match &root {
        Some(root) if git(root, &["ls-files", "src", "pyproject.toml"]).is_some() => {
            let mut paths = Vec::new();
            collect_files(&root.join("src"), "py", &mut paths);
            paths.sort();
            let mut yamls = Vec::new();
            collect_files(&root.join("src"), "yaml", &mut yamls);
            yamls.sort();
            paths.extend(yamls);
            paths.push(root.join("pyproject.toml"));
            paths.push(root.join("uv.lock"));
            let mut digest = Sha256::new();
            for p in &paths {
                if p.is_file() {
                    if let Ok(rel) = p.strip_prefix(root) {
                        digest.update(rel.to_string_lossy().as_bytes());
                    }
                    if let Ok(bytes) = std::fs::read(p) {
                        digest.update(bytes);
                    }
                }
            }
            let sha: String = digest
                .finalize()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let status = git(root, &["status", "--porcelain", "--untracked-files=normal"]);
            (
                git(root, &["rev-parse", "HEAD"])
                    .map(Value::String)
                    .unwrap_or(Value::Null),
                status
                    .map(|s| Value::Bool(!s.is_empty()))
                    .unwrap_or(Value::Null),
                Value::String(sha),
            )
        }
        Some(root) => (
            git(root, &["rev-parse", "HEAD"])
                .map(Value::String)
                .unwrap_or(Value::Null),
            Value::Null,
            Value::Null,
        ),
        None => (Value::Null, Value::Null, Value::Null),
    };
    json!({
        "git_commit": commit,
        "dirty": dirty,
        "source_sha256": source_sha,
        "dependencies": {
            "anthropic": null, "openai": null, "httpx2": null,
            "pydantic": null, "pyyaml": null, "typer": null
        }
    })
}
