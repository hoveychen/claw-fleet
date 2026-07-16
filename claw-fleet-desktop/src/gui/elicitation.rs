use super::*;

// ── Elicitation (AskUserQuestion interception) ──────────────────────────────

#[tauri::command]
pub(crate) fn apply_elicitation_hook(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().apply_elicitation_hook()
}

#[tauri::command]
pub(crate) fn remove_elicitation_hook(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().remove_elicitation_hook()
}

#[tauri::command]
pub(crate) fn apply_interaction_mode(state: tauri::State<AppState>) -> Result<(), String> {
    let title = state.user_title.lock().unwrap().clone();
    let locale = state.locale.lock().unwrap().clone();
    state.backend.read().unwrap().apply_interaction_mode(&title, &locale)
}

#[tauri::command]
pub(crate) fn remove_interaction_mode(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().remove_interaction_mode()
}

#[tauri::command]
pub(crate) fn apply_wiki_guidance(state: tauri::State<AppState>) -> Result<(), String> {
    let locale = state.locale.lock().unwrap().clone();
    state.backend.read().unwrap().apply_wiki_guidance(&locale)
}

#[tauri::command]
pub(crate) fn remove_wiki_guidance(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().remove_wiki_guidance()
}

#[tauri::command]
pub(crate) fn apply_model_guidance(state: tauri::State<AppState>) -> Result<(), String> {
    let locale = state.locale.lock().unwrap().clone();
    state.backend.read().unwrap().apply_model_guidance(&locale)
}

#[tauri::command]
pub(crate) fn remove_model_guidance(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().remove_model_guidance()
}

#[tauri::command]
pub(crate) fn get_interaction_diagnostics(
    state: tauri::State<AppState>,
) -> Vec<claw_fleet_core::interaction_mode_diagnostics::DiagnosticCheck> {
    state.backend.read().unwrap().interaction_diagnostics()
}

#[tauri::command]
pub(crate) fn test_decision_frontend_only(
    app: tauri::AppHandle,
) -> Result<claw_fleet_core::interaction_mode_test::TestRunResult, String> {
    use tauri::Emitter;
    let req = claw_fleet_core::interaction_mode_test::build_test_request();
    app.emit("elicitation-request", &req)
        .map_err(|e| format!("emit elicitation-request: {e}"))?;
    Ok(claw_fleet_core::interaction_mode_test::TestRunResult {
        kind: "frontend_only".into(),
        request_id: Some(req.id),
        message: "Synthetic event emitted directly to the Tauri listener — no file or SSE involved".into(),
        claude_output: None,
    })
}

#[tauri::command]
pub(crate) async fn test_decision_end_to_end(
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::interaction_mode_test::TestRunResult, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || backend.read().unwrap().test_decision_end_to_end())
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
pub(crate) async fn test_decision_via_claude_cli(
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::interaction_mode_test::TestRunResult, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || backend.read().unwrap().test_decision_via_claude_cli())
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
pub(crate) async fn test_fleet_ask_end_to_end(
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::interaction_mode_test::TestRunResult, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || backend.read().unwrap().test_fleet_ask_end_to_end())
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
pub(crate) async fn test_fleet_ask_via_claude_cli(
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::interaction_mode_test::TestRunResult, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || backend.read().unwrap().test_fleet_ask_via_claude_cli())
        .await
        .map_err(|e| format!("task join error: {e}"))?
}

#[tauri::command]
pub(crate) fn apply_prd_mode(state: tauri::State<AppState>) -> Result<(), String> {
    let title = state.user_title.lock().unwrap().clone();
    let locale = state.locale.lock().unwrap().clone();
    state.backend.read().unwrap().apply_prd_mode(&title, &locale)
}

#[tauri::command]
pub(crate) fn remove_prd_mode(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().remove_prd_mode()
}

#[tauri::command]
pub(crate) fn apply_codex_guidance(state: tauri::State<AppState>) -> Result<(), String> {
    let title = state.user_title.lock().unwrap().clone();
    let locale = state.locale.lock().unwrap().clone();
    state
        .backend
        .read()
        .unwrap()
        .apply_codex_guidance(&title, &locale)
}

#[tauri::command]
pub(crate) fn remove_codex_guidance(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().remove_codex_guidance()
}

