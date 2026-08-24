//! Scheduled task — a one-shot prompt that fires at an absolute wall-clock time,
//! however far out, and survives the turn boundary.
//!
//! This is the sibling of [`crate::agent_loop`]. A *loop* is "do this again every
//! N" (recurring, interval-relative, capped at Claude Code's 7-day cron horizon);
//! a *schedule* is "do this once, at time T" (one-shot, absolute time, no 7-day
//! ceiling). Both exist because Claude Code's own `CronCreate` / `ScheduleWakeup`
//! / `/loop` are in-process REPL timers that evaporate the instant a headless
//! `claude -p` turn ends (see [`crate::bg_guard`]); a Fleet schedule instead lives
//! on disk and fires from a detached timer, so nothing depends on the registering
//! session's process still being alive.
//!
//! Why a separate module rather than a flag on `agent_loop`: the semantics差得
//! 远。A schedule fires exactly once and then retires; it is addressed by an
//! absolute time the user names (`--at "2026-07-25 09:00"` or `--in 5d`); and it
//! deliberately drops the 7-day expiry that bounds a loop, because "remind me in
//! three weeks" is the whole point. Overloading the loop record with an
//! at-time-once mode would have made both harder to reason about.
//!
//! ## Firing, exactly once
//!
//! `fleet schedule create` arms a detached timer (`fleet schedule fire <id>
//! <gen>`) that sleeps until [`ScheduleRecord::fire_at`], then claims and spawns.
//! [`claim_fire`] is the single serialization point: it re-reads the record,
//! refuses anything not still `Pending`, and flips it to `Fired` before returning
//! — so a second timer (armed by [`reconcile`] after the first was thought dead)
//! that wakes late finds the record already `Fired` and spawns nothing. This is
//! the same `generation` + re-read-at-claim contract `agent_loop` ships; a
//! schedule keeps the fired record on disk (status `Fired`) rather than deleting
//! it, so `fleet schedule list` is a durable history of what ran, which is what a
//! trackable future-task needs.
//!
//! File layout: `~/.fleet/schedules/<id>.json`, one record per schedule.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Furthest out a schedule may be set. A one-shot has no recurring expiry — the
/// whole feature is "fire once, however far out" — but a horizon still guards
/// against a fat-fingered epoch that would park a timer for a decade.
pub const MAX_HORIZON_MS: u64 = 365 * 24 * 60 * 60 * 1000;

/// A non-LLM precondition on a schedule's fire: once the schedule is due, the
/// `--until` shell command is polled every `poll_secs` and the session spawns
/// only when it exits 0. If `timeout_secs` elapses past the due time with the
/// gate never met, the schedule is abandoned (marked fired, timed-out, **no**
/// session spawned) — the chosen semantics over watch's "fire a timeout
/// notice". Mirrors `fleet watch --until`, but gates a *spawn* instead of a
/// *resume*. Reuses [`crate::watch`]'s poll/timeout grammar and floors so
/// `--poll` / `--timeout` mean the same across both features.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleGate {
    /// Shell command polled once the schedule is due; exit 0 ⇒ fire.
    pub until_cmd: String,
    /// Seconds between gate polls (already clamped to the watch poll floor).
    pub poll_secs: u64,
    /// Seconds after the due time to keep polling before abandoning.
    pub timeout_secs: u64,
}

/// Lifecycle of a schedule. `Cancelled` is not a stored state — cancelling
/// deletes the file; the enum has two variants so a fired schedule stays visible
/// in `list` as history.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleStatus {
    /// Armed, not yet fired.
    Pending,
    /// Already fired — retained on disk as history, never re-armed.
    Fired,
}

impl Default for ScheduleStatus {
    fn default() -> Self {
        Self::Pending
    }
}

/// One registered schedule.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRecord {
    pub id: String,
    /// Workspace the fired session is spawned in.
    pub workspace_path: String,
    /// The prompt the fired session runs — the schedule's full context.
    pub prompt: String,
    /// Epoch ms of the scheduled fire.
    pub fire_at: u64,
    #[serde(default)]
    pub status: ScheduleStatus,
    /// Epoch ms the fire actually happened (may lag `fire_at` if the machine was
    /// asleep and a `reconcile` sweep caught it late). `None` until fired.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fired_at: Option<u64>,
    /// Model the fired session runs on — inherited from the registering session
    /// so a fable-5 schedule doesn't wake up as opus.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<String>,
    /// Bumped on claim; a stale timer holding the old generation is refused. Same
    /// role as `agent_loop`'s generation. See module docs.
    pub generation: u64,
    pub created: u64,
    /// Session that registered the schedule, for provenance.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub created_by_session: Option<String>,
    /// Agent source the fired session is spawned on (`"claude"` / `"codex"`),
    /// inherited from the registering session so a codex schedule wakes as codex.
    /// `None` = the historical default (claude).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_source: Option<String>,
    /// The session the fire produced, for `fleet schedule list`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fired_session_id: Option<String>,
    /// Optional non-LLM gate command (`--until`): once due, the schedule only
    /// fires when this exits 0. `None` = fire the moment it's due (the classic
    /// behaviour). See [`ScheduleGate`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub until_cmd: Option<String>,
    /// Seconds between gate polls once due. `None` (or when there's no gate) =
    /// [`DEFAULT_GATE_POLL_SECS`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub poll_secs: Option<u64>,
    /// Seconds past the due time to keep polling the gate before abandoning.
    /// `None` (or when there's no gate) = [`DEFAULT_GATE_TIMEOUT_SECS`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gate_timeout_secs: Option<u64>,
    /// Stamped `true` when a gated schedule was abandoned because its gate never
    /// met within the timeout — it is `Fired` (history) but spawned nothing, so
    /// `fleet schedule list` can tell an abandoned schedule from one that ran.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gate_timed_out: bool,
}

/// Default seconds between gate polls once a gated schedule is due. Same value
/// as [`crate::watch::DEFAULT_POLL_SECS`].
pub const DEFAULT_GATE_POLL_SECS: u64 = crate::watch::DEFAULT_POLL_SECS;

/// Default give-up window (seconds past the due time) for a gated schedule when
/// `--timeout` is omitted. Same value as [`crate::watch::DEFAULT_TIMEOUT_SECS`].
pub const DEFAULT_GATE_TIMEOUT_SECS: u64 = crate::watch::DEFAULT_TIMEOUT_SECS;

impl ScheduleRecord {
    pub fn is_pending(&self) -> bool {
        self.status == ScheduleStatus::Pending
    }

    /// Milliseconds until this schedule is due (0 when it already is).
    pub fn due_in_ms(&self, now: u64) -> u64 {
        self.fire_at.saturating_sub(now)
    }

    pub fn is_due(&self, now: u64) -> bool {
        self.fire_at <= now
    }

    /// Still needs firing: pending and its due time has arrived. Unlike a loop
    /// there is no overdue expiry — a schedule the machine slept through must
    /// still fire whenever Fleet next notices it.
    pub fn is_claimable(&self, now: u64) -> bool {
        self.is_pending() && self.is_due(now)
    }

    /// Whether a non-LLM gate must be satisfied before this schedule fires.
    pub fn has_gate(&self) -> bool {
        self.until_cmd.is_some()
    }

    /// Seconds between gate polls (defaulted).
    pub fn gate_poll_secs(&self) -> u64 {
        self.poll_secs.unwrap_or(DEFAULT_GATE_POLL_SECS)
    }

    /// Absolute epoch-ms after which a still-unmet gate is abandoned. `None`
    /// when the schedule has no gate. Measured from the due time (`fire_at`), so
    /// the give-up window is "this long after it became due", not after it was
    /// created.
    pub fn gate_deadline(&self) -> Option<u64> {
        if !self.has_gate() {
            return None;
        }
        let timeout = self.gate_timeout_secs.unwrap_or(DEFAULT_GATE_TIMEOUT_SECS);
        Some(self.fire_at.saturating_add(timeout.saturating_mul(1000)))
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// `~/.fleet/schedules`.
pub fn schedules_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("schedules"))
}

fn record_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn write_record(dir: &Path, rec: &ScheduleRecord) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create schedules dir: {e}"))?;
    let json = serde_json::to_string_pretty(rec).map_err(|e| format!("serialize schedule: {e}"))?;
    fs::write(record_path(dir, &rec.id), json).map_err(|e| format!("write schedule: {e}"))
}

