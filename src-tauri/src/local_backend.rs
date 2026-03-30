//! LocalBackend — file-based implementation that aggregates multiple agent sources.
//!
//! Owns a `Vec<Box<dyn AgentSource>>` and delegates session scanning, message
//! reading, and process management to the appropriate source.  Runs a `notify`
//! file-watcher thread for filesystem-based sources and periodic polling for
//! sources that require it.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::agent_source::{AgentSource, WatchStrategy};
use crate::backend::{Backend, WaitingAlert};
use crate::log_debug;
use crate::search_index::SearchIndex;
use crate::session::{SessionInfo, SessionStatus};

// ── Struct ────────────────────────────────────────────────────────────────────

/// Message sent to the dedicated indexer thread.
type IndexRequest = Vec<(String, String)>; // Vec<(jsonl_path, session_id)>

pub struct LocalBackend {
    app: AppHandle,
    /// Registered agent sources (Claude Code, Cursor, OpenClaw, …).
    sources: Arc<Vec<Box<dyn AgentSource>>>,
    sessions: Arc<Mutex<Vec<SessionInfo>>>,
    watch: Arc<crate::backend::WatchState>,
    /// Active waiting-input alerts, keyed by session ID.
    waiting_alerts: Arc<Mutex<HashMap<String, WaitingAlert>>>,
    /// Semantic outcome tags per session, set by background analysis.
    /// Cleared when a session transitions away from WaitingInput/Idle.
    session_outcomes: Arc<Mutex<HashMap<String, Vec<String>>>>,
    /// Audit cache: maps session ID → (last-scanned byte offset, cached events).
    /// Cleared when a session disappears from the active list.
    audit_cache: Arc<Mutex<HashMap<String, (u64, Vec<crate::audit::AuditEvent>)>>>,
    /// Full-text search index — used for read-only queries from the main thread.
    search_index: Arc<Mutex<SearchIndex>>,
    /// Channel to send indexing requests to the dedicated indexer thread.
    /// Kept alive so the indexer thread doesn't exit (dropping closes the channel).
    #[allow(dead_code)]
    index_tx: std::sync::mpsc::Sender<IndexRequest>,
    /// Kept alive so the watcher thread keeps running.
    /// Dropping this field closes the event channel and the thread exits.
    _watcher: RecommendedWatcher,
}

