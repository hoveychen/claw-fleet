//! Watch — a one-shot condition wait that survives the turn boundary.
//!
//! `Monitor`, background `Bash`, and `ScheduleWakeup` all promise "tell me when
//! X happens" — and in a Fleet session all three are dead ends. Every Fleet
//! session is a headless `claude -p` run that exits the instant its turn ends;
//! any background task spawned inside it is torn down with the process, and
//! nothing is left alive to deliver the event. The tool reports the watch as
//! armed; the notification never arrives. See [`crate::bg_guard`].
//!
//! This module is the mechanism that does work. A watch is a *conditional handoff
//! to yourself*: the condition and the target session live on disk, and when the
//! condition fires Fleet **resumes the original (now-dead) session** via
//! `claude --resume <id> -p <event-text>` — so the agent's next turn sees the
//! event and can report it. Nothing depends on the registering session's process
//! still being alive.
//!
//! It follows [`crate::agent_loop`]'s design almost exactly, with two deltas:
//!   1. the detached timer polls a **shell condition** (`until_cmd` exits 0)
//!      instead of sleeping to a fixed time, and
//!   2. firing **resumes a specific existing session** (like [`crate::handoff`])
//!      instead of spawning a fresh one (like a loop iteration).
//!
//! A watch fires **once** and then retires — it is a wait, not a schedule.
//!
//! File layout: `~/.fleet/watches/<watch_id>.json`, one record per watch.
//!
//! ## Why `generation`
//!
//! A timer process naps between polls. In that window the watch may be stopped or
//! re-armed by the reconcile sweep, leaving two timers racing for the same fire.
//! Every record carries a `generation`; the reconcile re-arm bumps it, so a timer
//! that wakes holding a stale generation — or finds the record gone — exits
//! without resuming. On top of that, [`claim_fire`] *removes* the record as it
//! grants the fire, so even two timers at the same generation resolve to exactly
//! one resume: whoever unlinks the file first wins, the other sees `Gone`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Shortest poll interval we accept. A poll just runs a shell command (cheap,
/// unlike a loop iteration that spawns a whole agent), so the floor is low — but
/// a sub-second poll is a busy-loop, not a watch.
pub const MIN_POLL_SECS: u64 = 5;

/// Default poll interval when the caller doesn't specify one. Matches [`POLL_CAP`]
/// so a healthy timer wakes about twice a minute.
pub const DEFAULT_POLL_SECS: u64 = 30;

/// Default deadline when the caller doesn't specify a timeout: two hours covers a
/// long CI run or build without waiting forever on a condition that never fires.
pub const DEFAULT_TIMEOUT_SECS: u64 = 2 * 60 * 60;

/// Hard ceiling on how long a watch may wait, whatever the caller asked for. A
/// watch that never fires still costs a resumed session on timeout; a floor under
/// the wait keeps a typo from parking one for a month. Mirrors Claude Code's own
/// 7-day cron expiry.
pub const MAX_TIMEOUT_SECS: u64 = 7 * 24 * 60 * 60;

/// One registered watch.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WatchRecord {
    pub id: String,
    /// The session to resume when the condition fires — the dead headless `-p`
    /// session that registered this watch. Unlike a loop (which spawns a fresh
    /// session each iteration), a watch reanimates *this specific* session so the
    /// event lands in the same conversation that asked to wait.
    pub session_id: String,
    /// Workspace the session lives in, passed through to the resume.
    pub workspace_path: String,
    /// Shell command polled each interval; exit status 0 ⇒ the condition is met
    /// (the thing being waited for is done) and the watch fires.
    pub until_cmd: String,
    /// Shell command whose stdout becomes the event text handed to the resumed
    /// session. `None` ⇒ a generic "your watch fired" line is used instead.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub capture_cmd: Option<String>,
    /// Human note describing what is being waited for, prepended to the resume
    /// prompt so the woken agent knows why it came back.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
    /// Seconds between condition polls.
    pub poll_secs: u64,
    /// Epoch ms after which the watch gives up and fires a timeout resume.
    pub deadline_at: u64,
    /// Model the resumed session runs on — inherited from the registering session
    /// so a fable-5 session doesn't wake up as opus.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub effort: Option<String>,
    /// Agent source to resume on (`"claude"` / `"codex"`), inherited from the
    /// registering session so a codex session resumes as codex. `None` = the
    /// historical default (claude).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub agent_source: Option<String>,
    /// Bumped by the reconcile re-arm; stale timers exit without firing. See module docs.
    pub generation: u64,
    pub created: u64,
}

