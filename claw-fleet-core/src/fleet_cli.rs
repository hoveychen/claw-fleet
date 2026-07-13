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

/// The CLI as shipped in `exe_dir`, if it is there at all. A `cargo run` of the
/// desktop app has no sidecar beside it, which is why callers treat a missing
/// one as "nothing to publish" rather than an error.
fn sidecar_in(exe_dir: &Path) -> Option<PathBuf> {
    SIDECAR_NAMES
        .iter()
        .map(|n| exe_dir.join(n))
        .find(|p| p.is_file())
}

/// Publish `src` at `dest` as a symlink, replacing a stale link in place.
/// Idempotent: an existing link already pointing at `src` is left alone.
#[cfg(unix)]
fn publish(src: &Path, dest: &Path) -> Result<(), String> {
    if std::fs::read_link(dest).is_ok_and(|t| t == src) {
        return Ok(());
    }
    // `symlink` refuses to clobber, so an outdated link/file must go first.
    if dest.symlink_metadata().is_ok() {
        std::fs::remove_file(dest).map_err(|e| format!("remove stale {}: {e}", dest.display()))?;
    }
    std::os::unix::fs::symlink(src, dest)
        .map_err(|e| format!("symlink {} -> {}: {e}", dest.display(), src.display()))
}

/// Publish `src` at `dest` by copying. Windows only creates symlinks for
/// privileged processes (or under Developer Mode), so a copy is the only
/// approach that works for an ordinary user. Skipped when `dest` already
/// matches `src`, so startup doesn't rewrite the file every launch.
#[cfg(not(unix))]
fn publish(src: &Path, dest: &Path) -> Result<(), String> {
    if is_up_to_date(src, dest) {
        return Ok(());
    }
    std::fs::copy(src, dest)
        .map(|_| ())
        .map_err(|e| format!("copy {} -> {}: {e}", src.display(), dest.display()))
}

/// `dest` is a copy of the current `src`: same size, and not older than it.
/// Keeps startup from rewriting the binary on every launch (and, on Windows,
/// from hitting a sharing violation when the published copy is in use).
#[cfg(not(unix))]
fn is_up_to_date(src: &Path, dest: &Path) -> bool {
    let (Ok(s), Ok(d)) = (src.metadata(), dest.metadata()) else {
        return false;
    };
    let (Ok(sm), Ok(dm)) = (s.modified(), d.modified()) else {
        return false;
    };
    s.len() == d.len() && dm >= sm
}

/// Publish the CLI shipped in `exe_dir` into [`fleet_bin_dir`], returning the
/// published path.
fn ensure_link_from(exe_dir: &Path) -> Result<PathBuf, String> {
    let src = sidecar_in(exe_dir)
        .ok_or_else(|| format!("no fleet CLI shipped in {}", exe_dir.display()))?;
    let dir = fleet_bin_dir().ok_or("cannot resolve home directory")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let dest = dir.join(LINK_NAME);
    publish(&src, &dest)?;
    Ok(dest)
}

/// Publish the CLI shipped next to the running executable into
/// [`fleet_bin_dir`] so spawned sessions can run `fleet`.
///
/// Call this on desktop startup. A failure is not fatal — it only means the
/// agent's `fleet plan …` calls won't resolve — so callers log and continue.
pub fn ensure_fleet_cli_link() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let dir = exe
        .parent()
        .ok_or("running executable has no parent directory")?;
    ensure_link_from(dir)
}

/// Resolve a usable `fleet` binary path, in order: the sidecar next to the
/// running executable, the copy [`ensure_fleet_cli_link`] published, the
/// macOS installer's symlink, then PATH.
///
/// This is the one resolver — `hooks` (which bakes an absolute path into the
/// hook commands it writes to `settings.json`) and the desktop MCP injector
/// both go through it, so none of them can drift back to being POSIX-only.
pub fn resolve_fleet_binary() -> Option<PathBuf> {
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        if let Some(p) = sidecar_in(&dir) {
            return Some(p);
        }
    }
    if let Some(published) = fleet_bin_dir().map(|d| d.join(LINK_NAME)) {
        if published.is_file() {
            return Some(published);
        }
    }
    if !cfg!(windows) && Path::new("/usr/local/bin/fleet").is_file() {
        return Some(PathBuf::from("/usr/local/bin/fleet"));
    }
    // PATH lookup: `which` everywhere but Windows, which spells it `where` and
    // may list several hits, newline-separated.
    let probe = if cfg!(windows) { "where" } else { "which" };
    let out = crate::process_util::command(probe).arg("fleet").output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(PathBuf::from)
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
