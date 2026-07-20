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
                    || crate::auto_resume::is_backed_off(
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
        let spawn_result = crate::agent_source::resume_session(
            &agent_source,
            &crate::agent_source::ResumeSpec {
                session_id: session_id.clone(),
                workspace_path: workspace_path.clone(),
                prompt: "continue".to_string(),
                model: None,
                effort: None,
                permission_mode: None,
            },
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
        let spawn_result = crate::agent_source::resume_session(
            &agent_source,
            &crate::agent_source::ResumeSpec {
                session_id: session_id.clone(),
                workspace_path: workspace_path.clone(),
                prompt: "continue".to_string(),
                model: None,
                effort: None,
                permission_mode: None,
            },
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
            std::thread::spawn(move || {
                run_with_interval(scan, running, Duration::from_millis(5))
            })
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
    fn isolated_fleet_home() -> TempHome {
        let base = std::env::temp_dir().join(format!(
            "fleet-headless-test-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::SeqCst)
        ));
        let fleet = base.join(".fleet");
        std::fs::create_dir_all(&fleet).unwrap();
        std::env::set_var("FLEET_HOME", &fleet);
        TempHome { base }
    }

    static NEXT_ID: TestCounter = TestCounter::new(0);

    struct TempHome {
        base: std::path::PathBuf,
    }
    impl Drop for TempHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }
}
