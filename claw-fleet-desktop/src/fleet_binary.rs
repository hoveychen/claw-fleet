//! Cross-platform sibling lookup for the fleet CLI binary. Used by the
//! `mcp_injector` acquire path so the desktop app can publish a working
//! command path in `~/.claude.json`'s `mcpServers.fleet.command`.
//!
//! - macOS production bundle: Tauri sidecar lands as
//!   `<app>/Contents/MacOS/fleet`.
//! - Cargo dev build: the fleet-cli crate names its binary `fleet-cli`.
//! - Windows production: same sibling layout, `fleet.exe`.

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
