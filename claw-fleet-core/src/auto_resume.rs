//! Auto-resume configuration + policy for rate-limited sessions.
//!
//! When `enabled`, the scheduler will spawn `claude --resume <id> -p continue`
//! as soon as a rate-limited session's `resets_at` timestamp has passed.
//! `max_wait_hours` is a safety guard: if the total wait (error → reset) is
//! longer than this, the session is left alone so the user is not surprised
//! by a job that resumes 24h later.

use std::path::PathBuf;

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

/// Decide whether a rate-limited session is eligible for auto-resume *right now*.
///
/// Returns `true` only when ALL of:
/// - `config.enabled`
/// - the session is in `RateLimited` state with a `rate_limit` payload
/// - `now >= resets_at` (the wait has elapsed)
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
    if session.status != crate::session::SessionStatus::RateLimited {
        return false;
    }
    let Some(rl) = session.rate_limit.as_ref() else { return false };
    if now < rl.resets_at {
        return false;
    }
    let wait = rl.resets_at - rl.error_timestamp;
    let max_wait = chrono::Duration::hours(config.max_wait_hours as i64);
    wait <= max_wait
}

/// Headlessly resume a rate-limited session by spawning
/// `claude --resume <session_id> -p "continue"` detached in the given workspace.
pub fn spawn_resume(session_id: &str, workspace_path: &str) -> Result<(), String> {
    let (found, claude_path) = crate::check_cli_installed();
    if !found {
        return Err("Claude CLI not found on PATH".to_string());
    }
    let claude = claude_path.unwrap_or_else(|| "claude".to_string());
    if !std::path::Path::new(workspace_path).is_dir() {
        return Err(format!("Workspace directory not found: {}", workspace_path));
    }
    crate::log_debug(&format!(
        "resume_session: claude --resume {} -p 'continue' (cwd={})",
        session_id, workspace_path
    ));
    let child = std::process::Command::new(&claude)
        .arg("--resume")
        .arg(session_id)
        .arg("-p")
        .arg("continue")
        .current_dir(workspace_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn claude --resume failed: {e}"))?;
    crate::log_debug(&format!(
        "resume_session: spawned pid {} for session {}",
        child.id(),
        session_id
    ));
    std::mem::drop(child);
    Ok(())
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
            is_subagent: false,
            parent_session_id: None,
            agent_type: None,
            agent_description: None,
            slug: None,
            ai_title: None,
            status,
            token_speed: 0.0,
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
        // reset was 1 minute ago, original wait was 5h
        let s = mk_session(
            SessionStatus::RateLimited,
            Some(mk_rl(-1, 5)),
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
}