impl LocalBackend {
    pub fn new(
        app: AppHandle,
        locale: Arc<Mutex<String>>,
        sources: Vec<Box<dyn AgentSource>>,
    ) -> Self {
        let sources = Arc::new(sources);
        let sessions: Arc<Mutex<Vec<SessionInfo>>> = Arc::new(Mutex::new(Vec::new()));
        let watch = Arc::new(crate::backend::WatchState::new());
        let waiting_alerts: Arc<Mutex<HashMap<String, WaitingAlert>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let session_outcomes: Arc<Mutex<HashMap<String, Vec<String>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let audit_cache: Arc<Mutex<HashMap<String, (u64, Vec<crate::audit::AuditEvent>)>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Open (or create) the full-text search index.
        let search_index = Arc::new(Mutex::new(
            SearchIndex::open().unwrap_or_else(|e| {
                log_debug(&format!("search index open failed, retrying fresh: {e}"));
                // If the DB is corrupt, delete and retry.
                if let Some(home) = dirs::home_dir() {
                    let _ = fs::remove_file(home.join(".claude").join("fleet-search.db"));
                }
                SearchIndex::open().expect("search index open failed twice")
            }),
        ));

        // Dedicated indexer thread — receives session lists via channel,
        // coalesces rapid requests, and runs indexing off the scan threads.
        let (index_tx, index_rx) = std::sync::mpsc::channel::<IndexRequest>();
        {
            let idx = search_index.clone();
            std::thread::Builder::new()
                .name("fleet-search-indexer".into())
                .spawn(move || {
                    indexer_thread(idx, index_rx);
                })
                .expect("failed to spawn indexer thread");
        }

        // Initial scan — run in a background thread so the UI appears immediately.
        {
            let app_bg = app.clone();
            let sess_bg = sessions.clone();
            let sources_bg = sources.clone();
            let idx_tx = index_tx.clone();
            std::thread::spawn(move || {
                let initial = crate::session::scan_all_sources(&sources_bg);
                *sess_bg.lock().unwrap() = initial.clone();
                let _ = app_bg.emit("sessions-updated", &initial);
                let _ = app_bg.emit("scan-ready", true);
                crate::update_tray(&app_bg, &initial);

                // Send to indexer thread (non-blocking).
                let _ = idx_tx.send(sessions_to_index_request(&initial));
            });
        }

        // Set up filesystem watcher for all sources that use Filesystem strategy.
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher =
            RecommendedWatcher::new(tx, Config::default()).expect("failed to create file watcher");

        // Collect all trigger extensions across sources.
        let mut all_trigger_exts: HashSet<&'static str> = HashSet::new();
        // Track all dirs being watched so we don't add duplicates for memory paths.
        let mut watched_dirs: HashSet<std::path::PathBuf> = HashSet::new();
        for source in sources.iter() {
            if matches!(source.watch_strategy(), WatchStrategy::Filesystem) {
                for dir in source.watch_paths() {
                    if dir.is_dir() {
                        if let Err(e) = watcher.watch(&dir, RecursiveMode::Recursive) {
                            eprintln!("[LocalBackend] failed to watch {:?}: {}", dir, e);
                        } else {
                            watched_dirs.insert(dir);
                        }
                    }
                }
                for ext in source.trigger_extensions() {
                    all_trigger_exts.insert(ext);
                }
            }
        }

        // Also watch memory_watch_paths() for every source (filesystem or polling)
        // that stores memory outside of its session watch dirs.
        for source in sources.iter() {
            for dir in source.memory_watch_paths() {
                if dir.is_dir() && !watched_dirs.contains(&dir) {
                    if let Err(e) = watcher.watch(&dir, RecursiveMode::Recursive) {
                        eprintln!("[LocalBackend] failed to watch memory {:?}: {}", dir, e);
                    } else {
                        watched_dirs.insert(dir);
                    }
                }
            }
        }

        // Shared analyzing set — prevents duplicate analysis when both the
        // filesystem watcher and the polling thread detect the same transition.
        let analyzing: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

        // Clone Arcs for the watcher thread.
        let app2 = app.clone();
        let sess2 = sessions.clone();
        let sources2 = sources.clone();
        let wa2 = waiting_alerts.clone();
        let so2 = session_outcomes.clone();
        let locale2 = locale.clone();
        let watch2 = watch.clone();
        let analyzing2 = analyzing.clone();
        let idx_tx2 = index_tx.clone();

        // Filesystem watcher thread — batches events so we rescan at most once
        // every 2 seconds, while still tailing the viewed session immediately.
        std::thread::spawn(move || {
            let mut prev_statuses: HashMap<String, SessionStatus> = HashMap::new();
            let analyzing = analyzing2;
            let mut last_rescan = Instant::now();
            let mut last_memory_rescan = Instant::now();
            let rescan_interval = Duration::from_secs(2);
            let memory_rescan_interval = Duration::from_secs(1);
            let mut pending_rescan = false;
            let mut pending_memory_rescan = false;

            loop {
                // Wait for events; use a short timeout when a rescan is pending
                // so we flush it promptly after the coalescing window.
                let timeout = if pending_rescan || pending_memory_rescan {
                    let remaining_session = if pending_rescan {
                        rescan_interval.saturating_sub(last_rescan.elapsed())
                    } else {
                        Duration::from_secs(60)
                    };
                    let remaining_memory = if pending_memory_rescan {
                        memory_rescan_interval.saturating_sub(last_memory_rescan.elapsed())
                    } else {
                        Duration::from_secs(60)
                    };
                    remaining_session.min(remaining_memory)
                } else {
                    Duration::from_secs(60)
                };

                match rx.recv_timeout(timeout) {
                    Ok(Ok(event)) => {
                        if !matches!(
                            event.kind,
                            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                        ) {
                            continue;
                        }

                        for path in &event.paths {
                            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

                            // Memory file change: .md file inside a `memory/` directory.
                            if ext == "md" && path_is_in_memory_dir(path) {
                                pending_memory_rescan = true;
                            }

                            if !all_trigger_exts.contains(ext) {
                                continue;
                            }

                            pending_rescan = true;

                            // Tail the currently-viewed session immediately (keeps
                            // the detail view responsive even while rescans are batched).
                            if ext == "jsonl" {
                                if let Some(ref vpath) = watch2.current_path() {
                                    let path_str = path.to_string_lossy();
                                    if vpath == path_str.as_ref() {
                                        emit_tail_lines(path, &app2, &watch2);
                                    }
                                }
                            }
                        }
                    }
                    Ok(Err(_)) => {} // watch error, ignore
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }

                // Flush the batched session rescan once the coalescing window has elapsed.
                if pending_rescan && last_rescan.elapsed() >= rescan_interval {
                    rescan_and_emit(&sources2, &app2, &sess2, &so2);
                    detect_waiting_transitions(
                        &sess2,
                        &mut prev_statuses,
                        &analyzing,
                        &wa2,
                        &so2,
                        &app2,
                        &locale2,
                    );
                    // Send to indexer thread (non-blocking).
                    let _ = idx_tx2.send(sessions_to_index_request(&sess2.lock().unwrap()));
                    last_rescan = Instant::now();
                    pending_rescan = false;
                }

                // Flush the batched memory rescan — just emit the event; the
                // frontend calls `list_memories` itself.
                if pending_memory_rescan && last_memory_rescan.elapsed() >= memory_rescan_interval {
                    let _ = app2.emit("memories-updated", ());
                    last_memory_rescan = Instant::now();
                    pending_memory_rescan = false;
                }
            }
        });

        // Polling thread for sources with WatchStrategy::Poll.
        let has_poll_sources = sources.iter().any(|s| matches!(s.watch_strategy(), WatchStrategy::Poll(_)));
        if has_poll_sources {
            let app3 = app.clone();
            let sess3 = sessions.clone();
            let sources3 = sources.clone();
            let wa3 = waiting_alerts.clone();
            let so3 = session_outcomes.clone();
            let locale3 = locale.clone();
            let analyzing3 = analyzing.clone();
            let idx_tx3 = index_tx.clone();

            std::thread::spawn(move || {
                let mut prev_statuses: HashMap<String, SessionStatus> = HashMap::new();
                let analyzing = analyzing3;

                // Use the shortest poll interval among all polling sources.
                let interval = sources3
                    .iter()
                    .filter_map(|s| match s.watch_strategy() {
                        WatchStrategy::Poll(d) => Some(d),
                        _ => None,
                    })
                    .min()
                    .unwrap_or(Duration::from_secs(5));

                loop {
                    std::thread::sleep(interval);
                    rescan_and_emit(&sources3, &app3, &sess3, &so3);
                    detect_waiting_transitions(
                        &sess3,
                        &mut prev_statuses,
                        &analyzing,
                        &wa3,
                        &so3,
                        &app3,
                        &locale3,
                    );
                    // Send to indexer thread (non-blocking).
                    let _ = idx_tx3.send(sessions_to_index_request(&sess3.lock().unwrap()));
                }
            });
        }

        LocalBackend {
            app,
            sources,
            sessions,
            watch,
            waiting_alerts,
            session_outcomes,
            audit_cache,
            search_index,
            index_tx,
            _watcher: watcher,
        }
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Convert a session list into an index request (list of (path, id) pairs).
fn sessions_to_index_request(sessions: &[SessionInfo]) -> IndexRequest {
    sessions.iter().map(|s| (s.jsonl_path.clone(), s.id.clone())).collect()
}

/// Dedicated indexer thread. Receives session lists via channel, coalesces
/// rapid-fire requests, and runs incremental indexing without blocking scan threads.
fn indexer_thread(
    search_index: Arc<Mutex<SearchIndex>>,
    rx: std::sync::mpsc::Receiver<IndexRequest>,
) {
    loop {
        // Block until the first request arrives.
        let first = match rx.recv() {
            Ok(req) => req,
            Err(_) => break, // channel closed, exit
        };

        // Drain any additional pending requests (coalescing).
        // Only keep the latest one since it has the most up-to-date session list.
        let mut latest = first;
        while let Ok(newer) = rx.try_recv() {
            latest = newer;
        }

        // Now do the actual indexing work.
        if let Ok(idx) = search_index.lock() {
            idx.index_batch(&latest);

            let live: HashSet<String> = latest.iter().map(|(path, _)| path.clone()).collect();
            if let Err(e) = idx.cleanup_stale(&live) {
                log_debug(&format!("search index cleanup error: {e}"));
            }
        }
    }
}

/// Rescan all sources and emit updated sessions (with outcome tags injected).
fn rescan_and_emit(
    sources: &[Box<dyn AgentSource>],
    app: &AppHandle,
    sessions: &Arc<Mutex<Vec<SessionInfo>>>,
    outcomes: &Arc<Mutex<HashMap<String, Vec<String>>>>,
) {
    let mut s = crate::session::scan_all_sources(sources);

    // Inject cached outcome tags into each session.
    {
        let oc = outcomes.lock().unwrap();
        for sess in &mut s {
            if let Some(tags) = oc.get(&sess.id) {
                sess.last_outcome = Some(tags.clone());
            }
        }
    }

    *sessions.lock().unwrap() = s.clone();
    let _ = app.emit("sessions-updated", &s);
    crate::update_tray(app, &s);
}

/// Returns true when `path` is a `.md` file residing inside a `memory/`
/// directory — the marker we use to identify auto-memory files for any source.
fn path_is_in_memory_dir(path: &std::path::Path) -> bool {
    path.components().any(|c| c.as_os_str() == "memory")
}

fn emit_tail_lines(path: &std::path::Path, app: &AppHandle, watch: &crate::backend::WatchState) {
    let mut guard = watch.offset.lock().unwrap();
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

use crate::agent_source::find_source_for_path;

// ── Public kill helpers (used by ClaudeCodeSource and OpenClawSource) ────────

/// Kill a process by PID (with process tree cleanup).
pub fn kill_pid_impl(pid: u32) -> Result<(), String> {
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

        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2000));
            for &p in pids.iter().rev() {
                if unsafe { libc::kill(p as libc::pid_t, 0) } == 0 {
                    unsafe { libc::kill(p as libc::pid_t, libc::SIGKILL) };
                }
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
        Ok(())
    }
}

/// Kill all processes in a workspace.
pub fn kill_workspace_impl(workspace_path: &str) -> Result<(), String> {
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
            return Err(format!("No agent processes found in {}", workspace_path));
        }

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

        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2000));
            for &p in pids.iter().rev() {
                if unsafe { libc::kill(p as libc::pid_t, 0) } == 0 {
                    unsafe { libc::kill(p as libc::pid_t, libc::SIGKILL) };
                }
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
        Ok(())
    }
}

