//! Auto-resume configuration + policy for rate-limited sessions.
//!
//! When `enabled`, the scheduler will spawn `claude --resume <id> -p continue`
//! as soon as a rate-limited session's `resets_at` timestamp has passed.
//! `max_wait_hours` is a safety guard: if the total wait (error → reset) is
//! longer than this, the session is left alone so the user is not surprised
//! by a job that resumes 24h later.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoResumeConfig {
    pub enabled: bool,
    /// Don't auto-resume if the configured wait window is longer than this.
    /// Manual "Resume now" always works; this only gates the scheduler.
    pub max_wait_hours: u64,
    /// Auto-retry Fleet-headless sessions that hit a transient `server_error`
    /// (the "Server error mid-response / Response stalled mid-stream" family).
    /// Independent of the rate-limit `enabled` gate above? No — it is ANDed with
    /// `enabled`, so turning the whole feature off also stops server-error
    /// retries; this flag lets a user keep rate-limit resume on but opt out of
    /// server-error retries specifically.
    pub retry_server_errors: bool,
    /// Give up after this many consecutive server-error retries for one session
    /// so a persistently-failing turn (or a server that stays down) stops
    /// re-firing forever. Enforced by the scheduler's per-session retry counter.
    pub max_server_error_retries: u32,
}

impl Default for AutoResumeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_wait_hours: 12,
            retry_server_errors: true,
            max_server_error_retries: 3,
        }
    }
}

fn config_path() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("auto_resume.json"))
}

impl AutoResumeConfig {
    pub fn load() -> Self {
        let Some(path) = config_path() else { return Self::default() };
        let Ok(content) = std::fs::read_to_string(&path) else { return Self::default() };
        serde_json::from_str(&content).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path().ok_or_else(|| "no fleet dir".to_string())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let content = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, content).map_err(|e| e.to_string())
    }
}

/// Wait this long past `resets_at` before firing — Anthropic's server can
/// still hand back 429 for tens of seconds right at the boundary, and burning
/// a spawn on a 429 produces exactly the silent-failure pattern we saw at
/// 21:10 / 21:12 on 2026-04-25.
pub const RESET_GRACE: chrono::Duration = chrono::Duration::seconds(60);

/// Utilization (0–1 fraction) below which a limit's window is treated as
/// "usable again". A rate-limited account pins the relevant metric right up
/// near 1.0; once the window resets — or foxy-switcher swaps in a fresh
/// account — it drops well below this. Set conservatively high so a metric
/// still sitting at 0.97 doesn't get mistaken for recovered and burn a 429.
pub const RECOVERED_UTILIZATION: f64 = 0.95;

/// Which sampled usage metric reflects a given rate-limit type. `None` for
/// limit types that don't map to exactly one sampled metric — those fall back
/// to the hint-time gate alone (there's no single utilization number to read).
fn metric_for_limit(limit_type: crate::rate_limit_parser::RateLimitType) -> Option<&'static str> {
    use crate::rate_limit_parser::RateLimitType::*;
    match limit_type {
        SessionLimit => Some("five_hour"),
        // Opus limit is a weekly cap; the 7-day metric is the closest signal.
        WeeklyLimit | OpusLimit => Some("seven_day"),
        SonnetLimit => Some("seven_day_sonnet"),
        // Generic / overage / unknown have no dedicated metric — hint gate only.
        UsageLimit | OutOfExtraUsage | Unknown => None,
    }
}

/// Whether the account limit behind a rate-limit has actually come back,
/// judged purely from the latest cached usage snapshot — **no network call**
/// (the snapshot is read off disk by [`crate::account::latest_usage_snapshot`]).
///
/// Returns `true` only when BOTH hold:
/// - the snapshot post-dates the rate-limit error (`snap.ts > error_timestamp`).
///   A snapshot older than the error reflects the *pre-limit* past, so its low
///   utilization says nothing about recovery — this guard is what makes a
///   cache-only, no-extra-request check safe even when the sampler is stale.
/// - the metric matching this limit type has dropped below
///   [`RECOVERED_UTILIZATION`] — or is absent from a snapshot whose other
///   metrics are populated (the usage API omits zero-usage windows, so
///   absence on a successful sample means the window is empty).
///
/// This covers both a plain window reset and a foxy-switcher account swap:
/// either way the *live* account's utilization for that metric is now low.
pub fn limit_recovered(
    rl: &crate::session::RateLimitState,
    snapshot: Option<&crate::account::UsageHistoryPoint>,
) -> bool {
    let Some(snap) = snapshot else { return false };
    let Some(metric) = metric_for_limit(rl.limit_type) else { return false };
    if snap.ts <= rl.error_timestamp.timestamp_millis() {
        return false;
    }
    let util = match metric {
        "five_hour" => snap.five_hour,
        "seven_day" => snap.seven_day,
        "seven_day_sonnet" => snap.seven_day_sonnet,
        _ => None,
    };
    match util {
        Some(u) => u < RECOVERED_UTILIZATION,
        // The usage API omits a window with zero usage in it, so a fresh
        // snapshot whose other metrics are populated but whose target metric
        // is absent means this window is *empty* — the strongest recovery
        // signal (a just-swapped-in account has no five_hour entry at all;
        // observed 2026-07-18, where this gate stayed shut for 12 minutes
        // until another session burned usage into the window). A fully-empty
        // snapshot proves nothing — could be a failed sample — and stays inert.
        None => {
            [snap.five_hour, snap.seven_day, snap.seven_day_sonnet]
                .iter()
                .any(|m| m.is_some())
        }
    }
}

