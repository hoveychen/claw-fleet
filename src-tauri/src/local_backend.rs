//! LocalBackend — file-based implementation that reads from ~/.claude/.
//!
//! Runs a `notify` file-watcher thread that emits `sessions-updated` and
//! `session-tail` Tauri events, replacing the former watcher.rs module and the
//! local branches that were scattered across lib.rs command handlers.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::account::AccountInfo;
use crate::backend::Backend;
use crate::session::{get_claude_dir, scan_sessions, SessionInfo};

// ── Struct ────────────────────────────────────────────────────────────────────

pub struct LocalBackend {
    app: AppHandle,
    sessions: Arc<Mutex<Vec<SessionInfo>>>,
    viewed_session: Arc<Mutex<Option<String>>>,
    viewed_offset: Arc<Mutex<u64>>,
    /// Kept alive so the watcher thread keeps running.
    /// Dropping this field closes the event channel and the thread exits.
    _watcher: RecommendedWatcher,
}

impl LocalBackend {
    pub fn new(app: AppHandle) -> Self {
        let sessions: Arc<Mutex<Vec<SessionInfo>>> = Arc::new(Mutex::new(Vec::new()));
        let viewed_session: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let viewed_offset: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));

        let claude_dir = get_claude_dir();

        // Synchronous initial scan so list_sessions() is populated immediately
        // after new() returns.
        if let Some(ref dir) = claude_dir {
            let initial = scan_sessions(dir);
            *sessions.lock().unwrap() = initial.clone();
            let _ = app.emit("sessions-updated", &initial);
            crate::update_tray(&app, &initial);
        }

        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher =
            RecommendedWatcher::new(tx, Config::default()).expect("failed to create file watcher");

        if let Some(ref dir) = claude_dir {
            if let Err(e) = watcher.watch(dir, RecursiveMode::Recursive) {
                eprintln!("[LocalBackend] failed to watch {:?}: {}", dir, e);
            }
        }

        // Clone Arcs for the watcher thread.
        let app2 = app.clone();
        let sess2 = sessions.clone();
        let vs = viewed_session.clone();
        let vo = viewed_offset.clone();
        let dir_thread = claude_dir;

        std::thread::spawn(move || {
            let Some(dir) = dir_thread else { return };
            let mut last_handled: HashMap<String, Instant> = HashMap::new();

            for result in rx {
                let Ok(event) = result else { break }; // sender dropped → exit

                if !matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ) {
                    continue;
                }

                for path in &event.paths {
                    let path_str = path.to_string_lossy().to_string();

                    // Per-path debounce (300 ms)
                    let now = Instant::now();
                    if let Some(last) = last_handled.get(&path_str) {
                        if now.duration_since(*last) < Duration::from_millis(300) {
                            continue;
                        }
                    }
                    last_handled.insert(path_str.clone(), now);

                    match path.extension().and_then(|e| e.to_str()) {
                        Some("lock") => rescan_and_emit(&dir, &app2, &sess2),
                        Some("jsonl") => {
                            rescan_and_emit(&dir, &app2, &sess2);
                            // If this is the currently-viewed session, tail new lines.
                            let viewed = vs.lock().unwrap().clone();
                            if let Some(ref vpath) = viewed {
                                if vpath == &path_str {
                                    emit_tail_lines(path, &app2, &vo);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        });

        LocalBackend {
            app,
            sessions,
            viewed_session,
            viewed_offset,
            _watcher: watcher,
        }
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn rescan_and_emit(
    dir: &std::path::Path,
    app: &AppHandle,
    sessions: &Arc<Mutex<Vec<SessionInfo>>>,
) {
    let s = scan_sessions(dir);
    *sessions.lock().unwrap() = s.clone();
    let _ = app.emit("sessions-updated", &s);
    crate::update_tray(app, &s);
}

fn emit_tail_lines(path: &std::path::Path, app: &AppHandle, offset: &Arc<Mutex<u64>>) {
    let mut guard = offset.lock().unwrap();
    let cur = *guard;

    let Ok(mut file) = fs::File::open(path) else { return };
    let Ok(size) = file.metadata().map(|m| m.len()) else { return };
    if size <= cur {
        return;
    }
    if file.seek(SeekFrom::Start(cur)).is_err() {
        return;
    }

    let mut buf = String::new();
    if file.read_to_string(&mut buf).is_err() {
        return;
    }

    let lines: Vec<Value> = buf
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    if !lines.is_empty() {
        *guard = size;
        let _ = app.emit("session-tail", &lines);
    }
}

// ── Backend impl ──────────────────────────────────────────────────────────────

impl Backend for LocalBackend {
    fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions.lock().unwrap().clone()
    }

    fn get_messages(&self, path: &str) -> Result<Vec<Value>, String> {
        let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Ok(content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect())
    }

    fn kill_workspace(&self, workspace_path: String) -> Result<(), String> {
        #[cfg(unix)]
        {
            use crate::session::scan_cli_processes;
            let procs = scan_cli_processes();
            let root_pids: Vec<u32> = procs
                .iter()
                .filter(|p| p.cwd == workspace_path)
                .map(|p| p.pid)
                .collect();

            if root_pids.is_empty() {
                return Err(format!("No claude processes found in {}", workspace_path));
            }

            // Collect the full process tree for all root pids.
            let mut all_pids: std::collections::HashSet<u32> = std::collections::HashSet::new();
            for &root in &root_pids {
                for pid in collect_process_tree(root) {
                    all_pids.insert(pid);
                }
            }
            let pids: Vec<u32> = all_pids.into_iter().collect();

            crate::log_debug(&format!(
                "kill_workspace: SIGTERM to {} pids for workspace '{}': {:?}",
                pids.len(),
                workspace_path,
                pids
            ));

            for &p in pids.iter().rev() {
                unsafe { libc::kill(p as libc::pid_t, libc::SIGTERM) };
            }

            let app = self.app.clone();
            let sessions = self.sessions.clone();
            let dir = get_claude_dir();

            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(500));
                if let Some(ref d) = dir {
                    rescan_and_emit(d, &app, &sessions);
                }
                std::thread::sleep(Duration::from_millis(1500));
                for &p in pids.iter().rev() {
                    if unsafe { libc::kill(p as libc::pid_t, 0) } == 0 {
                        unsafe { libc::kill(p as libc::pid_t, libc::SIGKILL) };
                    }
                }
                if let Some(ref d) = dir {
                    rescan_and_emit(d, &app, &sessions);
                }
            });

            Ok(())
        }

        #[cfg(not(unix))]
        {
            std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID"])
                .args(
                    crate::session::scan_cli_processes()
                        .iter()
                        .filter(|p| p.cwd == workspace_path)
                        .map(|p| p.pid.to_string())
                        .collect::<Vec<_>>(),
                )
                .status()
                .map_err(|e| format!("taskkill failed: {e}"))?;

            let app = self.app.clone();
            let sessions = self.sessions.clone();
            let dir = get_claude_dir();

            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(500));
                if let Some(ref d) = dir {
                    rescan_and_emit(d, &app, &sessions);
                }
            });

            Ok(())
        }
    }

    fn kill_pid(&self, pid: u32) -> Result<(), String> {
        #[cfg(unix)]
        {
            let pids = collect_process_tree(pid);
            crate::log_debug(&format!(
                "kill_pid: SIGTERM to {} pids (root={}): {:?}",
                pids.len(),
                pid,
                pids
            ));
            for &p in pids.iter().rev() {
                unsafe { libc::kill(p as libc::pid_t, libc::SIGTERM) };
            }

            let app = self.app.clone();
            let sessions = self.sessions.clone();
            let dir = get_claude_dir();

            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(500));
                if let Some(ref d) = dir {
                    rescan_and_emit(d, &app, &sessions);
                }
                std::thread::sleep(Duration::from_millis(1500));
                for &p in pids.iter().rev() {
                    if unsafe { libc::kill(p as libc::pid_t, 0) } == 0 {
                        unsafe { libc::kill(p as libc::pid_t, libc::SIGKILL) };
                    }
                }
                if let Some(ref d) = dir {
                    rescan_and_emit(d, &app, &sessions);
                }
            });

            Ok(())
        }

        #[cfg(not(unix))]
        {
            std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .status()
                .map_err(|e| format!("taskkill failed: {e}"))?;

            let app = self.app.clone();
            let sessions = self.sessions.clone();
            let dir = get_claude_dir();

            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(500));
                if let Some(ref d) = dir {
                    rescan_and_emit(d, &app, &sessions);
                }
            });

            Ok(())
        }
    }

    fn account_info(&self) -> Result<AccountInfo, String> {
        crate::account::fetch_account_info()
    }

    fn start_watch(&self, path: String) -> Result<u64, String> {
        let size = std::fs::metadata(&path)
            .map(|m| m.len())
            .map_err(|e| e.to_string())?;
        *self.viewed_session.lock().unwrap() = Some(path);
        *self.viewed_offset.lock().unwrap() = size;
        Ok(size)
    }

    fn stop_watch(&self) {
        *self.viewed_session.lock().unwrap() = None;
        *self.viewed_offset.lock().unwrap() = 0;
    }

    fn list_memories(&self) -> Vec<crate::memory::WorkspaceMemory> {
        crate::memory::scan_all_memories()
    }

    fn get_memory_content(&self, path: &str) -> Result<String, String> {
        crate::memory::read_memory_file(path)
    }

    fn get_memory_history(&self, path: &str) -> Vec<crate::memory::MemoryHistoryEntry> {
        crate::memory::trace_memory_history(path)
    }
}

// ── Unix process-tree helper ──────────────────────────────────────────────────

#[cfg(unix)]
fn collect_process_tree(root_pid: u32) -> Vec<u32> {
    let output = match std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![root_pid],
    };
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let pid: u32 = match parts.next().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        let ppid: u32 = match parts.next().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        children.entry(ppid).or_default().push(pid);
    }

    let mut result = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(root_pid);
    while let Some(pid) = queue.pop_front() {
        result.push(pid);
        if let Some(kids) = children.get(&pid) {
            for &kid in kids {
                queue.push_back(kid);
            }
        }
    }
    result
}
