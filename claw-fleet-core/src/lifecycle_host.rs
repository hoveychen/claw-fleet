//! Phase 1 implementation of `claw_fleet_task::runner::TaskLifecycleHost`
//! that delegates straight to `crate::supervisor`'s existing functions.
//! This preserves legacy behaviour: all task lifecycle side-effects write
//! to the shared `~/.fleet/fleet-sessions.json` table.
//!
//! Phase 2's `fleet-task` binary will swap this out for a binary-local
//! host that spawns Claude Code directly without touching the table.

use std::path::PathBuf;

use claw_fleet_task::master::MasterSpawnSpec;
use claw_fleet_task::runner::TaskLifecycleHost;
use claw_fleet_task::task::Task;
use claw_fleet_task::worker::WorkerSpawnSpec;

pub struct SupervisorHost;

impl TaskLifecycleHost for SupervisorHost {
    fn workspace_for_task(&self, task: &Task) -> Result<PathBuf, String> {
        let project = crate::project::list_projects()
            .into_iter()
            .find(|p| p.id == task.project_id)
            .ok_or_else(|| {
                format!("project {} not found for task {}", task.project_id, task.id)
            })?;
        Ok(PathBuf::from(&project.workspace))
    }

    fn enqueue_master(&self, spec: &MasterSpawnSpec) -> Result<String, String> {
        crate::supervisor::enqueue_master(spec)
    }

    fn enqueue_worker(&self, spec: &WorkerSpawnSpec) -> Result<String, String> {
        crate::supervisor::enqueue_worker(spec)
    }

    fn pause_task_sessions(&self, task_id: &str) -> Result<usize, String> {
        crate::supervisor::pause_task_sessions(task_id)
    }

    fn resume_task_sessions(&self, task_id: &str) -> Result<usize, String> {
        crate::supervisor::resume_task_sessions(task_id)
    }

    fn terminate_task_sessions(&self, task_id: &str) -> Result<usize, String> {
        crate::supervisor::terminate_task_sessions(task_id)
    }
}
