//! Headless control-plane orchestration extracted from the desktop
//! `LocalBackend`.
//!
//! The desktop backend runs a periodic ticker that drives three
//! non-UI reconciliation jobs: auto-resume of rate-limited/errored sessions,
//! delivery of queued follow-up messages, and interruption of hung Codex
//! turns. None of this touches the Tauri `AppHandle` or emits events — it is
//! pure orchestration over `SessionInfo` and core primitives — so it can run
//! unchanged inside a headless host (`fleet serve`) where there is no window.
//!
//! The individual `maybe_*` functions stay standalone so the desktop can keep
//! its three call sites (fs-watcher / poll / ticker) sharing one set of state
//! maps. Headless callers instead use [`run`], which owns its own state and
//! loops on a 30s ticker.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::log_debug;
use crate::session::SessionInfo;

/// Ticker interval. Matches the desktop auto-resume ticker: rate-limited
/// sessions produce no JSONL writes, so a file watcher alone would let
/// `resets_at` pass unnoticed — a fixed cadence guarantees a check.
const TICK_INTERVAL: Duration = Duration::from_secs(30);

/// Max number of `claude --resume` auto-resume processes alive at once. Each
/// is a full Claude Code process (~150-200MB), so an unbounded fan-out of a
/// few hundred is tens of GB of RSS — the startup runaway this caps.
const AUTO_RESUME_MAX_CONCURRENT: usize = 4;

/// After this many consecutive failed resumes, a session is backed off and no
/// longer re-fired — stops the endless re-fire loop (24k+ doomed spawns seen
/// in the field) for any session whose resume can never succeed.
const AUTO_RESUME_FAILURE_BACKOFF: u32 = 3;

/// Per-session hard cap on watchdog interrupts within one run. A turn that
/// stalls again after every resume is a persistent environment problem the
/// watchdog can't fix — stop after this many attempts and leave it to the user.
const STALL_MAX_INTERRUPTS: u32 = 2;
/// Minimum spacing between watchdog interrupts of the same session, so the
/// interrupt → drain → resume → (possibly re-stall) cycle gets a full silence
/// window to prove itself before the next intervention.
const STALL_COOLDOWN: Duration = Duration::from_secs(15 * 60);

/// Deliver any queued follow-up message to a session whose turn just ended.
///
/// Runs on the same session-refresh ticks as [`maybe_fire_auto_resume`]. Each
/// session snapshot carries a fresh `proc_alive`, which is the gate
/// [`crate::pending_message::maybe_drain`] uses to know the turn is over — so a
/// message typed while the session was running is fired here, on the first tick
/// after the `claude` process exits. Independent of the auto-resume enabled
/// toggle: queuing a follow-up is a direct user action, not the rate-limit
/// recovery policy.
pub fn maybe_drain_pending_messages(sessions: &Arc<Mutex<Vec<SessionInfo>>>) {
    // Snapshot under the lock, then drain without holding it — draining spawns a
    // detached `claude`, which must not run inside the sessions mutex.
    let snapshot: Vec<SessionInfo> = { sessions.lock().unwrap().clone() };
    for session in &snapshot {
        crate::pending_message::maybe_drain(session);
    }
}

/// Detect and interrupt alive-but-hung Codex turns (see
/// [`crate::codex_source::detect_stalled_codex_turns`]). Runs off the 30s
/// ticker; detection is cheap (a process-table scan plus one stat per live
/// Codex session) and the interrupt path only fires for sessions past the
/// 10-minute silence threshold with no pending decision card.
pub fn maybe_interrupt_stalled_codex(
    sessions: &Arc<Mutex<Vec<SessionInfo>>>,
    fired: &mut HashMap<String, u32>,
    last_fire: &mut HashMap<String, Instant>,
) {
    let snapshot: Vec<SessionInfo> = { sessions.lock().unwrap().clone() };
    for stall in crate::codex_source::detect_stalled_codex_turns(&snapshot) {
        let attempts = fired.get(&stall.session_id).copied().unwrap_or(0);
        if attempts >= STALL_MAX_INTERRUPTS {
            continue;
        }
        if let Some(at) = last_fire.get(&stall.session_id) {
            if at.elapsed() < STALL_COOLDOWN {
                continue;
            }
        }
        match crate::codex_source::interrupt_stalled_codex_turn(&stall) {
            Ok(()) => log_debug(&format!(
                "[CODEX-STALL] interrupted {} (pid {}) after {}min rollout silence (attempt {}/{})",
                stall.session_id,
                stall.pid,
                stall.silence_secs / 60,
                attempts + 1,
                STALL_MAX_INTERRUPTS,
            )),
            Err(e) => log_debug(&format!(
                "[CODEX-STALL] interrupt {} (pid {}) failed: {e}",
                stall.session_id, stall.pid,
            )),
        }
        fired.insert(stall.session_id.clone(), attempts + 1);
        last_fire.insert(stall.session_id.clone(), Instant::now());
    }
}