/// Mirror the Claude concept toggles onto codex's AGENTS.md. Called by the
/// desktop after any concept toggle (interaction / PRD / wiki / model) and on
/// startup, so codex's per-concept blocks always match the Claude sentinels.
#[tauri::command]
pub(crate) fn reconcile_codex_guidance(state: tauri::State<AppState>) -> Result<(), String> {
    let title = state.user_title.lock().unwrap().clone();
    let locale = state.locale.lock().unwrap().clone();
    state
        .backend
        .read()
        .unwrap()
        .reconcile_codex_guidance(&title, &locale)
}

#[tauri::command]
pub(crate) fn respond_to_elicitation(
    state: tauri::State<AppState>,
    id: String,
    declined: bool,
    answers: std::collections::HashMap<String, String>,
) -> Result<(), String> {
    state
        .backend
        .read()
        .unwrap()
        .respond_to_elicitation(&id, declined, answers)
}

#[tauri::command]
pub(crate) fn respond_to_fleet_ask(
    state: tauri::State<AppState>,
    id: String,
    cancelled: bool,
    answers: std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    state
        .backend
        .read()
        .unwrap()
        .respond_to_fleet_ask(&id, cancelled, answers)
}

#[tauri::command]
pub(crate) fn respond_to_permission_prompt(
    state: tauri::State<AppState>,
    id: String,
    allow: bool,
    reason: Option<String>,
) -> Result<(), String> {
    state
        .backend
        .read()
        .unwrap()
        .respond_to_permission_prompt(&id, allow, reason)
}

#[tauri::command]
pub(crate) fn respond_to_a2ui_render(
    state: tauri::State<AppState>,
    id: String,
    cancelled: bool,
    action_name: Option<String>,
    action_context: std::collections::BTreeMap<String, String>,
) -> Result<(), String> {
    state
        .backend
        .read()
        .unwrap()
        .respond_to_a2ui_render(&id, cancelled, action_name, action_context)
}

/// One-click fix for the `mcp_injection` diagnostic. Resolves the fleet
/// sibling binary and re-acquires the `mcpServers.fleet` entry in
/// `~/.claude.json`. Returns an error when the sibling binary can't be
/// located so the frontend can surface a useful hint.
#[tauri::command]
pub(crate) fn apply_mcp_injector(state: tauri::State<AppState>) -> Result<(), String> {
    let p = crate::fleet_binary::resolve_fleet_binary()
        .ok_or("Fleet sibling binary not found near this Fleet desktop process — \
                 build fleet-cli or install the production sidecar so the MCP \
                 injector can point at a real `command` path")?;
    let fleet_path = p.to_string_lossy().to_string();
    state.backend.read().unwrap().apply_mcp_injector(&fleet_path)
}

#[tauri::command]
pub(crate) fn upload_elicitation_attachment(
    state: tauri::State<AppState>,
    source_path: String,
    from_clipboard: bool,
) -> Result<String, String> {
    state
        .backend
        .read()
        .unwrap()
        .upload_attachment(std::path::Path::new(&source_path), from_clipboard)
}

/// Writes clipboard/drag-drop bytes to the OS temp dir and returns the absolute
/// path so the caller can feed it to `upload_elicitation_attachment`.
#[tauri::command]
pub(crate) fn stage_pasted_attachment(bytes: Vec<u8>, extension: String) -> Result<String, String> {
    use std::time::{SystemTime, UNIX_EPOCH};
    if (bytes.len() as u64) > claw_fleet_core::backend::MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "attachment too large: {} bytes (max {})",
            bytes.len(),
            claw_fleet_core::backend::MAX_ATTACHMENT_BYTES
        ));
    }
    let dir = std::env::temp_dir().join("fleet-pasted");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let ext = extension.trim_start_matches('.');
    let filename = if ext.is_empty() {
        format!("paste-{nanos}-{pid}.bin")
    } else {
        format!("paste-{nanos}-{pid}.{ext}")
    };
    let dest = dir.join(&filename);
    std::fs::write(&dest, &bytes).map_err(|e| e.to_string())?;
    Ok(dest.to_string_lossy().into_owned())
}

/// Reads the contents of an arbitrary local file path so the renderer can
/// turn a `plugin-dialog` selection into the same `bytes` payload that the
/// existing paste / drag-drop path produces.
#[tauri::command]
pub(crate) fn read_local_file_bytes(path: String) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    if (bytes.len() as u64) > claw_fleet_core::backend::MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "file too large: {} bytes (max {})",
            bytes.len(),
            claw_fleet_core::backend::MAX_ATTACHMENT_BYTES
        ));
    }
    Ok(bytes)
}

