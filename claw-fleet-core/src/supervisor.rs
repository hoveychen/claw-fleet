//! Supervisor — manages fleet-managed Claude sessions.
//!
//! Responsibilities:
//!   - enqueue: append a queued FleetSession entry to fleet-sessions.json
//!   - tick: periodic loop that spawns queued sessions up to project.concurrency,
//!     reaps finished pids, transitions state.
//!   - cancel: SIGTERM a running session and mark it complete.
//!   - resume: re-enqueue a completed session with a follow-up prompt
//!     (claude --resume <sid> -p <prompt>).
//!
//! State machine (column ids match `project::DEFAULT_COLUMN_*`):
//!   queued (enqueued, awaiting slot)
//!   → running (claude child spawned, pid alive)
//!   ↔ pending (transcript scanner sees unanswered AskUserQuestion / guard /
//!     elicitation — set externally by `claw_fleet_desktop::detect_waiting_transitions`)
//!   → complete (process exited, cancelled, or agent self-reported via fleet-cli)
//!
//! Concurrency: per-project slot count from `Project.concurrency`. `expedited`
//! sessions bypass the slot check (Boss's "插队" toggle).
//!
//! Spawn is macOS-only in v1 per the architecture decision; on other platforms
//! `tick` is a no-op and `enqueue` still writes the queued record (so
//! Linux/Windows users see queued items but never get spawned).

use std::process::{Command, Stdio};

use crate::project::{
    self, FleetSession, LauncherForm, DEFAULT_COLUMN_COMPLETE, DEFAULT_COLUMN_QUEUED,
    DEFAULT_COLUMN_RUNNING,
};

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Append a new fleet session (status = `queued`) to fleet-sessions.json.
/// The next `tick()` picks it up.
///
/// If `form.fleetsession_dir` is `Some`, also writes a `<sid>.fleetsession`
/// JSON file to that directory so the file browser can render the session
/// inline with the rest of the workspace files (Boss's "虚拟 session 文件"
/// landing on the user's current browse dir).
pub fn enqueue(form: LauncherForm) -> Result<FleetSession, String> {
    if form.prompt.trim().is_empty() {
        return Err("prompt is required".into());
    }
    let projects = project::list_projects();
    let proj = projects
        .iter()
        .find(|p| p.id == form.project_id)
        .ok_or_else(|| format!("project not found: {}", form.project_id))?;

    let id = uuid::Uuid::new_v4().to_string();
    let fleetsession_path = if let Some(dir) = form.fleetsession_dir.as_ref() {
        let p = std::path::Path::new(dir).join(format!("{id}.fleetsession"));
        let body = serde_json::json!({
            "id": id,
            "projectId": proj.id,
            "createdAt": now_ms(),
        });
        if let Err(e) = std::fs::write(&p, serde_json::to_string_pretty(&body).unwrap_or_default()) {
            return Err(format!("write .fleetsession file: {e}"));
        }
        Some(p.to_string_lossy().to_string())
    } else {
        None
    };

    let mut sessions = project::list_fleet_sessions();
    let new = FleetSession {
        id,
        project_id: proj.id.clone(),
        workspace: proj.workspace.clone(),
        fleetsession_path,
        prompt: form.prompt,
        context_files: form.context_files,
        status: DEFAULT_COLUMN_QUEUED.into(),
        note: None,
        created_at: now_ms(),
        started_at: None,
        completed_at: None,
        pid: None,
        expedited: form.expedited,
    };
    sessions.push(new.clone());
    project::save_fleet_sessions(&sessions)?;
    Ok(new)
}

/// Cancel a fleet session by id. SIGTERMs the process if still running, then
/// marks the session `complete` with note="cancelled".
pub fn cancel(session_id: &str) -> Result<(), String> {
    let mut sessions = project::list_fleet_sessions();
    let idx = sessions
        .iter()
        .position(|s| s.id == session_id)
        .ok_or_else(|| format!("session not found: {}", session_id))?;
    if let Some(pid) = sessions[idx].pid {
        let _ = kill_pid(pid);
    }
    sessions[idx].status = DEFAULT_COLUMN_COMPLETE.into();
    sessions[idx].completed_at = Some(now_ms());
    sessions[idx].pid = None;
    sessions[idx].note = Some("cancelled".into());
    project::save_fleet_sessions(&sessions)
}