/// A session this process's scheduler will bring back later, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResumeIntent {
    pub session_id: String,
    pub mechanism: &'static str,
    pub reason: String,
    pub not_before_ms: Option<u64>,
}

/// Lifetime of a scheduler intent. Deliberately long: the intent is withdrawn
/// explicitly the tick its session stops qualifying, and dropped with this
/// process if it dies, so the TTL is only a backstop. A short one would lapse
/// while the Mac sleeps — exactly when the reviver and the retry both come due
/// on wake (semgap-recount, 2026-09-25).
const RESUME_INTENT_TTL_MS: u64 = 24 * 3600 * 1000;

/// Sessions the rate-limit resume or the server-error retry will fire for once
/// their gate opens: the eligibility of [`crate::auto_resume::should_auto_resume`]
/// minus its time gate, and a server-error retry with budget left. Pure so the
/// selection is testable without a scheduler.
pub(crate) fn desired_resume_intents(
    sessions: &[SessionInfo],
    config: &crate::auto_resume::AutoResumeConfig,
    server_errors: &HashMap<String, u32>,
    failures: &HashMap<String, u32>,
) -> Vec<ResumeIntent> {
    sessions
        .iter()
        .filter(|s| !s.proc_alive)
        .filter_map(|s| {
            if let Some(rl) = crate::auto_resume::auto_resume_eligible(s, config) {
                if crate::auto_resume::is_backed_off(failures, &s.id, AUTO_RESUME_FAILURE_BACKOFF) {
                    return None;
                }
                return Some(ResumeIntent {
                    session_id: s.id.clone(),
                    mechanism: "auto_resume",
                    reason: format!("rate limit resets at {}", rl.resets_at.to_rfc3339()),
                    not_before_ms: u64::try_from(rl.resets_at.timestamp_millis()).ok(),
                });
            }
            let budget_left = server_errors
                .get(&s.id)
                .is_none_or(|&n| n < config.max_server_error_retries);
            (crate::auto_resume::should_retry_server_error(s, config) && budget_left).then(|| {
                ResumeIntent {
                    session_id: s.id.clone(),
                    mechanism: "server_error_retry",
                    reason: "retry after a server error".into(),
                    not_before_ms: None,
                }
            })
        })
        .filter(|i| {
            sessions
                .iter()
                .find(|s| s.id == i.session_id)
                .is_some_and(|s| crate::auto_resume::can_reach_workspace(&s.workspace_path))
        })
        // A retired session is never resumed again (the dispatcher refuses it),
        // so it must not reserve itself either: that would read as "covered" to
        // the reviver. Last, so the store is only read for real candidates.
        .filter(|i| crate::session_driver::successor_of(&i.session_id).is_none())
        .collect()
}

/// session id → the pending-intent token this process registered for it.
fn intent_tokens() -> &'static Mutex<HashMap<String, String>> {
    static TOKENS: std::sync::OnceLock<Mutex<HashMap<String, String>>> = std::sync::OnceLock::new();
    TOKENS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Bring this process's registered intents in line with `desired`: register the
/// new ones (and any another driver revoked while the session still qualifies),
/// withdraw the ones whose session no longer does — a retry budget spent, a
/// session the user resumed, the feature switched off.
fn sync_resume_intents(desired: &[ResumeIntent]) {
    let mut tokens = intent_tokens().lock().unwrap_or_else(|p| p.into_inner());
    tokens.retain(|sid, token| {
        let keep = desired.iter().any(|d| &d.session_id == sid);
        if !keep {
            crate::session_driver::withdraw_pending(sid, token);
        }
        keep
    });
    for d in desired {
        if let Some(t) = tokens.get(&d.session_id) {
            if crate::session_driver::pending_held(&d.session_id, t) {
                continue;
            }
        }
        let spec = crate::session_driver::PendingSpec {
            reason: d.reason.clone(),
            not_before_ms: d.not_before_ms,
            ttl_ms: RESUME_INTENT_TTL_MS,
            yield_to_turn: true,
        };
        match crate::session_driver::register_pending(
            &d.session_id,
            crate::session_driver::Driver::continue_(d.mechanism),
            spec,
        ) {
            Some(t) => {
                tokens.insert(d.session_id.clone(), t);
            }
            None => {
                tokens.remove(&d.session_id);
            }
        }
    }
}

