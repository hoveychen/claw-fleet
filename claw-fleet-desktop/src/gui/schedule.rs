use super::*;

// ── Future-task commands (agent loops + one-shot schedules) ───────────────────
// Display + cancel only; create/update stay on the CLI (`fleet loop|schedule`)
// so an agent adjusts plans, while the desktop panel shows and cancels them.
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
