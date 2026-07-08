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
#[serde(rename_all = "camelCase")]
pub struct AutoResumeConfig {
    pub enabled: bool,
    /// Don't auto-resume if the configured wait window is longer than this.
    /// Manual "Resume now" always works; this only gates the scheduler.
    pub max_wait_hours: u64,
}

impl Default for AutoResumeConfig {
    fn default() -> Self {
        Self { enabled: true, max_wait_hours: 12 }
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

/// Decide whether a rate-limited session is eligible for auto-resume *right now*.
///
/// Returns `true` only when ALL of:
/// - `config.enabled`
/// - the session is NOT a subagent (`agent-*` transcripts can't be resumed)
/// - the session is NOT attached to an interactive IDE (`ide_name == None`);
///   IDE (VS Code / Claude app / Cursor) sessions must not be resumed headlessly
/// - the session's `agent_source == "claude-code"` (only claude-code transcripts
///   are loadable by `claude --resume`)
/// - the session is in `RateLimited` state with a `rate_limit` payload
/// - `now >= resets_at + RESET_GRACE` (the wait has elapsed plus a grace
///   window so we don't race the reset boundary)
/// - `resets_at - error_timestamp <= max_wait_hours` (the configured window
///   was short enough that unattended resume is safe)
pub fn should_auto_resume(
    session: &crate::session::SessionInfo,
    config: &AutoResumeConfig,
    now: chrono::DateTime<chrono::Utc>,
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
    // inside an interactive IDE (VS Code / Claude app / Cursor) writes an ide
    // lock file, so `ide_name` is Some — firing `claude --resume <id> -p continue`
    // as a detached headless process behind the user's open editor is not what
    // they want. Headless Fleet-spawned `claude --print` sessions have no lock
    // (`ide_name == None`) and are the only ones we resume.
    if session.ide_name.is_some() {
        return false;
    }
    // `claude --resume` can only reload a claude-code transcript. Cursor / Codex
    // sessions carry a different `agent_source` and would just fail the resume.
    if session.agent_source != "claude-code" {
        return false;
    }
    if session.status != crate::session::SessionStatus::RateLimited {
        return false;
    }
    let Some(rl) = session.rate_limit.as_ref() else { return false };
    if now < rl.resets_at + RESET_GRACE {
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
pub fn select_resume_candidates(
    sessions: &[crate::session::SessionInfo],
    config: &AutoResumeConfig,
    now: chrono::DateTime<chrono::Utc>,
    skip: impl Fn(&str) -> bool,
    slots: usize,
) -> Vec<(String, String)> {
    sessions
        .iter()
        .filter(|s| should_auto_resume(s, config, now))
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

#[allow(clippy::too_many_arguments)]
fn spawn_resume_tracked_prompt(
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
    args.push("--output-format".to_string());
    args.push("stream-json".to_string());
    args.push("--verbose".to_string());
    args.push("--include-partial-messages".to_string());
    args.extend(override_args.iter().cloned());
    // Route native permission prompts to Fleet's Decision Panel instead of
    // headless auto-deny (no-op when the fleet MCP server isn't injected).
    args.extend(crate::session_launch::permission_prompt_tool_args());
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
            workspace_path: "/w".into(),
            workspace_name: "w".into(),
            ide_name: None,
            entrypoint: None,
            is_subagent: false,
            parent_session_id: None,
            agent_type: None,
            agent_description: None,
            slug: None,
            ai_title: None,
            status,
            token_speed: 0.0,
            agent_token_speed: 0.0,
            total_output_tokens: 0,
            total_cost_usd: 0.0,
            agent_total_cost_usd: 0.0,
            cost_speed_usd_per_min: 0.0,
            last_message_preview: None,
            last_activity_ms: 0,
            created_at_ms: 0,
            jsonl_path: "/w/s1.jsonl".into(),
            model: None,
            thinking_level: None,
            pid: None,
            pid_precise: false,
            last_skill: None,
            context_percent: None,
            agent_source: "claude-code".into(),
            last_outcome: None,
            rate_limit: rl,
            todos: None,
            task_plan: None, handoff: None,            compact_count: 0,
            compact_pre_tokens: 0,
            compact_post_tokens: 0,
            compact_cost_usd: 0.0,
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
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12 };
        // reset was 2 minutes ago — past the 60s grace — original wait was 5h
        let s = mk_session(
            SessionStatus::RateLimited,
            Some(mk_rl(-2, 5)),
        );
        assert!(should_auto_resume(&s, &cfg, Utc::now()));
    }

    #[test]
    fn blocked_when_still_waiting() {
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12 };
        // reset is 10 minutes away
        let s = mk_session(SessionStatus::RateLimited, Some(mk_rl(10, 5)));
        assert!(!should_auto_resume(&s, &cfg, Utc::now()));
    }

    #[test]
    fn blocked_when_wait_exceeds_max() {
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12 };
        // reset was 1 min ago, but the original wait was 24h (weekly limit) — skip.
        let s = mk_session(SessionStatus::RateLimited, Some(mk_rl(-1, 24)));
        assert!(!should_auto_resume(&s, &cfg, Utc::now()));
    }

    #[test]
    fn blocked_when_disabled() {
        let cfg = AutoResumeConfig { enabled: false, max_wait_hours: 12 };
        let s = mk_session(SessionStatus::RateLimited, Some(mk_rl(-1, 5)));
        assert!(!should_auto_resume(&s, &cfg, Utc::now()));
    }

    #[test]
    fn blocked_when_ide_attached() {
        // A session running inside an interactive IDE (VS Code / Claude app /
        // Cursor) writes an ide lock, so `ide_name` is Some. Headlessly firing
        // `claude --resume <id> -p continue` behind the user's editor is exactly
        // what we must not do — auto-resume is only for Fleet-spawned headless
        // sessions (`ide_name == None`).
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12 };
        let mut s = mk_session(SessionStatus::RateLimited, Some(mk_rl(-2, 5)));
        s.ide_name = Some("Visual Studio Code".into());
        assert!(!should_auto_resume(&s, &cfg, Utc::now()));
    }

