//! Launch a brand-new headless Claude Code session in a workspace.
//!
//! This hosts the generic "spawn a detached `claude` process with stderr
//! logging + a reaper thread" machinery. `auto_resume::spawn_resume` delegates
//! here for its `claude --resume <id> -p continue` shape; the sessions page's
//! "new session" button uses [`spawn_new_session`] for the
//! `claude -p "<initial prompt>"` shape. The spawned session's own JSONL is
//! picked up by the scanner, so the new session appears in the session list
//! without any explicit registration.

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Request body for the remote `/spawn_session` endpoint.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SpawnSessionRequest {
    pub workspace_path: String,
    pub prompt: String,
}

/// Response body for the remote `/spawn_session` endpoint.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SpawnSessionResponse {
    pub pid: u32,
}

/// Spawn `claude <args>` detached in `workspace_path`, with stderr redirected
/// to `stderr_log` and a background thread that reaps the child and records
/// its exit status — so failures are not silent and we don't accumulate
/// zombies.
///
/// `label` tags the log lines (e.g. `auto_resume`, `new_session`); `detail`
/// carries per-spawn context (e.g. `session=<id>`). `on_exit(success)` fires
/// from the reaper thread when the child exits.
///
/// Returns the child pid.
pub fn spawn_claude_detached(
    claude_path: &str,
    args: &[String],
    workspace_path: &str,
    stderr_log: &Path,
    label: &str,
    detail: &str,
    on_exit: impl FnOnce(bool) + Send + 'static,
) -> Result<u32, String> {
    if !Path::new(workspace_path).is_dir() {
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
            "[{}] {} spawn {} cwd={}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            label,
            detail,
            workspace_path
        );
    }

    let stderr_file = std::fs::OpenOptions::new()
        .append(true)
        .open(stderr_log)
        .map_err(|e| format!("reopen stderr log {}: {}", stderr_log.display(), e))?;

    let mut child = crate::process_util::command(claude_path)
        .args(args)
        .current_dir(workspace_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(stderr_file))
        .spawn()
        .map_err(|e| format!("spawn claude failed: {e}"))?;
    let pid = child.id();

    let label_owned = label.to_string();
    let detail_owned = detail.to_string();
    let log_path_owned = stderr_log.to_path_buf();
    std::thread::spawn(move || {
        let result = child.wait();
        let success = matches!(&result, Ok(status) if status.success());
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path_owned)
        {
            match result {
                Ok(status) => {
                    let _ = writeln!(
                        f,
                        "[{}] {} exit {} code={:?} success={}",
                        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                        label_owned,
                        detail_owned,
                        status.code(),
                        status.success(),
                    );
                }
                Err(e) => {
                    let _ = writeln!(
                        f,
                        "[{}] {} wait_err {} err={}",
                        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                        label_owned,
                        detail_owned,
                        e
                    );
                }
            }
        }
        // Notify the caller the child exited (and whether it succeeded).
        on_exit(success);
    });

    Ok(pid)
}

/// Start a brand-new headless Claude Code session: spawns
/// `claude -p "<prompt>"` detached in `workspace_path`. Returns as soon as
/// the child is spawned; the session's JSONL will be created by the claude
/// process itself and discovered by the scanner.
pub fn spawn_new_session(workspace_path: &str, prompt: &str) -> Result<u32, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("prompt is required".to_string());
    }
    let (found, claude_path) = crate::check_cli_installed();
    if !found {
        return Err("Claude CLI not found on PATH".to_string());
    }
    let claude = claude_path.unwrap_or_else(|| "claude".to_string());
    let stderr_log = crate::session::get_fleet_dir()
        .map(|d| d.join("new_session_stderr.log"))
        .ok_or_else(|| "no fleet dir".to_string())?;
    crate::log_debug(&format!(
        "new_session: claude -p <prompt {} chars> (cwd={}, stderr_log={})",
        prompt.len(),
        workspace_path,
        stderr_log.display()
    ));
    let pid = spawn_claude_detached(
        &claude,
        &["-p".to_string(), prompt.to_string()],
        workspace_path,
        &stderr_log,
        "new_session",
        "",
        |_success| {},
    )?;
    crate::log_debug(&format!(
        "new_session: spawned pid {} in {}",
        pid, workspace_path
    ));
    Ok(pid)
}

#[cfg(test)]
mod tests {
    #[test]
    fn spawn_new_session_rejects_empty_prompt() {
        // Prompt validation must fire before any CLI/filesystem checks so the
        // frontend gets a stable error regardless of the host environment.
        let err = super::spawn_new_session("/", "   ").unwrap_err();
        assert_eq!(err, "prompt is required");
    }

    #[cfg(unix)]
    #[test]
    fn spawn_claude_detached_rejects_missing_workspace() {
        let log = std::env::temp_dir().join(format!(
            "fleet_test_spawn_detached_{}.log",
            std::process::id()
        ));
        let err = super::spawn_claude_detached(
            "/bin/sh",
            &["-c".to_string(), "true".to_string()],
            "/nonexistent/fleet/workspace/dir",
            &log,
            "test",
            "",
            |_| {},
        )
        .unwrap_err();
        assert!(
            err.contains("Workspace directory not found"),
            "unexpected error: {err}"
        );
    }
}
