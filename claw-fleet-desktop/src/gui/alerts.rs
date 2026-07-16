use super::*;

// ── Waiting alerts ──────────────────────────────────────────────────────────

#[tauri::command(async)]
pub(crate) fn get_waiting_alerts(state: tauri::State<'_, AppState>) -> Vec<backend::WaitingAlert> {
    state.backend.read().unwrap().get_waiting_alerts()
}

