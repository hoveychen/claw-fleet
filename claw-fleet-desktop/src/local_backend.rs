//! LocalBackend — file-based implementation that aggregates multiple agent sources.
//!
//! Owns a `Vec<Box<dyn AgentSource>>` and delegates session scanning, message
//! reading, and process management to the appropriate source.  Runs a `notify`
//! file-watcher thread for filesystem-based sources and periodic polling for
//! sources that require it.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

/// Mutex on "a session scan is in flight". The initial scan thread and the
/// polling thread share one of these so they never run concurrently — on
/// Windows with massive Cursor history a single scan can pull GB-sized JSON
/// blobs into memory; two concurrent scans push the process into pagefile
/// thrashing and freeze the webview that lives in the same process.
pub(crate) struct ScanGate(AtomicBool);

impl ScanGate {
    pub(crate) fn new() -> Self { Self(AtomicBool::new(false)) }

    /// Returns `Some(guard)` if this caller now owns the scan slot.
    /// Returns `None` when another scan is in flight — the caller should
    /// skip this round. The slot is released when the returned guard drops
    /// (including on panic).
    pub(crate) fn try_enter(&self) -> Option<ScanGuard<'_>> {
        if self
            .0
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            Some(ScanGuard(&self.0))
        } else {
            None
        }
    }
}

pub(crate) struct ScanGuard<'a>(&'a AtomicBool);

impl<'a> Drop for ScanGuard<'a> {
    fn drop(&mut self) { self.0.store(false, Ordering::SeqCst); }
}

pub struct LocalBackend {
    app: AppHandle,
    /// Registered agent sources (Claude Code, Cursor, OpenClaw, …).
    sources: Arc<Vec<Box<dyn AgentSource>>>,
    sessions: Arc<Mutex<Vec<SessionInfo>>>,
    watch: Arc<crate::WatchState>,
    /// Active waiting-input alerts, keyed by session ID.
    waiting_alerts: Arc<Mutex<HashMap<String, WaitingAlert>>>,
    /// Semantic outcome tags per session, set by background analysis.
    /// Cleared when a session transitions away from WaitingInput/Idle.
    session_outcomes: Arc<Mutex<HashMap<String, Vec<String>>>>,
    /// Audit cache: maps session ID → (last-scanned byte offset, cached events).
    /// Cleared when a session disappears from the active list.
    audit_cache: Arc<Mutex<HashMap<String, (u64, Vec<crate::audit::AuditEvent>)>>>,
    /// Persistent audit history — events from sessions that went idle are saved
    /// here so they survive process restarts.
    audit_history: Arc<Mutex<crate::audit::AuditHistory>>,
    /// Full-text search index — used for read-only queries from the main thread.
    search_index: Arc<Mutex<SearchIndex>>,
    /// Channel to send indexing requests to the dedicated indexer thread.
    /// Kept alive so the indexer thread doesn't exit (dropping closes the channel).
    #[allow(dead_code)]
    index_tx: std::sync::mpsc::Sender<IndexRequest>,
    /// Daily report store — SQLite-backed, shared with the scheduler thread.
    report_store: Arc<Mutex<crate::daily_report::ReportStore>>,
    /// User's UI locale (e.g. "en", "zh").
    locale: Arc<Mutex<String>>,
    /// LLM provider configuration (which CLI + models to use for analysis).
    llm_config: Arc<Mutex<crate::llm_provider::LlmConfig>>,
    /// Tracks the last wall-clock time we spawned an auto-resume for a given
    /// session, keyed by session id. Prevents the scheduler from firing twice
    /// within a short debounce window if two rescans land back-to-back.
    /// Kept alive so the watcher thread keeps running.
    /// Dropping this field closes the event channel and the thread exits.
    _watcher: RecommendedWatcher,
    /// Shared cancellation flag. Long-running threads (poll, heartbeat, guard /
    /// elicitation / plan-approval directory watchers) check this at the top of
    /// each loop iteration. `Drop` flips it to false so threads exit when this
    /// backend is replaced (e.g. during `connect_remote` / `disconnect_remote`).
    /// Without this, successive remote↔local swaps leave old watcher threads
    /// running, and every one of them emits the same `elicitation-request` /
    /// `guard-request` event, producing duplicate decision tabs.
    running: Arc<AtomicBool>,
}

impl Drop for LocalBackend {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

impl LocalBackend {
    /// Workspace paths the file explorer may browse. Prefers the session
    /// cache; falls back to a direct source scan when the first background
    /// scan hasn't populated it yet.
    fn known_workspaces(&self) -> Vec<String> {
        let mut paths: Vec<String> = self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.workspace_path.clone())
            .collect();
        if paths.is_empty() {
            paths = self
                .sources
                .iter()
                .flat_map(|s| s.scan_sessions())
                .map(|s| s.workspace_path)
                .collect();
        }
        paths.sort();
        paths.dedup();
        paths
    }

    /// Re-stamp the cached sessions from the on-disk mark / read state and push
    /// them to the frontend. The mark and read toggles only write a file, so
    /// without this the launchpad's segment counts and unread badge wouldn't
    /// move until the next natural rescan. Re-runs the enrichers rather than
    /// patching the one row by hand so this can't drift from the scan path.
    /// Holds the lock across the enrich so a concurrent rescan can't be
    /// clobbered by a stale snapshot.
    fn restamp_marks_and_emit(&self) {
        let snapshot = {
            let mut list = self.sessions.lock().unwrap();
            claw_fleet_core::session_mark::enrich_sessions(&mut list);
            claw_fleet_core::session_read::enrich_sessions(&mut list);
            list.clone()
        };
        let _ = self.app.emit("sessions-updated", &snapshot);
        crate::update_tray(&self.app, &snapshot);
        publish_mobile_sessions(&snapshot);
    }

