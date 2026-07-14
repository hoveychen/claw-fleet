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

#[tauri::command]
pub(crate) fn set_user_title(title: String, state: tauri::State<AppState>) {
    *state.user_title.lock().unwrap() = title.clone();
    reapply_interaction_mode_if_installed(&state, &title, None);
    reapply_prd_mode_if_installed(&state, &title, None);
}

/// If the interaction-mode guidance is currently installed, regenerate it with
/// fresh title/locale values. Silent on failure — it's a convenience re-sync.
pub(crate) fn reapply_interaction_mode_if_installed(
    state: &tauri::State<AppState>,
    title_override: &str,
    locale_override: Option<&str>,
) {
    let backend = state.backend.read().unwrap();
    let plan = backend.get_hooks_plan();
    if !plan.interaction_mode_installed {
        return;
    }
    let locale = match locale_override {
        Some(l) => l.to_string(),
        None => state.locale.lock().unwrap().clone(),
    };
    if let Err(e) = backend.apply_interaction_mode(title_override, &locale) {
        eprintln!("re-apply interaction mode failed: {e}");
    }
}

pub(crate) fn reapply_prd_mode_if_installed(
    state: &tauri::State<AppState>,
    title_override: &str,
    locale_override: Option<&str>,
) {
    let backend = state.backend.read().unwrap();
    let plan = backend.get_hooks_plan();
    if !plan.prd_discipline_installed {
        return;
    }
    let locale = match locale_override {
        Some(l) => l.to_string(),
        None => state.locale.lock().unwrap().clone(),
    };
    if let Err(e) = backend.apply_prd_mode(title_override, &locale) {
        eprintln!("re-apply prd mode failed: {e}");
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

