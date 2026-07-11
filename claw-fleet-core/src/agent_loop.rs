//! Agent loop — a recurring prompt that survives the turn boundary.
//!
//! Claude Code ships three ways to say "do this again later" — `CronCreate`,
//! `ScheduleWakeup` and the `/loop` skill — and in a Fleet session all three are
//! dead ends. They are in-process timers scoped to the REPL (`CronCreate` says so
//! itself: "Session-only (not written to disk, dies when Claude exits)"), and
//! every Fleet session is a headless `claude -p` run that exits the instant its
//! turn ends. The tool reports the job as scheduled; nothing ever fires. See
//! [`crate::bg_guard`], which now refuses that stop rather than letting the agent
//! walk into it.
//!
//! This module is the mechanism that does work. A loop is a *delayed, repeating
//! handoff to yourself*: the schedule lives on disk, and each iteration spawns a
//! brand-new session in the workspace — so nothing depends on the previous
//! session's process still being alive.
//!
//! Firing follows [`crate::handoff`]'s design rather than the desktop app's:
//! `fleet` itself arms a detached timer process, which sleeps and then spawns the
//! iteration. The hook runs on the machine the session lives on, so local and
//! remote workspaces behave identically, and a loop keeps ticking with the Fleet
//! desktop app closed. See `fleet loop fire` for the timer side.
//!
//! File layout: `~/.fleet/loops/<loop_id>.json`, one record per loop.
//!
//! ## Why `generation`
//!
//! A timer process sleeps for minutes. In that window the loop may be stopped, or
//! re-armed by another path, leaving two timers racing for the same fire. Every
//! record carries a `generation` that [`claim_fire`] bumps as it hands out an
//! iteration; a timer that wakes holding a stale generation — or finds the record
//! gone — exits without spawning. The claim is the single serialization point, so
//! a loop cannot double-fire no matter how many timers are armed.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Hard ceiling on iterations, whatever the user asked for. A loop is an agent
/// spawning agents; it must have a floor under it that a typo can't remove.
pub const MAX_ITERATIONS: u32 = 500;

/// Shortest interval we accept. Sessions cost tokens and a sub-minute loop is
/// almost always a mistake for a prompt that spawns a whole agent.
pub const MIN_INTERVAL_SECS: u64 = 60;

/// A loop untouched for this long is abandoned: `fleet loop list` still shows it,
/// but a timer refuses to fire it. Mirrors Claude Code's own 7-day cron expiry.
pub const EXPIRY_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// One registered loop.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LoopRecord {
    pub id: String,
    /// Workspace each iteration is spawned in.
    pub workspace_path: String,
    /// The prompt every iteration runs.
    pub prompt: String,
    pub interval_secs: u64,
    /// Epoch ms of the next scheduled spawn.
    pub next_fire_at: u64,
    pub iterations_done: u32,
    /// `None` = run until stopped (still bounded by [`MAX_ITERATIONS`]).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_iterations: Option<u32>,
    /// Model each iteration runs on — inherited from the session that created the
    /// loop, so a fable-5 loop doesn't silently wake up as opus.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<String>,
    /// Bumped on every claim; stale timers exit without firing. See module docs.
    pub generation: u64,
    pub created: u64,
    /// Session that registered the loop, for provenance.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub created_by_session: Option<String>,
    /// Most recent iteration Fleet spawned.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_session_id: Option<String>,
}

impl LoopRecord {
    /// Iterations remain and the record hasn't rotted.
    pub fn is_live(&self, now: u64) -> bool {
        if now.saturating_sub(self.created) > EXPIRY_MS {
            return false;
        }
        self.remaining_iterations() > 0
    }

    fn remaining_iterations(&self) -> u32 {
        let cap = self.max_iterations.unwrap_or(MAX_ITERATIONS).min(MAX_ITERATIONS);
        cap.saturating_sub(self.iterations_done)
    }

    /// Milliseconds until this loop is due (0 when it already is).
    pub fn due_in_ms(&self, now: u64) -> u64 {
        self.next_fire_at.saturating_sub(now)
    }

    pub fn is_due(&self, now: u64) -> bool {
        self.next_fire_at <= now
    }
}

