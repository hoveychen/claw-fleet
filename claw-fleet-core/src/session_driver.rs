//! Who may drive a session right now — the drive lease.
//!
//! Fleet has more than a dozen mechanisms that resume a session or start a
//! successor for it (rate-limit auto-resume, server-error retry, queued-message
//! drain, plan revive, finish-button continue, watch, parked-card answers, …).
//! Before this module each one decided on its own whether the session was free,
//! and the only shared signal was "is there a process whose argv names this
//! session" ([`crate::parked::session_alive`]). That signal has two blind spots:
//!
//! - **A: dead but about to be brought back.** A server-error retry waiting to
//!   fire, a watch armed on a timer, a card waiting for an answer. None of these
//!   show up in the process table. Observed 5/43 revives on 2026-09-23..30: the
//!   reviver started a successor, then `headless_runtime`'s retry resumed the
//!   predecessor 30 s later and both ran the same plan.
//! - **B: just spawned, argv not visible yet** — the first seconds of a resume.
//!
//! The lease closes both. State lives in `~/.fleet/drive/<session>.json`:
//!
//! - a **Running** lease, taken by the dispatcher
//!   ([`crate::agent_source::resume_session`]) *before* the process starts and
//!   released when it exits. For the first [`SPAWN_GRACE_MS`] it counts as live
//!   on its own (blind spot B); after that it is live only while the session's
//!   process can still be found, so a lease leaked by a crashed holder heals
//!   itself instead of wedging the session.
//! - any number of **Pending** intents — "I will bring this session back later"
//!   (blind spot A). Each names its holder process; an intent whose holder died
//!   is dropped, because a timer that died with its process will never fire.
//!
//! Arbitration ([`decide`]) follows the priority agreed in
//! `arch/session-driver-unify`: manual > answer > same-session continue >
//! takeover (a new session replacing this one). A granted turn revokes the
//! intents that only wanted to restart the turn (`yield_to_turn`), since the new
//! turn already does that.

use serde::{Deserialize, Serialize};

/// How long a fresh Running lease counts as live without its process being
/// visible. Covers the gap between `spawn()` and the child's argv appearing in
/// the process table, with a wide margin for a loaded machine.
pub const SPAWN_GRACE_MS: u64 = 90_000;

/// Hard ceiling on any Running lease. Liveness is normally decided by the
/// process table; this only bounds a record nobody ever released.
pub const RUNNING_MAX_MS: u64 = 24 * 3600 * 1000;

/// Lifetime of the hold a takeover leaves on the session it replaces, so a
/// second takeover arriving seconds later (reviver vs finish-button) is refused.
pub const TAKEOVER_HOLD_MS: u64 = 180_000;

/// Priority class of whoever wants to drive a session. Ordered: a higher class
/// is never blocked by a lower class's pending intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriverClass {
    /// Start a *new* session that replaces this one (plan revive, finish-button
    /// continue). Lowest: only when nothing else wants the session.
    Takeover,
    /// Bring the same session back to finish its turn (server-error retry,
    /// rate-limit resume, watch fire).
    Continue,
    /// Deliver an answer the session is waiting for (parked card, turn card,
    /// queued message).
    Answer,
    /// The user explicitly asked for it.
    Manual,
}

/// A driver: its class plus the mechanism name that shows up in logs and in
/// refusal messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Driver {
    pub class: DriverClass,
    pub mechanism: &'static str,
}

