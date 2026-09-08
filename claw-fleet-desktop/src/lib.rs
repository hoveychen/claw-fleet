// Re-export everything from core so that `crate::session`, `crate::ui_types`,
// `crate::pattern_update`, etc. keep working in desktop-only modules that
// were written with `use crate::…` / `use super::…` paths.
pub use claw_fleet_core::*;

// ── Desktop-only modules ────────────────────────────────────────────────────
// These are always compiled — this crate IS the GUI app, so no #[cfg] gates.
pub mod app_nap;
mod cmd_probe;
pub mod fleet_binary;
mod gui;
pub mod keep_awake;
mod main_thread_probe;
pub mod local_backend;
pub mod rca_provision;
pub mod traffic_lights;
pub mod version_check;

pub use gui::*;

// ── Desktop-only backend helpers ────────────────────────────────────────────

use std::sync::Mutex;

/// Tracks which session file is currently being watched (tailed) for live
/// updates by `LocalBackend`.
pub struct WatchState {
    pub session: Mutex<Option<String>>,
    pub offset: Mutex<u64>,
}

impl WatchState {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(None),
            offset: Mutex::new(0),
        }
    }

    pub fn set(&self, path: String, offset: u64) {
        *self.session.lock().unwrap() = Some(path);
        *self.offset.lock().unwrap() = offset;
    }

    pub fn clear(&self) {
        *self.session.lock().unwrap() = None;
        *self.offset.lock().unwrap() = 0;
    }

    pub fn current_path(&self) -> Option<String> {
        self.session.lock().unwrap().clone()
    }
}

// ── Desktop-only pattern update helpers ─────────────────────────────────────
// These use `tauri::AppHandle` to resolve bundled resources, so they cannot
// live in the platform-agnostic core crate.

pub mod desktop_pattern_update {
    use crate::audit::{self, ExternalPatternsFile};

    /// Read the version from the bundled resource file.  Returns 0 if absent.
    fn bundled_version(app_handle: &tauri::AppHandle) -> u32 {
        use tauri::Manager;
        app_handle
            .path()
            .resolve(
                "resources/audit-patterns.json",
                tauri::path::BaseDirectory::Resource,
            )
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<ExternalPatternsFile>(&s).ok())
            .map(|f| f.version)
            .unwrap_or(0)
    }

    /// On first run (no local file), copy the bundled resource to
    /// `~/.fleet/fleet-audit-patterns.json` so the audit module has something
    /// to load without waiting for the first remote check.
    ///
    /// Also upgrades the local file if the bundled version is newer (happens
    /// after an app upgrade ships new built-in patterns).
    pub fn bootstrap_patterns(app_handle: &tauri::AppHandle) {
        use tauri::Manager;
        let Some(local) = crate::session::real_home_dir()
            .map(|h| h.join(".fleet").join("fleet-audit-patterns.json"))
        else {
            return;
        };
        if local.exists() {
            // Already have a local file.  Check if the bundled version is newer.
            let lv = local_version(&local);
            let bv = bundled_version(app_handle);
            if bv > lv {
                if let Ok(bundled_path) = app_handle
                    .path()
                    .resolve(
                        "resources/audit-patterns.json",
                        tauri::path::BaseDirectory::Resource,
                    )
                {
                    if let Ok(content) = std::fs::read_to_string(&bundled_path) {
                        let _ = atomic_write(&local, &content);
                        audit::reload_patterns();
                        crate::log_debug(&format!(
                            "pattern_update: upgraded local patterns v{lv} → v{bv} from bundled resource"
                        ));
                    }
                }
            }
            return;
        }
        // No local file — seed from bundled resource.
        if let Ok(bundled_path) = app_handle
            .path()
            .resolve(
                "resources/audit-patterns.json",
                tauri::path::BaseDirectory::Resource,
            )
        {
            if let Ok(content) = std::fs::read_to_string(&bundled_path) {
                let _ = atomic_write(&local, &content);
                crate::log_debug(
                    "pattern_update: seeded local patterns from bundled resource",
                );
            }
        }
    }

    /// Read the version from a local JSON file.  Returns 0 if absent / unparseable.
    fn local_version(path: &std::path::Path) -> u32 {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<ExternalPatternsFile>(&s).ok())
            .map(|f| f.version)
            .unwrap_or(0)
    }

    /// Atomic file write: write to temp, then rename.
    fn atomic_write(target: &std::path::Path, content: &str) -> Result<(), String> {
        let dir = target.parent().ok_or("no parent dir")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir: {e}"))?;
        let tmp = dir.join(format!(
            ".fleet-audit-patterns-{}.tmp",
            std::process::id()
        ));
        std::fs::write(&tmp, content).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("write tmp: {e}")
        })?;
        std::fs::rename(&tmp, target).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("rename: {e}")
        })?;
        Ok(())
    }
}
