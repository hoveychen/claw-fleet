use super::*;

// ── Locale ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn set_locale(app: tauri::AppHandle, locale: String, state: tauri::State<AppState>) {
    let prev = std::mem::replace(&mut *state.locale.lock().unwrap(), locale.clone());
    let title = state.user_title.lock().unwrap().clone();
    // Refresh every installed guidance carrier on this startup sync (set_locale
    // fires on every App mount), so wiki/model/codex pick up the latest bundled
    // template after an app upgrade instead of only when Settings is opened.
    reapply_all_guidance_if_installed(&state, &title, Some(&locale));
    // Rebuild the app menu only if the language prefix actually changed, so
    // we don't churn the native menu on every startup call.
    let prev_prefix = prev.get(..2).unwrap_or("");
    let next_prefix = locale.get(..2).unwrap_or("");
    if prev_prefix != next_prefix {
        install_app_menu(&app);
    }
}

