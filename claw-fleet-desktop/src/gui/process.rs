use super::*;

// ── Process kill ──────────────────────────────────────────────────────────────

/// Graceful stop: abort the in-flight tool call, keep the transcript resumable.
/// Only ever called with a `pidPrecise` session — SIGINT aimed at the wrong
/// sibling would abort someone else's turn.
#[tauri::command(async)]
pub(crate) fn interrupt_session(pid: u32, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.write().unwrap().interrupt_pid(pid)
}

#[tauri::command(async)]
pub(crate) fn kill_session(pid: u32, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.write().unwrap().kill_pid(pid)
}

#[tauri::command(async)]
pub(crate) fn kill_workspace_sessions(workspace_path: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.write().unwrap().kill_workspace(workspace_path)
}

#[tauri::command(async)]
pub(crate) fn resume_rate_limited_session(
    session_id: String,
    workspace_path: String,
    prompt: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    permission_mode: Option<String>,
    agent_source: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.write().unwrap().resume_session(
        session_id,
        workspace_path,
        prompt,
        model,
        effort,
        permission_mode,
        agent_source.unwrap_or_default(),
    )
}

/// Queue a follow-up message for a session that is still mid-turn. The message
/// is delivered via `claude --resume` the moment the current turn ends (see
/// [`claw_fleet_core::pending_message`]).
#[tauri::command(async)]
pub(crate) fn enqueue_session_message(
    session_id: String,
    workspace_path: String,
    text: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state
        .backend
        .write()
        .unwrap()
        .enqueue_message(session_id, workspace_path, text)
}

/// Async so the spawn never blocks the UI thread. The codex path
/// (`spawn_new_codex_session`) synchronously waits up to 30s for the child's
/// `thread.started` line to learn its thread id before returning — on the main
/// thread that froze the whole window. `spawn_blocking` moves that wait onto a
/// tokio worker; the claude path returns immediately regardless, so both stay
/// responsive.
#[tauri::command]
pub(crate) async fn spawn_new_claude_session(
    workspace_path: String,
    prompt: String,
    model: Option<String>,
    effort: Option<String>,
    permission_mode: Option<String>,
    // Which agent tool to launch: "claude" (or omitted) / "codex".
    tool: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::session_launch::SpawnSessionResponse, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || {
        backend
            .read()
            .unwrap()
            .spawn_new_session(workspace_path, prompt, model, effort, permission_mode, tool)
    })
    .await
    .map_err(|e| format!("join: {e}"))?
}

/// Absolute path of the pure-chat workspace, created on demand. The launcher
/// pins it as a fixed entry — it has no prior sessions to be discovered from,
/// and under a remote connection it resolves against the probe host's home.
#[tauri::command(async)]
pub(crate) fn chat_workspace(state: tauri::State<'_, AppState>) -> Result<String, String> {
    state.backend.write().unwrap().chat_workspace()
}

/// One level of directories under `path` on the *backend host* (`None` = its
/// home), for the launcher's workspace picker.
///
/// Goes through the backend rather than Tauri's native dialog plugin because the
/// dialog can only ever browse the machine the desktop runs on — under a remote
/// connection that is the wrong host, which is why the launcher used to just hide
/// its Browse button in that mode.
#[tauri::command(async)]
pub(crate) fn browse_dir(
    state: tauri::State<'_, AppState>,
    path: Option<String>,
) -> Result<claw_fleet_core::workspace_browse::BrowseDirResponse, String> {
    state.backend.read().unwrap().browse_dir(path)
}

#[tauri::command(async)]
pub(crate) fn get_auto_resume_config(
    state: tauri::State<'_, AppState>,
) -> claw_fleet_core::auto_resume::AutoResumeConfig {
    state.backend.read().unwrap().get_auto_resume_config()
}

#[tauri::command(async)]
pub(crate) fn set_auto_resume_config(
    config: claw_fleet_core::auto_resume::AutoResumeConfig,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.write().unwrap().set_auto_resume_config(config)
}

/// Set (or clear, when `mark` is null) the human's manual review mark for a
/// session. Delegates via the backend so LocalBackend writes the local
/// side-channel file and RemoteBackend POSTs it to the probe.
#[tauri::command(async)]
pub(crate) fn set_session_mark(
    session_id: String,
    workspace_path: String,
    mark: Option<claw_fleet_core::session_mark::SessionMark>,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state
        .backend
        .write()
        .unwrap()
        .set_session_mark(session_id, workspace_path, mark)
}

/// Mark a batch of sessions read as of now (a single row-read is a batch of one,
/// "一键清除未读" a batch of many). Unread is derived from `last_read_ms` vs
/// `last_activity_ms`, so this only stamps the read time.
#[tauri::command(async)]
pub(crate) fn mark_sessions_read(
    items: Vec<claw_fleet_core::session_read::SessionReadItem>,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.write().unwrap().mark_sessions_read(items)
}

// ── Keep-awake (caffeinate -i equivalent) ────────────────────────────────────
// Desktop-local power state, deliberately NOT routed through the Backend
// trait: the assertion controls the machine the desktop app runs on (like
// app_nap / tray), not the session host. See keep_awake.rs.

#[tauri::command]
pub(crate) fn keep_awake_supported() -> bool {
    crate::keep_awake::is_supported()
}

#[tauri::command]
pub(crate) fn get_keep_awake() -> bool {
    crate::keep_awake::is_enabled()
}

#[tauri::command]
pub(crate) fn set_keep_awake(app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    let state = crate::keep_awake::set_enabled(enabled)?;
    // Keep the main-window toolbar button and the standalone settings window
    // in sync — both listen for this event.
    let _ = app.emit("keep-awake-changed", state);
    Ok(state)
}