impl Driver {
    pub const fn manual(mechanism: &'static str) -> Self {
        Self { class: DriverClass::Manual, mechanism }
    }
    pub const fn answer(mechanism: &'static str) -> Self {
        Self { class: DriverClass::Answer, mechanism }
    }
    pub const fn continue_(mechanism: &'static str) -> Self {
        Self { class: DriverClass::Continue, mechanism }
    }
    pub const fn takeover(mechanism: &'static str) -> Self {
        Self { class: DriverClass::Takeover, mechanism }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningLease {
    pub token: String,
    pub class: DriverClass,
    pub mechanism: String,
    pub holder_pid: u32,
    pub acquired_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingIntent {
    pub token: String,
    pub class: DriverClass,
    pub mechanism: String,
    pub reason: String,
    /// Process that will act on the intent. `0` = no holder process (a takeover
    /// hold): the intent lives until `expires_at_ms` regardless.
    pub holder_pid: u32,
    /// Start time of `holder_pid`, so a recycled pid cannot keep a dead
    /// holder's intent alive.
    #[serde(default)]
    pub holder_start_time: u64,
    pub registered_at_ms: u64,
    /// Earliest time the holder plans to act; informational (shown to the
    /// reviver and the UI).
    #[serde(default)]
    pub not_before_ms: Option<u64>,
    pub expires_at_ms: u64,
    /// `true` when all the intent wants is to get the turn going again (retry,
    /// rate-limit resume): any turn granted to someone else supersedes it.
    #[serde(default)]
    pub yield_to_turn: bool,
}

/// The on-disk record for one session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveState {
    #[serde(default)]
    pub running: Option<RunningLease>,
    #[serde(default)]
    pub pending: Vec<PendingIntent>,
}

impl DriveState {
    fn is_empty(&self) -> bool {
        self.running.is_none() && self.pending.is_empty()
    }
}

/// Why a driver was turned away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Another driver's turn is in flight.
    Running { mechanism: String },
    /// The session's process is running without a lease (an interactive
    /// session, or one started before leases existed).
    ProcessAlive,
    /// A higher-priority driver has said it will bring the session back.
    Pending { mechanism: String, reason: String },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Running { mechanism } => write!(f, "session is already being driven by {mechanism}"),
            Refusal::ProcessAlive => write!(f, "session process is still running"),
            Refusal::Pending { mechanism, reason } => {
                write!(f, "session is reserved by {mechanism} ({reason})")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Go ahead. `revoke` lists the pending tokens the new turn supersedes.
    Grant { revoke: Vec<String> },
    Refuse(Refusal),
}

/// The arbitration table, pure so every row is testable without a process
/// table or a filesystem.
///
/// - `running_live`: whether `state.running` still holds (see [`running_live`]).
/// - `session_alive`: whether the session's process is visible right now.
/// - `own_pending`: the caller's own intent token, when it is the holder of a
///   pending intent now turning into a turn — it never blocks itself.
///
/// `state.pending` must already be pruned of expired / orphaned intents.
pub fn decide(
    state: &DriveState,
    running_live: bool,
    session_alive: bool,
    driver: Driver,
    own_pending: Option<&str>,
) -> Decision {
    if running_live {
        if let Some(r) = &state.running {
            return Decision::Refuse(Refusal::Running { mechanism: r.mechanism.clone() });
        }
    }
    if session_alive {
        return Decision::Refuse(Refusal::ProcessAlive);
    }
    let others = state
        .pending
        .iter()
        .filter(|p| Some(p.token.as_str()) != own_pending);
    // A takeover is the last resort: any intent at all means someone still
    // plans to bring this session back, so replacing it would double-drive the
    // plan. Everyone else is blocked only by a strictly higher class.
    let blocker = others.clone().find(|p| {
        driver.class == DriverClass::Takeover || p.class > driver.class
    });
    if let Some(p) = blocker {
        return Decision::Refuse(Refusal::Pending {
            mechanism: p.mechanism.clone(),
            reason: p.reason.clone(),
        });
    }
    let revoke = if driver.class == DriverClass::Takeover {
        Vec::new()
    } else {
        others.filter(|p| p.yield_to_turn).map(|p| p.token.clone()).collect()
    };
    Decision::Grant { revoke }
}

/// Is a Running lease still holding, given its age and whether the session's
/// process is visible?
pub fn running_live(lease: &RunningLease, now_ms: u64, session_alive: bool) -> bool {
    let age = now_ms.saturating_sub(lease.acquired_at_ms);
    if age >= RUNNING_MAX_MS {
        return false;
    }
    age < SPAWN_GRACE_MS || session_alive
}

/// Drop intents that expired or whose holder process is gone.
pub fn prune_pending(state: &mut DriveState, now_ms: u64, holder_alive: &dyn Fn(u32, u64) -> bool) {
    state.pending.retain(|p| {
        p.expires_at_ms > now_ms && (p.holder_pid == 0 || holder_alive(p.holder_pid, p.holder_start_time))
    });
}

// ── On-disk store ────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn new_token() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn drive_dir() -> Option<std::path::PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("drive"))
}

/// `~/.fleet/drive/<session>.json`. Session ids are uuids / Codex thread ids,
/// but dsh ids can carry separators, so anything outside a safe set is mapped
/// to `_`.
fn state_path(session_id: &str) -> Option<std::path::PathBuf> {
    let safe: String = session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect();
    if safe.is_empty() || safe.chars().all(|c| c == '.') {
        return None;
    }
    drive_dir().map(|d| d.join(format!("{safe}.json")))
}

fn load(path: &std::path::Path) -> DriveState {
    match crate::atomic_json::load_preserving::<DriveState>(path) {
        crate::atomic_json::JsonLoad::Loaded(s) => s,
        _ => DriveState::default(),
    }
}

fn store(path: &std::path::Path, state: &DriveState) {
    if state.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    match serde_json::to_vec_pretty(state) {
        Ok(bytes) => {
            if let Err(e) = crate::atomic_json::write_atomic(path, &bytes) {
                crate::log_debug(&format!("[drive] write {}: {e}", path.display()));
            }
        }
        Err(e) => crate::log_debug(&format!("[drive] serialize: {e}")),
    }
}

fn holder_alive(pid: u32, start_time: u64) -> bool {
    crate::session::is_process_alive(pid)
        && (start_time == 0 || crate::session::process_start_time(pid) == Some(start_time))
}

/// Liveness probes, injectable so tests do not depend on the process table.
struct Probes<'a> {
    session_alive: &'a dyn Fn(&str) -> bool,
    holder_alive: &'a dyn Fn(u32, u64) -> bool,
    /// Intents implied by other stores (see [`derived_pending`]). Merged into
    /// every arbitration and snapshot, never written to the drive store.
    derived: &'a dyn Fn(&str) -> Vec<PendingIntent>,
}

