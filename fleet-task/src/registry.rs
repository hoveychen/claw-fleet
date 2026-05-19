//! Runtime registry: each live `fleet-task` process writes a small JSON file
//! to `~/.fleet/runtime/<task_id>.json` so external tools (desktop monitor,
//! `fleet-cli task`) can discover it by scanning the directory.
//!
//! Stale entries (process gone) are filtered out at read time via a `kill(pid, 0)`
//! existence probe. Writers do best-effort cleanup on shutdown.
//!
//! Path layout uses `claw_fleet_task::paths::get_fleet_dir()` so FLEET_HOME
//! overrides apply uniformly with the rest of the workspace.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistryEntry {
    pub task_id: String,
    pub pid: u32,
    pub port: u16,
    /// RFC3339 timestamp of when the entry was first written.
    pub started_at: String,
}

#[derive(Debug)]
pub enum RegistryError {
    NoFleetHome,
    Io(std::io::Error),
    Serde(serde_json::Error),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::NoFleetHome => write!(f, "FLEET_HOME / $HOME unavailable"),
            RegistryError::Io(e) => write!(f, "registry io: {e}"),
            RegistryError::Serde(e) => write!(f, "registry serde: {e}"),
        }
    }
}

impl std::error::Error for RegistryError {}

impl From<std::io::Error> for RegistryError {
    fn from(e: std::io::Error) -> Self {
        RegistryError::Io(e)
    }
}

impl From<serde_json::Error> for RegistryError {
    fn from(e: serde_json::Error) -> Self {
        RegistryError::Serde(e)
    }
}

pub fn runtime_dir() -> Result<PathBuf, RegistryError> {
    let fleet = claw_fleet_task::paths::get_fleet_dir().ok_or(RegistryError::NoFleetHome)?;
    Ok(fleet.join("runtime"))
}

pub fn entry_path(task_id: &str) -> Result<PathBuf, RegistryError> {
    Ok(runtime_dir()?.join(format!("{task_id}.json")))
}

pub fn write(entry: &RegistryEntry) -> Result<(), RegistryError> {
    let dir = runtime_dir()?;
    fs::create_dir_all(&dir)?;
    let final_path = dir.join(format!("{}.json", entry.task_id));
    let tmp_path = dir.join(format!(".{}.json.tmp", entry.task_id));
    {
        let mut f = fs::File::create(&tmp_path)?;
        let bytes = serde_json::to_vec_pretty(entry)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp_path, &final_path)?;
    Ok(())
}

pub fn read(task_id: &str) -> Result<Option<RegistryEntry>, RegistryError> {
    let path = entry_path(task_id)?;
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn remove(task_id: &str) -> Result<(), RegistryError> {
    let path = entry_path(task_id)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Read every `<task_id>.json` under `runtime/`, filter out entries whose pid
/// no longer exists. Stale files are *not* deleted here — the next live writer
/// will overwrite, or callers can opt in via `prune_stale`.
pub fn list_alive() -> Result<Vec<RegistryEntry>, RegistryError> {
    let dir = match runtime_dir() {
        Ok(d) => d,
        Err(e) => return Err(e),
    };
    let read_dir = match fs::read_dir(&dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for ent in read_dir {
        let ent = match ent {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = ent.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !name.ends_with(".json") || name.starts_with('.') {
            continue;
        }
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let entry: RegistryEntry = match serde_json::from_slice(&bytes) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if pid_alive(entry.pid) {
            out.push(entry);
        }
    }
    Ok(out)
}

/// Delete entry files whose recorded pid is no longer alive. Returns the
/// number of stale files removed. Use when bootstrapping to clean up after
/// crashed predecessors.
pub fn prune_stale() -> Result<usize, RegistryError> {
    let dir = runtime_dir()?;
    let read_dir = match fs::read_dir(&dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let mut removed = 0usize;
    for ent in read_dir {
        let ent = match ent {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = ent.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !name.ends_with(".json") || name.starts_with('.') {
            continue;
        }
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let entry: RegistryEntry = match serde_json::from_slice(&bytes) {
            Ok(e) => e,
            Err(_) => {
                let _ = fs::remove_file(&path);
                removed += 1;
                continue;
            }
        };
        if !pid_alive(entry.pid) {
            let _ = fs::remove_file(&path);
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use claw_fleet_task::paths::fleet_home_lock;

    struct FleetHomeOverride {
        prev: Option<std::ffi::OsString>,
    }
    impl FleetHomeOverride {
        fn new(tmp: &std::path::Path) -> Self {
            let prev = std::env::var_os("FLEET_HOME");
            unsafe { std::env::set_var("FLEET_HOME", tmp) };
            FleetHomeOverride { prev }
        }
    }
    impl Drop for FleetHomeOverride {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(p) => std::env::set_var("FLEET_HOME", p),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
        }
    }

    fn entry(task_id: &str, pid: u32, port: u16) -> RegistryEntry {
        RegistryEntry {
            task_id: task_id.into(),
            pid,
            port,
            started_at: "2026-05-19T00:00:00Z".into(),
        }
    }

    #[test]
    fn write_then_read_roundtrips() {
        let _g = fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        let e = entry("task-a", std::process::id(), 12345);
        write(&e).unwrap();
        let got = read("task-a").unwrap().unwrap();
        assert_eq!(got, e);
    }

    #[test]
    fn remove_clears_entry() {
        let _g = fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        let e = entry("task-b", std::process::id(), 1);
        write(&e).unwrap();
        remove("task-b").unwrap();
        assert!(read("task-b").unwrap().is_none());
    }

    #[test]
    fn list_alive_filters_dead_pids() {
        let _g = fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        // own pid is alive; pid 1 is also alive on macOS (launchd) but for the
        // dead-pid test we use a large pid that's almost certainly unallocated.
        // signal 0 returns ESRCH for unknown pids, so kill(pid, 0) == 0 is false.
        let live = entry("task-live", std::process::id(), 1);
        let dead = entry("task-dead", 999_999, 2);
        write(&live).unwrap();
        write(&dead).unwrap();

        let alive = list_alive().unwrap();
        let ids: Vec<_> = alive.iter().map(|e| e.task_id.clone()).collect();
        assert!(ids.contains(&"task-live".to_string()));
        assert!(!ids.contains(&"task-dead".to_string()));
    }

    #[test]
    fn prune_stale_removes_dead_files() {
        let _g = fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        let live = entry("task-live2", std::process::id(), 1);
        let dead = entry("task-dead2", 999_998, 2);
        write(&live).unwrap();
        write(&dead).unwrap();

        let removed = prune_stale().unwrap();
        assert_eq!(removed, 1);
        assert!(read("task-live2").unwrap().is_some());
        assert!(read("task-dead2").unwrap().is_none());
    }

    #[test]
    fn list_alive_handles_missing_runtime_dir() {
        let _g = fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());
        // no write() call, so runtime/ doesn't exist yet
        let alive = list_alive().unwrap();
        assert!(alive.is_empty());
    }
}
