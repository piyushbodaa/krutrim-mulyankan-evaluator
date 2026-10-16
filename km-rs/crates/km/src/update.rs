//! km updates itself. Once a day (after the command you ran has finished) it reads
//! the latest release manifest; when there is a newer version it downloads the
//! archive for this computer, checks it against SHA256SUMS, checks that list's
//! signature against the key built into km, and swaps the executable in place.
//! KM_NO_AUTO_UPDATE=1 (or any CI environment) turns the automatic check off;
//! `km update` always works.

use base64::Engine;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const REPOSITORY: &str = "piyushbodaa/krutrim-mulyankan-evaluator";
/// Same key as the desktop updater (desktop/src-tauri/tauri.conf.json).
const UPDATE_PUBLIC_KEY: &str = "RWRCwcUgkx0kxRFDfbkrHgrCLkOfFOe+zsUAwl7K0XB0YVWFBqEEeAl4";
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn archive_name() -> Option<(&'static str, &'static str)> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some(("km-linux-x86_64.tar.gz", "km")),
        ("windows", "x86_64") => Some(("km-windows-x86_64.zip", "km.exe")),
        ("macos", "aarch64") => Some(("km-macos-aarch64.tar.gz", "km")),
        ("macos", "x86_64") => Some(("km-macos-x86_64.tar.gz", "km")),
        _ => None,
    }
}

/// True when `candidate` is a later release than `installed` (numeric x.y.z).
pub fn is_newer(candidate: &str, installed: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split(['.', '-', '+'])
            .take(3)
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    parts(candidate) > parts(installed)
}

/// Checks a Tauri/minisign `.sig` (base64 of the signature file) for `data`.
pub fn verify_signature(data: &[u8], signature_b64: &str, public_key: &str) -> Result<(), String> {
    let text = base64::engine::general_purpose::STANDARD
        .decode(signature_b64.trim())
        .map_err(|_| "the signature is not readable".to_string())?;
    let text = String::from_utf8(text).map_err(|_| "the signature is not readable".to_string())?;
    let key = minisign_verify::PublicKey::from_base64(public_key)
        .map_err(|e| format!("bad update key: {e}"))?;
    let signature = minisign_verify::Signature::decode(&text)
        .map_err(|_| "the signature is not readable".to_string())?;
    key.verify(data, &signature, false)
        .map_err(|_| "the download's signature does not match; nothing was changed".to_string())
}

/// The SHA-256 that a signed SHA256SUMS lists for `asset`, if the list belongs to `version`.
pub fn listed_digest(sums: &str, asset: &str, version: &str) -> Result<String, String> {
    if !sums.contains(&format!("kritrim_mulyankan-{version}-py3-none-any.whl")) {
        return Err(format!("the checksum list is not for version {version}"));
    }
    sums.lines()
        .find_map(|line| {
            let (digest, name) = line.split_once("  ")?;
            (name.trim() == asset).then(|| digest.trim().to_lowercase())
        })
        .ok_or_else(|| format!("{asset} is not in the checksum list"))
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(timeout)
        .user_agent(&format!("km/{}", current()))
        .build()
}

fn get(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, String> {
    let response = agent.get(url).call().map_err(|e| match e {
        ureq::Error::Status(code, _) => format!("{url} answered {code}"),
        ureq::Error::Transport(t) => format!("could not reach GitHub ({t})"),
    })?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut response.into_reader().take(200 << 20), &mut bytes)
        .map_err(|e| format!("download interrupted: {e}"))?;
    Ok(bytes)
}

use std::io::Read as _;

/// The newest released version, from the manifest the desktop app also reads.
pub fn latest_version(timeout: Duration) -> Result<String, String> {
    let url = std::env::var("KM_UPDATE_MANIFEST_URL").unwrap_or_else(|_| {
        format!("https://github.com/{REPOSITORY}/releases/latest/download/latest.json")
    });
    let body = get(&agent(timeout), &url)?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| "the release manifest is not readable".to_string())?;
    manifest["version"]
        .as_str()
        .map(|v| v.trim_start_matches('v').to_string())
        .ok_or_else(|| "the release manifest has no version".to_string())
}

