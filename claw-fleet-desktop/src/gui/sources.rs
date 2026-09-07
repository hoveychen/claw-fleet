use super::*;

// ── Agent sources config ─────────────────────────────────────────────────────

/// Return the current sources config merged with availability info.
#[tauri::command(async)]
pub(crate) fn get_sources_config(state: tauri::State<'_, AppState>) -> Vec<agent_source::SourceInfo> {
    state.backend.get_sources_config()
}

/// Codex profile-v2 files on this machine — the non-official half of the
/// model picker.
#[tauri::command(async)]
pub(crate) fn list_codex_profiles(
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::codex_launch::CodexProfile> {
    state.backend.list_codex_profiles()
}

/// Toggle a source on/off and persist to disk (local or remote).
#[tauri::command(async)]
pub(crate) fn set_source_enabled(name: String, enabled: bool, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.set_source_enabled(&name, enabled)
}

