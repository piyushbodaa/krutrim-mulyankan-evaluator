//! Claude Code tool-free draft runner, mirroring runners/claude_draft.py.
//! Uses existing subscription auth only: a fresh operator access proof is
//! required before the CLI is ever invoked, and API credentials are never
//! passed through the environment allow-list.

use crate::provenance::redact;
use crate::pypy::sha256_hex;
use crate::runners::{RunContext, RunOutput, Usage};
use crate::suite::Task;
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::Instant;

const ALLOWED_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "TMPDIR",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
];

const WINDOWS_ENV: &[&str] = &[
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "HOMEDRIVE",
    "HOMEPATH",
    "TEMP",
    "TMP",
    "USERNAME",
];

fn allowed_env(name: &str, windows: bool) -> bool {
    if windows {
        ALLOWED_ENV
            .iter()
            .chain(WINDOWS_ENV)
            .any(|key| key.eq_ignore_ascii_case(name))
    } else {
        ALLOWED_ENV.contains(&name)
    }
}

pub fn cli_environment() -> BTreeMap<String, String> {
    std::env::vars()
        .filter(|(k, _)| allowed_env(k, cfg!(windows)))
        .collect()
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    find_executable(name, std::env::split_paths(&path), cfg!(windows))
}

fn find_executable(
    name: &str,
    paths: impl Iterator<Item = PathBuf>,
    windows: bool,
) -> Option<PathBuf> {
    // npm also installs an extensionless POSIX shim on Windows. Prefer
    // native executables, then Windows batch shims; never select that shim.
    let extensions: &[&str] = if windows {
        &[".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    paths
        .flat_map(|dir| {
            extensions
                .iter()
                .map(move |extension| dir.join(format!("{name}{extension}")))
        })
        .find(|p| p.is_file())
}

pub fn cli_version(executable: &Path) -> Option<String> {
    let out = Command::new(executable)
        .arg("--version")
        .env_clear()
        .envs(cli_environment())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout)
        .trim()
        .chars()
        .take(100)
        .collect::<String>();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

pub struct ClaudeDraft {
    pub access_proof: PathBuf,
    pub executable: Option<PathBuf>,
}

impl ClaudeDraft {
    pub fn new(access_proof: PathBuf) -> ClaudeDraft {
        ClaudeDraft {
            executable: which("claude"),
            access_proof,
        }
    }

    pub fn as_runner(self) -> crate::runners::Runner {
        crate::runners::Runner {
            name: "harness:claude-draft".into(),
            kind: crate::runners::RunnerKind::Harness,
            inner: crate::runners::RunnerInner::ClaudeDraft(self),
        }
    }

    pub fn problems(&self) -> Vec<String> {
        let Some(_exe) = &self.executable else {
            return vec!["Installed Claude CLI not found".into()];
        };
        let valid = (|| -> bool {
            let Ok(text) = std::fs::read_to_string(&self.access_proof) else {
                return false;
            };
            let Ok(proof) = serde_json::from_str::<Value>(&text) else {
                return false;
            };
            let Some(expires) = proof.get("expires_at").and_then(|v| v.as_str()) else {
                return false;
            };
            let Ok(exp) = DateTime::parse_from_rfc3339(expires) else {
                return false;
            };
            proof.get("usage_credits_enabled") == Some(&Value::Bool(false))
                && proof.get("auth_method").and_then(|v| v.as_str()) == Some("claude.ai")
                && proof.get("operator_verified") == Some(&Value::Bool(true))
                && exp > Utc::now()
        })();
        if valid {
            Vec::new()
        } else {
            vec!["Fresh included-plan/usage-credits-off verification is required".into()]
        }
    }

    pub fn configuration_extra(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("framework".into(), json!("Claude Code CLI"));
        m.insert(
            "framework_version".into(),
            self.executable
                .as_ref()
                .and_then(|e| cli_version(e))
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        let mut options = Map::new();
        options.insert("tools".into(), json!([]));
        options.insert("mcp".into(), json!([]));
        options.insert("safe_mode".into(), json!(true));
        options.insert("restricted".into(), json!(true));
        options.insert("session_persistence".into(), json!(false));
        options.insert("api_key_environment".into(), json!(false));
        m.insert("options".into(), Value::Object(options));
        m.insert("endpoint".into(), Value::Null);
        m.insert("seed_supported".into(), json!(false));
        m.insert(
            "cost_scope".into(),
            json!("Existing included subscription; actual charge unknown"),
        );
        m.insert(
            "access_proof_sha256".into(),
            std::fs::read(&self.access_proof)
                .map(|b| Value::String(sha256_hex(&b)))
                .unwrap_or(Value::Null),
        );
        m
    }

    pub fn run(&self, task: &Task, prompt: &str, ctx: &RunContext) -> Result<RunOutput, String> {
        let issues = self.problems();
        if !issues.is_empty() {
            return Err(format!("RuntimeError: {}", issues.join("; ")));
        }
        if task.capability_track != "text" || !task.fixtures.is_empty() {
            return Err(
                "ValueError: Claude draft supports frozen synthetic text tasks only".into(),
            );
        }
        let exe = self.executable.clone().unwrap();
        let start = Instant::now();
        let auth = Command::new(&exe)
            .args(["auth", "status", "--json"])
            .env_clear()
            .envs(cli_environment())
            .current_dir(&ctx.workdir)
            .output()
            .map_err(|e| format!("RuntimeError: CLI authentication status unavailable ({e})"))?;
        let state: Value = serde_json::from_slice(&auth.stdout)
            .map_err(|_| "RuntimeError: CLI authentication status unavailable".to_string())?;
        if !(state
            .get("loggedIn")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            && state.get("authMethod").and_then(|v| v.as_str()) == Some("claude.ai")
            && state.get("apiProvider").and_then(|v| v.as_str()) == Some("firstParty"))
        {
            return Err(
                "RuntimeError: CLI is not using verified subscription authentication".into(),
            );
        }
        let args = [
            "--print",
            "--output-format",
            "json",
            "--tools",
            "",
            "--strict-mcp-config",
            "--mcp-config",
            "{\"mcpServers\":{}}",
            "--safe-mode",
            "--restricted",
            "--disable-slash-commands",
            "--no-chrome",
            "--no-session-persistence",
            "--setting-sources",
            "",
        ];
        let mut cmd = Command::new(&exe);
        cmd.args(args)
            .current_dir(&ctx.workdir)
            .env_clear()
            .envs(cli_environment())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("RuntimeError: could not start Claude CLI ({e})"))?;
        // Drain both pipes while the process runs. Waiting for exit first
        // deadlocks once a response fills the OS pipe buffer (especially on
        // Windows, where even an ordinary JSON response can fill it).
        let stdout_reader = read_pipe(child.stdout.take());
        let stderr_reader = read_pipe(child.stderr.take());
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(prompt.as_bytes());
        }
        let deadline = ctx.timeout_s.min(90.0);
        let mut reason: Option<String> = None;
        let status = loop {
            if ctx
                .cancel
                .as_ref()
                .map(|c| c.load(Ordering::SeqCst))
                .unwrap_or(false)
            {
                reason = Some("Cancelled; CLI process stopped. Billing not instrumented.".into());
                break None;
            }
            if start.elapsed().as_secs_f64() >= deadline {
                reason = Some("CLI timed out; process stopped. Billing not instrumented.".into());
                break None;
            }
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
                Err(e) => return Err(format!("RuntimeError: Claude CLI failed ({e})")),
            }
        };
        let status = match status {
            Some(s) => s,
            None => terminate_group(&mut child),
        };
        let stdout = stdout_reader.join().unwrap_or_default();
        let stderr = stderr_reader.join().unwrap_or_default();
        let exit_code = status.code();
        let mut raw = Map::new();
        raw.insert("stdout".into(), json!(stdout));
        raw.insert("stderr".into(), json!(stderr));
        raw.insert("exit_code".into(), json!(exit_code));
        raw.insert(
            "billing".into(),
            json!("included-plan check; token-price estimates are not actual charges"),
        );
        let raw = match redact(&Value::Object(raw)) {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        let stdout_clean = raw
            .get("stdout")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let Ok(response) = serde_json::from_str::<Value>(&stdout_clean) else {
            return Ok(RunOutput {
                error: Some(reason.unwrap_or_else(|| "CLI returned no valid JSON envelope".into())),
                raw,
                ..Default::default()
            });
        };
        let mut raw = raw;
        raw.insert("response".into(), redact(&response));
        let result_text = response
            .get("result")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if response
            .get("result")
            .map(|v| !v.is_string())
            .unwrap_or(false)
        {
            return Ok(RunOutput {
                error: Some("CLI envelope has unsupported result shape".into()),
                raw,
                ..Default::default()
            });
        }
        let usage = response.get("usage").cloned().unwrap_or_else(|| json!({}));
        let model_usage = response
            .get("modelUsage")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !usage.is_object() || !model_usage.is_object() {
            return Ok(RunOutput {
                error: Some("CLI envelope has unsupported usage shape".into()),
                raw,
                ..Default::default()
            });
        }
        let models: Vec<String> = model_usage
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        raw.insert(
            "resolved_model".into(),
            if models.len() == 1 {
                json!(models[0])
            } else {
                Value::Null
            },
        );
        raw.insert(
            "reported_token_price_usd".into(),
            response
                .get("total_cost_usd")
                .cloned()
                .unwrap_or(Value::Null),
        );
        let failed = exit_code.map(|c| c != 0).unwrap_or(true)
            || response
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
        let error = reason.or_else(|| {
            if failed {
                Some("CLI execution failed; inspect private envelope".into())
            } else {
                None
            }
        });
        Ok(RunOutput {
            output: result_text,
            error,
            usage: Usage {
                input_tokens: usage.get("input_tokens").and_then(|v| v.as_i64()),
                output_tokens: usage.get("output_tokens").and_then(|v| v.as_i64()),
                cost_usd: None,
            },
            tool_calls: Some(0),
            raw,
            ..Default::default()
        })
    }
}

fn read_pipe(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut output = String::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_string(&mut output);
        }
        output
    })
}