// ── time parsing ───────────────────────────────────────────────────────────────

/// Parse a relative duration (`5d`, `2h`, `90m`) into an absolute fire time.
/// Reuses [`crate::agent_loop::parse_interval`] so the accepted units and the
/// 60-second floor are identical to a loop's `--interval` — no second grammar to
/// keep in sync.
pub fn parse_in(spec: &str, now: u64) -> Result<u64, String> {
    let secs = crate::agent_loop::parse_interval(spec)?;
    Ok(now + secs * 1000)
}

/// Parse an absolute local wall-clock time into epoch ms. Accepts
/// `YYYY-MM-DD HH:MM[:SS]`, `YYYY-MM-DDTHH:MM[:SS]`, or a bare `HH:MM[:SS]` (the
/// next occurrence today, or tomorrow if that time already passed today). The
/// wall-clock is interpreted in the machine's local timezone, matching what the
/// user reads off their clock.
pub fn parse_at(spec: &str, now: u64) -> Result<u64, String> {
    use chrono::{Local, NaiveDateTime, NaiveTime, TimeZone};
    let s = spec.trim();
    if s.is_empty() {
        return Err("--at time is required (e.g. \"2026-07-25 09:00\")".into());
    }
    // Full datetime forms first.
    let full = [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
    ]
    .iter()
    .find_map(|fmt| NaiveDateTime::parse_from_str(s, fmt).ok());
    let naive = match full {
        Some(dt) => dt,
        None => {
            // Bare time-of-day → today, or tomorrow if already past.
            let t = NaiveTime::parse_from_str(s, "%H:%M:%S")
                .or_else(|_| NaiveTime::parse_from_str(s, "%H:%M"))
                .map_err(|_| {
                    format!("cannot parse --at time '{s}' (try \"2026-07-25 09:00\" or \"09:00\")")
                })?;
            let now_local = Local
                .timestamp_millis_opt(now as i64)
                .single()
                .ok_or("clock out of range")?;
            let today = now_local.date_naive();
            let candidate = NaiveDateTime::new(today, t);
            if to_epoch_ms_local(candidate)? <= now {
                NaiveDateTime::new(today.succ_opt().ok_or("date overflow")?, t)
            } else {
                candidate
            }
        }
    };
    to_epoch_ms_local(naive)
}

/// A naive local datetime → epoch ms, picking the earlier instant across a DST
/// fold and rejecting a time that falls in a spring-forward gap.
fn to_epoch_ms_local(naive: chrono::NaiveDateTime) -> Result<u64, String> {
    use chrono::{Local, TimeZone};
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => Ok(dt.timestamp_millis().max(0) as u64),
        chrono::LocalResult::Ambiguous(dt, _) => Ok(dt.timestamp_millis().max(0) as u64),
        chrono::LocalResult::None => Err("that local time does not exist (DST gap)".into()),
    }
}

// ── create / read / cancel ─────────────────────────────────────────────────────

/// Register a one-shot schedule that fires at absolute epoch-ms `fire_at`.
#[allow(clippy::too_many_arguments)]
pub fn create(
    workspace_path: &str,
    prompt: &str,
    fire_at: u64,
    model: Option<&str>,
    effort: Option<&str>,
    agent_source: Option<&str>,
    created_by_session: Option<&str>,
    gate: Option<ScheduleGate>,
) -> Result<ScheduleRecord, String> {
    let dir = schedules_dir().ok_or("cannot determine home dir")?;
    create_in(
        &dir,
        workspace_path,
        prompt,
        fire_at,
        model,
        effort,
        agent_source,
        created_by_session,
        gate,
        &uuid::Uuid::new_v4().to_string()[..8],
        now_ms(),
    )
}

#[allow(clippy::too_many_arguments)]
fn create_in(
    dir: &Path,
    workspace_path: &str,
    prompt: &str,
    fire_at: u64,
    model: Option<&str>,
    effort: Option<&str>,
    agent_source: Option<&str>,
    created_by_session: Option<&str>,
    gate: Option<ScheduleGate>,
    id: &str,
    now: u64,
) -> Result<ScheduleRecord, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("schedule prompt is required".to_string());
    }
    if fire_at <= now {
        return Err("schedule time is in the past".to_string());
    }
    if fire_at.saturating_sub(now) > MAX_HORIZON_MS {
        return Err(format!(
            "schedule time is more than {} days out (horizon guard)",
            MAX_HORIZON_MS / (24 * 60 * 60 * 1000)
        ));
    }
    let blank = |s: &str| s.trim().is_empty();
    // A gate whose command is blank is no gate at all — drop it so the schedule
    // fires the moment it's due rather than polling an empty string forever.
    let gate = gate.filter(|g| !blank(&g.until_cmd));
    let rec = ScheduleRecord {
        id: id.to_string(),
        workspace_path: workspace_path.to_string(),
        prompt: prompt.to_string(),
        fire_at,
        status: ScheduleStatus::Pending,
        fired_at: None,
        model: model.filter(|m| !blank(m)).map(str::to_string),
        effort: effort.filter(|e| !blank(e)).map(str::to_string),
        generation: 0,
        created: now,
        created_by_session: created_by_session.map(str::to_string),
        agent_source: agent_source.filter(|s| !blank(s)).map(str::to_string),
        fired_session_id: None,
        until_cmd: gate.as_ref().map(|g| g.until_cmd.trim().to_string()),
        poll_secs: gate.as_ref().map(|g| g.poll_secs.max(crate::watch::MIN_POLL_SECS)),
        gate_timeout_secs: gate.as_ref().map(|g| g.timeout_secs),
        gate_timed_out: false,
    };
    write_record(dir, &rec)?;
    Ok(rec)
}

pub fn get(id: &str) -> Option<ScheduleRecord> {
    get_in(&schedules_dir()?, id)
}

fn get_in(dir: &Path, id: &str) -> Option<ScheduleRecord> {
    let s = fs::read_to_string(record_path(dir, id)).ok()?;
    serde_json::from_str(&s).ok()
}

/// Every registered schedule, soonest-due first (pending and fired history).
pub fn list() -> Vec<ScheduleRecord> {
    let Some(dir) = schedules_dir() else {
        return Vec::new();
    };
    list_in(&dir)
}

fn list_in(dir: &Path) -> Vec<ScheduleRecord> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<ScheduleRecord> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| {
            let s = fs::read_to_string(e.path()).ok()?;
            serde_json::from_str(&s).ok()
        })
        .collect();
    out.sort_by_key(|r| r.fire_at);
    out
}

/// Cancel (or forget-history-of) a schedule. Idempotent — returns whether a
/// record was actually removed. A timer already sleeping on this schedule will
/// find the record gone when it wakes and exit without firing.
pub fn cancel(id: &str) -> bool {
    let Some(dir) = schedules_dir() else {
        return false;
    };
    cancel_in(&dir, id)
}

fn cancel_in(dir: &Path, id: &str) -> bool {
    fs::remove_file(record_path(dir, id)).is_ok()
}

/// A requested change to a still-`Pending` schedule. Every field is optional:
/// `None` = leave that field untouched (CLI partial update). For the three
/// inherited spawn knobs (`model` / `effort` / `agent_source`), `Some("")` (or
/// whitespace) clears the field back to "inherit the CLI default" — this mirrors
/// the desktop pickers' `""`-means-default convention, so an edit form that
/// always sends every field can both set and clear. Crosses the `fleet serve`
/// HTTP boundary, hence `Serialize + Deserialize` in camelCase.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleUpdate {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fire_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_source: Option<String>,
}

