//! Durable, single-writer workspace checkpoints and authoritative reset intents.
//!
//! A reset is committed when its intent has been synchronized, before either
//! checkpoint is replaced. If checkpointing then fails, the caller must retain
//! the cleared workspace and retry; reopening replays the same intent.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::workspace::{ResetKind, SCHEMA_VERSION, Snapshot};

#[path = "workspace_store_platform.rs"]
mod platform;

static UNIQUE: AtomicU64 = AtomicU64::new(0);

pub struct Store {
    path: PathBuf,
    // Keep the same native file locked for the entire lifetime, including resets.
    _lock: File,
    state: Mutex<State>,
}

pub struct Opened {
    pub store: Store,
    pub snapshot: Option<Snapshot>,
    pub warning: Option<String>,
    pub writable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveResult {
    pub revision: u64,
    pub reset_epoch: u64,
    pub written: bool,
}

struct State {
    writable: bool,
    revision: Option<u64>,
    durable_revision: Option<u64>,
    epoch: u64,
    authoritative_reset_epoch: Option<u64>,
    pending: Option<ResetIntent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResetIntent {
    snapshot: Snapshot,
    kind: ResetKind,
}

struct Loaded {
    status: LoadStatus,
    snapshot: Option<Snapshot>,
}

enum LoadStatus {
    Missing,
    Valid,
    Corrupt(String),
    Future(u64),
}

impl Loaded {
    fn without_snapshot(status: LoadStatus) -> Self {
        Self {
            status,
            snapshot: None,
        }
    }
}

#[derive(Debug)]
struct AtomicError {
    message: String,
    renamed: bool,
}

impl Store {
    pub fn open_default() -> Result<Opened, String> {
        Self::open_at(default_path()?)
    }

    /// `path` is the checkpoint file, not a directory. Tests must pass a private
    /// temporary path and never modify the process's HOME or XDG environment.
    pub fn open_at(path: PathBuf) -> Result<Opened, String> {
        let directory = parent(&path)?;
        if !path.is_absolute() {
            return Err(format!(
                "Workspace path must be absolute: {}",
                path.display()
            ));
        }
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder
            .create(directory)
            .map_err(|e| error("create workspace directory", directory, e))?;
        #[cfg(unix)]
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|e| error("make workspace directory private", directory, e))?;
        // Windows files inherit the per-user profile directory's native ACL.
        let lock_path = sibling(&path, ".lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        options.mode(0o600);
        let lock = options
            .open(&lock_path)
            .map_err(|e| error("open workspace lock", &lock_path, e))?;
        lock.try_lock().map_err(|e| {
            format!(
                "Cannot acquire workspace lock {} (another instance may be running): {e}",
                lock_path.display()
            )
        })?;
        #[cfg(unix)]
        lock.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| error("make workspace lock private", &lock_path, e))?;
        let mut store = Self {
            path,
            _lock: lock,
            state: Mutex::new(State {
                writable: true,
                revision: None,
                durable_revision: None,
                epoch: 0,
                authoritative_reset_epoch: None,
                pending: None,
            }),
        };
        let intent_path = sibling(&store.path, ".reset");
        if let Some(bytes) = read_optional(&intent_path)? {
            let mut value: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|e| format!("Invalid authoritative reset intent {}: {e}; preserving it and refusing to restore older data", intent_path.display()))?;
            let version = version_of(value.get("snapshot").unwrap_or(&serde_json::Value::Null))
                .map_err(|e| {
                    format!(
                        "Invalid authoritative reset intent {}: {e}",
                        intent_path.display()
                    )
                })?;
            if version > u64::from(SCHEMA_VERSION) {
                store
                    .state
                    .get_mut()
                    .map_err(|_| "Workspace mutex is poisoned".to_string())?
                    .writable = false;
                return Ok(Opened {
                    store,
                    snapshot: None,
                    warning: Some(format!(
                        "Reset intent uses newer workspace schema {version}; files preserved. Explicit Reset can replace them."
                    )),
                    writable: false,
                });
            }
            let saved = value
                .get_mut("snapshot")
                .ok_or("Authoritative reset intent has no snapshot")?;
            let snapshot = Snapshot::from_value(std::mem::take(saved))
                .map_err(|e| format!("Invalid authoritative reset intent: {e}"))?;
            *saved = serde_json::to_value(snapshot).map_err(|e| e.to_string())?;
            let intent: ResetIntent = serde_json::from_value(value).map_err(|e| {
                format!(
                    "Invalid authoritative reset intent {}: {e}",
                    intent_path.display()
                )
            })?;
            intent
                .snapshot
                .validate()
                .map_err(|e| format!("Invalid authoritative reset intent: {e}"))?;
            let state = store
                .state
                .get_mut()
                .map_err(|_| "Workspace mutex is poisoned".to_string())?;
            state.epoch = intent.snapshot.reset_epoch;
            state.authoritative_reset_epoch = Some(intent.snapshot.reset_epoch);
            let (snapshot, warning) = match store.finish_reset(&intent) {
                Ok(()) => (
                    intent.snapshot,
                    "Recovered an interrupted workspace reset.".to_string(),
                ),
                Err(e) => {
                    let snapshot = intent.snapshot.clone();
                    store
                        .state
                        .get_mut()
                        .map_err(|_| "Workspace mutex is poisoned".to_string())?
                        .pending = Some(intent);
                    (
                        snapshot,
                        format!(
                            "Recovered authoritative reset; checkpoint completion needs retry: {e}"
                        ),
                    )
                }
            };
            return Ok(Opened {
                store,
                snapshot: Some(snapshot),
                warning: Some(warning),
                writable: true,
            });
        }

