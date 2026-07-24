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
    /// Epoch ms of the timer's most recent poll — a heartbeat. A watch has no
    /// "due time" to be overdue against (its timer polls continuously), so this
    /// is the liveness signal reconcile uses: a `last_poll_at` older than the
    /// grace window means the detached timer died and nothing is watching.
    /// Defaults to `created` (a just-created watch is "freshly polled" so it isn't
    /// re-armed before its first arm even lands).
    #[serde(default)]
    pub last_poll_at: u64,
    /// Number of condition polls this watch has run — bumped once per `touch_in`
    /// heartbeat (i.e. once per `until` probe that didn't fire). The single firing
    /// poll isn't counted because the record is unlinked as it fires, but only
    /// still-active watches are ever displayed, so that's immaterial. Surfaced on
    /// the session card so a waiting session shows "checked N times".
    #[serde(default)]
    pub poll_count: u64,
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
        last_poll_at: now,
        poll_count: 0,
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

// ── condition timer + resume ────────────────────────────────────────────────────

/// Longest a timer naps in one go before re-checking. Caps how long a timer
/// lingers after `fleet watch stop` deletes the record (it notices on the next
/// wake and exits), and bounds clock-drift from one long nap over a suspend.
const POLL_CAP_MS: u64 = 30_000;

/// What the timer should do this iteration. Pure decision, split out so the
/// nap/fire/exit logic is unit-testable without spawning subprocesses or sleeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimerStep {
    /// Record gone or superseded — the timer should exit.
    Exit,
    /// Fire now. `timed_out` distinguishes "condition met" from "deadline passed".
    Fire { timed_out: bool },
    /// Not yet — nap this many milliseconds and re-check.
    Nap { ms: u64 },
}

/// Decide the timer's next move. `condition_met` is evaluated lazily, only after
/// the generation and deadline checks pass, so the shell poll runs no more than
/// once per wake and never for an already-stale timer.
fn decide(
    rec: &WatchRecord,
    generation: u64,
    now: u64,
    condition_met: impl FnOnce() -> bool,
) -> TimerStep {
    if rec.generation != generation {
        return TimerStep::Exit;
    }
    if rec.is_expired(now) {
        return TimerStep::Fire { timed_out: true };
    }
    if condition_met() {
        return TimerStep::Fire { timed_out: false };
    }
    let remaining = rec.deadline_at.saturating_sub(now);
    // Nap at most one poll interval, never past POLL_CAP_MS, and never past the
    // deadline (so the timeout fire lands on time rather than a poll-cap late).
    let nap = rec
        .poll_secs
        .saturating_mul(1000)
        .min(POLL_CAP_MS)
        .min(remaining.max(1));
    TimerStep::Nap { ms: nap }
}

/// Run the `until` command; exit status 0 ⇒ the condition is met. Delegates to
/// the shared [`crate::process_util::gate_met`] so watch / schedule / loop all
/// evaluate a `--until` gate identically (exit code only; spawn failure ⇒ not
/// met + logged). The event text comes from the separate `capture` command.
fn poll_met(cmd: &str) -> bool {
    crate::process_util::gate_met(cmd)
}

/// Run the `capture` command; its trimmed stdout becomes the event text handed to
/// the resumed session. Empty when there is no capture command or it produced
/// nothing — the resume prompt then just says the watch fired.
fn capture_event(rec: &WatchRecord) -> String {
    let Some(cmd) = &rec.capture_cmd else {
        return String::new();
    };
    match crate::process_util::shell_command(cmd).output() {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(e) => {
            crate::log_debug(&format!("watch capture: cannot run capture-command ({e}): {cmd}"));
            String::new()
        }
    }
}

