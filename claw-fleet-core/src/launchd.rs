//! Canonical `fleet serve` discovery files under `~/.claude/fleet/`.
//!
//! `fleet serve` writes its live port + auth token here (see
//! `hooks_server::serve`) so tools launched inside managed sessions and the
//! desktop app can discover a running serve process regardless of how it was
//! started. The macOS LaunchAgent installer that used to live in this module
//! was removed together with the fleet-session supervisor feature.

use std::path::PathBuf;

use crate::session::real_home_dir;

/// `~/.claude/fleet/port` — written by `fleet serve` on startup.
pub fn port_file_path() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".claude").join("fleet").join("port"))
}

/// `~/.claude/fleet/token` — written by `fleet serve` on startup.
pub fn token_file_path() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".claude").join("fleet").join("token"))
}