/// Decide whether a rate-limited session is eligible for auto-resume *right now*.
///
/// Returns `true` only when ALL of:
/// - `config.enabled`
/// - the session is NOT a subagent (`agent-*` transcripts can't be resumed)
/// - the session is NOT attached to an interactive IDE (`ide_name == None`);
///   IDE (VS Code / Claude app) sessions must not be resumed headlessly
/// - the session is in `RateLimited` state with a `rate_limit` payload
/// - EITHER `now >= resets_at + RESET_GRACE` (the hint wait has elapsed plus a
///   grace window so we don't race the reset boundary) OR [`limit_recovered`]
///   reports the account's live utilization for this limit has already dropped
///   below [`RECOVERED_UTILIZATION`] (a window reset or a foxy-switcher account
///   swap made capacity available ahead of the hinted `resets_at`)
/// - `resets_at - error_timestamp <= max_wait_hours` (the configured window
///   was short enough that unattended resume is safe). This outer safety valve
///   applies to BOTH the hint-elapsed and limit-recovered paths — it encodes
///   the user's "don't auto-resume jobs that were meant to wait longer than N
///   hours" intent, which an early recovery doesn't override.
///
/// `usage` is the latest cached usage snapshot (pass `None` to gate on the hint
/// time alone); it is read cache-only by the caller, never fetched here.
pub fn should_auto_resume(
    session: &crate::session::SessionInfo,
    config: &AutoResumeConfig,
    now: chrono::DateTime<chrono::Utc>,
    usage: Option<&crate::account::UsageHistoryPoint>,
) -> bool {
    if !config.enabled {
        return false;
    }
    // Subagent transcripts (`agent-*.jsonl`) are not independently resumable:
    // `claude --resume agent-X` always fails, leaving the session RateLimited
    // so it re-fires forever. Never treat a subagent as a resume candidate.
    if session.is_subagent {
        return false;
    }
    // Only Fleet-spawned headless sessions are auto-resumable. A session running
    // inside an interactive IDE (VS Code / Claude app) writes an ide
    // lock file, so `ide_name` is Some — firing `claude --resume <id> -p continue`
    // as a detached headless process behind the user's open editor is not what
    // they want. Headless Fleet-spawned `claude --print` sessions have no lock
    // (`ide_name == None`) and are the only ones we resume.
    if session.ide_name.is_some() {
        return false;
    }
    // Source is no longer a hard gate here: the resume *form* is dispatched by
    // `agent_source` at spawn time (claude → `claude --resume`, codex →
    // `codex exec resume`), so eligibility is decided purely by the rate-limit
    // payload below. A codex session's `rate_limit` is built by
    // [`crate::codex_source::codex_rate_limit_state_from_usage`] (M2 P7), which
    // encodes codex's own reset semantics (epoch-seconds `resetsAt`, weekly vs
    // secondary windows) into the same `RateLimitState` this predicate reads —
    // so the gate below is source-agnostic. Live population of that payload onto
    // a rate-limited codex session lands with the Fleet-owned marking (M3 P9)
    // and is validated end-to-end in P8.
    if session.status != crate::session::SessionStatus::RateLimited {
        return false;
    }
    let Some(rl) = session.rate_limit.as_ref() else { return false };
    let hint_elapsed = now >= rl.resets_at + RESET_GRACE;
    if !hint_elapsed && !limit_recovered(rl, usage) {
        return false;
    }
    let wait = rl.resets_at - rl.error_timestamp;
    let max_wait = chrono::Duration::hours(config.max_wait_hours as i64);
    wait <= max_wait
}

/// Pick which sessions to auto-resume on this tick, bounded by the number of
/// available concurrency `slots`.
///
/// Without a bound, a single rescan tick can find hundreds of eligible
/// sessions at once and fire `claude --resume` for every one of them
/// simultaneously — each spawned process is ~150-200MB, so a few hundred at
/// once is tens of GB of RSS (the 40GB runaway). `slots` is `cap - in_flight`
/// computed by the caller, so the live auto-resume process count never exceeds
/// the cap.
///
/// `skip(id)` returns true for sessions that should be passed over this tick
/// (e.g. still within the debounce window, or backed off after repeated
/// failures). Results preserve session-list order and never exceed `slots`.
///
/// `usage` is the latest cached usage snapshot, read once by the caller and
/// shared across every session's [`should_auto_resume`] check this tick.
/// Could a resume spawned into this workspace possibly start?
///
/// `spawn_claude_detached` refuses a cwd that isn't a directory, so a session
/// whose workspace has vanished fails before any process exists — the scheduler
/// then just backs off and tries again on the next window, forever, with
/// nothing but a debug-log line to show for it. Two cases are deliberately NOT
/// treated as missing:
/// - a TCC-protected path, because the only way to check it is the `stat` that
///   fires the macOS permission dialog. Fleet never withholds an action on the
///   strength of a check it refuses to run.
/// - a registered remote workspace, whose local mirror directory is created at
///   launch time by `remote_workspace::wrap_launch` — its absence right now
///   says nothing about whether the resume would succeed.
fn can_reach_workspace(path: &str) -> bool {
    let p = std::path::Path::new(path);
    crate::tcc::is_tcc_protected(p)
        || p.is_dir()
        || crate::remote_workspace::find_for_path(path).is_some()
}

pub fn select_resume_candidates(
    sessions: &[crate::session::SessionInfo],
    config: &AutoResumeConfig,
    now: chrono::DateTime<chrono::Utc>,
    usage: Option<&crate::account::UsageHistoryPoint>,
    skip: impl Fn(&str) -> bool,
    slots: usize,
) -> Vec<(String, String)> {
    sessions
        .iter()
        .filter(|s| should_auto_resume(s, config, now, usage))
        // Never resume a session whose CLI is still live: a rate-limit status
        // can lag behind a session that already recovered (or a resume already
        // in flight), and firing `claude --resume` on a live turn puts a second
        // process on the same transcript — the corruption the watch gate guards
        // against. The server-error retry path skips `proc_alive` in its caller
        // closure; centralise it here so both schedulers (local + remote) get it.
        .filter(|s| !s.proc_alive)
        .filter(|s| can_reach_workspace(&s.workspace_path))
        .filter(|s| !skip(&s.id))
        .take(slots)
        .map(|s| (s.id.clone(), s.workspace_path.clone()))
        .collect()
}

/// Decide whether a `ServerErrored` session is eligible for an immediate retry.
///
/// Unlike [`should_auto_resume`] there is NO `resets_at` / usage gate — a
/// server error is transient, so the resume fires as soon as the scheduler's
/// debounce and per-session attempt cap allow. Eligibility requires ALL of:
/// - `config.enabled` AND `config.retry_server_errors` (the feature is on)
/// - the session is NOT a subagent (`agent-*` transcripts can't be resumed)
/// - the session is NOT attached to an interactive IDE (`ide_name == None`) —
///   same rule as auto-resume: never fire a detached headless `claude --resume`
///   behind the user's open editor
/// - the session is in `ServerErrored` state
pub fn should_retry_server_error(
    session: &crate::session::SessionInfo,
    config: &AutoResumeConfig,
) -> bool {
    if !config.enabled || !config.retry_server_errors {
        return false;
    }
    // Same non-resumable gates as auto-resume: subagent transcripts can't be
    // resumed, and IDE-attached sessions must not be resumed headlessly behind
    // the user's editor.
    if session.is_subagent || session.ide_name.is_some() {
        return false;
    }
    session.status == crate::session::SessionStatus::ServerErrored
}

