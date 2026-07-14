use super::*;

// ── Permissions injector toggle ──────────────────────────────────────────────

#[tauri::command]
pub(crate) fn get_permissions_config() -> claw_fleet_core::permissions_injector::PermissionsConfig {
    claw_fleet_core::permissions_injector::load_config()
}

/// Persist the toggle and immediately apply it:
///   - flipping to `true`  → acquire(pid) right now
///   - flipping to `false` → deactivate() right now
///
/// This toggle is the *only* thing that un-injects settings.json. Quitting Fleet
/// does not, because the `claude` sessions Fleet spawned outlive the app and
/// would stall on permission prompts if the allow rules disappeared underneath
/// them. `deactivate` is unconditional even when a peer `fleet serve` still
/// holds the lock: the toggle is global, so every Fleet process's watchdog then
/// reads `enabled == false` and stops re-injecting.
#[tauri::command]
pub(crate) fn set_permissions_config(
    cfg: claw_fleet_core::permissions_injector::PermissionsConfig,
) -> Result<claw_fleet_core::permissions_injector::PermissionsConfig, String> {
    claw_fleet_core::permissions_injector::save_config(&cfg).map_err(|e| e.to_string())?;
    if cfg.enabled {
        claw_fleet_core::permissions_injector::acquire(std::process::id())
            .map_err(|e| e.to_string())?;
    } else {
        claw_fleet_core::permissions_injector::deactivate().map_err(|e| e.to_string())?;
    }
    Ok(cfg)
}