    pub fn new(
        app: AppHandle,
        locale: Arc<Mutex<String>>,
        llm_config: Arc<Mutex<crate::llm_provider::LlmConfig>>,
        sources: Vec<Box<dyn AgentSource>>,
    ) -> Self {
        let _t0 = std::time::Instant::now();
        macro_rules! step {
            ($label:expr) => {
                let elapsed = _t0.elapsed().as_millis();
                crate::log_debug(&format!("[BACKEND-INIT] {} at +{}ms", $label, elapsed));
            };
        }
        step!("start");

        let sources = Arc::new(sources);
        let sessions: Arc<Mutex<Vec<SessionInfo>>> = Arc::new(Mutex::new(Vec::new()));

        // Let the mobile relay pull the current snapshot the instant a phone
        // connects, instead of leaving it to sit on a blank task list until the
        // next scan-driven change (which never comes while every session is
        // idle — the fs watcher only rescans on a jsonl write). `fleet serve`
        // force-pushes on new-client presence in its own loop, so this provider
        // is the equivalent hook for the desktop LocalBackend path.
        {
            let sess_for_relay = sessions.clone();
            claw_fleet_core::mobile_relay::set_snapshot_provider(move || {
                let list = sess_for_relay.lock().ok()?;
                serde_json::to_value(&*list).ok()
            });
        }

        let watch = Arc::new(crate::WatchState::new());
        let waiting_alerts: Arc<Mutex<HashMap<String, WaitingAlert>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let session_outcomes: Arc<Mutex<HashMap<String, Vec<String>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let audit_cache: Arc<Mutex<HashMap<String, (u64, Vec<crate::audit::AuditEvent>)>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let audit_history: Arc<Mutex<crate::audit::AuditHistory>> =
            Arc::new(Mutex::new(crate::audit::AuditHistory::load()));

        step!("allocs done");

        // Open (or create) the full-text search index.
        let search_index = Arc::new(Mutex::new(
            SearchIndex::open().unwrap_or_else(|e| {
                log_debug(&format!("search index open failed, retrying fresh: {e}"));
                // If the DB is corrupt, delete and retry.
                if let Some(home) = crate::session::real_home_dir() {
                    let _ = fs::remove_file(home.join(".fleet").join("fleet-search.db"));
                }
                SearchIndex::open().expect("search index open failed twice")
            }),
        ));

        // Open (or create) the daily report store.
        let report_store = Arc::new(Mutex::new(
            crate::daily_report::ReportStore::open().unwrap_or_else(|e| {
                log_debug(&format!("report store open failed, retrying fresh: {e}"));
                if let Some(home) = crate::session::real_home_dir() {
                    let _ = fs::remove_file(home.join(".fleet").join("fleet-reports.db"));
                }
                crate::daily_report::ReportStore::open().expect("report store open failed twice")
            }),
        ));

        step!("DBs opened");

        // ── Startup zombie recovery ───────────────────────────────────────
        // Start the outbound Feishu WS long-connection if credentials already
        // exist (no-op otherwise). Idempotent. card.action.trigger arrives over
        // this outbound socket, so no public webhook/tunnel is needed.
        claw_fleet_core::feishu::ensure_ws_client();
        // Same topology for the mobile relay channel (outbound WS to
        // fleet-relay); no-op unless enabled with a secret in
        // ~/.fleet/mobile-relay.json.
        claw_fleet_core::mobile_relay::ensure_ws_client();
        step!("zombie recovery done");


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

        step!("indexer spawned");

        // Auto-resume scheduler dedup map — cloned into watcher/poll threads.
        let auto_resume_last_fire: Arc<Mutex<HashMap<String, Instant>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Count of auto-resume `claude --resume` processes currently alive.
        // Bounds how many we fire per tick (cap - in_flight) so a tick that
        // finds hundreds of eligible sessions can't spawn hundreds of ~150MB
        // processes at once — that was the 40GB startup runaway. Decremented by
        // each spawned process's reaper via the on_exit callback.
        let auto_resume_in_flight: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));

        // Consecutive resume-failure count per session. A session whose resume
        // keeps failing is backed off (skipped) so it stops being re-fired
        // forever — the field log showed 24k+ such doomed spawns. A success
        // resets the count. Cloned into watcher/poll/ticker threads.
        let auto_resume_failures: Arc<Mutex<HashMap<String, u32>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Cancellation flag for long-running threads. Flipped to false by `Drop`
        // so old threads exit when the backend is replaced.
        let running = Arc::new(AtomicBool::new(true));

        // Shared gate so the initial scan thread and the polling thread never
        // run concurrent scans (see ScanGate doc above).
        let scan_gate: Arc<ScanGate> = Arc::new(ScanGate::new());

        // Initial scan — run in a background thread so the UI appears immediately.
        {
            let app_bg = app.clone();
            let sess_bg = sessions.clone();
            let sources_bg = sources.clone();
            let idx_tx = index_tx.clone();
            let gate_bg = scan_gate.clone();
            std::thread::spawn(move || {
                let _slot = match gate_bg.try_enter() {
                    Some(g) => g,
                    None => {
                        log_debug("[BACKEND-INIT] initial scan skipped (another scan already in flight)");
                        return;
                    }
                };
                let started = Instant::now();
                let initial = crate::session::scan_all_sources(&sources_bg);
                let elapsed = started.elapsed();
                if elapsed > Duration::from_secs(30) {
                    log_debug(&format!(
                        "[BACKEND-INIT] initial scan slow: took {}s",
                        elapsed.as_secs()
                    ));
                }
                *sess_bg.lock().unwrap() = initial.clone();
                let _ = app_bg.emit("sessions-updated", &initial);
                let _ = app_bg.emit("scan-ready", true);
                crate::update_tray(&app_bg, &initial);
                publish_mobile_sessions(&initial);

                // Send to indexer thread (non-blocking).
                let _ = idx_tx.send(sessions_to_index_request(&initial));
            });
        }

        step!("scan thread spawned");

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

        step!("watch paths registered");

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

        step!("memory watch paths registered");

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
        let llm_config2 = llm_config.clone();
        let watch2 = watch.clone();
        let analyzing2 = analyzing.clone();
        let idx_tx2 = index_tx.clone();
        let ar2 = auto_resume_last_fire.clone();
        let arif2 = auto_resume_in_flight.clone();
        let arfail2 = auto_resume_failures.clone();

        // Pre-compute each filesystem source's watch dirs for fast path matching.
        let source_watch_dirs: Vec<(usize, Vec<std::path::PathBuf>)> = sources
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s.watch_strategy(), WatchStrategy::Filesystem))
            .map(|(i, s)| (i, s.watch_paths()))
            .collect();

        // Filesystem watcher thread — batches events so we rescan at most once
        // every 2 seconds, while still tailing the viewed session immediately.
        // Only rescans sources whose watch directories contain changed paths.
        std::thread::spawn(move || {
            let mut prev_statuses: HashMap<String, SessionStatus> = HashMap::new();
            let analyzing = analyzing2;
            let mut last_rescan = Instant::now();
            let mut last_memory_rescan = Instant::now();
            let rescan_interval = Duration::from_secs(2);
            let memory_rescan_interval = Duration::from_secs(1);
            // Track which source indices have pending changes (replaces boolean flag).
            let mut dirty_sources: HashSet<usize> = HashSet::new();
            let mut pending_memory_rescan = false;

            loop {
                // Wait for events; use a short timeout when a rescan is pending
                // so we flush it promptly after the coalescing window.
                let has_pending = !dirty_sources.is_empty();
                let timeout = if has_pending || pending_memory_rescan {
                    let remaining_session = if has_pending {
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

                            // Mark only the source(s) whose watch dirs contain this path.
                            for (idx, dirs) in &source_watch_dirs {
                                if dirs.iter().any(|d| path.starts_with(d)) {
                                    dirty_sources.insert(*idx);
                                }
                            }

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
                if !dirty_sources.is_empty() && last_rescan.elapsed() >= rescan_interval {
                    incremental_rescan_and_emit(
                        &sources2, &app2, &sess2, &so2, &dirty_sources,
                    );
                    detect_waiting_transitions(
                        &sess2,
                        &mut prev_statuses,
                        &analyzing,
                        &wa2,
                        &so2,
                        &app2,
                        &locale2,
                        &llm_config2,
                    );
                    maybe_fire_auto_resume(&sess2, &ar2, &arif2, &arfail2);
                    // Send to indexer thread (non-blocking).
                    let _ = idx_tx2.send(sessions_to_index_request(&sess2.lock().unwrap()));
                    last_rescan = Instant::now();
                    dirty_sources.clear();
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
            let llm_config3 = llm_config.clone();
            let analyzing3 = analyzing.clone();
            let idx_tx3 = index_tx.clone();
            let ar3 = auto_resume_last_fire.clone();
            let arif3 = auto_resume_in_flight.clone();
            let arfail3 = auto_resume_failures.clone();

            // Indices of polling sources — only these need rescanning on each tick.
            let poll_source_indices: HashSet<usize> = sources
                .iter()
                .enumerate()
                .filter(|(_, s)| matches!(s.watch_strategy(), WatchStrategy::Poll(_)))
                .map(|(i, _)| i)
                .collect();

            let running_poll = running.clone();
            let gate_poll = scan_gate.clone();
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

                // The initial-scan thread emits `scan-ready` once its first scan
                // finishes (see above). But it early-returns *without* emitting
                // when another scan already holds the gate, and this poll thread
                // never emitted readiness at all — so on that race the frontend
                // could stay stuck on "scanning…" forever. Emit `scan-ready` once
                // after this thread's first completed scan too. The event is
                // idempotent (frontend just sets `scanReady = true`), so the
                // common case where the initial scan already emitted is harmless.
                let mut emitted_scan_ready = false;

                loop {
                    std::thread::sleep(interval);
                    if !running_poll.load(Ordering::SeqCst) {
                        break;
                    }
                    // Skip this round if the initial scan (or a previous poll
                    // tick) is still running — on Windows with massive Cursor
                    // history a single scan can take tens of seconds and a
                    // second concurrent scan pushes the process into pagefile
                    // thrashing.
                    let _slot = match gate_poll.try_enter() {
                        Some(g) => g,
                        None => {
                            log_debug(
                                "[POLL] skip tick: previous scan still in flight",
                            );
                            continue;
                        }
                    };
                    let started = Instant::now();
                    incremental_rescan_and_emit(
                        &sources3, &app3, &sess3, &so3, &poll_source_indices,
                    );
                    if !emitted_scan_ready {
                        let _ = app3.emit("scan-ready", true);
                        emitted_scan_ready = true;
                    }
                    let elapsed = started.elapsed();
                    if elapsed > Duration::from_secs(30) {
                        log_debug(&format!(
                            "[POLL] scan slow: took {}s",
                            elapsed.as_secs()
                        ));
                    }
                    detect_waiting_transitions(
                        &sess3,
                        &mut prev_statuses,
                        &analyzing,
                        &wa3,
                        &so3,
                        &app3,
                        &locale3,
                        &llm_config3,
                    );
                    maybe_fire_auto_resume(&sess3, &ar3, &arif3, &arfail3);
                    // Send to indexer thread (non-blocking).
                    let _ = idx_tx3.send(sessions_to_index_request(&sess3.lock().unwrap()));
                }
            });
        }

        // Dedicated auto-resume ticker. The watch thread only fires when JSONL
        // files change, and rate-limited sessions produce no writes — without
        // this ticker, `resets_at` could pass with nobody checking until some
        // other session happens to wake the watcher.
        {
            let sess_ar = sessions.clone();
            let ar_ar = auto_resume_last_fire.clone();
            let arif_ar = auto_resume_in_flight.clone();
            let arfail_ar = auto_resume_failures.clone();
            let running_ar = running.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(30));
                if !running_ar.load(Ordering::SeqCst) {
                    break;
                }
                maybe_fire_auto_resume(&sess_ar, &ar_ar, &arif_ar, &arfail_ar);
            });
        }

        // Consumer heartbeat — tells `fleet guard`/`fleet elicitation` that a
        // head is alive and will consume requests.  Without this they fall
        // through (allow / native UI) instead of blocking Claude for 120s.
        //
        // Interval kept tight (500ms) so brief macOS process throttling — e.g.
        // during window resize drag — can't push the file's timestamp past the
        // hook's liveness_window (30s) and trigger a spurious panel-vanish.
        //
        // Self-check: if our own monotonic gap between iterations ever exceeds
        // STALL_WARN, the thread itself was starved (process suspended, GC
        // stall, scheduler pressure). That instantly distinguishes "we stopped
        // running" from "we ran but the file write was lost" without needing
        // hook-side correlation.
        {
            let running_hb = running.clone();
            std::thread::spawn(move || {
                const STALL_WARN: Duration = Duration::from_millis(2000);
                let mut last_tick: Option<Instant> = None;
                loop {
                    if !running_hb.load(Ordering::SeqCst) {
                        break;
                    }
                    let now = Instant::now();
                    if let Some(prev) = last_tick {
                        let gap = now.saturating_duration_since(prev);
                        if gap >= STALL_WARN {
                            claw_fleet_core::log_debug(&format!(
                                "[heartbeat] self-check: thread stalled for {:.2}s (expected ~0.5s; process likely suspended/throttled)",
                                gap.as_secs_f64()
                            ));
                        }
                    }
                    last_tick = Some(now);
                    claw_fleet_core::consumer_heartbeat::write_heartbeat();
                    std::thread::sleep(Duration::from_millis(500));
                }
            });
        }

        // Guard directory watcher — polls for new guard requests from `fleet guard`.
        {
            let app_guard = app.clone();
            let sess_guard = sessions.clone();
            let running_guard = running.clone();
            std::thread::spawn(move || {
                let mut known: HashSet<String> = HashSet::new();
                loop {
                    std::thread::sleep(Duration::from_millis(500));
                    if !running_guard.load(Ordering::SeqCst) {
                        break;
                    }
                    // Use the strict variant so a transient `read_dir` error
                    // doesn't get conflated with "all requests vanished" and
                    // dismiss every active panel (closing the user's decision
                    // panel for no reason). On Err we just skip this tick.
                    let pending = match crate::guard::list_pending_requests_checked() {
                        Ok(v) => v,
                        Err(e) => {
                            crate::log_debug(&format!(
                                "[guard watcher] read_dir failed (skipping dismissal step): {e}"
                            ));
                            continue;
                        }
                    };
                    for id in &pending {
                        if known.insert(id.clone()) {
                            // New request — read it and emit a Tauri event.
                            if let Some(mut req) = crate::guard::read_request(id) {
                                let (ws, ai) =
                                    resolve_session_display(&sess_guard, &req.session_id);
                                if req.workspace_name.is_empty() {
                                    req.workspace_name = ws;
                                }
                                if req.ai_title.is_none() {
                                    req.ai_title = ai;
                                }
                                crate::log_debug(&format!(
                                    "[guard] new request: {} cmd={}",
                                    id, req.command_summary
                                ));
                                let _ = app_guard.emit("guard-request", &req);
                                send_guard_card(&req);
                                publish_mobile_decision(
                                    "guard",
                                    &req,
                                    format!("{} · 命令待审批", notify_workspace(&req.workspace_name)),
                                    notify_preview(&req.command_summary),
                                );
                            }
                        }
                    }
                    // Emit a dismiss event for any known id that no longer has
                    // a pending request file (answered by another client, or
                    // timed out / cleaned up by `fleet guard`).
                    for id in known.iter().filter(|id| !pending.contains(*id)) {
                        let _ = app_guard.emit("guard-dismissed", id.clone());
                        claw_fleet_core::mobile_relay::publish_decision_resolved("guard", id);
                    }
                    known.retain(|id| pending.contains(id));
                }
            });
        }

        // Elicitation directory watcher — polls for new elicitation requests from `fleet elicitation`.
        {
            let app_elicit = app.clone();
            let sess_elicit = sessions.clone();
            let running_elicit = running.clone();
            std::thread::spawn(move || {
                let mut known: HashSet<String> = HashSet::new();
                loop {
                    std::thread::sleep(Duration::from_millis(500));
                    if !running_elicit.load(Ordering::SeqCst) {
                        break;
                    }
                    let pending = match crate::elicitation::list_pending_requests_checked() {
                        Ok(v) => v,
                        Err(e) => {
                            crate::log_debug(&format!(
                                "[elicitation watcher] read_dir failed (skipping dismissal step): {e}"
                            ));
                            continue;
                        }
                    };
                    for id in &pending {
                        if known.insert(id.clone()) {
                            if let Some(mut req) = crate::elicitation::read_request(id) {
                                let (ws, ai) =
                                    resolve_session_display(&sess_elicit, &req.session_id);
                                if req.workspace_name.is_empty() {
                                    req.workspace_name = ws;
                                }
                                if req.ai_title.is_none() {
                                    req.ai_title = ai;
                                }
                                crate::log_debug(&format!(
                                    "[elicitation] new request: {} questions={}",
                                    id,
                                    req.questions.len()
                                ));
                                let _ = app_elicit.emit("elicitation-request", &req);
                                send_elicitation_card(&req);
                                publish_mobile_decision(
                                    "elicitation",
                                    &req,
                                    format!("{} · 有问题请示", notify_workspace(&req.workspace_name)),
                                    notify_preview(
                                        req.questions.first().map(|q| q.question.as_str()).unwrap_or(""),
                                    ),
                                );
                            }
                        }
                    }
                    for id in known.iter().filter(|id| !pending.contains(*id)) {
                        let _ = app_elicit.emit("elicitation-dismissed", id.clone());
                        claw_fleet_core::mobile_relay::publish_decision_resolved("elicitation", id);
                    }
                    known.retain(|id| pending.contains(id));
                }
            });
        }

        // fleet__ask directory watcher — polls for new fleet_ask requests from
        // the `fleet mcp` server. Mirror of the elicitation watcher: same
        // 500ms cadence, same orphan-filter via `list_pending_requests_checked`,
        // same `(workspace, ai_title)` resolution from the SessionInfo cache.
        // Emits `fleet-ask-request` / `fleet-ask-dismissed` Tauri events that
        // P3 wires up in the frontend.
        {
            let app_ask = app.clone();
            let sess_ask = sessions.clone();
            let running_ask = running.clone();
            std::thread::spawn(move || {
                let mut known: HashSet<String> = HashSet::new();
                loop {
                    std::thread::sleep(Duration::from_millis(500));
                    if !running_ask.load(Ordering::SeqCst) {
                        break;
                    }
                    let pending = match claw_fleet_core::mcp_ipc::list_pending_requests_checked() {
                        Ok(v) => v,
                        Err(e) => {
                            crate::log_debug(&format!(
                                "[fleet-ask watcher] read_dir failed (skipping dismissal step): {e}"
                            ));
                            continue;
                        }
                    };
                    for id in &pending {
                        if known.insert(id.clone()) {
                            if let Some(mut req) = claw_fleet_core::mcp_ipc::read_request(id) {
                                let (ws, ai) =
                                    resolve_session_display(&sess_ask, &req.session_id);
                                if req.workspace_name.is_empty() {
                                    req.workspace_name = ws;
                                }
                                if req.ai_title.is_none() {
                                    req.ai_title = ai;
                                }
                                crate::log_debug(&format!(
                                    "[fleet-ask] new request: {} questions={}",
                                    id,
                                    req.questions.len()
                                ));
                                let _ = app_ask.emit("fleet-ask-request", &req);
                                publish_mobile_decision(
                                    "fleet-ask",
                                    &req,
                                    format!("{} · 决策卡待处理", notify_workspace(&req.workspace_name)),
                                    notify_preview(
                                        req.ai_title.as_deref().unwrap_or_else(|| {
                                            req.questions
                                                .first()
                                                .map(|q| q.question.as_str())
                                                .unwrap_or("")
                                        }),
                                    ),
                                );
                            }
                        }
                    }
                    for id in known.iter().filter(|id| !pending.contains(*id)) {
                        let _ = app_ask.emit("fleet-ask-dismissed", id.clone());
                        claw_fleet_core::mobile_relay::publish_decision_resolved("fleet-ask", id);
                    }
                    known.retain(|id| pending.contains(id));
                }
            });
        }

        // fleet__render_a2ui watcher — parallel channel to fleet__ask. Polls
        // `~/.fleet/fleet-render-a2ui/` for new MCP-side A2UI render requests
        // and emits `a2ui-render-request` / `a2ui-render-dismissed` Tauri
        // events that the frontend's DecisionPanel consumes.
        {
            let app_a2ui = app.clone();
            let sess_a2ui = sessions.clone();
            let running_a2ui = running.clone();
            std::thread::spawn(move || {
                let mut known: HashSet<String> = HashSet::new();
                loop {
                    std::thread::sleep(Duration::from_millis(500));
                    if !running_a2ui.load(Ordering::SeqCst) {
                        break;
                    }
                    let pending = match claw_fleet_core::mcp_a2ui_ipc::list_pending_requests_checked() {
                        Ok(v) => v,
                        Err(e) => {
                            crate::log_debug(&format!(
                                "[a2ui-render watcher] read_dir failed (skipping dismissal step): {e}"
                            ));
                            continue;
                        }
                    };
                    for id in &pending {
                        if known.insert(id.clone()) {
                            if let Some(mut req) = claw_fleet_core::mcp_a2ui_ipc::read_request(id) {
                                let (ws, ai) =
                                    resolve_session_display(&sess_a2ui, &req.session_id);
                                if req.workspace_name.is_empty() {
                                    req.workspace_name = ws;
                                }
                                if req.ai_title.is_none() {
                                    req.ai_title = ai;
                                }
                                crate::log_debug(&format!(
                                    "[a2ui-render] new request: {}",
                                    id
                                ));
                                let _ = app_a2ui.emit("a2ui-render-request", &req);
                                publish_mobile_decision(
                                    "a2ui-render",
                                    &req,
                                    format!("{} · Agent 界面", notify_workspace(&req.workspace_name)),
                                    notify_preview(req.ai_title.as_deref().unwrap_or("A2UI 自定义界面")),
                                );
                            }
                        }
                    }
                    for id in known.iter().filter(|id| !pending.contains(*id)) {
                        let _ = app_a2ui.emit("a2ui-render-dismissed", id.clone());
                        claw_fleet_core::mobile_relay::publish_decision_resolved("a2ui-render", id);
                    }
                    known.retain(|id| pending.contains(id));
                }
            });
        }

        // fleet__permission_prompt watcher — parallel channel to fleet__ask.
        // Polls `~/.fleet/permission-prompt/` for native permission prompts
        // routed from headless sessions via `--permission-prompt-tool` and
        // emits `permission-prompt-request` / `permission-prompt-dismissed`
        // Tauri events for the frontend's DecisionPanel.
        {
            let app_pp = app.clone();
            let sess_pp = sessions.clone();
            let running_pp = running.clone();
            std::thread::spawn(move || {
                let mut known: HashSet<String> = HashSet::new();
                loop {
                    std::thread::sleep(Duration::from_millis(500));
                    if !running_pp.load(Ordering::SeqCst) {
                        break;
                    }
                    let pending = match claw_fleet_core::permission_prompt_ipc::list_pending_requests_checked() {
                        Ok(v) => v,
                        Err(e) => {
                            crate::log_debug(&format!(
                                "[permission-prompt watcher] read_dir failed (skipping dismissal step): {e}"
                            ));
                            continue;
                        }
                    };
                    for id in &pending {
                        if known.insert(id.clone()) {
                            if let Some(mut req) =
                                claw_fleet_core::permission_prompt_ipc::read_request(id)
                            {
                                let (ws, ai) =
                                    resolve_session_display(&sess_pp, &req.session_id);
                                if req.workspace_name.is_empty() {
                                    req.workspace_name = ws;
                                }
                                if req.ai_title.is_none() {
                                    req.ai_title = ai;
                                }
                                crate::log_debug(&format!(
                                    "[permission-prompt] new request: {} tool={}",
                                    id, req.tool_name
                                ));
                                let _ = app_pp.emit("permission-prompt-request", &req);
                                publish_mobile_decision(
                                    "permission-prompt",
                                    &req,
                                    format!("{} · 权限请求", notify_workspace(&req.workspace_name)),
                                    notify_preview(&req.tool_name),
                                );
                            }
                        }
                    }
                    for id in known.iter().filter(|id| !pending.contains(*id)) {
                        let _ = app_pp.emit("permission-prompt-dismissed", id.clone());
                        claw_fleet_core::mobile_relay::publish_decision_resolved("permission-prompt", id);
                    }
                    known.retain(|id| pending.contains(id));
                }
            });
        }

        // Plan-approval directory watcher — polls for new ExitPlanMode requests from `fleet plan-approval`.
        {
            let app_plan = app.clone();
            let sess_plan = sessions.clone();
            let running_plan = running.clone();
            std::thread::spawn(move || {
                let mut known: HashSet<String> = HashSet::new();
                loop {
                    std::thread::sleep(Duration::from_millis(500));
                    if !running_plan.load(Ordering::SeqCst) {
                        break;
                    }
                    let pending = match crate::plan_approval::list_pending_requests_checked() {
                        Ok(v) => v,
                        Err(e) => {
                            crate::log_debug(&format!(
                                "[plan-approval watcher] read_dir failed (skipping dismissal step): {e}"
                            ));
                            continue;
                        }
                    };
                    for id in &pending {
                        if known.insert(id.clone()) {
                            if let Some(mut req) = crate::plan_approval::read_request(id) {
                                let (ws, ai) =
                                    resolve_session_display(&sess_plan, &req.session_id);
                                if req.workspace_name.is_empty() {
                                    req.workspace_name = ws;
                                }
                                if req.ai_title.is_none() {
                                    req.ai_title = ai;
                                }
                                crate::log_debug(&format!(
                                    "[plan-approval] new request: {} plan_len={}",
                                    id,
                                    req.plan_content.len()
                                ));
                                let _ = app_plan.emit("plan-approval-request", &req);
                                send_plan_card(&req);
                                publish_mobile_decision(
                                    "plan-approval",
                                    &req,
                                    format!("{} · 计划待审批", notify_workspace(&req.workspace_name)),
                                    notify_preview(req.ai_title.as_deref().unwrap_or("ExitPlanMode 计划审批")),
                                );
                            }
                        }
                    }
                    for id in known.iter().filter(|id| !pending.contains(*id)) {
                        let _ = app_plan.emit("plan-approval-dismissed", id.clone());
                        claw_fleet_core::mobile_relay::publish_decision_resolved("plan-approval", id);
                    }
                    known.retain(|id| pending.contains(id));
                }
            });
        }

        // Start the daily report scheduler (backfills missing reports in background).
        crate::daily_report::start_report_scheduler(
            report_store.clone(),
            locale.clone(),
            llm_config.clone(),
            running.clone(),
        );

        step!("threads spawned, constructing result");

        let result = LocalBackend {
            app,
            sources,
            sessions,
            watch,
            waiting_alerts,
            session_outcomes,
            audit_cache,
            audit_history,
            search_index,
            index_tx,
            report_store,
            locale,
            llm_config,
            _watcher: watcher,
            running,
        };
        step!("LocalBackend::new() complete");
        result
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Convert a session list into an index request (list of (path, id) pairs).
fn sessions_to_index_request(sessions: &[SessionInfo]) -> IndexRequest {
    sessions.iter().map(|s| (s.jsonl_path.clone(), s.id.clone())).collect()
}

