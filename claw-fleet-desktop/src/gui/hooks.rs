use super::*;
use crate::hooks;

// ── Hooks setup ──────────────────────────────────────────────────────────────

#[tauri::command(async)]
pub(crate) fn get_hooks_setup_plan(state: tauri::State<'_, AppState>) -> hooks::HookSetupPlan {
    state.backend.read().unwrap().get_hooks_plan()
}

#[tauri::command(async)]
pub(crate) fn apply_hooks_setup(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.write().unwrap().apply_hooks()
}

#[tauri::command(async)]
pub(crate) fn remove_hooks(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.write().unwrap().remove_hooks()
}