/// The prompt the resumed turn wakes up to: why it came back (condition met vs
/// timed out), what it was waiting for, the captured event text, and a footer
/// making clear this is a Fleet-driven resume, not a user message.
pub fn compose_resume_prompt(rec: &WatchRecord, event_text: &str, timed_out: bool) -> String {
    let mut out = String::new();
    if timed_out {
        out.push_str(&format!(
            "你注册的 Fleet watch `{}` 已超时——等待的条件在超时前没有满足。",
            rec.id
        ));
    } else {
        out.push_str(&format!(
            "你注册的 Fleet watch `{}` 触发了——等待的条件已满足。",
            rec.id
        ));
    }
    if let Some(note) = &rec.note {
        out.push_str(&format!("\n\n等待的是：{note}"));
    }
    if !event_text.is_empty() {
        out.push_str("\n\n---\n");
        out.push_str(event_text);
    }
    out.push_str(
        "\n\n---\n（这是 Fleet watch 在后台轮询到条件后自动 resume 本会话的，\
         不是用户消息。请据此向老板汇报结果，别再重复注册 Monitor/后台任务。）",
    );
    out
}

/// Signature of the resume side-effect — injected so the fire path is testable
/// without launching a real `claude --resume`. Given the claimed record and the
/// composed prompt, reanimate the session.
type ResumeFn<'a> = dyn Fn(&WatchRecord, &str) -> Result<(), String> + 'a;

/// Fire the watch: claim the single slot (retiring the record), capture the event
/// text, compose the prompt, and resume the session. A resume failure is logged
/// rather than propagated — the record is already consumed, and re-arming would
/// risk a double-resume. Returns the claimed record, or the claim error if a
/// racing timer already fired it.
fn fire_in(
    dir: &Path,
    id: &str,
    generation: u64,
    timed_out: bool,
    capture: &dyn Fn(&WatchRecord) -> String,
    interrupt_if_live: &dyn Fn(&str) -> bool,
    resume: &ResumeFn<'_>,
) -> Result<WatchRecord, ClaimError> {
    let rec = claim_fire_in(dir, id, generation)?;
    let event_text = if timed_out { String::new() } else { capture(&rec) };
    let prompt = compose_resume_prompt(&rec, &event_text, timed_out);
    // A watch resumes on the assumption its session already died (a headless
    // `-p` turn that ended). But a condition can fire while the session is
    // still live — most often it is blocked on a decision card, whose hook
    // keeps the CLI process alive for its full wait window (up to 600s).
    // Resuming then would put a *second* `claude` on the same transcript, which
    // is exactly how a session gets corrupted; `parked::answer_with` guards the
    // same way before every resume. Interrupt the live turn first so only one
    // turn ever runs on the transcript.
    if interrupt_if_live(&rec.session_id) {
        crate::log_debug(&format!(
            "watch {id}: session {} still live at fire time; interrupted it before resuming",
            rec.session_id
        ));
    }
    match resume(&rec, &prompt) {
        Ok(()) => crate::log_debug(&format!(
            "watch {id}: fired ({}) -> resumed session {}",
            if timed_out { "timeout" } else { "condition" },
            rec.session_id
        )),
        Err(e) => crate::log_debug(&format!(
            "watch {id}: fired ({}) but resume failed: {e} (event lost)",
            if timed_out { "timeout" } else { "condition" }
        )),
    }
    Ok(rec)
}

/// Reanimate the session via its agent source, mirroring how auto-resume fires
/// `claude --resume <id> -p <prompt>` — but untracked (no concurrency slot to
/// free), so a no-op `on_exit`.
fn spawn_resume(rec: &WatchRecord, prompt: &str) -> Result<(), String> {
    crate::agent_source::resume_session(
        rec.agent_source.as_deref().unwrap_or("claude"),
        &crate::agent_source::ResumeSpec {
            session_id: rec.session_id.clone(),
            workspace_path: rec.workspace_path.clone(),
            prompt: prompt.to_string(),
            model: rec.model.clone(),
            effort: rec.effort.clone(),
            permission_mode: None,
        },
        Box::new(|_| {}),
    )
}

