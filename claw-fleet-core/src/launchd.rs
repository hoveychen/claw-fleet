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

/// `~/.fleet/host` — the address `fleet serve` / `fleet webui` actually bound.
///
/// Its own file beside `port` rather than a new format for `port`, so nothing
/// that already reads `port` has to change. Written on every startup.
///
/// Load-bearing for share links: the port alone does not say whether another
/// device can reach this server. A server bound to `127.0.0.1` is unreachable
/// at the machine's LAN address — verified, the connection is simply refused —
/// so a share URL built from the LAN IP would be dead on arrival while looking
/// perfectly fine. Whoever builds such a URL has to know what was bound.
pub fn host_file_path() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".fleet").join("host"))
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
    match crate::session::get_claude_dir() {
        Some(claude_dir) => remove_legacy_fleet_dir_at(&claude_dir),
        None => Ok(()),
    }
}

fn remove_legacy_fleet_dir_at(claude_dir: &std::path::Path) -> std::io::Result<()> {
    let dir = claude_dir.join("fleet");
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// Label of the macOS LaunchAgent an old Fleet build installed to keep a
/// `fleet serve` process running at login (`RunAtLoad` + `KeepAlive`).
const LEGACY_SERVE_LAUNCHAGENT_LABEL: &str = "com.claudefleet.serve";

/// Remove the legacy `com.claudefleet.serve` macOS LaunchAgent.
///
/// An old Fleet build installed a login-time LaunchAgent that kept a
/// `fleet serve` process running (`RunAtLoad` + `KeepAlive`). That installer
/// was removed together with the fleet-session supervisor feature, so current
/// code installs no LaunchAgent — but machines that ran the old build still
/// have the plist and launchd keeps respawning the serve process.
///
/// On a desktop machine that stray `fleet serve` becomes a **second**
/// mobile-relay provider alongside the desktop app: the relay fans every phone
/// `spawn_session` frame out to *all* agent-side connections (fleet-relay
/// `registry::deliver` loops over every opposite-role member), and the
/// per-process `SPAWNED_SESSIONS` dedup can't see across processes — so each
/// mobile submit launches two `claude --session-id <same>` processes, the
/// double-submitted-prompt / duplicate-decision-card bug. The desktop app is
/// the sole intended local provider, so this plist is pure legacy: remove it on
/// desktop startup. Best-effort; non-macOS and a missing plist are clean
/// no-ops.
pub fn remove_legacy_serve_launchagent() -> std::io::Result<()> {
    // Stop + unload a still-loaded service first. With `KeepAlive=true`,
    // deleting the plist alone leaves the current process running (relaunched
    // until the next logout), so the plist must be booted out of the domain
    // before it is removed.
    #[cfg(target_os = "macos")]
    unload_legacy_serve_launchagent();
    match real_home_dir() {
        Some(home) => {
            remove_legacy_serve_launchagent_at(&home.join("Library").join("LaunchAgents"))
        }
        None => Ok(()),
    }
}

/// `launchctl bootout` the legacy serve service so a running (KeepAlive) copy is
/// stopped and dropped from the GUI domain. Best-effort: a service that isn't
/// loaded makes `bootout` exit non-zero, which we ignore.
#[cfg(target_os = "macos")]
fn unload_legacy_serve_launchagent() {
    let uid = unsafe { libc::getuid() };
    let target = format!("gui/{uid}/{LEGACY_SERVE_LAUNCHAGENT_LABEL}");
    let _ = std::process::Command::new("launchctl").args(["bootout", &target]).output();
}

fn remove_legacy_serve_launchagent_at(launch_agents_dir: &std::path::Path) -> std::io::Result<()> {
    let plist = launch_agents_dir.join(format!("{LEGACY_SERVE_LAUNCHAGENT_LABEL}.plist"));
    if plist.exists() {
        std::fs::remove_file(&plist)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three discovery files sit together, so anything that reads one can
    /// read the others without a second convention to remember.
    #[test]
    fn the_discovery_files_are_siblings_under_the_fleet_dir() {
        let (port, host, token) = (
            port_file_path().unwrap(),
            host_file_path().unwrap(),
            token_file_path().unwrap(),
        );
        assert_eq!(port.parent(), host.parent());
        assert_eq!(port.parent(), token.parent());
        assert_eq!(port.file_name().unwrap(), "port");
        assert_eq!(host.file_name().unwrap(), "host");
        assert!(host.parent().unwrap().ends_with(".fleet"));
    }

    #[test]
    fn removes_legacy_dir_and_is_noop_when_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let claude_dir = tmp.path().join(".claude");
        let legacy = claude_dir.join("fleet");
        std::fs::create_dir_all(legacy.join("bin")).unwrap();
        std::fs::write(legacy.join("hooks.jsonl"), b"stale").unwrap();
        std::fs::write(legacy.join("port"), b"1").unwrap();
        assert!(legacy.exists());

        remove_legacy_fleet_dir_at(&claude_dir).unwrap();
        assert!(!legacy.exists(), "legacy dir must be removed");

        // Idempotent: a second call on an absent dir is a clean no-op.
        remove_legacy_fleet_dir_at(&claude_dir).unwrap();
    }

    #[test]
    fn removes_legacy_serve_plist_and_is_noop_when_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let launch_agents = tmp.path().join("LaunchAgents");
        std::fs::create_dir_all(&launch_agents).unwrap();
        let plist = launch_agents.join("com.claudefleet.serve.plist");
        std::fs::write(&plist, b"<plist/>").unwrap();
        assert!(plist.exists());

        remove_legacy_serve_launchagent_at(&launch_agents).unwrap();
        assert!(!plist.exists(), "legacy serve LaunchAgent plist must be removed");

        // Idempotent: a second call on an absent plist is a clean no-op.
        remove_legacy_serve_launchagent_at(&launch_agents).unwrap();
    }
}