/// Re-queue a completed session with a follow-up prompt. Spawn will be
/// `claude --resume <sid> -p <prompt>` on the next tick.
pub fn resume(session_id: &str, follow_up_prompt: String) -> Result<(), String> {
    if follow_up_prompt.trim().is_empty() {
        return Err("follow-up prompt is required".into());
    }
    let mut sessions = project::list_fleet_sessions();
    let idx = sessions
        .iter()
        .position(|s| s.id == session_id)
        .ok_or_else(|| format!("session not found: {}", session_id))?;
    if sessions[idx].pid.is_some() {
        return Err("session is still running; cancel first".into());
    }
    sessions[idx].prompt = follow_up_prompt;
    sessions[idx].status = DEFAULT_COLUMN_QUEUED.into();
    sessions[idx].completed_at = None;
    sessions[idx].started_at = None;
    sessions[idx].note = None;
    project::save_fleet_sessions(&sessions)
}

/// Mutate a session's `status` (and optional `note`). Used by the
/// `fleet session status` self-report subcommand (P6) and by the kanban
/// drag-and-drop in P5.
pub fn set_status(session_id: &str, new_status: &str, note: Option<String>) -> Result<(), String> {
    let mut sessions = project::list_fleet_sessions();
    let idx = sessions
        .iter()
        .position(|s| s.id == session_id)
        .ok_or_else(|| format!("session not found: {}", session_id))?;

    // Validate that the status either matches a kanban column on the owning
    // project, or fall back to free-text `note` if it doesn't.
    let projects = project::list_projects();
    let proj = projects.iter().find(|p| p.id == sessions[idx].project_id);
    let valid_column = proj
        .map(|p| p.kanban_columns.iter().any(|c| c.id == new_status))
        .unwrap_or(false);

    if valid_column {
        sessions[idx].status = new_status.into();
        if new_status == DEFAULT_COLUMN_COMPLETE {
            sessions[idx].completed_at = Some(now_ms());
        }
    } else {
        // Unknown status string from agent — record as note, do not move column.
        sessions[idx].note = Some(new_status.into());
    }
    if let Some(n) = note {
        sessions[idx].note = Some(n);
    }
    project::save_fleet_sessions(&sessions)
}

// ── Background tick ──────────────────────────────────────────────────────────