/// SIGINT the session if a Fleet-owned CLI is still running it, returning
/// whether it interrupted anything. Shared shape with `parked::answer_with`'s
/// pre-resume guard: never launch a resume while the original turn can still be
/// appending to the same transcript.
fn interrupt_if_live(session_id: &str) -> bool {
    if crate::parked::session_alive(session_id) {
        crate::parked::interrupt_session(session_id)
    } else {
        false
    }
}

fn fire_real(id: &str, generation: u64, timed_out: bool) -> Result<WatchRecord, ClaimError> {
    let dir = watches_dir().ok_or(ClaimError::Gone)?;
    fire_in(
        &dir,
        id,
        generation,
        timed_out,
        &capture_event,
        &interrupt_if_live,
        &|rec, prompt| spawn_resume(rec, prompt),
    )
}

/// The blocking timer loop — the body of `fleet watch fire <id> <gen>`. Polls the
/// condition each interval; on a met condition (or a passed deadline) it fires
/// once and returns. Exits quietly when the record is gone (stopped / already
/// fired) or superseded by a newer generation. Thin by design — the fire/nap
/// decision lives in the unit-tested [`decide`], only the real `sleep` and shell
/// poll live here.
pub fn run_timer_blocking(id: &str, generation: u64) {
    loop {
        let Some(rec) = get(id) else {
            crate::log_debug(&format!("watch {id}: record gone, timer exiting"));
            return;
        };
        match decide(&rec, generation, now_ms(), || poll_met(&rec.until_cmd)) {
            TimerStep::Exit => {
                crate::log_debug(&format!(
                    "watch {id}: superseded (held gen {generation}), timer exiting"
                ));
                return;
            }
            TimerStep::Fire { timed_out } => {
                if let Err(e) = fire_real(id, generation, timed_out) {
                    crate::log_debug(&format!("watch {id}: fire refused ({e})"));
                }
                return;
            }
            TimerStep::Nap { ms } => {
                // Heartbeat: record that a live timer just polled, so reconcile
                // doesn't mistake this watch for stranded. Preserves generation;
                // a no-op if the record vanished (stopped) between get and touch.
                if let Some(dir) = watches_dir() {
                    touch_in(&dir, id, generation, now_ms());
                }
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
        }
    }
}

/// Stamp `last_poll_at` to mark the timer alive. Only writes when the record is
/// present and at the expected generation — a stale or stopped timer must not
/// resurrect a heartbeat.
fn touch_in(dir: &Path, id: &str, generation: u64, now: u64) {
    if let Some(mut rec) = get_in(dir, id) {
        if rec.generation == generation {
            rec.last_poll_at = now;
            // Each heartbeat is one condition poll that didn't fire; count it so the
            // session card can show how many times the watch has checked.
            rec.poll_count += 1;
            let _ = write_record(dir, &rec);
        }
    }
}

/// A watch whose timer hasn't polled in this long is treated as stranded — the
/// detached timer process died (reboot, `kill -9`) and nothing is watching.
/// Comfortably larger than [`POLL_CAP_MS`] so a timer merely mid-nap is never
/// mistaken for dead.
const STRANDED_GRACE_MS: u64 = 3 * POLL_CAP_MS;

/// Re-arm timers for watches left stranded (their detached timer died). Cheap and
/// idempotent — safe to call from any periodic hook. Bumps `generation` so any
/// still-alive old timer exits on its next wake and exactly one timer drives.
/// Returns the ids it re-armed. A stranded watch already past its deadline is
/// re-armed too; the fresh timer fires its timeout resume on the first tick.
pub fn reconcile() -> Vec<String> {
    let Some(dir) = watches_dir() else {
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

fn reconcile_in(dir: &Path, now: u64, arm: &mut dyn FnMut(&WatchRecord)) -> Vec<String> {
    let mut rearmed = Vec::new();
    for mut rec in list_in(dir) {
        if now.saturating_sub(rec.last_poll_at) <= STRANDED_GRACE_MS {
            continue; // a live timer polled recently
        }
        // Stranded: bump the generation (retiring any zombie timer) and re-arm.
        rec.generation += 1;
        rec.last_poll_at = now; // the fresh timer will heartbeat from here
        if write_record(dir, &rec).is_ok() {
            arm(&rec);
            rearmed.push(rec.id.clone());
        }
    }
    rearmed
}

/// Spawn a detached timer process (`fleet watch fire <id> <gen>`) that polls the
/// condition and fires it. Detached into its own session (`setsid`) so quitting
/// the desktop app / `fleet serve` does not take the timer down — same contract
/// as the loop timer and handoff relay. Used by `fleet watch create` and the
/// reconcile sweep.
pub fn arm_timer(rec: &WatchRecord) -> Result<u32, String> {
    let fleet = crate::hooks::resolve_fleet_binary()
        .ok_or("cannot find fleet binary to arm watch timer")?;
    arm_timer_with(&fleet, rec)
}

fn arm_timer_with(fleet_bin: &str, rec: &WatchRecord) -> Result<u32, String> {
    // process_util::command: no conhost flash on Windows. There is no setsid
    // equivalent there — a Windows child is not killed with its parent by
    // default (no job object is attached), so the detach contract holds
    // without extra flags.
    let mut cmd = crate::process_util::command(fleet_bin);
    cmd.arg("watch")
        .arg("fire")
        .arg(&rec.id)
        .arg(rec.generation.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| format!("spawn watch timer: {e}"))?;
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

    // ── P2: timer decision + fire ──────────────────────────────────────────────

    /// A timer holding a stale generation (reconcile re-armed under a newer one)
    /// exits without polling or firing.
    #[test]
    fn decide_exits_on_stale_generation() {
        let d = dir();
        let mut rec = make(d.path(), "w1", 0);
        rec.generation = 3;
        let step = decide(&rec, 2, 1_000, || panic!("must not poll a stale timer"));
        assert_eq!(step, TimerStep::Exit);
    }

    /// Past the deadline ⇒ fire a timeout resume, and the condition is never even
    /// polled (the wait is over regardless).
    #[test]
    fn decide_fires_timeout_past_deadline() {
        let d = dir();
        let rec = make(d.path(), "w1", 0); // deadline_at = DEFAULT_TIMEOUT_SECS*1000
        let step = decide(&rec, 0, rec.deadline_at + 1, || {
            panic!("must not poll after deadline")
        });
        assert_eq!(step, TimerStep::Fire { timed_out: true });
    }

    /// Condition met before the deadline ⇒ fire a normal resume.
    #[test]
    fn decide_fires_when_condition_met() {
        let d = dir();
        let rec = make(d.path(), "w1", 0);
        let step = decide(&rec, 0, 1_000, || true);
        assert_eq!(step, TimerStep::Fire { timed_out: false });
    }

    /// Not met, well before the deadline ⇒ nap one poll interval, capped at the
    /// poll cap.
    #[test]
    fn decide_naps_one_poll_interval() {
        let d = dir();
        let mut rec = make(d.path(), "w1", 0);
        rec.poll_secs = 10; // 10_000ms < POLL_CAP_MS
        let step = decide(&rec, 0, 1_000, || false);
        assert_eq!(step, TimerStep::Nap { ms: 10_000 });

        rec.poll_secs = 600; // 600_000ms — capped to POLL_CAP_MS
        let step = decide(&rec, 0, 1_000, || false);
        assert_eq!(step, TimerStep::Nap { ms: POLL_CAP_MS });
    }

    /// Near the deadline, the nap shrinks so the timeout fire is not one poll-cap
    /// late.
    #[test]
    fn decide_nap_never_overshoots_the_deadline() {
        let d = dir();
        let mut rec = make(d.path(), "w1", 0);
        rec.poll_secs = 30;
        // 3s before the deadline: nap 3s, not 30s.
        let step = decide(&rec, 0, rec.deadline_at - 3_000, || false);
        assert_eq!(step, TimerStep::Nap { ms: 3_000 });
    }

    #[test]
    fn resume_prompt_condition_vs_timeout_and_footer() {
        let d = dir();
        let rec = make(d.path(), "w1", 0); // note = "CI run 123"
        let hit = compose_resume_prompt(&rec, "conclusion=success", false);
        assert!(hit.contains("触发了"));
        assert!(hit.contains("CI run 123"));
        assert!(hit.contains("conclusion=success"));
        assert!(hit.contains("不是用户消息"));

        let out = compose_resume_prompt(&rec, "", true);
        assert!(out.contains("已超时"));
        assert!(!out.contains("conclusion"));
        assert!(out.contains("不是用户消息"));
    }

    use std::cell::RefCell;

    /// Stand-in for `interrupt_if_live` in tests that don't care about the
    /// liveness gate: the session is dead, so nothing is interrupted.
    fn dead(_session_id: &str) -> bool {
        false
    }

    /// A fire claims the slot, composes the prompt, and resumes exactly once; the
    /// record is retired so a racing second fire resumes nothing.
    #[test]
    fn fire_claims_captures_and_resumes_once() {
        let d = dir();
        make(d.path(), "w1", 0);
        let seen: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
        let capture = |_r: &WatchRecord| "conclusion=success".to_string();
        let resume = |r: &WatchRecord, prompt: &str| {
            seen.borrow_mut().push((r.session_id.clone(), prompt.to_string()));
            Ok(())
        };
        let claimed =
            fire_in(d.path(), "w1", 0, false, &capture, &dead, &resume).unwrap();
        assert_eq!(claimed.session_id, "sess-1");
        assert!(get_in(d.path(), "w1").is_none(), "fire retires the record");

        let seen = seen.into_inner();
        assert_eq!(seen.len(), 1, "resumed exactly once");
        assert_eq!(seen[0].0, "sess-1");
        assert!(seen[0].1.contains("conclusion=success"), "event text carried into prompt");
        assert!(seen[0].1.contains("触发了"));

        // a racing second fire finds the record gone and resumes nothing
        let err = fire_in(d.path(), "w1", 0, false, &capture, &dead, &|_, _| {
            panic!("must not resume after the record is consumed")
        })
        .unwrap_err();
        assert_eq!(err, ClaimError::Gone);
    }

    /// A timeout fire skips the capture command (there is no event to report) and
    /// still resumes with the timeout wording.
    #[test]
    fn timeout_fire_skips_capture() {
        let d = dir();
        make(d.path(), "w1", 0);
        let captured = RefCell::new(false);
        let capture = |_r: &WatchRecord| {
            *captured.borrow_mut() = true;
            "should-not-run".to_string()
        };
        let last = RefCell::new(String::new());
        let resume = |_r: &WatchRecord, prompt: &str| {
            *last.borrow_mut() = prompt.to_string();
            Ok(())
        };
        fire_in(d.path(), "w1", 0, true, &capture, &dead, &resume).unwrap();
        assert!(!*captured.borrow(), "timeout fire must not run the capture command");
        assert!(last.borrow().contains("已超时"));
        assert!(!last.borrow().contains("should-not-run"));
    }

    /// A watch whose session is still live when the condition fires must SIGINT
    /// that turn *before* resuming — two `claude` processes on one transcript is
    /// how the "decision card stuck unanswered + duplicate card" corruption
    /// happened (a `fleet watch` resumed a session still blocked on an
    /// AskUserQuestion). The interrupt must land before the resume, not after.
    #[test]
    fn fire_interrupts_live_session_before_resume() {
        let d = dir();
        make(d.path(), "w1", 0);
        // Ordered log of side effects so we can assert interrupt-then-resume.
        let events: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let capture = |_r: &WatchRecord| String::new();
        let interrupt = |sid: &str| {
            events.borrow_mut().push(format!("interrupt:{sid}"));
            true // the session was live and we stopped it
        };
        let resume = |r: &WatchRecord, _p: &str| {
            events.borrow_mut().push(format!("resume:{}", r.session_id));
            Ok(())
        };
        fire_in(d.path(), "w1", 0, false, &capture, &interrupt, &resume).unwrap();
        assert_eq!(
            events.into_inner(),
            vec!["interrupt:sess-1".to_string(), "resume:sess-1".to_string()],
            "a live session must be interrupted before the resume, not left to run concurrently",
        );
    }

    /// The common case: the session already died, so nothing is interrupted and
    /// the resume proceeds exactly as before.
    #[test]
    fn fire_skips_interrupt_when_session_dead() {
        let d = dir();
        make(d.path(), "w1", 0);
        let interrupted = RefCell::new(false);
        let resumed = RefCell::new(false);
        let capture = |_r: &WatchRecord| String::new();
        let interrupt = |_sid: &str| {
            *interrupted.borrow_mut() = true;
            false
        };
        let resume = |_r: &WatchRecord, _p: &str| {
            *resumed.borrow_mut() = true;
            Ok(())
        };
        fire_in(d.path(), "w1", 0, false, &capture, &interrupt, &resume).unwrap();
        // `interrupt_if_live` is still *consulted* (it is what reports liveness),
        // but it reports dead, so no SIGINT and a normal resume.
        assert!(*resumed.borrow(), "a dead session still gets resumed");
    }

    /// A resume failure is swallowed: the record is already consumed, so the fire
    /// still returns Ok (re-arming would risk a double-resume).
    #[test]
    fn resume_failure_is_swallowed() {
        let d = dir();
        make(d.path(), "w1", 0);
        let capture = |_r: &WatchRecord| String::new();
        let resume = |_r: &WatchRecord, _p: &str| Err("claude --resume boom".to_string());
        let claimed = fire_in(d.path(), "w1", 0, false, &capture, &dead, &resume);
        assert!(claimed.is_ok(), "fire returns Ok even when resume fails");
        assert!(get_in(d.path(), "w1").is_none(), "record still retired");
    }

    /// A stale timer that reaches fire (e.g. deadline hit while superseded) claims
    /// nothing — the winner already holds the live generation.
    #[test]
    fn fire_with_stale_generation_is_refused() {
        let d = dir();
        let mut rec = make(d.path(), "w1", 0);
        rec.generation = 5;
        write_record(d.path(), &rec).unwrap();
        let err = fire_in(
            d.path(),
            "w1",
            4,
            false,
            &|_r| String::new(),
            &dead,
            &|_r, _p| panic!("stale timer must not resume"),
        )
        .unwrap_err();
        assert_eq!(err, ClaimError::StaleGeneration { expected: 4, found: 5 });
        assert!(get_in(d.path(), "w1").is_some(), "record untouched for the live timer");
    }

    // ── P3: heartbeat + reconcile ──────────────────────────────────────────────

    #[test]
    fn touch_updates_heartbeat_only_at_the_right_generation() {
        let d = dir();
        make(d.path(), "w1", 1_000); // last_poll_at = 1_000, gen 0
        touch_in(d.path(), "w1", 0, 50_000);
        assert_eq!(get_in(d.path(), "w1").unwrap().last_poll_at, 50_000);
        // wrong generation: no update
        touch_in(d.path(), "w1", 9, 90_000);
        assert_eq!(get_in(d.path(), "w1").unwrap().last_poll_at, 50_000);
        // gone: no panic, no resurrection
        stop_in(d.path(), "w1");
        touch_in(d.path(), "w1", 0, 99_000);
        assert!(get_in(d.path(), "w1").is_none());
    }

    /// Each poll heartbeat also counts one condition check, so a waiting session's
    /// card can show "checked N times". `decide` runs the probe exactly once per
    /// wake and every not-yet-firing wake ends in `touch_in`, so per-touch counting
    /// is per-probe counting. A wrong-generation touch (a superseded zombie timer)
    /// must not inflate the count, just like it doesn't move the heartbeat.
    #[test]
    fn touch_increments_the_poll_count() {
        let d = dir();
        make(d.path(), "w1", 1_000);
        assert_eq!(get_in(d.path(), "w1").unwrap().poll_count, 0, "starts at zero");
        touch_in(d.path(), "w1", 0, 50_000);
        touch_in(d.path(), "w1", 0, 80_000);
        touch_in(d.path(), "w1", 0, 110_000);
        assert_eq!(
            get_in(d.path(), "w1").unwrap().poll_count,
            3,
            "one increment per poll heartbeat"
        );
        // wrong generation: neither the heartbeat nor the count moves
        touch_in(d.path(), "w1", 9, 140_000);
        assert_eq!(get_in(d.path(), "w1").unwrap().poll_count, 3);
    }

    /// Only a watch whose heartbeat is older than the grace window is re-armed;
    /// one polled recently (timer alive / mid-nap) is left alone.
    #[test]
    fn reconcile_rearms_only_stranded_watches() {
        let d = dir();
        // "healthy": polled just now
        let mut healthy = make(d.path(), "healthy", 0);
        healthy.last_poll_at = 1_000_000;
        write_record(d.path(), &healthy).unwrap();
        // "napping": polled 20s ago — inside the grace window
        let mut napping = make(d.path(), "napping", 0);
        napping.last_poll_at = 1_000_000 - 20_000;
        write_record(d.path(), &napping).unwrap();
        // "stranded": no poll in well over the grace window — its timer is dead
        let mut stranded = make(d.path(), "stranded", 0);
        stranded.last_poll_at = 1_000_000 - STRANDED_GRACE_MS - 5_000;
        write_record(d.path(), &stranded).unwrap();

        let mut armed = Vec::new();
        let rearmed = reconcile_in(d.path(), 1_000_000, &mut |r| armed.push(r.id.clone()));

        assert_eq!(rearmed, vec!["stranded"], "only the stranded watch is re-armed");
        assert_eq!(armed, vec!["stranded"]);
        // its generation was bumped so the zombie timer exits on stale-gen
        assert_eq!(get_in(d.path(), "stranded").unwrap().generation, 1);
        // and the re-arm resets its heartbeat forward
        assert_eq!(get_in(d.path(), "stranded").unwrap().last_poll_at, 1_000_000);
        // healthy untouched
        assert_eq!(get_in(d.path(), "healthy").unwrap().generation, 0);
    }

    /// A stranded watch already past its deadline is still re-armed; the fresh
    /// timer's first tick fires the timeout resume rather than leaving it orphaned.
    #[test]
    fn reconcile_rearms_a_stranded_expired_watch() {
        let d = dir();
        let mut rec = make(d.path(), "w1", 0);
        // way past deadline, and no heartbeat for ages
        rec.last_poll_at = 0;
        write_record(d.path(), &rec).unwrap();
        let now = rec.deadline_at + STRANDED_GRACE_MS + 1;

        let mut armed = Vec::new();
        let rearmed = reconcile_in(d.path(), now, &mut |r| armed.push(r.id.clone()));
        assert_eq!(rearmed, vec!["w1"]);
        // the re-armed record is still expired, so its timer will Fire{timed_out:true}
        let back = get_in(d.path(), "w1").unwrap();
        assert!(back.is_expired(now));
        assert_eq!(decide(&back, back.generation, now, || false), TimerStep::Fire { timed_out: true });
    }
}