// ── Feishu card hooks ─────────────────────────────────────────────────────────
//
// These wrappers render Card 2.0 JSON for an incoming decision request and
// hand it to `claw_fleet_core::feishu`.  When Feishu isn't connected the
// `bound_open_id` check inside `notify_decision_created` short-circuits and
// these become free no-ops.  Failures are logged and swallowed — a flaky
// Feishu session must never block the local desktop watcher.
//
// Each `send_*_card` runs the actual HTTP POST on a detached thread so a
// slow Feishu round-trip (up to FEISHU_HTTP_TIMEOUT) cannot stall the
// pending-request poller.  Race window: if the user responds on desktop
// before the send-thread completes, `resolve_feishu_card` finds no
// mapping and the card stays in its un-resolved state.  Acceptable for
// now — proper resolution on out-of-order events is tracked for the
// webhook slice.

/// Mirror a sessions snapshot onto the mobile relay channel. The relay side
/// additionally throttles (≥2s between pushes) and skips when no mobile
/// client is online; the `is_connected` gate here just avoids serializing
/// the whole list when the channel is down.
fn publish_mobile_sessions(sessions: &[claw_fleet_core::session::SessionInfo]) {
    if !claw_fleet_core::mobile_relay::is_connected() {
        return;
    }
    if let Ok(v) = serde_json::to_value(sessions) {
        claw_fleet_core::mobile_relay::publish_sessions(&v);
    }
}

/// Mirror a new pending decision onto the mobile relay channel, alongside the
/// Tauri emit and the Feishu card. Serialization is skipped entirely while
/// the relay is disconnected.
fn publish_mobile_decision<T: serde::Serialize>(kind: &str, req: &T, title: String, body: String) {
    if !claw_fleet_core::mobile_relay::is_connected() {
        return;
    }
    if let Ok(v) = serde_json::to_value(req) {
        claw_fleet_core::mobile_relay::publish_decision_created(kind, v, &title, &body);
    }
}

use claw_fleet_core::mobile_relay::{notify_preview, notify_workspace};

fn send_guard_card(req: &claw_fleet_core::guard::GuardRequest) {
    let req = req.clone();
    std::thread::spawn(move || {
        let card = claw_fleet_core::feishu::GuardCard {
            workspace: req.workspace_name.clone(),
            command: req.command.clone(),
            risk_label: if req.risk_tags.is_empty() {
                None
            } else {
                Some(req.risk_tags.join(", "))
            },
            llm_analysis: None,
            decision_id: req.id.clone(),
        };
        if let Err(e) = claw_fleet_core::feishu::notify_decision_created(
            &req.id,
            claw_fleet_core::feishu::DecisionKind::Guard,
            &card.to_card_json(),
        ) {
            crate::log_debug(&format!("[feishu] send guard card failed: {e}"));
        }
    });
}

