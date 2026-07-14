use super::*;

// ── Workspace command runner (文件 page) ─────────────────────────────────────

#[tauri::command]
pub(crate) fn list_workspace_procs(
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::proc_runner::ProcRecord> {
    state.backend.read().unwrap().list_procs()
}

#[tauri::command]
pub(crate) fn run_workspace_proc(
    workspace_path: String,
    command: String,
    cols: u16,
    rows: u16,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::proc_runner::ProcRecord, String> {
    state
        .backend
        .read()
        .unwrap()
        .spawn_proc(workspace_path, command, cols, rows)
}

#[tauri::command]
pub(crate) fn kill_workspace_proc(
    id: String,
    force: bool,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.read().unwrap().kill_proc(id, force)
}

#[tauri::command]
pub(crate) fn read_workspace_proc_output(
    id: String,
    offset: Option<u64>,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::proc_runner::ProcOutputChunk, String> {
    state.backend.read().unwrap().proc_output(id, offset)
}

#[tauri::command]
pub(crate) fn write_workspace_proc_input(
    id: String,
    data_b64: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.read().unwrap().proc_input(id, data_b64)
}

#[tauri::command]
pub(crate) fn resize_workspace_proc(
    id: String,
    cols: u16,
    rows: u16,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.read().unwrap().proc_resize(id, cols, rows)
}

#[tauri::command]
pub(crate) fn clear_workspace_procs(
    id: Option<String>,
    workspace_path: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<u32, String> {
    state.backend.read().unwrap().clear_procs(id, workspace_path)
}

