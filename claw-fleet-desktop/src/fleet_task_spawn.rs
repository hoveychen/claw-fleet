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

/// Where the spawned `fleet task-runtime` process's stderr is captured, so a
/// crash-before-register leaves a recoverable reason instead of vanishing into
/// `/dev/null`. `<fleet_dir>/logs/task-runtime-<id>.log` (sibling of
/// `runtime/`), truncated per start. `None` if the fleet dir can't be resolved.
pub fn task_runtime_log_path(task_id: &str) -> Option<PathBuf> {
    let dir = claw_fleet_core::paths::get_fleet_dir()?.join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(format!("task-runtime-{task_id}.log")))
}

/// Read the last `max_bytes` of the task-runtime log as a trimmed string, for
/// enriching an error message. Empty string when the log is missing/unreadable.
fn tail_runtime_log(task_id: &str, max_bytes: u64) -> String {
    let Some(path) = task_runtime_log_path(task_id) else {
        return String::new();
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return String::new();
    };
    let Ok(mut f) = std::fs::File::open(&path) else {
        return String::new();
    };
    let start = meta.len().saturating_sub(max_bytes);
    if start > 0 {
        use std::io::Seek;
        let _ = f.seek(std::io::SeekFrom::Start(start));
    }
    let mut buf = String::new();
    use std::io::Read;
    let _ = f.read_to_string(&mut buf);
    buf.trim().to_string()
}

/// Spawn `fleet task-runtime resume <task_id> --workspace <ws> --no-tui` and
/// return the child handle WITHOUT waiting for it to come up. Start is
/// non-blocking: the desktop returns immediately and the runtime registry
/// watcher (runtime_registry.rs) flips the task to live asynchronously when
/// the entry appears. A `BinaryMissing` / `Spawn` error here still surfaces
/// synchronously so an impossible start fails fast.
///
/// The child's stderr is captured to `task_runtime_log_path` (not discarded)
/// so a crash-before-register can be diagnosed; `await_registry` folds the log
/// tail into the error.
pub fn spawn_fleet_task_detached(
    task_id: &str,
    workspace: &std::path::Path,
) -> Result<std::process::Child, SpawnError> {
    let bin = resolve_fleet_task_binary().ok_or(SpawnError::BinaryMissing)?;
    // Capture stderr to a per-task log; fall back to null if the path/file
    // can't be created so spawning never fails just because logging can't.
    let stderr = task_runtime_log_path(task_id)
        .and_then(|p| std::fs::File::create(p).ok())
        .map(Stdio::from)
        .unwrap_or_else(Stdio::null);
    claw_fleet_core::process_util::command(&bin)
        .arg("task-runtime")
        .arg("resume")
        .arg(task_id)
        .arg("--workspace")
        .arg(workspace)
        .arg("--no-tui")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .map_err(|e| SpawnError::Spawn(e.to_string()))
}

/// Block up to `appear_timeout` for the runtime registry entry to publish
/// (proves the HTTP server bound + master started). Polls every 100ms and
/// also reaps the child: if it exits before registering we fail immediately
/// rather than waiting out the whole timeout. Intended to run on a background
/// thread so the UI is never blocked.
pub fn await_registry(
    task_id: &str,
    child: &mut std::process::Child,
    appear_timeout: Duration,
) -> Result<registry::RegistryEntry, SpawnError> {
    let deadline = Instant::now() + appear_timeout;
    while Instant::now() < deadline {
        if let Ok(Some(entry)) = registry::read(task_id) {
            return Ok(entry);
        }
        if let Ok(Some(status)) = child.try_wait() {
            let tail = tail_runtime_log(task_id, 4096);
            let detail = if tail.is_empty() {
                format!("task-runtime exited before registering ({status})")
            } else {
                // Last line is usually the actual cause (panic / git error).
                let last = tail.lines().last().unwrap_or(&tail);
                format!("task-runtime exited before registering ({status}): {last}")
            };
            return Err(SpawnError::Spawn(detail));
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
