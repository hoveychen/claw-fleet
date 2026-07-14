use super::*;

// ── Agent sources config ─────────────────────────────────────────────────────

/// Return the current sources config merged with availability info.
#[tauri::command]
pub(crate) fn get_sources_config(state: tauri::State<AppState>) -> Vec<agent_source::SourceInfo> {
    state.backend.read().unwrap().get_sources_config()
}

/// Toggle a source on/off and persist to disk (local or remote).
#[tauri::command]
pub(crate) fn set_source_enabled(name: String, enabled: bool, state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().set_source_enabled(&name, enabled)
}

