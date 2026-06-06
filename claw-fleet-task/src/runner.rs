//! Task runner — the per-task lifecycle facade that Phase 2's `fleet-task`
//! binary will use to drive a single task's plan from Drafting to Done.
//!
//! Phase 1 implements the runner skeleton plus a minimal `step()` that
//! advances a Running task by one dispatchable P-item. Reaping and master
//! lifecycle are still driven by `claw_fleet_core::supervisor::tick` —
//! Phase 2 will integrate those into `TaskRunner` so the binary owns the
//! whole loop.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::fleet_config::FleetConfig;
use crate::master::MasterSpawnSpec;
use crate::pitem::{PItemId, PItemStatus, ResourceName};
use crate::plan::DagPlan;
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

// [REQ-047] Config-aware scheduling: the bare `claw_fleet_core::scheduler`
// treats every resource as a mutex (concurrency 1). fleet.yaml's custom locks
// can declare `concurrency: N`, turning a resource into a counting semaphore
// (N holders may run at once). This function is the scheduler's view *with*
// the fleet.yaml config applied: it picks a non-conflicting set of unblocked
// P-items where each resource is held by no more than its configured cap,
// counting both `currently_held` (resources already taken by running workers)
// and the items it tentatively selects in this pass.
//
// Built-in / undeclared resources fall back to cap 1 (mutex), preserving the
// core scheduler's behaviour when no fleet.yaml is present or when a resource
// isn't declared custom.
//
// `currently_held` maps each held resource to how many holders currently have
// it (a running phase that holds `custom:db` contributes 1 to that key).
pub fn dispatchable_with_config(
    plan: &DagPlan,
    config: &FleetConfig,
    currently_held: &HashMap<ResourceName, u32>,
) -> Result<Vec<PItemId>, String> {
    if let Some(cycle) = plan.cycle_detect() {
        return Err(format!("plan contains a cycle: {cycle:?}"));
    }
    // Running tally of holders per resource = external holders + what we've
    // tentatively selected this pass.
    let mut held: HashMap<ResourceName, u32> = currently_held.clone();
    let mut selected: Vec<PItemId> = Vec::new();

    // `unblocked_items()` is already sorted alphabetically, matching the core
    // scheduler's deterministic selection order.
    for id in plan.unblocked_items() {
        let Some(item) = plan.get(&id) else { continue };
        // Would adding this item exceed any resource's cap?
        let fits = item.resources.iter().all(|r| {
            let cap = config.concurrency_for(r);
            let current = held.get(r).copied().unwrap_or(0);
            current + 1 <= cap
        });
        if !fits {
            continue;
        }
        for r in &item.resources {
            *held.entry(r.clone()).or_insert(0) += 1;
        }
        selected.push(id);
    }
    Ok(selected)
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

#[cfg(test)]
mod config_scheduling_tests {
    use super::*;
    use crate::fleet_config::parse_fleet_config;
    use crate::pitem::PItem;

    fn item(id: &str, resources: &[&str]) -> PItem {
        PItem {
            id: id.into(),
            desc: id.into(),
            touches: vec![],
            depends_on: vec![],
            resources: resources.iter().map(|s| (*s).to_string()).collect(),
            estimate_secs: None,
            acceptance: vec![],
            artifacts: vec![],
            skippable: None,
            human_gate: false,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        }
    }

    // [REQ-047] Acceptance scenario: a custom:db resource declared in
    // fleet.yaml with concurrency 1 → the scheduler detects the conflict and
    // refuses to dispatch two contending P-items concurrently. The earlier id
    // wins this pass; the second waits until the resource frees.
    #[test]
    fn custom_db_concurrency_one_serialises_contenders() {
        let cfg = parse_fleet_config(
            "resources:\n  custom:db:\n    concurrency: 1\n",
        )
        .unwrap();
        let plan = DagPlan::from_items(vec![
            item("P1", &["custom:db"]),
            item("P2", &["custom:db"]),
            item("P3", &["build"]),
        ]);
        let ready = dispatchable_with_config(&plan, &cfg, &HashMap::new()).unwrap();
        // P1 takes custom:db; P2 must wait; P3 (build) runs in parallel.
        assert!(ready.contains(&"P1".to_string()), "P1 should dispatch: {ready:?}");
        assert!(!ready.contains(&"P2".to_string()), "P2 must NOT run with P1: {ready:?}");
        assert!(ready.contains(&"P3".to_string()), "P3 (build) is disjoint: {ready:?}");
    }

    // [REQ-047] With concurrency 2 the same custom:db resource becomes a
    // counting semaphore: both contenders may run at once. This proves the
    // cap actually drives the decision (vs. always-mutex).
    #[test]
    fn custom_db_concurrency_two_allows_two_holders() {
        let cfg = parse_fleet_config(
            "resources:\n  custom:db:\n    concurrency: 2\n",
        )
        .unwrap();
        let plan = DagPlan::from_items(vec![
            item("P1", &["custom:db"]),
            item("P2", &["custom:db"]),
            item("P3", &["custom:db"]),
        ]);
        let ready = dispatchable_with_config(&plan, &cfg, &HashMap::new()).unwrap();
        // Two holders fit; the third must wait.
        assert!(ready.contains(&"P1".to_string()), "{ready:?}");
        assert!(ready.contains(&"P2".to_string()), "{ready:?}");
        assert!(!ready.contains(&"P3".to_string()), "third holder must wait: {ready:?}");
    }

    // [REQ-047] currently_held (resources taken by already-running workers) is
    // honoured: if custom:db is already held at its cap, no new contender runs.
    #[test]
    fn respects_externally_held_resource() {
        let cfg = parse_fleet_config(
            "resources:\n  custom:db:\n    concurrency: 1\n",
        )
        .unwrap();
        let plan = DagPlan::from_items(vec![item("P1", &["custom:db"])]);
        let mut held = HashMap::new();
        held.insert("custom:db".to_string(), 1u32);
        let ready = dispatchable_with_config(&plan, &cfg, &held).unwrap();
        assert!(ready.is_empty(), "custom:db already held at cap: {ready:?}");
    }

    // [REQ-047] Undeclared / built-in resources fall back to mutex (cap 1),
    // matching the core scheduler when no fleet.yaml override exists.
    #[test]
    fn builtin_resource_defaults_to_mutex() {
        let cfg = FleetConfig::default();
        let plan = DagPlan::from_items(vec![
            item("P1", &["build"]),
            item("P2", &["build"]),
        ]);
        let ready = dispatchable_with_config(&plan, &cfg, &HashMap::new()).unwrap();
        assert_eq!(ready, vec!["P1".to_string()]);
    }
}
