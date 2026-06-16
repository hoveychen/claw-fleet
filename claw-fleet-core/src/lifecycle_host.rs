//! Phase 1 implementation of `claw_fleet_task::runner::TaskLifecycleHost`
//! that delegates straight to `crate::supervisor`'s existing functions.
//! This preserves legacy behaviour: all task lifecycle side-effects write
//! to the shared `~/.fleet/fleet-sessions.json` table.
//!
//! Phase 2's `fleet-task` binary will swap this out for a binary-local
//! host that spawns Claude Code directly without touching the table.

use std::path::PathBuf;

use claw_fleet_task::runner::{LlmMediator, Resolution, TaskLifecycleHost};
use claw_fleet_task::task::Task;
use claw_fleet_task::worker::WorkerSpawnSpec;
use claw_fleet_task::worktree::ConflictSpec;

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

impl LlmMediator for SupervisorHost {
    fn resolve_conflicts(&self, files: &[ConflictSpec]) -> Result<Vec<Resolution>, String> {
        let mediations = crate::merge_mediator::mediate(files).map_err(|e| e.to_string())?;
        Ok(mediations
            .into_iter()
            .map(|m| Resolution {
                path: m.path,
                resolved_content: m.resolved_content,
            })
            .collect())
    }
}
