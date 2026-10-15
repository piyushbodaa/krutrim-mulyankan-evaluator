//! API keys managed by the interactive menu. Stored in
//! `$XDG_CONFIG_HOME/krutrim-mulyankan/keys.env` (default `~/.config/...`
//! on Linux/macOS, `%APPDATA%/krutrim-mulyankan/keys.env` on Windows),
//! one `NAME=value` per line, file mode 0600 on Unix. Real environment
//! variables and a project `.env` always win; this file only fills gaps.
//! The Python km does not read this file; keys saved here apply to the
//! Rust km (CLI and TUI runs).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

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
    keys_path_from(|name| std::env::var_os(name), cfg!(windows))
}

fn keys_path_from(env: impl Fn(&str) -> Option<std::ffi::OsString>, windows: bool) -> PathBuf {
    if let Some(path) = env("KM_KEYS_FILE") {
        return PathBuf::from(path);
    }
    let base = env("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            if windows {
                env("APPDATA")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .or_else(|| {
                        env("USERPROFILE")
                            .filter(|value| !value.is_empty())
                            .map(|home| PathBuf::from(home).join("AppData").join("Roaming"))
                    })
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            env("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
                .unwrap_or_else(|| PathBuf::from(".config"))
        });
    base.join("krutrim-mulyankan").join("keys.env")
}

// Ownership is recorded only when this module actually populates a variable.
// Matching the value on disk is NOT evidence of ownership: a shell or .env may
// deliberately contain that very same value (including an empty value).
#[derive(Default)]
struct KeyEnvironment {
    managed: BTreeMap<String, (PathBuf, OsString)>,
}

static KEY_ENVIRONMENT: OnceLock<Mutex<KeyEnvironment>> = OnceLock::new();

fn key_environment() -> &'static Mutex<KeyEnvironment> {
    KEY_ENVIRONMENT.get_or_init(|| Mutex::new(KeyEnvironment::default()))
}

trait Environment {
    fn get(&self, name: &str) -> Option<OsString>;
    fn set(&mut self, name: &str, value: &str);
    fn remove(&mut self, name: &str);
}

struct ProcessEnvironment;

impl Environment for ProcessEnvironment {
    fn get(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
    }

    fn set(&mut self, name: &str, value: &str) {
        // Startup/menu calls must occur while no evaluator worker is running.
        unsafe { std::env::set_var(name, value) };
    }

    fn remove(&mut self, name: &str) {
        unsafe { std::env::remove_var(name) };
    }
}