/// Pick which `ServerErrored` sessions to retry this tick, bounded by `slots`
/// (the free concurrency slots the caller computed as `cap - in_flight`) — the
/// same runaway guard [`select_resume_candidates`] uses. `skip(id)` passes over
/// sessions still within the debounce window or that have exhausted their
/// per-session server-error retry budget. Results preserve session-list order
/// and never exceed `slots`.
pub fn select_server_error_retries(
    sessions: &[crate::session::SessionInfo],
    config: &AutoResumeConfig,
    skip: impl Fn(&str) -> bool,
    slots: usize,
) -> Vec<(String, String)> {
    sessions
        .iter()
        .filter(|s| should_retry_server_error(s, config))
        // Same gate as the rate-limit path: a workspace we can prove is gone
        // cannot host a spawn, so retrying it only burns slots and log lines.
        .filter(|s| can_reach_workspace(&s.workspace_path))
        .filter(|s| !skip(&s.id))
        .take(slots)
        .map(|s| (s.id.clone(), s.workspace_path.clone()))
        .collect()
}

/// A session is "backed off" once its resume has failed `threshold` consecutive
/// times. Backed-off sessions are skipped by the scheduler so a resume that
/// keeps failing (e.g. a session that can never make progress) stops being
/// re-fired forever — that endless re-fire is what produced 24k+ spawns in the
/// field. A later success clears the count (see [`record_resume_outcome`]).
pub fn is_backed_off(
    failures: &std::collections::HashMap<String, u32>,
    id: &str,
    threshold: u32,
) -> bool {
    failures.get(id).is_some_and(|&n| n >= threshold)
}

/// Update a session's consecutive-failure count after a resume attempt exits.
/// A successful exit resets the count (entry removed); a failure increments it.
pub fn record_resume_outcome(
    failures: &mut std::collections::HashMap<String, u32>,
    id: &str,
    success: bool,
) {
    if success {
        failures.remove(id);
    } else {
        *failures.entry(id.to_string()).or_insert(0) += 1;
    }
}

/// Request body for the remote `POST /resume_session` endpoint.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ResumeSessionRequest {
    pub session_id: String,
    pub workspace_path: String,
    /// Follow-up prompt for the resumed turn; `None`/empty = "continue".
    #[serde(default)]
    pub prompt: Option<String>,
    /// Optional `--model` override for the resumed session.
    #[serde(default)]
    pub model: Option<String>,
    /// Optional `--effort` override for the resumed session.
    #[serde(default)]
    pub effort: Option<String>,
    /// Optional `--permission-mode` override (validated against
    /// [`crate::session_launch::PERMISSION_MODES`]).
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// Session's source ("claude-code" / "codex"), so the probe routes the
    /// resume to the right launcher. Blank/absent (older desktop clients)
    /// defaults to claude, preserving backward compatibility.
    #[serde(default)]
    pub agent_source: String,
}

/// Headlessly resume a rate-limited session by spawning
/// `claude --resume <session_id> -p "continue"` detached in the given workspace.
pub fn spawn_resume(session_id: &str, workspace_path: &str) -> Result<(), String> {
    spawn_resume_prompt(session_id, workspace_path, "continue", None, None, None)
}

/// [`spawn_resume`] with a caller-supplied follow-up prompt (the history
/// panel's "恢复会话" box). Empty/whitespace prompts fall back to "continue".
/// `model` / `effort` / `permission_mode` mirror the new-session overrides and
/// are passed through to the resumed `claude` invocation when non-blank.
pub fn spawn_resume_prompt(
    session_id: &str,
    workspace_path: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    permission_mode: Option<&str>,
) -> Result<(), String> {
    spawn_resume_tracked_prompt(
        session_id,
        workspace_path,
        prompt,
        model,
        effort,
        permission_mode,
        |_success| {},
    )
}

/// Like [`spawn_resume`] but invokes `on_exit(success)` from the reaper thread
/// when the spawned process exits. The auto-resume scheduler uses this to
/// decrement its in-flight counter (concurrency cap) and to record consecutive
/// failures for backoff — see `maybe_fire_auto_resume`.
pub fn spawn_resume_tracked(
    session_id: &str,
    workspace_path: &str,
    on_exit: impl FnOnce(bool) + Send + 'static,
) -> Result<(), String> {
    spawn_resume_tracked_prompt(session_id, workspace_path, "continue", None, None, None, on_exit)
}

/// Full-featured Claude resume: `claude --resume <id> -p <prompt>` with
/// optional `--model` / `--effort` / `--permission-mode` overrides, live-thinking
/// tee, and a reaper that invokes `on_exit(success)`. All the other
/// `spawn_resume*` helpers are thin wrappers over this; [`ClaudeCodeSource::resume`]
/// calls it directly so the resume dispatch stays per-source.
#[allow(clippy::too_many_arguments)]
pub fn spawn_resume_tracked_prompt(
    session_id: &str,
    workspace_path: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    permission_mode: Option<&str>,
    on_exit: impl FnOnce(bool) + Send + 'static,
) -> Result<(), String> {
    let prompt = prompt.trim();
    let prompt = if prompt.is_empty() { "continue" } else { prompt };
    // Validate overrides (permission mode) before any CLI/filesystem checks so
    // the frontend gets a stable error regardless of the host environment.
    let mut override_args = Vec::new();
    crate::session_launch::push_session_override_args(
        &mut override_args,
        model,
        effort,
        permission_mode,
    )?;
    let (found, claude_path) = crate::check_cli_installed();
    if !found {
        return Err("Claude CLI not found on PATH".to_string());
    }
    let claude = claude_path.unwrap_or_else(|| "claude".to_string());
    // A resume may carry its own `--model` / `--effort`; record them so the note
    // describes what the session is running *now*, not only what it first
    // launched with. A resume with no overrides records nothing and leaves the
    // original note standing.
    crate::launch_spec::record(session_id, model, effort);
    let stderr_log = crate::session::get_fleet_dir()
        .map(|d| d.join("auto_resume_stderr.log"))
        .ok_or_else(|| "no fleet dir".to_string())?;
    crate::log_debug(&format!(
        "resume_session: claude --resume {} -p <{} chars> (cwd={}, stderr_log={})",
        session_id,
        prompt.len(),
        workspace_path,
        stderr_log.display()
    ));
    let pid = spawn_resume_with_path(
        &claude,
        session_id,
        workspace_path,
        prompt,
        &override_args,
        &stderr_log,
        on_exit,
    )?;
    crate::log_debug(&format!(
        "resume_session: spawned pid {} for session {}",
        pid, session_id
    ));
    Ok(())
}

