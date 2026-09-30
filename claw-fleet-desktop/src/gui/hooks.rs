use super::*;
use crate::hooks;

// ── Hooks setup ──────────────────────────────────────────────────────────────

#[tauri::command(async)]
pub(crate) fn get_hooks_setup_plan(state: tauri::State<'_, AppState>) -> hooks::HookSetupPlan {
    state.backend.get_hooks_plan()
}
