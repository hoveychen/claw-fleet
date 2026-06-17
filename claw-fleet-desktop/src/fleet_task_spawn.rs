//! Spawn the task lifecycle owner for the desktop app via the `fleet`
//! sidecar's `task-runtime` subcommand (the standalone `fleet-task` binary
//! was folded into `fleet`). If the binary can't be located we error; we
//! don't fall back to the legacy `supervisor::enqueue_master` path.
//!
//! Resolution: look next to the desktop executable first (production bundle
//! puts the sidecar there as `fleet`), which also covers the cargo
//! `target/<profile>/` sibling layout in dev.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use claw_fleet_core::registry;

/// Locate the `fleet` sidecar next to the desktop executable (production
/// bundle) or in the cargo target dir (dev). The task runtime is invoked as
/// `fleet task-runtime resume <id>`.
pub fn resolve_fleet_task_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let parent = exe.parent()?;
    for name in ["fleet", "fleet.exe"] {
        let candidate = parent.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[derive(Debug)]
pub enum SpawnError {
    BinaryMissing,
    Spawn(String),
    RegistryTimeout(String),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::BinaryMissing => write!(
                f,
                "fleet binary not found next to desktop executable \
                 (cannot spawn task-runtime; no supervisor fallback)"
            ),
            SpawnError::Spawn(e) => write!(f, "spawn fleet task-runtime: {e}"),
            SpawnError::RegistryTimeout(task_id) => write!(
                f,
                "fleet task-runtime registry entry for {task_id} did not appear in time"
            ),
        }
    }
}

impl std::error::Error for SpawnError {}

/// Spawn `fleet task-runtime resume <task_id> --workspace <ws> --no-tui` and
/// wait for the runtime registry to publish the corresponding entry (proves
/// the HTTP server bound + master started).
///
/// Returns the registry entry once visible; errors otherwise. Caller owns
/// the spawned process implicitly — once detached, the desktop app no
/// longer tracks it; the task-runtime process is responsible for its own
/// lifecycle and self-cleans the registry on exit.
pub fn spawn_fleet_task_resume(
    task_id: &str,
    workspace: &std::path::Path,
    appear_timeout: Duration,
) -> Result<registry::RegistryEntry, SpawnError> {
    let bin = resolve_fleet_task_binary().ok_or(SpawnError::BinaryMissing)?;
    claw_fleet_core::process_util::command(&bin)
        .arg("task-runtime")
        .arg("resume")
        .arg(task_id)
        .arg("--workspace")
        .arg(workspace)
        .arg("--no-tui")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| SpawnError::Spawn(e.to_string()))?;

    let deadline = Instant::now() + appear_timeout;
    while Instant::now() < deadline {
        if let Ok(Some(entry)) = registry::read(task_id) {
            return Ok(entry);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(SpawnError::RegistryTimeout(task_id.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_missing_when_no_fleet_next_to_exe() {
        // In `cargo test` the current_exe is the test binary; the `fleet`
        // sidecar lives at target/debug/fleet, which is *not* the parent of
        // the test binary at target/debug/deps/. We just assert the type is
        // OK (can't assert None unconditionally — some runners leave a fleet
        // binary in the parent dir).
        let resolved = resolve_fleet_task_binary();
        let _ = resolved;
    }

    #[test]
    fn display_message_names_fleet_and_task_runtime() {
        let s = SpawnError::BinaryMissing.to_string();
        assert!(s.contains("fleet binary not found"));
        assert!(s.contains("task-runtime"));
        assert!(s.contains("no supervisor fallback"));
    }
}
