//! Host traits the task `actions::*` functions and the deterministic
//! [`crate::orchestrator::Orchestrator`] are written against.
//!
//! The old LLM-master `TaskRunner` was removed in the task-subsystem rebuild —
//! the `fleet-task` process now drives a task via `Orchestrator::step` instead
//! of spawning a master session. These traits remain because `actions::*`
//! (dispatch_pitem / mark_done / pause / resume / clear) still take a host to
//! resolve the workspace, spawn workers, and signal sessions.

use std::path::PathBuf;

use crate::task::Task;
use crate::worker::WorkerSpawnSpec;

/// Side-effects the task actions issue against a host: resolve the project
/// workspace, spawn workers, and pause/resume/terminate the session
/// subprocesses. Implemented in claw-fleet-core by `SupervisorHost` (shared
/// session table) and in `fleet-task` by `LocalHost` (local pid map).
pub trait TaskLifecycleHost {
    /// Resolve the project workspace path for the given task — the worktree
    /// base for worker spawns.
    fn workspace_for_task(&self, task: &Task) -> Result<PathBuf, String>;
    fn enqueue_worker(&self, spec: &WorkerSpawnSpec) -> Result<String, String>;
    fn pause_task_sessions(&self, task_id: &str) -> Result<usize, String>;
    fn resume_task_sessions(&self, task_id: &str) -> Result<usize, String>;
    fn terminate_task_sessions(&self, task_id: &str) -> Result<usize, String>;
}

/// LLM-driven merge conflict mediation. Implemented in claw-fleet-core by
/// `SupervisorHost`; fleet-task's `LocalHost` ships a stub.
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
/// impl makes any `TaskLifecycleHost + LlmMediator` automatically a `TaskHost`.
pub trait TaskHost: TaskLifecycleHost + LlmMediator {}
impl<T: TaskLifecycleHost + LlmMediator + ?Sized> TaskHost for T {}
