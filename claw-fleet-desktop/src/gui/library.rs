//! 资料 (Library): the per-project folder schema and the merged wiki + artifact
//! tree. Thin commands over `claw_fleet_core::library`; the same calls back the
//! `/library_*` routes of `fleet serve` for the browser build.

use super::*;

#[tauri::command(async)]
pub(crate) fn library_tree(
    workspace: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::library::LibraryProject>, String> {
    state.backend.library_tree(workspace.as_deref())
}

#[tauri::command(async)]
pub(crate) fn library_create_folder(
    workspace_path: String,
    path: String,
    description: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::library::LibraryFolder, String> {
    state
        .backend
        .library_create_folder(&workspace_path, &path, description.as_deref())
}

#[tauri::command(async)]
pub(crate) fn library_describe_folder(
    workspace_path: String,
    path: String,
    description: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::library::LibraryFolder, String> {
    state
        .backend
        .library_describe_folder(&workspace_path, &path, &description)
}

/// Refused by core while any doc or artifact is still filed inside.
#[tauri::command(async)]
pub(crate) fn library_delete_folder(
    workspace_path: String,
    path: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.library_delete_folder(&workspace_path, &path)
}

#[tauri::command(async)]
pub(crate) fn library_rename_folder(
    workspace_path: String,
    from: String,
    to: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::library::FolderRename, String> {
    state
        .backend
        .library_rename_folder(&workspace_path, &from, &to)
}

/// File a doc or artifact into a schema folder; `folder: ""` unfiles it.
#[tauri::command(async)]
pub(crate) fn library_move_item(
    kind: String,
    id: String,
    folder: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::library::LibraryItem, String> {
    state.backend.library_move_item(&kind, &id, &folder)
}