/// Downloads, verifies and returns the new km executable for `version`.
fn fetch_verified(version: &str) -> Result<Vec<u8>, String> {
    let (asset, member) = archive_name().ok_or("there is no km download for this computer")?;
    let base = std::env::var("KM_UPDATE_DOWNLOAD_BASE")
        .unwrap_or_else(|_| format!("https://github.com/{REPOSITORY}/releases/download/v{version}"));
    let agent = agent(Duration::from_secs(300));
    let sums = get(&agent, &format!("{base}/SHA256SUMS"))?;
    let signature = String::from_utf8(get(&agent, &format!("{base}/SHA256SUMS.sig"))?)
        .map_err(|_| "the signature is not readable".to_string())?;
    verify_signature(&sums, &signature, UPDATE_PUBLIC_KEY)?;
    let sums = String::from_utf8(sums).map_err(|_| "the checksum list is not readable")?;
    let expected = listed_digest(&sums, asset, version)?;
    let archive = get(&agent, &format!("{base}/{asset}"))?;
    let actual: String = Sha256::digest(&archive).iter().map(|b| format!("{b:02x}")).collect();
    if actual != expected {
        return Err("the download does not match its checksum; nothing was changed".into());
    }
    if asset.ends_with(".zip") {
        crate::archive::from_zip(&archive, member)
    } else {
        crate::archive::from_tar_gz(&archive, member)
    }
}

/// Puts `binary` where `exe` is, after checking it runs and reports `version`.
pub fn replace_executable(exe: &Path, binary: &[u8], version: &str) -> Result<(), String> {
    let dir = exe.parent().ok_or("cannot find km's folder")?;
    let staged = dir.join(format!(".km-update-{}{}", std::process::id(), std::env::consts::EXE_SUFFIX));
    std::fs::write(&staged, binary).map_err(|e| format!("cannot write next to km ({e})"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
    }
    let reported = std::process::Command::new(&staged)
        .arg("--version")
        .env("KM_NO_AUTO_UPDATE", "1")
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default();
    if reported != format!("km {version}") {
        let _ = std::fs::remove_file(&staged);
        return Err(format!("the new km did not start correctly ({reported:?})"));
    }
    if cfg!(windows) {
        // A running .exe cannot be overwritten, but it can be moved aside.
        let old = exe.with_extension("old.exe");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old).map_err(|e| format!("cannot replace km ({e})"))?;
    }
    std::fs::rename(&staged, exe).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!("cannot replace km ({e})")
    })
}

fn installed_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::fs::canonicalize(&exe).map_err(|e| e.to_string())
}

/// Development builds (inside a cargo target folder) never replace themselves.
fn is_dev_build(exe: &Path) -> bool {
    exe.components().any(|c| c.as_os_str() == "target")
}

pub enum Outcome {
    UpToDate,
    Updated(String),
}

/// Check and, when newer, install. Shared by `km update` and the daily check.
pub fn update_now(timeout: Duration) -> Result<Outcome, String> {
    let latest = latest_version(timeout)?;
    if !is_newer(&latest, current()) {
        return Ok(Outcome::UpToDate);
    }
    let exe = installed_exe()?;
    if is_dev_build(&exe) {
        return Err(format!("km {latest} is available, but this is a development build"));
    }
    let binary = fetch_verified(&latest)?;
    replace_executable(&exe, &binary, &latest)?;
    Ok(Outcome::Updated(latest))
}

fn state_file() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
    }?;
    Some(base.join("krutrim-mulyankan").join("km-last-update-check"))
}

fn auto_disabled() -> bool {
    let off = |name: &str| std::env::var(name).map(|v| !v.is_empty() && v != "0").unwrap_or(false);
    off("KM_NO_AUTO_UPDATE") || off("CI")
}

