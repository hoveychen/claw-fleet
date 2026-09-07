use super::*;

// ── Future-task commands (agent loops + one-shot schedules) ───────────────────
// Display + cancel for both; schedules additionally support edit (the desktop
// "编辑" form). Creation stays agent-driven on the CLI (`fleet loop|schedule`).

#[tauri::command(async)]
pub(crate) fn list_loops(
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::agent_loop::LoopRecord> {
    state.backend.list_loops()
}

#[tauri::command(async)]
pub(crate) fn list_schedules(
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::schedule::ScheduleRecord> {
    state.backend.list_schedules()
}

#[tauri::command(async)]
pub(crate) fn cancel_loop(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.cancel_loop(id)
}

#[tauri::command(async)]
pub(crate) fn cancel_schedule(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.cancel_schedule(id)
}

#[tauri::command(async)]
pub(crate) fn update_schedule(
    update: claw_fleet_core::schedule::ScheduleUpdate,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::schedule::ScheduleRecord, String> {
    state.backend.update_schedule(update)
}
