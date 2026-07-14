use super::*;
use crate::hooks;

// ── Hooks setup ──────────────────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn get_hooks_setup_plan(state: tauri::State<AppState>) -> hooks::HookSetupPlan {
    state.backend.read().unwrap().get_hooks_plan()
}

#[tauri::command]
pub(crate) fn apply_hooks_setup(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().apply_hooks()
}

#[tauri::command]
pub(crate) fn remove_hooks(state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().remove_hooks()
}

