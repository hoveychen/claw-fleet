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

#[cfg(target_os = "macos")]
use std::process::Stdio;

use serde::{Deserialize, Serialize};

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
        final_by_agent: false,
        session_kind: project::SessionKind::Regular,
        task_id: None,
        p_item_id: None,
        system_prompt: None,
        model: None,
    };
    sessions.push(new.clone());
    project::save_fleet_sessions(&sessions)?;
    Ok(new)
}

/// Enqueue a **Worker** session for `(task_id, p_item_id)` using the prebuilt
/// `WorkerSpawnSpec`. Same pattern as `enqueue_master`: pre-allocate the
/// session id, persist queued, supervisor tick spawns the subprocess.
///
/// Callers should also stamp the returned id into
/// `task.plan.items[p_item_id].agent_session_id` so the master's
/// `read-output` tool can locate the worker's transcript.
pub fn enqueue_worker(spec: &crate::worker_executor::WorkerSpawnSpec) -> Result<String, String> {
    let projects = project::list_projects();
    let task = crate::task::get_task(&spec.task_id)?;
    let proj = projects
        .iter()
        .find(|p| p.id == task.project_id)
        .ok_or_else(|| format!("project {} not found for task {}", task.project_id, task.id))?;
    let id = uuid::Uuid::new_v4().to_string();
    let pitem = task.plan.get(&spec.p_item_id).ok_or_else(|| {
        format!(
            "p-item {} not found in task {} plan",
            spec.p_item_id, task.id
        )
    })?;
    let session = FleetSession {
        id: id.clone(),
        project_id: proj.id.clone(),
        workspace: spec.cwd.to_string_lossy().to_string(),
        fleetsession_path: None,
        prompt: format!(
            "Execute P-item `{}` per the SYSTEM prompt above. When you are done, \
             stop the process; the master will run the acceptance audit.\n\n\
             Your goal:\n{}",
            spec.p_item_id, pitem.desc
        ),
        context_files: vec![],
        status: DEFAULT_COLUMN_QUEUED.into(),
        note: Some(format!("worker: {}/{}", task.id, spec.p_item_id)),
        created_at: now_ms(),
        started_at: None,
        completed_at: None,
        pid: None,
        // Workers race the master's dispatch decision, not the kanban queue —
        // expedited so a fleet workspace with full concurrency doesn't starve
        // them.
        expedited: true,
        final_by_agent: false,
        session_kind: project::SessionKind::Worker,
        task_id: Some(task.id.clone()),
        p_item_id: Some(spec.p_item_id.clone()),
        system_prompt: Some(spec.system_prompt.clone()),
        model: Some(spec.model.to_string()),
    };
    let mut sessions = project::list_fleet_sessions();
    sessions.push(session);
    project::save_fleet_sessions(&sessions)?;
    Ok(id)
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
    // User-initiated cancel counts as proactive resolution — suppress the
    // wait-for-input card so it doesn't pop on a sid the user just killed.
    sessions[idx].final_by_agent = true;
    crate::idle::clear_idle(session_id);
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
    // Resume = "agent will run again", so we re-enter the wait-for-input
    // lifecycle from scratch: clear any leftover idle sentinel from the
    // previous turn (otherwise the card watcher would re-emit between this
    // call and the next supervisor tick that respawns claude) and reset
    // final_by_agent so the next end-of-turn can again show the card if the
    // agent doesn't self-mark.
    crate::idle::clear_idle(session_id);
    sessions[idx].final_by_agent = false;
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
        // Stamp completed_at + flag final_by_agent when moving into ANY
        // terminal column (default `complete` plus any user-defined
        // `is_terminal: true` column). `set_status` is the proactive path —
        // called from agent self-mark (`fleet session status complete`) or
        // from the SessionPendingCard buttons. The supervisor's own Pass 1
        // auto-complete on process exit takes a different code path that
        // intentionally leaves `final_by_agent = false`.
        let is_terminal = proj
            .map(|p| project::is_terminal_status(p, new_status))
            .unwrap_or(false);
        if is_terminal {
            sessions[idx].completed_at = Some(now_ms());
            sessions[idx].final_by_agent = true;
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

// ── Startup migration ────────────────────────────────────────────────────────

/// One-shot migration: scan fleet-sessions.json and recover any session that's
/// stuck in `running` / `pending` but whose pid is gone — either because the
/// pid is None (process was never recorded, or cleared without flipping
/// status), or because the recorded pid no longer responds to signal 0
/// (process exited but tick never reaped it, e.g. host crashed).
///
/// Should be called once when the backend starts up (LocalBackend::new and
/// `fleet serve` boot path). Idempotent — safe to call multiple times.
///
/// Returns the number of sessions that were migrated, for logging.
pub fn migrate_zombie_running() -> Result<usize, String> {
    let mut sessions = project::list_fleet_sessions();
    let mut migrated = 0usize;

    for s in sessions.iter_mut() {
        let stuck = s.status == DEFAULT_COLUMN_RUNNING || s.status == "pending";
        if !stuck {
            continue;
        }
        let dead = match s.pid {
            None => true,
            Some(pid) => !pid_alive(pid),
        };
        if !dead {
            continue;
        }
        s.pid = None;
        s.status = DEFAULT_COLUMN_COMPLETE.into();
        if s.completed_at.is_none() {
            s.completed_at = Some(now_ms());
        }
        if s.note.is_none() {
            s.note = Some("recovered on startup".into());
        }
        migrated += 1;
    }

    if migrated > 0 {
        project::save_fleet_sessions(&sessions)?;
    }
    Ok(migrated)
}

/// Build the "alive worktree" set from the task store and clean up the
/// rest. Called at backend startup (LocalBackend::new + `fleet serve`).
///
/// Returns the count of orphan worktree dirs that were removed, for
/// logging.
pub fn gc_orphan_worktrees() -> Result<usize, String> {
    let tasks = crate::task::list_tasks(None);
    let alive: Vec<(String, Vec<String>)> = tasks
        .iter()
        .filter(|t| matches!(t.status, crate::task::TaskStatus::Running))
        .map(|t| {
            let running_p_ids = t
                .plan
                .items
                .iter()
                .filter(|(_, p)| matches!(p.status, crate::pitem::PItemStatus::Running))
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            (t.id.clone(), running_p_ids)
        })
        .collect();
    let reaped = crate::worktree::gc_stale(&alive)?;
    Ok(reaped.len())
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
                // Intentionally NOT calling `idle::clear_idle` here. If the
                // agent's Stop hook fired right before the process exited
                // (the normal `claude --print` end-of-turn flow), the
                // sentinel survives this auto-complete so the wait-for-input
                // DecisionPanel card can pop and ask the user to confirm
                // (set_status will then flag `final_by_agent`) or send a
                // follow-up via `resume`. Hard cleanup of the sentinel
                // happens in `set_status` (terminal-column path) and in
                // `resume`, both of which represent a proactive user/agent
                // action.
                changed = true;
            }
        }
    }

    // Pass 1bis: recover sessions stuck in running/pending with no pid at all.
    // This catches host-crash zombies that Pass 1 can't see (it only checks
    // sessions whose pid field is Some).
    for s in sessions.iter_mut() {
        if s.pid.is_none()
            && (s.status == DEFAULT_COLUMN_RUNNING || s.status == "pending")
        {
            s.status = DEFAULT_COLUMN_COMPLETE.into();
            if s.completed_at.is_none() {
                s.completed_at = Some(now_ms());
            }
            if s.note.is_none() {
                s.note = Some("recovered on startup".into());
            }
            changed = true;
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
        match spawn_claude(
            &s.id,
            &s.workspace,
            &s.prompt,
            &s.context_files,
            is_resume,
            s.system_prompt.as_deref(),
            s.model.as_deref(),
            s.session_kind.clone(),
            s.task_id.as_deref(),
            s.p_item_id.as_deref(),
        ) {
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
    // Pass 4: reconcile task-as-unit completions — any Running task whose
    // master subprocess just exited AND whose plan is fully terminal now
    // flips to Done. Best-effort; an error here doesn't fail the tick.
    let _ = crate::task::reconcile_task_completion();
    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
fn spawn_claude(
    session_id: &str,
    workspace: &str,
    prompt: &str,
    context_files: &[String],
    is_resume: bool,
    system_prompt: Option<&str>,
    model: Option<&str>,
    kind: project::SessionKind,
    task_id: Option<&str>,
    p_item_id: Option<&str>,
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

    let mut cmd = crate::process_util::command(&claude.path);
    cmd.current_dir(workspace).arg("--print");
    if is_resume {
        cmd.arg("--resume").arg(session_id);
    } else {
        cmd.arg("--session-id").arg(session_id);
    }
    if let Some(sp) = system_prompt {
        cmd.arg("--append-system-prompt").arg(sp);
    }
    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }
    // Regular sessions: route native permission prompts to Fleet's Decision
    // Panel via --permission-prompt-tool (no-op when the fleet MCP server
    // isn't injected). Master/Worker sessions stay on headless auto-deny —
    // the task pipeline is meant to run unattended, and a blocking card
    // would stall the whole cluster instead of failing the one P-item.
    if matches!(kind, project::SessionKind::Regular) {
        for a in crate::session_launch::permission_prompt_tool_args() {
            cmd.arg(a);
        }
    }
    cmd.arg(&full_prompt)
        .env("FLEET_SESSION_ID", session_id);

    // Task-as-unit cluster env: workers consult FLEET_TASK_ID +
    // FLEET_P_ITEM_ID inside the Edit/Write touches hook (P8). Master
    // sessions only need FLEET_TASK_ID so they can route `[event]` /
    // `[user]` messages and acceptance-audit by id.
    if matches!(kind, project::SessionKind::Master | project::SessionKind::Worker) {
        if let Some(tid) = task_id {
            cmd.env("FLEET_TASK_ID", tid);
        }
        if let Some(pid) = p_item_id {
            cmd.env("FLEET_P_ITEM_ID", pid);
        }
        cmd.env(
            "FLEET_SESSION_KIND",
            match kind {
                project::SessionKind::Master => "master",
                project::SessionKind::Worker => "worker",
                project::SessionKind::Regular => "regular",
            },
        );
    }

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

/// One fleet session that has gone idle (Stop hook fired) and is awaiting user
/// input, paired with the project's terminal column ids so the DecisionPanel
/// card can render its action buttons. Returned by
/// [`fleet_sessions_needing_input`].
#[derive(Clone, Debug)]
pub struct PendingFleetSession {
    pub session: FleetSession,
    /// `(id, name)` pairs for every terminal kanban column on the owning
    /// project, in display order. Always at least one entry (`complete`).
    pub terminal_columns: Vec<(String, String)>,
}

/// One terminal kanban column, in the wire-format shape used by the
/// `session-pending-request` Tauri event and the
/// `GET /fleet_sessions/needing-input` HTTP endpoint.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalColumn {
    pub id: String,
    pub name: String,
}

/// Wait-for-input DecisionPanel request payload. Emitted as
/// `session-pending-request` by the desktop watcher and returned verbatim by
/// `GET /fleet_sessions/needing-input` (the remote-backend probe contract).
///
/// `id` is the unique key the frontend uses to dedup cards; we set it to
/// `session_id` because there is at most one pending-input card per session
/// at a time.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionPendingRequest {
    pub id: String,
    pub session_id: String,
    pub project_id: String,
    pub workspace: String,
    /// Display name for the card / tab (workspace basename in the wire
    /// payload; LocalBackend's watcher overwrites with the live SessionInfo
    /// workspace name before emitting the Tauri event).
    pub workspace_name: String,
    /// First non-empty line of the originating prompt, capped at 200 chars,
    /// so the card has something to show even before the frontend has
    /// fetched the live transcript tail.
    pub prompt_preview: String,
    pub terminal_columns: Vec<TerminalColumn>,
    /// `idle::IdleRecord.since` (ms since epoch) — when the agent's Stop hook
    /// last fired. `None` if the sentinel was cleared between listing and read.
    pub since_ms: Option<u64>,
}

/// Wire-format list for the `session-pending-request` event source. Wraps
/// [`fleet_sessions_needing_input`] with prompt-preview trimming and idle
/// timestamp lookup so both LocalBackend (direct call) and RemoteBackend
/// (via HTTP `GET /fleet_sessions/needing-input`) speak the same shape.
pub fn session_pending_requests() -> Vec<SessionPendingRequest> {
    fleet_sessions_needing_input()
        .into_iter()
        .map(|p| {
            let prompt_preview = p
                .session
                .prompt
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .chars()
                .take(200)
                .collect::<String>();
            let since_ms = crate::idle::read_idle(&p.session.id).map(|r| r.since);
            let workspace_name = std::path::Path::new(&p.session.workspace)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| p.session.workspace.clone());
            SessionPendingRequest {
                id: p.session.id.clone(),
                session_id: p.session.id.clone(),
                project_id: p.session.project_id.clone(),
                workspace: p.session.workspace.clone(),
                workspace_name,
                prompt_preview,
                terminal_columns: p
                    .terminal_columns
                    .into_iter()
                    .map(|(id, name)| TerminalColumn { id, name })
                    .collect(),
                since_ms,
            }
        })
        .collect()
}

