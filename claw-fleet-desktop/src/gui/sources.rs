use super::*;

// ── Agent sources config ─────────────────────────────────────────────────────

/// Return the current sources config merged with availability info.
#[tauri::command(async)]
pub(crate) fn get_sources_config(state: tauri::State<'_, AppState>) -> Vec<agent_source::SourceInfo> {
    state.backend.read().unwrap().get_sources_config()
}

/// Toggle a source on/off and persist to disk (local or remote).
#[tauri::command(async)]
pub(crate) fn set_source_enabled(name: String, enabled: bool, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.write().unwrap().set_source_enabled(&name, enabled)
}