/// `5m`, `90s`, `2h`, `1d` → seconds. Bare digits are read as seconds.
///
/// Rejects anything below [`MIN_INTERVAL_SECS`]: a 10-second loop spawns six
/// agent sessions a minute, which is never what the user meant.
pub fn parse_interval(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("interval is required (e.g. 5m, 30m, 2h)".to_string());
    }
    let (digits, unit) = match s.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => (&s[..i], &s[i..]),
        None => (s, ""),
    };
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("cannot parse interval '{s}' (expected e.g. 5m, 30m, 2h)"))?;
    let secs = match unit.to_ascii_lowercase().as_str() {
        "" | "s" | "sec" | "secs" => n,
        "m" | "min" | "mins" => n * 60,
        "h" | "hr" | "hrs" => n * 3600,
        "d" | "day" | "days" => n * 86400,
        other => return Err(format!("unknown interval unit '{other}' (use s/m/h/d)")),
    };
    if secs < MIN_INTERVAL_SECS {
        return Err(format!(
            "interval {secs}s is below the {MIN_INTERVAL_SECS}s minimum — \
             each iteration spawns a whole agent session"
        ));
    }
    Ok(secs)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// `~/.fleet/loops`.
pub fn loops_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("loops"))
}

fn record_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn write_record(dir: &Path, rec: &LoopRecord) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create loops dir: {e}"))?;
    let json = serde_json::to_string_pretty(rec).map_err(|e| format!("serialize loop: {e}"))?;
    fs::write(record_path(dir, &rec.id), json).map_err(|e| format!("write loop: {e}"))
}

// ── create / read / stop ──────────────────────────────────────────────────────

/// Register a loop. The first iteration fires one full interval from now — a loop
/// is "do this again later", and the session creating it is already doing it now.
#[allow(clippy::too_many_arguments)]
pub fn create(
    workspace_path: &str,
    prompt: &str,
    interval_secs: u64,
    max_iterations: Option<u32>,
    model: Option<&str>,
    effort: Option<&str>,
    created_by_session: Option<&str>,
) -> Result<LoopRecord, String> {
    let dir = loops_dir().ok_or("cannot determine home dir")?;
    create_in(
        &dir,
        workspace_path,
        prompt,
        interval_secs,
        max_iterations,
        model,
        effort,
        created_by_session,
        &uuid::Uuid::new_v4().to_string()[..8],
        now_ms(),
    )
}

#[allow(clippy::too_many_arguments)]
fn create_in(
    dir: &Path,
    workspace_path: &str,
    prompt: &str,
    interval_secs: u64,
    max_iterations: Option<u32>,
    model: Option<&str>,
    effort: Option<&str>,
    created_by_session: Option<&str>,
    id: &str,
    now: u64,
) -> Result<LoopRecord, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("loop prompt is required".to_string());
    }
    if interval_secs < MIN_INTERVAL_SECS {
        return Err(format!(
            "interval {interval_secs}s is below the {MIN_INTERVAL_SECS}s minimum"
        ));
    }
    if let Some(0) = max_iterations {
        return Err("max iterations must be at least 1".to_string());
    }
    let blank = |s: &str| s.trim().is_empty();
    let rec = LoopRecord {
        id: id.to_string(),
        workspace_path: workspace_path.to_string(),
        prompt: prompt.to_string(),
        interval_secs,
        next_fire_at: now + interval_secs * 1000,
        iterations_done: 0,
        max_iterations: max_iterations.map(|m| m.min(MAX_ITERATIONS)),
        model: model.filter(|m| !blank(m)).map(str::to_string),
        effort: effort.filter(|e| !blank(e)).map(str::to_string),
        generation: 0,
        created: now,
        created_by_session: created_by_session.map(str::to_string),
        last_session_id: None,
    };
    write_record(dir, &rec)?;
    Ok(rec)
}

pub fn get(id: &str) -> Option<LoopRecord> {
    get_in(&loops_dir()?, id)
}

fn get_in(dir: &Path, id: &str) -> Option<LoopRecord> {
    let s = fs::read_to_string(record_path(dir, id)).ok()?;
    serde_json::from_str(&s).ok()
}

/// Every registered loop, soonest-due first.
pub fn list() -> Vec<LoopRecord> {
    let Some(dir) = loops_dir() else {
        return Vec::new();
    };
    list_in(&dir)
}

fn list_in(dir: &Path) -> Vec<LoopRecord> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<LoopRecord> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| {
            let s = fs::read_to_string(e.path()).ok()?;
            serde_json::from_str(&s).ok()
        })
        .collect();
    out.sort_by_key(|r| r.next_fire_at);
    out
}

