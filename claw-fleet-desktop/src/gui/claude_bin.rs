use super::*;

// ── Claude binary discovery & override ──────────────────────────────────────

#[tauri::command(async)]
pub(crate) fn list_claude_binaries(state: tauri::State<'_, AppState>) -> Vec<claude_binary::ClaudeBinary> {
    state.backend.read().unwrap().list_claude_binaries()
}

#[tauri::command(async)]
pub(crate) fn get_claude_binary_override(state: tauri::State<'_, AppState>) -> Option<String> {
    state.backend.read().unwrap().get_claude_binary_override()
}

#[tauri::command(async)]
pub(crate) fn set_claude_binary_override(path: Option<String>, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.write().unwrap().set_claude_binary_override(path)
}

