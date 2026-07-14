use super::*;

// ── File explorer ─────────────────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn list_explorer_roots(
    workspace: String,
    state: tauri::State<AppState>,
) -> Result<Vec<claw_fleet_core::file_explorer::ExplorerRoot>, String> {
    state.backend.read().unwrap().list_explorer_roots(&workspace)
}

#[tauri::command]
pub(crate) fn list_explorer_dir(
    workspace: String,
    root: String,
    rel_path: String,
    show_ignored: bool,
    state: tauri::State<AppState>,
) -> Result<Vec<claw_fleet_core::file_explorer::ExplorerEntry>, String> {
    state
        .backend
        .read()
        .unwrap()
        .list_explorer_dir(&workspace, &root, &rel_path, show_ignored)
}

#[tauri::command]
pub(crate) fn read_explorer_file(
    workspace: String,
    root: String,
    rel_path: String,
    state: tauri::State<AppState>,
) -> Result<claw_fleet_core::file_explorer::ExplorerFileContent, String> {
    state
        .backend
        .read()
        .unwrap()
        .read_explorer_file(&workspace, &root, &rel_path)
}