        let main = load(&store.path)?;
        // A future primary wins over any old backup, with no renames or writes.
        if let LoadStatus::Future(version) = main.status {
            store
                .state
                .get_mut()
                .map_err(|_| "Workspace mutex is poisoned".to_string())?
                .writable = false;
            return Ok(Opened {
                store,
                snapshot: None,
                warning: Some(format!(
                    "Workspace uses newer schema {version}; files preserved. Explicit Reset can replace them."
                )),
                writable: false,
            });
        }
        let backup_path = sibling(&store.path, ".backup");
        let backup = load(&backup_path)?;
        if let LoadStatus::Future(version) = backup.status {
            let snapshot = main.snapshot;
            let state = store
                .state
                .get_mut()
                .map_err(|_| "Workspace mutex is poisoned".to_string())?;
            state.writable = false;
            state.epoch = snapshot.as_ref().map_or(0, |s| s.reset_epoch);
            return Ok(Opened {
                store,
                snapshot,
                warning: Some(format!(
                    "Recovery backup uses newer schema {version}; files preserved. Explicit Reset can replace them."
                )),
                writable: false,
            });
        }
        let mut warnings = Vec::new();
        #[cfg(unix)]
        for (path, loaded) in [(&store.path, &main), (&backup_path, &backup)] {
            if matches!(loaded.status, LoadStatus::Valid | LoadStatus::Corrupt(_)) {
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                    .map_err(|e| error("make workspace file private", path, e))?;
            }
        }
        if let LoadStatus::Corrupt(reason) = &main.status {
            let preserved = quarantine(&store.path)?;
            warnings.push(format!(
                "Invalid workspace ({reason}) preserved at {}.",
                preserved.display()
            ));
        }
        if let LoadStatus::Corrupt(reason) = &backup.status {
            let preserved = quarantine(&backup_path)?;
            warnings.push(format!(
                "Invalid recovery backup ({reason}) preserved at {}.",
                preserved.display()
            ));
        }
        let (snapshot, recover) = match (main.snapshot, backup.snapshot) {
            (Some(main), Some(backup)) if backup.reset_epoch > main.reset_epoch => {
                (Some(backup), true)
            }
            (Some(main), _) => (Some(main), false),
            (_, Some(backup)) => (Some(backup), true),
            _ => (None, false),
        };
        if recover {
            let snapshot = snapshot.as_ref().expect("recovery has a validated backup");
            atomic_json(&store.path, snapshot).map_err(|e| e.message)?;
            warnings.push("Restored the last validated workspace backup.".to_string());
        } else if snapshot.is_none() && !warnings.is_empty() {
            warnings.push(
                "No valid checkpoint remains; starting with the default workspace.".to_string(),
            );
        }
        store
            .state
            .get_mut()
            .map_err(|_| "Workspace mutex is poisoned".to_string())?
            .epoch = snapshot.as_ref().map_or(0, |s| s.reset_epoch);
        Ok(Opened {
            store,
            snapshot,
            warning: if warnings.is_empty() {
                None
            } else {
                Some(warnings.join(" "))
            },
            writable: true,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn writable(&self) -> bool {
        self.state.lock().is_ok_and(|state| state.writable)
    }

    /// True even if checkpoint completion returned an error after the reset
    /// intent became authoritative. The caller must not restore uncleared state.
    pub fn reset_is_authoritative(&self, epoch: u64) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| state.authoritative_reset_epoch == Some(epoch))
    }

