//! Window shell. API keys stay in the OS keychain. The Python engine hears them only as
//! environment variables or a private stdin message, never as a file.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use keyring::Entry;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

const SERVICE: &str = "krutrim-mulyankan";
const KEYS: &[(&str, &str)] = &[
    ("openai", "OPENAI_API_KEY"),
    ("anthropic", "ANTHROPIC_API_KEY"),
    ("gemini", "GEMINI_API_KEY"),
    ("xai", "XAI_API_KEY"),
    ("openrouter", "OPENROUTER_API_KEY"),
    ("compatible", "COMPATIBLE_API_KEY"),
];

struct Engine {
    stdin: Mutex<Box<dyn Write + Send>>,
    // Held so dropping the shell also drops the engine process.
    #[allow(dead_code)]
    child: Mutex<Option<Child>>,
    waiters: Mutex<HashMap<String, mpsc::Sender<Value>>>,
    next_id: AtomicU64,
}

fn saved_secrets() -> Vec<(&'static str, String)> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut found = Vec::new();
        for (provider, env_name) in KEYS {
            let Ok(entry) = Entry::new(SERVICE, provider) else {
                continue;
            };
            if let Ok(secret) = entry.get_password() {
                if !secret.is_empty() {
                    found.push((*env_name, secret));
                }
            }
        }
        let _ = tx.send(found);
    });
    rx.recv_timeout(Duration::from_secs(3)).unwrap_or_default()
}

fn env_for(provider: &str) -> Option<&'static str> {
    KEYS.iter().find(|(id, _)| *id == provider).map(|(_, name)| *name)
}

fn find_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..5 {
        if dir.join("pyproject.toml").is_file() && dir.join("suites").is_dir() {
            return Some(dir);
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

fn engine_command(app: &AppHandle) -> Result<Command, String> {
    let mut cmd = if let Ok(python) = std::env::var("KM_PYTHON") {
        let mut cmd = Command::new(python);
        cmd.arg("-m").arg("kritrim_mulyankan.desktop_bridge");
        cmd
    } else if cfg!(debug_assertions) {
        let program = if cfg!(windows) { "python" } else { "python3" };
        let mut cmd = Command::new(program);
        cmd.arg("-m").arg("kritrim_mulyankan.desktop_bridge");
        cmd
    } else {
        let name = if cfg!(windows) {
            "km-desktop.exe"
        } else {
            "km-desktop"
        };
        let path = app
            .path()
            .resource_dir()
            .map_err(|err| err.to_string())?
            .join("engine")
            .join(name);
        if !path.is_file() {
            return Err("The bundled evaluation engine is missing.".into());
        }
        Command::new(path)
    };
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .env("PYTHONUNBUFFERED", "1");
    if let Some(root) = find_root() {
        cmd.env("KM_RESOURCE_ROOT", &root);
        cmd.env("KM_UI_ROOT", root.join("desktop").join("ui"));
        cmd.env("MJ_SUITES_DIR", root.join("suites"));
        let source = root.join("src");
        if source.is_dir() {
            cmd.env("PYTHONPATH", source);
        }
    }
    if let Ok(data) = app.path().app_data_dir() {
        let runs = data.join("runs");
        let _ = std::fs::create_dir_all(&runs);
        cmd.env("MJ_RUNS_DIR", &runs);
        cmd.env("MJ_DB", runs.join("results.db"));
    }
    // A locked or missing Secret Service must not freeze the window on Hyprland.
    for (env_name, secret) in saved_secrets() {
        cmd.env(env_name, secret);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(cmd)
}

impl Engine {
    fn write_line(&self, line: &str) -> Result<(), String> {
        let mut stdin = self.stdin.lock().map_err(|_| "The engine is busy.".to_string())?;
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush())
            .map_err(|_| "The evaluation engine is not running.".to_string())
    }

    fn call(&self, method: &str, params: Value) -> Result<(), String> {
        let id = format!("kc-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = mpsc::channel();
        self.waiters
            .lock()
            .map_err(|_| "The engine is busy.".to_string())?
            .insert(id.clone(), tx);
        let line = json!({"id": id, "method": method, "params": params}).to_string();
        if let Err(err) = self.write_line(&line) {
            if let Ok(mut waiters) = self.waiters.lock() {
                waiters.remove(&id);
            }
            return Err(err);
        }
        let message = rx.recv_timeout(Duration::from_secs(30)).map_err(|_| {
            "The evaluation engine did not answer.".to_string()
        })?;
        if message.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(())
        } else {
            Err(message
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("The evaluation engine reported a problem.")
                .to_string())
        }
    }
}

#[tauri::command]
fn engine_call(state: State<'_, Arc<Engine>>, request: String) -> Result<(), String> {
    let value: Value = serde_json::from_str(&request).map_err(|_| "The message could not be read.")?;
    if value.get("method").and_then(Value::as_str) == Some("set_keys") {
        return Err("Save keys in the system keychain.".into());
    }
    state.write_line(&request)
}

#[tauri::command]
fn keychain_set(state: State<'_, Arc<Engine>>, provider: String, secret: String) -> Result<String, String> {
    let env_name = env_for(&provider).ok_or("Unknown provider.")?;
    if secret.len() > 4096 || secret.chars().any(|char| matches!(char, '\n' | '\r' | '\0')) {
        return Err("A key cannot contain a line break.".into());
    }
    // Persist when an OS credential store answers. Without one (no Secret
    // Service, locked keyring), still hand the key to the engine for this
    // session and say so, instead of refusing a key the user just typed.
    let persisted = match Entry::new(SERVICE, &provider) {
        Ok(entry) if secret.is_empty() => entry.delete_credential().is_ok(),
        Ok(entry) => entry.set_password(&secret).is_ok(),
        Err(_) => false,
    };
    state.call("set_keys", json!({"keys": {env_name: secret}}))?;
    Ok(if persisted { "keychain".into() } else { "memory".into() })
}

#[tauri::command]
fn keychain_status() -> Vec<String> {
    // Same guard as saved_secrets(): a locked or missing Secret Service must
    // report "nothing saved" after a short wait, never freeze the window.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut saved = Vec::new();
        for (provider, _) in KEYS {
            let Ok(entry) = Entry::new(SERVICE, *provider) else {
                continue;
            };
            if let Ok(secret) = entry.get_password() {
                if !secret.is_empty() {
                    saved.push((*provider).to_string());
                }
            }
        }
        let _ = tx.send(saved);
    });
    rx.recv_timeout(Duration::from_secs(3)).unwrap_or_default()
}