fn real_probes() -> Probes<'static> {
    Probes {
        session_alive: &crate::parked::session_alive,
        holder_alive: &holder_alive,
        derived: &derived_pending,
    }
}

/// Token prefix of a derived intent. It is not in the store, so no holder can
/// withdraw it and no turn can revoke it: it goes away with its source record.
pub const DERIVED_TOKEN_PREFIX: &str = "derived:";

fn derived_intent(class: DriverClass, mechanism: &str, source_id: &str, reason: String) -> PendingIntent {
    PendingIntent {
        token: format!("{DERIVED_TOKEN_PREFIX}{mechanism}:{source_id}"),
        class,
        mechanism: mechanism.to_string(),
        reason,
        holder_pid: 0,
        holder_start_time: 0,
        registered_at_ms: 0,
        not_before_ms: None,
        expires_at_ms: u64::MAX,
        yield_to_turn: false,
    }
}

/// Intents that already exist as records elsewhere, read at decision time
/// instead of being registered a second time (two copies would drift):
///
/// - a **parked card** means the session is waiting for the user's answer —
///   Answer class, so a continue resume cannot answer it with nothing;
/// - a **queued message** will be delivered by the drain — Continue class: the
///   drain already skips sessions a retry or a rate-limit wait is holding, so an
///   Answer-class queue would deadlock with the retry;
/// - a **live watch** will resume the session when it fires — Continue class.
///
/// Each of the three blocks a takeover, which is the gap this closes.
fn derived_pending(session_id: &str) -> Vec<PendingIntent> {
    let now = now_ms();
    let mut out = Vec::new();
    for c in crate::parked::list().into_iter().filter(|c| c.session_id == session_id) {
        out.push(derived_intent(DriverClass::Answer, "parked_card", &c.id, "a card is waiting for an answer".into()));
    }
    if crate::pending_message::has_pending(session_id) {
        out.push(derived_intent(DriverClass::Continue, "pending_message", session_id, "a message is queued".into()));
    }
    for w in crate::watch::list().into_iter().filter(|w| w.session_id == session_id && w.is_live(now)) {
        out.push(derived_intent(DriverClass::Continue, "watch", &w.id, format!("watch {} is armed", w.id)));
    }
    out
}

/// `state` with the derived intents appended — the view [`decide`] sees.
fn with_derived(state: &DriveState, derived: &[PendingIntent]) -> DriveState {
    let mut view = state.clone();
    view.pending.extend(derived.iter().cloned());
    view
}

/// Read-modify-write one session's record under its cross-process lock, with
/// expired / orphaned intents pruned before `f` sees it.
fn with_state<R>(
    session_id: &str,
    probes: &Probes<'_>,
    f: impl FnOnce(&mut DriveState, u64) -> R,
) -> Option<R> {
    let path = state_path(session_id)?;
    // One lock for the whole store rather than a `<session>.json.lock` per
    // session: every critical section is a tiny read-modify-write, and
    // per-session lock files would pile up forever.
    let lock_anchor = path.with_file_name("store");
    Some(crate::atomic_json::with_file_lock(&lock_anchor, || {
        let loaded = load(&path);
        let mut state = loaded.clone();
        let now = now_ms();
        prune_pending(&mut state, now, probes.holder_alive);
        // A lease whose holder exited without releasing it — the watch timer
        // always does, it quits right after firing — is dropped once it is past
        // the spawn grace and its process is gone. The process probe only runs
        // for such a candidate.
        if let Some(r) = &state.running {
            let past_grace = now.saturating_sub(r.acquired_at_ms) >= SPAWN_GRACE_MS;
            if past_grace && !running_live(r, now, (probes.session_alive)(session_id)) {
                state.running = None;
            }
        }
        let out = f(&mut state, now);
        if state != loaded {
            store(&path, &state);
        }
        out
    }))
}

/// A held Running lease. Released on drop, so a resume whose `on_exit` closure
/// is dropped without being called (the source returned an error) still frees
/// the session.
#[derive(Debug)]
pub struct Lease {
    session_id: String,
    token: Option<String>,
}

impl Lease {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// A lease that records nothing — for a session id the store cannot name.
    fn untracked(session_id: &str) -> Self {
        Self { session_id: session_id.to_string(), token: None }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let Some(token) = self.token.take() else { return };
        let probes = real_probes();
        with_state(&self.session_id, &probes, |state, _| {
            if state.running.as_ref().map(|r| r.token == token).unwrap_or(false) {
                state.running = None;
            }
        });
    }
}