    pub fn save(&self, revision: u64, snapshot: &Snapshot) -> Result<SaveResult, String> {
        snapshot.validate()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Workspace mutex is poisoned".to_string())?;
        let result = SaveResult {
            revision,
            reset_epoch: snapshot.reset_epoch,
            written: false,
        };
        if snapshot.reset_epoch < state.epoch
            || state.revision.is_some_and(|saved| revision < saved)
            || state.durable_revision == Some(revision)
        {
            return Ok(result);
        }
        if !state.writable {
            return Err("Workspace is read-only because a newer schema was found; only an explicit Reset may replace it".into());
        }
        if snapshot.reset_epoch != state.epoch {
            return Err("A workspace epoch can change only through an explicit reset".into());
        }
        // Fence older callbacks even if this write fails. The same revision can
        // be retried until its checkpoint has actually completed.
        state.revision = Some(revision);
        if let Some(intent) = &state.pending {
            self.finish_reset(intent)?;
            state.pending = None;
        }
        let backup = load(&sibling(&self.path, ".backup"))?;
        match backup.status {
            LoadStatus::Missing => {}
            LoadStatus::Valid => {
                let backup = backup.snapshot.expect("validated backup has snapshot");
                if backup.reset_epoch > state.epoch {
                    return Err(
                        "Recovery backup has a newer reset epoch; refusing to overwrite it".into(),
                    );
                }
            }
            LoadStatus::Corrupt(reason) => {
                return Err(format!(
                    "Recovery backup changed to invalid data; refusing to overwrite it: {reason}"
                ));
            }
            LoadStatus::Future(version) => {
                state.writable = false;
                return Err(format!(
                    "Recovery backup changed to newer schema {version}; refusing to overwrite it"
                ));
            }
        }
        // Never replace an unreadable, invalid, or unknown file that appeared
        // after open. Ordinary saves are not a destructive recovery mechanism.
        let main = load(&self.path)?;
        match main.status {
            LoadStatus::Valid => {
                let previous = main.snapshot.expect("validated checkpoint has snapshot");
                if previous.reset_epoch > state.epoch {
                    return Err(
                        "Checkpoint has a newer reset epoch; refusing to overwrite it".into(),
                    );
                }
                atomic_json(&sibling(&self.path, ".backup"), &previous).map_err(|e| e.message)?;
            }
            LoadStatus::Missing => {}
            LoadStatus::Corrupt(reason) => {
                return Err(format!(
                    "Checkpoint changed to invalid data; refusing to overwrite it: {reason}"
                ));
            }
            LoadStatus::Future(version) => {
                state.writable = false;
                return Err(format!(
                    "Checkpoint changed to newer schema {version}; refusing to overwrite it"
                ));
            }
        }
        atomic_json(&self.path, snapshot).map_err(|e| e.message)?;
        state.durable_revision = Some(revision);
        Ok(SaveResult {
            written: true,
            ..result
        })
    }

    pub fn reset(&self, revision: u64, snapshot: &Snapshot) -> Result<SaveResult, String> {
        self.reset_with_kind(revision, snapshot, ResetKind::Current)
    }