/// List the fleet sessions that are currently idle (Stop hook fired) AND not
/// yet **proactively** finalised by the agent or user, AND whose owning
/// project we still know about. This is the source of truth for the
/// wait-for-input DecisionPanel card — guard / elicitation pendings are
/// surfaced through their own dedicated cards and are intentionally
/// excluded here.
///
/// The trigger is `idle::list_idle_sessions()`. The suppression check is
/// `!final_by_agent`, NOT a status-terminal check: when headless
/// `claude --print` exits at end-of-turn the supervisor's Pass 1 auto-marks
/// status = complete (per Boss's "process exit is normal" decision), but
/// `final_by_agent` stays false until the agent self-marks via fleet-cli
/// (`fleet session status <terminal>`) or the user picks a terminal-column
/// button on the SessionPendingCard. So sessions whose status was just
/// auto-completed by Pass 1 still surface here.
///
/// Sessions launched outside of fleet (no `FleetSession` record) are
/// skipped: the DecisionPanel only intervenes for fleet-managed main
/// agents. Subagents don't appear here either because the idle hook is
/// only wired to the `Stop` event, never `SubagentStop`.
pub fn fleet_sessions_needing_input() -> Vec<PendingFleetSession> {
    let idle_ids = crate::idle::list_idle_sessions();
    if idle_ids.is_empty() {
        return Vec::new();
    }
    let projects = project::list_projects();
    let sessions = project::list_fleet_sessions();

    let mut out = Vec::new();
    for sid in idle_ids {
        let Some(s) = sessions.iter().find(|s| s.id == sid) else {
            continue;
        };
        let Some(proj) = projects.iter().find(|p| p.id == s.project_id) else {
            continue;
        };
        if s.final_by_agent {
            continue;
        }
        let mut cols: Vec<&project::KanbanColumn> = proj
            .kanban_columns
            .iter()
            .filter(|c| c.is_terminal || c.id == DEFAULT_COLUMN_COMPLETE)
            .collect();
        cols.sort_by_key(|c| c.order);
        let terminal_columns: Vec<(String, String)> = cols
            .into_iter()
            .map(|c| (c.id.clone(), c.name.clone()))
            .collect();
        out.push(PendingFleetSession {
            session: s.clone(),
            terminal_columns,
        });
    }
    out
}