fn send_elicitation_card(req: &claw_fleet_core::elicitation::ElicitationRequest) {
    // Mirror every question onto a single Feishu card as one form; the
    // user answers all of them and submits once (single/multi-select both
    // supported). The webhook reassembles the answers map.
    let req = req.clone();
    std::thread::spawn(move || {
        if req.questions.is_empty() {
            return;
        }
        let questions = req
            .questions
            .iter()
            .map(|q| claw_fleet_core::feishu::ElicitationQuestionCard {
                question: q.question.clone(),
                options: q
                    .options
                    .iter()
                    .map(|o| claw_fleet_core::feishu::ElicitationOptionCard {
                        label: o.label.clone(),
                        description: if o.description.is_empty() {
                            None
                        } else {
                            Some(o.description.clone())
                        },
                        value: o.label.clone(),
                    })
                    .collect(),
                multi_select: q.multi_select,
            })
            .collect();
        let card = claw_fleet_core::feishu::ElicitationCard {
            workspace: req.workspace_name.clone(),
            questions,
            decision_id: req.id.clone(),
        };
        if let Err(e) = claw_fleet_core::feishu::notify_decision_created(
            &req.id,
            claw_fleet_core::feishu::DecisionKind::Elicitation,
            &card.to_card_json(),
        ) {
            crate::log_debug(&format!("[feishu] send elicitation card failed: {e}"));
        }
    });
}

fn send_plan_card(req: &claw_fleet_core::plan_approval::PlanApprovalRequest) {
    let req = req.clone();
    std::thread::spawn(move || {
        let card = claw_fleet_core::feishu::PlanCard {
            workspace: req.workspace_name.clone(),
            plan_markdown: req.plan_content.clone(),
            decision_id: req.id.clone(),
        };
        if let Err(e) = claw_fleet_core::feishu::notify_decision_created(
            &req.id,
            claw_fleet_core::feishu::DecisionKind::PlanApproval,
            &card.to_card_json(),
        ) {
            crate::log_debug(&format!("[feishu] send plan card failed: {e}"));
        }
    });
}

fn resolve_feishu_card(decision_id: &str, card: serde_json::Value) {
    let decision_id = decision_id.to_string();
    std::thread::spawn(move || {
        if let Err(e) = claw_fleet_core::feishu::notify_decision_resolved(&decision_id, card) {
            crate::log_debug(&format!("[feishu] resolve card failed: {e}"));
        }
    });
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

/// Resolve a display name (AI title preferred, falling back to workspace name)
/// for a given session id. Returns an empty string if the session is unknown.
/// Look up a session by id and return `(workspace_name, ai_title)` so decision
/// cards can display the real workspace name alongside the AI-generated title
/// without conflating them.
fn resolve_session_display(
    sessions: &Arc<Mutex<Vec<SessionInfo>>>,
    session_id: &str,
) -> (String, Option<String>) {
    let list = sessions.lock().unwrap();
    list.iter()
        .find(|s| s.id == session_id)
        .map(|s| (s.workspace_name.clone(), s.ai_title.clone()))
        .unwrap_or_default()
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
    publish_mobile_sessions(&s);
}

/// Build the session list an incremental rescan should produce: keep the
/// sessions of clean sources (aged out), re-scan only the dirty ones, then
/// re-apply the scan-time enrichers over the merged list.
///
/// Split out of `incremental_rescan_and_emit` so it can be tested without an
/// `AppHandle` — the enrichers are exactly what this path used to get wrong.
fn build_incremental_sessions(
    sources: &[Box<dyn AgentSource>],
    existing: &[SessionInfo],
    dirty: &HashSet<usize>,
    now_ms: u64,
) -> Vec<SessionInfo> {
    // Collect the source names of dirty sources so we can partition existing sessions.
    // Must use `name()` (e.g. "claude-code") not `api_name()` (e.g. "claude")
    // because `SessionInfo::agent_source` stores the full source name.
    let dirty_names: HashSet<&str> = dirty
        .iter()
        .filter_map(|&i| sources.get(i).map(|s| s.name()))
        .collect();

    // Keep sessions from clean sources, rescan only dirty ones.
    // Re-apply age_out_status to retained sessions so their status still
    // transitions to Idle when the underlying file hasn't been touched.
    let mut s: Vec<SessionInfo> = existing
        .iter()
        .filter(|sess| !dirty_names.contains(sess.agent_source.as_str()))
        .cloned()
        .collect();
    for sess in &mut s {
        let age_secs = now_ms.saturating_sub(sess.last_activity_ms) as f64 / 1000.0;
        crate::session::age_out_status(sess, age_secs);
    }

    for &idx in dirty {
        if let Some(source) = sources.get(idx) {
            if source.is_available() {
                s.extend(source.scan_sessions());
            }
        }
    }

    // Re-stamp the out-of-jsonl state for retained AND freshly-scanned sessions.
    // Freshly-scanned ones arrive with `user_mark` / `last_read_ms` / `handoff`
    // unset, and a handoff link can appear while a predecessor's source stays
    // clean — so this runs over the whole merged list, not just the new rows.
    crate::session::enrich_all(&mut s);
    crate::session::sort_sessions(&mut s);
    s
}

/// Incremental rescan: only rescan sources whose indices appear in `dirty`.
/// Sessions from clean sources are kept as-is, avoiding expensive readdir/stat
/// calls for directories that haven't changed.
fn incremental_rescan_and_emit(
    sources: &[Box<dyn AgentSource>],
    app: &AppHandle,
    sessions: &Arc<Mutex<Vec<SessionInfo>>>,
    outcomes: &Arc<Mutex<HashMap<String, Vec<String>>>>,
    dirty: &HashSet<usize>,
) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let mut s = {
        let existing = sessions.lock().unwrap();
        build_incremental_sessions(sources, &existing, dirty, now_ms)
    };

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
    publish_mobile_sessions(&s);
}

/// Returns true when `path` is a `.md` file residing inside a `memory/`
/// directory — the marker we use to identify auto-memory files for any source.
fn path_is_in_memory_dir(path: &std::path::Path) -> bool {
    path.components().any(|c| c.as_os_str() == "memory")
}

fn emit_tail_lines(path: &std::path::Path, app: &AppHandle, watch: &crate::WatchState) {
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
        claw_fleet_core::process_util::command("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .status()
            .map_err(|e| format!("taskkill failed: {e}"))?;
        Ok(())
    }
}

/// Headlessly resume a rate-limited session by spawning
/// `claude --resume <session_id> -p "continue"` detached in the given workspace.
///
/// Returns as soon as the child is spawned; the process's stdout/stderr are
/// discarded (the session's own JSONL will capture the new turn for the
/// scanner to pick up).
pub fn resume_session_impl(
    session_id: &str,
    workspace_path: &str,
    prompt: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
    permission_mode: Option<&str>,
) -> Result<(), String> {
    claw_fleet_core::auto_resume::spawn_resume_prompt(
        session_id,
        workspace_path,
        prompt.unwrap_or("continue"),
        model,
        effort,
        permission_mode,
    )
}

/// Max number of `claude --resume` auto-resume processes alive at once. Each
/// is a full Claude Code process (~150-200MB), so an unbounded fan-out of a
/// few hundred is tens of GB of RSS — the startup runaway this caps.
const AUTO_RESUME_MAX_CONCURRENT: usize = 4;

/// After this many consecutive failed resumes, a session is backed off and no
/// longer re-fired — stops the endless re-fire loop (24k+ doomed spawns seen
/// in the field) for any session whose resume can never succeed.
const AUTO_RESUME_FAILURE_BACKOFF: u32 = 3;