    /// Only an explicit All reset may replace future-version files. A successful
    /// intent rename is conservatively authoritative even if the following
    /// directory sync fails; it may survive a crash and must never be ignored.
    pub fn reset_with_kind(
        &self,
        revision: u64,
        snapshot: &Snapshot,
        kind: ResetKind,
    ) -> Result<SaveResult, String> {
        snapshot.validate()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Workspace mutex is poisoned".to_string())?;
        let result = SaveResult {
            revision,
            reset_epoch: snapshot.reset_epoch,
            written: false,
        };
        if let Some(intent) = &state.pending {
            if intent.snapshot == *snapshot && intent.kind == kind {
                self.finish_reset(intent)?;
                state.pending = None;
                state.revision = Some(
                    state
                        .revision
                        .map_or(revision, |previous| previous.max(revision)),
                );
                state.durable_revision = Some(revision);
                return Ok(SaveResult {
                    written: true,
                    ..result
                });
            }
            return Err(
                "An authoritative reset is pending; retry that reset before starting another"
                    .into(),
            );
        }
        if !state.writable && kind == ResetKind::Current {
            return Err("A newer workspace cannot be cleared partially; only Reset everything may replace it".into());
        }
        if state.revision.is_some_and(|saved| revision < saved)
            || state.durable_revision == Some(revision)
            || (state.writable && snapshot.reset_epoch <= state.epoch)
        {
            return Ok(result);
        }
        if kind == ResetKind::Current {
            for path in [&self.path, &sibling(&self.path, ".backup")] {
                if let LoadStatus::Future(version) = load(path)?.status {
                    state.writable = false;
                    return Err(format!(
                        "Workspace changed to newer schema {version}; only Reset everything may replace it"
                    ));
                }
            }
        }
        let intent = ResetIntent {
            snapshot: snapshot.clone(),
            kind,
        };
        let written = atomic_json(&sibling(&self.path, ".reset"), &intent);
        if match &written {
            Ok(()) => true,
            Err(e) => e.renamed,
        } {
            state.epoch = snapshot.reset_epoch;
            state.authoritative_reset_epoch = Some(snapshot.reset_epoch);
            state.revision = Some(revision);
            state.writable = true;
            state.pending = Some(intent);
        }
        written.map_err(|e| e.message)?;
        self.finish_reset(
            state
                .pending
                .as_ref()
                .expect("successful intent is pending"),
        )?;
        state.pending = None;
        state.durable_revision = Some(revision);
        Ok(SaveResult {
            written: true,
            ..result
        })
    }

    fn finish_reset(&self, intent: &ResetIntent) -> Result<(), String> {
        // Both copies must contain the cleared state before the barrier goes.
        atomic_json(&sibling(&self.path, ".backup"), &intent.snapshot).map_err(|e| e.message)?;
        atomic_json(&self.path, &intent.snapshot).map_err(|e| e.message)?;
        if intent.kind == ResetKind::All {
            self.remove_obsolete_copies()?;
        }
        let intent_path = sibling(&self.path, ".reset");
        #[cfg(windows)]
        let intent_path = {
            // Deletion has no Windows directory-fsync equivalent. Durably move
            // the barrier to an ignored temporary name first, so a lost unlink
            // cannot replay this reset over a later successfully saved state.
            let retired = sibling(&intent_path, &format!(".tmp-completed-{}", unique_suffix()));
            match platform::replace(&intent_path, &retired) {
                Ok(()) => retired,
                Err(e)
                    if e.kind() == std::io::ErrorKind::NotFound
                        && matches!(intent_path.try_exists(), Ok(false)) =>
                {
                    return Ok(());
                }
                Err(e) => {
                    return Err(error("retire completed reset intent", &intent_path, e));
                }
            }
        };
        match fs::remove_file(&intent_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(error("remove completed reset intent", &intent_path, e)),
        }
        #[cfg(unix)]
        sync_directory(parent(&self.path)?)?;
        Ok(())
    }

    fn remove_obsolete_copies(&self) -> Result<(), String> {
        let directory = parent(&self.path)?;
        let name = self
            .path
            .file_name()
            .ok_or("Workspace path has no file name")?
            .to_string_lossy();
        let prefixes = [
            format!("{name}.corrupt-"),
            format!("{name}.backup.corrupt-"),
            format!("{name}.tmp-"),
            format!("{name}.backup.tmp-"),
            format!("{name}.reset.tmp-"),
        ];
        for entry in fs::read_dir(directory)
            .map_err(|e| error("list obsolete workspace copies", directory, e))?
        {
            let entry = entry.map_err(|e| error("read workspace directory entry", directory, e))?;
            let entry_name = entry.file_name();
            let entry_name = entry_name.to_string_lossy();
            if prefixes.iter().any(|prefix| entry_name.starts_with(prefix)) {
                fs::remove_file(entry.path())
                    .map_err(|e| error("remove obsolete workspace copy", &entry.path(), e))?;
            }
        }
        #[cfg(unix)]
        sync_directory(directory)?;
        Ok(())
    }
}

fn default_path() -> Result<PathBuf, String> {
    let xdg = env::var_os("XDG_STATE_HOME");
    #[cfg(unix)]
    let (home, local_app_data) = (env::var_os("HOME"), None);
    #[cfg(windows)]
    let (home, local_app_data) = (
        absolute_directory(env::var_os("USERPROFILE"))
            .or_else(platform::profile_home)
            .map(PathBuf::into_os_string),
        absolute_directory(env::var_os("LOCALAPPDATA"))
            .or_else(platform::local_app_data)
            .map(PathBuf::into_os_string),
    );
    default_path_from(xdg, home, local_app_data)
}

fn absolute_directory(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    value.map(PathBuf::from).filter(|path| path.is_absolute())
}

// Pure path selection: tests never mutate the process environment.
fn default_path_from(
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    _local_app_data: Option<std::ffi::OsString>,
) -> Result<PathBuf, String> {
    let root = absolute_directory(xdg).or_else(|| {
        #[cfg(windows)]
        {
            absolute_directory(_local_app_data)
                .or_else(|| absolute_directory(home).map(|home| home.join("AppData/Local")))
        }
        #[cfg(target_os = "macos")]
        {
            absolute_directory(home).map(|home| home.join("Library/Application Support"))
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            absolute_directory(home).map(|home| home.join(".local/state"))
        }
    });
    root.map(|root| root.join("mexc-chart/workspace.json"))
        .ok_or_else(|| "No absolute per-user workspace directory is available (XDG_STATE_HOME, HOME, or Windows local application data/profile)".into())
}

fn parent(path: &Path) -> Result<&Path, String> {
    path.parent()
        .filter(|directory| !directory.as_os_str().is_empty())
        .ok_or_else(|| {
            format!(
                "Workspace file requires a parent directory: {}",
                path.display()
            )
        })
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn unique_suffix() -> String {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{}-{time}-{}",
        std::process::id(),
        UNIQUE.fetch_add(1, Ordering::Relaxed)
    )
}

fn error(action: &str, path: &Path, error: impl std::fmt::Display) -> String {
    format!("Cannot {action} {}: {error}", path.display())
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(error("read workspace file", path, e)),
    }
}