/// A blank/whitespace spawn-knob string means "inherit the CLI default" ⇒ `None`.
fn norm_knob(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Adjust a still-`Pending` schedule: change its fire time, prompt, and/or the
/// inherited spawn knobs (model / effort / agent source). Bumps `generation` so
/// any detached timer sleeping on the old time exits as superseded when it next
/// wakes — the caller (`fleet schedule update`, or the desktop edit form via
/// `Backend::update_schedule`) re-arms a fresh timer from the returned record. A
/// fired schedule is history and cannot be updated.
pub fn update(u: &ScheduleUpdate) -> Result<ScheduleRecord, String> {
    let dir = schedules_dir().ok_or("cannot determine home dir")?;
    update_in(&dir, u, now_ms())
}

fn update_in(dir: &Path, u: &ScheduleUpdate, now: u64) -> Result<ScheduleRecord, String> {
    let mut rec = get_in(dir, &u.id).ok_or_else(|| format!("no schedule with id {}", u.id))?;
    if rec.status == ScheduleStatus::Fired {
        return Err("cannot update a schedule that already fired".to_string());
    }
    if let Some(fa) = u.fire_at {
        if fa <= now {
            return Err("schedule time is in the past".to_string());
        }
        if fa.saturating_sub(now) > MAX_HORIZON_MS {
            return Err(format!(
                "schedule time is more than {} days out (horizon guard)",
                MAX_HORIZON_MS / (24 * 60 * 60 * 1000)
            ));
        }
        rec.fire_at = fa;
    }
    if let Some(p) = &u.prompt {
        let p = p.trim();
        if p.is_empty() {
            return Err("schedule prompt cannot be empty".to_string());
        }
        rec.prompt = p.to_string();
    }
    // Spawn knobs: Some("") clears to inherit-default (None); Some(x) sets; None leaves.
    if let Some(m) = &u.model {
        rec.model = norm_knob(m);
    }
    if let Some(e) = &u.effort {
        rec.effort = norm_knob(e);
    }
    if let Some(s) = &u.agent_source {
        rec.agent_source = norm_knob(s);
    }
    rec.generation += 1;
    write_record(dir, &rec)?;
    Ok(rec)
}

// ── firing ─────────────────────────────────────────────────────────────────────

/// Why a claim was refused, so the timer process can log something truthful.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimError {
    /// The schedule was cancelled while the timer slept.
    Gone,
    /// Already fired — a stale timer woke after the real fire.
    AlreadyFired,
    /// Another timer bumped the generation (re-armed by reconcile).
    StaleGeneration { expected: u64, found: u64 },
    /// Woke early — not due yet.
    NotDue { due_in_ms: u64 },
}

impl std::fmt::Display for ClaimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gone => write!(f, "schedule was cancelled"),
            Self::AlreadyFired => write!(f, "schedule already fired"),
            Self::StaleGeneration { expected, found } => write!(
                f,
                "stale timer (held generation {expected}, record is at {found})"
            ),
            Self::NotDue { due_in_ms } => write!(f, "not due for another {due_in_ms}ms"),
        }
    }
}

/// Atomically claim the one fire of a schedule.
///
/// The single serialization point: it re-reads the record, refuses anything not
/// still `Pending` (already fired ⇒ another timer won) or holding a stale
/// generation, then flips the record to `Fired` and persists it before returning.
/// A second timer that wakes holding the old generation — or finds the record
/// already `Fired` — is refused, so the schedule fires exactly once. The fired
/// record stays on disk as history; only [`record_fired_session`] touches it
/// afterwards to stamp the spawned session id.
pub fn claim_fire(id: &str, generation: u64) -> Result<ScheduleRecord, ClaimError> {
    let dir = schedules_dir().ok_or(ClaimError::Gone)?;
    claim_fire_in(&dir, id, generation, now_ms())
}

fn claim_fire_in(
    dir: &Path,
    id: &str,
    generation: u64,
    now: u64,
) -> Result<ScheduleRecord, ClaimError> {
    claim_transition_in(dir, id, generation, now, false)
}

/// The single serialization point for both firing and abandoning: flips a
/// still-`Pending`, due, current-generation schedule to `Fired`. `timed_out`
/// stamps [`ScheduleRecord::gate_timed_out`] — `true` for the abandon path (gate
/// never met within the window; caller spawns nothing), `false` for a normal
/// fire. Refuses an already-fired record or a stale generation so a schedule
/// transitions exactly once regardless of how many timers race.
fn claim_transition_in(
    dir: &Path,
    id: &str,
    generation: u64,
    now: u64,
    timed_out: bool,
) -> Result<ScheduleRecord, ClaimError> {
    let mut rec = get_in(dir, id).ok_or(ClaimError::Gone)?;
    if rec.status == ScheduleStatus::Fired {
        return Err(ClaimError::AlreadyFired);
    }
    if rec.generation != generation {
        return Err(ClaimError::StaleGeneration {
            expected: generation,
            found: rec.generation,
        });
    }
    if !rec.is_due(now) {
        return Err(ClaimError::NotDue {
            due_in_ms: rec.due_in_ms(now),
        });
    }

    rec.status = ScheduleStatus::Fired;
    rec.fired_at = Some(now);
    rec.gate_timed_out = timed_out;
    rec.generation += 1;
    write_record(dir, &rec).map_err(|_| ClaimError::Gone)?;
    Ok(rec)
}

/// Abandon a gated schedule whose gate never met within its timeout window:
/// claim it (flipping to `Fired` with `gate_timed_out = true`) and spawn
/// nothing. Returns the abandoned record. The chosen timeout semantics — the
/// schedule is retained as history so `fleet schedule list` shows it timed out,
/// but no session runs.
pub fn abandon_on_timeout(id: &str, generation: u64) -> Result<ScheduleRecord, ClaimError> {
    let dir = schedules_dir().ok_or(ClaimError::Gone)?;
    abandon_on_timeout_in(&dir, id, generation, now_ms())
}

fn abandon_on_timeout_in(
    dir: &Path,
    id: &str,
    generation: u64,
    now: u64,
) -> Result<ScheduleRecord, ClaimError> {
    let claimed = claim_transition_in(dir, id, generation, now, true)?;
    crate::log_debug(&format!(
        "schedule {id}: gate timed out, abandoned (marked fired, no session spawned)"
    ));
    Ok(claimed)
}

/// Stamp the session a fire produced onto the (already `Fired`) record, so
/// `fleet schedule list` shows what ran. Best-effort — a cancelled record just
/// means the stamp is dropped.
pub fn record_fired_session(id: &str, session_id: &str) {
    let Some(dir) = schedules_dir() else { return };
    record_fired_session_in(&dir, id, session_id);
}

fn record_fired_session_in(dir: &Path, id: &str, session_id: &str) {
    if let Some(mut rec) = get_in(dir, id) {
        rec.fired_session_id = Some(session_id.to_string());
        let _ = write_record(dir, &rec);
    }
}

/// Schedules that are due now and still pending — the reconcile sweep's input,
/// for fires missed while the machine was asleep or Fleet wasn't running.
pub fn due_schedules(now: u64) -> Vec<ScheduleRecord> {
    let Some(dir) = schedules_dir() else {
        return Vec::new();
    };
    due_schedules_in(&dir, now)
}

fn due_schedules_in(dir: &Path, now: u64) -> Vec<ScheduleRecord> {
    list_in(dir)
        .into_iter()
        .filter(|r| r.is_claimable(now))
        .collect()
}

// ── fire spawn + timer ───────────────────────────────────────────────────────

/// `CLAUDE_CODE_ENTRYPOINT` for scheduled fires, so the scanner and transcript
/// readers can tell an auto-spawned scheduled session from a user-initiated one.
pub const SCHEDULE_ENTRYPOINT: &str = "claw-fleet-schedule";

/// The prompt the fired session runs: the user's prompt, plus a short footer
/// naming the schedule so the agent isn't confused about why it woke up.
pub fn compose_fire_prompt(rec: &ScheduleRecord) -> String {
    let mut out = String::new();
    out.push_str(&rec.prompt);
    out.push_str("\n\n---\n");
    out.push_str(&format!(
        "（这是 Fleet 定时任务 `{}`：你在 {} 注册它、约定 {} 触发，现在到点了。\
         这是一次性任务，触发后已自动归档为历史，无需你注册任何 cron 或 wakeup——\
         那些在 headless 会话里不会触发。用 `fleet schedule list` 看历史。\
         本会话是**无人值守**的自动触发，背后没有人能回答决策卡：完成后请直接\
         以纯文本收尾结束回合，**不要**调用 `fleet__ask`/`AskUserQuestion` 交回\
         控制权（交互模式对本回合豁免）。）",
        rec.id,
        fmt_local(rec.created),
        fmt_local(rec.fire_at),
    ));
    out
}