/// The once-a-day automatic update, run after a command finishes. Never fails the command.
pub fn auto() {
    if auto_disabled() || installed_exe().map(|exe| is_dev_build(&exe)).unwrap_or(true) {
        return;
    }
    let Some(stamp) = state_file() else { return };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let last = std::fs::read_to_string(&stamp)
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if now.saturating_sub(last) < CHECK_EVERY.as_secs() {
        return;
    }
    if let Some(dir) = stamp.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&stamp, now.to_string());
    match update_now(Duration::from_secs(4)) {
        Ok(Outcome::Updated(version)) => {
            eprintln!("km updated itself to {version}. The new version runs from your next command.")
        }
        Ok(Outcome::UpToDate) => {}
        Err(error) if error.contains("is available") => eprintln!("{error}."),
        Err(_) => {} // Offline or GitHub unavailable: try again tomorrow.
    }
}

/// `km update [--check]`
pub fn command(args: &[String]) -> Result<u8, String> {
    let check_only = args.iter().any(|a| a == "--check");
    if check_only {
        let latest = latest_version(Duration::from_secs(15))?;
        if is_newer(&latest, current()) {
            println!("km {latest} is available (you have {}). Run `km update` to install it.", current());
        } else {
            println!("km {} is up to date.", current());
        }
        return Ok(0);
    }
    println!("Checking for a new km…");
    match update_now(Duration::from_secs(15))? {
        Outcome::UpToDate => println!("km {} is up to date.", current()),
        Outcome::Updated(version) => println!("km is now {version} (was {}).", current()),
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // SHA256SUMS sample signed in CI style (tauri signer) with the real update key.
    const SAMPLE: &[u8] = b"abc  km-linux-x86_64.tar.gz\n";
    const SAMPLE_SIG: &str = include_str!("../tests/fixtures/sample-SHA256SUMS.sig");

    #[test]
    fn accepts_only_data_signed_with_the_update_key() {
        verify_signature(SAMPLE, SAMPLE_SIG, UPDATE_PUBLIC_KEY).unwrap();
        assert!(verify_signature(b"abd  km-linux-x86_64.tar.gz\n", SAMPLE_SIG, UPDATE_PUBLIC_KEY).is_err());
        assert!(verify_signature(SAMPLE, "not base64!", UPDATE_PUBLIC_KEY).is_err());
    }

    #[test]
    fn reads_km_from_the_release_archive_formats() {
        // Made with Python's tarfile and zipfile, like scripts/package_downloads.py.
        let tar = include_bytes!("../tests/fixtures/km-sample.tar.gz");
        let zip = include_bytes!("../tests/fixtures/km-sample.zip");
        let want = b"#!/bin/sh\necho km 9.9.9\n".to_vec();
        assert_eq!(crate::archive::from_tar_gz(tar, "km").unwrap(), want);
        assert_eq!(crate::archive::from_zip(zip, "km.exe").unwrap(), want);
        assert!(crate::archive::from_tar_gz(tar, "missing").is_err());
        assert!(crate::archive::from_zip(zip, "km").is_err());
        assert!(crate::archive::from_zip(b"not a zip", "km.exe").is_err());
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.4.10", "0.4.9"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(!is_newer("0.4.1", "0.4.1"));
        assert!(!is_newer("0.4.0", "0.4.1"));
    }

    #[test]
    fn checksum_list_must_belong_to_the_version() {
        let sums = "aa  km-linux-x86_64.tar.gz\nbb  kritrim_mulyankan-0.5.0-py3-none-any.whl\n";
        assert_eq!(listed_digest(sums, "km-linux-x86_64.tar.gz", "0.5.0").unwrap(), "aa");
        assert!(listed_digest(sums, "km-linux-x86_64.tar.gz", "0.6.0").is_err());
        assert!(listed_digest(sums, "km-macos-x86_64.tar.gz", "0.5.0").is_err());
    }
}