/// Take the intent token for a session about to be fired, so the resume is not
/// blocked by its own reservation (and consumes it).
fn take_intent_token(session_id: &str) -> Option<String> {
    intent_tokens()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(session_id)
}

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
pub fn maybe_fire_auto_resume(
    sessions: &Arc<Mutex<Vec<SessionInfo>>>,
    last_fire: &Arc<Mutex<HashMap<String, Instant>>>,
    in_flight: &Arc<AtomicUsize>,
    failures: &Arc<Mutex<HashMap<String, u32>>>,
    server_errors: &Arc<Mutex<HashMap<String, u32>>>,
) {
    let config = crate::auto_resume::AutoResumeConfig::load();
    // Before any early return: a disabled feature or a full slot table still
    // has to keep the reservations truthful.
    let desired = {
        let sess = sessions.lock().unwrap();
        let se_map = server_errors.lock().unwrap();
        let fail_map = failures.lock().unwrap();
        desired_resume_intents(&sess, &config, &se_map, &fail_map)
    };
    sync_resume_intents(&desired);
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
    let usage = crate::account::latest_usage_snapshot();
    // (id, workspace, agent_source) — source is captured under the lock so the
    // tracked resume can be dispatched by source (claude vs codex) after the
    // lock is released.
    let candidates: Vec<(String, String, String)> = {
        let sess = sessions.lock().unwrap();
        let mut fire_map = last_fire.lock().unwrap();
        let fail_map = failures.lock().unwrap();
        // Drop entries older than the debounce window so the map doesn't grow
        // unboundedly for sessions that come and go.
        fire_map.retain(|_, t| t.elapsed() < debounce * 10);
        let picked = crate::auto_resume::select_resume_candidates(
            &sess,
            &config,
            now,
            usage.as_ref(),
            // Skip a session that's still debounced OR backed off after
            // repeated failures.
            |id| {
                fire_map.get(id).is_some_and(|t| t.elapsed() < debounce)
                    || crate::auto_resume::is_backed_off(&fail_map, id, AUTO_RESUME_FAILURE_BACKOFF)
            },
            slots,
        );
        for (id, _) in &picked {
            fire_map.insert(id.clone(), Instant::now());
        }
        // Attach each candidate's source from the same locked snapshot.
        picked
            .into_iter()
            .map(|(id, ws)| {
                let source = sess
                    .iter()
                    .find(|s| s.id == id)
                    .map(|s| s.agent_source.clone())
                    .unwrap_or_else(|| "claude-code".to_string());
                (id, ws, source)
            })
            .collect()
    };

    for (session_id, workspace_path, agent_source) in candidates {
        log_debug(&format!(
            "auto_resume: firing for session {} ({}) in {} (in_flight={})",
            session_id,
            agent_source,
            workspace_path,
            in_flight.load(Ordering::SeqCst)
        ));
        // Reserve a slot now; the reaper releases it when the process exits.
        in_flight.fetch_add(1, Ordering::SeqCst);
        let in_flight_done = in_flight.clone();
        let failures_done = failures.clone();
        let id_done = session_id.clone();
        let token = take_intent_token(&session_id);
        let spawn_result = crate::agent_source::resume_session_with_pending(
            &agent_source,
            &crate::agent_source::ResumeSpec {
                session_id: session_id.clone(),
                workspace_path: workspace_path.clone(),
                prompt: "continue".to_string(),
                model: None,
                effort: None,
                permission_mode: None,
                images: Vec::new(),
            },
            crate::session_driver::Driver::continue_("auto_resume"),
            token.as_deref(),
            Box::new(move |success| {
                in_flight_done.fetch_sub(1, Ordering::SeqCst);
                if let Ok(mut fail_map) = failures_done.lock() {
                    crate::auto_resume::record_resume_outcome(&mut fail_map, &id_done, success);
                }
            }),
        );
        if let Err(e) = spawn_result {
            // Spawn failed before any process exists → release the slot here
            // and record the failure, since no reaper will fire on_exit.
            in_flight.fetch_sub(1, Ordering::SeqCst);
            if let Ok(mut fail_map) = failures.lock() {
                crate::auto_resume::record_resume_outcome(&mut fail_map, &session_id, false);
            }
            log_debug(&format!("auto_resume: failed for {}: {}", session_id, e));
        }
    }

    // ── Transient server_error retries ──────────────────────────────────────
    // A ServerErrored session resumes immediately (no resets_at wait). Recompute
    // free slots — the rate-limit fires above may have consumed some — then retry
    // eligible Fleet-headless sessions, capped per error episode so a turn that
    // keeps erroring (or a server that stays down) can't re-fire forever.
    if !config.retry_server_errors {
        return;
    }
    let se_slots = AUTO_RESUME_MAX_CONCURRENT.saturating_sub(in_flight.load(Ordering::SeqCst));
    if se_slots == 0 {
        return;
    }
    let se_candidates: Vec<(String, String, String)> = {
        let sess = sessions.lock().unwrap();
        let mut fire_map = last_fire.lock().unwrap();
        let mut se_map = server_errors.lock().unwrap();
        // Reset the retry budget for any session no longer ServerErrored — a
        // successful retry (or the user resuming) ends the episode, so the next
        // error starts fresh. Also keeps the map bounded.
        let errored: std::collections::HashSet<String> = sess
            .iter()
            .filter(|s| s.status == crate::session::SessionStatus::ServerErrored)
            .map(|s| s.id.clone())
            .collect();
        se_map.retain(|id, _| errored.contains(id));
        let max_retries = config.max_server_error_retries;
        let picked = crate::auto_resume::select_server_error_retries(
            &sess,
            &config,
            // Skip a session that's debounced, already has a resume/turn running,
            // or has exhausted its per-episode retry budget.
            |id| {
                fire_map.get(id).is_some_and(|t| t.elapsed() < debounce)
                    || se_map.get(id).is_some_and(|&n| n >= max_retries)
                    || sess.iter().any(|s| s.id == id && s.proc_alive)
            },
            se_slots,
        );
        for (id, _) in &picked {
            fire_map.insert(id.clone(), Instant::now());
            *se_map.entry(id.clone()).or_insert(0) += 1;
        }
        picked
            .into_iter()
            .map(|(id, ws)| {
                let source = sess
                    .iter()
                    .find(|s| s.id == id)
                    .map(|s| s.agent_source.clone())
                    .unwrap_or_else(|| "claude-code".to_string());
                (id, ws, source)
            })
            .collect()
    };

    for (session_id, workspace_path, agent_source) in se_candidates {
        log_debug(&format!(
            "server_error_retry: firing for session {} ({}) in {} (in_flight={})",
            session_id,
            agent_source,
            workspace_path,
            in_flight.load(Ordering::SeqCst)
        ));
        in_flight.fetch_add(1, Ordering::SeqCst);
        let in_flight_done = in_flight.clone();
        let token = take_intent_token(&session_id);
        let spawn_result = crate::agent_source::resume_session_with_pending(
            &agent_source,
            &crate::agent_source::ResumeSpec {
                session_id: session_id.clone(),
                workspace_path: workspace_path.clone(),
                prompt: "continue".to_string(),
                model: None,
                effort: None,
                permission_mode: None,
                images: Vec::new(),
            },
            crate::session_driver::Driver::continue_("server_error_retry"),
            token.as_deref(),
            // The per-episode se_map cap (not the failures backoff) bounds these,
            // so the reaper only needs to release the concurrency slot.
            Box::new(move |_success| {
                in_flight_done.fetch_sub(1, Ordering::SeqCst);
            }),
        );
        if let Err(e) = spawn_result {
            in_flight.fetch_sub(1, Ordering::SeqCst);
            log_debug(&format!(
                "server_error_retry: failed for {}: {}",
                session_id, e
            ));
        }
    }
}

