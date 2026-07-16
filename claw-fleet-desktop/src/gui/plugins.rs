use super::*;
use crate::plugins;

// ── Plugins ──────────────────────────────────────────────────────────────────

#[tauri::command(async)]
pub(crate) fn list_plugins(state: tauri::State<'_, AppState>) -> Vec<plugins::PluginItem> {
    state.backend.read().unwrap().list_plugins()
}

#[tauri::command(async)]
pub(crate) fn set_plugin_enabled(
    state: tauri::State<'_, AppState>,
    plugin_id: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .backend
        .write()
        .unwrap()
        .set_plugin_enabled(&plugin_id, enabled)
}

#[tauri::command(async)]
pub(crate) fn install_plugin(state: tauri::State<'_, AppState>, plugin_id: String) -> Result<(), String> {
    state.backend.write().unwrap().install_plugin(&plugin_id)
}

#[tauri::command(async)]
pub(crate) fn uninstall_plugin(state: tauri::State<'_, AppState>, plugin_id: String) -> Result<(), String> {
    state.backend.write().unwrap().uninstall_plugin(&plugin_id)
}

#[tauri::command(async)]
pub(crate) fn list_marketplaces(
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::claude_cli::CliMarketplace> {
    state.backend.read().unwrap().list_marketplaces()
}

#[tauri::command(async)]
pub(crate) fn add_marketplace(state: tauri::State<'_, AppState>, source: String) -> Result<(), String> {
    state.backend.write().unwrap().add_marketplace(&source)
}

#[tauri::command(async)]
pub(crate) fn remove_marketplace(state: tauri::State<'_, AppState>, name: String) -> Result<(), String> {
    state.backend.write().unwrap().remove_marketplace(&name)
}