/// One tick of the supervisor loop. Should be called every ~1s by whoever
/// hosts the loop (fleet serve in v1).
///
/// On non-macOS this is a no-op for v1 (D module is macOS-only per the
/// architecture decision).
pub fn tick() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        tick_macos()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn tick_macos() -> Result<(), String> {
    let projects = project::list_projects();
    let mut sessions = project::list_fleet_sessions();
    let mut changed = false;

    // Pass 1: reap finished pids → mark complete.
    for s in sessions.iter_mut() {
        if let Some(pid) = s.pid {
            if !pid_alive(pid) {
                s.pid = None;
                if s.status == DEFAULT_COLUMN_RUNNING || s.status == "pending" {
                    s.status = DEFAULT_COLUMN_COMPLETE.into();
                    s.completed_at = Some(now_ms());
                    if s.note.is_none() {
                        s.note = Some("process exited".into());
                    }
                }
                // Drop any leftover idle sentinel — the agent's Stop hook may have
                // touched it just before the process died, but a complete session
                // shouldn't keep flipping the kanban via Pass 1.5.
                crate::idle::clear_idle(&s.id);
                changed = true;
            }
        }
    }

    // Pass 1.5: pending detection — flip running ↔ pending based on
    // guard / elicitation pending requests for our session ids.
    let pending_sids = pending_session_ids();
    for s in sessions.iter_mut() {
        if s.pid.is_none() {
            continue;
        }
        let is_pending = pending_sids.contains(&s.id);
        let target = if is_pending { "pending" } else { DEFAULT_COLUMN_RUNNING };
        // Don't override user-set custom statuses or `complete`.
        if s.status == DEFAULT_COLUMN_RUNNING || s.status == "pending" {
            if s.status != target {
                s.status = target.into();
                changed = true;
            }
        }
    }

    // Pass 2: count current running per project.
    let mut running_by_proj: std::collections::HashMap<String, u32> =
        std::collections::HashMap::new();
    for s in &sessions {
        if s.pid.is_some() {
            *running_by_proj.entry(s.project_id.clone()).or_insert(0) += 1;
        }
    }

    // Pass 3: spawn queued sessions FIFO, respecting per-project concurrency.
    // Iterate in insertion order so the oldest queued goes first (FIFO).
    for s in sessions.iter_mut() {
        if s.status != DEFAULT_COLUMN_QUEUED || s.pid.is_some() {
            continue;
        }
        let cap = projects
            .iter()
            .find(|p| p.id == s.project_id)
            .map(|p| p.concurrency)
            .unwrap_or(1);
        let cur = *running_by_proj.get(&s.project_id).unwrap_or(&0);
        if !s.expedited && cur >= cap {
            continue;
        }

        // Decide whether this is the initial spawn or a --resume follow-up.
        let is_resume = s.started_at.is_some() || s.completed_at.is_some();
        // Drop any stale idle sentinel from a previous run before respawning,
        // otherwise Pass 1.5 would immediately flip the new run back to pending.
        crate::idle::clear_idle(&s.id);
        match spawn_claude(&s.id, &s.workspace, &s.prompt, &s.context_files, is_resume) {
            Ok(pid) => {
                s.pid = Some(pid);
                s.status = DEFAULT_COLUMN_RUNNING.into();
                if s.started_at.is_none() {
                    s.started_at = Some(now_ms());
                }
                *running_by_proj
                    .entry(s.project_id.clone())
                    .or_insert(0) += 1;
                changed = true;
            }
            Err(e) => {
                s.status = DEFAULT_COLUMN_COMPLETE.into();
                s.note = Some(format!("spawn failed: {e}"));
                s.completed_at = Some(now_ms());
                changed = true;
            }
        }
    }

    if changed {
        project::save_fleet_sessions(&sessions)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn spawn_claude(
    session_id: &str,
    workspace: &str,
    prompt: &str,
    context_files: &[String],
    is_resume: bool,
) -> Result<u32, String> {
    let claude = crate::claude_binary::resolve(None).ok_or("claude CLI not found")?;
    let mut full_prompt = prompt.to_string();
    if !context_files.is_empty() {
        full_prompt.push_str("\n\nContext files:\n");
        for f in context_files {
            full_prompt.push_str("- ");
            full_prompt.push_str(f);
            full_prompt.push('\n');
        }
    }

    let mut cmd = Command::new(&claude.path);
    cmd.current_dir(workspace).arg("--print");
    if is_resume {
        cmd.arg("--resume").arg(session_id);
    } else {
        cmd.arg("--session-id").arg(session_id);
    }
    cmd.arg(&full_prompt)
        .env("FLEET_SESSION_ID", session_id);

    // Prepend ~/.claude/fleet/bin to PATH so `fleet session status` is
    // discoverable from inside the agent (the symlink is created in P6 by
    // `claw_fleet_core::supervisor::ensure_fleet_cli_link`).
    if let Some(home) = crate::session::real_home_dir() {
        let bin = home.join(".claude").join("fleet").join("bin");
        let cur_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", bin.display(), cur_path);
        cmd.env("PATH", new_path);
    }

    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    Ok(child.id())
}

/// Create / refresh `~/.claude/fleet/bin/fleet → <fleet binary path>` so that
/// child agent processes can find `fleet session status` on PATH.
///
/// Called by Tauri / fleet serve at startup. Idempotent.
pub fn ensure_fleet_cli_link(fleet_path: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        let home = crate::session::real_home_dir().ok_or("no home dir")?;
        let bin_dir = home.join(".claude").join("fleet").join("bin");
        std::fs::create_dir_all(&bin_dir).map_err(|e| format!("mkdir bin: {e}"))?;
        let link = bin_dir.join("fleet");
        if link.exists() {
            // Compare existing symlink target — replace if it differs.
            if let Ok(existing) = std::fs::read_link(&link) {
                if existing.to_string_lossy() == fleet_path {
                    return Ok(());
                }
            }
            let _ = std::fs::remove_file(&link);
        }
        std::os::unix::fs::symlink(fleet_path, &link)
            .map_err(|e| format!("symlink: {e}"))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = fleet_path;
        Err("symlink helper is unix-only in v1".into())
    }
}

// ── Pending detection ───────────────────────────────────────────────────────

/// Build a set of session IDs that currently have a pending guard or
/// elicitation request awaiting user response, OR an idle sentinel touched by
/// the agent's Stop hook (turn ended, awaiting next user prompt). Used by
/// `tick_macos` to flip running↔pending for our fleet sessions.
fn pending_session_ids() -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for id in crate::guard::list_pending_requests() {
        if let Some(req) = crate::guard::read_request(&id) {
            out.insert(req.session_id);
        }
    }
    for id in crate::elicitation::list_pending_requests() {
        if let Some(req) = crate::elicitation::read_request(&id) {
            out.insert(req.session_id);
        }
    }
    for sid in crate::idle::list_idle_sessions() {
        out.insert(sid);
    }
    out
}