/// Owns the per-run bookkeeping the ticker needs. Grouped so headless [`run`]
/// can construct it once and drive one iteration at a time via [`Self::tick`],
/// which also makes the loop body unit-testable without a 30s sleep.
struct TickState {
    sessions: Arc<Mutex<Vec<SessionInfo>>>,
    // Auto-resume dedup/backoff maps (shared shape with the desktop backend).
    last_fire: Arc<Mutex<HashMap<String, Instant>>>,
    in_flight: Arc<AtomicUsize>,
    failures: Arc<Mutex<HashMap<String, u32>>>,
    server_errors: Arc<Mutex<HashMap<String, u32>>>,
    // Stall-watchdog bookkeeping (per run): interrupts already fired per session
    // (hard cap) and the last fire time (cooldown).
    stall_fired: HashMap<String, u32>,
    stall_last_fire: HashMap<String, Instant>,
}

impl TickState {
    fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(Vec::new())),
            last_fire: Arc::new(Mutex::new(HashMap::new())),
            in_flight: Arc::new(AtomicUsize::new(0)),
            failures: Arc::new(Mutex::new(HashMap::new())),
            server_errors: Arc::new(Mutex::new(HashMap::new())),
            stall_fired: HashMap::new(),
            stall_last_fire: HashMap::new(),
        }
    }

    /// Run one reconciliation pass: refresh the session list from `scan`, heal
    /// dead Codex liveness, interrupt hung Codex turns, fire auto-resumes, then
    /// drain queued follow-ups — the same order as the desktop ticker, minus
    /// the event emit (headless has no Tauri sink).
    fn tick<F: Fn() -> Vec<SessionInfo>>(&mut self, scan: &F) {
        // Refresh the shared list from a fresh scan. Headless has no fs-watcher,
        // so the ticker is the sole source of session updates.
        {
            let mut s = self.sessions.lock().unwrap();
            *s = scan();
            // Heal Codex sessions frozen at `proc_alive = true` by a turn that
            // died mid-flight without a `task_complete`. Core version, no emit.
            crate::codex_source::refresh_dead_codex_liveness(&mut s);
        }
        maybe_interrupt_stalled_codex(
            &self.sessions,
            &mut self.stall_fired,
            &mut self.stall_last_fire,
        );
        maybe_fire_auto_resume(
            &self.sessions,
            &self.last_fire,
            &self.in_flight,
            &self.failures,
            &self.server_errors,
        );
        maybe_drain_pending_messages(&self.sessions);
        // Wake a fresh session for plans nobody is responsible for any more.
        // Self-throttled and off-thread; see `plan_revive`.
        crate::plan_revive::maybe_tick_in_background();
    }
}