// ── Backend impl ──────────────────────────────────────────────────────────────

impl Backend for LocalBackend {
    fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions.lock().unwrap().clone()
    }

    fn get_messages(&self, path: &str) -> Result<Vec<Value>, String> {
        match find_source_for_path(&self.sources, path) {
            Some(source) => source.get_messages(path),
            None => Err(format!("No agent source can handle path: {path}")),
        }
    }

    fn kill_pid(&self, pid: u32) -> Result<(), String> {
        kill_pid_impl(pid)?;
        // Trigger a rescan after a delay.
        let app = self.app.clone();
        let sessions = self.sessions.clone();
        let sources = self.sources.clone();
        let outcomes = self.session_outcomes.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            rescan_and_emit(&sources, &app, &sessions, &outcomes);
        });
        Ok(())
    }

    fn kill_workspace(&self, workspace_path: String) -> Result<(), String> {
        kill_workspace_impl(&workspace_path)?;
        // Trigger a rescan after a delay.
        let app = self.app.clone();
        let sessions = self.sessions.clone();
        let sources = self.sources.clone();
        let outcomes = self.session_outcomes.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            rescan_and_emit(&sources, &app, &sessions, &outcomes);
        });
        Ok(())
    }

    fn account_info(&self) -> crate::backend::AccountInfoFuture {
        Box::pin(crate::account::fetch_account_info())
    }

    fn source_account(&self, source: &str) -> crate::backend::SourceDataFuture {
        let config = crate::agent_source::SourcesConfig::load();
        if !config.is_source_enabled(source) {
            let msg = format!("Source '{}' is disabled", source);
            return Box::pin(async move { Err(msg) });
        }
        // Clone Arc and move the blocking work into the future so the backend
        // mutex is released before the (potentially slow) HTTP / subprocess
        // call runs.  This lets multiple source fetches run in parallel.
        let sources = self.sources.clone();
        let source = source.to_string();
        Box::pin(async move {
            match crate::agent_source::find_source_by_api_name(&sources, &source) {
                Some(s) => s.fetch_account(),
                None => Err(format!("Unknown source: {source}")),
            }
        })
    }

    fn source_usage(&self, source: &str) -> crate::backend::SourceDataFuture {
        let config = crate::agent_source::SourcesConfig::load();
        if !config.is_source_enabled(source) {
            let msg = format!("Source '{}' is disabled", source);
            return Box::pin(async move { Err(msg) });
        }
        // Clone Arc and move the blocking work into the future so the backend
        // mutex is released before the (potentially slow) HTTP / subprocess
        // call runs.  This lets multiple source fetches run in parallel.
        let sources = self.sources.clone();
        let source = source.to_string();
        Box::pin(async move {
            match crate::agent_source::find_source_by_api_name(&sources, &source) {
                Some(s) => s.fetch_usage(),
                None => Err(format!("Unknown source: {source}")),
            }
        })
    }

    fn usage_summaries(&self) -> Vec<crate::backend::SourceUsageSummary> {
        self.sources
            .iter()
            .filter(|s| s.is_available())
            .filter_map(|s| s.usage_summary())
            .collect()
    }

    fn check_setup(&self) -> crate::backend::SetupStatus {
        let (cli_installed, cli_path) = crate::check_cli_installed();
        let claude_dir_exists = dirs::home_dir()
            .map(|h| h.join(".claude").is_dir())
            .unwrap_or(false);
        let sessions = self.sessions.lock().unwrap().clone();
        let detected_tools = crate::detect_installed_tools(&sessions);
        let logged_in = crate::account::read_keychain_credentials().is_ok();
        let has_sessions = !sessions.is_empty();

        crate::backend::SetupStatus {
            cli_installed,
            cli_path,
            claude_dir_exists,
            detected_tools,
            logged_in,
            has_sessions,
            credentials_valid: None,
        }
    }

    fn start_watch(&self, path: String) -> Result<u64, String> {
        // For non-filesystem sources (polling-based), no file to tail.
        let is_file_based = find_source_for_path(&self.sources, &path)
            .map(|s| matches!(s.watch_strategy(), WatchStrategy::Filesystem))
            .unwrap_or(false);

        if !is_file_based {
            self.watch.set(path, 0);
            return Ok(0);
        }

        let size = std::fs::metadata(&path)
            .map(|m| m.len())
            .map_err(|e| e.to_string())?;
        self.watch.set(path, size);
        Ok(size)
    }

    fn stop_watch(&self) {
        self.watch.clear();
    }

    fn list_memories(&self) -> Vec<crate::memory::WorkspaceMemory> {
        let mut all = Vec::new();
        for source in self.sources.iter() {
            all.extend(source.list_memories());
        }
        all
    }

    fn get_memory_content(&self, path: &str) -> Result<String, String> {
        for source in self.sources.iter() {
            if let Ok(content) = source.get_memory_content(path) {
                return Ok(content);
            }
        }
        Err("Memory file not found in any source".to_string())
    }

    fn get_memory_history(&self, path: &str) -> Vec<crate::memory::MemoryHistoryEntry> {
        for source in self.sources.iter() {
            let history = source.get_memory_history(path);
            if !history.is_empty() {
                return history;
            }
        }
        vec![]
    }

    fn list_skills(&self) -> Vec<crate::skills::SkillItem> {
        crate::skills::scan_all_skills()
    }

    fn get_skill_content(&self, path: &str) -> Result<String, String> {
        crate::skills::read_skill_file(path)
    }

    fn get_waiting_alerts(&self) -> Vec<WaitingAlert> {
        self.waiting_alerts.lock().unwrap().values().cloned().collect()
    }

    fn get_hooks_plan(&self) -> crate::hooks::HookSetupPlan {
        crate::hooks::plan_hook_setup()
    }

    fn apply_hooks(&self) -> Result<(), String> {
        crate::hooks::apply_hook_setup()
    }

    fn remove_hooks(&self) -> Result<(), String> {
        crate::hooks::remove_fleet_hooks()
    }

    fn get_sources_config(&self) -> Vec<crate::agent_source::SourceInfo> {
        crate::agent_source::get_sources_config_local()
    }

    fn set_source_enabled(&self, name: &str, enabled: bool) -> Result<(), String> {
        crate::agent_source::set_source_enabled_local(name, enabled)
    }

    fn get_audit_events(&self) -> crate::audit::AuditSummary {
        let all_sessions = self.sessions.lock().unwrap().clone();
        let active_ids: HashSet<String> = all_sessions
            .iter()
            .filter(|s| s.status != SessionStatus::Idle)
            .map(|s| s.id.clone())
            .collect();
        let sessions: Vec<_> = all_sessions
            .into_iter()
            .filter(|s| active_ids.contains(&s.id))
            .collect();
        let total = sessions.len();

        let mut cache = self.audit_cache.lock().unwrap();

        // Evict sessions that are no longer active.
        cache.retain(|id, _| active_ids.contains(id));

        let mut all_events = Vec::new();
        for session in &sessions {
            let path = &session.jsonl_path;
            let is_plain_path = !path.contains("://");

            if is_plain_path {
                // Incremental scan: only read bytes added since last scan.
                let file_size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                let (prev_offset, prev_events) = cache
                    .get(&session.id)
                    .cloned()
                    .unwrap_or((0, Vec::new()));

                if file_size <= prev_offset {
                    // File unchanged (or truncated) — reuse cached events.
                    all_events.extend(prev_events);
                    continue;
                }

                // Read only the new bytes from prev_offset → EOF.
                let new_messages = match fs::File::open(path) {
                    Ok(mut file) => {
                        if file.seek(SeekFrom::Start(prev_offset)).is_err() {
                            all_events.extend(prev_events);
                            continue;
                        }
                        let mut buf = String::new();
                        if file.read_to_string(&mut buf).is_err() {
                            all_events.extend(prev_events);
                            continue;
                        }
                        buf.lines()
                            .filter(|l| !l.trim().is_empty())
                            .filter_map(|l| serde_json::from_str(l).ok())
                            .collect::<Vec<Value>>()
                    }
                    Err(_) => {
                        all_events.extend(prev_events);
                        continue;
                    }
                };

                let new_events = crate::audit::extract_audit_events(&new_messages, session);
                let mut combined = prev_events;
                combined.extend(new_events);
                cache.insert(session.id.clone(), (file_size, combined.clone()));
                all_events.extend(combined);
            } else {
                // URI-prefixed path (cursor://, etc.) — full re-read via source.
                let source = self.sources.iter().find(|s| {
                    let prefix = s.uri_prefix();
                    !prefix.is_empty() && path.starts_with(prefix)
                });
                if let Some(src) = source {
                    if let Ok(messages) = src.get_messages(path) {
                        let events = crate::audit::extract_audit_events(&messages, session);
                        all_events.extend(events);
                    }
                }
            }
        }

        all_events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        crate::audit::AuditSummary {
            events: all_events,
            total_sessions_scanned: total,
        }
    }

    fn search_sessions(&self, query: &str, limit: usize) -> Vec<crate::search_index::SearchHit> {
        match self.search_index.lock() {
            Ok(idx) => idx.search(query, limit).unwrap_or_default(),
            Err(_) => vec![],
        }
    }
}

