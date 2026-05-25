//! macOS startup helper: ensure the `fleet serve` LaunchAgent is installed and
//! pointing at the fleet binary that ships with this app.
//!
//! Without this, queued FleetSession entries sit in
//! `~/.claude/fleet/fleet-sessions.json` forever — the supervisor `tick()` loop
//! only runs inside `fleet serve`, and nothing else launches it. The Tauri
//! command `install_fleet_daemon` exists but no UI calls it, so first-run users
//! never get a daemon.
//!
//! Called from the Tauri setup hook on every app launch. Idempotent: skip if
//! already installed and the plist points at the same fleet binary.

#[cfg(target_os = "macos")]
pub fn ensure_supervisor_daemon() {
    use claw_fleet_core::launchd;

    let Some(fleet_path) = resolve_fleet_binary() else {
        eprintln!(
            "[daemon-autostart] fleet binary not found near current_exe; \
             supervisor will not auto-spawn queued sessions"
        );
        return;
    };
    let fleet_path_str = fleet_path.to_string_lossy().to_string();

    // Re-create `~/.claude/fleet/bin/fleet → <fleet_path>` so child agents
    // (claude --print spawned by supervisor) find `fleet session status` on
    // PATH. Cheap; safe to call every startup.
    if let Err(e) = claw_fleet_core::supervisor::ensure_fleet_cli_link(&fleet_path_str) {
        eprintln!("[daemon-autostart] ensure_fleet_cli_link: {e}");
    }

    // Install Stop / UserPromptSubmit hooks so the kanban Pending column lights
    // up when an agent ends its turn. Hooks no-op for non-fleet sessions (the
    // CLI checks FLEET_SESSION_ID and exits 0 if unset). Idempotent: marker
    // dedup inside apply_idle_hooks.
    if let Err(e) = claw_fleet_core::hooks::apply_idle_hooks() {
        eprintln!("[daemon-autostart] apply_idle_hooks: {e}");
    }

    if launchd::is_installed() && launchd::installed_plist_points_at(&fleet_path_str) {
        return;
    }

    let token = match launchd::read_or_create_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[daemon-autostart] token: {e}");
            return;
        }
    };

    match launchd::install(&fleet_path_str, 0, &token) {
        Ok(()) => eprintln!(
            "[daemon-autostart] LaunchAgent installed → {}",
            fleet_path_str
        ),
        Err(e) => eprintln!("[daemon-autostart] install failed: {e}"),
    }
}

/// Cross-platform sibling lookup for the fleet CLI binary. Used by both
/// the macOS supervisor-daemon autostart AND the `mcp_injector` acquire
/// call so the desktop app can publish a working command path in
/// `~/.claude.json`'s `mcpServers.fleet.command`.
///
/// - macOS production bundle: Tauri sidecar lands as
///   `<app>/Contents/MacOS/fleet`.
/// - Cargo dev build: the fleet-cli crate names its binary `fleet-cli`.
/// - Windows production: same sibling layout, `fleet.exe`.
pub fn resolve_fleet_binary() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let parent = exe.parent()?;
    let candidates: &[&str] = if cfg!(windows) {
        &["fleet.exe", "fleet-cli.exe"]
    } else {
        &["fleet", "fleet-cli"]
    };
    for name in candidates {
        let p = parent.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
pub fn ensure_supervisor_daemon() {}