/// Stop a loop. Idempotent — returns whether a record was actually removed.
///
/// Any timer already sleeping on this loop will find the record gone when it
/// wakes and exit without firing, so no process needs to be killed.
pub fn stop(id: &str) -> bool {
    let Some(dir) = loops_dir() else {
        return false;
    };
    stop_in(&dir, id)
}

fn stop_in(dir: &Path, id: &str) -> bool {
    fs::remove_file(record_path(dir, id)).is_ok()
}

/// Record which session an iteration produced, for `fleet loop list`.
pub fn record_iteration_session(id: &str, session_id: &str) {
    let Some(dir) = loops_dir() else { return };
    record_iteration_session_in(&dir, id, session_id);
}

fn record_iteration_session_in(dir: &Path, id: &str, session_id: &str) {
    if let Some(mut rec) = get_in(dir, id) {
        rec.last_session_id = Some(session_id.to_string());
        let _ = write_record(dir, &rec);
    }
}

// ── firing ────────────────────────────────────────────────────────────────────

/// Why a claim was refused, so the timer process can log something truthful.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimError {
    /// The loop was stopped while the timer slept.
    Gone,
    /// Another timer already claimed this fire (or re-armed the loop).
    StaleGeneration { expected: u64, found: u64 },
    /// Woke early — not due yet.
    NotDue { due_in_ms: u64 },
    /// Iteration cap reached, or the record is older than [`EXPIRY_MS`].
    Exhausted,
}

impl std::fmt::Display for ClaimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gone => write!(f, "loop was stopped"),
            Self::StaleGeneration { expected, found } => write!(
                f,
                "stale timer (held generation {expected}, record is at {found})"
            ),
            Self::NotDue { due_in_ms } => write!(f, "not due for another {due_in_ms}ms"),
            Self::Exhausted => write!(f, "loop is exhausted or expired"),
        }
    }
}

/// Atomically take the next iteration of a loop.
///
/// This is the only place a fire is granted, and it both bumps `generation` and
/// advances `next_fire_at` before returning — so a second timer holding the old
/// generation is refused, and a crash between claim and spawn costs one iteration
/// rather than looping forever on a failing prompt.
///
/// Returns the record *as claimed* (with the new generation and the next due
/// time already written), so the caller can spawn the iteration and then arm the
/// next timer straight from it.
pub fn claim_fire(id: &str, generation: u64) -> Result<LoopRecord, ClaimError> {
    let dir = loops_dir().ok_or(ClaimError::Gone)?;
    claim_fire_in(&dir, id, generation, now_ms())
}

fn claim_fire_in(
    dir: &Path,
    id: &str,
    generation: u64,
    now: u64,
) -> Result<LoopRecord, ClaimError> {
    let mut rec = get_in(dir, id).ok_or(ClaimError::Gone)?;
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
    if !rec.is_live(now) {
        return Err(ClaimError::Exhausted);
    }

    rec.iterations_done += 1;
    rec.generation += 1;
    // Schedule from `now`, not from the missed `next_fire_at`: a loop that was
    // late (laptop asleep, long iteration) must not then fire a burst of catch-up
    // iterations back to back.
    rec.next_fire_at = now + rec.interval_secs * 1000;

    if rec.is_live(now) {
        write_record(dir, &rec).map_err(|_| ClaimError::Gone)?;
    } else {
        // Final iteration — retire the record so no timer re-arms on it.
        stop_in(dir, id);
    }
    Ok(rec)
}

/// Loops that are due now and still live — the reconcile sweep's input, for
/// fires missed while the machine was asleep or Fleet wasn't running.
pub fn due_loops(now: u64) -> Vec<LoopRecord> {
    let Some(dir) = loops_dir() else {
        return Vec::new();
    };
    due_loops_in(&dir, now)
}

fn due_loops_in(dir: &Path, now: u64) -> Vec<LoopRecord> {
    list_in(dir)
        .into_iter()
        .filter(|r| r.is_due(now) && r.is_live(now))
        .collect()
}

// ── iteration spawn + timer ────────────────────────────────────────────────────

/// `CLAUDE_CODE_ENTRYPOINT` for loop iterations, so the scanner and transcript
/// readers can tell an auto-spawned iteration from a user-initiated session.
pub const LOOP_ENTRYPOINT: &str = "claw-fleet-loop";