fn version_of(value: &serde_json::Value) -> Result<u64, String> {
    value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "schema_version must be an unsigned integer".to_string())
}

fn load(path: &Path) -> Result<Loaded, String> {
    let Some(bytes) = read_optional(path)? else {
        return Ok(Loaded::without_snapshot(LoadStatus::Missing));
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(e) => return Ok(Loaded::without_snapshot(LoadStatus::Corrupt(e.to_string()))),
    };
    let version = match version_of(&value) {
        Ok(version) => version,
        Err(e) => return Ok(Loaded::without_snapshot(LoadStatus::Corrupt(e))),
    };
    if version > u64::from(SCHEMA_VERSION) {
        return Ok(Loaded::without_snapshot(LoadStatus::Future(version)));
    }
    match Snapshot::from_value(value) {
        Ok(snapshot) => Ok(Loaded {
            status: LoadStatus::Valid,
            snapshot: Some(snapshot),
        }),
        Err(e) => Ok(Loaded::without_snapshot(LoadStatus::Corrupt(e))),
    }
}

fn quarantine(path: &Path) -> Result<PathBuf, String> {
    let preserved = sibling(path, &format!(".corrupt-{}", unique_suffix()));
    platform::replace(path, &preserved)
        .map_err(|e| error("preserve invalid workspace", path, e))?;
    #[cfg(unix)]
    sync_directory(parent(path)?)?;
    Ok(preserved)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|e| error("synchronize workspace directory", path, e))
}

fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), AtomicError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| AtomicError {
        message: format!("Cannot encode workspace: {e}"),
        renamed: false,
    })?;
    atomic_bytes(path, &bytes)
}

fn atomic_bytes(path: &Path, bytes: &[u8]) -> Result<(), AtomicError> {
    // Keep temporary writes beside their destination for atomic rename.
    let temporary = sibling(path, &format!(".tmp-{}", unique_suffix()));
    let mut renamed = false;
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&temporary)
            .map_err(|e| error("create workspace temporary file", &temporary, e))?;
        file.write_all(bytes)
            .map_err(|e| error("write workspace temporary file", &temporary, e))?;
        platform::sync_file(&file)
            .map_err(|e| error("synchronize workspace temporary file", &temporary, e))?;
        drop(file);
        if let Err(e) = platform::replace(&temporary, path) {
            // A write-through operation can report a late I/O failure. If the
            // source disappeared (or cannot be inspected), conservatively keep
            // reset authority rather than resurrecting an older checkpoint.
            renamed = !temporary.try_exists().unwrap_or(false);
            return Err(error("replace workspace checkpoint", path, e));
        }
        renamed = true;
        #[cfg(unix)]
        sync_directory(parent(path)?)?;
        Ok(())
    })();
    if let Err(message) = result {
        if !renamed {
            let _ = fs::remove_file(&temporary);
        }
        return Err(AtomicError { message, renamed });
    }
    Ok(())
}

#[cfg(test)]
#[path = "workspace_store_tests.rs"]
mod tests;