/// Scan the current session list for auto-resume candidates and fire them,
/// bounded by a global concurrency cap.
///
/// Two layers of protection against spamming:
/// - **Debounce**: a given session can't be auto-resumed twice within 120s, so
///   if the spawned `claude --resume` hasn't appended a new turn to the JSONL
///   yet on the next rescan tick, we won't fire it again.
/// - **Concurrency cap**: at most `AUTO_RESUME_MAX_CONCURRENT` resume processes
///   run at once. A tick that finds 300 eligible sessions fires only enough to
///   fill the free slots; `in_flight` is decremented by each process's reaper.
fn maybe_fire_auto_resume(
    sessions: &Arc<Mutex<Vec<SessionInfo>>>,
    last_fire: &Arc<Mutex<HashMap<String, Instant>>>,
    in_flight: &Arc<AtomicUsize>,
    failures: &Arc<Mutex<HashMap<String, u32>>>,
) {
    let config = claw_fleet_core::auto_resume::AutoResumeConfig::load();
    if !config.enabled {
        return;
    }
    let now = chrono::Utc::now();
    let debounce = Duration::from_secs(120);

    // Only fire enough to fill the free concurrency slots this tick.
    let slots = AUTO_RESUME_MAX_CONCURRENT.saturating_sub(in_flight.load(Ordering::SeqCst));
    if slots == 0 {
        return;
    }

    // Read the latest usage snapshot off disk once for this tick (no network
    // call) so `should_auto_resume` can fire early when the account's limit has
    // already recovered — a window reset or a foxy-switcher account swap — ahead
    // of the hinted `resets_at`. `None` (no snapshot yet) simply means we fall
    // back to the hint-time gate.
    let usage = claw_fleet_core::account::latest_usage_snapshot();
    let candidates: Vec<(String, String)> = {
        let sess = sessions.lock().unwrap();
        let mut fire_map = last_fire.lock().unwrap();
        let fail_map = failures.lock().unwrap();
        // Drop entries older than the debounce window so the map doesn't grow
        // unboundedly for sessions that come and go.
        fire_map.retain(|_, t| t.elapsed() < debounce * 10);
        let picked = claw_fleet_core::auto_resume::select_resume_candidates(
            &sess,
            &config,
            now,
            usage.as_ref(),
            // Skip a session that's still debounced OR backed off after
            // repeated failures.
            |id| {
                fire_map.get(id).is_some_and(|t| t.elapsed() < debounce)
                    || claw_fleet_core::auto_resume::is_backed_off(
                        &fail_map,
                        id,
                        AUTO_RESUME_FAILURE_BACKOFF,
                    )
            },
            slots,
        );
        for (id, _) in &picked {
            fire_map.insert(id.clone(), Instant::now());
        }
        picked
    };

    for (session_id, workspace_path) in candidates {
        log_debug(&format!(
            "auto_resume: firing for session {} in {} (in_flight={})",
            session_id,
            workspace_path,
            in_flight.load(Ordering::SeqCst)
        ));
        // Reserve a slot now; the reaper releases it when the process exits.
        in_flight.fetch_add(1, Ordering::SeqCst);
        let in_flight_done = in_flight.clone();
        let failures_done = failures.clone();
        let id_done = session_id.clone();
        let spawn_result = claw_fleet_core::auto_resume::spawn_resume_tracked(
            &session_id,
            &workspace_path,
            move |success| {
                in_flight_done.fetch_sub(1, Ordering::SeqCst);
                if let Ok(mut fail_map) = failures_done.lock() {
                    claw_fleet_core::auto_resume::record_resume_outcome(
                        &mut fail_map,
                        &id_done,
                        success,
                    );
                }
            },
        );
        if let Err(e) = spawn_result {
            // Spawn failed before any process exists → release the slot here
            // and record the failure, since no reaper will fire on_exit.
            in_flight.fetch_sub(1, Ordering::SeqCst);
            if let Ok(mut fail_map) = failures.lock() {
                claw_fleet_core::auto_resume::record_resume_outcome(
                    &mut fail_map,
                    &session_id,
                    false,
                );
            }
            log_debug(&format!("auto_resume: failed for {}: {}", session_id, e));
        }
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
        claw_fleet_core::process_util::command("taskkill")
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

    fn get_messages_tail(&self, path: &str, n: usize) -> Result<Vec<Value>, String> {
        match find_source_for_path(&self.sources, path) {
            Some(source) => source.get_messages_tail(path, n),
            None => Err(format!("No agent source can handle path: {path}")),
        }
    }

    fn interrupt_pid(&self, pid: u32) -> Result<(), String> {
        claw_fleet_core::session::interrupt_pid_impl(pid)?;
        // The CLI needs a moment to write its interrupt marker and exit, so the
        // rescan lags further behind than the kill path's.
        let app = self.app.clone();
        let sessions = self.sessions.clone();
        let sources = self.sources.clone();
        let outcomes = self.session_outcomes.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            rescan_and_emit(&sources, &app, &sessions, &outcomes);
        });
        Ok(())
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

    fn resume_session(
        &self,
        session_id: String,
        workspace_path: String,
        prompt: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        permission_mode: Option<String>,
    ) -> Result<(), String> {
        resume_session_impl(
            &session_id,
            &workspace_path,
            prompt.as_deref(),
            model.as_deref(),
            effort.as_deref(),
            permission_mode.as_deref(),
        )?;
        // Trigger a rescan after a delay so the UI picks up the new turn
        // (which will also clear the RateLimited badge via detect_rate_limit).
        let app = self.app.clone();
        let sessions = self.sessions.clone();
        let sources = self.sources.clone();
        let outcomes = self.session_outcomes.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            rescan_and_emit(&sources, &app, &sessions, &outcomes);
        });
        Ok(())
    }

    fn spawn_new_session(
        &self,
        workspace_path: String,
        prompt: String,
        model: Option<String>,
        effort: Option<String>,
        permission_mode: Option<String>,
    ) -> Result<claw_fleet_core::session_launch::SpawnSessionResponse, String> {
        let resp = claw_fleet_core::session_launch::spawn_new_session(
            &workspace_path,
            &prompt,
            model.as_deref(),
            effort.as_deref(),
            permission_mode.as_deref(),
        )?;
        // Trigger a rescan after a delay so the freshly created JSONL shows up
        // in the session list without waiting for the next scheduled scan.
        let app = self.app.clone();
        let sessions = self.sessions.clone();
        let sources = self.sources.clone();
        let outcomes = self.session_outcomes.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            rescan_and_emit(&sources, &app, &sessions, &outcomes);
        });
        Ok(resp)
    }

    fn get_auto_resume_config(&self) -> claw_fleet_core::auto_resume::AutoResumeConfig {
        claw_fleet_core::auto_resume::AutoResumeConfig::load()
    }

    fn set_auto_resume_config(
        &self,
        config: claw_fleet_core::auto_resume::AutoResumeConfig,
    ) -> Result<(), String> {
        config.save()
    }

    fn set_session_mark(
        &self,
        session_id: String,
        workspace_path: String,
        mark: Option<claw_fleet_core::session_mark::SessionMark>,
    ) -> Result<(), String> {
        claw_fleet_core::session_mark::set_mark(&session_id, &workspace_path, mark)?;
        self.restamp_marks_and_emit();
        Ok(())
    }

    fn mark_sessions_read(
        &self,
        items: Vec<claw_fleet_core::session_read::SessionReadItem>,
    ) -> Result<(), String> {
        claw_fleet_core::session_read::mark_read(&items)?;
        self.restamp_marks_and_emit();
        Ok(())
    }

    fn list_procs(&self) -> Vec<claw_fleet_core::proc_runner::ProcRecord> {
        claw_fleet_core::proc_runner::list_procs()
    }

    fn spawn_proc(
        &self,
        workspace_path: String,
        command: String,
        cols: u16,
        rows: u16,
    ) -> Result<claw_fleet_core::proc_runner::ProcRecord, String> {
        // The desktop binary itself is the host re-exec target — its main()
        // intercepts the fleet-proc-host marker before Tauri boots.
        let exe = std::env::current_exe().map_err(|e| format!("cannot locate own binary: {e}"))?;
        claw_fleet_core::proc_runner::spawn_proc(&exe, &workspace_path, &command, cols, rows)
    }

    fn kill_proc(&self, id: String, force: bool) -> Result<(), String> {
        claw_fleet_core::proc_runner::kill_proc(&id, force)
    }

    fn proc_output(
        &self,
        id: String,
        offset: Option<u64>,
    ) -> Result<claw_fleet_core::proc_runner::ProcOutputChunk, String> {
        claw_fleet_core::proc_runner::proc_output(&id, offset)
    }

    fn proc_input(&self, id: String, data_b64: String) -> Result<(), String> {
        claw_fleet_core::proc_runner::proc_input(&id, &data_b64)
    }

    fn proc_resize(&self, id: String, cols: u16, rows: u16) -> Result<(), String> {
        claw_fleet_core::proc_runner::proc_resize(&id, cols, rows)
    }

    fn clear_procs(&self, id: Option<String>, workspace_path: Option<String>) -> Result<u32, String> {
        match id {
            Some(id) => claw_fleet_core::proc_runner::clear_proc(&id).map(|()| 1),
            None => claw_fleet_core::proc_runner::clear_finished_procs(workspace_path.as_deref()),
        }
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

    fn today_usage(&self) -> crate::today_usage::TodayUsage {
        let sessions = self.sessions.lock().unwrap().clone();
        crate::today_usage::today_usage(&sessions)
    }

    fn check_setup(&self) -> crate::backend::SetupStatus {
        let (cli_installed, cli_path) = crate::check_cli_installed();
        let claude_dir_exists = crate::session::real_home_dir()
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

    fn read_live_thinking(
        &self,
        session_id: &str,
    ) -> Option<claw_fleet_core::live_thinking::LiveThinking> {
        claw_fleet_core::live_thinking::read_live_thinking(session_id)
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

    fn list_wiki_docs(&self) -> Vec<crate::wiki::WikiDoc> {
        crate::wiki::list_docs()
    }

    fn get_handoff_chain(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::handoff::HandoffChain>, String> {
        Ok(crate::handoff::chain_containing(session_id))
    }

    fn get_wiki_doc(&self, slug: &str) -> Result<crate::wiki::WikiDoc, String> {
        crate::wiki::get_doc(slug)
    }

    fn get_wiki_file(
        &self,
        slug: &str,
        version: &str,
        relpath: &str,
    ) -> Result<crate::wiki::WikiFileBytes, String> {
        crate::wiki::get_file(slug, version, relpath)
    }

    fn get_decision_asset(
        &self,
        id: &str,
        qidx: &str,
        relpath: &str,
    ) -> Result<crate::mcp_ipc::DecisionAssetBytes, String> {
        crate::mcp_ipc::read_decision_asset(id, qidx, relpath)
    }

    fn delete_wiki_doc(&self, slug: &str) -> Result<(), String> {
        crate::wiki::delete_doc(slug)
    }

    fn delete_wiki_version(&self, slug: &str, version: &str) -> Result<(), String> {
        crate::wiki::delete_version(slug, version)
    }

    fn move_wiki_doc(&self, from: &str, to: &str) -> Result<crate::wiki::WikiDoc, String> {
        crate::wiki::move_doc(from, to)
    }

    fn move_wiki_folder(&self, from: &str, to: &str) -> Result<Vec<crate::wiki::WikiDoc>, String> {
        crate::wiki::move_folder(from, to)
    }

    fn delete_wiki_folder(&self, prefix: &str) -> Result<usize, String> {
        crate::wiki::delete_folder(prefix)
    }

    fn search_wiki_docs(&self, query: &str) -> Vec<crate::wiki::WikiSearchHit> {
        crate::wiki::search_docs(query)
    }

    fn export_wiki_doc(&self, slug: &str, version: &str) -> Result<crate::wiki::WikiExport, String> {
        crate::wiki::export_doc(slug, version)
    }

    fn get_task_plans(
        &self,
        workspace_path: &str,
        session_id: Option<&str>,
    ) -> Vec<crate::prd_tasks::TaskPlanDetail> {
        crate::prd_tasks::list_workspace_task_plans(std::path::Path::new(workspace_path), session_id)
    }

    fn list_explorer_roots(
        &self,
        workspace: &str,
    ) -> Result<Vec<crate::file_explorer::ExplorerRoot>, String> {
        crate::file_explorer::list_roots(workspace, &self.known_workspaces())
    }

    fn list_explorer_dir(
        &self,
        workspace: &str,
        root: &str,
        rel_path: &str,
        show_ignored: bool,
    ) -> Result<Vec<crate::file_explorer::ExplorerEntry>, String> {
        crate::file_explorer::list_dir(
            workspace,
            root,
            rel_path,
            show_ignored,
            &self.known_workspaces(),
        )
    }

    fn read_explorer_file(
        &self,
        workspace: &str,
        root: &str,
        rel_path: &str,
    ) -> Result<crate::file_explorer::ExplorerFileContent, String> {
        crate::file_explorer::read_file(workspace, root, rel_path, &self.known_workspaces())
    }

    fn list_scratchpad_dir(
        &self,
        workspace: &str,
        session_id: &str,
        rel_path: &str,
    ) -> Result<Vec<crate::file_explorer::ExplorerEntry>, String> {
        crate::file_explorer::list_scratchpad_dir(
            workspace,
            session_id,
            rel_path,
            &self.known_workspaces(),
        )
    }

    fn read_scratchpad_file(
        &self,
        workspace: &str,
        session_id: &str,
        rel_path: &str,
    ) -> Result<crate::file_explorer::ExplorerFileContent, String> {
        crate::file_explorer::read_scratchpad_file(
            workspace,
            session_id,
            rel_path,
            &self.known_workspaces(),
        )
    }

    fn git_status(
        &self,
        workspace: &str,
        root: &str,
    ) -> Result<crate::git_ops::GitStatus, String> {
        crate::git_ops::git_status(workspace, root, &self.known_workspaces())
    }

    fn git_push(
        &self,
        workspace: &str,
        root: &str,
    ) -> Result<crate::git_ops::GitOpResult, String> {
        crate::git_ops::git_push(workspace, root, &self.known_workspaces())
    }

    fn git_pull(
        &self,
        workspace: &str,
        root: &str,
    ) -> Result<crate::git_ops::GitOpResult, String> {
        crate::git_ops::git_pull(workspace, root, &self.known_workspaces())
    }

    fn list_skills(&self) -> Vec<crate::skills::SkillItem> {
        crate::skills::scan_all_skills()
    }

    fn list_plugins(&self) -> Vec<crate::plugins::PluginItem> {
        crate::plugins::scan_with_catalog()
    }

    fn set_plugin_enabled(&self, plugin_id: &str, enabled: bool) -> Result<(), String> {
        crate::claude_cli::set_plugin_enabled(plugin_id, enabled).map_err(|e| e.to_string())
    }

    fn install_plugin(&self, plugin_id: &str) -> Result<(), String> {
        crate::claude_cli::install_plugin(plugin_id).map_err(|e| e.to_string())
    }

    fn uninstall_plugin(&self, plugin_id: &str) -> Result<(), String> {
        crate::claude_cli::uninstall_plugin(plugin_id).map_err(|e| e.to_string())
    }

    fn list_marketplaces(&self) -> Vec<crate::claude_cli::CliMarketplace> {
        crate::claude_cli::list_marketplaces().unwrap_or_default()
    }

    fn add_marketplace(&self, source: &str) -> Result<(), String> {
        crate::claude_cli::add_marketplace(source).map_err(|e| e.to_string())
    }

    fn remove_marketplace(&self, name: &str) -> Result<(), String> {
        crate::claude_cli::remove_marketplace(name).map_err(|e| e.to_string())
    }

    fn get_skill_content(&self, path: &str) -> Result<String, String> {
        crate::skills::read_skill_file(path)
    }

    fn list_skill_files(
        &self,
        skill_path: &str,
    ) -> Result<Vec<crate::skills::SkillFileEntry>, String> {
        crate::skills::list_skill_files(skill_path)
    }

    fn delete_skill(&self, skill_path: &str) -> Result<(), String> {
        crate::skills::delete_skill(skill_path)
    }

    fn get_skill_history(
        &self,
        jsonl_path: &str,
    ) -> Result<Vec<claw_fleet_core::skill_history::SkillInvocation>, String> {
        use claw_fleet_core::skill_history;
        let main_path = std::path::Path::new(jsonl_path);
        let main_msgs = self.get_messages(jsonl_path)?;
        let mut out = skill_history::extract_from_messages(&main_msgs, false);

        for sub in skill_history::subagent_jsonl_paths(main_path) {
            let sub_str = sub.to_string_lossy().to_string();
            // Best-effort: a single broken subagent file shouldn't lose the rest.
            let Ok(msgs) = self.get_messages(&sub_str) else { continue };
            out.extend(skill_history::extract_from_messages(&msgs, true));
        }

        skill_history::sort_by_timestamp(&mut out);
        Ok(out)
    }

    fn get_workflow_trees(
        &self,
        jsonl_path: &str,
    ) -> Result<Vec<claw_fleet_core::workflow::WorkflowTree>, String> {
        Ok(claw_fleet_core::workflow::discover_workflow_trees(
            std::path::Path::new(jsonl_path),
        ))
    }

    fn get_task_token_breakdown(
        &self,
        main_jsonl_path: &str,
        project_root: Option<&str>,
    ) -> Result<claw_fleet_core::token_analysis::TaskTokenBreakdown, String> {
        let main_path = std::path::Path::new(main_jsonl_path);
        let project_path = project_root.map(std::path::Path::new);
        claw_fleet_core::token_analysis::aggregate_task(main_path, project_path)
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

    fn apply_guard_hook(&self) -> Result<(), String> {
        crate::hooks::apply_guard_hook()
    }

    fn remove_guard_hook(&self) -> Result<(), String> {
        crate::hooks::remove_guard_hook()
    }

    fn respond_to_guard(
        &self,
        id: &str,
        allow: bool,
        always_allow: Option<crate::guard::GuardAlwaysAllow>,
        reason: Option<String>,
    ) -> Result<(), String> {
        use crate::guard::{GuardDecision, GuardResponse};

        // Persist the user's "always allow" rule before writing the response
        // file — that way a later guard invocation for the same prefix already
        // sees the rule on disk and short-circuits.  Only honour always_allow
        // when the user actually allowed; a "Block + always allow" combo would
        // be nonsensical.
        if allow {
            if let Some(rule) = always_allow.as_ref() {
                if !rule.prefix.trim().is_empty() {
                    crate::audit::add_guard_allow_rule(
                        rule.prefix.clone(),
                        rule.source_tag.clone(),
                    );
                }
            }
        }

        let resp = GuardResponse {
            id: id.to_string(),
            decision: if allow {
                GuardDecision::Allow
            } else {
                GuardDecision::Block
            },
            reason: if allow { None } else { reason },
        };
        let result = crate::guard::write_response(&resp);
        let summary = if allow { "✅ Allow" } else { "🚫 Block" };
        resolve_feishu_card(
            id,
            claw_fleet_core::feishu::resolved_card(
                claw_fleet_core::feishu::DecisionKind::Guard,
                summary,
            ),
        );
        result
    }

    fn analyze_guard_command(&self, command: &str, context: &str, lang: &str) -> Result<String, String> {
        use crate::audit;
        use crate::guard;
        use crate::llm_provider;

        let risk_tags = audit::classify_bash_command_pub(command)
            .map(|(_, tags)| tags)
            .unwrap_or_default();

        let prompt = guard::build_analysis_prompt(command, &risk_tags, context, lang);

        let llm_cfg = self.get_llm_config();
        if llm_cfg.provider == "none" {
            return Err("LLM provider is disabled".to_string());
        }

        let provider = llm_provider::resolve_provider(&llm_cfg.provider)
            .ok_or("LLM provider not available")?;

        if !provider.is_available() {
            return Err(format!("{} CLI not found", provider.display_name()));
        }

        let timeout = std::time::Duration::from_secs(30);
        crate::llm_usage::complete_accounted(
            provider.as_ref(),
            &prompt,
            &llm_cfg.fast_model,
            timeout,
            crate::llm_usage::SCENARIO_GUARD_COMMAND,
        )
        .ok_or_else(|| "LLM analysis timed out or failed".to_string())
    }

    fn list_guard_allow_rules(&self) -> Vec<crate::audit::GuardAllowRule> {
        crate::audit::list_guard_allow_rules()
    }

    fn remove_guard_allow_rule(&self, id: &str) -> Result<(), String> {
        crate::audit::remove_guard_allow_rule(id)
    }

    fn apply_elicitation_hook(&self) -> Result<(), String> {
        crate::hooks::apply_elicitation_hook()
    }

    fn remove_elicitation_hook(&self) -> Result<(), String> {
        crate::hooks::remove_elicitation_hook()
    }

    fn apply_interaction_mode(&self, user_title: &str, locale: &str) -> Result<(), String> {
        crate::interaction_mode::apply_interaction_mode(user_title, locale)
    }

    fn remove_interaction_mode(&self) -> Result<(), String> {
        crate::interaction_mode::remove_interaction_mode()
    }

    fn apply_wiki_guidance(&self, locale: &str) -> Result<(), String> {
        crate::wiki_guidance::apply_wiki_guidance(locale)
    }

    fn remove_wiki_guidance(&self) -> Result<(), String> {
        crate::wiki_guidance::remove_wiki_guidance()
    }

    fn interaction_diagnostics(
        &self,
    ) -> Vec<crate::interaction_mode_diagnostics::DiagnosticCheck> {
        crate::interaction_mode_diagnostics::run_checks()
    }

    fn test_decision_end_to_end(
        &self,
    ) -> Result<crate::interaction_mode_test::TestRunResult, String> {
        crate::interaction_mode_test::run_end_to_end_test(std::time::Duration::from_secs(10))
    }

    fn test_decision_via_claude_cli(
        &self,
    ) -> Result<crate::interaction_mode_test::TestRunResult, String> {
        crate::interaction_mode_test::run_claude_cli_test(std::time::Duration::from_secs(60))
    }

    fn apply_prd_mode(&self, user_title: &str, locale: &str) -> Result<(), String> {
        crate::prd_discipline::apply_prd_discipline(user_title, locale)?;
        crate::hooks::apply_prd_context_hook()?;
        Ok(())
    }

    fn remove_prd_mode(&self) -> Result<(), String> {
        // Remove both halves regardless of which fails — best effort, then
        // surface the first error if any so the UI can re-try.
        let r1 = crate::prd_discipline::remove_prd_discipline();
        let r2 = crate::hooks::remove_prd_context_hook();
        r1.and(r2)
    }

    fn respond_to_elicitation(
        &self,
        id: &str,
        declined: bool,
        answers: std::collections::HashMap<String, String>,
    ) -> Result<(), String> {
        // Render the read-only Feishu card (questions + all options, pick(s)
        // highlighted) before `answers` is moved into the response.
        let resolved =
            claw_fleet_core::feishu::resolved_elicitation_card(id, &answers, declined);
        let resp = crate::elicitation::ElicitationResponse {
            id: id.to_string(),
            declined,
            answers,
        };
        let result = crate::elicitation::write_response(&resp);
        resolve_feishu_card(id, resolved);
        result
    }

    fn respond_to_fleet_ask(
        &self,
        id: &str,
        cancelled: bool,
        answers: std::collections::BTreeMap<String, String>,
    ) -> Result<(), String> {
        let resp = claw_fleet_core::mcp_ipc::FleetAskResponse {
            id: id.to_string(),
            answers,
            cancelled,
        };
        claw_fleet_core::mcp_ipc::write_response(&resp)
    }

    fn respond_to_permission_prompt(
        &self,
        id: &str,
        allow: bool,
        reason: Option<String>,
    ) -> Result<(), String> {
        let resp = claw_fleet_core::permission_prompt_ipc::PermissionPromptResponse {
            id: id.to_string(),
            decision: if allow {
                claw_fleet_core::permission_prompt_ipc::PermissionPromptDecision::Allow
            } else {
                claw_fleet_core::permission_prompt_ipc::PermissionPromptDecision::Deny
            },
            reason,
        };
        claw_fleet_core::permission_prompt_ipc::write_response(&resp)
    }

    fn respond_to_a2ui_render(
        &self,
        id: &str,
        cancelled: bool,
        action_name: Option<String>,
        action_context: std::collections::BTreeMap<String, String>,
    ) -> Result<(), String> {
        let resp = claw_fleet_core::mcp_a2ui_ipc::A2uiRenderResponse {
            id: id.to_string(),
            action_name,
            action_context,
            cancelled,
        };
        claw_fleet_core::mcp_a2ui_ipc::write_response(&resp)
    }

    fn apply_mcp_injector(&self, fleet_path: &str) -> Result<(), String> {
        claw_fleet_core::mcp_injector::acquire(std::process::id(), fleet_path)
            .map_err(|e| e.to_string())
    }

    fn apply_plan_approval_hook(&self) -> Result<(), String> {
        crate::hooks::apply_plan_approval_hook()
    }

    fn remove_plan_approval_hook(&self) -> Result<(), String> {
        crate::hooks::remove_plan_approval_hook()
    }

    fn list_pending_plan_approvals(&self) -> Vec<crate::plan_approval::PlanApprovalRequest> {
        let ids = crate::plan_approval::list_pending_requests();
        let sessions = self.sessions.lock().unwrap().clone();
        ids.iter()
            .filter_map(|id| {
                let mut req = crate::plan_approval::read_request(id)?;
                if let Some(s) = sessions.iter().find(|s| s.id == req.session_id) {
                    if req.workspace_name.is_empty() {
                        req.workspace_name = s.workspace_name.clone();
                    }
                    if req.ai_title.is_none() {
                        req.ai_title = s.ai_title.clone();
                    }
                }
                Some(req)
            })
            .collect()
    }

    fn list_pending_decisions(&self) -> claw_fleet_core::backend::PendingDecisions {
        let mut pending = claw_fleet_core::backend::PendingDecisions {
            guard: crate::guard::list_pending_requests()
                .iter()
                .filter_map(|id| crate::guard::read_request(id))
                .collect(),
            elicitation: crate::elicitation::list_pending_requests()
                .iter()
                .filter_map(|id| crate::elicitation::read_request(id))
                .collect(),
            fleet_ask: claw_fleet_core::mcp_ipc::list_pending_requests()
                .iter()
                .filter_map(|id| claw_fleet_core::mcp_ipc::read_request(id))
                .collect(),
            a2ui_render: claw_fleet_core::mcp_a2ui_ipc::list_pending_requests()
                .iter()
                .filter_map(|id| claw_fleet_core::mcp_a2ui_ipc::read_request(id))
                .collect(),
            plan_approval: crate::plan_approval::list_pending_requests()
                .iter()
                .filter_map(|id| crate::plan_approval::read_request(id))
                .collect(),
            permission_prompt: claw_fleet_core::permission_prompt_ipc::list_pending_requests()
                .iter()
                .filter_map(|id| claw_fleet_core::permission_prompt_ipc::read_request(id))
                .collect(),
        };
        let sessions = self.sessions.lock().unwrap().clone();
        claw_fleet_core::backend::resolve_pending_display(&mut pending, &sessions);
        pending
    }

    fn respond_to_plan_approval(
        &self,
        id: &str,
        decision: &str,
        edited_plan: Option<String>,
        feedback: Option<String>,
    ) -> Result<(), String> {
        let summary = match decision {
            "approve" => "✅ Approved".to_string(),
            "reject" => match feedback.as_deref() {
                Some(f) if !f.is_empty() => format!("🚫 Rejected — {f}"),
                _ => "🚫 Rejected".to_string(),
            },
            other => format!("decision: {other}"),
        };
        let resp = crate::plan_approval::PlanApprovalResponse {
            id: id.to_string(),
            decision: decision.to_string(),
            edited_plan,
            feedback,
        };
        let result = crate::plan_approval::write_response(&resp);
        resolve_feishu_card(
            id,
            claw_fleet_core::feishu::resolved_card(
                claw_fleet_core::feishu::DecisionKind::PlanApproval,
                &summary,
            ),
        );
        result
    }

    fn list_session_decisions(
        &self,
        session_id: &str,
        jsonl_path: Option<&str>,
    ) -> Vec<crate::decision_history::DecisionHistoryRecord> {
        let resolved = if jsonl_path.is_none() {
            self.sessions
                .lock()
                .unwrap()
                .iter()
                .find(|s| s.id == session_id)
                .map(|s| s.jsonl_path.clone())
        } else {
            None
        };
        let path = jsonl_path.or(resolved.as_deref()).map(std::path::Path::new);
        crate::decision_history::list_session_records_with_jsonl(session_id, path)
    }

    fn get_sources_config(&self) -> Vec<crate::agent_source::SourceInfo> {
        crate::agent_source::get_sources_config_local()
    }

    fn set_source_enabled(&self, name: &str, enabled: bool) -> Result<(), String> {
        crate::agent_source::set_source_enabled_local(name, enabled)
    }

    fn list_claude_binaries(&self) -> Vec<crate::claude_binary::ClaudeBinary> {
        crate::claude_binary::discover()
    }

    fn get_claude_binary_override(&self) -> Option<String> {
        crate::claude_binary::ClaudeBinaryConfig::load().override_path
    }

    fn set_claude_binary_override(&self, path: Option<String>) -> Result<(), String> {
        let cleaned = path.and_then(|p| {
            let trimmed = p.trim().to_string();
            if trimmed.is_empty() { None } else { Some(trimmed) }
        });
        let config = crate::claude_binary::ClaudeBinaryConfig { override_path: cleaned };
        config.save()
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

        // Collect events from sessions that just went idle — persist them
        // before evicting from the cache.
        let evicted: Vec<crate::audit::AuditEvent> = cache
            .iter()
            .filter(|(id, _)| !active_ids.contains(id.as_str()))
            .flat_map(|(_, (_, events))| events.clone())
            .collect();
        cache.retain(|id, _| active_ids.contains(id));

        let mut history = self.audit_history.lock().unwrap();
        history.persist_evicted(evicted);

        // If a session became active again, remove it from history so we don't
        // double-count.  The live cache will re-scan the full file.
        history.remove_sessions(&active_ids);

        let mut all_events = Vec::new();

        // Include persisted historical events.
        all_events.extend_from_slice(history.events());
        drop(history);

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

    fn get_audit_rules(&self) -> Vec<crate::audit::AuditRuleInfo> {
        crate::audit::get_all_rules()
    }

    fn set_audit_rule_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        crate::audit::set_rule_enabled(id, enabled)
    }

    fn save_custom_audit_rule(&self, rule: crate::audit::AuditRuleInfo) -> Result<(), String> {
        crate::audit::save_custom_rule(rule)
    }

    fn delete_custom_audit_rule(&self, id: &str) -> Result<(), String> {
        crate::audit::delete_custom_rule(id)
    }

    fn suggest_audit_rules(&self, concern: &str, lang: &str) -> Result<Vec<crate::audit::SuggestedRule>, String> {
        let existing_tags: Vec<String> = crate::audit::get_all_rules()
            .iter()
            .map(|r| r.tag.clone())
            .collect();
        let prompt = crate::audit::build_suggest_rules_prompt(concern, lang, &existing_tags);

        let cfg = self.llm_config.lock().unwrap().clone();
        let provider = crate::llm_provider::resolve_provider(&cfg.provider)
            .ok_or_else(|| "No LLM provider available".to_string())?;

        let response = crate::llm_usage::complete_accounted(
            provider.as_ref(),
            &prompt,
            &cfg.standard_model,
            std::time::Duration::from_secs(120),
            crate::llm_usage::SCENARIO_AUDIT_RULES,
        )
        .ok_or_else(|| "LLM did not return a response".to_string())?;

        // Extract JSON array from the response (may have markdown fences).
        let json_str = response.trim();
        let json_str = json_str
            .strip_prefix("```json")
            .or_else(|| json_str.strip_prefix("```"))
            .unwrap_or(json_str);
        let json_str = json_str.strip_suffix("```").unwrap_or(json_str).trim();

        serde_json::from_str::<Vec<crate::audit::SuggestedRule>>(json_str)
            .map_err(|e| format!("Failed to parse LLM response: {e}"))
    }

    fn search_sessions(&self, query: &str, limit: usize) -> Vec<crate::search_index::SearchHit> {
        match self.search_index.lock() {
            Ok(idx) => idx.search(query, limit).unwrap_or_default(),
            Err(_) => vec![],
        }
    }

    fn get_daily_report(&self, date: &str) -> Result<Option<crate::daily_report::DailyReport>, String> {
        self.report_store.lock().unwrap().get_report(date)
    }

    fn list_daily_report_stats(&self, from: &str, to: &str) -> Vec<crate::daily_report::DailyReportStats> {
        self.report_store
            .lock()
            .unwrap()
            .list_stats(from, to)
            .unwrap_or_default()
    }

    fn generate_daily_report(&self, date: &str) -> Result<crate::daily_report::DailyReport, String> {
        // Try in-memory session cache first (covers last 7 days)
        let cached: Vec<SessionInfo> = {
            let all = self.sessions.lock().unwrap();
            all.iter()
                .filter(|s| {
                    if s.created_at_ms == 0 {
                        return false;
                    }
                    let secs = (s.created_at_ms / 1000) as i64;
                    chrono::DateTime::from_timestamp(secs, 0)
                        .map(|dt| {
                            dt.with_timezone(&chrono::Local)
                                .format("%Y-%m-%d")
                                .to_string()
                                == date
                        })
                        .unwrap_or(false)
                })
                .cloned()
                .collect()
        };

        let sessions = if cached.is_empty() {
            // Fallback: scan from disk (for older dates / backfill)
            crate::daily_report::scan_sessions_for_date(date)
        } else {
            cached
        };

        if sessions.is_empty() {
            return Err(format!("No sessions found for {date}"));
        }

        let session_refs: Vec<&SessionInfo> = sessions.iter().collect();
        let tz = chrono::Local::now().format("%Z").to_string();
        let report =
            crate::daily_report::generate_report_from_sessions(date, &tz, &session_refs);

        self.report_store
            .lock()
            .unwrap()
            .save_report(&report)
            .map_err(|e| format!("save report: {e}"))?;

        Ok(report)
    }

    fn generate_daily_report_ai_summary(&self, date: &str) -> Result<String, String> {
        let report = self
            .report_store
            .lock()
            .unwrap()
            .get_report(date)
            .map_err(|e| format!("load report: {e}"))?
            .ok_or_else(|| format!("No report found for {date}"))?;

        let lang = self.locale.lock().unwrap().clone();
        let cfg = self.llm_config.lock().unwrap().clone();
        let provider = crate::llm_provider::resolve_provider(&cfg.provider)
            .ok_or_else(|| format!("unknown LLM provider '{}'", cfg.provider))?;
        let summary = crate::daily_report::generate_ai_summary(
            provider.as_ref(), &cfg.standard_model, &report, &lang,
        ).ok_or_else(|| "AI summary generation failed".to_string())?;

        self.report_store
            .lock()
            .unwrap()
            .update_ai_summary(date, &summary)
            .map_err(|e| format!("save summary: {e}"))?;

        Ok(summary)
    }

    fn generate_daily_report_lessons(&self, date: &str) -> Result<Vec<crate::daily_report::Lesson>, String> {
        let report = self
            .report_store
            .lock()
            .unwrap()
            .get_report(date)
            .map_err(|e| format!("load report: {e}"))?
            .ok_or_else(|| format!("No report found for {date}"))?;

        let lang = self.locale.lock().unwrap().clone();
        let cfg = self.llm_config.lock().unwrap().clone();
        let provider = crate::llm_provider::resolve_provider(&cfg.provider)
            .ok_or_else(|| format!("unknown LLM provider '{}'", cfg.provider))?;
        let lessons = crate::daily_report::generate_lessons(
            provider.as_ref(), &cfg.standard_model, &report, &lang,
        ).ok_or_else(|| "Lessons generation failed".to_string())?;

        self.report_store
            .lock()
            .unwrap()
            .update_lessons(date, &lessons)
            .map_err(|e| format!("save lessons: {e}"))?;

        Ok(lessons)
    }

    fn append_lesson_to_claude_md(&self, lesson: &crate::daily_report::Lesson) -> Result<(), String> {
        crate::daily_report::append_lesson_to_claude_md(lesson)
    }

    fn list_llm_providers(&self) -> Vec<crate::llm_provider::LlmProviderInfo> {
        crate::llm_provider::all_provider_infos()
    }

    fn get_llm_config(&self) -> crate::llm_provider::LlmConfig {
        self.llm_config.lock().unwrap().clone()
    }

    fn set_llm_config(&self, config: crate::llm_provider::LlmConfig) -> Result<(), String> {
        // Mirror into the process-wide slot so the mobile relay's
        // `guard_analyze` follows the same provider choice.
        crate::llm_provider::set_shared_config(config.clone());
        *self.llm_config.lock().unwrap() = config;
        Ok(())
    }

    fn list_fleet_llm_usage_daily(
        &self,
        from_ms: u64,
        to_ms: u64,
    ) -> Vec<crate::llm_usage::FleetLlmUsageDailyBucket> {
        crate::llm_usage::list_usage_daily_buckets(from_ms, to_ms)
    }

    fn usage_history(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Vec<crate::account::UsageHistoryPoint> {
        crate::account::load_usage_history(from_ms, to_ms)
    }

    fn upload_attachment(
        &self,
        source_path: &std::path::Path,
        from_clipboard: bool,
    ) -> Result<String, String> {
        let abs = source_path.canonicalize().map_err(|e| e.to_string())?;
        let meta = std::fs::metadata(&abs).map_err(|e| e.to_string())?;
        if meta.len() > claw_fleet_core::backend::MAX_ATTACHMENT_BYTES {
            return Err(format!(
                "attachment too large: {} bytes (max {})",
                meta.len(),
                claw_fleet_core::backend::MAX_ATTACHMENT_BYTES
            ));
        }
        if !from_clipboard {
            // A file the user picked — the agent runs on this machine and the
            // path already means something. Hand it back untouched.
            return Ok(abs.to_string_lossy().into_owned());
        }
        // Pasted bytes, currently parked in $TMPDIR. Move them somewhere that
        // outlives the OS's temp reaper, since this path goes into the transcript.
        let name = abs
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "attachment.bin".to_string());
        let stored = claw_fleet_core::user_attachments::ingest(&abs, &name)?;
        Ok(stored.to_string_lossy().into_owned())
    }

    fn get_user_attachment(
        &self,
        key: &str,
        name: &str,
    ) -> Result<claw_fleet_core::mcp_ipc::DecisionAssetBytes, String> {
        claw_fleet_core::user_attachments::read_user_attachment(key, name)
    }

    fn start_feishu_oauth(&self) -> Result<claw_fleet_core::feishu::OauthHandle, String> {
        claw_fleet_core::feishu::start_oauth()
    }
    fn poll_feishu_oauth(
        &self,
        state: &str,
    ) -> Result<claw_fleet_core::feishu::OauthStatus, String> {
        claw_fleet_core::feishu::poll_oauth(state)
    }
    fn feishu_status(&self) -> Result<claw_fleet_core::feishu::FeishuConnection, String> {
        claw_fleet_core::feishu::status()
    }
    fn disconnect_feishu(&self) -> Result<(), String> {
        claw_fleet_core::feishu::disconnect()
    }
    fn get_feishu_creds(&self) -> Result<claw_fleet_core::feishu::StoredCreds, String> {
        Ok(claw_fleet_core::feishu::get_stored_creds())
    }
    fn set_feishu_creds(
        &self,
        creds: claw_fleet_core::feishu::StoredCreds,
    ) -> Result<(), String> {
        claw_fleet_core::feishu::set_stored_creds(creds)
    }

    fn get_mobile_relay_config(
        &self,
    ) -> Result<claw_fleet_core::mobile_relay::MobileRelayConfig, String> {
        Ok(claw_fleet_core::mobile_relay::load_config())
    }

    fn set_mobile_relay_config(
        &self,
        cfg: claw_fleet_core::mobile_relay::MobileRelayConfig,
    ) -> Result<claw_fleet_core::mobile_relay::MobileRelayConfig, String> {
        claw_fleet_core::mobile_relay::set_config_normalized(cfg)
    }

    fn rotate_mobile_relay_secret(
        &self,
    ) -> Result<claw_fleet_core::mobile_relay::MobileRelayConfig, String> {
        claw_fleet_core::mobile_relay::rotate_secret()
    }

    fn mobile_relay_status(
        &self,
    ) -> Result<claw_fleet_core::mobile_relay::MobileRelayStatus, String> {
        Ok(claw_fleet_core::mobile_relay::status())
    }

    fn mobile_relay_qr_svg(&self) -> Result<String, String> {
        claw_fleet_core::mobile_relay::qr_svg()
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

/// Statuses that represent the agent actively working. A transition from any
/// of these into `WaitingInput` is a genuine "task completed" signal worth
/// notifying the user about.
///
/// Note: `Delegating` belongs here — a main session in `Delegating` is waiting
/// on its subagents and is genuinely busy. A `Delegating → WaitingInput`
/// transition (subagents finish, main session returns to prompt) must notify.
const BUSY_STATUSES: &[SessionStatus] = &[
    SessionStatus::Thinking,
    SessionStatus::Executing,
    SessionStatus::Streaming,
    SessionStatus::Processing,
    SessionStatus::Delegating,
    SessionStatus::Active,
];

/// Decide whether a status change should fire a "task completed" notification.
///
/// Requires a genuine busy→WaitingInput transition: the previous observation
/// must have been one of `BUSY_STATUSES`. This intentionally suppresses:
/// - Cold start (prev is None) — Fleet just saw this session for the first time.
/// - `--resume` of an old session (prev is None or Idle) — opening a past
///   session re-touches the JSONL and makes it look WaitingInput, but no task
///   was actually completed just now.
/// - WaitingInput → WaitingInput (already waiting, no new transition).
pub(crate) fn should_notify_waiting_transition(
    prev: Option<&SessionStatus>,
    current: &SessionStatus,
) -> bool {
    if current != &SessionStatus::WaitingInput {
        return false;
    }
    match prev {
        Some(p) => BUSY_STATUSES.contains(p),
        None => false,
    }
}

fn detect_waiting_transitions(
    sessions: &Arc<Mutex<Vec<SessionInfo>>>,
    prev_statuses: &mut HashMap<String, SessionStatus>,
    analyzing: &Arc<Mutex<HashSet<String>>>,
    waiting_alerts: &Arc<Mutex<HashMap<String, WaitingAlert>>>,
    session_outcomes: &Arc<Mutex<HashMap<String, Vec<String>>>>,
    app: &AppHandle,
    locale: &Arc<Mutex<String>>,
    llm_config: &Arc<Mutex<crate::llm_provider::LlmConfig>>,
) {
    let current = sessions.lock().unwrap().clone();
    let mut alerts_changed = false;

    for sess in &current {
        if sess.is_subagent {
            continue;
        }

        let prev = prev_statuses.get(&sess.id);
        let is_waiting = sess.status == SessionStatus::WaitingInput;
        let was_waiting = prev == Some(&SessionStatus::WaitingInput);
        let was_busy = prev.map_or(false, |p| BUSY_STATUSES.contains(p));
        let should_notify = should_notify_waiting_transition(prev, &sess.status);

        // Session just transitioned from a busy state into WaitingInput →
        // run semantic analysis. Cold start / --resume (prev == None) is
        // deliberately suppressed by should_notify_waiting_transition.
        if should_notify {
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
            let agent_source = sess.agent_source.clone();
            let wa = waiting_alerts.clone();
            let so = session_outcomes.clone();
            let an = analyzing.clone();
            let app_bg = app.clone();
            let lang = locale.lock().unwrap().clone();
            let title = get_user_title(&app_bg);
            let cfg = llm_config.lock().unwrap().clone();

            std::thread::spawn(move || {
                let analysis_text = extract_last_assistant_text(&jsonl_path, 1000)
                    .unwrap_or(last_text);

                let provider = crate::llm_provider::resolve_provider(&cfg.provider);
                let result = provider.as_ref().and_then(|p| {
                    crate::claude_analyze::analyze_session_outcome(
                        p.as_ref(), &cfg.fast_model, &analysis_text, &lang, &session_id, &title,
                    )
                });
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
                        source: agent_source.clone(),
                    };
                    wa.lock().unwrap().insert(session_id, alert);
                    let alerts: Vec<WaitingAlert> =
                        wa.lock().unwrap().values().cloned().collect();
                    let _ = app_bg.emit("waiting-alerts-updated", &alerts);
                    if should_os_notify {
                        send_os_notification(&app_bg, &display_name, &summary);
                    }

                    // Play TTS from backend (blocks until done).
                    // Claude Code sessions route their wait-for-input through
                    // the AskUserQuestion → DecisionPanel bridge, which owns
                    // audio playback there. Suppress the waitalert TTS for
                    // those to avoid double-announcements.
                    if agent_source != "claude-code" {
                        crate::play_tts_for_notification(&app_bg, &summary);
                    }
                }
            });
        } else if !is_waiting && was_waiting {
            // Session left WaitingInput → clear alert.
            if waiting_alerts.lock().unwrap().remove(&sess.id).is_some() {
                alerts_changed = true;
            }
        }

        // Session became busy again → clear stale outcome tags.
        if BUSY_STATUSES.contains(&sess.status) && !was_busy {
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
    let content = claw_fleet_core::bom::read_to_string_no_bom(jsonl_path).ok()?;
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

/// Read the current user title from AppState (empty string = default "老板"/"Boss").
pub(crate) fn get_user_title(app: &AppHandle) -> String {
    use tauri::Manager;
    app.try_state::<crate::AppState>()
        .map(|s| s.user_title.lock().unwrap().clone())
        .unwrap_or_default()
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

    fn mk_session(id: &str, source: &str) -> crate::session::SessionInfo {
        use crate::session::{SessionInfo, SessionStatus};
        SessionInfo {
            id: id.into(),
            workspace_path: "/tmp/test".into(),
            workspace_name: "test".into(),
            ide_name: None,
            entrypoint: None,
            is_subagent: false,
            parent_session_id: None,
            agent_type: None,
            agent_description: None,
            slug: None,
            ai_title: None,
            status: SessionStatus::Idle,
            token_speed: 0.0,
            agent_token_speed: 0.0,
            total_output_tokens: 0,
            total_cost_usd: 0.0,
            agent_total_cost_usd: 0.0,
            cost_speed_usd_per_min: 0.0,
            last_message_preview: None,
            last_activity_ms: 0,
            created_at_ms: 0,
            jsonl_path: format!("/tmp/{id}.jsonl"),
            model: None,
            thinking_level: None,
            pid: None,
            pid_precise: false,
            proc_alive: false,
            pending_tool_batch: false,
            last_skill: None,
            context_percent: None,
            agent_source: source.into(),
            last_outcome: None,
            rate_limit: None,
            todos: None,
            background_tasks: Vec::new(),
            task_plan: None,
            handoff: None,
            user_mark: None,
            last_read_ms: None,
            compact_count: 0,
            compact_pre_tokens: 0,
            compact_post_tokens: 0,
            compact_cost_usd: 0.0,
        }
    }

    /// The launchpad's mark filter reads `user_mark` off the sessions the
    /// scanner emits, and the read/unread dot reads `last_read_ms`. Both are
    /// stamped by scan-time enrichers, and the *incremental* rescan (the hot
    /// path behind every file event) used to run only the handoff enricher —
    /// so a freshly-scanned session came back with both fields cleared and the
    /// segment counts never moved off "all pending".
    #[test]
    fn incremental_rescan_stamps_mark_and_read_state() {
        use claw_fleet_core::session_mark::SessionMark;
        use claw_fleet_core::session_read::SessionReadItem;

        let _lock = claw_fleet_core::paths::fleet_home_lock();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        std::env::set_var("FLEET_HOME", tmp.path());

        // The human marked this session done and read it — both live on disk.
        claw_fleet_core::session_mark::set_mark("sess-1", "/tmp/test", Some(SessionMark::Done))
            .unwrap();
        claw_fleet_core::session_read::mark_read(&[SessionReadItem {
            session_id: "sess-1".into(),
            workspace_path: "/tmp/test".into(),
        }])
        .unwrap();

        // A file event marks the source dirty, so its sessions get re-scanned
        // fresh off the jsonl — i.e. with `user_mark`/`last_read_ms` unset.
        let sources: Vec<Box<dyn AgentSource>> = vec![Box::new(MockSource {
            sessions: vec![mk_session("sess-1", "claude-code")],
            ..MockSource::new("claude-code", "claude", "")
        })];
        let out = build_incremental_sessions(&sources, &[], &HashSet::from([0]), 0);

        match prev {
            Some(v) => std::env::set_var("FLEET_HOME", v),
            None => std::env::remove_var("FLEET_HOME"),
        }

        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].user_mark,
            Some(SessionMark::Done),
            "incremental rescan dropped the on-disk done mark",
        );
        assert!(
            out[0].last_read_ms.is_some(),
            "incremental rescan dropped the on-disk read state",
        );
    }

    /// Minimal mock for local_backend tests (duplicated to avoid cross-module test deps).
    struct MockSource {
        name: &'static str,
        api_name: &'static str,
        prefix: &'static str,
        available: bool,
        account: Result<serde_json::Value, String>,
        usage: Result<serde_json::Value, String>,
        summary: Option<SourceUsageSummary>,
        sessions: Vec<crate::session::SessionInfo>,
    }

    impl MockSource {
        fn new(name: &'static str, api_name: &'static str, prefix: &'static str) -> Self {
            Self {
                name, api_name, prefix,
                available: true,
                account: Err("n/a".into()),
                usage: Err("n/a".into()),
                summary: None,
                sessions: vec![],
            }
        }
    }

    impl AgentSource for MockSource {
        fn name(&self) -> &'static str { self.name }
        fn api_name(&self) -> &'static str { self.api_name }
        fn uri_prefix(&self) -> &'static str { self.prefix }
        fn is_available(&self) -> bool { self.available }
        fn scan_sessions(&self) -> Vec<crate::session::SessionInfo> { self.sessions.clone() }
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

    // ── should_notify_waiting_transition ───────────────────────────────────

    use SessionStatus::*;

    #[test]
    fn notify_busy_to_waiting_fires_for_all_busy_statuses() {
        // Canonical "task just completed" transitions — all must notify.
        for busy in &[Thinking, Executing, Streaming, Processing, Delegating, Active] {
            assert!(
                should_notify_waiting_transition(Some(busy), &WaitingInput),
                "{:?} → WaitingInput should notify",
                busy
            );
        }
    }

    #[test]
    fn notify_cold_start_waiting_does_not_fire() {
        // Fleet just started / session was previously absent. We have no
        // evidence the agent was busy, so we must NOT claim "task completed".
        assert!(!should_notify_waiting_transition(None, &WaitingInput));
    }

    #[test]
    fn notify_resume_from_idle_does_not_fire() {
        // `--resume` of a session that had aged out to Idle: opening it can
        // re-touch the JSONL and flip it to WaitingInput, but no task was
        // actually completed right now — suppress the notification.
        assert!(!should_notify_waiting_transition(Some(&Idle), &WaitingInput));
    }

    #[test]
    fn notify_waiting_to_waiting_does_not_fire() {
        // Already in WaitingInput — the user hasn't done anything new.
        assert!(!should_notify_waiting_transition(Some(&WaitingInput), &WaitingInput));
    }

    #[test]
    fn notify_non_waiting_target_never_fires() {
        // Only transitions *to* WaitingInput are notification triggers.
        for target in &[Thinking, Executing, Streaming, Processing, Delegating, Active, Idle] {
            assert!(
                !should_notify_waiting_transition(Some(&Streaming), target),
                "Streaming → {:?} should not notify",
                target
            );
            assert!(
                !should_notify_waiting_transition(None, target),
                "None → {:?} should not notify",
                target
            );
        }
    }

    // ── ScanGate ────────────────────────────────────────────────────────────

    #[test]
    fn scan_gate_blocks_second_entrant_until_first_drops() {
        let gate = ScanGate::new();
        let first = gate.try_enter().expect("first try_enter must succeed");
        assert!(
            gate.try_enter().is_none(),
            "second try_enter must fail while first guard is held"
        );
        drop(first);
        assert!(
            gate.try_enter().is_some(),
            "try_enter must succeed once the prior guard drops"
        );
    }

    #[test]
    fn scan_gate_never_admits_two_at_once_under_contention() {
        // Mimics the Windows pathology: many threads racing into the gate
        // while one is already holding it. The contract is "at any instant
        // ≤ 1 guard exists". We track concurrent guard count and assert
        // it never exceeds 1 across all racing threads.
        use std::sync::atomic::AtomicUsize;
        let gate = Arc::new(ScanGate::new());
        let concurrent = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..32 {
            let g = gate.clone();
            let c = concurrent.clone();
            let p = peak.clone();
            handles.push(std::thread::spawn(move || {
                if let Some(_slot) = g.try_enter() {
                    let now = c.fetch_add(1, Ordering::SeqCst) + 1;
                    p.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    c.fetch_sub(1, Ordering::SeqCst);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1, "gate let two scans run at once");
        assert!(gate.try_enter().is_some(), "gate must be free after all contenders exit");
    }
}