/// The prompt each iteration runs: the user's prompt, plus a short footer naming
/// the loop and how to stop it, so the agent isn't confused about why it woke up
/// and has the off switch in hand.
pub fn compose_iteration_prompt(rec: &LoopRecord) -> String {
    let mut out = String::new();
    out.push_str(&rec.prompt);
    out.push_str("\n\n---\n");
    out.push_str(&format!(
        "（这是 Fleet 循环 `{}` 的第 {} 次迭代，每 {} 触发一次。\
         Fleet 会在 {} 后自动拉起下一次，你不需要注册任何 cron 或 wakeup——\
         那些在 headless 会话里不会触发。要停止这个循环用 `fleet loop stop {}`。）",
        rec.id,
        rec.iterations_done,
        humanize_secs(rec.interval_secs),
        humanize_secs(rec.interval_secs),
        rec.id,
    ));
    out
}

fn humanize_secs(secs: u64) -> String {
    if secs % 86400 == 0 {
        format!("{}d", secs / 86400)
    } else if secs % 3600 == 0 {
        format!("{}h", secs / 3600)
    } else if secs % 60 == 0 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// Signature of the session spawner, matching
/// [`crate::session_launch::spawn_new_session_with_entrypoint`] — injected so the
/// fire path is testable without launching a real `claude`.
type SpawnFn<'a> = dyn Fn(
        &str,
        &str,
        Option<&str>,
        Option<&str>,
        Option<&str>,
        &str,
    ) -> Result<crate::session_launch::SpawnSessionResponse, String>
    + 'a;

/// Fire one iteration: claim the slot, spawn the iteration session, and record
/// which session it produced. Returns the claimed record (carrying the *next*
/// due time and generation) so the caller can sleep until the next fire.
///
/// A spawn failure after a successful claim is deliberately swallowed to a logged
/// error rather than un-claiming: re-claiming would risk a double-fire, and a
/// loop that skips one broken iteration and continues is safer than one that
/// wedges or doubles. The claimed record's advanced schedule stands either way.
pub fn fire_once(id: &str, generation: u64) -> Result<LoopRecord, ClaimError> {
    let dir = loops_dir().ok_or(ClaimError::Gone)?;
    fire_once_in(&dir, id, generation, now_ms(), &move |ws, prompt, model, effort, perm, ep| {
        crate::session_launch::spawn_new_session_with_entrypoint(ws, prompt, model, effort, perm, ep)
    })
}

fn fire_once_in(
    dir: &Path,
    id: &str,
    generation: u64,
    now: u64,
    spawn: &SpawnFn<'_>,
) -> Result<LoopRecord, ClaimError> {
    let claimed = claim_fire_in(dir, id, generation, now)?;
    let prompt = compose_iteration_prompt(&claimed);
    match spawn(
        &claimed.workspace_path,
        &prompt,
        claimed.model.as_deref(),
        claimed.effort.as_deref(),
        None,
        LOOP_ENTRYPOINT,
    ) {
        Ok(resp) => {
            if let Some(sid) = resp.session_id {
                // The record may have been retired (final iteration) — only stamp
                // if it's still around.
                if get_in(dir, id).is_some() {
                    record_iteration_session_in(dir, id, &sid);
                }
                crate::log_debug(&format!(
                    "loop {id}: fired iteration {} -> session {sid}",
                    claimed.iterations_done
                ));
            }
        }
        Err(e) => {
            crate::log_debug(&format!(
                "loop {id}: iteration {} spawn failed: {e} (schedule already advanced, will retry next interval)",
                claimed.iterations_done
            ));
        }
    }
    Ok(claimed)
}

/// Longest a timer sleeps in one go before re-checking the record. Caps how
/// long a timer lingers after `fleet loop stop` deletes the record (it notices
/// on the next wake and exits), and bounds clock-drift error from one long sleep
/// spanning a laptop suspend.
const POLL_CAP: std::time::Duration = std::time::Duration::from_secs(30);

/// The blocking timer loop — the body of `fleet loop fire <id> <gen>`. Sleeps
/// until the loop is due, fires one iteration, then continues with the new
/// generation the fire produced. Returns when the loop is stopped, exhausted, or
/// superseded by another timer (stale generation). Never re-arms via a new
/// process: a single detached timer drives every iteration by looping here.
///
/// Thin by design — every branch delegates to the unit-tested [`claim_fire`] /
/// [`fire_once`] beneath it; only the real `sleep` lives here.
pub fn run_timer_blocking(id: &str, mut generation: u64) {
    loop {
        let Some(rec) = get(id) else {
            crate::log_debug(&format!("loop {id}: record gone, timer exiting"));
            return;
        };
        if rec.generation != generation {
            crate::log_debug(&format!(
                "loop {id}: timer superseded (held gen {generation}, record at {}), exiting",
                rec.generation
            ));
            return;
        }
        if !rec.is_live(now_ms()) {
            crate::log_debug(&format!("loop {id}: exhausted/expired, timer exiting"));
            let _ = stop(id);
            return;
        }
        let wait = rec.due_in_ms(now_ms());
        if wait > 0 {
            let nap = POLL_CAP.min(std::time::Duration::from_millis(wait));
            std::thread::sleep(nap);
            continue;
        }
        match fire_once(id, generation) {
            Ok(claimed) => generation = claimed.generation,
            Err(ClaimError::NotDue { .. }) => continue,
            Err(e) => {
                crate::log_debug(&format!("loop {id}: timer stopping ({e})"));
                return;
            }
        }
    }
}