/// Fetch usage summaries from all available sources via trait dispatch.
/// All network I/O happens here, outside any Mutex guard.
pub fn fetch_usage_summaries_from_sources(sources: &[Box<dyn AgentSource>]) -> Vec<crate::backend::SourceUsageSummary> {
    sources
        .iter()
        .filter(|s| s.is_available())
        .filter_map(|s| s.usage_summary())
        .collect()
}

// ── Waiting-input detection & outcome analysis ──────────────────────────────

fn detect_waiting_transitions(
    sessions: &Arc<Mutex<Vec<SessionInfo>>>,
    prev_statuses: &mut HashMap<String, SessionStatus>,
    analyzing: &Arc<Mutex<HashSet<String>>>,
    waiting_alerts: &Arc<Mutex<HashMap<String, WaitingAlert>>>,
    session_outcomes: &Arc<Mutex<HashMap<String, Vec<String>>>>,
    app: &AppHandle,
    locale: &Arc<Mutex<String>>,
) {
    let current = sessions.lock().unwrap().clone();
    let mut alerts_changed = false;
    let busy_statuses = [
        SessionStatus::Thinking,
        SessionStatus::Executing,
        SessionStatus::Streaming,
        SessionStatus::Processing,
        SessionStatus::Active,
    ];

    for sess in &current {
        if sess.is_subagent {
            continue;
        }

        let prev = prev_statuses.get(&sess.id);
        let is_waiting = sess.status == SessionStatus::WaitingInput;
        let was_waiting = prev == Some(&SessionStatus::WaitingInput);
        let was_busy = prev.map_or(false, |p| busy_statuses.contains(p));

        // Session just transitioned to WaitingInput → run semantic analysis.
        if is_waiting && !was_waiting {
            let mut guard = analyzing.lock().unwrap();
            if guard.contains(&sess.id) {
                continue;
            }
            guard.insert(sess.id.clone());
            drop(guard);

            let session_id = sess.id.clone();
            let display_name = sess.ai_title.clone().unwrap_or_else(|| sess.workspace_name.clone());
            let jsonl_path = sess.jsonl_path.clone();
            let last_text = sess.last_message_preview.clone().unwrap_or_default();
            let wa = waiting_alerts.clone();
            let so = session_outcomes.clone();
            let an = analyzing.clone();
            let app_bg = app.clone();
            let lang = locale.lock().unwrap().clone();

            std::thread::spawn(move || {
                let analysis_text = extract_last_assistant_text(&jsonl_path, 1000)
                    .unwrap_or(last_text);

                let result = crate::claude_analyze::analyze_session_outcome(&analysis_text, &lang, &session_id);
                an.lock().unwrap().remove(&session_id);

                // Always store outcome tags for the mascot.
                if let Some(ref result) = result {
                    so.lock().unwrap().insert(session_id.clone(), result.tags.clone());
                }

                let has_needs_input = result.as_ref()
                    .map_or(false, |r| r.tags.contains(&"needs_input".to_string()));
                let mode = get_notification_mode(&app_bg);

                // Decide whether to create an in-app alert and/or OS notification.
                let should_alert = mode == "all" || has_needs_input;
                let should_os_notify = mode != "none" && (mode == "all" || has_needs_input);

                if should_alert {
                    let summary = result.as_ref().and_then(|r| r.summary.clone())
                        .unwrap_or_else(|| fallback_summary_for_tags(
                            result.as_ref().map(|r| r.tags.as_slice()).unwrap_or(&[])
                        ));
                    let alert = WaitingAlert {
                        session_id: session_id.clone(),
                        workspace_name: display_name.clone(),
                        summary: summary.clone(),
                        detected_at_ms: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                        jsonl_path: jsonl_path.clone(),
                    };
                    wa.lock().unwrap().insert(session_id, alert);
                    let alerts: Vec<WaitingAlert> =
                        wa.lock().unwrap().values().cloned().collect();
                    let _ = app_bg.emit("waiting-alerts-updated", &alerts);
                    if should_os_notify {
                        send_os_notification(&app_bg, &display_name, &summary);
                    }

                    // Play TTS from backend (blocks until done).
                    crate::play_tts_for_notification(&app_bg, &summary);
                }
            });
        } else if !is_waiting && was_waiting {
            // Session left WaitingInput → clear alert.
            if waiting_alerts.lock().unwrap().remove(&sess.id).is_some() {
                alerts_changed = true;
            }
        }

        // Session became busy again → clear stale outcome tags.
        if busy_statuses.contains(&sess.status) && !was_busy {
            session_outcomes.lock().unwrap().remove(&sess.id);
        }
    }

    {
        // Prune alerts for sessions that no longer exist or left WaitingInput.
        let waiting_ids: HashSet<String> = current
            .iter()
            .filter(|s| s.status == SessionStatus::WaitingInput)
            .map(|s| s.id.clone())
            .collect();
        let mut wa = waiting_alerts.lock().unwrap();
        let before = wa.len();
        wa.retain(|id, _| waiting_ids.contains(id));
        if wa.len() != before {
            alerts_changed = true;
        }
    }

    if alerts_changed {
        let alerts: Vec<WaitingAlert> =
            waiting_alerts.lock().unwrap().values().cloned().collect();
        let _ = app.emit("waiting-alerts-updated", &alerts);
    }

    prev_statuses.clear();
    for sess in &current {
        if !sess.is_subagent {
            prev_statuses.insert(sess.id.clone(), sess.status.clone());
        }
    }
}