#[tauri::command]
fn save_text(app: AppHandle, filename: String, contents: String) -> Result<Option<String>, String> {
    let chosen = app.dialog().file().set_file_name(&filename).blocking_save_file();
    let Some(path) = chosen else {
        return Ok(None);
    };
    let path = path.into_path().map_err(|err| err.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    std::fs::write(&path, contents).map_err(|err| err.to_string())?;
    Ok(Some(path.display().to_string()))
}

fn spawn_reader(app: AppHandle, stdout: impl Read + Send + 'static) {
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if let Ok(message) = serde_json::from_str::<Value>(&line) {
                if message.get("ok").is_some() {
                    if let Some(id) = message.get("id").and_then(Value::as_str) {
                        if let Some(engine) = app.try_state::<Arc<Engine>>() {
                            if let Some(sender) = engine
                                .waiters
                                .lock()
                                .ok()
                                .and_then(|mut waiters| waiters.remove(id))
                            {
                                let _ = sender.send(message);
                            }
                        }
                    }
                }
            }
            let _ = app.emit("engine://line", line);
        }
        let _ = app.emit(
            "engine://line",
            json!({"id": null, "ok": false, "error": "The evaluation engine stopped."}).to_string(),
        );
    });
}

fn prepare(app: &AppHandle) -> Result<(Arc<Engine>, impl Read + Send + 'static), String> {
    let mut child = engine_command(app)?
        .spawn()
        .map_err(|err| format!("Could not start the evaluation engine: {err}"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or("The evaluation engine has no input.")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("The evaluation engine has no output.")?;
    let engine = Arc::new(Engine {
        stdin: Mutex::new(Box::new(stdin)),
        child: Mutex::new(Some(child)),
        waiters: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(1),
    });
    Ok((engine, stdout))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let (engine, stdout) = prepare(app.handle())?;
            app.manage(engine);
            spawn_reader(app.handle().clone(), stdout);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            engine_call,
            keychain_set,
            keychain_status,
            save_text
        ])
        .run(tauri::generate_context!())
        .expect("error while running Krutrim Mulyankan");
}
