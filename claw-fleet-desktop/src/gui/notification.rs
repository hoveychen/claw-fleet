use super::*;

// ── Notification mode ────────────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn get_notification_mode(state: tauri::State<AppState>) -> String {
    state.notification_mode.lock().unwrap().clone()
}

#[tauri::command]
pub(crate) fn set_notification_mode(mode: String, state: tauri::State<AppState>) {
    let valid = matches!(mode.as_str(), "all" | "user_action" | "none");
    if valid {
        *state.notification_mode.lock().unwrap() = mode;
    }
}

#[tauri::command]
pub(crate) fn get_user_title(state: tauri::State<AppState>) -> String {
    state.user_title.lock().unwrap().clone()
}

/// `(async)` for the same reason as `set_locale`: the reconcile below writes
/// launch-guidance files for three harnesses, and none of it belongs on the
/// event loop. Nothing here needs the main thread.
#[tauri::command(async)]
pub(crate) fn set_user_title(title: String, state: tauri::State<'_, AppState>) {
    *state.user_title.lock().unwrap() = title.clone();
    reconcile_launch_guidance(&state, &title, None);
}

/// Re-render the guidance Fleet hands its own sessions — claude's launch
/// voice, codex's launch guidance, dsh's plugin — with the title and locale the
/// frontend just pushed, so an app upgrade's new wording reaches the next
/// session. Which concepts are included comes from `control_plane_prefs`; none
/// of this touches `~/.claude`.
///
/// Called from the two frontend-driven startup syncs — `set_locale` (fires on
/// every App mount) and `set_user_title` — because they carry the real
/// title/locale, unlike `setup()` whose AppState still holds the `en` /
/// empty-title defaults.
pub(crate) fn reconcile_launch_guidance(
    state: &tauri::State<AppState>,
    title: &str,
    locale_override: Option<&str>,
) {
    let locale = match locale_override {
        Some(l) => l.to_string(),
        None => state.locale.lock().unwrap().clone(),
    };
    if let Err(e) = state.backend.reconcile_codex_guidance(title, &locale) {
        eprintln!("reconcile launch guidance failed: {e}");
    }
}

#[tauri::command]
pub(crate) fn open_notification_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.notifications")
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = claw_fleet_core::process_util::command("cmd")
            .args(["/C", "start", "ms-settings:notifications"])
            .spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Best-effort for Linux / other — most DEs don't have a unified URL.
        let _ = std::process::Command::new("xdg-open")
            .arg("settings://notifications")
            .spawn();
    }
}