fn extract_last_assistant_text(jsonl_path: &str, max_chars: usize) -> Option<String> {
    let content = fs::read_to_string(jsonl_path).ok()?;
    let lines: Vec<&str> = content.lines().rev().take(100).collect();

    for line in &lines {
        let msg: Value = serde_json::from_str(line).ok()?;
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let content = msg.get("message")?.get("content")?.as_array()?;
        for block in content.iter().rev() {
            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    let preview: String = text.chars().rev().take(max_chars).collect::<String>()
                        .chars().rev().collect();
                    return Some(preview);
                }
            }
        }
    }
    None
}

/// Produce a short fallback summary based on outcome tags when the LLM did not
/// return a SUMMARY field.
pub(crate) fn fallback_summary_for_tags(tags: &[String]) -> String {
    let first = tags.first().map(|s| s.as_str()).unwrap_or("reporting");
    match first {
        "needs_input"   => "Waiting for input".to_string(),
        "bug_fixed"     => "Bug fixed".to_string(),
        "feature_added" => "Feature added".to_string(),
        "stuck"         => "Agent is stuck".to_string(),
        "apologizing"   => "Agent ran into an issue".to_string(),
        "show_off"      => "Task completed".to_string(),
        "concerned"     => "Potential issues detected".to_string(),
        "confused"      => "Agent is confused".to_string(),
        "celebrating"   => "Task completed successfully".to_string(),
        "quick_fix"     => "Quick fix applied".to_string(),
        "overwhelmed"   => "Extensive changes made".to_string(),
        "scheming"      => "Planning next steps".to_string(),
        "reporting"     => "Status update".to_string(),
        _               => "Status update".to_string(),
    }
}