/// Ask to drive `session_id` now. On success the caller holds a Running lease
/// until the returned [`Lease`] drops — move it into the process's exit
/// callback. `own_pending` is the caller's own intent token when it is the
/// mechanism that registered one; it is consumed on success.
pub fn acquire(session_id: &str, driver: Driver, own_pending: Option<&str>) -> Result<Lease, Refusal> {
    sweep_throttled();
    acquire_with(session_id, driver, own_pending, &real_probes())
}

/// Minimum gap between two whole-store sweeps in one process.
const SWEEP_EVERY_MS: u64 = 10 * 60 * 1000;

/// Drop stale records across the whole store, at most once per
/// [`SWEEP_EVERY_MS`] per process. Without it a record left by an exited
/// holder would sit on disk until that one session was driven again.
fn sweep_throttled() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = now_ms();
    let last = LAST.load(Ordering::Relaxed);
    if now.saturating_sub(last) < SWEEP_EVERY_MS
        || LAST.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_err()
    {
        return;
    }
    sweep_with(&real_probes());
}

fn sweep_with(probes: &Probes<'_>) {
    let Some(dir) = drive_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".json")) else { continue };
        // The file stem is the (sanitised) session id; `with_state` prunes and
        // removes the file once nothing is left in it.
        with_state(stem, probes, |_, _| ());
    }
}

fn acquire_with(
    session_id: &str,
    driver: Driver,
    own_pending: Option<&str>,
    probes: &Probes<'_>,
) -> Result<Lease, Refusal> {
    // Read outside the store lock: the sources have their own files.
    let derived = (probes.derived)(session_id);
    let outcome = with_state(session_id, probes, |state, now| {
        let alive = (probes.session_alive)(session_id);
        let live = state.running.as_ref().map(|r| running_live(r, now, alive)).unwrap_or(false);
        if !live {
            state.running = None;
        }
        match decide(&with_derived(state, &derived), live, alive, driver, own_pending) {
            Decision::Refuse(r) => Err(r),
            Decision::Grant { revoke } => {
                state.pending.retain(|p| {
                    !revoke.contains(&p.token) && Some(p.token.as_str()) != own_pending
                });
                let token = new_token();
                state.running = Some(RunningLease {
                    token: token.clone(),
                    class: driver.class,
                    mechanism: driver.mechanism.to_string(),
                    holder_pid: std::process::id(),
                    acquired_at_ms: now,
                });
                Ok((token, revoke))
            }
        }
    });
    match outcome {
        None => Ok(Lease::untracked(session_id)),
        Some(Ok((token, revoke))) => {
            if !revoke.is_empty() {
                crate::log_debug(&format!(
                    "[drive] {session_id}: {} supersedes {} pending intent(s)",
                    driver.mechanism,
                    revoke.len()
                ));
            }
            Ok(Lease { session_id: session_id.to_string(), token: Some(token) })
        }
        Some(Err(r)) => {
            crate::log_debug(&format!("[drive] {session_id}: refused {}: {r}", driver.mechanism));
            Err(r)
        }
    }
}

/// Check whether a takeover of `session_id` may go ahead, and if so leave a
/// short hold on it so a second takeover arriving moments later is refused.
/// The replaced session itself is not leased — a manual or answer turn on it
/// stays possible.
pub fn acquire_takeover(session_id: &str, driver: Driver) -> Result<(), Refusal> {
    acquire_takeover_with(session_id, driver, &real_probes())
}

fn acquire_takeover_with(session_id: &str, driver: Driver, probes: &Probes<'_>) -> Result<(), Refusal> {
    debug_assert_eq!(driver.class, DriverClass::Takeover);
    let derived = (probes.derived)(session_id);
    let outcome = with_state(session_id, probes, |state, now| {
        let alive = (probes.session_alive)(session_id);
        let live = state.running.as_ref().map(|r| running_live(r, now, alive)).unwrap_or(false);
        if !live {
            state.running = None;
        }
        match decide(&with_derived(state, &derived), live, alive, driver, None) {
            Decision::Refuse(r) => Err(r),
            Decision::Grant { .. } => {
                state.pending.push(PendingIntent {
                    token: new_token(),
                    class: DriverClass::Takeover,
                    mechanism: driver.mechanism.to_string(),
                    reason: "replaced by a new session".into(),
                    holder_pid: 0,
                    holder_start_time: 0,
                    registered_at_ms: now,
                    not_before_ms: None,
                    expires_at_ms: now + TAKEOVER_HOLD_MS,
                    yield_to_turn: false,
                });
                Ok(())
            }
        }
    });
    match outcome {
        None | Some(Ok(())) => Ok(()),
        Some(Err(r)) => {
            crate::log_debug(&format!("[drive] {session_id}: refused takeover by {}: {r}", driver.mechanism));
            Err(r)
        }
    }
}

/// Parameters of a pending intent.
#[derive(Debug, Clone)]
pub struct PendingSpec {
    pub reason: String,
    pub not_before_ms: Option<u64>,
    /// How long the intent may stand before it is presumed abandoned.
    pub ttl_ms: u64,
    pub yield_to_turn: bool,
}