impl WatchRecord {
    /// The watch hasn't passed its deadline yet.
    pub fn is_live(&self, now: u64) -> bool {
        now < self.deadline_at
    }

    /// Past the deadline: the condition never came true in time.
    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.deadline_at
    }
}

/// `5m`, `90s`, `2h`, `1d` → seconds. Bare digits are read as seconds. Shared by
/// the `--poll` and `--timeout` parsers, which apply their own floors/ceilings on
/// top. Deliberately not [`crate::agent_loop::parse_interval`]: that one bakes in
/// a 60s floor meant for loops that spawn a whole agent each tick, which is wrong
/// for a cheap shell poll.
pub fn parse_duration_secs(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("duration is required (e.g. 30s, 5m, 2h)".to_string());
    }
    let (digits, unit) = match s.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => (&s[..i], &s[i..]),
        None => (s, ""),
    };
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("cannot parse duration '{s}' (expected e.g. 30s, 5m, 2h)"))?;
    let secs = match unit.to_ascii_lowercase().as_str() {
        "" | "s" | "sec" | "secs" => n,
        "m" | "min" | "mins" => n * 60,
        "h" | "hr" | "hrs" => n * 3600,
        "d" | "day" | "days" => n * 86400,
        other => return Err(format!("unknown duration unit '{other}' (use s/m/h/d)")),
    };
    Ok(secs)
}

/// Parse a `--poll` value, clamping to the [`MIN_POLL_SECS`] floor.
pub fn parse_poll(s: &str) -> Result<u64, String> {
    Ok(parse_duration_secs(s)?.max(MIN_POLL_SECS))
}

/// Parse a `--timeout` value, clamping to the [`MAX_TIMEOUT_SECS`] ceiling. A
/// zero (or sub-poll) timeout would fire immediately, which is never intended.
pub fn parse_timeout(s: &str) -> Result<u64, String> {
    let secs = parse_duration_secs(s)?;
    if secs == 0 {
        return Err("timeout must be greater than zero".to_string());
    }
    Ok(secs.min(MAX_TIMEOUT_SECS))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// `~/.fleet/watches`.
pub fn watches_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("watches"))
}

fn record_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

fn write_record(dir: &Path, rec: &WatchRecord) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create watches dir: {e}"))?;
    let json = serde_json::to_string_pretty(rec).map_err(|e| format!("serialize watch: {e}"))?;
    fs::write(record_path(dir, &rec.id), json).map_err(|e| format!("write watch: {e}"))
}

// ── create / read / stop ──────────────────────────────────────────────────────

/// Register a watch. Fields describe both the condition (`until_cmd`, `poll_secs`,
/// `timeout_secs`) and how to reanimate the session when it fires (`session_id`,
/// `workspace_path`, `model`, `effort`, `agent_source`).
#[allow(clippy::too_many_arguments)]
pub fn create(
    session_id: &str,
    workspace_path: &str,
    until_cmd: &str,
    capture_cmd: Option<&str>,
    note: Option<&str>,
    poll_secs: u64,
    timeout_secs: u64,
    model: Option<&str>,
    effort: Option<&str>,
    agent_source: Option<&str>,
) -> Result<WatchRecord, String> {
    let dir = watches_dir().ok_or("cannot determine home dir")?;
    create_in(
        &dir,
        session_id,
        workspace_path,
        until_cmd,
        capture_cmd,
        note,
        poll_secs,
        timeout_secs,
        model,
        effort,
        agent_source,
        &uuid::Uuid::new_v4().to_string()[..8],
        now_ms(),
    )
}

