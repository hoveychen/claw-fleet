//! Filesystem watcher for `~/.fleet/tasks/`. Phase 5 P2.
//!
//! When `fleet-task new` (or any out-of-process tool) writes / updates /
//! deletes a task json, this watcher notices and pushes a single
//! `tasks-updated` Tauri event so the frontend store can re-invoke
//! `list_tasks` without the user having to switch projects.
//!
//! Companion to `runtime_registry::RuntimeRegistryWatcher`, which covers
//! the runtime/ directory (live task processes). Together they make the
//! desktop reactive to fleet-task CLI activity in real time.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter};

use claw_fleet_core::paths;

/// Debounce window — multiple writes to the same task json within this
/// interval collapse into one emitted event. Keeps the frontend from
/// thrashing during the multi-write sequence inside `actions::start_task`.
const DEBOUNCE: Duration = Duration::from_millis(150);

pub struct TasksDirWatcher {
    /// Kept alive so the underlying notify channel doesn't close.
    _watcher: Option<RecommendedWatcher>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl TasksDirWatcher {
    pub fn start(app: AppHandle) -> Self {
        let Some(fleet_dir) = paths::get_fleet_dir() else {
            eprintln!("[TasksDirWatcher] no fleet dir; watcher disabled");
            return Self::empty();
        };
        let tasks_dir = fleet_dir.join("tasks");
        // Best-effort create — first run hasn't written any task yet.
        let _ = std::fs::create_dir_all(&tasks_dir);

        let (tx, rx) = mpsc::channel();
        let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("[TasksDirWatcher] notify init failed: {e}");
                return Self::empty();
            }
        };
        if let Err(e) = watcher.watch(&tasks_dir, RecursiveMode::NonRecursive) {
            eprintln!(
                "[TasksDirWatcher] failed to watch {}: {e}",
                tasks_dir.display()
            );
            return Self::empty();
        }

        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let join = thread::Builder::new()
            .name("fleet-tasks-watcher".into())
            .spawn(move || {
                let mut pending = false;
                let mut last_event = std::time::Instant::now()
                    - Duration::from_secs(60);
                loop {
                    if stop_thread.load(Ordering::SeqCst) {
                        return;
                    }
                    match rx.recv_timeout(DEBOUNCE) {
                        Ok(Ok(event)) => {
                            if matches!(
                                event.kind,
                                EventKind::Create(_)
                                    | EventKind::Modify(_)
                                    | EventKind::Remove(_)
                            ) {
                                pending = true;
                                last_event = std::time::Instant::now();
                            }
                        }
                        Ok(Err(_)) => continue,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            // Debounce window elapsed — flush.
                            if pending && last_event.elapsed() >= DEBOUNCE {
                                pending = false;
                                let _ = app.emit("tasks-updated", ());
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
            })
            .expect("spawn tasks dir watcher");

        Self {
            _watcher: Some(watcher),
            stop,
            join: Some(join),
        }
    }

    fn empty() -> Self {
        Self {
            _watcher: None,
            stop: Arc::new(AtomicBool::new(true)),
            join: None,
        }
    }
}

impl Drop for TasksDirWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}
