use super::*;

// ── Agent sources config ─────────────────────────────────────────────────────

/// Return the current sources config merged with availability info.
#[tauri::command(async)]
pub(crate) fn get_sources_config(state: tauri::State<'_, AppState>) -> Vec<agent_source::SourceInfo> {
    state.backend.read().unwrap().get_sources_config()
}

/// Codex profile-v2 files on whichever host will run Codex — the non-official
/// half of the model picker. Goes through the backend so a remote workspace
/// reports the probe host's profiles, not the desktop's.
#[tauri::command(async)]
pub(crate) fn list_codex_profiles(
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::codex_launch::CodexProfile> {
    state.backend.read().unwrap().list_codex_profiles()
}

/// Toggle a source on/off and persist to disk (local or remote).
#[tauri::command(async)]
pub(crate) fn set_source_enabled(name: String, enabled: bool, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.write().unwrap().set_source_enabled(&name, enabled)
}

