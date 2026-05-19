//! Path utilities mirrored from `claw_fleet_core::session` so this crate can
//! operate without a reverse dependency on core. Keep these two functions in
//! lock-step with the originals (`real_home_dir` / `get_fleet_dir`); the
//! test-only `fleet_home_lock` is also mirrored here so worktree's existing
//! tests can serialise on the same notion of "claim the FLEET_HOME env var".

use std::path::PathBuf;

/// Returns the real user home directory, bypassing sandbox container redirection.
///
/// Mirrors `claw_fleet_core::session::real_home_dir`. See that doc comment for
/// the macOS sandbox / `FLEET_HOME` rationale.
pub fn real_home_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("FLEET_HOME") {
        let path = PathBuf::from(dir);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CStr;
        let pw = unsafe { libc::getpwuid(libc::getuid()) };
        if !pw.is_null() {
            let home = unsafe { CStr::from_ptr((*pw).pw_dir) };
            return Some(PathBuf::from(home.to_string_lossy().into_owned()));
        }
    }
    dirs::home_dir()
}

pub fn get_fleet_dir() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".fleet"))
}

/// Process-wide mutex used by tests to serialise access to the
/// `FLEET_HOME` environment variable. Exposed unconditionally so
/// integration tests (separate test binaries) can serialise against the
/// same notion of "claim FLEET_HOME" as in-crate unit tests.
pub fn fleet_home_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let lock = LOCK.get_or_init(|| Mutex::new(()));
    match lock.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}