/// Build a set of session IDs that currently have a pending guard or
/// elicitation request awaiting user response, OR an idle sentinel touched by
/// the agent's Stop hook (turn ended, awaiting next user prompt). Used by
/// `tick_macos` to flip running↔pending for our fleet sessions.
#[cfg(any(target_os = "macos", test))]
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

#[cfg(unix)]
fn signal_pid(pid: u32, signal: i32) -> Result<(), String> {
    let r = unsafe { libc::kill(pid as libc::pid_t, signal) };
    if r == 0 {
        Ok(())
    } else {
        Err(format!("signal {signal}: {}", std::io::Error::last_os_error()))
    }
}

// signal_pid has no non-unix stub: every caller is itself inside
// `#[cfg(unix)]`, so a Windows stub would just be unreachable dead code.

// ── Task-as-unit signal helpers ──────────────────────────────────────────────

/// SIGSTOP every live fleet session attached to `task_id` (the Master plus
/// any in-flight Workers). Used by `task::pause_task` per TASKS P20 — user
/// hits pause, master + worker subprocesses freeze in place until `resume`.
///
/// Sessions without a live pid are silently skipped (already terminal or
/// not yet spawned). Returns the count of sessions signalled.
pub fn pause_task_sessions(task_id: &str) -> Result<usize, String> {
    #[cfg(unix)]
    {
        let mut n = 0usize;
        for s in project::list_fleet_sessions() {
            if s.task_id.as_deref() != Some(task_id) {
                continue;
            }
            if let Some(pid) = s.pid {
                if pid_alive(pid) {
                    // Best-effort: a missed signal isn't fatal — user can
                    // re-issue pause and the supervisor's next tick will
                    // try again on still-running pids.
                    let _ = signal_pid(pid, libc::SIGSTOP);
                    n += 1;
                }
            }
        }
        Ok(n)
    }
    #[cfg(not(unix))]
    {
        let _ = task_id;
        Ok(0)
    }
}

