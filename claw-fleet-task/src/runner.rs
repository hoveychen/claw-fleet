//! Task runner — the per-task lifecycle facade that Phase 2's `fleet-task`
//! binary will use to drive a single task's plan from Drafting to Done.
//!
//! Phase 1 implements the runner skeleton plus a minimal `step()` that
//! advances a Running task by one dispatchable P-item. Reaping and master
//! lifecycle are still driven by `claw_fleet_core::supervisor::tick` —
//! Phase 2 will integrate those into `TaskRunner` so the binary owns the
//! whole loop.

use std::path::PathBuf;

use crate::master::MasterSpawnSpec;
use crate::pitem::{PItemId, PItemStatus};
use crate::task::{get_task, write_task_atomic, task_write_lock, Task, TaskId, TaskStatus};
use crate::worker::{worker_spawn_spec, WorkerSpawnSpec};
use crate::worktree;

/// The side-effects a `TaskRunner` issues when it advances a task's plan
/// — resolve project workspace, spawn the master, spawn workers,
/// pause/resume/terminate the session subprocesses. Implemented in
/// claw-fleet-core by `SupervisorHost` (preserves Phase 1 behaviour: writes
/// to the shared `fleet-sessions.json`). Phase 2's `fleet-task` binary
/// provides an alternative implementation that spawns Claude Code locally
/// without going through the shared table.
pub trait TaskLifecycleHost {
    /// Resolve the project workspace path for the given task. The runner
    /// uses this as the worktree base for worker spawns.
    fn workspace_for_task(&self, task: &Task) -> Result<PathBuf, String>;
    fn enqueue_master(&self, spec: &MasterSpawnSpec) -> Result<String, String>;
    fn enqueue_worker(&self, spec: &WorkerSpawnSpec) -> Result<String, String>;
    fn pause_task_sessions(&self, task_id: &str) -> Result<usize, String>;
    fn resume_task_sessions(&self, task_id: &str) -> Result<usize, String>;
    fn terminate_task_sessions(&self, task_id: &str) -> Result<usize, String>;
}

/// LLM-driven merge conflict mediation. Implemented in claw-fleet-core by
/// `SupervisorHost` (delegating to `merge_mediator::mediate`); fleet-task's
/// `LocalHost` ships a stub for Phase 2 — wiring it to a real provider is
/// Phase 3 work.
pub trait LlmMediator {
    fn resolve_conflicts(
        &self,
        files: &[crate::worktree::ConflictSpec],
    ) -> Result<Vec<Resolution>, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub path: std::path::PathBuf,
    pub resolved_content: String,
}

/// Combined host trait used by the business `actions::*` functions. A blanket
/// impl makes any `TaskLifecycleHost + LlmMediator` automatically a `TaskHost`,
/// so callers don't need to implement an extra trait.
pub trait TaskHost: TaskLifecycleHost + LlmMediator {}
impl<T: TaskLifecycleHost + LlmMediator + ?Sized> TaskHost for T {}

/// Outcome of a single `TaskRunner::step` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerStep {
    /// A worker was enqueued for the named P-item. Carries the worker
    /// session id returned by the host.
    SpawnedWorker(PItemId, String),
    /// No P-item was dispatchable this tick (all blocked, or task isn't
    /// Running). Caller should sleep / retry later.
    Idle,
    /// Task is in a terminal state.
    Done,
}

pub struct TaskRunner<'a> {
    task_id: TaskId,
    host: &'a dyn TaskLifecycleHost,
}

impl<'a> TaskRunner<'a> {
    pub fn new(task_id: TaskId, host: &'a dyn TaskLifecycleHost) -> Self {
        Self { task_id, host }
    }

    pub fn task_id(&self) -> &TaskId {
        &self.task_id
    }

    /// Advance the task by one step. Looks at the task's current state and
    /// either dispatches the next unblocked P-item (spawning a worker via
    /// the host) or reports `Idle` / `Done`.
    ///
    /// **Does not block**. Reaping finished workers is still the
    /// supervisor's responsibility in Phase 1.
    pub fn step(&self) -> Result<RunnerStep, String> {
        let task = get_task(&self.task_id)?;
        match task.status {
            TaskStatus::Done | TaskStatus::Abandoned => Ok(RunnerStep::Done),
            TaskStatus::Drafting
            | TaskStatus::Planning
            | TaskStatus::Ready
            | TaskStatus::Paused => Ok(RunnerStep::Idle),
            TaskStatus::Running => self.dispatch_next_pitem(&task),
        }
    }

    fn dispatch_next_pitem(&self, task: &Task) -> Result<RunnerStep, String> {
        let unblocked = task.plan.unblocked_items();
        let Some(p_item_id) = unblocked.into_iter().next() else {
            return Ok(RunnerStep::Idle);
        };
        let lock = task_write_lock(&task.id);
        let _g = lock.lock().expect("task write mutex poisoned");
        // Re-read inside the lock so we don't race another writer.
        let mut task = get_task(&self.task_id)?;
        {
            let Some(item) = task.plan.get_mut(&p_item_id) else {
                return Ok(RunnerStep::Idle);
            };
            if !matches!(
                item.status,
                PItemStatus::WaitDeps | PItemStatus::WaitResource
            ) {
                // Someone else dispatched it between unblocked_items() and
                // the lock acquisition — try again next tick.
                return Ok(RunnerStep::Idle);
            }
            item.status = PItemStatus::Running;
            item.started_at = Some(chrono::Utc::now().timestamp());
        }
        write_task_atomic(&task)?;
        let workspace = self.host.workspace_for_task(&task)?;
        let task_branch = task.task_branch.as_deref().ok_or_else(|| {
            format!(
                "task {} has no task_branch — call start_task before dispatching P-items",
                task.id
            )
        })?;
        let cwd = worktree::provision(&workspace, task_branch, &task.id, &p_item_id)?;
        let spec = worker_spawn_spec(&task, &p_item_id, cwd)?;
        let worker_sid = self.host.enqueue_worker(&spec)?;
        if let Some(item) = task.plan.get_mut(&p_item_id) {
            item.agent_session_id = Some(worker_sid.clone());
        }
        write_task_atomic(&task)?;
        Ok(RunnerStep::SpawnedWorker(p_item_id, worker_sid))
    }
}