/// The prompt a **manual run** ("立即运行") uses. Unlike [`compose_fire_prompt`],
/// this run does *not* consume the schedule: the record stays `Pending` and its
/// timer is untouched, so it will still fire at the originally scheduled time.
/// The footer says so, so the agent doesn't think it was the scheduled fire.
pub fn compose_run_now_prompt(rec: &ScheduleRecord) -> String {
    let mut out = String::new();
    out.push_str(&rec.prompt);
    out.push_str("\n\n---\n");
    out.push_str(&format!(
        "（这是 Fleet 定时任务 `{}` 的一次**手动运行**：你在 {} 注册它、约定 {} 触发，\
         老板现在手动跑了一次。原定时任务并未消耗——它仍会在原定时间自动触发一次。\
         你无需注册任何 cron 或 wakeup。用 `fleet schedule list` 看它的状态。）",
        rec.id,
        fmt_local(rec.created),
        fmt_local(rec.fire_at),
    ));
    out
}

/// Epoch ms → human-readable local time for the fire-prompt footer.
fn fmt_local(ms: u64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms as i64) {
        chrono::LocalResult::Single(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
        _ => format!("{ms}ms"),
    }
}

/// Signature of the session spawner — injected so the fire path is testable
/// without launching a real agent. Mirrors [`crate::agent_loop`]'s `SpawnFn`.
type SpawnFn<'a> = dyn Fn(
        &str,
        &str,
        &str,
        Option<&str>,
        Option<&str>,
        Option<&str>,
        &str,
    ) -> Result<crate::session_launch::SpawnSessionResponse, String>
    + 'a;

/// Fire a schedule: claim the one slot, spawn the session, and stamp which
/// session it produced. Returns the claimed (now `Fired`) record.
///
/// A spawn failure after a successful claim is swallowed to a logged error rather
/// than un-firing: re-claiming would risk a double-fire, and the record already
/// reads `Fired` so no timer re-arms it.
pub fn fire_once(id: &str, generation: u64) -> Result<ScheduleRecord, ClaimError> {
    let dir = schedules_dir().ok_or(ClaimError::Gone)?;
    fire_once_in(
        &dir,
        id,
        generation,
        now_ms(),
        &move |source, ws, prompt, model, effort, perm, ep| {
            crate::agent_source::spawn_session(
                source,
                &crate::agent_source::SpawnSpec {
                    workspace_path: ws.to_string(),
                    prompt: prompt.to_string(),
                    model: model.map(str::to_string),
                    effort: effort.map(str::to_string),
                    permission_mode: perm.map(str::to_string),
                    session_id: None,
                    entrypoint: ep.to_string(),
                images: Vec::new(),
                },
            )
        },
    )
}

fn fire_once_in(
    dir: &Path,
    id: &str,
    generation: u64,
    now: u64,
    spawn: &SpawnFn<'_>,
) -> Result<ScheduleRecord, ClaimError> {
    let claimed = claim_fire_in(dir, id, generation, now)?;
    let prompt = compose_fire_prompt(&claimed);
    match spawn(
        claimed.agent_source.as_deref().unwrap_or("claude"),
        &claimed.workspace_path,
        &prompt,
        claimed.model.as_deref(),
        claimed.effort.as_deref(),
        None,
        SCHEDULE_ENTRYPOINT,
    ) {
        Ok(resp) => {
            if let Some(sid) = resp.session_id {
                record_fired_session_in(dir, id, &sid);
                crate::log_debug(&format!("schedule {id}: fired -> session {sid}"));
            }
        }
        Err(e) => {
            crate::log_debug(&format!(
                "schedule {id}: fire spawn failed: {e} (record marked fired, will not retry)"
            ));
        }
    }
    Ok(claimed)
}

/// Manually run a schedule **now**, without touching its record. Spawns a session
/// with the schedule's own prompt/workspace/model/effort/source, but does *not*
/// claim, flip `status`, bump `generation`, or stamp `firedSessionId` — so the
/// pending schedule and its armed timer are untouched and it still fires at its
/// scheduled time. Works whether the schedule is `Pending` or already `Fired`
/// (a fired one is just re-run). Returns the spawned session id, if any.
pub fn run_now(id: &str) -> Result<Option<String>, String> {
    let dir = schedules_dir().ok_or_else(|| "no fleet home".to_string())?;
    run_now_in(
        &dir,
        id,
        &move |source, ws, prompt, model, effort, perm, ep| {
            crate::agent_source::spawn_session(
                source,
                &crate::agent_source::SpawnSpec {
                    workspace_path: ws.to_string(),
                    prompt: prompt.to_string(),
                    model: model.map(str::to_string),
                    effort: effort.map(str::to_string),
                    permission_mode: perm.map(str::to_string),
                    session_id: None,
                    entrypoint: ep.to_string(),
                images: Vec::new(),
                },
            )
        },
    )
}

fn run_now_in(dir: &Path, id: &str, spawn: &SpawnFn<'_>) -> Result<Option<String>, String> {
    let rec = get_in(dir, id).ok_or_else(|| format!("no schedule with id {id}"))?;
    let prompt = compose_run_now_prompt(&rec);
    let resp = spawn(
        rec.agent_source.as_deref().unwrap_or("claude"),
        &rec.workspace_path,
        &prompt,
        rec.model.as_deref(),
        rec.effort.as_deref(),
        None,
        SCHEDULE_ENTRYPOINT,
    )
    .map_err(|e| format!("run-now spawn failed: {e}"))?;
    if let Some(sid) = &resp.session_id {
        crate::log_debug(&format!("schedule {id}: manual run -> session {sid}"));
    }
    Ok(resp.session_id)
}

/// Longest a timer sleeps in one go before re-checking the record. Caps how long
/// a timer lingers after `fleet schedule cancel` deletes the record (it notices
/// on the next wake and exits), and bounds clock-drift from one long sleep
/// spanning a laptop suspend. Same value/rationale as `agent_loop`'s POLL_CAP.
const POLL_CAP: std::time::Duration = std::time::Duration::from_secs(30);

/// What the timer should do this iteration. Pure decision, split out so the
/// nap/fire/abandon/exit logic is unit-testable without spawning subprocesses or
/// sleeping. Mirrors [`crate::watch::TimerStep`], with an `Abandon` step for the
/// gate-timed-out path (fire a *nothing*, unlike watch's timeout-resume).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedStep {
    /// Record superseded (stale generation) or already fired — timer exits.
    Exit,
    /// Fire now (spawn the session).
    Fire,
    /// Gate never met within its window — mark fired/timed-out, spawn nothing.
    Abandon,
    /// Not yet — nap this many ms and re-check.
    Nap { ms: u64 },
}

/// Decide the timer's next move. `gate_met` is evaluated lazily — only after the
/// generation / pending / due / deadline checks pass — so the shell poll runs no
/// more than once per wake, never before the schedule is due, and never for an
/// already-stale timer. Without a gate a due schedule fires immediately (the
/// classic behaviour).
fn decide(
    rec: &ScheduleRecord,
    generation: u64,
    now: u64,
    gate_met: impl FnOnce() -> bool,
) -> SchedStep {
    let cap_ms = POLL_CAP.as_millis() as u64;
    if rec.generation != generation {
        return SchedStep::Exit;
    }
    if !rec.is_pending() {
        return SchedStep::Exit;
    }
    if !rec.is_due(now) {
        // Nap until due, capped so cancel is noticed within POLL_CAP.
        return SchedStep::Nap {
            ms: cap_ms.min(rec.due_in_ms(now).max(1)),
        };
    }
    // Due. No gate ⇒ fire straight away.
    if !rec.has_gate() {
        return SchedStep::Fire;
    }
    // Gated: abandon once the give-up window has elapsed; else poll the gate.
    if let Some(deadline) = rec.gate_deadline() {
        if now >= deadline {
            return SchedStep::Abandon;
        }
    }
    if gate_met() {
        return SchedStep::Fire;
    }
    // Poll again: at most one poll interval, never past POLL_CAP, never past the
    // give-up deadline (so the abandon lands on time, not a poll-cap late).
    let remaining = rec
        .gate_deadline()
        .map(|d| d.saturating_sub(now))
        .unwrap_or(u64::MAX);
    let nap = rec
        .gate_poll_secs()
        .saturating_mul(1000)
        .min(cap_ms)
        .min(remaining.max(1));
    SchedStep::Nap { ms: nap }
}

