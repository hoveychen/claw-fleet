//! Auto-resume configuration + policy for rate-limited sessions.
//!
//! When `enabled`, the scheduler will spawn `claude --resume <id> -p continue`
//! as soon as a rate-limited session's `resets_at` timestamp has passed.
//! `max_wait_hours` is a safety guard: if the total wait (error → reset) is
//! longer than this, the session is left alone so the user is not surprised
//! by a job that resumes 24h later.

use std::io::Write;
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

/// Headlessly resume a rate-limited session by spawning
/// `claude --resume <session_id> -p "continue"` detached in the given workspace.
pub fn spawn_resume(session_id: &str, workspace_path: &str) -> Result<(), String> {
    let (found, claude_path) = crate::check_cli_installed();
    if !found {
        return Err("Claude CLI not found on PATH".to_string());
    }
    let claude = claude_path.unwrap_or_else(|| "claude".to_string());
    let stderr_log = crate::session::get_fleet_dir()
        .map(|d| d.join("auto_resume_stderr.log"))
        .ok_or_else(|| "no fleet dir".to_string())?;
    crate::log_debug(&format!(
        "resume_session: claude --resume {} -p 'continue' (cwd={}, stderr_log={})",
        session_id,
        workspace_path,
        stderr_log.display()
    ));
    let pid = spawn_resume_with_path(&claude, session_id, workspace_path, &stderr_log)?;
    crate::log_debug(&format!(
        "resume_session: spawned pid {} for session {}",
        pid, session_id
    ));
    Ok(())
}

/// Spawn `claude --resume <id> -p continue` with stderr redirected to
/// `stderr_log` and a background thread that reaps the child and records its
/// exit status — so failures stop being silent and we don't accumulate zombies.
fn spawn_resume_with_path(
    claude_path: &str,
    session_id: &str,
    workspace_path: &str,
    stderr_log: &Path,
) -> Result<u32, String> {
    if !std::path::Path::new(workspace_path).is_dir() {
        return Err(format!("Workspace directory not found: {}", workspace_path));
    }
    if let Some(parent) = stderr_log.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create stderr log dir {}: {}", parent.display(), e))?;
    }

    {
        let mut header = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(stderr_log)
            .map_err(|e| format!("open stderr log {}: {}", stderr_log.display(), e))?;
        let _ = writeln!(
            header,
            "[{}] auto_resume spawn session={} cwd={}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            session_id,
            workspace_path
        );
    }

    let stderr_file = std::fs::OpenOptions::new()
        .append(true)
        .open(stderr_log)
        .map_err(|e| format!("reopen stderr log {}: {}", stderr_log.display(), e))?;

    let mut child = std::process::Command::new(claude_path)
        .arg("--resume")
        .arg(session_id)
        .arg("-p")
        .arg("continue")
        .current_dir(workspace_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(stderr_file))
        .spawn()
        .map_err(|e| format!("spawn claude --resume failed: {e}"))?;
    let pid = child.id();

    let session_id_owned = session_id.to_string();
    let log_path_owned = stderr_log.to_path_buf();
    std::thread::spawn(move || {
        let result = child.wait();
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path_owned)
        {
            match result {
                Ok(status) => {
                    let _ = writeln!(
                        f,
                        "[{}] auto_resume exit session={} code={:?} success={}",
                        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                        session_id_owned,
                        status.code(),
                        status.success(),
                    );
                }
                Err(e) => {
                    let _ = writeln!(
                        f,
                        "[{}] auto_resume wait_err session={} err={}",
                        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                        session_id_owned,
                        e
                    );
                }
            }
        }
    });

    Ok(pid)
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

    #[test]
    fn spawn_resume_with_path_records_stderr_and_reaps_child() {
        // Repro for the silent-failure bug: when the spawned process exits
        // non-zero, we must (a) record stderr to the log, (b) record the exit
        // code, and (c) reap the child so it does not become a zombie.
        //
        // We use /bin/sh as a stand-in "claude" — passing `--resume <id> -p continue`
        // makes sh complain to stderr and exit non-zero, which is exactly the
        // shape of the real failure mode.
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

        let pid = super::spawn_resume_with_path(
            "/bin/sh",
            "test-session-id",
            &tmp.to_string_lossy(),
            &log,
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

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
