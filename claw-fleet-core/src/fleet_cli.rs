//! Publish the bundled `fleet` CLI where a spawned session's PATH can find it.
//!
//! [`crate::session_launch`] prepends `~/.claude/fleet/bin` to every spawned
//! agent's PATH so its Bash tool can run `fleet plan check` and friends — the
//! PRD-discipline guidance we inject into `~/.claude/CLAUDE.md` tells the agent
//! to do exactly that. Nothing ever created that directory: the only `fleet`
//! that reached PATH was the `/usr/local/bin/fleet` symlink made by the
//! macOS-only `install_fleet_cli` command, so on Linux and Windows every
//! `fleet plan …` died with "command not found" — silently, because a plan that
//! is never recorded just makes the task tab not render.
//!
//! [`ensure_fleet_cli_link`] fills the directory at startup. It writes inside
//! the user's own home, so unlike the macOS installer it needs no privilege
//! escalation and works the same on all three platforms.

use std::path::{Path, PathBuf};

/// Basenames the CLI ships under next to the desktop executable. A Tauri
/// sidecar lands as `fleet`; a plain `cargo build` names it `fleet-cli`.
#[cfg(windows)]
const SIDECAR_NAMES: &[&str] = &["fleet.exe", "fleet-cli.exe"];
#[cfg(not(windows))]
const SIDECAR_NAMES: &[&str] = &["fleet", "fleet-cli"];

/// What `fleet …` must resolve to once the directory is on PATH.
#[cfg(windows)]
pub const LINK_NAME: &str = "fleet.exe";
#[cfg(not(windows))]
pub const LINK_NAME: &str = "fleet";

/// `~/.claude/fleet/bin` — the directory [`crate::session_launch`] prepends to
/// every spawned session's PATH.
pub fn fleet_bin_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".claude").join("fleet").join("bin"))
}

/// Publish the CLI shipped in `exe_dir` into [`fleet_bin_dir`], returning the
/// published path.
fn ensure_link_from(exe_dir: &Path) -> Result<PathBuf, String> {
    let _ = exe_dir;
    let dir = fleet_bin_dir().ok_or("cannot resolve home directory")?;
    Ok(dir.join(LINK_NAME))
}

/// Publish the CLI shipped next to the running executable into
/// [`fleet_bin_dir`] so spawned sessions can run `fleet`.
pub fn ensure_fleet_cli_link() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let dir = exe
        .parent()
        .ok_or("running executable has no parent directory")?;
    ensure_link_from(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of `~/.claude/fleet/bin` is that a spawned agent can run
    /// `fleet plan check`. That only works if something actually puts the binary
    /// there — which, before this module, nothing did.
    #[test]
    fn ensure_link_publishes_the_sidecar_into_the_session_path_dir() {
        let _guard = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!("fleet_cli_link_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let home = tmp.join("home");
        let exe_dir = tmp.join("app");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&exe_dir).unwrap();

        // Stand-in for the Tauri sidecar shipped next to the desktop binary.
        const SIDECAR_BODY: &[u8] = b"#!/bin/sh\necho fleet\n";
        std::fs::write(exe_dir.join(LINK_NAME), SIDECAR_BODY).unwrap();

        let prev = std::env::var_os("FLEET_HOME");
        std::env::set_var("FLEET_HOME", &home);
        let published = ensure_link_from(&exe_dir);
        match prev {
            Some(v) => std::env::set_var("FLEET_HOME", v),
            None => std::env::remove_var("FLEET_HOME"),
        }

        let published = published.expect("ensure_link_from failed");
        let expected = home
            .join(".claude")
            .join("fleet")
            .join("bin")
            .join(LINK_NAME);
        assert_eq!(published, expected, "published to the wrong path");
        assert!(
            expected.exists(),
            "session PATH carries {}, but nothing published fleet into it",
            expected.display()
        );
        assert_eq!(
            std::fs::read(&expected).unwrap(),
            SIDECAR_BODY,
            "published fleet must resolve to the sidecar's bytes"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