/// SIGCONT counterpart to `pause_task_sessions`.
pub fn resume_task_sessions(task_id: &str) -> Result<usize, String> {
    #[cfg(unix)]
    {
        let mut n = 0usize;
        for s in project::list_fleet_sessions() {
            if s.task_id.as_deref() != Some(task_id) {
                continue;
            }
            if let Some(pid) = s.pid {
                if pid_alive(pid) {
                    let _ = signal_pid(pid, libc::SIGCONT);
                    n += 1;
                }
            }
        }
        Ok(n)
    }
    #[cfg(not(unix))]
    {
        let _ = task_id;
        Ok(0)
    }
}

/// SIGTERM every live fleet session attached to `task_id` AND mark the
/// session records `complete` with note="task cleared". Used by
/// `task::clear_task`. The next supervisor tick reaps the pids; this
/// function eagerly marks the records so the UI updates without waiting.
pub fn terminate_task_sessions(task_id: &str) -> Result<usize, String> {
    let mut sessions = project::list_fleet_sessions();
    let mut n = 0usize;
    for s in sessions.iter_mut() {
        if s.task_id.as_deref() != Some(task_id) {
            continue;
        }
        if let Some(pid) = s.pid {
            if pid_alive(pid) {
                let _ = kill_pid(pid);
                n += 1;
            }
            s.pid = None;
        }
        if s.status != DEFAULT_COLUMN_COMPLETE {
            s.status = DEFAULT_COLUMN_COMPLETE.into();
            s.completed_at = Some(now_ms());
            s.note = Some("task cleared".into());
        }
    }
    project::save_fleet_sessions(&sessions)?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    // real_home_dir() reads $FLEET_HOME globally, so tests that mutate it
    // must serialize. Uses the crate-wide `crate::session::fleet_home_lock`
    // so tests in different modules don't race on the global env.

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
    /// pending set so Pass 1.5 will flip the session to Pending. After
    /// `clear_idle`, the sid must drop out of the set.
    #[test]
    fn idle_sentinel_appears_in_pending_session_ids() {
        let _g = crate::session::fleet_home_lock();
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
        let _g = crate::session::fleet_home_lock();
        let home = fresh_tmp_home("idle-isolate");
        let _override = FleetHomeOverride::new(&home);

        crate::idle::mark_idle("sess-a").unwrap();
        let pending = pending_session_ids();
        assert!(pending.contains("sess-a"));
        assert!(!pending.contains("sess-b"));

        let _ = std::fs::remove_dir_all(&home);
    }

    // ── fleet_sessions_needing_input ────────────────────────────────────────

    fn write_project_with_columns(home: &std::path::Path, cols: Vec<project::KanbanColumn>) -> project::Project {
        let dir = home.join(".claude").join("fleet");
        std::fs::create_dir_all(&dir).unwrap();
        let p = project::Project {
            id: "proj-1".into(),
            name: "test".into(),
            workspace: home.to_string_lossy().to_string(),
            concurrency: 1,
            kanban_columns: cols,
            created_at: 0,
            updated_at: 0,
            manual_review_all: false,
        };
        let json = serde_json::to_string_pretty(&vec![p.clone()]).unwrap();
        std::fs::write(dir.join("projects.json"), json).unwrap();
        p
    }

    fn write_session(home: &std::path::Path, sid: &str, project_id: &str, status: &str) {
        write_session_full(home, sid, project_id, status, false);
    }

    fn write_session_full(
        home: &std::path::Path,
        sid: &str,
        project_id: &str,
        status: &str,
        final_by_agent: bool,
    ) {
        let dir = home.join(".claude").join("fleet");
        std::fs::create_dir_all(&dir).unwrap();
        let mut existing: Vec<project::FleetSession> =
            std::fs::read_to_string(dir.join("fleet-sessions.json"))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
        existing.push(project::FleetSession {
            id: sid.into(),
            project_id: project_id.into(),
            workspace: home.to_string_lossy().to_string(),
            fleetsession_path: None,
            prompt: "p".into(),
            context_files: vec![],
            status: status.into(),
            note: None,
            created_at: 0,
            started_at: Some(0),
            completed_at: None,
            pid: None,
            expedited: false,
            final_by_agent,
            session_kind: project::SessionKind::Regular,
            task_id: None,
            p_item_id: None,
            system_prompt: None,
            model: None,
        });
        let json = serde_json::to_string_pretty(&existing).unwrap();
        std::fs::write(dir.join("fleet-sessions.json"), json).unwrap();
    }

    #[test]
    fn pending_input_excludes_only_final_by_agent_sessions() {
        // After Boss's "process exit is normal" decision, status=complete
        // alone no longer suppresses the card — Pass 1 auto-complete leaves
        // `final_by_agent = false` so the user can still pop the card and
        // confirm. Only sessions whose agent or user has *proactively*
        // marked final (via fleet-cli or the card buttons) are filtered.
        let _g = crate::session::fleet_home_lock();
        let home = fresh_tmp_home("pending-input-final-by-agent");
        let _override = FleetHomeOverride::new(&home);

        let project = write_project_with_columns(&home, project::default_kanban_columns());
        // Auto-completed by Pass 1 (process exited): status=complete but
        // final_by_agent=false → must still appear in the pending list.
        write_session_full(
            &home,
            "sess-auto-complete",
            &project.id,
            project::DEFAULT_COLUMN_COMPLETE,
            false,
        );
        // Agent self-marked complete: status=complete + final_by_agent=true
        // → must be excluded from the pending list (no card).
        write_session_full(
            &home,
            "sess-marked-final",
            &project.id,
            project::DEFAULT_COLUMN_COMPLETE,
            true,
        );
        crate::idle::mark_idle("sess-auto-complete").unwrap();
        crate::idle::mark_idle("sess-marked-final").unwrap();

        let pending = fleet_sessions_needing_input();
        let ids: Vec<&str> = pending.iter().map(|p| p.session.id.as_str()).collect();
        assert_eq!(ids, vec!["sess-auto-complete"]);

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn pending_input_skips_sessions_outside_known_projects() {
        let _g = crate::session::fleet_home_lock();
        let home = fresh_tmp_home("pending-input-orphan");
        let _override = FleetHomeOverride::new(&home);

        // No projects.json at all → an idle sentinel for an untracked sid must
        // produce zero card emissions.
        crate::idle::mark_idle("rogue-sid").unwrap();
        assert!(fleet_sessions_needing_input().is_empty());

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn pending_input_lists_terminal_columns_in_order() {
        let _g = crate::session::fleet_home_lock();
        let home = fresh_tmp_home("pending-input-cols");
        let _override = FleetHomeOverride::new(&home);

        let mut cols = project::default_kanban_columns();
        cols.push(project::KanbanColumn {
            id: "shipped".into(),
            name: "Shipped".into(),
            color: None,
            is_default: false,
            is_terminal: true,
            order: 5, // after complete (order=3)
        });
        cols.push(project::KanbanColumn {
            id: "blocked".into(),
            name: "Blocked".into(),
            color: None,
            is_default: false,
            is_terminal: false,
            order: 4, // after complete but not terminal — must be filtered
        });
        let project = write_project_with_columns(&home, cols);
        write_session(&home, "sess-1", &project.id, project::DEFAULT_COLUMN_RUNNING);
        crate::idle::mark_idle("sess-1").unwrap();

        let pending = fleet_sessions_needing_input();
        assert_eq!(pending.len(), 1);
        let cols = &pending[0].terminal_columns;
        assert_eq!(
            cols,
            &vec![
                (project::DEFAULT_COLUMN_COMPLETE.into(), "Complete".into()),
                ("shipped".into(), "Shipped".into()),
            ],
            "terminal_columns must be sorted by order and exclude non-terminal entries"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn session_pending_requests_includes_prompt_preview_and_since_ms() {
        let _g = crate::session::fleet_home_lock();
        let home = fresh_tmp_home("pending-input-wire");
        let _override = FleetHomeOverride::new(&home);

        let project = write_project_with_columns(&home, project::default_kanban_columns());
        write_session(&home, "sess-1", &project.id, project::DEFAULT_COLUMN_RUNNING);
        // Override the prompt to test prompt_preview trimming.
        let mut sessions = project::list_fleet_sessions();
        sessions[0].prompt = "  \n\nfirst line that should appear\nsecond line should not\n"
            .into();
        project::save_fleet_sessions(&sessions).unwrap();
        crate::idle::mark_idle("sess-1").unwrap();

        let requests = session_pending_requests();
        assert_eq!(requests.len(), 1);
        let r = &requests[0];
        assert_eq!(r.id, r.session_id);
        assert_eq!(r.session_id, "sess-1");
        assert_eq!(r.project_id, project.id);
        assert_eq!(r.prompt_preview, "first line that should appear");
        assert!(r.since_ms.is_some(), "since_ms must be populated when sentinel exists");
        assert_eq!(
            r.terminal_columns,
            vec![TerminalColumn {
                id: project::DEFAULT_COLUMN_COMPLETE.into(),
                name: "Complete".into()
            }]
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn session_pending_request_serialises_camel_case() {
        // The wire contract used by the HTTP probe and Tauri event emitter:
        // verify keys are camelCase so the TypeScript side parses cleanly
        // without an extra rename layer.
        let req = SessionPendingRequest {
            id: "s1".into(),
            session_id: "s1".into(),
            project_id: "p1".into(),
            workspace: "/ws".into(),
            workspace_name: "ws".into(),
            prompt_preview: "hi".into(),
            terminal_columns: vec![TerminalColumn {
                id: "complete".into(),
                name: "Complete".into(),
            }],
            since_ms: Some(123),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("sessionId").is_some(), "sessionId (camelCase) expected");
        assert!(json.get("projectId").is_some());
        assert!(json.get("workspaceName").is_some());
        assert!(json.get("promptPreview").is_some());
        assert!(json.get("terminalColumns").is_some());
        assert!(json.get("sinceMs").is_some());
        // snake_case keys must NOT exist.
        assert!(json.get("session_id").is_none());
    }

    #[test]
    fn set_status_stamps_completed_at_and_final_by_agent_for_terminal_column() {
        let _g = crate::session::fleet_home_lock();
        let home = fresh_tmp_home("set-status-terminal");
        let _override = FleetHomeOverride::new(&home);

        let mut cols = project::default_kanban_columns();
        cols.push(project::KanbanColumn {
            id: "shipped".into(),
            name: "Shipped".into(),
            color: None,
            is_default: false,
            is_terminal: true,
            order: 5,
        });
        let project = write_project_with_columns(&home, cols);
        write_session(&home, "sess-1", &project.id, project::DEFAULT_COLUMN_RUNNING);

        set_status("sess-1", "shipped", None).unwrap();

        let sessions = project::list_fleet_sessions();
        let s = sessions.iter().find(|s| s.id == "sess-1").unwrap();
        assert_eq!(s.status, "shipped");
        assert!(
            s.completed_at.is_some(),
            "completed_at must be stamped when moving into a terminal column"
        );
        assert!(
            s.final_by_agent,
            "final_by_agent must be set when proactively moving into a terminal column"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn resume_clears_idle_sentinel_and_resets_final_by_agent() {
        // Resuming a session with a follow-up prompt re-opens the lifecycle.
        // Both `idle` sentinel and `final_by_agent` flag must be reset so the
        // card watcher won't keep emitting the (now stale) card and the next
        // end-of-turn can again fire a fresh card.
        let _g = crate::session::fleet_home_lock();
        let home = fresh_tmp_home("resume-clears-sentinel");
        let _override = FleetHomeOverride::new(&home);

        let project = write_project_with_columns(&home, project::default_kanban_columns());
        write_session_full(
            &home,
            "sess-1",
            &project.id,
            project::DEFAULT_COLUMN_COMPLETE,
            true,
        );
        crate::idle::mark_idle("sess-1").unwrap();
        assert!(crate::idle::list_idle_sessions().contains(&"sess-1".to_string()));

        resume("sess-1", "follow-up please".into()).unwrap();

        assert!(
            !crate::idle::list_idle_sessions().contains(&"sess-1".to_string()),
            "resume must clear the idle sentinel"
        );
        let sessions = project::list_fleet_sessions();
        let s = sessions.iter().find(|s| s.id == "sess-1").unwrap();
        assert_eq!(s.status, project::DEFAULT_COLUMN_QUEUED);
        assert!(!s.final_by_agent, "resume must reset final_by_agent");

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn migrate_zombie_running_recovers_pidless_running_and_pending() {
        let _g = crate::session::fleet_home_lock();
        let home = fresh_tmp_home("migrate-zombie");
        let _override = FleetHomeOverride::new(&home);

        let project = write_project_with_columns(&home, project::default_kanban_columns());
        // pidless Running — must be recovered
        write_session(&home, "sess-zombie-running", &project.id, project::DEFAULT_COLUMN_RUNNING);
        // pidless Pending — must be recovered
        write_session(&home, "sess-zombie-pending", &project.id, "pending");
        // Queued — must NOT be touched (still waiting for slot)
        write_session(&home, "sess-queued", &project.id, project::DEFAULT_COLUMN_QUEUED);
        // Already complete — must NOT be touched
        write_session(&home, "sess-done", &project.id, project::DEFAULT_COLUMN_COMPLETE);

        let migrated = migrate_zombie_running().unwrap();
        assert_eq!(migrated, 2, "exactly the two zombie rows must be migrated");

        let sessions = project::list_fleet_sessions();
        let by_id = |sid: &str| sessions.iter().find(|s| s.id == sid).unwrap().clone();

        let r = by_id("sess-zombie-running");
        assert_eq!(r.status, project::DEFAULT_COLUMN_COMPLETE);
        assert!(r.completed_at.is_some());
        assert_eq!(r.note.as_deref(), Some("recovered on startup"));

        let p = by_id("sess-zombie-pending");
        assert_eq!(p.status, project::DEFAULT_COLUMN_COMPLETE);
        assert!(p.completed_at.is_some());

        assert_eq!(by_id("sess-queued").status, project::DEFAULT_COLUMN_QUEUED);
        assert_eq!(by_id("sess-done").status, project::DEFAULT_COLUMN_COMPLETE);

        // Idempotency — running again must report 0 migrated.
        let again = migrate_zombie_running().unwrap();
        assert_eq!(again, 0, "second run is a no-op");

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn migrate_zombie_running_handles_dead_pid() {
        let _g = crate::session::fleet_home_lock();
        let home = fresh_tmp_home("migrate-zombie-deadpid");
        let _override = FleetHomeOverride::new(&home);

        let project = write_project_with_columns(&home, project::default_kanban_columns());

        // Running session with a clearly-dead pid (pid 0 is reserved on unix
        // and never represents a live user process; pid_alive returns false).
        let dir = home.join(".claude").join("fleet");
        std::fs::create_dir_all(&dir).unwrap();
        let s = project::FleetSession {
            id: "sess-dead-pid".into(),
            project_id: project.id.clone(),
            workspace: home.to_string_lossy().to_string(),
            fleetsession_path: None,
            prompt: "p".into(),
            context_files: vec![],
            status: project::DEFAULT_COLUMN_RUNNING.into(),
            note: None,
            created_at: 0,
            started_at: Some(0),
            completed_at: None,
            // Pick a pid that the kernel definitely does not have allocated.
            // i32::MAX is well beyond pid_max on every supported platform.
            pid: Some(i32::MAX as u32 - 1),
            expedited: false,
            final_by_agent: false,
            session_kind: project::SessionKind::Regular,
            task_id: None,
            p_item_id: None,
            system_prompt: None,
            model: None,
        };
        let json = serde_json::to_string_pretty(&vec![s]).unwrap();
        std::fs::write(dir.join("fleet-sessions.json"), json).unwrap();

        let migrated = migrate_zombie_running().unwrap();
        assert_eq!(migrated, 1);

        let sessions = project::list_fleet_sessions();
        let r = sessions.iter().find(|x| x.id == "sess-dead-pid").unwrap();
        assert_eq!(r.status, project::DEFAULT_COLUMN_COMPLETE);
        assert!(r.pid.is_none(), "pid must be cleared after migration");

        let _ = std::fs::remove_dir_all(&home);
    }
}
