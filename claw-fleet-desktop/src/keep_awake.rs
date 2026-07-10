//! System keep-awake toggle — the in-process equivalent of `caffeinate -i`.
//!
//! Prevents *idle system sleep* on the machine running the desktop app while
//! the user has the toggle on (long local sessions keep streaming overnight).
//! The display may still sleep, and closing a laptop lid still sleeps the
//! machine — matching `caffeinate -i` semantics exactly. Lid-close sleep can
//! only be blocked by `pmset disablesleep`, which needs root; we deliberately
//! don't go there.
//!
//! Platform notes:
//! - macOS: an IOKit power assertion (`PreventUserIdleSystemSleep`). Works
//!   inside the App Sandbox with no extra entitlement, no TCC prompt, and no
//!   helper daemon — `caffeinate` itself is just a CLI wrapper around this
//!   API. The assertion dies with the process, so there is nothing to clean
//!   up on exit. This complements `app_nap.rs`, whose NSActivity explicitly
//!   *allows* idle system sleep.
//! - Windows: `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`.
//!   The flag is per-thread, so a dedicated keeper thread owns it; the state
//!   likewise clears automatically when the process exits.
//! - Linux: unsupported for now (would need a D-Bus logind/ScreenSaver
//!   inhibitor); the UI hides the toggle when `is_supported()` is false.
//!
//! The enabled flag persists to `~/.fleet/keep-awake.json` so the choice
//! survives app restarts; `restore_at_startup()` re-acquires it on launch.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct KeepAwakeConfig {
    enabled: bool,
}

fn config_path() -> Option<std::path::PathBuf> {
    claw_fleet_core::session::real_home_dir().map(|h| h.join(".fleet").join("keep-awake.json"))
}

