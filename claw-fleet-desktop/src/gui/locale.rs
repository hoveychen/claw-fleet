use super::*;

// ── Locale ──────────────────────────────────────────────────────────────────

/// `(async)`, with only the menu rebuild hopped back to the main thread.
///
/// This was a plain sync command, i.e. inlined on the event loop — and its
/// body is not cheap: `reapply_all_guidance_if_installed` runs six carriers,
/// each reading the hooks plan (`~/.claude/settings.json`) and rewriting its
/// guidance file. That is a dozen file operations, and because the frontend
/// calls `set_locale` on **every App mount** they all land during boot, in the
/// same seconds the user is first clicking around. A blocked event loop does
/// not just delay this command: it holds up the delivery of every other
/// invoke's answer and any native panel (wiki `desktop/ipc-stall-forensics`).
///
/// The guidance writers are whole-file rewrites rather than read-modify-write
/// accumulations, so losing the main thread's implicit serialization cannot
/// corrupt one — two racing callers leave one of the two versions, which is
/// also what two racing sync callers would have left.
///
/// `install_app_menu` is the one part that must stay on the main thread (it
/// builds a native menu), so it is dispatched there explicitly.
#[tauri::command(async)]
pub(crate) fn set_locale(
    app: tauri::AppHandle,
    locale: String,
    state: tauri::State<'_, AppState>,
) {
    let prev = std::mem::replace(&mut *state.locale.lock().unwrap(), locale.clone());
    let title = state.user_title.lock().unwrap().clone();
    // Refresh every installed guidance carrier on this startup sync, so
    // wiki/model/codex pick up the latest bundled template after an app upgrade
    // instead of only when Settings is opened.
    reapply_all_guidance_if_installed(&state, &title, Some(&locale));
    // Rebuild the app menu only if the language prefix actually changed, so
    // we don't churn the native menu on every startup call.
    let prev_prefix = prev.get(..2).unwrap_or("");
    let next_prefix = locale.get(..2).unwrap_or("");
    if prev_prefix != next_prefix {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || install_app_menu(&handle));
    }
}

