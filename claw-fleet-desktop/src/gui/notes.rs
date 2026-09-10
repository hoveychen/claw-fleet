use super::*;

// ── Session notes (read-only) ───────────────────────────────────────────────────
//
// Writes stay with the agent (`fleet__notes` / `fleet notes`): the notes are an
// account of what a run knew at each checkpoint, and a reader that could edit
// them would be rewriting that account.

#[tauri::command(async)]
pub(crate) fn list_session_notes(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::session_notes::NoteFile>, String> {
    state.backend.list_session_notes(&session_id)
}

/// `session_id` is the note's **owner** (the `sessionId` on the listed entry),
/// which differs from the session on screen for every inherited note.
#[tauri::command(async)]
pub(crate) fn read_session_note(
    session_id: String,
    path: String,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    state.backend.read_session_note(&session_id, &path)
}