fn terminate_group(child: &mut Child) -> std::process::ExitStatus {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // A .cmd shim launches another process. Stopping only cmd.exe can
        // leave that process running (and holding the stdout pipe open).
        let taskkill = std::env::var_os("SystemRoot")
            .map(|root| PathBuf::from(root).join("System32").join("taskkill.exe"))
            .unwrap_or_else(|| PathBuf::from("taskkill.exe"));
        let _ = Command::new(taskkill)
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = child.kill();
    }
    #[cfg(unix)]
    unsafe {
        libc::killpg(child.id() as i32, libc::SIGTERM);
    }
    let deadline = Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(s)) => return s,
            _ if Instant::now() >= deadline => break,
            _ => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
    #[cfg(unix)]
    unsafe {
        libc::killpg(child.id() as i32, libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = child.kill();
    child.wait().expect("wait after kill")
}

pub fn access_proof_path(runs_dir: &Path) -> PathBuf {
    runs_dir.join("automation-access.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_larger_than_pipe_buffers_does_not_block_process_exit() {
        const CHILD: &str = "KM_TEST_LARGE_CLI_OUTPUT";
        const BYTES: usize = 256 * 1024;
        if std::env::var_os(CHILD).is_some() {
            std::io::stdout().write_all(&vec![b'A'; BYTES]).unwrap();
            std::io::stderr().write_all(&vec![b'B'; BYTES]).unwrap();
            return;
        }
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "claude_draft::tests::output_larger_than_pipe_buffers_does_not_block_process_exit",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = read_pipe(child.stdout.take());
        let stderr = read_pipe(child.stderr.take());
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        let completed = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status.success();
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let stdout = stdout.join().unwrap();
        let stderr = stderr.join().unwrap();
        assert!(
            completed,
            "CLI blocked writing output larger than its pipe buffers"
        );
        assert_eq!(stdout.bytes().filter(|byte| *byte == b'A').count(), BYTES);
        assert_eq!(stderr.bytes().filter(|byte| *byte == b'B').count(), BYTES);
    }

    #[cfg(windows)]
    #[test]
    fn windows_batch_cli_starts_with_filtered_environment_and_a_spaced_path() {
        let dir = std::env::temp_dir().join(format!("km cli batch {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("claude.cmd");
        std::fs::write(&path, "@echo off\r\necho test-cli-1.0\r\n").unwrap();
        assert_eq!(cli_version(&path).as_deref(), Some("test-cli-1.0"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn windows_environment_keeps_runtime_paths_but_excludes_credentials() {
        for name in [
            "Path",
            "SystemRoot",
            "ComSpec",
            "APPDATA",
            "USERPROFILE",
            "TEMP",
        ] {
            assert!(allowed_env(name, true), "{name}");
        }
        for name in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "OPENAI_API_KEY",
            "OTHER_SECRET",
        ] {
            assert!(!allowed_env(name, true), "{name}");
            assert!(!allowed_env(name, false), "{name}");
        }
        assert!(allowed_env("PATH", false));
        assert!(!allowed_env("Path", false));
    }

    #[test]
    fn windows_finds_native_and_npm_cli_without_selecting_posix_shim() {
        let dir = std::env::temp_dir().join(format!("km-cli-resolution-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["claude", "claude.cmd", "claude.exe"] {
            std::fs::write(dir.join(name), "test fixture").unwrap();
        }
        let lookup = |windows| find_executable("claude", std::iter::once(dir.clone()), windows);
        assert_eq!(lookup(false), Some(dir.join("claude")));
        assert_eq!(lookup(true), Some(dir.join("claude.exe")));
        std::fs::remove_file(dir.join("claude.exe")).unwrap();
        assert_eq!(lookup(true), Some(dir.join("claude.cmd")));
        std::fs::remove_file(dir.join("claude.cmd")).unwrap();
        assert_eq!(lookup(true), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