/// Record "this process will bring `session_id` back later". Returns the token
/// the holder later passes to [`acquire`] (as `own_pending`), [`pending_held`]
/// or [`withdraw_pending`]; `None` when the store cannot name the session.
pub fn register_pending(session_id: &str, driver: Driver, spec: PendingSpec) -> Option<String> {
    let probes = real_probes();
    let pid = std::process::id();
    let start = crate::session::process_start_time(pid).unwrap_or(0);
    with_state(session_id, &probes, |state, now| {
        let token = new_token();
        state.pending.push(PendingIntent {
            token: token.clone(),
            class: driver.class,
            mechanism: driver.mechanism.to_string(),
            reason: spec.reason,
            holder_pid: pid,
            holder_start_time: start,
            registered_at_ms: now,
            not_before_ms: spec.not_before_ms,
            expires_at_ms: now.saturating_add(spec.ttl_ms),
            yield_to_turn: spec.yield_to_turn,
        });
        token
    })
}

/// Whether the intent `token` still stands — `false` once another turn
/// superseded it or it expired. Holders check this before acting.
pub fn pending_held(session_id: &str, token: &str) -> bool {
    let probes = real_probes();
    with_state(session_id, &probes, |state, _| state.pending.iter().any(|p| p.token == token))
        .unwrap_or(true)
}

/// Drop an intent the holder no longer plans to act on.
pub fn withdraw_pending(session_id: &str, token: &str) {
    let probes = real_probes();
    with_state(session_id, &probes, |state, _| state.pending.retain(|p| p.token != token));
}

/// The session's current record, pruned, with the derived intents included
/// (read-only view for the reviver and the UI). A Running lease is reported as
/// recorded; use [`running_live`] to judge it.
pub fn snapshot(session_id: &str) -> DriveState {
    snapshot_with(session_id, &real_probes())
}

/// Who has claimed `session_id` in the drive store — a Running lease still
/// inside its spawn grace or with its process up, or any registered intent
/// (retry, rate-limit wait, takeover hold) — as `"<mechanism>: <reason>"`.
/// Derived intents are left out: callers that scan whole stores (the reviver)
/// already read their sources once for every session.
pub fn reservation(session_id: &str) -> Option<String> {
    reservation_with(session_id, &real_probes())
}

fn reservation_with(session_id: &str, probes: &Probes<'_>) -> Option<String> {
    with_state(session_id, probes, |state, _| {
        // `with_state` has already dropped a Running lease that stopped holding.
        if let Some(r) = &state.running {
            return Some(format!("{}: turn in flight", r.mechanism));
        }
        state.pending.first().map(|p| format!("{}: {}", p.mechanism, p.reason))
    })
    .flatten()
}

fn snapshot_with(session_id: &str, probes: &Probes<'_>) -> DriveState {
    let derived = (probes.derived)(session_id);
    with_state(session_id, probes, |state, _| with_derived(state, &derived)).unwrap_or_default()
}

// ── Retired sessions ─────────────────────────────────────────────────────────

/// A session some other session took the work over from, outside the handoff
/// chain (which records its own links): a plan revive's successor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeoverRecord {
    pub successor: String,
    pub mechanism: String,
    pub at_ms: u64,
}

fn retired_path(session_id: &str) -> Option<std::path::PathBuf> {
    let file = state_path(session_id)?.file_name()?.to_owned();
    crate::session::get_fleet_dir().map(|d| d.join("retired").join(file))
}

/// Record that `successor` took the work over from `replaced`. From then on
/// automatic drivers leave `replaced` alone (see [`live_end`]).
pub fn record_takeover(replaced: &str, successor: &str, mechanism: &str) {
    let Some(path) = retired_path(replaced) else { return };
    let rec = TakeoverRecord { successor: successor.to_string(), mechanism: mechanism.to_string(), at_ms: now_ms() };
    let written = serde_json::to_vec_pretty(&rec)
        .map_err(|e| e.to_string())
        .and_then(|b| crate::atomic_json::write_atomic(&path, &b).map_err(|e| e.to_string()));
    if let Err(e) = written {
        crate::log_debug(&format!("[drive] record takeover of {replaced}: {e}"));
    }
}

fn takeover_successor(session_id: &str) -> Option<String> {
    let path = retired_path(session_id)?;
    let rec: TakeoverRecord = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    Some(rec.successor)
}

/// The session that took over from `session_id` — its handoff successor, or
/// the session a plan revive started in its place — or `None` when it was never
/// replaced.
pub fn successor_of(session_id: &str) -> Option<String> {
    crate::handoff::successor_session_of(session_id).or_else(|| takeover_successor(session_id))
}

/// The live end of `session_id`'s succession (handoffs and takeovers mixed),
/// or `None` when `session_id` itself is still the one doing the work.
pub fn live_end(session_id: &str) -> Option<String> {
    live_end_with(session_id, &successor_of)
}

