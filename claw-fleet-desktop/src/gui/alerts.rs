use super::*;

// ── Waiting alerts ──────────────────────────────────────────────────────────

#[tauri::command(async)]
pub(crate) fn get_waiting_alerts(state: tauri::State<'_, AppState>) -> Vec<ui_types::WaitingAlert> {
    state.backend.get_waiting_alerts()
}

