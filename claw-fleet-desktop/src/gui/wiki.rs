use super::*;

// ── Wiki knowledge base ───────────────────────────────────────────────────────

#[tauri::command(async)]
pub(crate) fn list_wiki_docs(state: tauri::State<'_, AppState>) -> Vec<claw_fleet_core::wiki::WikiDoc> {
    state.backend.read().unwrap().list_wiki_docs()
}

/// Relay chain containing `session_id`, for the SessionCard handoff popover.
#[tauri::command(async)]
pub(crate) fn get_handoff_chain(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Option<claw_fleet_core::handoff::HandoffChain>, String> {
    state.backend.read().unwrap().get_handoff_chain(&session_id)
}

#[tauri::command(async)]
pub(crate) fn get_wiki_doc(
    slug: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::wiki::WikiDoc, String> {
    state.backend.read().unwrap().get_wiki_doc(&slug)
}

/// UTF-8 text of one wiki file (markdown preview path — HTML goes through the
/// `fleet-wiki://` protocol instead so relative assets resolve).
#[tauri::command(async)]
pub(crate) fn get_wiki_file_text(
    slug: String,
    version: String,
    relpath: String,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let f = state.backend.read().unwrap().get_wiki_file(&slug, &version, &relpath)?;
    String::from_utf8(f.bytes).map_err(|_| "file is not valid UTF-8".to_string())
}

/// Export one version of a wiki doc to `dest` on the local filesystem. Bytes
/// come through the backend, so remote docs download transparently; the save
/// dialog runs on the frontend (plugin-dialog), which hands us the path.
#[tauri::command(async)]
pub(crate) fn export_wiki_doc(
    slug: String,
    version: String,
    dest: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let export = state.backend.read().unwrap().export_wiki_doc(&slug, &version)?;
    std::fs::write(&dest, export.bytes).map_err(|e| format!("write '{dest}': {e}"))
}

#[tauri::command(async)]
pub(crate) fn search_wiki_docs(
    query: String,
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::wiki::WikiSearchHit> {
    state.backend.read().unwrap().search_wiki_docs(&query)
}

#[tauri::command]
pub(crate) fn delete_wiki_doc(slug: String, state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().delete_wiki_doc(&slug)
}

#[tauri::command]
pub(crate) fn delete_wiki_version(
    slug: String,
    version: String,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    state.backend.read().unwrap().delete_wiki_version(&slug, &version)
}

/// Re-key a doc — how the 知识库 board drags a doc into another folder.
#[tauri::command]
pub(crate) fn move_wiki_doc(
    from: String,
    to: String,
    state: tauri::State<AppState>,
) -> Result<claw_fleet_core::wiki::WikiDoc, String> {
    state.backend.read().unwrap().move_wiki_doc(&from, &to)
}

/// Rename a folder, or dissolve it into the tree root when `to` is empty.
#[tauri::command]
pub(crate) fn move_wiki_folder(
    from: String,
    to: String,
    state: tauri::State<AppState>,
) -> Result<Vec<claw_fleet_core::wiki::WikiDoc>, String> {
    state.backend.read().unwrap().move_wiki_folder(&from, &to)
}

/// Delete every doc under a folder. Returns how many were removed.
#[tauri::command]
pub(crate) fn delete_wiki_folder(prefix: String, state: tauri::State<AppState>) -> Result<usize, String> {
    state.backend.read().unwrap().delete_wiki_folder(&prefix)
}