#[allow(clippy::too_many_arguments)]
fn create_in(
    dir: &Path,
    session_id: &str,
    workspace_path: &str,
    until_cmd: &str,
    capture_cmd: Option<&str>,
    note: Option<&str>,
    poll_secs: u64,
    timeout_secs: u64,
    model: Option<&str>,
    effort: Option<&str>,
    agent_source: Option<&str>,
    id: &str,
    now: u64,
) -> Result<WatchRecord, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("watch session id is required".to_string());
    }
    let until_cmd = until_cmd.trim();
    if until_cmd.is_empty() {
        return Err("watch --until command is required".to_string());
    }
    if timeout_secs == 0 {
        return Err("watch timeout must be greater than zero".to_string());
    }
    let poll_secs = poll_secs.max(MIN_POLL_SECS);
    let timeout_secs = timeout_secs.min(MAX_TIMEOUT_SECS);
    let blank = |s: &str| s.trim().is_empty();
    let rec = WatchRecord {
        id: id.to_string(),
        session_id: session_id.to_string(),
        workspace_path: workspace_path.to_string(),
        until_cmd: until_cmd.to_string(),
        capture_cmd: capture_cmd.filter(|c| !blank(c)).map(str::to_string),
        note: note.filter(|n| !blank(n)).map(str::to_string),
        poll_secs,
        deadline_at: now + timeout_secs * 1000,
        model: model.filter(|m| !blank(m)).map(str::to_string),
        effort: effort.filter(|e| !blank(e)).map(str::to_string),
        agent_source: agent_source.filter(|s| !blank(s)).map(str::to_string),
        generation: 0,
        created: now,
    };
    write_record(dir, &rec)?;
    Ok(rec)
}

pub fn get(id: &str) -> Option<WatchRecord> {
    get_in(&watches_dir()?, id)
}

fn get_in(dir: &Path, id: &str) -> Option<WatchRecord> {
    let s = fs::read_to_string(record_path(dir, id)).ok()?;
    serde_json::from_str(&s).ok()
}

/// Every registered watch, soonest-deadline first.
pub fn list() -> Vec<WatchRecord> {
    let Some(dir) = watches_dir() else {
        return Vec::new();
    };
    list_in(&dir)
}

fn list_in(dir: &Path) -> Vec<WatchRecord> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<WatchRecord> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| {
            let s = fs::read_to_string(e.path()).ok()?;
            serde_json::from_str(&s).ok()
        })
        .collect();
    out.sort_by_key(|r| r.deadline_at);
    out
}

/// Stop a watch. Idempotent — returns whether a record was actually removed.
///
/// Any timer already napping on this watch finds the record gone on its next wake
/// and exits without firing, so no process needs to be killed.
pub fn stop(id: &str) -> bool {
    let Some(dir) = watches_dir() else {
        return false;
    };
    stop_in(&dir, id)
}

fn stop_in(dir: &Path, id: &str) -> bool {
    fs::remove_file(record_path(dir, id)).is_ok()
}

// ── firing ────────────────────────────────────────────────────────────────────

/// Why a claim was refused, so the timer process can log something truthful.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimError {
    /// The watch was stopped, or another timer already fired it (the record is gone).
    Gone,
    /// The reconcile sweep re-armed the watch under a newer generation; this timer
    /// is stale and must exit.
    StaleGeneration { expected: u64, found: u64 },
}

impl std::fmt::Display for ClaimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gone => write!(f, "watch was stopped or already fired"),
            Self::StaleGeneration { expected, found } => write!(
                f,
                "stale timer (held generation {expected}, record is at {found})"
            ),
        }
    }
}

/// Atomically take the single fire of a watch.
///
/// A watch fires once, so this *removes* the record as it grants the fire. That
/// unlink is the serialization point: if two timers both see the condition met in
/// the same poll window, the first to unlink wins and the second gets [`Gone`] —
/// exactly one resume, no matter how many timers are armed.
///
/// Returns the record so the caller can resume the session straight from it.
pub fn claim_fire(id: &str, generation: u64) -> Result<WatchRecord, ClaimError> {
    let dir = watches_dir().ok_or(ClaimError::Gone)?;
    claim_fire_in(&dir, id, generation)
}