/// Drive the headless control-plane ticker until `running` flips to `false`.
///
/// `scan` produces a fresh `Vec<SessionInfo>` each tick — headless callers wire
/// this to the same source-scan `fleet serve` uses per request. This owns all
/// its bookkeeping (session list, auto-resume maps, stall watchdog), unlike the
/// desktop backend which shares those maps across three call sites.
///
/// Intended to be called on a dedicated thread; it sleeps [`TICK_INTERVAL`]
/// between passes and checks `running` before each.
pub fn run<F: Fn() -> Vec<SessionInfo>>(scan: F, running: Arc<AtomicBool>) {
    run_with_interval(scan, running, TICK_INTERVAL);
}

/// [`run`] with a caller-chosen interval — split out so tests can drive the
/// loop with a millisecond cadence instead of the production 30s.
fn run_with_interval<F: Fn() -> Vec<SessionInfo>>(
    scan: F,
    running: Arc<AtomicBool>,
    interval: Duration,
) {
    let mut state = TickState::new();
    loop {
        std::thread::sleep(interval);
        if !running.load(Ordering::SeqCst) {
            break;
        }
        state.tick(&scan);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize as TestCounter;

    /// One `tick` with an empty scan must not panic and must leave the shared
    /// session list holding exactly what `scan` returned. Auto-resume is
    /// disabled by default (no config on disk), so no processes are spawned.
    #[test]
    fn tick_refreshes_session_list_from_scan() {
        let _guard = isolated_fleet_home();
        let mut state = TickState::new();
        let calls = Arc::new(TestCounter::new(0));
        let calls2 = calls.clone();
        let scan = move || {
            calls2.fetch_add(1, Ordering::SeqCst);
            Vec::<SessionInfo>::new()
        };
        state.tick(&scan);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "scan called once per tick");
        assert!(
            state.sessions.lock().unwrap().is_empty(),
            "sessions reflects the fresh (empty) scan result"
        );
    }

    /// `run` must stop ticking once `running` flips false, and the pre-tick
    /// gate must skip the tick on the wake that sees the cleared flag. We let a
    /// couple of ticks happen, clear the flag, and confirm the loop terminates
    /// with a bounded scan count (no runaway, no extra tick after shutdown).
    #[test]
    fn run_stops_ticking_when_running_flag_cleared() {
        let _guard = isolated_fleet_home();
        let running = Arc::new(AtomicBool::new(true));
        let calls = Arc::new(TestCounter::new(0));
        let calls2 = calls.clone();
        let running_scan = running.clone();
        // Flip the flag false the moment the third tick runs, so the loop's
        // next wake takes the shutdown branch instead of a fourth tick.
        let scan = move || {
            if calls2.fetch_add(1, Ordering::SeqCst) + 1 >= 3 {
                running_scan.store(false, Ordering::SeqCst);
            }
            Vec::<SessionInfo>::new()
        };
        let handle = {
            let running = running.clone();
            std::thread::spawn(move || run_with_interval(scan, running, Duration::from_millis(5)))
        };
        handle.join().expect("ticker thread must not panic");
        // Exactly 3 ticks: the 3rd cleared the flag, and the 4th wake hit the
        // gate and broke before scanning again.
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(!running.load(Ordering::SeqCst));
    }

    /// Set FLEET_HOME to a fresh temp dir so `AutoResumeConfig::load()` and
    /// `latest_usage_snapshot()` read an empty (disabled) config, never the
    /// developer's real `~/.fleet`.
    ///
    /// `FLEET_HOME` is process-wide, so this repoints Fleet's home for *every*
    /// test running concurrently, not just this one. The returned handle
    /// therefore carries the process-wide lock (and restores the previous value
    /// on drop): without it these tests made unrelated assertions elsewhere read
    /// this temp home — `interaction_mode_test`'s HOME check was observed
    /// failing with this very directory on one side of the comparison.
    fn isolated_fleet_home() -> TempHome {
        let lock = crate::session::fleet_home_lock();
        let base = std::env::temp_dir().join(format!(
            "fleet-headless-test-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        let fleet = base.join(".fleet");
        std::fs::create_dir_all(&fleet).unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        std::env::set_var("FLEET_HOME", &fleet);
        TempHome {
            base,
            prev,
            _lock: lock,
        }
    }

    static NEXT_ID: TestCounter = TestCounter::new(0);

    struct TempHome {
        base: std::path::PathBuf,
        prev: Option<std::ffi::OsString>,
        /// Held for as long as the temp home is in place — dropping it is what
        /// lets the next home-repointing test run.
        _lock: std::sync::MutexGuard<'static, ()>,
    }
    impl Drop for TempHome {
        fn drop(&mut self) {
            // Restore before the lock goes: the next waiter must not observe
            // this test's home.
            match &self.prev {
                Some(v) => std::env::set_var("FLEET_HOME", v),
                None => std::env::remove_var("FLEET_HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    // ── Pending drive intents ───────────────────────────────────────────────

    fn intent_session(id: &str, status: crate::session::SessionStatus) -> SessionInfo {
        let mut s = crate::session::test_session(id);
        s.workspace_path = std::env::temp_dir().to_string_lossy().into_owned();
        s.status = status;
        s
    }

    fn rate_limited(id: &str) -> SessionInfo {
        let mut s = intent_session(id, crate::session::SessionStatus::RateLimited);
        let now = chrono::Utc::now();
        s.rate_limit = Some(crate::session::RateLimitState {
            resets_at: now + chrono::Duration::hours(2),
            limit_type: crate::rate_limit_parser::RateLimitType::SessionLimit,
            parsed: true,
            error_timestamp: now,
        });
        s
    }

    #[test]
    fn desired_intents_cover_waiting_rate_limits_and_retries_with_budget() {
        let cfg = crate::auto_resume::AutoResumeConfig::default();
        let mut alive = intent_session("alive", crate::session::SessionStatus::ServerErrored);
        alive.proc_alive = true;
        let sessions = vec![
            rate_limited("rl"),
            intent_session("se", crate::session::SessionStatus::ServerErrored),
            intent_session("spent", crate::session::SessionStatus::ServerErrored),
            intent_session("idle", crate::session::SessionStatus::Idle),
            alive,
        ];
        let se_map = HashMap::from([("spent".to_string(), cfg.max_server_error_retries)]);
        let got = desired_resume_intents(&sessions, &cfg, &se_map, &HashMap::new());
        let ids: Vec<_> = got.iter().map(|i| (i.session_id.as_str(), i.mechanism)).collect();
        assert_eq!(ids, [("rl", "auto_resume"), ("se", "server_error_retry")]);
        assert!(got[0].not_before_ms.is_some(), "rate-limit intent names its reset time");
    }

    #[test]
    fn desired_intents_are_empty_when_the_feature_is_off() {
        let cfg = crate::auto_resume::AutoResumeConfig { enabled: false, ..Default::default() };
        let sessions = vec![rate_limited("rl"), intent_session("se", crate::session::SessionStatus::ServerErrored)];
        assert!(desired_resume_intents(&sessions, &cfg, &HashMap::new(), &HashMap::new()).is_empty());
    }

    #[test]
    fn sync_registers_then_withdraws_and_blocks_takeover_meanwhile() {
        let _guard = isolated_fleet_home();
        let sid = format!("sync-{}", uuid::Uuid::new_v4());
        let want = vec![ResumeIntent {
            session_id: sid.clone(),
            mechanism: "server_error_retry",
            reason: "retry after a server error".into(),
            not_before_ms: None,
        }];
        sync_resume_intents(&want);
        let reserved = crate::session_driver::snapshot(&sid);
        assert_eq!(reserved.pending.len(), 1);
        assert!(
            crate::session_driver::acquire_takeover(&sid, crate::session_driver::Driver::takeover("plan_revive"))
                .is_err(),
            "a pending retry keeps the reviver off the session"
        );
        // Idempotent: a second tick does not stack a second intent.
        sync_resume_intents(&want);
        assert_eq!(crate::session_driver::snapshot(&sid).pending.len(), 1);
        // Budget spent / session recovered: the intent is withdrawn.
        sync_resume_intents(&[]);
        assert!(crate::session_driver::snapshot(&sid).pending.is_empty());
        assert!(!intent_tokens().lock().unwrap().contains_key(&sid));
    }

    #[test]
    fn semgap_recount_retry_pending_keeps_the_reviver_off_the_plan() {
        // 2026-09-25 semgap-recount: predecessor 5f2a53c7 died of ENOTFOUND at
        // 08:47:51Z (a DarkWake), the Mac slept, and on waking the reviver
        // started successor a1520529 at 09:52:31Z; 29 s later the server-error
        // retry resumed the predecessor and both drove the same plan. The retry
        // was in budget the whole hour, so its intent stood before the sleep.
        let _guard = isolated_fleet_home();
        let sid = format!("semgap-{}", uuid::Uuid::new_v4());
        sync_resume_intents(&[ResumeIntent {
            session_id: sid.clone(),
            mechanism: "server_error_retry",
            reason: "getaddrinfo ENOTFOUND api.anthropic.com".into(),
            not_before_ms: None,
        }]);

        // The reviver's tick: the plan's only owner counts as covered.
        let owners = std::collections::HashSet::from([sid.clone()]);
        let owner_list = vec![sid.clone()];
        let coverage = crate::plan_revive::gather_coverage(&owners);
        assert_eq!(
            coverage.covering(&owner_list).map(|(_, s)| s),
            Some(crate::plan_revive::AttendanceState::Reserved)
        );
        assert!(coverage.reason(&owner_list).is_some(), "covered, so no successor");
        // Even a reviver that skipped the check is refused at the spawn.
        assert!(crate::session_driver::acquire_takeover(
            &sid,
            crate::session_driver::Driver::takeover("plan_revive")
        )
        .is_err());

        // The retry itself still goes ahead, and the plan is free again after.
        let token = take_intent_token(&sid).expect("registered");
        let lease = crate::session_driver::acquire(
            &sid,
            crate::session_driver::Driver::continue_("server_error_retry"),
            Some(&token),
        )
        .expect("the retry is not blocked by its own intent");
        drop(lease);
        assert_eq!(crate::session_driver::reservation(&sid), None);
    }

    #[test]
    fn firing_consumes_the_own_intent() {
        let _guard = isolated_fleet_home();
        let sid = format!("fire-{}", uuid::Uuid::new_v4());
        sync_resume_intents(&[ResumeIntent {
            session_id: sid.clone(),
            mechanism: "auto_resume",
            reason: "r".into(),
            not_before_ms: None,
        }]);
        let token = take_intent_token(&sid).expect("registered");
        let lease = crate::session_driver::acquire(
            &sid,
            crate::session_driver::Driver::continue_("auto_resume"),
            Some(&token),
        )
        .expect("own intent does not block its holder");
        assert!(crate::session_driver::snapshot(&sid).pending.is_empty());
        drop(lease);
    }
}
