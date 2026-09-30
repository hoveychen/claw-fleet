use super::*;

// ── Locale ──────────────────────────────────────────────────────────────────

/// `(async)`, with only the menu rebuild hopped back to the main thread.
///
/// This was a plain sync command, i.e. inlined on the event loop — and its
/// body is not cheap: `reconcile_launch_guidance` re-renders the launch
/// guidance of three harnesses. That is several file operations, and because the frontend
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
pub(crate) fn set_locale(app: tauri::AppHandle, locale: String, state: tauri::State<'_, AppState>) {
    let prev = std::mem::replace(&mut *state.locale.lock().unwrap(), locale.clone());
    let title = state.user_title.lock().unwrap().clone();
    // Re-render the launch guidance on this startup sync, so the next Fleet
    // session picks up the latest bundled template after an app upgrade.
    reconcile_launch_guidance(&state, &title, Some(&locale));
    // Rebuild the app menu only if the language prefix actually changed, so
    // we don't churn the native menu on every startup call.
    let prev_prefix = prev.get(..2).unwrap_or("");
    let next_prefix = locale.get(..2).unwrap_or("");
    if prev_prefix != next_prefix {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || install_app_menu(&handle));
    }
}
