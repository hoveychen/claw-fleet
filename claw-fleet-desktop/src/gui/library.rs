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

/// Full-text search over both stores, ranked; `workspace` scopes it to one
/// project.
#[tauri::command(async)]
pub(crate) fn library_search(
    query: String,
    workspace: Option<String>,
    kind: Option<String>,
    limit: Option<usize>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::library_index::LibraryHit>, String> {
    state
        .backend
        .library_search(&query, workspace, kind, limit.unwrap_or(0))
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

/// The user keeps a folder an AI created: clears its review marker.
#[tauri::command(async)]
pub(crate) fn library_accept_folder(
    workspace_path: String,
    path: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::library::LibraryFolder, String> {
    state.backend.library_accept_folder(&workspace_path, &path)
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

// ── Uploads ──────────────────────────────────────────────────────────────────
// Thin over `claw_fleet_core::library_ingest`; the browser build reaches the
// same functions through the `/library_ingest_*` routes (it uploads bytes, the
// desktop hands over local paths).

/// Upload dropped or picked local files and folders into a project. Returns
/// at once; the batch is filed in the background.
#[tauri::command(async)]
pub(crate) fn library_ingest_start_paths(
    workspace_path: String,
    paths: Vec<String>,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::library_ingest::IngestBatch, String> {
    state.backend.library_ingest_start_paths(&workspace_path, &paths)
}

#[tauri::command(async)]
pub(crate) fn library_ingest_batches(
    workspace: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::library_ingest::BatchSummary>, String> {
    state.backend.library_ingest_batches(workspace.as_deref())
}

#[tauri::command(async)]
pub(crate) fn library_ingest_batch(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::library_ingest::IngestBatch, String> {
    state.backend.library_ingest_batch(&id)
}

/// Undo one item, or the whole batch when `item_id` is absent.
#[tauri::command(async)]
pub(crate) fn library_ingest_undo(
    batch_id: String,
    item_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::library_ingest::IngestBatch, String> {
    state.backend.library_ingest_undo(&batch_id, item_id.as_deref())
}

#[tauri::command(async)]
pub(crate) fn library_ingest_move(
    batch_id: String,
    item_id: String,
    folder: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::library_ingest::IngestBatch, String> {
    state.backend.library_ingest_move(&batch_id, &item_id, &folder)
}

#[tauri::command(async)]
pub(crate) fn library_ingest_dismiss(
    batch_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.library_ingest_dismiss(&batch_id)
}
