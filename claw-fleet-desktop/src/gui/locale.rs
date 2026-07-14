use super::*;

// ── Locale ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn set_locale(app: tauri::AppHandle, locale: String, state: tauri::State<AppState>) {
    let prev = std::mem::replace(&mut *state.locale.lock().unwrap(), locale.clone());
    let title = state.user_title.lock().unwrap().clone();
    reapply_interaction_mode_if_installed(&state, &title, Some(&locale));
    reapply_prd_mode_if_installed(&state, &title, Some(&locale));
    // Rebuild the app menu only if the language prefix actually changed, so
    // we don't churn the native menu on every startup call.
    let prev_prefix = prev.get(..2).unwrap_or("");
    let next_prefix = locale.get(..2).unwrap_or("");
    if prev_prefix != next_prefix {
        install_app_menu(&app);
    }
}

