use super::*;

// ── File explorer ─────────────────────────────────────────────────────────────

// ── User-added browse paths ───────────────────────────────────────────────────
//
// The explorer page's hand-added / just-cloned cards. These live on the backend
// (see `claw_fleet_core::browse_paths`) rather than in the front-end store:
// they widen what the explorer will read, and the UI store is neither
// persistent nor authoritative for a remote host.

#[tauri::command(async)]
pub(crate) fn list_browse_paths(state: tauri::State<'_, AppState>) -> Vec<String> {
    state.backend.list_browse_paths()
}

#[tauri::command(async)]
pub(crate) fn add_browse_path(
    path: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    state.backend.add_browse_path(&path)
}

#[tauri::command(async)]
pub(crate) fn remove_browse_path(
    path: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    state.backend.remove_browse_path(&path)
}

#[tauri::command(async)]
pub(crate) fn list_explorer_roots(
    workspace: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::file_explorer::ExplorerRoot>, String> {
    state.backend.list_explorer_roots(&workspace)
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
        .read_explorer_file(&workspace, &root, &rel_path)
}

/// Locate a file by the tail of its path. The explorer page's fallback when the
/// literal path a chip resolved to reveals nothing.
#[tauri::command(async)]
pub(crate) fn find_explorer_path(
    workspace: String,
    root: String,
    rel_suffix: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, String> {
    state
        .backend
        .find_explorer_path(&workspace, &root, &rel_suffix)
}

/// Whole file as base64 + mime, for the PDF / Office previews the webview
/// renders itself. Refuses files over 30 MiB.
#[tauri::command(async)]
pub(crate) fn read_explorer_bytes(
    workspace: String,
    root: String,
    rel_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::file_explorer::ExplorerBytes, String> {
    state
        .backend
        .read_explorer_bytes(&workspace, &root, &rel_path)
}

/// Open an explorer file with the system default application. Desktop-only
/// shell action (no `fleet serve` route: a browser tab must not open apps on
/// the host); the path goes through the same gate as `read_explorer_file`, so
/// it cannot escape the root. Same shape as `open_artifact_external`.
#[tauri::command(async)]
pub(crate) fn open_explorer_file(
    app: tauri::AppHandle,
    workspace: String,
    root: String,
    rel_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let probe = crate::cmd_probe::CmdProbe::start_watched("open_explorer_file", &rel_path);
    let result = (|| {
        let path = state
            .backend
            .resolve_explorer_file(&workspace, &root, &rel_path)?;
        if !path.exists() {
            return Err(format!("file no longer exists: {}", path.display()));
        }
        app.opener()
            .open_path(path.to_string_lossy(), None::<&str>)
            .map_err(|e| e.to_string())
    })();
    probe.done(|| match &result {
        Ok(()) => "ok".to_string(),
        Err(e) => e.clone(),
    });
    result
}

/// Find files and directories in a project by name (case-insensitive
/// substring, all whitespace-separated terms must match). Backs the Files
/// page's search box.
#[tauri::command(async)]
pub(crate) fn search_explorer_files(
    workspace: String,
    root: String,
    query: String,
    show_ignored: bool,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::file_explorer::ExplorerEntry>, String> {
    state
        .backend
        .search_explorer_files(&workspace, &root, &query, show_ignored)
}

/// Preview a path that belongs to no workspace — what a `/tmp/foo.md` chip in
/// agent prose resolves to. Single file, no listing.
#[tauri::command(async)]
pub(crate) fn read_external_file(
    path: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::file_explorer::ExplorerFileContent, String> {
    state.backend.read_external_file(&path)
}

/// Turn a path as an agent wrote it into one that exists, if any reading of it
/// does. The webview cannot do this itself — resolving needs `stat`, and the
/// chip's join is computed during render — so the click asks here first.
#[tauri::command(async)]
pub(crate) fn resolve_prose_path(
    workspace: String,
    path: String,
    state: tauri::State<'_, AppState>,
) -> claw_fleet_core::file_explorer::PathResolution {
    state.backend.resolve_prose_path(&workspace, &path)
}
