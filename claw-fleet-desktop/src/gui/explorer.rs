use super::*;

// ── File explorer ─────────────────────────────────────────────────────────────

// ── User-added browse paths ───────────────────────────────────────────────────
//
// The 仓库 page's hand-added / just-cloned cards. These live on the backend
// (see `claw_fleet_core::browse_paths`) rather than in the front-end store:
// they widen what the explorer will read, and the UI store is neither
// persistent nor authoritative for a remote host.

#[tauri::command(async)]
pub(crate) fn list_browse_paths(state: tauri::State<'_, AppState>) -> Vec<String> {
    state.backend.read().unwrap().list_browse_paths()
}

#[tauri::command(async)]
pub(crate) fn add_browse_path(
    path: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    state.backend.read().unwrap().add_browse_path(&path)
}

#[tauri::command(async)]
pub(crate) fn remove_browse_path(
    path: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    state.backend.read().unwrap().remove_browse_path(&path)
}

#[tauri::command(async)]
pub(crate) fn list_explorer_roots(
    workspace: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::file_explorer::ExplorerRoot>, String> {
    state.backend.read().unwrap().list_explorer_roots(&workspace)
}

#[tauri::command(async)]
pub(crate) fn list_explorer_dir(
    workspace: String,
    root: String,
    rel_path: String,
    show_ignored: bool,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::file_explorer::ExplorerEntry>, String> {
    state
        .backend
        .read()
        .unwrap()
        .list_explorer_dir(&workspace, &root, &rel_path, show_ignored)
}

#[tauri::command(async)]
pub(crate) fn read_explorer_file(
    workspace: String,
    root: String,
    rel_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::file_explorer::ExplorerFileContent, String> {
    state
        .backend
        .read()
        .unwrap()
        .read_explorer_file(&workspace, &root, &rel_path)
}

