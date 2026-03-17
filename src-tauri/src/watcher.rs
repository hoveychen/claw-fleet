use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::session::{scan_sessions, SessionInfo};

pub struct WatcherState {
    pub sessions: Arc<Mutex<Vec<SessionInfo>>>,
    pub viewed_session: Arc<Mutex<Option<String>>>, // jsonl_path currently viewed
    pub viewed_offset: Arc<Mutex<u64>>,             // byte offset for tail
}

pub fn run(app_handle: AppHandle, state: Arc<WatcherState>) {
    let claude_dir = match crate::session::get_claude_dir() {
        Some(d) => d,
        None => {
            eprintln!("[watcher] cannot find ~/.claude dir");
            return;
        }
    };

    // Initial scan
    let initial = scan_sessions(&claude_dir);
    *state.sessions.lock().unwrap() = initial.clone();
    let _ = app_handle.emit("sessions-updated", &initial);
    crate::update_tray(&app_handle, &initial);

    // Set up file watcher
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("[watcher] failed to create watcher: {}", e);
            return;
        }
    };

    if let Err(e) = watcher.watch(&claude_dir, RecursiveMode::Recursive) {
        eprintln!("[watcher] failed to watch {:?}: {}", claude_dir, e);
        return;
    }

    // Simple per-path debounce: only process once per 300ms per file
    let mut last_handled: HashMap<String, Instant> = HashMap::new();

    for result in rx {
        let Ok(event) = result else { continue };

        // Filter relevant event kinds
        let relevant = matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        );
        if !relevant {
            continue;
        }

        for path in &event.paths {
            let path_str = path.to_string_lossy().to_string();

            // Debounce
            let now = Instant::now();
            if let Some(last) = last_handled.get(&path_str) {
                if now.duration_since(*last) < Duration::from_millis(300) {
                    continue;
                }
            }
            last_handled.insert(path_str.clone(), now);

            let ext = path.extension().and_then(|e| e.to_str());
            match ext {
                Some("lock") => {
                    // IDE session changed → full rescan
                    handle_rescan(&claude_dir, &app_handle, &state);
                }
                Some("jsonl") => {
                    handle_jsonl_change(path, &path_str, &claude_dir, &app_handle, &state);
                }
                _ => {}
            }
        }
    }
}

fn handle_rescan(claude_dir: &Path, app_handle: &AppHandle, state: &WatcherState) {
    let sessions = scan_sessions(claude_dir);
    *state.sessions.lock().unwrap() = sessions.clone();
    let _ = app_handle.emit("sessions-updated", &sessions);
    crate::update_tray(app_handle, &sessions);
}

fn handle_jsonl_change(
    path: &Path,
    path_str: &str,
    claude_dir: &Path,
    app_handle: &AppHandle,
    state: &WatcherState,
) {
    // Full sessions rescan (only parses the changed file minimally; scan_sessions is cheap)
    handle_rescan(claude_dir, app_handle, state);

    // Tail: if this is the currently viewed session, emit new lines
    let viewed = state.viewed_session.lock().unwrap().clone();
    if let Some(ref vpath) = viewed {
        if vpath == path_str {
            emit_tail_lines(path, app_handle, state);
        }
    }
}

fn emit_tail_lines(path: &Path, app_handle: &AppHandle, state: &WatcherState) {
    let mut offset = state.viewed_offset.lock().unwrap();
    let current_offset = *offset;

    let Ok(mut file) = fs::File::open(path) else {
        return;
    };
    let Ok(file_size) = file.metadata().map(|m| m.len()) else {
        return;
    };
    if file_size <= current_offset {
        return;
    }

    if file.seek(SeekFrom::Start(current_offset)).is_err() {
        return;
    }

    let mut buf = String::new();
    if file.read_to_string(&mut buf).is_err() {
        return;
    }

    let new_lines: Vec<Value> = buf
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    if !new_lines.is_empty() {
        *offset = file_size;
        let _ = app_handle.emit("session-tail", &new_lines);
    }
}