fn load_config() -> KeepAwakeConfig {
    let Some(path) = config_path() else {
        return KeepAwakeConfig::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(cfg: &KeepAwakeConfig) -> Result<(), String> {
    let path = config_path().ok_or_else(|| "cannot resolve home directory".to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(path, body).map_err(|e| e.to_string())
}

/// Whether this platform can hold a keep-awake assertion at all. The UI
/// hides the toggle when false.
pub fn is_supported() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

/// The persisted toggle state (what the user last chose).
pub fn is_enabled() -> bool {
    load_config().enabled
}

/// Flip the toggle: acquire/release the OS assertion, then persist. Returns
/// the effective state. On unsupported platforms this is an error so the
/// caller can surface it instead of silently pretending.
pub fn set_enabled(enabled: bool) -> Result<bool, String> {
    if !is_supported() {
        return Err("keep-awake is not supported on this platform".to_string());
    }
    platform::apply(enabled)?;
    save_config(&KeepAwakeConfig { enabled })?;
    claw_fleet_core::log_debug(&format!(
        "[keep_awake] {} system idle-sleep prevention",
        if enabled { "acquired" } else { "released" }
    ));
    Ok(enabled)
}

/// Re-acquire the assertion on app launch when the user left the toggle on.
/// Failure is logged, not fatal — the toggle in the UI still reflects the
/// persisted intent and the user can flip it to retry.
pub fn restore_at_startup() {
    if is_supported() && is_enabled() {
        if let Err(e) = platform::apply(true) {
            claw_fleet_core::log_debug(&format!("[keep_awake] startup restore failed: {e}"));
        } else {
            claw_fleet_core::log_debug("[keep_awake] restored idle-sleep prevention from config");
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2_foundation::NSString;
    use std::ffi::c_void;
    use std::sync::Mutex;

    type IOPMAssertionID = u32;
    const K_IOPM_ASSERTION_LEVEL_ON: u32 = 255;

    /// Name this process registers its assertion under. `pmset -g assertions`
    /// reports it verbatim, so the test greps for it.
    pub const ASSERTION_NAME: &str = "Claw Fleet keep-awake toggle";

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        // CFStringRef parameters; NSString is toll-free bridged to CFString,
        // so we pass `*const NSString` casted to `*const c_void`.
        fn IOPMAssertionCreateWithName(
            assertion_type: *const c_void,
            assertion_level: u32,
            assertion_name: *const c_void,
            assertion_id: *mut IOPMAssertionID,
        ) -> i32;
        fn IOPMAssertionRelease(assertion_id: IOPMAssertionID) -> i32;
    }

    static ASSERTION: Mutex<Option<IOPMAssertionID>> = Mutex::new(None);

    pub fn apply(enabled: bool) -> Result<(), String> {
        let mut slot = ASSERTION.lock().unwrap();
        if enabled {
            if slot.is_some() {
                return Ok(()); // already held — idempotent
            }
            // Same assertion type `caffeinate -i` creates.
            let assertion_type = NSString::from_str("PreventUserIdleSystemSleep");
            let name = NSString::from_str(ASSERTION_NAME);
            let mut id: IOPMAssertionID = 0;
            let ret = unsafe {
                IOPMAssertionCreateWithName(
                    &*assertion_type as *const NSString as *const c_void,
                    K_IOPM_ASSERTION_LEVEL_ON,
                    &*name as *const NSString as *const c_void,
                    &mut id,
                )
            };
            if ret != 0 {
                return Err(format!("IOPMAssertionCreateWithName failed: {ret:#x}"));
            }
            *slot = Some(id);
        } else if let Some(id) = slot.take() {
            unsafe { IOPMAssertionRelease(id) };
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::sync::mpsc::{channel, Sender};
    use std::sync::{Mutex, OnceLock};

    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetThreadExecutionState(es_flags: u32) -> u32;
    }

    // ES_CONTINUOUS is per-thread state, so a single long-lived keeper thread
    // owns the flag; toggles are marshalled to it over a channel.
    static KEEPER: OnceLock<Mutex<Sender<bool>>> = OnceLock::new();

    pub fn apply(enabled: bool) -> Result<(), String> {
        let keeper = KEEPER.get_or_init(|| {
            let (tx, rx) = channel::<bool>();
            std::thread::Builder::new()
                .name("keep-awake".into())
                .spawn(move || {
                    while let Ok(on) = rx.recv() {
                        let flags = if on {
                            ES_CONTINUOUS | ES_SYSTEM_REQUIRED
                        } else {
                            ES_CONTINUOUS
                        };
                        unsafe { SetThreadExecutionState(flags) };
                    }
                })
                .expect("spawn keep-awake keeper thread");
            Mutex::new(tx)
        });
        keeper
            .lock()
            .unwrap()
            .send(enabled)
            .map_err(|e| format!("keep-awake keeper thread gone: {e}"))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    pub fn apply(_enabled: bool) -> Result<(), String> {
        Err("keep-awake is not supported on this platform".to_string())
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    // End-to-end check against the real power-management daemon: acquire the
    // assertion, confirm `pmset -g assertions` lists it under our name, then
    // release and confirm it's gone. Calls platform::apply directly so the
    // test never touches the user's ~/.fleet/keep-awake.json.
    #[test]
    fn assertion_visible_in_pmset() {
        use super::platform::ASSERTION_NAME;

        fn pmset_assertions() -> String {
            let out = std::process::Command::new("pmset")
                .args(["-g", "assertions"])
                .output()
                .expect("run pmset");
            String::from_utf8_lossy(&out.stdout).into_owned()
        }

        /// Does *this* process hold the assertion?
        ///
        /// `pmset` lists every process's assertions, and a Claw Fleet app
        /// running on the same machine registers one under the very same name.
        /// Matching on the name alone made the post-release check fail whenever
        /// the desktop app happened to be running, so scope it to our own pid:
        /// `pmset` prefixes each line with `pid <n>(<proc>):`.
        fn ours(text: &str) -> bool {
            let pid_tag = format!("pid {}(", std::process::id());
            text.lines()
                .any(|l| l.contains(&pid_tag) && l.contains(ASSERTION_NAME))
        }

        super::platform::apply(true).expect("acquire assertion");
        let held = pmset_assertions();
        assert!(ours(&held), "our assertion not visible in pmset output:\n{held}");

        super::platform::apply(false).expect("release assertion");
        let released = pmset_assertions();
        assert!(
            !ours(&released),
            "our assertion still present after release:\n{released}"
        );
    }
}
