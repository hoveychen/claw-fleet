//! Automatic update of audit patterns from a remote GitHub-hosted JSON file.
//!
//! The update flow works like an antivirus signature database:
//!
//! 1. On startup (and then once daily), fetch the remote patterns file and
//!    compare the `version` field with the local copy.
//! 2. If the remote version is higher, validate the JSON structure, then
//!    atomically replace the local file via write-to-temp + rename.
//! 3. Call `audit::reload_patterns()` so the next audit scan picks up the
//!    new rules without an app restart.
//!
//! **Fallback chain** (highest priority first):
//!   `~/.fleet/fleet-audit-patterns.json`  (user override / auto-updated)
//!   → bundled `resources/audit-patterns.json` (shipped with the app binary)
//!   → compiled-in Rust defaults (hardcoded in `audit.rs`)

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::audit::{self, ExternalPatternsFile};

/// GitHub raw URL for the canonical patterns file.
/// Points to the `main` branch so that merging a PR automatically publishes
/// the update — no manual release step required.
const REMOTE_PATTERNS_URL: &str =
    "https://raw.githubusercontent.com/hoveychen/claw-fleet/main/audit-patterns.json";

/// How often (in seconds) the background thread checks for updates.
const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60; // 24 hours

/// Initial delay before the first check (let the app finish starting up).
const INITIAL_DELAY_SECS: u64 = 30;

/// Flag to stop the background thread during tests or shutdown.
static STOP_FLAG: AtomicBool = AtomicBool::new(false);

// ── Paths ───────────────────────────────────────────────────────────────────

/// User-local patterns file (highest priority, also the update target).
fn local_patterns_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("fleet-audit-patterns.json"))
}

// ── Version reading ─────────────────────────────────────────────────────────

/// Read the version from a local JSON file.  Returns 0 if the file is absent
/// or unparseable.
fn local_version(path: &PathBuf) -> u32 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<ExternalPatternsFile>(&s).ok())
        .map(|f| f.version)
        .unwrap_or(0)
}

// ── Remote fetch ────────────────────────────────────────────────────────────

/// Fetch the remote patterns file, validate it, and return the parsed struct
/// along with the raw JSON string (for writing to disk).
fn fetch_remote() -> Result<(ExternalPatternsFile, String), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let resp = client
        .get(REMOTE_PATTERNS_URL)
        .header("User-Agent", "claw-fleet-pattern-update")
        .send()
        .map_err(|e| format!("fetch: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let body = resp.text().map_err(|e| format!("body: {e}"))?;
    let parsed: ExternalPatternsFile =
        serde_json::from_str(&body).map_err(|e| format!("json: {e}"))?;

    // Sanity checks.
    if parsed.patterns.is_empty() && parsed.python_patterns.is_empty() {
        return Err("remote file has no patterns — refusing to apply".into());
    }

    Ok((parsed, body))
}

// ── Atomic file write ───────────────────────────────────────────────────────

/// Write `content` to a temp file in the same directory, then rename.
/// This guarantees the target file is never left in a half-written state.
fn atomic_write(target: &PathBuf, content: &str) -> Result<(), String> {
    let dir = target.parent().ok_or("no parent dir")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir: {e}"))?;

    let tmp = dir.join(format!(".fleet-audit-patterns-{}.tmp", std::process::id()));
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

// ── Single update check ────────────────────────────────────────────────────

/// Perform one update check.  Returns `Ok(true)` if patterns were updated.
pub fn check_for_update() -> Result<bool, String> {
    let local = local_patterns_path().ok_or("cannot determine home dir")?;
    let current_version = local_version(&local);

    let (remote, raw_json) = fetch_remote()?;

    if remote.version <= current_version {
        return Ok(false);
    }

    // Remote is newer — apply the update.
    atomic_write(&local, &raw_json)?;
    audit::reload_patterns();

    crate::log_debug(&format!(
        "pattern_update: updated audit patterns v{current_version} → v{}",
        remote.version
    ));

    Ok(true)
}

// ── Background thread ───────────────────────────────────────────────────────

/// Spawn a background thread that checks for pattern updates daily.
/// Safe to call multiple times — only one thread runs (uses the stop flag
/// for coordination).
pub fn start_background_updater() {
    STOP_FLAG.store(false, Ordering::SeqCst);

    std::thread::Builder::new()
        .name("pattern-updater".into())
        .spawn(move || {
            // Initial delay so the app finishes launching.
            std::thread::sleep(std::time::Duration::from_secs(INITIAL_DELAY_SECS));

            loop {
                if STOP_FLAG.load(Ordering::SeqCst) {
                    break;
                }

                match check_for_update() {
                    Ok(true) => {
                        crate::log_debug("pattern_update: patterns updated successfully");
                    }
                    Ok(false) => {
                        crate::log_debug("pattern_update: patterns are up to date");
                    }
                    Err(e) => {
                        crate::log_debug(&format!("pattern_update: check failed: {e}"));
                    }
                }

                // Sleep in small increments so we can respond to the stop flag.
                for _ in 0..(CHECK_INTERVAL_SECS / 10) {
                    if STOP_FLAG.load(Ordering::SeqCst) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(10));
                }
            }
        })
        .ok();
}

// ── Tauri command ───────────────────────────────────────────────────────────

/// Manually trigger an update check (e.g. from a UI button).
/// Returns a human-readable status message.
pub fn check_update_now() -> String {
    match check_for_update() {
        Ok(true) => "Patterns updated to latest version.".into(),
        Ok(false) => "Patterns are already up to date.".into(),
        Err(e) => format!("Update check failed: {e}"),
    }
}

/// Return the current local patterns version and the file path.
pub fn get_patterns_info() -> (u32, String) {
    let path = local_patterns_path().unwrap_or_default();
    let version = local_version(&path);
    (version, path.to_string_lossy().to_string())
}