/// Read the current notification mode from AppState ("all" | "user_action" | "none").
pub(crate) fn get_notification_mode(app: &AppHandle) -> String {
    use tauri::Manager;
    app.try_state::<crate::AppState>()
        .map(|s| s.notification_mode.lock().unwrap().clone())
        .unwrap_or_else(|| "user_action".to_string())
}

pub(crate) fn send_os_notification(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app.notification().builder()
        .title(title)
        .body(body)
        .show()
    {
        log_debug(&format!("[notify] tauri notification failed: {e}"));
    } else {
        log_debug("[notify] tauri notification sent");
    }
}

// ── Unix process-tree helper ──────────────────────────────────────────────────

#[cfg(unix)]
pub(crate) fn collect_process_tree(root_pid: u32) -> Vec<u32> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{SourceUsageSummary, UsageBar};
    use serde_json::json;

    /// Minimal mock for local_backend tests (duplicated to avoid cross-module test deps).
    struct MockSource {
        name: &'static str,
        api_name: &'static str,
        prefix: &'static str,
        available: bool,
        account: Result<serde_json::Value, String>,
        usage: Result<serde_json::Value, String>,
        summary: Option<SourceUsageSummary>,
    }

    impl MockSource {
        fn new(name: &'static str, api_name: &'static str, prefix: &'static str) -> Self {
            Self {
                name, api_name, prefix,
                available: true,
                account: Err("n/a".into()),
                usage: Err("n/a".into()),
                summary: None,
            }
        }
    }

    impl AgentSource for MockSource {
        fn name(&self) -> &'static str { self.name }
        fn api_name(&self) -> &'static str { self.api_name }
        fn uri_prefix(&self) -> &'static str { self.prefix }
        fn is_available(&self) -> bool { self.available }
        fn scan_sessions(&self) -> Vec<crate::session::SessionInfo> { vec![] }
        fn get_messages(&self, _: &str) -> Result<Vec<serde_json::Value>, String> { Ok(vec![]) }
        fn watch_strategy(&self) -> WatchStrategy { WatchStrategy::Poll(Duration::from_secs(5)) }
        fn fetch_account(&self) -> Result<serde_json::Value, String> { self.account.clone() }
        fn fetch_usage(&self) -> Result<serde_json::Value, String> { self.usage.clone() }
        fn usage_summary(&self) -> Option<SourceUsageSummary> { self.summary.clone() }
    }

    #[test]
    fn fetch_usage_summaries_from_sources_collects_available_only() {
        let sources: Vec<Box<dyn AgentSource>> = vec![
            Box::new(MockSource {
                summary: Some(SourceUsageSummary {
                    source: "a".into(),
                    plan: Some("pro".into()),
                    bars: vec![UsageBar { label: "5h".into(), utilization: 0.3, resets_at: None }],
                }),
                ..MockSource::new("a", "a", "a://")
            }),
            Box::new(MockSource {
                available: false,
                summary: Some(SourceUsageSummary {
                    source: "b".into(),
                    plan: None,
                    bars: vec![],
                }),
                ..MockSource::new("b", "b", "b://")
            }),
            Box::new(MockSource::new("c", "c", "c://")), // no summary
        ];

        let result = fetch_usage_summaries_from_sources(&sources);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].source, "a");
        assert_eq!(result[0].bars.len(), 1);
        assert!((result[0].bars[0].utilization - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn fetch_usage_summaries_empty_sources() {
        let sources: Vec<Box<dyn AgentSource>> = vec![];
        let result = fetch_usage_summaries_from_sources(&sources);
        assert!(result.is_empty());
    }

    #[test]
    fn find_source_by_api_name_delegates_account_and_usage() {
        let sources: Vec<Box<dyn AgentSource>> = vec![
            Box::new(MockSource {
                account: Ok(json!({"plan": "max5x"})),
                usage: Ok(json!({"used": 42})),
                ..MockSource::new("claude-code", "claude", "")
            }),
            Box::new(MockSource {
                account: Ok(json!({"email": "u@example.com"})),
                usage: Ok(json!({"requests": 100})),
                ..MockSource::new("cursor", "cursor", "cursor://")
            }),
        ];

        // Simulate what LocalBackend::source_account does
        let s = crate::agent_source::find_source_by_api_name(&sources, "claude").unwrap();
        assert_eq!(s.fetch_account().unwrap()["plan"], "max5x");

        let s = crate::agent_source::find_source_by_api_name(&sources, "cursor").unwrap();
        assert_eq!(s.fetch_usage().unwrap()["requests"], 100);

        // Unknown source
        assert!(crate::agent_source::find_source_by_api_name(&sources, "unknown").is_none());
    }
}
