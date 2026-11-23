use super::*;

// ── Resource queue panel ─────────────────────────────────────────────────────
//
// Every call takes the queue's exclusive file lock (and `kick` waits for a
// holder to let go — up to ~6s), so all of them run on the blocking pool:
// never inline on the main thread, never parked on an async worker.

/// The whole table. Polled every ~1.5s while the panel is open.
#[tauri::command]
pub(crate) async fn resource_snapshot(
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::resource_queue::ResourceSnapshot, String> {
    let backend = state.backend.clone();
    super::blocking::run_blocking_result(move || backend.resource_snapshot()).await
}

#[tauri::command]
pub(crate) async fn resource_move(
    id: String,
    target: claw_fleet_core::resource_queue::MoveTarget,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::resource_queue::ResourceSnapshot, String> {
    let backend = state.backend.clone();
    super::blocking::run_blocking_result(move || backend.resource_move(id, target)).await
}

#[tauri::command]
pub(crate) async fn resource_cancel(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::resource_queue::ResourceSnapshot, String> {
    let backend = state.backend.clone();
    super::blocking::run_blocking_result(move || backend.resource_cancel(id)).await
}

#[tauri::command]
pub(crate) async fn resource_set_capacity(
    name: String,
    capacity: u32,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::resource_queue::ResourceSnapshot, String> {
    let backend = state.backend.clone();
    super::blocking::run_blocking_result(move || backend.resource_set_capacity(name, capacity)).await
}

#[tauri::command]
pub(crate) async fn resource_kick(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::resource_queue::ResourceSnapshot, String> {
    let backend = state.backend.clone();
    super::blocking::run_blocking_result(move || backend.resource_kick(id)).await
}

#[tauri::command]
pub(crate) async fn resource_remove(
    name: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::resource_queue::ResourceSnapshot, String> {
    let backend = state.backend.clone();
    super::blocking::run_blocking_result(move || backend.resource_remove(name)).await
}

/// The queue's event log, newest first. `query` may be omitted (newest page).
#[tauri::command]
pub(crate) async fn resource_history(
    query: Option<claw_fleet_core::resource_queue::HistoryQuery>,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::resource_queue::ResourceHistory, String> {
    let backend = state.backend.clone();
    super::blocking::run_blocking_result(move || backend.resource_history(query.unwrap_or_default()))
        .await
}