fn invalid_data(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn valid_value(value: &str) -> bool {
    !value.contains(['\r', '\n', '\0'])
}

fn regular_file_or_missing(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(invalid_data(
            "Keys path must be a regular file, not a symlink or directory",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn read_keys_file_checked(path: &Path) -> io::Result<BTreeMap<String, String>> {
    regular_file_or_missing(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A replacement between lstat and open must not redirect the read or
        // block on a FIFO. Validate the opened handle again before reading.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.file_type().is_file() {
        return Err(invalid_data("Keys path must be a regular file"));
    }
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let mut out = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(invalid_data(&format!(
                "Invalid keys file entry on line {}",
                index + 1
            )));
        };
        let (key, value) = (key.trim(), value.trim().trim_matches(['"', '\'']));
        if !valid_name(key) || !valid_value(value) {
            // Never include malformed content: it may itself be a secret.
            return Err(invalid_data(&format!(
                "Invalid keys file entry on line {}",
                index + 1
            )));
        }
        out.insert(key.to_string(), value.to_string());
    }
    Ok(out)
}

/// Best-effort display lookup. Mutations use the checked reader so an
/// unreadable/malformed file is never mistaken for an empty key store.
pub fn read_keys_file(path: &Path) -> BTreeMap<String, String> {
    read_keys_file_checked(path).unwrap_or_default()
}

fn absolute_path(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn load_keys_from(
    path: &Path,
    state: &mut KeyEnvironment,
    environment: &mut impl Environment,
) -> io::Result<()> {
    let entries = read_keys_file_checked(path)?;
    let path = absolute_path(path)?;
    for (key, value) in entries {
        if environment.get(&key).is_none() {
            environment.set(&key, &value);
            state
                .managed
                .insert(key, (path.clone(), OsString::from(value)));
        }
    }
    Ok(())
}

/// Fill unset process env vars from the keys file (never overrides).
pub fn load_keys_file() {
    let result = key_environment()
        .lock()
        .map_err(|_| "Key environment state is unavailable".to_string())
        .and_then(|mut state| {
            load_keys_from(&keys_path(), &mut state, &mut ProcessEnvironment)
                .map_err(|error| format!("Could not load stored keys: {error}"))
        });
    if let Err(error) = result {
        eprintln!("{error}");
    }
}

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

struct TempKeyFile(PathBuf);

impl Drop for TempKeyFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn private_temp_file(parent: &Path) -> io::Result<(TempKeyFile, File)> {
    for _ in 0..128 {
        let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".km-keys-{}-{sequence}.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Set permissions at creation, before any secret bytes exist.
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((TempKeyFile(path), file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Could not create a private temporary keys file",
    ))
}

fn make_private_directory(parent: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent)
}

// Separated from serialization so failure paths can be tested without changing
// process permissions or relying on a non-root test runner.
fn atomic_replace(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
    replace: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    make_private_directory(parent)?;
    regular_file_or_missing(path)?;
    let (temp, mut file) = private_temp_file(parent)?;
    write(&mut file)?;
    // Close before rename for Windows. No target is truncated in place.
    drop(file);
    regular_file_or_missing(path)?;
    replace(&temp.0, path)
}

fn write_private(
    file: &mut File,
    text: &str,
    set_permissions: impl FnOnce(&File) -> io::Result<()>,
) -> io::Result<()> {
    // The same checked sequence is used in production and failure tests.
    set_permissions(file)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

fn write_keys_file(path: &Path, text: &str) -> io::Result<()> {
    atomic_replace(
        path,
        |file| {
            write_private(file, text, |file| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    // Restore owner access if umask removed it. Failure is fatal,
                    // and occurs before writing. Creation was already private.
                    file.set_permissions(fs::Permissions::from_mode(0o600))?;
                }
                #[cfg(not(unix))]
                let _ = file;
                Ok(())
            })
        },
        |from, to| fs::rename(from, to),
    )
}

fn save_key_with(
    path: &Path,
    name: &str,
    value: Option<&str>,
    state: &mut KeyEnvironment,
    environment: &mut impl Environment,
    persist: impl FnOnce(&Path, &str) -> io::Result<()>,
) -> io::Result<()> {
    if !valid_name(name) || value.is_some_and(|value| !valid_value(value)) {
        return Err(invalid_data("Invalid key name or value"));
    }
    let path = absolute_path(path)?;
    let mut entries = read_keys_file_checked(&path)?;
    match value {
        Some(value) => {
            entries.insert(name.to_string(), value.to_string());
        }
        None => {
            entries.remove(name);
        }
    }
    let mut text =
        String::from("# Krutrim Mulyankan API keys (managed by km). Keep this file private.\n");
    for (key, value) in &entries {
        text.push_str(&format!("{key}={value}\n"));
    }
    persist(&path, &text)?;

    // Only change process state after a successful disk replacement. Equality
    // here may revoke known ownership after an external change; it can never
    // establish ownership of a pre-existing shell/.env variable.
    let current = environment.get(name);
    let owned = state
        .managed
        .get(name)
        .is_some_and(|(source, injected)| source == &path && current.as_ref() == Some(injected));
    if current.is_none() || owned {
        match value {
            Some(value) => {
                environment.set(name, value);
                state
                    .managed
                    .insert(name.to_string(), (path, OsString::from(value)));
            }
            None => {
                if owned {
                    environment.remove(name);
                }
                state.managed.remove(name);
            }
        }
    } else if state
        .managed
        .get(name)
        .is_some_and(|(_, injected)| current.as_ref() != Some(injected))
    {
        state.managed.remove(name);
    }
    Ok(())
}

pub fn save_key(path: &Path, name: &str, value: Option<&str>) -> Result<(), String> {
    let mut state = key_environment()
        .lock()
        .map_err(|_| "Key environment state is unavailable".to_string())?;
    save_key_with(
        path,
        name,
        value,
        &mut state,
        &mut ProcessEnvironment,
        write_keys_file,
    )
    .map_err(|error| format!("Could not save stored key: {error}"))
}

fn key_source_in(
    name: &str,
    state: &KeyEnvironment,
    environment: &impl Environment,
) -> Option<&'static str> {
    let current = environment.get(name)?;
    if state
        .managed
        .get(name)
        .is_some_and(|(_, injected)| injected == &current)
    {
        Some("keys file")
    } else {
        Some("environment")
    }
}

/// Where the current process value actually came from, not a value comparison
/// with the file (which may match an unrelated environment/.env value).
pub fn key_source(name: &str, _file_entries: &BTreeMap<String, String>) -> Option<&'static str> {
    match key_environment().lock() {
        Ok(state) => key_source_in(name, &state, &ProcessEnvironment),
        Err(_) => ProcessEnvironment.get(name).map(|_| "environment"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_for(values: &[(&str, &str)], windows: bool) -> PathBuf {
        keys_path_from(
            |name| {
                values
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).into())
            },
            windows,
        )
    }

    #[test]
    fn windows_keys_use_the_user_profile_without_a_unix_home() {
        assert_eq!(
            path_for(&[("APPDATA", "profile/roaming")], true),
            PathBuf::from("profile/roaming/krutrim-mulyankan/keys.env")
        );
        assert_eq!(
            path_for(&[("APPDATA", ""), ("USERPROFILE", "profile")], true),
            PathBuf::from("profile/AppData/Roaming/krutrim-mulyankan/keys.env")
        );
    }

    #[test]
    fn key_file_overrides_and_unix_locations_are_preserved() {
        for windows in [false, true] {
            assert_eq!(
                path_for(
                    &[
                        ("KM_KEYS_FILE", "custom.env"),
                        ("XDG_CONFIG_HOME", "config")
                    ],
                    windows
                ),
                PathBuf::from("custom.env")
            );
            assert_eq!(
                path_for(
                    &[("XDG_CONFIG_HOME", "config"), ("APPDATA", "roaming")],
                    windows
                ),
                PathBuf::from("config/krutrim-mulyankan/keys.env")
            );
        }
        assert_eq!(
            path_for(&[("HOME", "home"), ("APPDATA", "roaming")], false),
            PathBuf::from("home/.config/krutrim-mulyankan/keys.env")
        );
    }

    #[derive(Default)]
    struct FakeEnvironment(BTreeMap<String, OsString>);

    impl Environment for FakeEnvironment {
        fn get(&self, name: &str) -> Option<OsString> {
            self.0.get(name).cloned()
        }
        fn set(&mut self, name: &str, value: &str) {
            self.0.insert(name.to_string(), value.into());
        }
        fn remove(&mut self, name: &str) {
            self.0.remove(name);
        }
    }

    struct Fixture {
        dir: PathBuf,
        path: PathBuf,
        state: KeyEnvironment,
        env: FakeEnvironment,
    }

    impl Fixture {
        fn new() -> Self {
            loop {
                let (reserved, file) = private_temp_file(&std::env::temp_dir()).unwrap();
                drop(file);
                let dir = reserved.0.with_extension("dir");
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                match builder.create(&dir) {
                    Ok(()) => {
                        return Self {
                            path: dir.join("keys.env"),
                            dir,
                            state: KeyEnvironment::default(),
                            env: FakeEnvironment::default(),
                        }
                    }
                    // Never adopt a stale or attacker-provided test directory.
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("Could not create test directory: {error}"),
                }
            }
        }

        fn save(&mut self, name: &str, value: Option<&str>) -> io::Result<()> {
            save_key_with(
                &self.path,
                name,
                value,
                &mut self.state,
                &mut self.env,
                write_keys_file,
            )
        }

        fn load(&mut self) -> io::Result<()> {
            load_keys_from(&self.path, &mut self.state, &mut self.env)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    const KEY: &str = "KM_DUMMY_API_KEY";

    #[test]
    fn roundtrip_and_permissions() {
        let mut f = Fixture::new();
        f.save(KEY, Some("dummy-test-123")).unwrap();
        assert_eq!(
            read_keys_file_checked(&f.path)
                .unwrap()
                .get(KEY)
                .map(String::as_str),
            Some("dummy-test-123")
        );
        assert_eq!(f.env.get(KEY), Some("dummy-test-123".into()));
        assert_eq!(key_source_in(KEY, &f.state, &f.env), Some("keys file"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&f.path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        f.save(KEY, None).unwrap();
        assert!(read_keys_file_checked(&f.path).unwrap().is_empty());
        assert!(f.env.get(KEY).is_none());
    }

    #[test]
    fn genuine_environment_wins_even_when_identical_or_empty() {
        for original in ["dummy-stored", "", "dummy-shell-or-dotenv"] {
            let mut f = Fixture::new();
            fs::write(&f.path, format!("{KEY}=dummy-stored\n")).unwrap();
            f.env.set(KEY, original);
            f.load().unwrap();
            assert_eq!(key_source_in(KEY, &f.state, &f.env), Some("environment"));
            f.save(KEY, Some("dummy-new")).unwrap();
            assert_eq!(f.env.get(KEY), Some(original.into()));
            f.save(KEY, None).unwrap();
            assert_eq!(f.env.get(KEY), Some(original.into()));
            assert_eq!(key_source_in(KEY, &f.state, &f.env), Some("environment"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_environment_is_preserved() {
        use std::os::unix::ffi::OsStringExt;
        let mut f = Fixture::new();
        let original = OsString::from_vec(vec![0xff, 0xfe]);
        f.env.0.insert(KEY.to_string(), original.clone());
        f.save(KEY, Some("dummy-stored")).unwrap();
        f.load().unwrap();
        f.save(KEY, None).unwrap();
        assert_eq!(f.env.get(KEY), Some(original));
        assert_eq!(key_source_in(KEY, &f.state, &f.env), Some("environment"));
    }

    #[test]
    fn loaded_keys_can_be_updated_and_removed() {
        let mut f = Fixture::new();
        fs::write(
            &f.path,
            format!("{KEY}=dummy-old\nKM_OTHER_DUMMY=dummy-other\n"),
        )
        .unwrap();
        f.load().unwrap();
        f.save(KEY, Some("dummy-new")).unwrap();
        assert_eq!(f.env.get(KEY), Some("dummy-new".into()));
        f.save(KEY, None).unwrap();
        assert!(f.env.get(KEY).is_none());
        assert_eq!(f.env.get("KM_OTHER_DUMMY"), Some("dummy-other".into()));
        assert_eq!(read_keys_file_checked(&f.path).unwrap().len(), 1);
    }

    #[test]
    fn later_external_override_is_not_owned() {
        let mut f = Fixture::new();
        f.save(KEY, Some("dummy-stored")).unwrap();
        f.env.set(KEY, "dummy-external");
        assert_eq!(key_source_in(KEY, &f.state, &f.env), Some("environment"));
        f.save(KEY, Some("dummy-new")).unwrap();
        f.save(KEY, None).unwrap();
        assert_eq!(f.env.get(KEY), Some("dummy-external".into()));
    }

    #[test]
    fn another_key_store_cannot_remove_loaded_environment() {
        let mut f = Fixture::new();
        f.save(KEY, Some("dummy-first")).unwrap();
        let other = f.dir.join("other.env");
        save_key_with(
            &other,
            KEY,
            Some("dummy-second"),
            &mut f.state,
            &mut f.env,
            write_keys_file,
        )
        .unwrap();
        save_key_with(&other, KEY, None, &mut f.state, &mut f.env, write_keys_file).unwrap();
        assert_eq!(f.env.get(KEY), Some("dummy-first".into()));
    }

    #[cfg(unix)]
    #[test]
    fn private_at_creation_and_atomic_replacement() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mut f = Fixture::new();
        f.save(KEY, Some("dummy-old")).unwrap();
        let old = File::open(&f.path).unwrap();
        let old_inode = old.metadata().unwrap().ino();
        atomic_replace(
            &f.path,
            |file| {
                assert_eq!(file.metadata()?.permissions().mode() & 0o077, 0);
                assert_eq!(file.metadata()?.len(), 0);
                file.write_all(b"KM_DUMMY_API_KEY=dummy-replaced\n")?;
                file.sync_all()
            },
            |from, to| fs::rename(from, to),
        )
        .unwrap();
        assert_ne!(old_inode, fs::metadata(&f.path).unwrap().ino());
        let mut old_text = String::new();
        (&old).read_to_string(&mut old_text).unwrap();
        assert!(old_text.contains("dummy-old"));
        assert!(!old_text.contains("dummy-replaced"));
        assert_eq!(fs::read_dir(&f.dir).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn new_directories_and_replacement_permissions_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let mut f = Fixture::new();
        f.path = f.dir.join("new-directory").join("keys.env");
        f.save(KEY, Some("dummy-first")).unwrap();
        assert_eq!(
            fs::metadata(f.path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
        // An older insecure file must be replaced with a private new inode.
        fs::set_permissions(&f.path, fs::Permissions::from_mode(0o644)).unwrap();
        f.save(KEY, Some("dummy-second")).unwrap();
        assert_eq!(
            fs::metadata(&f.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn unrestricted_umask_still_creates_private_file() {
        use std::os::unix::fs::PermissionsExt;
        const CHILD_FLAG: &str = "KM_DUMMY_KEYS_TEST_UMASK_CHILD";
        if std::env::var_os(CHILD_FLAG).is_some() {
            // Only this isolated, single-test child changes its process umask.
            unsafe {
                libc::umask(0);
            }
            let mut f = Fixture::new();
            let (temp, file) = private_temp_file(&f.dir).unwrap();
            assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(file.metadata().unwrap().len(), 0);
            drop(file);
            drop(temp);
            f.save(KEY, Some("dummy-umask-test")).unwrap();
            assert_eq!(
                fs::metadata(&f.path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "keys::tests::unrestricted_umask_still_creates_private_file",
                "--test-threads=1",
            ])
            .env(CHILD_FLAG, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Isolated umask test failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn failed_permission_write_sync_or_rename_preserves_file_and_environment() {
        for phase in ["permissions", "write", "sync", "rename"] {
            let mut f = Fixture::new();
            f.save(KEY, Some("dummy-old")).unwrap();
            let original = fs::read(&f.path).unwrap();
            let error = save_key_with(
                &f.path,
                KEY,
                Some("dummy-new"),
                &mut f.state,
                &mut f.env,
                |path, text| {
                    atomic_replace(
                        path,
                        |file| {
                            if phase == "permissions" {
                                let result = write_private(file, text, |_| {
                                    Err(io::Error::from(io::ErrorKind::PermissionDenied))
                                });
                                assert_eq!(
                                    file.metadata()?.len(),
                                    0,
                                    "No bytes may be written after a permission failure"
                                );
                                return result;
                            }
                            if phase == "write" {
                                file.write_all(b"partial dummy content")?;
                                return Err(io::Error::from(io::ErrorKind::WriteZero));
                            }
                            file.write_all(text.as_bytes())?;
                            if phase == "sync" {
                                return Err(io::Error::other("simulated sync failure"));
                            }
                            file.sync_all()
                        },
                        |from, to| {
                            if phase == "rename" {
                                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
                            }
                            fs::rename(from, to)
                        },
                    )
                },
            )
            .unwrap_err();
            assert!(!error.to_string().contains("dummy-new"));
            assert_eq!(fs::read(&f.path).unwrap(), original, "phase {phase}");
            assert_eq!(f.env.get(KEY), Some("dummy-old".into()), "phase {phase}");
            assert_eq!(key_source_in(KEY, &f.state, &f.env), Some("keys file"));
            assert_eq!(
                fs::read_dir(&f.dir).unwrap().count(),
                1,
                "temporary file leaked at {phase}"
            );
        }
    }

    #[test]
    fn corrupt_existing_file_is_never_overwritten() {
        for original in [
            b"\xffdummy-secret".as_slice(),
            b"dummy-malformed-secret",
            b"=dummy-secret",
            b"KEY=dummy\0secret",
        ] {
            let mut f = Fixture::new();
            fs::write(&f.path, original).unwrap();
            f.env.set(KEY, "dummy-external");
            let error = f.save(KEY, Some("dummy-new")).unwrap_err();
            assert!(!error.to_string().contains("dummy"));
            assert_eq!(fs::read(&f.path).unwrap(), original);
            assert_eq!(f.env.get(KEY), Some("dummy-external".into()));
            assert!(f.load().is_err());
        }
    }

    #[test]
    fn invalid_input_is_rejected_before_disk_or_environment_changes() {
        let mut f = Fixture::new();
        for (name, value) in [
            ("", "dummy"),
            ("BAD=NAME", "dummy"),
            (KEY, "dummy\nOTHER=injected"),
            (KEY, "dummy\rvalue"),
            (KEY, "dummy\0value"),
        ] {
            assert!(f.save(name, Some(value)).is_err());
            assert!(!f.path.exists());
            assert!(f.env.0.is_empty());
        }
    }

    #[test]
    fn directory_or_invalid_parent_errors_preserve_environment() {
        let mut f = Fixture::new();
        f.env.set(KEY, "dummy-external");
        fs::create_dir(&f.path).unwrap();
        assert!(f.save(KEY, Some("dummy-new")).is_err());
        assert!(f.path.is_dir());
        fs::remove_dir(&f.path).unwrap();
        fs::write(&f.path, "dummy-parent-file").unwrap();
        let child = f.path.join("keys.env");
        assert!(
            save_key_with(&child, KEY, None, &mut f.state, &mut f.env, write_keys_file).is_err()
        );
        assert_eq!(f.env.get(KEY), Some("dummy-external".into()));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_dangling_symlinks_are_rejected() {
        use std::os::unix::fs::symlink;
        for exists in [true, false] {
            let mut f = Fixture::new();
            let target = f.dir.join("other-file");
            if exists {
                fs::write(&target, "dummy-target-content").unwrap();
            }
            symlink(&target, &f.path).unwrap();
            assert!(f.load().is_err());
            assert!(f.save(KEY, Some("dummy-new")).is_err());
            assert!(fs::symlink_metadata(&f.path)
                .unwrap()
                .file_type()
                .is_symlink());
            assert!(f.env.0.is_empty());
            if exists {
                assert_eq!(fs::read_to_string(&target).unwrap(), "dummy-target-content");
            } else {
                assert!(!target.exists());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_created_during_write_is_rejected() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let target = f.dir.join("unrelated-file");
        fs::write(&target, "dummy-target-content").unwrap();
        let result = atomic_replace(
            &f.path,
            |file| {
                file.write_all(b"dummy-content")?;
                symlink(&target, &f.path)
            },
            |from, to| fs::rename(from, to),
        );
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "dummy-target-content");
        assert_eq!(fs::read_dir(&f.dir).unwrap().count(), 2);
    }
}