    #[test]
    fn blocked_when_not_claude_code_source() {
        // `claude --resume` can only reload a claude-code transcript. A Cursor
        // (or Codex) session must never be a candidate.
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12 };
        let mut s = mk_session(SessionStatus::RateLimited, Some(mk_rl(-2, 5)));
        s.agent_source = "cursor".into();
        assert!(!should_auto_resume(&s, &cfg, Utc::now()));
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
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12 };
        let sessions: Vec<SessionInfo> =
            (0..5).map(|i| mk_eligible_with_id(&format!("s{i}"))).collect();
        let picked = select_resume_candidates(&sessions, &cfg, Utc::now(), |_| false, 2);
        assert_eq!(picked.len(), 2, "must not exceed available slots");
        assert_eq!(picked[0].0, "s0");
        assert_eq!(picked[1].0, "s1");
    }

    #[test]
    fn select_zero_slots_fires_nothing() {
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12 };
        let sessions = vec![mk_eligible_with_id("s0")];
        let picked = select_resume_candidates(&sessions, &cfg, Utc::now(), |_| false, 0);
        assert!(picked.is_empty(), "no slots → fire nothing");
    }

    #[test]
    fn select_skips_predicate_matches() {
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12 };
        let sessions: Vec<SessionInfo> =
            (0..3).map(|i| mk_eligible_with_id(&format!("s{i}"))).collect();
        // Skip s1 (e.g. debounced / backed off) → only s0, s2 within 5 slots.
        let picked =
            select_resume_candidates(&sessions, &cfg, Utc::now(), |id| id == "s1", 5);
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
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12 };
        let mut s = mk_session(SessionStatus::RateLimited, Some(mk_rl(-2, 5)));
        s.is_subagent = true;
        assert!(!should_auto_resume(&s, &cfg, Utc::now()));
    }

    #[test]
    fn blocked_when_status_not_rate_limited() {
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12 };
        let s = mk_session(SessionStatus::Idle, Some(mk_rl(-1, 5)));
        assert!(!should_auto_resume(&s, &cfg, Utc::now()));
    }

    #[test]
    fn blocked_when_no_rate_limit_payload() {
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12 };
        let s = mk_session(SessionStatus::RateLimited, None);
        assert!(!should_auto_resume(&s, &cfg, Utc::now()));
    }

    #[test]
    fn blocked_within_grace_period_after_reset() {
        // The 21:10 / 21:12 incident: reset boundary just passed but the
        // server can still hand back 429 for ~tens of seconds. Wait at least
        // 60s past reset before firing so we don't burn a spawn on a bouncing
        // boundary.
        let cfg = AutoResumeConfig { enabled: true, max_wait_hours: 12 };
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
        assert!(!should_auto_resume(&s, &cfg, Utc::now()));
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
}
