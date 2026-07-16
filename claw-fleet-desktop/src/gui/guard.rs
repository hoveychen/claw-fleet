use super::*;

// ── Guard (real-time interception) ───────────────────────────────────────────

#[tauri::command(async)]
pub(crate) fn apply_guard_hook(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.write().unwrap().apply_guard_hook()
}

#[tauri::command(async)]
pub(crate) fn remove_guard_hook(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.write().unwrap().remove_guard_hook()
}

#[tauri::command(async)]
pub(crate) fn respond_to_guard(
    state: tauri::State<'_, AppState>,
    id: String,
    allow: bool,
    always_allow: Option<claw_fleet_core::guard::GuardAlwaysAllow>,
    reason: Option<String>,
) -> Result<(), String> {
    state
        .backend
        .write()
        .unwrap()
        .respond_to_guard(&id, allow, always_allow, reason)
}

#[tauri::command(async)]
pub(crate) fn list_guard_allow_rules(
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::audit::GuardAllowRule> {
    state.backend.read().unwrap().list_guard_allow_rules()
}

#[tauri::command(async)]
pub(crate) fn remove_guard_allow_rule(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    state.backend.write().unwrap().remove_guard_allow_rule(&id)
}

#[tauri::command]
pub(crate) async fn analyze_guard_command(
    state: tauri::State<'_, AppState>,
    command: String,
    context: String,
    lang: String,
) -> Result<String, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || {
        backend
            .read()
            .unwrap()
            .analyze_guard_command(&command, &context, &lang)
    })
    .await
    .map_err(|e| format!("task join error: {e}"))?
}

