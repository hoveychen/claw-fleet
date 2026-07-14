use super::*;
use crate::plugins;

// ── Plugins ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn list_plugins(state: tauri::State<AppState>) -> Vec<plugins::PluginItem> {
    state.backend.read().unwrap().list_plugins()
}

#[tauri::command]
pub(crate) fn set_plugin_enabled(
    state: tauri::State<AppState>,
    plugin_id: String,
    enabled: bool,
) -> Result<(), String> {
    state
        .backend
        .read()
        .unwrap()
        .set_plugin_enabled(&plugin_id, enabled)
}

#[tauri::command]
pub(crate) fn install_plugin(state: tauri::State<AppState>, plugin_id: String) -> Result<(), String> {
    state.backend.read().unwrap().install_plugin(&plugin_id)
}

#[tauri::command]
pub(crate) fn uninstall_plugin(state: tauri::State<AppState>, plugin_id: String) -> Result<(), String> {
    state.backend.read().unwrap().uninstall_plugin(&plugin_id)
}

#[tauri::command]
pub(crate) fn list_marketplaces(
    state: tauri::State<AppState>,
) -> Vec<claw_fleet_core::claude_cli::CliMarketplace> {
    state.backend.read().unwrap().list_marketplaces()
}

#[tauri::command]
pub(crate) fn add_marketplace(state: tauri::State<AppState>, source: String) -> Result<(), String> {
    state.backend.read().unwrap().add_marketplace(&source)
}

#[tauri::command]
pub(crate) fn remove_marketplace(state: tauri::State<AppState>, name: String) -> Result<(), String> {
    state.backend.read().unwrap().remove_marketplace(&name)
}

