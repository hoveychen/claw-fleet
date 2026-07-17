//! Canonical `fleet serve` discovery files under `~/.fleet/`.
//!
//! `fleet serve` writes its live port + auth token here (see
//! `hooks_server::serve`) so tools launched inside managed sessions and the
//! desktop app can discover a running serve process regardless of how it was
//! started. The macOS LaunchAgent installer that used to live in this module
//! was removed together with the fleet-session supervisor feature.

use std::path::PathBuf;

use crate::session::real_home_dir;

/// `~/.fleet/port` — written by `fleet serve` on startup.
pub fn port_file_path() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".fleet").join("port"))
}

/// `~/.fleet/token` — written by `fleet serve` on startup.
pub fn token_file_path() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".fleet").join("token"))
}

/// Remove the legacy `~/.claude/fleet/` runtime directory.
///
/// Everything Fleet kept there now lives under `~/.fleet/`: the `port`/`token`
/// discovery files and the published `fleet` CLI under `bin/` moved with this
/// change, and a defunct `hooks.jsonl` event log (an old build appended to it
/// on every hook event with no truncation — seen at 5.3 GB in the wild) plus
/// stale `fleet-sessions.json`/`projects.json` from a superseded data model
/// were never read by current code. Nothing in the tree reads or writes this
/// directory anymore, so it is pure legacy. Called best-effort on desktop
/// startup; a failure (including the dir simply not existing) is not fatal.
pub fn remove_legacy_fleet_dir() -> std::io::Result<()> {
    match real_home_dir() {
        Some(home) => remove_legacy_fleet_dir_at(&home),
        None => Ok(()),
    }
}

fn remove_legacy_fleet_dir_at(home: &std::path::Path) -> std::io::Result<()> {
    let dir = home.join(".claude").join("fleet");
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_legacy_dir_and_is_noop_when_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path();
        let legacy = home.join(".claude").join("fleet");
        std::fs::create_dir_all(legacy.join("bin")).unwrap();
        std::fs::write(legacy.join("hooks.jsonl"), b"stale").unwrap();
        std::fs::write(legacy.join("port"), b"1").unwrap();
        assert!(legacy.exists());

        remove_legacy_fleet_dir_at(home).unwrap();
        assert!(!legacy.exists(), "legacy dir must be removed");

        // Idempotent: a second call on an absent dir is a clean no-op.
        remove_legacy_fleet_dir_at(home).unwrap();
    }
}