/// Spawn `claude --resume <id> -p <prompt>` with stderr redirected to
/// `stderr_log` and a background thread that reaps the child and records its
/// exit status — so failures stop being silent and we don't accumulate zombies.
/// Thin wrapper over the generic [`crate::session_launch::spawn_claude_detached`].
/// `override_args` carries the pre-validated `--model` / `--effort` /
/// `--permission-mode` flags (empty for the auto-resume scheduler).
#[allow(clippy::too_many_arguments)]
fn spawn_resume_with_path(
    claude_path: &str,
    session_id: &str,
    workspace_path: &str,
    prompt: &str,
    override_args: &[String],
    stderr_log: &Path,
    on_exit: impl FnOnce(bool) + Send + 'static,
) -> Result<u32, String> {
    let mut args = vec![
        "--resume".to_string(),
        session_id.to_string(),
        "-p".to_string(),
        prompt.to_string(),
    ];
    // Stream the token-level thinking to stdout so it can be teed to a
    // live-thinking sidecar (same rationale as `session_launch::spawn_new_session`;
    // stdout was otherwise discarded, the JSONL transcript is unaffected).
    args.extend(crate::session_launch::live_thinking_stream_args());
    args.extend(override_args.iter().cloned());
    // Route native permission prompts to Fleet's Decision Panel instead of
    // headless auto-deny (no-op when the fleet MCP server isn't injected).
    args.extend(crate::session_launch::permission_prompt_tool_args());
    // Keep a chat a chat on every turn after the first: without these, the
    // resumed process would reload the global doctrine the initial spawn
    // deliberately excluded (no-op outside the chat workspace).
    args.extend(crate::chat_workspace::chat_launch_args(workspace_path));
    crate::session_launch::spawn_claude_detached_with_envs(
        claude_path,
        &args,
        workspace_path,
        stderr_log,
        "auto_resume",
        &format!("session={}", session_id),
        &[],
        true, // tee stdout to a live-thinking sidecar
        on_exit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rate_limit_parser::RateLimitType;
    use crate::session::{RateLimitState, SessionInfo, SessionStatus};
    use chrono::{Duration, Utc};

    fn mk_session(status: SessionStatus, rl: Option<RateLimitState>) -> SessionInfo {
        SessionInfo {
            id: "s1".into(),
            // A directory that really exists: selection now refuses a workspace
            // it can prove is gone (`can_reach_workspace`), so a made-up path
            // would make every eligibility test vacuously pass.
            workspace_path: std::env::temp_dir().to_string_lossy().into_owned(),
            workspace_name: "w".into(),
            ide_name: None,
            entrypoint: None,
            is_subagent: false,
            fleet_spawned: false,
            parent_session_id: None,
            agent_type: None,
            agent_description: None,
            slug: None,
            ai_title: None,
            status,
            token_speed: 0.0,
            agent_token_speed: 0.0,
            total_output_tokens: 0,
            reasoning_output_tokens: 0,
            total_input_tokens: 0,
            total_cost_usd: 0.0,
            agent_total_cost_usd: 0.0,
            cost_speed_usd_per_min: 0.0,
            last_message_preview: None,
            last_activity_ms: 0,
            agent_last_activity_ms: 0,
            running_subagent_count: 0,
            created_at_ms: 0,
            jsonl_path: "/w/s1.jsonl".into(),
            model: None,
            thinking_level: None,
            pid: None,
            pid_precise: false,
            proc_alive: false,
            pending_tool_batch: false,
            last_skill: None,
            context_percent: None,
            agent_source: "claude-code".into(),
            last_outcome: None,
            rate_limit: rl,
            todos: None,
            background_tasks: Vec::new(),
            task_plan: None, handoff: None, user_mark: None, title_override: None, last_read_ms: None,            compact_count: 0,
            compact_pre_tokens: 0,
            compact_post_tokens: 0,
            compact_cost_usd: 0.0,
            pending_messages: Vec::new(),
            watches: Vec::new(),
        }
    }

    fn mk_rl(resets_in_minutes: i64, wait_hours: i64) -> RateLimitState {
        let resets_at = Utc::now() + Duration::minutes(resets_in_minutes);
        let error_timestamp = resets_at - Duration::hours(wait_hours);
        RateLimitState {
            resets_at,
            limit_type: RateLimitType::SessionLimit,
            parsed: true,
            error_timestamp,
        }
    }

    #[test]
    fn eligible_when_reset_passed_and_wait_within_window() {
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        // reset was 2 minutes ago — past the 60s grace — original wait was 5h
        let s = mk_session(
            SessionStatus::RateLimited,
            Some(mk_rl(-2, 5)),
        );
        assert!(should_auto_resume(&s, &cfg, Utc::now(), None));
    }

    fn usage(ts_ms: i64, five_hour: Option<f64>) -> crate::account::UsageHistoryPoint {
        crate::account::UsageHistoryPoint { ts: ts_ms, five_hour, ..Default::default() }
    }

    #[test]
    fn limit_recovered_true_when_fresh_snapshot_below_threshold() {
        // error was ~4h ago (resets 1h out, 5h window); snapshot taken now with
        // five_hour utilization well below the 0.95 recovered threshold.
        let rl = mk_rl(60, 5);
        let snap = usage(Utc::now().timestamp_millis(), Some(0.10));
        assert!(limit_recovered(&rl, Some(&snap)));
    }

    #[test]
    fn limit_recovered_false_when_snapshot_predates_error() {
        // A snapshot older than the rate-limit error reflects the pre-limit past,
        // so its low utilization must NOT be read as recovery — this is the guard
        // that keeps the cache-only check safe against a stale sampler.
        let rl = mk_rl(60, 5);
        let stale_ts = rl.error_timestamp.timestamp_millis() - 1;
        let snap = usage(stale_ts, Some(0.10));
        assert!(!limit_recovered(&rl, Some(&snap)));
    }

    #[test]
    fn limit_recovered_false_when_metric_still_high() {
        let rl = mk_rl(60, 5);
        let snap = usage(Utc::now().timestamp_millis(), Some(0.97));
        assert!(!limit_recovered(&rl, Some(&snap)));
    }

    #[test]
    fn limit_recovered_true_when_target_metric_absent_but_snapshot_populated() {
        // The usage API omits a window that has zero usage: right after an
        // account swap the fresh account's response carries seven_day but no
        // five_hour at all (observed 2026-07-18 20:35–20:47, where the gate
        // stayed shut for 12 minutes). An absent target metric on an
        // otherwise-populated fresh snapshot means the window is empty — the
        // strongest recovery signal, not a missing one.
        let rl = mk_rl(60, 5); // SessionLimit → five_hour metric
        let snap = crate::account::UsageHistoryPoint {
            ts: Utc::now().timestamp_millis(),
            five_hour: None,
            seven_day: Some(0.59),
            ..Default::default()
        };
        assert!(limit_recovered(&rl, Some(&snap)));
    }

    #[test]
    fn limit_recovered_false_when_snapshot_fully_empty() {
        // A snapshot with no metrics at all proves nothing about the account
        // (failed or degenerate sample) — it must NOT be read as recovery.
        let rl = mk_rl(60, 5);
        let snap = usage(Utc::now().timestamp_millis(), None);
        assert!(!limit_recovered(&rl, Some(&snap)));
    }

    #[test]
    fn limit_recovered_false_for_limit_type_without_metric() {
        // UsageLimit maps to no single sampled metric — the utilization gate is
        // inapplicable, so it always falls back to the hint-time gate.
        let mut rl = mk_rl(60, 5);
        rl.limit_type = RateLimitType::UsageLimit;
        let snap = usage(Utc::now().timestamp_millis(), Some(0.05));
        assert!(!limit_recovered(&rl, Some(&snap)));
    }

    #[test]
    fn eligible_via_recovery_before_hint_elapsed() {
        // Hint says reset is still 60 min away, so the hint gate blocks. But the
        // live account's five_hour metric has dropped to 0.1 — a window reset or
        // foxy account swap — so the recovery path makes it eligible now.
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let s = mk_session(SessionStatus::RateLimited, Some(mk_rl(60, 5)));
        let snap = usage(Utc::now().timestamp_millis(), Some(0.10));
        assert!(!should_auto_resume(&s, &cfg, Utc::now(), None)); // hint gate alone: blocked
        assert!(should_auto_resume(&s, &cfg, Utc::now(), Some(&snap))); // recovery: eligible
    }

    #[test]
    fn recovery_still_bounded_by_max_wait() {
        // Even with the account fully recovered, a job whose original wait
        // exceeds max_wait_hours stays blocked — the safety valve applies to the
        // recovery path too, not just the hint path.
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let s = mk_session(SessionStatus::RateLimited, Some(mk_rl(60, 24)));
        let snap = usage(Utc::now().timestamp_millis(), Some(0.05));
        assert!(!should_auto_resume(&s, &cfg, Utc::now(), Some(&snap)));
    }

    #[test]
    fn blocked_when_still_waiting() {
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        // reset is 10 minutes away
        let s = mk_session(SessionStatus::RateLimited, Some(mk_rl(10, 5)));
        assert!(!should_auto_resume(&s, &cfg, Utc::now(), None));
    }

    #[test]
    fn blocked_when_wait_exceeds_max() {
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        // reset was 1 min ago, but the original wait was 24h (weekly limit) — skip.
        let s = mk_session(SessionStatus::RateLimited, Some(mk_rl(-1, 24)));
        assert!(!should_auto_resume(&s, &cfg, Utc::now(), None));
    }

    #[test]
    fn blocked_when_disabled() {
        let cfg = AutoResumeConfig { enabled: false, max_wait_hours: 12, ..Default::default() };
        let s = mk_session(SessionStatus::RateLimited, Some(mk_rl(-1, 5)));
        assert!(!should_auto_resume(&s, &cfg, Utc::now(), None));
    }

    #[test]
    fn blocked_when_ide_attached() {
        // A session running inside an interactive IDE (VS Code / Claude app)
        // writes an ide lock, so `ide_name` is Some. Headlessly firing
        // `claude --resume <id> -p continue` behind the user's editor is exactly
        // what we must not do — auto-resume is only for Fleet-spawned headless
        // sessions (`ide_name == None`).
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let mut s = mk_session(SessionStatus::RateLimited, Some(mk_rl(-2, 5)));
        s.ide_name = Some("Visual Studio Code".into());
        assert!(!should_auto_resume(&s, &cfg, Utc::now(), None));
    }

    #[test]
    fn source_no_longer_gates_eligibility() {
        // M2 P6 dismantled the hard `agent_source == "claude-code"` gate:
        // eligibility is now decided by the rate-limit payload alone, and the
        // resume *form* is dispatched by source at spawn time. A codex session
        // that carries a valid, elapsed rate_limit is therefore eligible — same
        // as an equivalent claude-code session.
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let mut s = mk_session(SessionStatus::RateLimited, Some(mk_rl(-2, 5)));
        s.agent_source = "codex".into();
        assert!(should_auto_resume(&s, &cfg, Utc::now(), None));
    }

    /// Build a codex account rate-limit snapshot with the given reached window,
    /// then feed it through the real codex→RateLimitState converter (M2 P7) so
    /// these tests exercise the actual codex reset judgment, not a hand-rolled
    /// RateLimitState.
    fn codex_state(
        reached: &str,
        window_mins: i64,
        resets_at: chrono::DateTime<chrono::Utc>,
    ) -> RateLimitState {
        let usage = crate::codex_source::CodexUsageItem {
            primary: Some(crate::codex_source::CodexRateLimitWindow {
                used_percent: 100,
                window_duration_mins: Some(window_mins),
                resets_at: Some(resets_at.timestamp()),
            }),
            rate_limit_reached_type: Some(reached.to_string()),
            ..Default::default()
        };
        crate::codex_source::codex_rate_limit_state_from_usage(&usage, Utc::now())
            .expect("reached snapshot → Some")
    }

    #[test]
    fn codex_secondary_window_eligible_once_reset_passed() {
        // A codex secondary (short, 5h) window whose reset has passed is
        // auto-resume eligible — same as a claude session limit. Source is no
        // longer a gate (P6); the codex reset judgment (P7) supplies the state.
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let mut s = mk_session(SessionStatus::RateLimited, None);
        s.agent_source = "codex".into();
        // reset was 2 min ago (past the 60s grace); 5h window → wait 5h ≤ 12h.
        s.rate_limit = Some(codex_state("secondary", 300, Utc::now() - Duration::minutes(2)));
        assert!(should_auto_resume(&s, &cfg, Utc::now(), None));
    }

    #[test]
    fn codex_weekly_window_blocked_by_max_wait() {
        // A codex primary (weekly, 10080-min) window is too long to auto-resume
        // unattended — the max_wait_hours valve blocks it exactly like a claude
        // weekly limit, because error_timestamp is set to the window start.
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let mut s = mk_session(SessionStatus::RateLimited, None);
        s.agent_source = "codex".into();
        // Even with the reset already passed, the 7-day wait exceeds max_wait.
        s.rate_limit = Some(codex_state("primary", 10080, Utc::now() - Duration::minutes(2)));
        assert!(!should_auto_resume(&s, &cfg, Utc::now(), None));
    }

    #[test]
    fn codex_secondary_window_blocked_before_reset() {
        // Before the secondary window's reset (+grace), the hint gate blocks —
        // and codex has no claude usage metric, so there's no early-recovery path.
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let mut s = mk_session(SessionStatus::RateLimited, None);
        s.agent_source = "codex".into();
        // reset is 30 min out.
        s.rate_limit = Some(codex_state("secondary", 300, Utc::now() + Duration::minutes(30)));
        assert!(!should_auto_resume(&s, &cfg, Utc::now(), None));
    }

    #[test]
    fn codex_without_rate_limit_is_blocked() {
        // The real-world codex case today: no rate_limit payload (codex
        // rate-limit population lands in M2 P7), so a codex session is not an
        // auto-resume candidate — it fails the `Some(rl)` check, not a source
        // string check.
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let mut s = mk_session(SessionStatus::RateLimited, None);
        s.agent_source = "codex".into();
        assert!(!should_auto_resume(&s, &cfg, Utc::now(), None));
    }

    fn mk_eligible_with_id(id: &str) -> SessionInfo {
        let mut s = mk_session(SessionStatus::RateLimited, Some(mk_rl(-2, 5)));
        s.id = id.to_string();
        s
    }

    #[test]
    fn select_caps_to_available_slots() {
        // Five eligible sessions but only 2 concurrency slots free → fire 2.
        // This is the core defense against the 40GB runaway: a tick that finds
        // hundreds of eligible sessions must not spawn hundreds of processes.
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let sessions: Vec<SessionInfo> =
            (0..5).map(|i| mk_eligible_with_id(&format!("s{i}"))).collect();
        let picked = select_resume_candidates(&sessions, &cfg, Utc::now(), None, |_| false, 2);
        assert_eq!(picked.len(), 2, "must not exceed available slots");
        assert_eq!(picked[0].0, "s0");
        assert_eq!(picked[1].0, "s1");
    }

    /// A rate-limited session whose CLI is still alive must NOT be auto-resumed:
    /// firing `claude --resume` on a live session puts a second turn on the same
    /// transcript (the same corruption the watch gate prevents). The
    /// server-error retry path already skips `proc_alive` sessions; the
    /// rate-limit path must too. Guard lives inside `select_resume_candidates`
    /// so both the local and remote schedulers inherit it.
    #[test]
    fn select_skips_live_sessions() {
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let mut live = mk_eligible_with_id("alive");
        live.proc_alive = true;
        let dead = mk_eligible_with_id("dead");
        let picked = select_resume_candidates(
            &[live, dead],
            &cfg,
            Utc::now(),
            None,
            |_| false,
            5,
        );
        let ids: Vec<&str> = picked.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["dead"], "a still-live rate-limited session must be skipped");
    }

    /// A session whose workspace directory is gone can never be resumed —
    /// `spawn_claude_detached` bails with "Workspace directory not found" before
    /// any process exists. Firing it anyway costs a concurrency slot and a
    /// stderr log every time the failure backoff expires, and the user sees
    /// none of it (issue #105: the resumes that kept going out with a shredded
    /// cwd). Skip it at selection instead.
    #[test]
    fn select_skips_sessions_whose_workspace_is_gone() {
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let mut gone = mk_eligible_with_id("gone");
        gone.workspace_path = std::env::temp_dir()
            .join(format!("fleet-no-such-ws-{}", std::process::id()))
            .to_string_lossy()
            .into_owned();
        let here = mk_eligible_with_id("here");
        let picked = select_resume_candidates(&[gone, here], &cfg, Utc::now(), None, |_| false, 5);
        let ids: Vec<&str> = picked.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["here"], "a vanished workspace can never resume");
    }

    #[test]
    fn select_zero_slots_fires_nothing() {
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let sessions = vec![mk_eligible_with_id("s0")];
        let picked = select_resume_candidates(&sessions, &cfg, Utc::now(), None, |_| false, 0);
        assert!(picked.is_empty(), "no slots → fire nothing");
    }

    #[test]
    fn select_skips_predicate_matches() {
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let sessions: Vec<SessionInfo> =
            (0..3).map(|i| mk_eligible_with_id(&format!("s{i}"))).collect();
        // Skip s1 (e.g. debounced / backed off) → only s0, s2 within 5 slots.
        let picked =
            select_resume_candidates(&sessions, &cfg, Utc::now(), None, |id| id == "s1", 5);
        let ids: Vec<&str> = picked.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["s0", "s2"]);
    }

    #[test]
    fn legacy_config_without_new_fields_parses_and_preserves_enabled() {
        // A pre-existing auto_resume.json written before the server-error fields
        // existed must still deserialize (container `#[serde(default)]`) — and
        // must NOT silently flip `enabled`. The new fields take their defaults.
        let legacy = r#"{"enabled": false, "maxWaitHours": 6}"#;
        let cfg: AutoResumeConfig = serde_json::from_str(legacy).expect("legacy config parses");
        assert!(!cfg.enabled, "enabled must be preserved, not reset to default");
        assert_eq!(cfg.max_wait_hours, 6);
        assert!(cfg.retry_server_errors, "new flag defaults on");
        assert_eq!(cfg.max_server_error_retries, 3);
    }

    // ── server-error retry tests ───────────────────────────────────────────

    fn mk_server_errored() -> SessionInfo {
        mk_session(SessionStatus::ServerErrored, None)
    }

    #[test]
    fn server_error_retry_eligible_when_headless_and_errored() {
        let cfg = AutoResumeConfig::default(); // enabled + retry_server_errors
        assert!(should_retry_server_error(&mk_server_errored(), &cfg));
    }

    #[test]
    fn server_error_retry_blocked_when_feature_disabled() {
        let mut cfg = AutoResumeConfig::default();
        cfg.enabled = false;
        assert!(!should_retry_server_error(&mk_server_errored(), &cfg));
    }

    #[test]
    fn server_error_retry_blocked_when_flag_off() {
        // enabled (rate-limit resume on) but the server-error opt-out is set.
        let mut cfg = AutoResumeConfig::default();
        cfg.retry_server_errors = false;
        assert!(!should_retry_server_error(&mk_server_errored(), &cfg));
    }

    #[test]
    fn server_error_retry_blocked_when_subagent() {
        let cfg = AutoResumeConfig::default();
        let mut s = mk_server_errored();
        s.is_subagent = true;
        assert!(!should_retry_server_error(&s, &cfg));
    }

    #[test]
    fn server_error_retry_blocked_when_ide_attached() {
        let cfg = AutoResumeConfig::default();
        let mut s = mk_server_errored();
        s.ide_name = Some("Visual Studio Code".into());
        assert!(!should_retry_server_error(&s, &cfg));
    }

    #[test]
    fn server_error_retry_blocked_when_status_not_errored() {
        let cfg = AutoResumeConfig::default();
        // A RateLimited session is owned by the resets_at path, not this one.
        let s = mk_session(SessionStatus::RateLimited, Some(mk_rl(-2, 5)));
        assert!(!should_retry_server_error(&s, &cfg));
        assert!(!should_retry_server_error(&mk_session(SessionStatus::Idle, None), &cfg));
    }

    #[test]
    fn select_server_error_caps_to_slots() {
        let cfg = AutoResumeConfig::default();
        let sessions: Vec<SessionInfo> = (0..5)
            .map(|i| {
                let mut s = mk_server_errored();
                s.id = format!("s{i}");
                s
            })
            .collect();
        let picked = select_server_error_retries(&sessions, &cfg, |_| false, 2);
        assert_eq!(picked.len(), 2, "must not exceed available slots");
        assert_eq!(picked[0].0, "s0");
        assert_eq!(picked[1].0, "s1");
    }

    #[test]
    fn select_server_error_skips_predicate() {
        let cfg = AutoResumeConfig::default();
        let sessions: Vec<SessionInfo> = (0..3)
            .map(|i| {
                let mut s = mk_server_errored();
                s.id = format!("s{i}");
                s
            })
            .collect();
        let picked = select_server_error_retries(&sessions, &cfg, |id| id == "s1", 5);
        let ids: Vec<&str> = picked.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["s0", "s2"]);
    }

    #[test]
    fn backoff_after_threshold_consecutive_failures() {
        use std::collections::HashMap;
        let mut failures: HashMap<String, u32> = HashMap::new();
        let threshold = 3;
        // First two failures: not yet backed off.
        record_resume_outcome(&mut failures, "s0", false);
        assert!(!is_backed_off(&failures, "s0", threshold));
        record_resume_outcome(&mut failures, "s0", false);
        assert!(!is_backed_off(&failures, "s0", threshold));
        // Third failure: now backed off (stops the endless re-fire loop).
        record_resume_outcome(&mut failures, "s0", false);
        assert!(is_backed_off(&failures, "s0", threshold));
    }

    #[test]
    fn backoff_cleared_by_success() {
        use std::collections::HashMap;
        let mut failures: HashMap<String, u32> = HashMap::new();
        record_resume_outcome(&mut failures, "s0", false);
        record_resume_outcome(&mut failures, "s0", false);
        record_resume_outcome(&mut failures, "s0", false);
        assert!(is_backed_off(&failures, "s0", 3));
        // A successful resume resets the counter so it can fire again later.
        record_resume_outcome(&mut failures, "s0", true);
        assert!(!is_backed_off(&failures, "s0", 3));
    }

    #[test]
    fn blocked_when_subagent() {
        // Subagent transcripts (`agent-*.jsonl` under `<sid>/subagents/`) are
        // not independently resumable — `claude --resume agent-X` always fails
        // (observed 21303/21303 success=false in the field). They must never be
        // auto-resume candidates regardless of rate-limit state, or the failed
        // resume leaves them RateLimited and they re-fire forever with no cap.
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let mut s = mk_session(SessionStatus::RateLimited, Some(mk_rl(-2, 5)));
        s.is_subagent = true;
        assert!(!should_auto_resume(&s, &cfg, Utc::now(), None));
    }

    #[test]
    fn blocked_when_status_not_rate_limited() {
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let s = mk_session(SessionStatus::Idle, Some(mk_rl(-1, 5)));
        assert!(!should_auto_resume(&s, &cfg, Utc::now(), None));
    }

    #[test]
    fn blocked_when_no_rate_limit_payload() {
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let s = mk_session(SessionStatus::RateLimited, None);
        assert!(!should_auto_resume(&s, &cfg, Utc::now(), None));
    }

    #[test]
    fn blocked_within_grace_period_after_reset() {
        // The 21:10 / 21:12 incident: reset boundary just passed but the
        // server can still hand back 429 for ~tens of seconds. Wait at least
        // 60s past reset before firing so we don't burn a spawn on a bouncing
        // boundary.
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        // reset was 30s ago — still inside the 60s grace window
        let resets_at = Utc::now() - Duration::seconds(30);
        let error_timestamp = resets_at - Duration::hours(5);
        let rl = RateLimitState {
            resets_at,
            limit_type: RateLimitType::SessionLimit,
            parsed: true,
            error_timestamp,
        };
        let s = mk_session(SessionStatus::RateLimited, Some(rl));
        assert!(!should_auto_resume(&s, &cfg, Utc::now(), None));
    }

    #[cfg(unix)]
    #[test]
    fn spawn_resume_with_path_records_stderr_and_reaps_child() {
        // Repro for the silent-failure bug: when the spawned process exits
        // non-zero, we must (a) record stderr to the log, (b) record the exit
        // code, and (c) reap the child so it does not become a zombie.
        //
        // We use /bin/sh as a stand-in "claude" — passing `--resume <id> -p continue`
        // makes sh complain to stderr and exit non-zero, which is exactly the
        // shape of the real failure mode. The fixture is unix-only; the
        // spawn/reap logic itself is platform-agnostic.
        let tmp = std::env::temp_dir().join(format!(
            "fleet_test_spawn_resume_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("create tmp dir");
        let log = tmp.join("stderr.log");

        let exit_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let exit_flag_cb = exit_flag.clone();
        let pid = super::spawn_resume_with_path(
            "/bin/sh",
            "test-session-id",
            &tmp.to_string_lossy(),
            "continue",
            &[],
            &log,
            move |_success| {
                exit_flag_cb.store(true, std::sync::atomic::Ordering::SeqCst);
            },
        )
        .expect("spawn should succeed");
        assert!(pid > 0, "pid should be non-zero");

        // Wait up to 3s for the reaper thread to record the exit.
        let mut content = String::new();
        let mut waited = std::time::Duration::ZERO;
        let max = std::time::Duration::from_secs(3);
        let step = std::time::Duration::from_millis(100);
        while waited < max {
            std::thread::sleep(step);
            waited += step;
            content = std::fs::read_to_string(&log).unwrap_or_default();
            if content.contains("auto_resume exit") {
                break;
            }
        }

        assert!(
            content.contains("auto_resume spawn"),
            "header line missing; log contents: {:?}",
            content
        );
        assert!(
            content.contains("auto_resume exit"),
            "exit line missing — reaper thread did not run; log contents: {:?}",
            content
        );
        assert!(
            content.contains("success=false"),
            "expected non-zero exit recorded; log contents: {:?}",
            content
        );

        #[cfg(unix)]
        {
            let alive = std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            assert!(!alive, "child pid {} should have been reaped", pid);
        }

        // The reaper must have invoked on_exit so the scheduler frees the slot.
        assert!(
            exit_flag.load(std::sync::atomic::Ordering::SeqCst),
            "on_exit callback was not invoked by the reaper",
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── M2 P8: live end-to-end codex resume validation ──────────────────────
    //
    // These spawn a real `codex` session + model, so they are ignored by
    // default. Run manually:
    //   cargo test -p claw-fleet-core auto_resume::tests::live_codex -- --ignored

    /// Manual resume of a codex session through the real dispatcher
    /// (`agent_source::resume_session("codex", …)`) continues the thread and the
    /// resumed process exits 0. Validates the whole manual-resume path a user
    /// hits from the resume composer / rate-limit button.
    #[test]
    #[ignore = "spawns + resumes a real codex session; run manually with --ignored"]
    fn live_codex_manual_resume_via_dispatcher_continues() {
        let ws = std::env::temp_dir().join(format!("fleet-codex-p8-manual-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        let resp = crate::codex_launch::spawn_new_codex_session(
            ws.to_str().unwrap(),
            "reply with exactly: FIRST",
            None,
            None,
            &[],
        )
        .expect("initial codex spawn");
        let tid = resp.session_id.expect("thread id");

        let ok = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ok_cb = ok.clone();
        let done_cb = done.clone();
        crate::agent_source::resume_session(
            "codex",
            &crate::agent_source::ResumeSpec {
                session_id: tid.clone(),
                workspace_path: ws.to_string_lossy().into_owned(),
                prompt: "reply with exactly: SECOND".to_string(),
                ..Default::default()
            },
            Box::new(move |success| {
                ok_cb.store(success, std::sync::atomic::Ordering::SeqCst);
                done_cb.store(true, std::sync::atomic::Ordering::SeqCst);
            }),
        )
        .expect("dispatcher should route codex resume");

        let mut waited = std::time::Duration::ZERO;
        while waited < std::time::Duration::from_secs(90)
            && !done.load(std::sync::atomic::Ordering::SeqCst)
        {
            std::thread::sleep(std::time::Duration::from_millis(500));
            waited += std::time::Duration::from_millis(500);
        }
        assert!(done.load(std::sync::atomic::Ordering::SeqCst), "resume never exited");
        assert!(ok.load(std::sync::atomic::Ordering::SeqCst), "resume exited non-zero");
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// Auto-resume end-to-end with a *constructed* codex rate limit (a live limit
    /// can't be triggered at low usage). Spawns a real codex thread, marks a
    /// SessionInfo RateLimited with a codex secondary-window state whose reset has
    /// passed, confirms `select_resume_candidates` picks it, then fires the resume
    /// via the same source-routed dispatcher the scheduler uses and confirms it
    /// continues (exit 0). The only piece NOT exercised here is the scanner
    /// *populating* that rate_limit onto a live rate-limited codex session, which
    /// depends on the Fleet-owned marking (M3 P9).
    #[test]
    #[ignore = "spawns + resumes a real codex session; run manually with --ignored"]
    fn live_codex_auto_resume_selects_and_fires() {
        let ws = std::env::temp_dir().join(format!("fleet-codex-p8-auto-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        let resp = crate::codex_launch::spawn_new_codex_session(
            ws.to_str().unwrap(),
            "reply with exactly: FIRST",
            None,
            None,
            &[],
        )
        .expect("initial codex spawn");
        let tid = resp.session_id.expect("thread id");

        // Construct the rate-limited codex session the scanner will build in P9.
        let mut s = mk_session(SessionStatus::RateLimited, None);
        s.id = tid.clone();
        s.workspace_path = ws.to_string_lossy().into_owned();
        s.agent_source = "codex".into();
        s.rate_limit = Some(codex_state("secondary", 300, Utc::now() - Duration::minutes(2)));

        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12, ..Default::default() };
        let picked =
            select_resume_candidates(std::slice::from_ref(&s), &cfg, Utc::now(), None, |_| false, 4);
        assert_eq!(picked.len(), 1, "constructed codex rate-limit must be selected");
        assert_eq!(picked[0].0, tid, "selected the codex session");

        // Fire the resume exactly as the scheduler does — routed by source.
        let ok = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ok_cb = ok.clone();
        let done_cb = done.clone();
        crate::agent_source::resume_session(
            &s.agent_source,
            &crate::agent_source::ResumeSpec {
                session_id: picked[0].0.clone(),
                workspace_path: picked[0].1.clone(),
                prompt: "continue".to_string(),
                ..Default::default()
            },
            Box::new(move |success| {
                ok_cb.store(success, std::sync::atomic::Ordering::SeqCst);
                done_cb.store(true, std::sync::atomic::Ordering::SeqCst);
            }),
        )
        .expect("scheduler-shaped codex resume should fire");

        let mut waited = std::time::Duration::ZERO;
        while waited < std::time::Duration::from_secs(90)
            && !done.load(std::sync::atomic::Ordering::SeqCst)
        {
            std::thread::sleep(std::time::Duration::from_millis(500));
            waited += std::time::Duration::from_millis(500);
        }
        assert!(done.load(std::sync::atomic::Ordering::SeqCst), "auto-resume never exited");
        assert!(ok.load(std::sync::atomic::Ordering::SeqCst), "auto-resume exited non-zero");
        let _ = std::fs::remove_dir_all(&ws);
    }
}
