use super::*;

// ── Future-task commands (agent loops + one-shot schedules) ───────────────────
// Display + cancel for both; schedules additionally support edit (the desktop
// "编辑" (Edit) form). The project page creates either directly as a routine
// (`create_routine`); agents still create theirs with `fleet loop|schedule`.

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
pub(crate) fn cancel_loop(id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.cancel_loop(id)
}

#[tauri::command(async)]
pub(crate) fn cancel_schedule(id: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.cancel_schedule(id)
}

#[tauri::command(async)]
pub(crate) fn update_schedule(
    update: claw_fleet_core::schedule::ScheduleUpdate,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::schedule::ScheduleRecord, String> {
    state.backend.update_schedule(update)
}

#[tauri::command(async)]
pub(crate) fn create_routine(
    spec: claw_fleet_core::routines::RoutineSpec,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::routines::CreatedRoutine, String> {
    state.backend.create_routine(&spec)
}

#[tauri::command(async)]
pub(crate) fn list_projects(
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::project_registry::Project> {
    state.backend.list_projects()
}

#[tauri::command(async)]
pub(crate) fn set_project_archived(
    path: String,
    archived: bool,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::project_registry::Project>, String> {
    state.backend.set_project_archived(&path, archived)
}