/// The blocking timer loop — the body of `fleet schedule fire <id> <gen>`. Sleeps
/// until the schedule is due, then (if gated) polls the `--until` gate until it
/// is met or the give-up window elapses, then fires once (or abandons) and
/// returns. One-shot: it never re-arms. Returns early if the schedule is
/// cancelled, already fired, or the timer was superseded (stale generation).
///
/// Thin by design — every branch delegates to the unit-tested [`decide`] /
/// [`fire_once`] / [`abandon_on_timeout`] beneath it; only the real `sleep`
/// and the gate poll live here.
pub fn run_timer_blocking(id: &str, generation: u64) {
    loop {
        let Some(rec) = get(id) else {
            crate::log_debug(&format!("schedule {id}: record gone, timer exiting"));
            return;
        };
        let step = decide(&rec, generation, now_ms(), || {
            rec.until_cmd
                .as_deref()
                .map(crate::process_util::gate_met)
                .unwrap_or(false)
        });
        match step {
            SchedStep::Exit => {
                if rec.generation != generation {
                    crate::log_debug(&format!(
                        "schedule {id}: timer superseded (held gen {generation}, record at {}), exiting",
                        rec.generation
                    ));
                } else {
                    crate::log_debug(&format!("schedule {id}: already fired, timer exiting"));
                }
                return;
            }
            SchedStep::Nap { ms } => {
                std::thread::sleep(std::time::Duration::from_millis(ms));
                continue;
            }
            SchedStep::Fire => match fire_once(id, generation) {
                Ok(_) => {
                    crate::log_debug(&format!("schedule {id}: fired, timer done"));
                    return;
                }
                Err(ClaimError::NotDue { .. }) => continue,
                Err(e) => {
                    crate::log_debug(&format!("schedule {id}: timer stopping ({e})"));
                    return;
                }
            },
            SchedStep::Abandon => match abandon_on_timeout(id, generation) {
                Ok(_) => {
                    crate::log_debug(&format!("schedule {id}: gate timed out, timer done"));
                    return;
                }
                Err(ClaimError::NotDue { .. }) => continue,
                Err(e) => {
                    crate::log_debug(&format!("schedule {id}: timer stopping ({e})"));
                    return;
                }
            },
        }
    }
}

/// Spawn a detached timer process (`fleet schedule fire <id> <gen>`) that sleeps
/// until the schedule is due, fires it, and exits. Idempotent-safe: arming twice
/// just means two timers race for the same fire and the claim lets exactly one
/// win. Used by `fleet schedule create` and the reconcile sweep.
pub fn arm_timer(rec: &ScheduleRecord) -> Result<u32, String> {
    let fleet = crate::hooks::resolve_fleet_binary()
        .ok_or("cannot find fleet binary to arm schedule timer")?;
    arm_timer_with(&fleet, rec)
}

/// A pending schedule overdue by more than this is treated as stranded — its
/// detached timer died (reboot, `kill -9`) and no process is left to fire it.
/// Must be comfortably larger than [`POLL_CAP`] so a healthy timer mid-nap is
/// never mistaken for dead. Same value/rationale as `agent_loop`.
const STRANDED_GRACE_MS: u64 = 3 * 30_000; // 3 × POLL_CAP

/// Re-arm timers for pending schedules left stranded (their detached timer died).
/// This is also what fires a schedule the machine slept through: on reboot the
/// timer process is gone, but the next `reconcile` sweep sees the schedule is
/// overdue and pending and arms a fresh timer that fires it immediately. Cheap
/// and idempotent — safe to call from any periodic hook. Returns the ids it
/// re-armed.
pub fn reconcile() -> Vec<String> {
    let Some(dir) = schedules_dir() else {
        return Vec::new();
    };
    let fleet = match crate::hooks::resolve_fleet_binary() {
        Some(f) => f,
        None => return Vec::new(),
    };
    reconcile_in(&dir, now_ms(), &mut |rec| {
        let _ = arm_timer_with(&fleet, rec);
    })
}

fn reconcile_in(dir: &Path, now: u64, arm: &mut dyn FnMut(&ScheduleRecord)) -> Vec<String> {
    let mut rearmed = Vec::new();
    for rec in list_in(dir) {
        if !rec.is_pending() {
            continue;
        }
        // Overdue past the grace window ⇒ no live timer is driving it (dead timer
        // or a fire the machine slept through). Arm a fresh timer to fire it now.
        if rec.is_due(now) && now.saturating_sub(rec.fire_at) > STRANDED_GRACE_MS {
            arm(&rec);
            rearmed.push(rec.id.clone());
        }
    }
    rearmed
}

