use super::*;

// ── Future-task commands (agent loops + one-shot schedules) ───────────────────
// Display + cancel for both; schedules additionally support edit (the desktop
// "编辑" form). Creation stays agent-driven on the CLI (`fleet loop|schedule`).
// All delegate through the Backend trait so remote workspaces work identically.

#[tauri::command(async)]
pub(crate) fn list_loops(
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::agent_loop::LoopRecord> {
    state.backend.read().unwrap().list_loops()
}

#[tauri::command(async)]
pub(crate) fn list_schedules(
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::schedule::ScheduleRecord> {
    state.backend.read().unwrap().list_schedules()
}

#[tauri::command(async)]
pub(crate) fn cancel_loop(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.read().unwrap().cancel_loop(id)
}

#[tauri::command(async)]
pub(crate) fn cancel_schedule(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.read().unwrap().cancel_schedule(id)
}

#[tauri::command(async)]
pub(crate) fn update_schedule(
    update: claw_fleet_core::schedule::ScheduleUpdate,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::schedule::ScheduleRecord, String> {
    state.backend.read().unwrap().update_schedule(update)
}
