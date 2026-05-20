//! Watches `~/.fleet/runtime/<task_id>.json` for live `fleet-task` processes
//! and pushes change events to the frontend. Replaces the legacy
//! `LocalBackend::sessions` table as the source of truth for *running* tasks
//! in Phase 3 (the `tasks/` JSON files remain authoritative for plan state).
//!
//! Wire-up: started once at `LocalBackend::new` time, polls
//! `claw_fleet_core::registry::list_alive()` every second, diffs against the
//! previous snapshot, and emits one of three Tauri events per change:
//!
//! - `runtime-task-appeared` (payload: `RegistryEntry`)
//! - `runtime-task-disappeared` (payload: `{ task_id }`)
//! - `runtime-task-changed` (payload: `RegistryEntry` — e.g. port reused
//!   after a restart)
//!
//! The diff logic in `diff_state` is split out so tests can verify it
//! without instantiating Tauri.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use claw_fleet_core::registry::{self, RegistryEntry};

const POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct StateDiff {
    pub appeared: Vec<RegistryEntry>,
    pub disappeared: Vec<String>,
    pub changed: Vec<RegistryEntry>,
}

impl StateDiff {
    pub fn is_empty(&self) -> bool {
        self.appeared.is_empty() && self.disappeared.is_empty() && self.changed.is_empty()
    }
}

#[derive(Debug, Serialize, Clone)]
struct DisappearedPayload<'a> {
    task_id: &'a str,
}

pub struct RuntimeRegistryWatcher {
    state: Arc<Mutex<HashMap<String, RegistryEntry>>>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl RuntimeRegistryWatcher {
    pub fn start(app: AppHandle) -> Self {
        let state: Arc<Mutex<HashMap<String, RegistryEntry>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let state_thread = state.clone();
        let stop_thread = stop.clone();
        let join = thread::Builder::new()
            .name("fleet-runtime-watcher".into())
            .spawn(move || {
                while !stop_thread.load(Ordering::SeqCst) {
                    let current = match registry::list_alive() {
                        Ok(entries) => entries.into_iter().map(|e| (e.task_id.clone(), e)).collect(),
                        Err(_) => HashMap::new(),
                    };
                    let mut prev = state_thread.lock().unwrap();
                    let diff = diff_state(&prev, &current);
                    *prev = current;
                    drop(prev);

                    for e in &diff.appeared {
                        let _ = app.emit("runtime-task-appeared", e);
                    }
                    for tid in &diff.disappeared {
                        let _ = app
                            .emit("runtime-task-disappeared", DisappearedPayload { task_id: tid });
                    }
                    for e in &diff.changed {
                        let _ = app.emit("runtime-task-changed", e);
                    }

                    thread::sleep(POLL_INTERVAL);
                }
            })
            .expect("spawn runtime registry watcher");

        RuntimeRegistryWatcher {
            state,
            stop,
            join: Some(join),
        }
    }

    pub fn snapshot(&self) -> Vec<RegistryEntry> {
        self.state.lock().unwrap().values().cloned().collect()
    }
}

impl Drop for RuntimeRegistryWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

pub fn diff_state(
    prev: &HashMap<String, RegistryEntry>,
    current: &HashMap<String, RegistryEntry>,
) -> StateDiff {
    let mut diff = StateDiff::default();
    for (id, entry) in current {
        match prev.get(id) {
            None => diff.appeared.push(entry.clone()),
            Some(old) if old != entry => diff.changed.push(entry.clone()),
            _ => {}
        }
    }
    for id in prev.keys() {
        if !current.contains_key(id) {
            diff.disappeared.push(id.clone());
        }
    }
    diff.appeared.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    diff.disappeared.sort();
    diff.changed.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    diff
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(task_id: &str, pid: u32, port: u16) -> RegistryEntry {
        RegistryEntry {
            task_id: task_id.into(),
            pid,
            port,
            started_at: "2026-05-20T00:00:00Z".into(),
        }
    }

    fn map(entries: Vec<RegistryEntry>) -> HashMap<String, RegistryEntry> {
        entries.into_iter().map(|e| (e.task_id.clone(), e)).collect()
    }

    #[test]
    fn diff_finds_newly_appeared() {
        let prev = HashMap::new();
        let current = map(vec![entry("a", 1, 100), entry("b", 2, 200)]);
        let d = diff_state(&prev, &current);
        assert_eq!(d.appeared.len(), 2);
        assert_eq!(d.appeared[0].task_id, "a");
        assert_eq!(d.appeared[1].task_id, "b");
        assert!(d.disappeared.is_empty());
        assert!(d.changed.is_empty());
    }

    #[test]
    fn diff_finds_disappeared() {
        let prev = map(vec![entry("a", 1, 100), entry("b", 2, 200)]);
        let current = map(vec![entry("a", 1, 100)]);
        let d = diff_state(&prev, &current);
        assert!(d.appeared.is_empty());
        assert_eq!(d.disappeared, vec!["b".to_string()]);
        assert!(d.changed.is_empty());
    }

    #[test]
    fn diff_finds_port_change() {
        let prev = map(vec![entry("a", 1, 100)]);
        let current = map(vec![entry("a", 1, 101)]);
        let d = diff_state(&prev, &current);
        assert!(d.appeared.is_empty());
        assert!(d.disappeared.is_empty());
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.changed[0].port, 101);
    }

    #[test]
    fn diff_no_changes() {
        let m = map(vec![entry("a", 1, 100)]);
        assert!(diff_state(&m, &m).is_empty());
    }
}
