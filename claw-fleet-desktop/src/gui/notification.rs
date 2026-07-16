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
    reapply_all_guidance_if_installed(&state, &title, None);
}

/// Re-sync every Fleet-managed guidance carrier that is currently installed, so
/// their `~/.claude` / `~/.codex` files pick up the latest bundled template
/// after an app upgrade. Idempotent: each concept only rewrites when it's
/// already installed on disk, and codex reconciles against the Claude sentinels
/// (a no-op when nothing is installed).
///
/// Called from the two frontend-driven startup syncs — `set_locale` (fires on
/// every App mount) and `set_user_title` — so the refresh happens at app
/// startup, not only when the Settings panel is opened. Both carry the real
/// title/locale the frontend just pushed, unlike `setup()` whose AppState still
/// holds the `en` / empty-title defaults.
pub(crate) fn reapply_all_guidance_if_installed(
    state: &tauri::State<AppState>,
    title_override: &str,
    locale_override: Option<&str>,
) {
    reapply_interaction_mode_if_installed(state, title_override, locale_override);
    reapply_prd_mode_if_installed(state, title_override, locale_override);
    reapply_wiki_guidance_if_installed(state, locale_override);
    reapply_model_guidance_if_installed(state, locale_override);
    reapply_codex_guidance(state, title_override, locale_override);
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

/// Wiki guidance is locale-only (no title). Re-sync it when installed.
pub(crate) fn reapply_wiki_guidance_if_installed(
    state: &tauri::State<AppState>,
    locale_override: Option<&str>,
) {
    let backend = state.backend.read().unwrap();
    if !backend.get_hooks_plan().wiki_guidance_installed {
        return;
    }
    let locale = match locale_override {
        Some(l) => l.to_string(),
        None => state.locale.lock().unwrap().clone(),
    };
    if let Err(e) = backend.apply_wiki_guidance(&locale) {
        eprintln!("re-apply wiki guidance failed: {e}");
    }
}

/// Model guidance is locale-only (no title). Re-sync it when installed.
pub(crate) fn reapply_model_guidance_if_installed(
    state: &tauri::State<AppState>,
    locale_override: Option<&str>,
) {
    let backend = state.backend.read().unwrap();
    if !backend.get_hooks_plan().model_guidance_installed {
        return;
    }
    let locale = match locale_override {
        Some(l) => l.to_string(),
        None => state.locale.lock().unwrap().clone(),
    };
    if let Err(e) = backend.apply_model_guidance(&locale) {
        eprintln!("re-apply model guidance failed: {e}");
    }
}

/// Mirror the Claude concept toggles onto codex's `AGENTS.md`. Self-gating: it
/// reads which concepts are installed from the Claude sentinels and reconciles
/// the matching codex blocks (removing `AGENTS.md` when nothing is installed),
/// so it's safe to call unconditionally — no `_if_installed` guard needed.
pub(crate) fn reapply_codex_guidance(
    state: &tauri::State<AppState>,
    title_override: &str,
    locale_override: Option<&str>,
) {
    let locale = match locale_override {
        Some(l) => l.to_string(),
        None => state.locale.lock().unwrap().clone(),
    };
    if let Err(e) = state
        .backend
        .read()
        .unwrap()
        .reconcile_codex_guidance(title_override, &locale)
    {
        eprintln!("re-apply codex guidance failed: {e}");
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

