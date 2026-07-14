use super::*;

// ── Session scratchpad ──────────────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn list_scratchpad_dir(
    workspace: String,
    session_id: String,
    rel_path: String,
    state: tauri::State<AppState>,
) -> Result<Vec<claw_fleet_core::file_explorer::ExplorerEntry>, String> {
    state
        .backend
        .read()
        .unwrap()
        .list_scratchpad_dir(&workspace, &session_id, &rel_path)
}

#[tauri::command]
pub(crate) fn read_scratchpad_file(
    workspace: String,
    session_id: String,
    rel_path: String,
    state: tauri::State<AppState>,
) -> Result<claw_fleet_core::file_explorer::ExplorerFileContent, String> {
    state
        .backend
        .read()
        .unwrap()
        .read_scratchpad_file(&workspace, &session_id, &rel_path)
}