// ── PID helpers ─────────────────────────────────────────────────────────────

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // signal 0 = existence check; returns 0 if pid is alive and signalable.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn kill_pid(pid: u32) -> Result<(), String> {
    let r = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if r == 0 {
        Ok(())
    } else {
        Err(format!("kill: {}", std::io::Error::last_os_error()))
    }
}

#[cfg(not(unix))]
fn kill_pid(_pid: u32) -> Result<(), String> {
    Err("kill not supported on this platform".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // real_home_dir() reads $FLEET_HOME globally, so tests that mutate it
    // must serialize. Mirrors the helper pattern used in decision_history.
    static FLEET_HOME_LOCK: Mutex<()> = Mutex::new(());

    struct FleetHomeOverride {
        prev: Option<std::ffi::OsString>,
    }

    impl FleetHomeOverride {
        fn new(tmp: &std::path::Path) -> Self {
            let prev = std::env::var_os("FLEET_HOME");
            unsafe { std::env::set_var("FLEET_HOME", tmp) };
            FleetHomeOverride { prev }
        }
    }

    impl Drop for FleetHomeOverride {
        fn drop(&mut self) {
            unsafe {
                if let Some(p) = &self.prev {
                    std::env::set_var("FLEET_HOME", p);
                } else {
                    std::env::remove_var("FLEET_HOME");
                }
            }
        }
    }

    fn fresh_tmp_home(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "fleet-supervisor-test-{}-{}-{}",
            tag,
            std::process::id(),
            now_ms()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn enqueue_rejects_empty_prompt() {
        let r = enqueue(LauncherForm {
            project_id: "fake".into(),
            prompt: "  ".into(),
            context_files: vec![],
            expedited: false,
            fleetsession_dir: None,
        });
        assert!(r.is_err());
    }

    #[test]
    fn resume_rejects_empty_prompt() {
        let r = resume("fake", "  ".into());
        assert!(r.is_err());
    }

    /// End-to-end check: the agent's Stop hook drops a sentinel into
    /// `~/.fleet/idle/<sid>.json` (via `crate::idle::mark_idle`). The
    /// supervisor's `pending_session_ids()` must include that sid in its
    /// pending set so Pass 1.5 will flip the kanban card to Pending. After
    /// `clear_idle`, the sid must drop out of the set.
    #[test]
    fn idle_sentinel_appears_in_pending_session_ids() {
        let _g = FLEET_HOME_LOCK.lock().unwrap();
        let home = fresh_tmp_home("idle-pending");
        let _override = FleetHomeOverride::new(&home);

        let sid = "test-session-idle-flip";

        // Baseline: sid not pending.
        assert!(
            !pending_session_ids().contains(sid),
            "sid must not be pending before mark_idle"
        );

        // Stop hook fires → idle::mark_idle.
        crate::idle::mark_idle(sid).unwrap();
        assert!(
            pending_session_ids().contains(sid),
            "sid must appear in pending set after mark_idle"
        );

        // UserPromptSubmit hook fires → idle::clear_idle.
        crate::idle::clear_idle(sid);
        assert!(
            !pending_session_ids().contains(sid),
            "sid must drop out of pending set after clear_idle"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    /// Coexistence with the existing guard/elicitation pending sources: an
    /// idle sentinel for one session must not affect another session that
    /// has neither a sentinel nor a guard/elicitation request.
    #[test]
    fn idle_sentinel_only_affects_its_own_session() {
        let _g = FLEET_HOME_LOCK.lock().unwrap();
        let home = fresh_tmp_home("idle-isolate");
        let _override = FleetHomeOverride::new(&home);

        crate::idle::mark_idle("sess-a").unwrap();
        let pending = pending_session_ids();
        assert!(pending.contains("sess-a"));
        assert!(!pending.contains("sess-b"));

        let _ = std::fs::remove_dir_all(&home);
    }
}
