//! the next run of every command, saved so that a restart doesn't throw away what they
//! have already waited

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    env, fs,
    io::ErrorKind,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::Notify;

use crate::{TabCmd, window::Window};

/// identifies a command by its content, so that a reload (or a restart) can tell which
/// ones did not change
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CmdKey {
    name: Option<String>,
    /// with the constants evaluated (and printed in full), so `2*30` and `60` are the same
    delay: String,
    window: Option<String>,
    /// with the PCRON_ settings
    env: Vec<(String, String)>,
    script: String,
}

impl CmdKey {
    pub fn new(cmd: &TabCmd) -> Self {
        Self {
            name: cmd.name.clone(),
            delay: cmd.delay.to_string(),
            window: cmd.window.as_ref().map(Window::to_string),
            env: cmd.env.clone(),
            script: cmd.script.clone(),
        }
    }
}

pub type Saved = HashMap<CmdKey, Vec<DateTime<Utc>>>;

#[derive(Serialize, Deserialize)]
struct StateFile {
    tab: PathBuf,
    timers: Vec<SavedTimer>,
}

#[derive(Serialize, Deserialize)]
struct SavedTimer {
    key: CmdKey,
    /// RFC 3339
    next: String,
}

/// $PCRON_STATE, or else $XDG_STATE_HOME/pcron/timers-<hash of the tab's path>.json (or
/// ~/.local/state/...), so that servers of different tabs don't share it
pub fn path(tab: &Path) -> Option<PathBuf> {
    if let Some(state) = env::var_os("PCRON_STATE").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(state));
    }
    let absolute = |var| {
        env::var_os(var)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let dir = absolute("XDG_STATE_HOME")
        .or_else(|| absolute("HOME").map(|home| home.join(".local/state")))?;
    let tab = fs::canonicalize(tab).unwrap_or_else(|_| tab.to_path_buf());
    let hash = fnv1a(tab.as_os_str().as_bytes());
    Some(dir.join("pcron").join(format!("timers-{hash:016x}.json")))
}

// stable across Rust versions, unlike DefaultHasher
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// the saved next runs, by command (nothing if there is no file yet)
pub fn load(path: &Path) -> Result<Saved> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Saved::new()),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let file: StateFile =
        serde_json::from_str(&text).with_context(|| format!("cannot parse {}", path.display()))?;

    let mut saved = Saved::new();
    for timer in file.timers {
        if let Ok(next) = DateTime::parse_from_rfc3339(&timer.next) {
            saved.entry(timer.key).or_default().push(next.to_utc());
        }
    }
    Ok(saved)
}

/// the next run of each command loop of the server
#[derive(Default)]
pub struct Timers {
    next_id: AtomicU64,
    timers: Mutex<HashMap<u64, (CmdKey, DateTime<Utc>)>>,
    pub changed: Notify,
}

impl Timers {
    pub fn register(self: &Arc<Self>, key: CmdKey) -> TimerHandle {
        TimerHandle {
            timers: self.clone(),
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            key,
        }
    }

    /// written to a temporary file first, so that a crash can't leave half a file
    pub fn save(&self, path: &Path, tab: &Path) -> Result<()> {
        let mut timers: Vec<_> = self
            .timers
            .lock()
            .unwrap()
            .values()
            .map(|(key, next)| SavedTimer {
                key: key.clone(),
                next: next.to_rfc3339(),
            })
            .collect();
        timers.sort_by(|a, b| a.next.cmp(&b.next));
        let file = StateFile {
            tab: tab.to_path_buf(),
            timers,
        };

        let write = || -> Result<()> {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            let tmp = path.with_extension("json.tmp");
            fs::write(&tmp, serde_json::to_string_pretty(&file)?)?;
            fs::rename(&tmp, path)?;
            Ok(())
        };
        write().with_context(|| format!("cannot write {}", path.display()))
    }
}

/// the timer of one command loop; dropping it (when the loop stops) forgets it
pub struct TimerHandle {
    timers: Arc<Timers>,
    id: u64,
    key: CmdKey,
}

impl TimerHandle {
    pub fn set(&self, next: DateTime<Utc>) {
        let mut timers = self.timers.timers.lock().unwrap();
        timers.insert(self.id, (self.key.clone(), next));
        // notify_one keeps the notification if nobody is waiting right now
        self.timers.changed.notify_one();
    }
}

impl Drop for TimerHandle {
    fn drop(&mut self) {
        self.timers.timers.lock().unwrap().remove(&self.id);
        self.timers.changed.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file;

    #[test]
    fn save_and_load() {
        let dir = env::temp_dir().join(format!("pcron-state-test-{}", std::process::id()));
        let path = dir.join("pcron").join("timers.json");
        let cmds = file::parse("a: 60 $ x\n60 $ y\n60 $ y").unwrap().cmds;
        let keys: Vec<_> = cmds.iter().map(CmdKey::new).collect();

        let timers = Arc::new(Timers::default());
        let t = |s| DateTime::parse_from_rfc3339(s).unwrap().to_utc();
        let a = timers.register(keys[0].clone());
        let y1 = timers.register(keys[1].clone());
        let y2 = timers.register(keys[2].clone());
        a.set(t("2024-01-05T12:00:00Z"));
        y1.set(t("2024-01-05T13:00:00Z"));
        y2.set(t("2024-01-05T14:00:00Z"));
        drop(y2);
        timers.save(&path, Path::new("/some/tab")).unwrap();

        let saved = load(&path).unwrap();
        assert_eq!(saved[&keys[0]], [t("2024-01-05T12:00:00Z")]);
        assert_eq!(saved[&keys[1]], [t("2024-01-05T13:00:00Z")]);

        fs::remove_dir_all(&dir).unwrap();
        assert!(load(&path).unwrap().is_empty(), "no file, no timers");
    }

    #[test]
    fn a_file_per_tab() {
        let a = path(Path::new("/one/tab")).unwrap();
        let b = path(Path::new("/other/tab")).unwrap();
        assert_ne!(a, b);
        assert_eq!(fnv1a(b"hello"), 0xa430_d846_80aa_bd0b);
    }
}
