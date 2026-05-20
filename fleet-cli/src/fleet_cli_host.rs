//! `TaskHost` implementation for the standalone `fleet` CLI process.
//!
//! Phase 3: master agents running under `fleet-task` invoke `fleet task
//! mark-done / mark-failed / update-plan / dispatch` as separate subprocesses.
//! Those subprocesses can't reach the in-fleet-task `LocalHost`, so this
//! host provides what the migrated `claw_fleet_task::actions::*` functions
//! need without touching the legacy fleet-sessions table:
//!
//! - `workspace_for_task` reads the workspace path from `task.workspace` —
//!   persisted by `actions::start_task` in Phase 3. Legacy task JSONs that
//!   don't carry the field fall back to the project-table lookup so the
//!   legacy `LocalBackend` path keeps working.
//! - `enqueue_master / enqueue_worker` error out: the master is already
//!   running inside fleet-task; spawning workers from fleet-cli would bypass
//!   the fleet-task LocalHost's pid tracking, so we direct the caller to
//!   POST `/p-items/<id>/dispatch` over HTTP instead (P8 wires that path).
//! - `pause / resume / terminate_task_sessions` error: signal control lives
//!   with fleet-task; these are user-only operations that go through the
//!   desktop / fleet-task TUI.
//! - `LlmMediator::resolve_conflicts` calls the in-process
//!   `claw_fleet_core::merge_mediator::mediate` so mark-done conflict
//!   resolution works from the CLI as it always did.

use std::path::PathBuf;

use claw_fleet_core::master::MasterSpawnSpec;
use claw_fleet_core::registry;
use claw_fleet_core::runner::{LlmMediator, Resolution, TaskLifecycleHost};
use claw_fleet_core::task::Task;
use claw_fleet_core::worker_executor::WorkerSpawnSpec;
use claw_fleet_core::worktree::ConflictSpec;

pub struct FleetCliHost;

impl TaskLifecycleHost for FleetCliHost {
    fn workspace_for_task(&self, task: &Task) -> Result<PathBuf, String> {
        if let Some(ws) = &task.workspace {
            return Ok(ws.clone());
        }
        // Legacy task (pre-Phase-3) — fall back to project table.
        let project = claw_fleet_core::project::list_projects()
            .into_iter()
            .find(|p| p.id == task.project_id)
            .ok_or_else(|| {
                format!(
                    "task {} has no persisted workspace and project {} not found",
                    task.id, task.project_id
                )
            })?;
        Ok(PathBuf::from(&project.workspace))
    }

    fn enqueue_master(&self, _spec: &MasterSpawnSpec) -> Result<String, String> {
        Err(
            "fleet-cli cannot enqueue master; spawn fleet-task instead \
             (`fleet-task new` or via the desktop launcher)"
                .into(),
        )
    }

    fn enqueue_worker(&self, _spec: &WorkerSpawnSpec) -> Result<String, String> {
        Err(http_dispatch_advice())
    }

    fn pause_task_sessions(&self, _task_id: &str) -> Result<usize, String> {
        Err(signal_route_advice("pause"))
    }

    fn resume_task_sessions(&self, _task_id: &str) -> Result<usize, String> {
        Err(signal_route_advice("resume"))
    }

    fn terminate_task_sessions(&self, _task_id: &str) -> Result<usize, String> {
        Err(signal_route_advice("terminate"))
    }
}

impl LlmMediator for FleetCliHost {
    fn resolve_conflicts(&self, files: &[ConflictSpec]) -> Result<Vec<Resolution>, String> {
        let mediations =
            claw_fleet_core::merge_mediator::mediate(files).map_err(|e| e.to_string())?;
        Ok(mediations
            .into_iter()
            .map(|m| Resolution {
                path: m.path,
                resolved_content: m.resolved_content,
            })
            .collect())
    }
}

fn http_dispatch_advice() -> String {
    "fleet-cli cannot spawn workers; the fleet-task runtime owns dispatch. \
     POST /p-items/<id>/dispatch on the task's HTTP port (see \
     `~/.fleet/runtime/<task_id>.json`) — fleet-cli HTTP routing lands in \
     Phase 3 P8."
        .into()
}

fn signal_route_advice(op: &str) -> String {
    format!(
        "{op}: signal control belongs to fleet-task; use the desktop / TUI \
         to pause/resume/terminate (fleet-cli does not own session pids)."
    )
}

/// Look up the live fleet-task port for `task_id` via the runtime registry.
/// Returns None when the task isn't backed by a running fleet-task process.
pub fn lookup_task_port(task_id: &str) -> Option<u16> {
    registry::read(task_id).ok().flatten().map(|e| e.port)
}