fn arm_timer_with(fleet_bin: &str, rec: &ScheduleRecord) -> Result<u32, String> {
    // process_util::command: no conhost flash on Windows; a Windows child
    // survives its parent by default, so no setsid analogue is needed there.
    let mut cmd = crate::process_util::command(fleet_bin);
    cmd.arg("schedule")
        .arg("fire")
        .arg(&rec.id)
        .arg(rec.generation.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Detach into its own session so quitting the desktop app / fleet serve does
    // not take the timer down — same contract as the loop timer and proc host.
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| format!("spawn schedule timer: {e}"))?;
    let pid = child.id();
    // Reap the direct child handle; the timer keeps running detached.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn make(d: &Path, id: &str, now: u64, fire_at: u64) -> ScheduleRecord {
        create_in(
            d,
            "/ws",
            "check the deploy",
            fire_at,
            None,
            None,
            None,
            Some("s1"),
            None,
            id,
            now,
        )
        .unwrap()
    }

    /// Build a resolved gate for tests (poll/timeout in seconds).
    fn gate(until: &str, poll: u64, timeout: u64) -> ScheduleGate {
        ScheduleGate {
            until_cmd: until.to_string(),
            poll_secs: poll,
            timeout_secs: timeout,
        }
    }

    #[test]
    fn create_stamps_absolute_fire_time_and_pending() {
        let d = dir();
        let rec = make(d.path(), "s1", 1_000_000, 1_000_000 + 5 * 86_400_000);
        assert_eq!(rec.fire_at, 1_000_000 + 5 * 86_400_000);
        assert_eq!(rec.status, ScheduleStatus::Pending);
        assert_eq!(rec.generation, 0);
        assert!(rec.fired_at.is_none());
        assert!(!rec.is_due(1_000_000));
        assert!(rec.is_due(rec.fire_at));
    }

    #[test]
    fn create_rejects_empty_prompt_past_time_and_beyond_horizon() {
        let d = dir();
        assert!(create_in(d.path(), "/ws", "  ", 2_000, None, None, None, None, None, "x", 1_000)
            .unwrap_err()
            .contains("prompt is required"));
        // in the past
        assert!(create_in(d.path(), "/ws", "p", 500, None, None, None, None, None, "x", 1_000)
            .unwrap_err()
            .contains("in the past"));
        // exactly now is still the past (must be strictly future)
        assert!(create_in(d.path(), "/ws", "p", 1_000, None, None, None, None, None, "x", 1_000)
            .unwrap_err()
            .contains("in the past"));
        // beyond the 365-day horizon
        let too_far = 1_000 + MAX_HORIZON_MS + 1;
        assert!(create_in(d.path(), "/ws", "p", too_far, None, None, None, None, None, "x", 1_000)
            .unwrap_err()
            .contains("horizon"));
    }

    /// The whole point of a schedule vs a loop: it fires however far out, with no
    /// 7-day expiry. A record set ~11 months ahead is valid and stays claimable
    /// at its fire time.
    #[test]
    fn a_schedule_far_beyond_the_loop_7day_expiry_is_valid() {
        let d = dir();
        let eleven_months = 330 * 86_400_000u64;
        let rec = make(d.path(), "far", 0, eleven_months);
        assert_eq!(rec.status, ScheduleStatus::Pending);
        assert!(rec.is_claimable(eleven_months));
    }

    #[test]
    fn roundtrip_list_get_cancel() {
        let d = dir();
        make(d.path(), "s1", 0, 2_000);
        make(d.path(), "s2", 0, 1_000);
        let listed = list_in(d.path());
        assert_eq!(listed.len(), 2);
        // soonest-due first
        assert_eq!(listed[0].id, "s2");
        assert_eq!(get_in(d.path(), "s1").unwrap().prompt, "check the deploy");
        // camelCase on the wire, like every other Fleet record
        let raw = fs::read_to_string(record_path(d.path(), "s1")).unwrap();
        assert!(raw.contains("\"workspacePath\""));
        assert!(raw.contains("\"fireAt\""));
        assert!(raw.contains("\"status\": \"pending\""));

        assert!(cancel_in(d.path(), "s1"));
        assert!(get_in(d.path(), "s1").is_none());
        assert_eq!(list_in(d.path()).len(), 1);
        // idempotent
        assert!(!cancel_in(d.path(), "s1"));
    }

    #[test]
    fn claim_flips_to_fired_and_bumps_generation() {
        let d = dir();
        make(d.path(), "s1", 0, 300_000);
        let claimed = claim_fire_in(d.path(), "s1", 0, 300_000).unwrap();
        assert_eq!(claimed.status, ScheduleStatus::Fired);
        assert_eq!(claimed.generation, 1);
        assert_eq!(claimed.fired_at, Some(300_000));

        // persisted, not just returned — and kept as history, not deleted
        let on_disk = get_in(d.path(), "s1").unwrap();
        assert_eq!(on_disk.status, ScheduleStatus::Fired);
        assert_eq!(on_disk.generation, 1);
    }

    /// Exactly-once: a second timer that already fired the schedule (or woke late)
    /// must be refused, so the one-shot never spawns two sessions.
    #[test]
    fn a_second_claim_is_refused_as_already_fired() {
        let d = dir();
        make(d.path(), "s1", 0, 300_000);
        assert!(claim_fire_in(d.path(), "s1", 0, 300_000).is_ok());
        let err = claim_fire_in(d.path(), "s1", 1, 300_000).unwrap_err();
        assert_eq!(err, ClaimError::AlreadyFired);
    }

    /// A stale timer holding the pre-claim generation is refused before firing.
    #[test]
    fn a_stale_generation_is_refused() {
        let d = dir();
        // simulate a record re-armed by reconcile to generation 2
        let mut rec = make(d.path(), "s1", 0, 300_000);
        rec.generation = 2;
        write_record(d.path(), &rec).unwrap();
        let err = claim_fire_in(d.path(), "s1", 0, 300_000).unwrap_err();
        assert_eq!(err, ClaimError::StaleGeneration { expected: 0, found: 2 });
    }

    #[test]
    fn a_cancelled_schedule_cannot_be_claimed() {
        let d = dir();
        make(d.path(), "s1", 0, 300_000);
        cancel_in(d.path(), "s1");
        assert_eq!(
            claim_fire_in(d.path(), "s1", 0, 300_000).unwrap_err(),
            ClaimError::Gone
        );
    }

    #[test]
    fn claiming_early_is_refused() {
        let d = dir();
        make(d.path(), "s1", 0, 300_000);
        let err = claim_fire_in(d.path(), "s1", 0, 100_000).unwrap_err();
        assert_eq!(err, ClaimError::NotDue { due_in_ms: 200_000 });
    }

    #[test]
    fn due_schedules_selects_only_pending_and_due() {
        let d = dir();
        make(d.path(), "soon", 0, 300_000);
        make(d.path(), "later", 0, 3_600_000);
        // a fired one must not appear even though it's "due"
        let mut fired = make(d.path(), "done", 0, 100_000);
        fired.status = ScheduleStatus::Fired;
        write_record(d.path(), &fired).unwrap();

        let due = due_schedules_in(d.path(), 400_000);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, "soon");
    }

    use std::cell::RefCell;

    #[derive(Default, Clone)]
    struct SpawnCall {
        agent_source: String,
        workspace: String,
        prompt: String,
        model: Option<String>,
        entrypoint: String,
    }

    fn ok_spawner<'a>(
        calls: &'a RefCell<Vec<SpawnCall>>,
        sid: &'a str,
    ) -> impl Fn(&str, &str, &str, Option<&str>, Option<&str>, Option<&str>, &str)
        -> Result<crate::session_launch::SpawnSessionResponse, String> + 'a {
        move |source, ws, prompt, model, _effort, _perm, ep| {
            calls.borrow_mut().push(SpawnCall {
                agent_source: source.to_string(),
                workspace: ws.to_string(),
                prompt: prompt.to_string(),
                model: model.map(str::to_string),
                entrypoint: ep.to_string(),
            });
            Ok(crate::session_launch::SpawnSessionResponse {
                pid: 1,
                session_id: Some(sid.to_string()),
            })
        }
    }

    #[test]
    fn fire_once_claims_spawns_and_records_the_session() {
        let d = dir();
        create_in(
            d.path(),
            "/ws",
            "check the deploy",
            300_000,
            Some("claude-fable-5"),
            Some("high"),
            None,
            None,
            None,
            "s1",
            0,
        )
        .unwrap();

        let calls = RefCell::new(Vec::new());
        let claimed = fire_once_in(d.path(), "s1", 0, 300_000, &ok_spawner(&calls, "fired-sid")).unwrap();
        assert_eq!(claimed.status, ScheduleStatus::Fired);

        let calls = calls.into_inner();
        assert_eq!(calls.len(), 1, "exactly one session spawned");
        assert_eq!(calls[0].workspace, "/ws");
        assert_eq!(calls[0].entrypoint, SCHEDULE_ENTRYPOINT);
        assert_eq!(calls[0].model.as_deref(), Some("claude-fable-5"));
        assert!(calls[0].prompt.contains("check the deploy"));
        assert!(calls[0].prompt.contains("fleet schedule list"));

        // the produced session id is stamped on the fired record (history)
        assert_eq!(
            get_in(d.path(), "s1").unwrap().fired_session_id.as_deref(),
            Some("fired-sid")
        );
    }

    /// The claim happens before the spawn; a stale/second timer must NOT spawn.
    #[test]
    fn a_stale_timer_fires_nothing() {
        let d = dir();
        make(d.path(), "s1", 0, 300_000);
        let calls = RefCell::new(Vec::new());
        fire_once_in(d.path(), "s1", 0, 300_000, &ok_spawner(&calls, "s1sid")).unwrap();
        // second timer still holding gen 0: the record is now fired (gen 1)
        let err = fire_once_in(d.path(), "s1", 0, 300_000, &ok_spawner(&calls, "s2sid")).unwrap_err();
        assert_eq!(err, ClaimError::AlreadyFired);
        assert_eq!(calls.into_inner().len(), 1, "the stale timer must not spawn");
    }

    /// A spawn failure must not un-fire: the record stays `Fired` so no timer
    /// re-arms and retries a broken prompt forever.
    #[test]
    fn a_spawn_failure_still_marks_fired() {
        let d = dir();
        make(d.path(), "s1", 0, 300_000);
        let failing = |_s: &str, _w: &str, _p: &str, _m: Option<&str>, _e: Option<&str>, _pm: Option<&str>, _ep: &str|
            -> Result<crate::session_launch::SpawnSessionResponse, String> { Err("boom".into()) };
        let claimed = fire_once_in(d.path(), "s1", 0, 300_000, &failing).unwrap();
        assert_eq!(claimed.status, ScheduleStatus::Fired);
        assert_eq!(get_in(d.path(), "s1").unwrap().status, ScheduleStatus::Fired);
    }

    #[test]
    fn codex_schedule_fires_on_codex_source() {
        let d = dir();
        create_in(d.path(), "/ws", "p", 300_000, None, None, Some("codex"), None, None, "cx", 0).unwrap();
        let calls = RefCell::new(Vec::new());
        fire_once_in(d.path(), "cx", 0, 300_000, &ok_spawner(&calls, "cx-sid")).unwrap();
        assert_eq!(calls.into_inner()[0].agent_source, "codex");
    }

    #[test]
    fn schedule_without_source_defaults_to_claude() {
        let d = dir();
        create_in(d.path(), "/ws", "p", 300_000, None, None, None, None, None, "cl", 0).unwrap();
        let calls = RefCell::new(Vec::new());
        fire_once_in(d.path(), "cl", 0, 300_000, &ok_spawner(&calls, "cl-sid")).unwrap();
        assert_eq!(calls.into_inner()[0].agent_source, "claude");
    }

    #[test]
    fn fire_prompt_names_the_schedule_and_history_switch() {
        let d = dir();
        let rec = make(d.path(), "abc123", 0, 300_000);
        let p = compose_fire_prompt(&rec);
        assert!(p.contains("check the deploy"));
        assert!(p.contains("abc123"));
        assert!(p.contains("fleet schedule list"));
        // The fired session is unattended — the footer must tell the agent to
        // finish silently instead of popping an unanswerable decision card.
        assert!(p.contains("无人值守"));
        assert!(p.contains("不要"));
        assert!(p.contains("fleet__ask"));
    }

    /// reconcile re-arms a pending schedule the machine slept through (overdue
    /// past the grace window) but leaves healthy, near-due, and fired ones alone.
    #[test]
    fn reconcile_rearms_only_stranded_pending_schedules() {
        let d = dir();
        // healthy: due in the future — a timer is presumably driving it
        make(d.path(), "healthy", 1_000_000, 1_000_000 + 300_000);
        // napping: just barely overdue — inside grace, timer likely mid-nap
        let mut napping = make(d.path(), "napping", 0, 300_000);
        napping.fire_at = 1_000_000 - 10_000;
        write_record(d.path(), &napping).unwrap();
        // stranded: overdue well past grace — its timer is dead
        let mut stranded = make(d.path(), "stranded", 0, 300_000);
        stranded.fire_at = 1_000_000 - STRANDED_GRACE_MS - 5_000;
        write_record(d.path(), &stranded).unwrap();
        // fired: overdue but already ran — must never re-arm
        let mut fired = make(d.path(), "fired", 0, 300_000);
        fired.fire_at = 0;
        fired.status = ScheduleStatus::Fired;
        write_record(d.path(), &fired).unwrap();

        let mut armed = Vec::new();
        let rearmed = reconcile_in(d.path(), 1_000_000, &mut |r| armed.push(r.id.clone()));
        assert_eq!(rearmed, vec!["stranded"], "only the stranded pending schedule is re-armed");
        assert_eq!(armed, vec!["stranded"]);
    }

    /// Build a `ScheduleUpdate` for a given id with only the fields a test sets.
    fn upd(id: &str) -> ScheduleUpdate {
        ScheduleUpdate {
            id: id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn update_changes_fields_and_bumps_generation() {
        let d = dir();
        make(d.path(), "s1", 0, 300_000);
        let updated = update_in(
            d.path(),
            &ScheduleUpdate {
                fire_at: Some(900_000),
                prompt: Some("new prompt".into()),
                ..upd("s1")
            },
            0,
        )
        .unwrap();
        assert_eq!(updated.fire_at, 900_000);
        assert_eq!(updated.prompt, "new prompt");
        assert_eq!(updated.generation, 1, "bump supersedes the old timer");
        // persisted
        let on_disk = get_in(d.path(), "s1").unwrap();
        assert_eq!(on_disk.fire_at, 900_000);
        assert_eq!(on_disk.generation, 1);
    }

    #[test]
    fn update_partial_leaves_other_fields() {
        let d = dir();
        make(d.path(), "s1", 0, 300_000);
        // only the prompt
        let u = update_in(
            d.path(),
            &ScheduleUpdate {
                prompt: Some("just prompt".into()),
                ..upd("s1")
            },
            0,
        )
        .unwrap();
        assert_eq!(u.fire_at, 300_000, "fire time unchanged");
        assert_eq!(u.prompt, "just prompt");
    }

    /// model / effort / agent_source: `Some(x)` sets, `Some("")` clears to
    /// inherit-default (`None`), `None` leaves the field untouched.
    #[test]
    fn update_sets_and_clears_spawn_knobs() {
        let d = dir();
        create_in(
            d.path(),
            "/ws",
            "p",
            300_000,
            Some("claude-opus-4-8"),
            Some("high"),
            Some("claude"),
            None,
            None,
            "s1",
            0,
        )
        .unwrap();
        // set model+effort+source to new values
        let u = update_in(
            d.path(),
            &ScheduleUpdate {
                model: Some("claude-fable-5".into()),
                effort: Some("max".into()),
                agent_source: Some("codex".into()),
                ..upd("s1")
            },
            0,
        )
        .unwrap();
        assert_eq!(u.model.as_deref(), Some("claude-fable-5"));
        assert_eq!(u.effort.as_deref(), Some("max"));
        assert_eq!(u.agent_source.as_deref(), Some("codex"));
        // blank clears back to inherit-default; None leaves effort untouched
        let u = update_in(
            d.path(),
            &ScheduleUpdate {
                model: Some("  ".into()),
                agent_source: Some("".into()),
                ..upd("s1")
            },
            0,
        )
        .unwrap();
        assert_eq!(u.model, None, "blank model clears to inherit-default");
        assert_eq!(u.effort.as_deref(), Some("max"), "effort left untouched");
        assert_eq!(u.agent_source, None, "blank source clears to inherit-default");
    }

    #[test]
    fn update_rejects_fired_past_and_beyond_horizon() {
        let d = dir();
        let mut fired = make(d.path(), "f", 0, 300_000);
        fired.status = ScheduleStatus::Fired;
        write_record(d.path(), &fired).unwrap();
        assert!(update_in(
            d.path(),
            &ScheduleUpdate { fire_at: Some(900_000), ..upd("f") },
            0
        )
        .unwrap_err()
        .contains("already fired"));

        make(d.path(), "s1", 0, 300_000);
        assert!(update_in(
            d.path(),
            &ScheduleUpdate { fire_at: Some(500), ..upd("s1") },
            1_000
        )
        .unwrap_err()
        .contains("in the past"));
        assert!(update_in(
            d.path(),
            &ScheduleUpdate { fire_at: Some(1_000 + MAX_HORIZON_MS + 1), ..upd("s1") },
            1_000
        )
        .unwrap_err()
        .contains("horizon"));
        // empty prompt rejected
        assert!(update_in(
            d.path(),
            &ScheduleUpdate { prompt: Some("   ".into()), ..upd("s1") },
            0
        )
        .unwrap_err()
        .contains("cannot be empty"));
        // unknown id
        assert!(update_in(
            d.path(),
            &ScheduleUpdate { prompt: Some("x".into()), ..upd("nope") },
            0
        )
        .unwrap_err()
        .contains("no schedule with id"));
    }

    #[test]
    fn parse_in_reuses_the_interval_grammar_and_floor() {
        assert_eq!(parse_in("5d", 1_000).unwrap(), 1_000 + 5 * 86_400 * 1000);
        assert_eq!(parse_in("90m", 0).unwrap(), 90 * 60 * 1000);
        assert_eq!(parse_in("2h", 0).unwrap(), 2 * 3600 * 1000);
        // the 60-second floor is inherited from agent_loop::parse_interval
        assert!(parse_in("10s", 0).unwrap_err().contains("minimum"));
    }

    #[test]
    fn parse_at_absolute_matches_local_interpretation() {
        use chrono::{Local, NaiveDateTime, TimeZone};
        for s in ["2026-07-25 09:30", "2026-07-25T09:30", "2026-07-25 09:30:15"] {
            let got = parse_at(s, 0).unwrap();
            let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
                .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M"))
                .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M"))
                .unwrap();
            let want = Local.from_local_datetime(&naive).single().unwrap().timestamp_millis() as u64;
            assert_eq!(got, want, "parse_at({s}) must match local-tz interpretation");
        }
    }

    #[test]
    fn parse_at_bare_time_is_always_in_the_future() {
        use chrono::{Duration, Local, TimeZone};
        let now = 1_770_000_000_000u64; // fixed, mid-range epoch ms
        let now_local = Local.timestamp_millis_opt(now as i64).single().unwrap();
        // a time-of-day one hour ago must roll to tomorrow
        let earlier = (now_local - Duration::hours(1)).format("%H:%M").to_string();
        let got = parse_at(&earlier, now).unwrap();
        assert!(got > now, "past time-of-day must roll forward");
        assert!(got <= now + 24 * 3600 * 1000);
        // a time-of-day two hours ahead stays today
        let later = (now_local + Duration::hours(2)).format("%H:%M").to_string();
        let got = parse_at(&later, now).unwrap();
        assert!(got > now && got <= now + 3 * 3600 * 1000);
    }

    #[test]
    fn parse_at_rejects_garbage_and_empty() {
        assert!(parse_at("", 0).unwrap_err().contains("required"));
        assert!(parse_at("not-a-date", 0).unwrap_err().contains("cannot parse"));
    }

    #[test]
    fn model_and_effort_are_carried_on_the_record() {
        let d = dir();
        let rec = create_in(
            d.path(),
            "/ws",
            "p",
            300_000,
            Some("claude-fable-5"),
            Some("high"),
            None,
            None,
            None,
            "s1",
            0,
        )
        .unwrap();
        assert_eq!(rec.model.as_deref(), Some("claude-fable-5"));
        assert_eq!(rec.effort.as_deref(), Some("high"));
        // blank strings are not a model
        let rec = create_in(d.path(), "/ws", "p", 300_000, Some("  "), Some(""), None, None, None, "s2", 0)
            .unwrap();
        assert_eq!(rec.model, None);
        assert_eq!(rec.effort, None);
    }

    #[test]
    fn run_now_spawns_but_leaves_the_record_untouched() {
        let d = dir();
        make(d.path(), "s1", 1_000_000, 1_000_000 + 300_000);
        let before = get_in(d.path(), "s1").unwrap();

        let calls = RefCell::new(Vec::new());
        let sid = run_now_in(d.path(), "s1", &ok_spawner(&calls, "manual-sid")).unwrap();
        assert_eq!(sid.as_deref(), Some("manual-sid"));

        // a session was spawned with the schedule's own params + entrypoint
        let calls = calls.into_inner();
        assert_eq!(calls.len(), 1, "exactly one session spawned");
        assert_eq!(calls[0].workspace, "/ws");
        assert_eq!(calls[0].entrypoint, SCHEDULE_ENTRYPOINT);
        assert!(calls[0].prompt.contains("check the deploy"));
        assert!(calls[0].prompt.contains("手动运行"), "run-now footer");
        // A manual run has 老板 present — it is NOT exempt from the decision
        // card, so its footer must NOT carry the unattended marker.
        assert!(!calls[0].prompt.contains("无人值守"), "manual run stays interactive");

        // the record is byte-for-byte unchanged: still pending, no generation
        // bump, no fired stamp — the scheduled fire is untouched.
        let after = get_in(d.path(), "s1").unwrap();
        assert_eq!(after, before, "run_now must not mutate the schedule record");
        assert_eq!(after.status, ScheduleStatus::Pending);
        assert!(after.fired_session_id.is_none());
    }

    #[test]
    fn run_now_on_missing_schedule_errors() {
        let d = dir();
        let calls = RefCell::new(Vec::new());
        let err = run_now_in(d.path(), "nope", &ok_spawner(&calls, "x")).unwrap_err();
        assert!(err.contains("no schedule with id nope"), "got: {err}");
        assert_eq!(calls.into_inner().len(), 0, "no spawn for a missing schedule");
    }

    // ── gate (--until) ───────────────────────────────────────────────────────

    /// A gate is stamped onto the record; its poll floor is applied; a blank
    /// gate command is dropped (no gate at all).
    #[test]
    fn create_stamps_gate_and_clamps_poll_floor() {
        let d = dir();
        let rec = create_in(
            d.path(), "/ws", "p", 300_000, None, None, None, None,
            Some(gate("test -f /tmp/ready", 1, 3600)), // poll below the 5s floor
            "g1", 0,
        )
        .unwrap();
        assert!(rec.has_gate());
        assert_eq!(rec.until_cmd.as_deref(), Some("test -f /tmp/ready"));
        assert_eq!(rec.poll_secs, Some(crate::watch::MIN_POLL_SECS), "poll clamps up to floor");
        assert_eq!(rec.gate_timeout_secs, Some(3600));
        // give-up deadline is measured from the due time
        assert_eq!(rec.gate_deadline(), Some(300_000 + 3600 * 1000));

        // a blank gate command ⇒ no gate
        let rec = create_in(
            d.path(), "/ws", "p", 300_000, None, None, None, None,
            Some(gate("   ", 30, 3600)), "g2", 0,
        )
        .unwrap();
        assert!(!rec.has_gate());
        assert_eq!(rec.gate_deadline(), None);
    }

    /// gate fields survive the JSON round-trip in camelCase.
    #[test]
    fn gate_fields_serialize_camelcase() {
        let d = dir();
        create_in(
            d.path(), "/ws", "p", 300_000, None, None, None, None,
            Some(gate("true", 30, 3600)), "g1", 0,
        )
        .unwrap();
        let raw = fs::read_to_string(record_path(d.path(), "g1")).unwrap();
        assert!(raw.contains("\"untilCmd\""));
        assert!(raw.contains("\"pollSecs\""));
        assert!(raw.contains("\"gateTimeoutSecs\""));
        // gate_timed_out is false by default and skipped from the wire
        assert!(!raw.contains("gateTimedOut"));
    }

    /// No gate: a due schedule fires immediately (classic behaviour); the gate
    /// closure is never even consulted.
    #[test]
    fn decide_without_gate_fires_when_due() {
        let d = dir();
        let rec = make(d.path(), "s1", 0, 300_000);
        // not due ⇒ nap toward the due time, capped at POLL_CAP (30s)
        assert_eq!(decide(&rec, 0, 100_000, || panic!("gate must not be polled")), SchedStep::Nap { ms: 30_000 });
        // due ⇒ fire, gate never consulted
        assert_eq!(decide(&rec, 0, 300_000, || panic!("gate must not be polled")), SchedStep::Fire);
    }

    /// Gated + due: naps while the gate is unmet, fires the moment it's met. The
    /// nap is bounded by the poll interval.
    #[test]
    fn decide_gated_naps_until_met_then_fires() {
        let d = dir();
        let rec = create_in(
            d.path(), "/ws", "p", 300_000, None, None, None, None,
            Some(gate("gate", 30, 3600)), "g1", 0,
        )
        .unwrap();
        // due, gate not met ⇒ nap one poll interval (30s), never past deadline
        assert_eq!(decide(&rec, 0, 300_000, || false), SchedStep::Nap { ms: 30_000 });
        // due, gate met ⇒ fire
        assert_eq!(decide(&rec, 0, 300_000, || true), SchedStep::Fire);
        // not due yet ⇒ nap to due, gate never polled
        assert_eq!(decide(&rec, 0, 100_000, || panic!("early: no poll")), SchedStep::Nap { ms: 30_000 });
    }

    /// Gated: once the give-up window has elapsed past the due time, abandon —
    /// even if the gate could be met, the deadline check comes first.
    #[test]
    fn decide_gated_abandons_past_deadline() {
        let d = dir();
        let rec = create_in(
            d.path(), "/ws", "p", 300_000, None, None, None, None,
            Some(gate("gate", 30, 60)), "g1", 0, // 60s window past due (300_000)
        )
        .unwrap();
        let deadline = 300_000 + 60_000;
        // just before the deadline, still polling
        assert_eq!(decide(&rec, 0, deadline - 1, || false), SchedStep::Nap { ms: 1 });
        // at/after the deadline, abandon regardless of gate
        assert_eq!(decide(&rec, 0, deadline, || true), SchedStep::Abandon);
        assert_eq!(decide(&rec, 0, deadline + 5_000, || false), SchedStep::Abandon);
    }

    /// The abandon path claims the schedule (flips Fired) and stamps
    /// `gate_timed_out`, but spawns nothing — the chosen timeout semantics.
    #[test]
    fn abandon_marks_fired_timed_out() {
        let d = dir();
        create_in(
            d.path(), "/ws", "p", 300_000, None, None, None, None,
            Some(gate("gate", 30, 60)), "g1", 0,
        )
        .unwrap();
        let claimed = abandon_on_timeout_in(d.path(), "g1", 0, 400_000).unwrap();
        assert_eq!(claimed.status, ScheduleStatus::Fired);
        assert!(claimed.gate_timed_out, "abandoned schedule is stamped timed-out");
        assert!(claimed.fired_session_id.is_none(), "abandon spawns nothing");
        // persisted, kept as history
        let on_disk = get_in(d.path(), "g1").unwrap();
        assert!(on_disk.gate_timed_out);
        assert_eq!(on_disk.status, ScheduleStatus::Fired);
    }

    /// A normal fire leaves `gate_timed_out` false, distinguishing it in history
    /// from an abandoned one.
    #[test]
    fn a_normal_fire_is_not_timed_out() {
        let d = dir();
        make(d.path(), "s1", 0, 300_000);
        let claimed = claim_fire_in(d.path(), "s1", 0, 300_000).unwrap();
        assert!(!claimed.gate_timed_out);
    }
}