fn claim_fire_in(dir: &Path, id: &str, generation: u64) -> Result<WatchRecord, ClaimError> {
    let rec = get_in(dir, id).ok_or(ClaimError::Gone)?;
    if rec.generation != generation {
        return Err(ClaimError::StaleGeneration {
            expected: generation,
            found: rec.generation,
        });
    }
    // One-shot: unlinking the record IS the fire. Whoever removes it first is the
    // sole winner; a racing timer's remove fails and it reads that as `Gone`.
    if fs::remove_file(record_path(dir, id)).is_err() {
        return Err(ClaimError::Gone);
    }
    Ok(rec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn make(d: &Path, id: &str, now: u64) -> WatchRecord {
        create_in(
            d,
            "sess-1",
            "/ws",
            "gh run view 123 --json status -q .status | grep -qx completed",
            Some("gh run view 123 --json conclusion -q .conclusion"),
            Some("CI run 123"),
            30,
            DEFAULT_TIMEOUT_SECS,
            None,
            None,
            None,
            id,
            now,
        )
        .unwrap()
    }

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration_secs("30").unwrap(), 30);
        assert_eq!(parse_duration_secs("90s").unwrap(), 90);
        assert_eq!(parse_duration_secs("5m").unwrap(), 300);
        assert_eq!(parse_duration_secs("2h").unwrap(), 7200);
        assert_eq!(parse_duration_secs("1d").unwrap(), 86400);
        assert_eq!(parse_duration_secs(" 15min ").unwrap(), 900);
    }

    #[test]
    fn rejects_garbage_durations() {
        assert!(parse_duration_secs("5x").unwrap_err().contains("unknown duration unit"));
        assert!(parse_duration_secs("abc").is_err());
        assert!(parse_duration_secs("").is_err());
    }

    /// Poll floors at the minimum; a 1s poll is a busy-loop.
    #[test]
    fn poll_clamps_to_the_floor() {
        assert_eq!(parse_poll("1s").unwrap(), MIN_POLL_SECS);
        assert_eq!(parse_poll("30s").unwrap(), 30);
    }

    /// Timeout ceilings at the max and refuses zero.
    #[test]
    fn timeout_clamps_and_refuses_zero() {
        assert_eq!(parse_timeout("30d").unwrap(), MAX_TIMEOUT_SECS);
        assert_eq!(parse_timeout("1h").unwrap(), 3600);
        assert!(parse_timeout("0").unwrap_err().contains("greater than zero"));
    }

    #[test]
    fn create_sets_deadline_and_defaults() {
        let d = dir();
        let rec = make(d.path(), "w1", 1_000_000);
        assert_eq!(rec.deadline_at, 1_000_000 + DEFAULT_TIMEOUT_SECS * 1000);
        assert_eq!(rec.generation, 0);
        assert_eq!(rec.session_id, "sess-1");
        assert!(rec.is_live(1_000_000));
        assert!(!rec.is_expired(1_000_000));
        assert!(rec.is_expired(rec.deadline_at));
    }

    #[test]
    fn create_rejects_empty_session_and_until() {
        let d = dir();
        assert!(create_in(
            d.path(), "  ", "/ws", "true", None, None, 30, 60, None, None, None, "x", 1
        )
        .unwrap_err()
        .contains("session id is required"));
        assert!(create_in(
            d.path(), "s", "/ws", "  ", None, None, 30, 60, None, None, None, "x", 1
        )
        .unwrap_err()
        .contains("--until command is required"));
        assert!(create_in(
            d.path(), "s", "/ws", "true", None, None, 30, 0, None, None, None, "x", 1
        )
        .unwrap_err()
        .contains("timeout must be greater than zero"));
    }

    #[test]
    fn create_clamps_poll_and_timeout() {
        let d = dir();
        let rec = create_in(
            d.path(), "s", "/ws", "true", None, None, 1, MAX_TIMEOUT_SECS + 999, None, None, None, "w", 0,
        )
        .unwrap();
        assert_eq!(rec.poll_secs, MIN_POLL_SECS, "sub-floor poll clamps up");
        assert_eq!(
            rec.deadline_at,
            MAX_TIMEOUT_SECS * 1000,
            "over-ceiling timeout clamps down"
        );
    }

    #[test]
    fn roundtrip_list_get_stop() {
        let d = dir();
        // w1 has the later deadline, w2 the earlier — list sorts by deadline
        create_in(d.path(), "s", "/ws", "true", None, None, 30, 7200, None, None, None, "w1", 1_000)
            .unwrap();
        create_in(d.path(), "s", "/ws", "true", None, None, 30, 60, None, None, None, "w2", 1_000)
            .unwrap();
        let all = list_in(d.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "w2", "earliest deadline first");

        // camelCase on the wire, like every other Fleet record
        let raw = fs::read_to_string(record_path(d.path(), "w1")).unwrap();
        assert!(raw.contains("\"sessionId\""));
        assert!(raw.contains("\"untilCmd\""));
        assert!(raw.contains("\"deadlineAt\""));

        assert!(stop_in(d.path(), "w1"));
        assert!(get_in(d.path(), "w1").is_none());
        assert_eq!(list_in(d.path()).len(), 1);
        // idempotent
        assert!(!stop_in(d.path(), "w1"));
    }

    /// The whole point of the one-shot claim: firing removes the record so a
    /// second timer racing the same condition resumes nothing.
    #[test]
    fn claim_removes_the_record_and_a_second_claim_is_gone() {
        let d = dir();
        make(d.path(), "w1", 0);
        let claimed = claim_fire_in(d.path(), "w1", 0).unwrap();
        assert_eq!(claimed.session_id, "sess-1");
        assert!(get_in(d.path(), "w1").is_none(), "fire retires the record");
        // second timer, same generation, finds it gone
        assert_eq!(claim_fire_in(d.path(), "w1", 0).unwrap_err(), ClaimError::Gone);
    }

    /// A timer re-armed by reconcile bumps generation; the old timer holding the
    /// stale generation must refuse to fire.
    #[test]
    fn a_stale_generation_is_refused() {
        let d = dir();
        let mut rec = make(d.path(), "w1", 0);
        rec.generation = 2;
        write_record(d.path(), &rec).unwrap();
        let err = claim_fire_in(d.path(), "w1", 1).unwrap_err();
        assert_eq!(err, ClaimError::StaleGeneration { expected: 1, found: 2 });
        // and the record is untouched — the live timer can still fire it
        assert!(get_in(d.path(), "w1").is_some());
    }

    /// `fleet watch stop` deletes the record; a timer napping on it must find it
    /// gone and not resume anything.
    #[test]
    fn a_stopped_watch_cannot_be_claimed() {
        let d = dir();
        make(d.path(), "w1", 0);
        stop_in(d.path(), "w1");
        assert_eq!(claim_fire_in(d.path(), "w1", 0).unwrap_err(), ClaimError::Gone);
    }

    #[test]
    fn optional_fields_survive_the_roundtrip() {
        let d = dir();
        let rec = create_in(
            d.path(), "s", "/ws", "true",
            Some("echo done"), Some("waiting on X"),
            30, 60, Some("claude-fable-5"), Some("high"), Some("codex"), "w1", 0,
        )
        .unwrap();
        assert_eq!(rec.capture_cmd.as_deref(), Some("echo done"));
        assert_eq!(rec.note.as_deref(), Some("waiting on X"));
        assert_eq!(rec.model.as_deref(), Some("claude-fable-5"));
        assert_eq!(rec.effort.as_deref(), Some("high"));
        assert_eq!(rec.agent_source.as_deref(), Some("codex"));
        let back = get_in(d.path(), "w1").unwrap();
        assert_eq!(back, rec);

        // blank optionals normalize to None
        let rec2 = create_in(
            d.path(), "s", "/ws", "true",
            Some("  "), Some(""), 30, 60, Some(" "), Some(""), Some("  "), "w2", 0,
        )
        .unwrap();
        assert_eq!(rec2.capture_cmd, None);
        assert_eq!(rec2.note, None);
        assert_eq!(rec2.model, None);
        assert_eq!(rec2.effort, None);
        assert_eq!(rec2.agent_source, None);
    }
}