fn live_end_with(session_id: &str, next: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let mut current = next(session_id)?;
    // Successions are short and acyclic; the bound only keeps a corrupted
    // store from spinning forever.
    for _ in 0..64 {
        match next(&current) {
            Some(n) if n != session_id => current = n,
            _ => break,
        }
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_end_follows_mixed_successions_and_survives_a_cycle() {
        let links = |s: &str| match s {
            "a" => Some("b".to_string()),
            "b" => Some("c".to_string()),
            "x" => Some("y".to_string()),
            "y" => Some("x".to_string()),
            _ => None,
        };
        assert_eq!(live_end_with("a", &links).as_deref(), Some("c"));
        assert_eq!(live_end_with("c", &links), None);
        assert_eq!(live_end_with("x", &links).as_deref(), Some("y"));
    }

    #[test]
    fn a_recorded_takeover_retires_the_replaced_session() {
        let _home = temp_home();
        assert_eq!(takeover_successor("old"), None);
        record_takeover("old", "new", "plan_revive");
        assert_eq!(takeover_successor("old").as_deref(), Some("new"));
        assert_eq!(takeover_successor("new"), None);
    }

    fn pending(token: &str, class: DriverClass, yield_to_turn: bool) -> PendingIntent {
        PendingIntent {
            token: token.into(),
            class,
            mechanism: format!("{class:?}"),
            reason: "r".into(),
            holder_pid: 1,
            holder_start_time: 0,
            registered_at_ms: 0,
            not_before_ms: None,
            expires_at_ms: u64::MAX,
            yield_to_turn,
        }
    }

    fn running(mech: &str, at: u64) -> RunningLease {
        RunningLease {
            token: "run".into(),
            class: DriverClass::Continue,
            mechanism: mech.into(),
            holder_pid: 1,
            acquired_at_ms: at,
        }
    }

    const RETRY: Driver = Driver::continue_("server_error_retry");
    const REVIVE: Driver = Driver::takeover("plan_revive");

    // ── decide ──────────────────────────────────────────────────────────────

    #[test]
    fn free_session_is_granted_to_anyone() {
        let s = DriveState::default();
        for d in [Driver::manual("m"), Driver::answer("a"), RETRY, REVIVE] {
            assert_eq!(decide(&s, false, false, d, None), Decision::Grant { revoke: vec![] });
        }
    }

    #[test]
    fn live_running_lease_refuses_everyone_including_manual() {
        let s = DriveState { running: Some(running("watch", 0)), pending: vec![] };
        for d in [Driver::manual("m"), Driver::answer("a"), RETRY, REVIVE] {
            assert!(matches!(decide(&s, true, false, d, None), Decision::Refuse(Refusal::Running { .. })));
        }
    }

    #[test]
    fn process_without_lease_refuses() {
        let s = DriveState::default();
        assert_eq!(
            decide(&s, false, true, Driver::manual("m"), None),
            Decision::Refuse(Refusal::ProcessAlive)
        );
    }

    #[test]
    fn revive_is_blocked_by_a_pending_retry() {
        // The semgap-recount collision (2026-09-25): a retry was about to bring
        // the predecessor back when the reviver started a successor.
        let s = DriveState { running: None, pending: vec![pending("t", DriverClass::Continue, true)] };
        assert!(matches!(decide(&s, false, false, REVIVE, None), Decision::Refuse(Refusal::Pending { .. })));
    }

    #[test]
    fn revive_is_blocked_by_another_takeover_hold() {
        let s = DriveState { running: None, pending: vec![pending("h", DriverClass::Takeover, false)] };
        assert!(matches!(decide(&s, false, false, REVIVE, None), Decision::Refuse(Refusal::Pending { .. })));
    }

    #[test]
    fn manual_preempts_and_revokes_turn_restart_intents_only() {
        let s = DriveState {
            running: None,
            pending: vec![
                pending("retry", DriverClass::Continue, true),
                pending("watch", DriverClass::Continue, false),
                pending("card", DriverClass::Answer, false),
            ],
        };
        assert_eq!(
            decide(&s, false, false, Driver::manual("m"), None),
            Decision::Grant { revoke: vec!["retry".into()] }
        );
    }

    #[test]
    fn continue_is_blocked_by_a_pending_answer() {
        // A card is waiting for the user; a "continue" resume would answer it
        // with nothing.
        let s = DriveState { running: None, pending: vec![pending("card", DriverClass::Answer, false)] };
        assert!(matches!(decide(&s, false, false, RETRY, None), Decision::Refuse(Refusal::Pending { .. })));
    }

    #[test]
    fn equal_class_intents_do_not_block_each_other() {
        // An armed watch must not stop a server-error retry of the same turn.
        let s = DriveState { running: None, pending: vec![pending("watch", DriverClass::Continue, false)] };
        assert_eq!(decide(&s, false, false, RETRY, None), Decision::Grant { revoke: vec![] });
    }

    #[test]
    fn own_pending_never_blocks_or_is_revoked_by_its_holder() {
        let s = DriveState { running: None, pending: vec![pending("mine", DriverClass::Answer, true)] };
        assert_eq!(
            decide(&s, false, false, Driver::continue_("x"), Some("mine")),
            Decision::Grant { revoke: vec![] }
        );
    }

    // ── liveness ────────────────────────────────────────────────────────────

    #[test]
    fn running_lease_is_live_during_spawn_grace_without_a_process() {
        let r = running("m", 1_000);
        assert!(running_live(&r, 1_000 + SPAWN_GRACE_MS - 1, false));
        assert!(!running_live(&r, 1_000 + SPAWN_GRACE_MS, false));
    }

    #[test]
    fn running_lease_follows_the_process_after_grace() {
        let r = running("m", 0);
        assert!(running_live(&r, SPAWN_GRACE_MS * 10, true));
        assert!(!running_live(&r, RUNNING_MAX_MS, true), "hard ceiling");
    }

    #[test]
    fn prune_drops_expired_and_orphaned_intents() {
        let mut s = DriveState {
            running: None,
            pending: vec![
                PendingIntent { expires_at_ms: 10, ..pending("expired", DriverClass::Continue, true) },
                PendingIntent { holder_pid: 7, ..pending("orphan", DriverClass::Continue, true) },
                PendingIntent { holder_pid: 0, ..pending("hold", DriverClass::Takeover, false) },
                pending("ok", DriverClass::Continue, true),
            ],
        };
        prune_pending(&mut s, 100, &|pid, _| pid != 7);
        let left: Vec<_> = s.pending.iter().map(|p| p.token.as_str()).collect();
        assert_eq!(left, ["hold", "ok"]);
    }

    // ── store ───────────────────────────────────────────────────────────────

    fn temp_home() -> crate::paths::FleetHomeGuard {
        crate::paths::fleet_home_guard_with(|| {
            let d = std::env::temp_dir().join(format!("fleet-drive-{}-{}", std::process::id(), new_token()));
            let _ = std::fs::create_dir_all(&d);
            d
        })
    }

    fn probes<'a>(alive: &'a dyn Fn(&str) -> bool) -> Probes<'a> {
        Probes { session_alive: alive, holder_alive: &|_, _| true, derived: &|_| Vec::new() }
    }

    fn probes_deriving<'a>(
        alive: &'a dyn Fn(&str) -> bool,
        derived: &'a dyn Fn(&str) -> Vec<PendingIntent>,
    ) -> Probes<'a> {
        Probes { session_alive: alive, holder_alive: &|_, _| true, derived }
    }

    #[test]
    fn derived_intents_block_takeover_and_are_never_stored() {
        let _home = temp_home();
        let dead = |_: &str| false;
        for (class, mech) in [
            (DriverClass::Continue, "watch"),
            (DriverClass::Continue, "pending_message"),
            (DriverClass::Answer, "parked_card"),
        ] {
            let derived = move |_: &str| vec![derived_intent(class, mech, "x", "r".into())];
            let p = probes_deriving(&dead, &derived);
            let err = acquire_takeover_with("d1", REVIVE, &p).unwrap_err();
            assert!(matches!(&err, Refusal::Pending { mechanism, .. } if mechanism == mech), "{mech}: {err}");
            assert!(!state_path("d1").unwrap().exists(), "{mech}: a refusal writes nothing");
            assert_eq!(snapshot_with("d1", &p).pending.len(), 1, "{mech}: snapshot shows it");
        }
    }

    #[test]
    fn parked_card_blocks_continue_but_not_answer_or_manual() {
        let _home = temp_home();
        let dead = |_: &str| false;
        let derived = |_: &str| vec![derived_intent(DriverClass::Answer, "parked_card", "c", "r".into())];
        let p = probes_deriving(&dead, &derived);
        assert!(acquire_with("d2", Driver::continue_("watch"), None, &p).is_err());
        drop(acquire_with("d2", Driver::answer("parked_card"), None, &p).unwrap());
        let lease = acquire_with("d2", Driver::manual("desktop"), None, &p).unwrap();
        assert!(
            state_path("d2").unwrap().exists() && snapshot_with("d2", &probes(&dead)).pending.is_empty(),
            "only the running lease is stored, the derived intent is not"
        );
        drop(lease);
    }

    #[test]
    fn reservation_names_stored_claims_only() {
        let _home = temp_home();
        let dead = |_: &str| false;
        let derived = |_: &str| vec![derived_intent(DriverClass::Continue, "watch", "w", "r".into())];
        assert_eq!(reservation_with("d4", &probes_deriving(&dead, &derived)), None);
        let token = register_pending(
            "d4",
            RETRY,
            PendingSpec { reason: "ENOTFOUND".into(), not_before_ms: None, ttl_ms: 60_000, yield_to_turn: true },
        )
        .unwrap();
        assert_eq!(reservation_with("d4", &probes(&dead)).as_deref(), Some("server_error_retry: ENOTFOUND"));
        withdraw_pending("d4", &token);
        let lease = acquire_with("d4", Driver::manual("desktop"), None, &probes(&dead)).unwrap();
        assert_eq!(reservation_with("d4", &probes(&dead)).as_deref(), Some("desktop: turn in flight"));
        drop(lease);
        assert_eq!(reservation_with("d4", &probes(&dead)), None);
    }

    #[test]
    fn armed_watch_does_not_block_a_retry_of_the_same_turn() {
        let _home = temp_home();
        let dead = |_: &str| false;
        let derived = |_: &str| vec![derived_intent(DriverClass::Continue, "watch", "w", "r".into())];
        let p = probes_deriving(&dead, &derived);
        drop(acquire_with("d3", RETRY, None, &p).expect("equal class"));
    }

    #[test]
    fn acquire_then_drop_frees_the_session() {
        let _home = temp_home();
        let dead = |_: &str| false;
        let p = probes(&dead);
        let lease = acquire_with("s1", RETRY, None, &p).expect("free");
        let second = acquire_with("s1", Driver::manual("m"), None, &p);
        assert!(matches!(second, Err(Refusal::Running { .. })), "grace covers the unseen spawn");
        drop(lease);
        assert!(acquire_with("s1", Driver::manual("m"), None, &p).is_ok());
        assert!(!state_path("s1").unwrap().exists(), "dropped lease leaves no record");
    }

    #[test]
    fn released_store_leaves_no_file() {
        let _home = temp_home();
        let dead = |_: &str| false;
        drop(acquire_with("s2", RETRY, None, &probes(&dead)).unwrap());
        assert!(!state_path("s2").unwrap().exists());
    }

    #[test]
    fn pending_is_revoked_by_manual_and_holder_sees_it() {
        let _home = temp_home();
        let token = register_pending(
            "s3",
            RETRY,
            PendingSpec { reason: "ENOTFOUND".into(), not_before_ms: None, ttl_ms: 60_000, yield_to_turn: true },
        )
        .unwrap();
        assert!(pending_held("s3", &token));
        let dead = |_: &str| false;
        assert!(acquire_takeover_with("s3", REVIVE, &probes(&dead)).is_err(), "retry pending blocks revive");
        let lease = acquire_with("s3", Driver::manual("desktop"), None, &probes(&dead)).unwrap();
        assert!(!pending_held("s3", &token), "manual turn supersedes the retry");
        drop(lease);
    }

    #[test]
    fn holder_converts_its_own_pending_into_a_turn() {
        let _home = temp_home();
        let token = register_pending(
            "s4",
            RETRY,
            PendingSpec { reason: "r".into(), not_before_ms: None, ttl_ms: 60_000, yield_to_turn: true },
        )
        .unwrap();
        let dead = |_: &str| false;
        let lease = acquire_with("s4", RETRY, Some(&token), &probes(&dead)).unwrap();
        assert!(snapshot("s4").pending.is_empty());
        drop(lease);
    }

    #[test]
    fn takeover_leaves_a_hold_that_refuses_a_second_takeover() {
        let _home = temp_home();
        let dead = |_: &str| false;
        let p = probes(&dead);
        acquire_takeover_with("s5", REVIVE, &p).unwrap();
        assert!(acquire_takeover_with("s5", Driver::takeover("finish_continue"), &p).is_err());
        // The replaced session stays reachable for the user.
        drop(acquire_with("s5", Driver::manual("m"), None, &p).unwrap());
    }

    #[test]
    fn sweep_removes_a_lease_its_exited_holder_never_released() {
        // Smoke 2026-09-30: every watch fire left `~/.fleet/drive/<sid>.json`
        // behind, because the timer process exits right after resuming and its
        // on_exit never runs.
        let _home = temp_home();
        let stale = DriveState { running: Some(running("watch", 0)), pending: vec![] };
        let fresh = DriveState { running: Some(running("watch", now_ms())), pending: vec![] };
        store(&state_path("old").unwrap(), &stale);
        store(&state_path("new").unwrap(), &fresh);
        let dead = |_: &str| false;
        sweep_with(&probes(&dead));
        assert!(!state_path("old").unwrap().exists(), "past grace, process gone");
        assert!(state_path("new").unwrap().exists(), "still inside the spawn grace");
    }

    #[test]
    fn withdraw_removes_the_intent() {
        let _home = temp_home();
        let token = register_pending(
            "s6",
            RETRY,
            PendingSpec { reason: "r".into(), not_before_ms: None, ttl_ms: 60_000, yield_to_turn: false },
        )
        .unwrap();
        withdraw_pending("s6", &token);
        assert!(!pending_held("s6", &token));
        assert!(!state_path("s6").unwrap().exists());
    }

    #[test]
    fn unsafe_session_ids_are_mapped_into_the_drive_dir() {
        let _home = temp_home();
        let p = state_path("../x/y:z").unwrap();
        assert_eq!(p.parent(), drive_dir().as_deref());
        assert!(state_path("..").is_none());
    }
}
