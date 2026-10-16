//! Automatic updates. The app checks the signed release manifest shortly after it opens
//! and every six hours, downloads new versions in the background, and installs them
//! where that needs no prompt (AppImage, macOS). Nothing restarts on its own: the
//! window shows one banner and the person chooses when to restart.

use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State, Url};
use tauri_plugin_updater::{Update, UpdaterExt};

const FIRST_CHECK_AFTER: Duration = Duration::from_secs(8);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const DOWNLOAD_PAGE: &str = "https://krutrimmulyankan.in/download";

#[derive(Default)]
pub struct Updates {
    status: Mutex<Value>,
    /// Downloaded and verified, waiting for the person to start the installer.
    pending: Mutex<Option<(Update, Vec<u8>)>>,
}

/// How this copy of the app can be updated.
#[derive(PartialEq)]
enum Kind {
    /// Replaced in place without any prompt; a restart picks it up.
    Quiet,
    /// Needs an installer or a password prompt, so it waits for a click.
    Prompt,
    /// Installed by a package manager we cannot drive (for example pacman).
    Manual,
}

fn kind() -> Kind {
    if cfg!(target_os = "macos") {
        return Kind::Quiet;
    }
    if cfg!(windows) {
        return Kind::Prompt;
    }
    if std::env::var_os("APPIMAGE").is_some() {
        return Kind::Quiet;
    }
    use tauri::utils::config::BundleType;
    match tauri::utils::platform::bundle_type() {
        Some(BundleType::Deb) if on_path("dpkg") => Kind::Prompt,
        Some(BundleType::Rpm) if on_path("rpm") => Kind::Prompt,
        _ => Kind::Manual,
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

fn set(app: &AppHandle, status: Value) {
    *app.state::<Updates>().status.lock().unwrap() = status.clone();
    let _ = app.emit("update://status", status);
}

async fn check_once(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<Updates>();
    let current = state.status.lock().unwrap()["state"].as_str().unwrap_or("").to_string();
    if current == "installed" || current == "ready" || current == "installing" {
        return Ok(());
    }
    let mut builder = app.updater_builder();
    // Testing hook only: any manifest must still carry signatures from our key.
    if let Ok(manifest) = std::env::var("KM_UPDATE_MANIFEST_URL") {
        let url = Url::parse(&manifest).map_err(|e| e.to_string())?;
        builder = builder.endpoints(vec![url]).map_err(|e| e.to_string())?;
    }
    let updater = builder.build().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Ok(());
    };
    let version = update.version.clone();
    let how = kind();
    if how == Kind::Manual {
        set(app, json!({"state": "manual", "version": version, "url": DOWNLOAD_PAGE}));
        return Ok(());
    }
    set(app, json!({"state": "downloading", "version": version}));
    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    if how == Kind::Quiet {
        update.install(&bytes).map_err(|e| e.to_string())?;
        set(app, json!({"state": "installed", "version": version}));
    } else {
        *state.pending.lock().unwrap() = Some((update, bytes));
        set(app, json!({"state": "ready", "version": version}));
    }
    Ok(())
}

/// Checks soon after start, then every few hours, for as long as the window is open.
pub fn start(app: AppHandle) {
    set(&app, json!({"state": "idle", "version": app.package_info().version.to_string()}));
    std::thread::spawn(move || {
        std::thread::sleep(FIRST_CHECK_AFTER);
        loop {
            if let Err(error) = tauri::async_runtime::block_on(check_once(&app)) {
                // Offline or GitHub unavailable: stay quiet and try again later.
                eprintln!("Update check skipped: {error}");
                let state = app.state::<Updates>();
                let stuck = state.status.lock().unwrap()["state"] == "downloading";
                if stuck {
                    set(&app, json!({"state": "idle"}));
                }
            }
            std::thread::sleep(CHECK_EVERY);
        }
    });
}

/// Start the new version. An AppImage is relaunched the way the launcher starts it,
/// so systems without FUSE keep working.
fn relaunch(app: &AppHandle) {
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        if std::process::Command::new(&appimage)
            .arg("--appimage-extract-and-run")
            .spawn()
            .is_ok()
        {
            app.exit(0);
            return;
        }
    }
    app.restart();
}

#[tauri::command]
pub fn update_status(state: State<'_, Updates>) -> Value {
    state.status.lock().unwrap().clone()
}

#[tauri::command]
pub fn update_restart(app: AppHandle) {
    relaunch(&app);
}

/// Runs the downloaded installer (Windows closes and reopens the app itself;
/// .deb and .rpm ask for the computer's password), then restarts.
#[tauri::command]
pub fn update_apply(app: AppHandle, state: State<'_, Updates>) -> Result<(), String> {
    let Some((update, bytes)) = state.pending.lock().unwrap().take() else {
        return Err("There is no downloaded update.".into());
    };
    let version = update.version.clone();
    set(&app, json!({"state": "installing", "version": version}));
    if let Err(error) = update.install(&bytes) {
        set(&app, json!({"state": "ready", "version": version}));
        *state.pending.lock().unwrap() = Some((update, bytes));
        return Err(format!("The update was not installed: {error}"));
    }
    relaunch(&app);
    Ok(())
}
