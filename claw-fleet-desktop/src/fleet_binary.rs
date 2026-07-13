//! Cross-platform lookup for the fleet CLI binary. Used by the `mcp_injector`
//! acquire path so the desktop app can publish a working command path in
//! `~/.claude.json`'s `mcpServers.fleet.command`.
//!
//! - macOS production bundle: Tauri sidecar lands as
//!   `<app>/Contents/MacOS/fleet`.
//! - Cargo dev build: the fleet-cli crate names its binary `fleet-cli`.
//! - Windows production: same sibling layout, `fleet.exe`.
//!
//! The lookup itself lives in `claw_fleet_core::fleet_cli` so the desktop app,
//! the hook installer, and `fleet serve` all resolve the binary the same way.
//! This module stays as the desktop-side name its callers already use.

pub fn resolve_fleet_binary() -> Option<std::path::PathBuf> {
    claw_fleet_core::fleet_cli::resolve_fleet_binary()
}
