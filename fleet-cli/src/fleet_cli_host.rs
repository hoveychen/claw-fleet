//! fleet-cli runtime-registry helper.
//!
//! The old `FleetCliHost` (a `TaskHost` for the deleted `mark-done` /
//! `mark-failed` CLI commands) is gone — the deterministic orchestrator inside
//! `fleet-task` owns dispatch / review / merge now, so the CLI no longer drives
//! those transitions. The one piece still needed is the runtime-port lookup the
//! `dispatch` command uses to reach a live fleet-task's HTTP API.

use claw_fleet_core::registry;

/// Look up the live fleet-task HTTP port for `task_id` via the runtime
/// registry. Returns `None` when the task isn't backed by a running
/// fleet-task process.
pub fn lookup_task_port(task_id: &str) -> Option<u16> {
    registry::read(task_id).ok().flatten().map(|e| e.port)
}