/// Spawn a detached timer process (`fleet loop fire <id> <gen>`) that sleeps
/// until the loop is due, fires it, and repeats. Idempotent-safe: arming twice
/// just means two timers race for the same fire and `generation` lets exactly
/// one win (see module docs). Used by `fleet loop create` and the reconcile
/// sweep; a running timer re-arms itself in-process and does not call this.
pub fn arm_timer(rec: &LoopRecord) -> Result<u32, String> {
    let fleet = crate::hooks::resolve_fleet_binary()
        .ok_or("cannot find fleet binary to arm loop timer")?;
    arm_timer_with(&fleet, rec)
}

/// A loop overdue by more than this is treated as stranded — its detached timer
/// died (machine reboot, `kill -9`) and no process is left to fire it. Must be
/// comfortably larger than [`POLL_CAP`] so a healthy timer merely mid-nap is
/// never mistaken for dead. Overdue-ness is the liveness signal: a live timer
/// fires a loop the instant it comes due, so a loop can only *stay* overdue if
/// nothing is watching it. This is why no pid needs tracking.
const STRANDED_GRACE_MS: u64 = 3 * 30_000; // 3 × POLL_CAP

/// Re-arm timers for loops that have been left stranded (their detached timer
/// process died). Cheap and idempotent — safe to call from any periodic hook.
/// Returns the ids it re-armed. The freshly-armed timer fires the overdue loop
/// on its first wake; `generation` guards the unlikely case where the old timer
/// is somehow still alive (both race, exactly one wins).
pub fn reconcile() -> Vec<String> {
    let Some(dir) = loops_dir() else {
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

fn reconcile_in(
    dir: &Path,
    now: u64,
    arm: &mut dyn FnMut(&LoopRecord),
) -> Vec<String> {
    let mut rearmed = Vec::new();
    for rec in list_in(dir) {
        if !rec.is_live(now) {
            continue;
        }
        // Overdue past the grace window ⇒ no live timer is driving it.
        if now.saturating_sub(rec.next_fire_at) > STRANDED_GRACE_MS {
            arm(&rec);
            rearmed.push(rec.id.clone());
        }
    }
    rearmed
}

fn arm_timer_with(fleet_bin: &str, rec: &LoopRecord) -> Result<u32, String> {
    let mut cmd = std::process::Command::new(fleet_bin);
    cmd.arg("loop")
        .arg("fire")
        .arg(&rec.id)
        .arg(rec.generation.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Detach into its own session so quitting the desktop app / fleet serve does
    // not take the timer down — same contract as the handoff relay and proc host.
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn loop timer: {e}"))?;
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

    fn make(d: &Path, id: &str, now: u64) -> LoopRecord {
        create_in(d, "/ws", "check the deploy", 300, None, None, None, Some("s1"), id, now)
            .unwrap()
    }

    #[test]
    fn parses_intervals() {
        assert_eq!(parse_interval("60").unwrap(), 60);
        assert_eq!(parse_interval("90s").unwrap(), 90);
        assert_eq!(parse_interval("5m").unwrap(), 300);
        assert_eq!(parse_interval("2h").unwrap(), 7200);
        assert_eq!(parse_interval("1d").unwrap(), 86400);
        assert_eq!(parse_interval(" 30min ").unwrap(), 1800);
    }

    /// Each iteration spawns a whole agent session — a 10s loop is a runaway, not
    /// a schedule. Refuse it at parse time rather than discovering it in billing.
    #[test]
    fn rejects_sub_minute_and_garbage_intervals() {
        assert!(parse_interval("10s").unwrap_err().contains("minimum"));
        assert!(parse_interval("0").unwrap_err().contains("minimum"));
        assert!(parse_interval("5x").unwrap_err().contains("unknown interval unit"));
        assert!(parse_interval("abc").is_err());
        assert!(parse_interval("").is_err());
    }

    /// The creating session is already running the prompt right now — firing the
    /// first iteration immediately would double it.
    #[test]
    fn first_iteration_is_one_interval_out() {
        let d = dir();
        let rec = make(d.path(), "l1", 1_000_000);
        assert_eq!(rec.next_fire_at, 1_000_000 + 300 * 1000);
        assert_eq!(rec.iterations_done, 0);
        assert_eq!(rec.generation, 0);
        assert!(!rec.is_due(1_000_000));
        assert!(rec.is_due(1_300_000));
    }

    #[test]
    fn create_rejects_empty_prompt_and_zero_iterations() {
        let d = dir();
        assert!(create_in(d.path(), "/ws", "  ", 300, None, None, None, None, "x", 1)
            .unwrap_err()
            .contains("prompt is required"));
        assert!(
            create_in(d.path(), "/ws", "p", 300, Some(0), None, None, None, "x", 1)
                .unwrap_err()
                .contains("at least 1")
        );
    }

    #[test]
    fn roundtrip_list_get_stop() {
        let d = dir();
        make(d.path(), "l1", 1_000);
        make(d.path(), "l2", 2_000);
        assert_eq!(list_in(d.path()).len(), 2);
        assert_eq!(get_in(d.path(), "l1").unwrap().prompt, "check the deploy");
        // camelCase on the wire, like every other Fleet record
        let raw = fs::read_to_string(record_path(d.path(), "l1")).unwrap();
        assert!(raw.contains("\"workspacePath\""));
        assert!(raw.contains("\"nextFireAt\""));

        assert!(stop_in(d.path(), "l1"));
        assert!(get_in(d.path(), "l1").is_none());
        assert_eq!(list_in(d.path()).len(), 1);
        // idempotent
        assert!(!stop_in(d.path(), "l1"));
    }

    #[test]
    fn claim_advances_generation_and_next_fire() {
        let d = dir();
        make(d.path(), "l1", 0);
        let claimed = claim_fire_in(d.path(), "l1", 0, 300_000).unwrap();
        assert_eq!(claimed.iterations_done, 1);
        assert_eq!(claimed.generation, 1);
        assert_eq!(claimed.next_fire_at, 300_000 + 300_000);

        // persisted, not just returned
        let on_disk = get_in(d.path(), "l1").unwrap();
        assert_eq!(on_disk.generation, 1);
        assert_eq!(on_disk.iterations_done, 1);
    }

    /// The whole point of `generation`: two timers armed for the same fire must
    /// produce exactly one session, not two.
    #[test]
    fn a_second_timer_holding_the_old_generation_is_refused() {
        let d = dir();
        make(d.path(), "l1", 0);
        assert!(claim_fire_in(d.path(), "l1", 0, 300_000).is_ok());
        let err = claim_fire_in(d.path(), "l1", 0, 300_000).unwrap_err();
        assert_eq!(
            err,
            ClaimError::StaleGeneration {
                expected: 0,
                found: 1
            }
        );
    }

    /// `fleet loop stop` deletes the record; a timer already sleeping on it must
    /// notice and die quietly rather than spawning an unwanted session.
    #[test]
    fn a_stopped_loop_cannot_be_claimed() {
        let d = dir();
        make(d.path(), "l1", 0);
        stop_in(d.path(), "l1");
        assert_eq!(claim_fire_in(d.path(), "l1", 0, 300_000).unwrap_err(), ClaimError::Gone);
    }

    #[test]
    fn claiming_early_is_refused() {
        let d = dir();
        make(d.path(), "l1", 0);
        let err = claim_fire_in(d.path(), "l1", 0, 100_000).unwrap_err();
        assert_eq!(err, ClaimError::NotDue { due_in_ms: 200_000 });
    }

    /// A laptop asleep for an hour must not wake up and fire twelve 5-minute
    /// iterations back to back.
    #[test]
    fn a_late_fire_reschedules_from_now_not_from_the_missed_slot() {
        let d = dir();
        make(d.path(), "l1", 0); // due at 300_000
        let claimed = claim_fire_in(d.path(), "l1", 0, 3_600_000).unwrap();
        assert_eq!(
            claimed.next_fire_at,
            3_600_000 + 300_000,
            "next fire is one interval from the actual fire, not a catch-up burst"
        );
    }

    /// The last iteration retires the record: nothing left for a timer to re-arm.
    #[test]
    fn a_bounded_loop_retires_itself_on_the_final_iteration() {
        let d = dir();
        create_in(d.path(), "/ws", "twice", 60, Some(2), None, None, None, "l1", 0).unwrap();
        let first = claim_fire_in(d.path(), "l1", 0, 60_000).unwrap();
        assert_eq!(first.iterations_done, 1);
        assert!(get_in(d.path(), "l1").is_some(), "one iteration left");

        let second = claim_fire_in(d.path(), "l1", 1, 120_000).unwrap();
        assert_eq!(second.iterations_done, 2);
        assert!(
            get_in(d.path(), "l1").is_none(),
            "final iteration must retire the record so no timer re-arms"
        );
        assert_eq!(claim_fire_in(d.path(), "l1", 2, 180_000).unwrap_err(), ClaimError::Gone);
    }

    /// A loop is an agent that spawns agents. `max_iterations` is user-supplied;
    /// the hard cap is not.
    #[test]
    fn max_iterations_is_clamped_to_the_hard_ceiling() {
        let d = dir();
        let rec =
            create_in(d.path(), "/ws", "p", 60, Some(99_999), None, None, None, "l1", 0).unwrap();
        assert_eq!(rec.max_iterations, Some(MAX_ITERATIONS));
    }

    #[test]
    fn an_expired_record_is_not_claimable() {
        let d = dir();
        make(d.path(), "l1", 0);
        let err = claim_fire_in(d.path(), "l1", 0, EXPIRY_MS + 1).unwrap_err();
        assert_eq!(err, ClaimError::Exhausted);
    }

    #[test]
    fn due_loops_selects_only_live_and_due() {
        let d = dir();
        make(d.path(), "soon", 0); // due at 300_000
        create_in(d.path(), "/ws", "p", 3600, None, None, None, None, "later", 0).unwrap();
        let due = due_loops_in(d.path(), 400_000);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, "soon");
    }

    use std::cell::RefCell;

    /// What the injected spawner saw, so a test can assert the iteration was
    /// launched with the right workspace / prompt / model.
    #[derive(Default, Clone)]
    struct SpawnCall {
        workspace: String,
        prompt: String,
        model: Option<String>,
        entrypoint: String,
    }

    fn ok_spawner<'a>(
        calls: &'a RefCell<Vec<SpawnCall>>,
        sid: &'a str,
    ) -> impl Fn(&str, &str, Option<&str>, Option<&str>, Option<&str>, &str)
        -> Result<crate::session_launch::SpawnSessionResponse, String> + 'a {
        move |ws, prompt, model, _effort, _perm, ep| {
            calls.borrow_mut().push(SpawnCall {
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
            d.path(), "/ws", "check the deploy", 300, None,
            Some("claude-fable-5"), Some("high"), None, "l1", 0,
        )
        .unwrap();

        let calls = RefCell::new(Vec::new());
        let claimed =
            fire_once_in(d.path(), "l1", 0, 300_000, &ok_spawner(&calls, "iter-sid-1")).unwrap();

        assert_eq!(claimed.iterations_done, 1);
        assert_eq!(claimed.generation, 1);

        let calls = calls.into_inner();
        assert_eq!(calls.len(), 1, "exactly one iteration spawned");
        assert_eq!(calls[0].workspace, "/ws");
        assert_eq!(calls[0].entrypoint, LOOP_ENTRYPOINT);
        assert_eq!(calls[0].model.as_deref(), Some("claude-fable-5"));
        // prompt carries the user's text plus the stop-hint footer
        assert!(calls[0].prompt.contains("check the deploy"));
        assert!(calls[0].prompt.contains("fleet loop stop l1"));

        // the produced session id is recorded on the loop
        assert_eq!(get_in(d.path(), "l1").unwrap().last_session_id.as_deref(), Some("iter-sid-1"));
    }

    /// The claim happens before the spawn; if two timers race, the second's
    /// claim is refused and it must NOT spawn a session.
    #[test]
    fn a_stale_timer_fires_nothing() {
        let d = dir();
        make(d.path(), "l1", 0);
        let calls = RefCell::new(Vec::new());
        // first timer fires
        fire_once_in(d.path(), "l1", 0, 300_000, &ok_spawner(&calls, "s1")).unwrap();
        // second timer, still holding gen 0, must be refused with no spawn
        let err = fire_once_in(d.path(), "l1", 0, 300_000, &ok_spawner(&calls, "s2")).unwrap_err();
        assert_eq!(err, ClaimError::StaleGeneration { expected: 0, found: 1 });
        assert_eq!(calls.into_inner().len(), 1, "the stale timer must not spawn");
    }

    /// A spawn failure must not un-claim (that would risk a double-fire) — the
    /// schedule stays advanced and the loop lives on for the next interval.
    #[test]
    fn a_spawn_failure_still_advances_the_schedule() {
        let d = dir();
        make(d.path(), "l1", 0);
        let failing = |_ws: &str, _p: &str, _m: Option<&str>, _e: Option<&str>, _pm: Option<&str>, _ep: &str|
            -> Result<crate::session_launch::SpawnSessionResponse, String> { Err("boom".into()) };
        let claimed = fire_once_in(d.path(), "l1", 0, 300_000, &failing).unwrap();
        assert_eq!(claimed.iterations_done, 1);
        // record persisted with advanced generation, still live for next time
        let on_disk = get_in(d.path(), "l1").unwrap();
        assert_eq!(on_disk.generation, 1);
        assert_eq!(on_disk.iterations_done, 1);
    }

    #[test]
    fn iteration_prompt_names_the_loop_and_the_off_switch() {
        let d = dir();
        let mut rec = make(d.path(), "abc123", 0);
        rec.iterations_done = 3;
        let p = compose_iteration_prompt(&rec);
        assert!(p.contains("check the deploy"));
        assert!(p.contains("abc123"));
        assert!(p.contains("第 3 次"));
        assert!(p.contains("5m"));
        assert!(p.contains("fleet loop stop abc123"));
    }

    #[test]
    fn reconcile_rearms_only_stranded_loops() {
        let d = dir();
        // "healthy": due in the future — a timer is presumably driving it
        create_in(d.path(), "/ws", "p", 300, None, None, None, None, "healthy", 1_000_000).unwrap();
        // "napping": due, but only just — inside the grace window, timer likely mid-nap
        let mut napping = make(d.path(), "napping", 0);
        napping.next_fire_at = 1_000_000 - 10_000; // 10s overdue < grace
        write_record(d.path(), &napping).unwrap();
        // "stranded": overdue well past the grace window — its timer is dead
        let mut stranded = make(d.path(), "stranded", 0);
        stranded.next_fire_at = 1_000_000 - STRANDED_GRACE_MS - 5_000;
        write_record(d.path(), &stranded).unwrap();

        let mut armed = Vec::new();
        let rearmed = reconcile_in(d.path(), 1_000_000, &mut |r| armed.push(r.id.clone()));

        assert_eq!(rearmed, vec!["stranded"], "only the stranded loop is re-armed");
        assert_eq!(armed, vec!["stranded"]);
    }

    #[test]
    fn reconcile_ignores_exhausted_and_expired() {
        let d = dir();
        // exhausted: max 1 iteration, already done — overdue but must not re-arm
        let mut done =
            create_in(d.path(), "/ws", "p", 60, Some(1), None, None, None, "done", 0).unwrap();
        done.iterations_done = 1;
        done.next_fire_at = 0; // very overdue
        write_record(d.path(), &done).unwrap();

        let mut armed = Vec::new();
        let rearmed = reconcile_in(d.path(), 10_000_000, &mut |r| armed.push(r.id.clone()));
        assert!(rearmed.is_empty(), "exhausted loop must not be resurrected");
        assert!(armed.is_empty());
    }

    #[test]
    fn humanize_reads_intervals_back() {
        assert_eq!(humanize_secs(300), "5m");
        assert_eq!(humanize_secs(7200), "2h");
        assert_eq!(humanize_secs(86400), "1d");
        assert_eq!(humanize_secs(90), "90s");
    }

    /// The loop must run on the model of the session that created it.
    #[test]
    fn model_and_effort_are_carried_on_the_record() {
        let d = dir();
        let rec = create_in(
            d.path(),
            "/ws",
            "p",
            300,
            None,
            Some("claude-fable-5"),
            Some("high"),
            None,
            "l1",
            0,
        )
        .unwrap();
        assert_eq!(rec.model.as_deref(), Some("claude-fable-5"));
        assert_eq!(rec.effort.as_deref(), Some("high"));
        // blank strings are not a model
        let rec = create_in(d.path(), "/ws", "p", 300, None, Some("  "), Some(""), None, "l2", 0)
            .unwrap();
        assert_eq!(rec.model, None);
        assert_eq!(rec.effort, None);
    }
}
