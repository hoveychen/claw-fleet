use super::*;

// ── Waiting alerts ──────────────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn get_waiting_alerts(state: tauri::State<AppState>) -> Vec<backend::WaitingAlert> {
    state.backend.read().unwrap().get_waiting_alerts()
}

