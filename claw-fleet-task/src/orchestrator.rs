//! Deterministic task orchestrator (the task-subsystem rebuild's core).
//!
//! Replaces the old LLM-master: instead of a Claude session driving dispatch /
//! acceptance / merge through CLI calls, the `fleet-task` process owns a plain
//! state-machine loop. Each [`Orchestrator::step`] advances one P-item by one
//! transition; the process calls it in a poll loop until the plan is complete.
//!
//! Side effects are kept behind two seams so the state machine itself is pure
//! logic and unit-testable with fakes:
//! - [`OrchestratorHost`] — provision+spawn a worker, poll whether a spawned
//!   session has exited, and merge+reap a finished P-item's worktree.
//! - [`ReviewGate`] — judge whether a finished worker actually met the P-item's
//!   acceptance criteria (the `/goal`-style review). P6 implements this with an
//!   isolated review session; tests inject a deterministic fake.
//!
//! Per-P-item lifecycle the loop drives:
//! `WaitDeps --dispatch--> Running --worker exits--> Reviewing
//!   --review achieved--> (merge+reap) Done | WaitHumanGate
//!   --review rejected--> Failed(ReviewRejected) (+ propagate_skip)`

use crate::pitem::{FailReason, PItem, PItemId, PItemStatus};
use crate::task::{get_task, task_write_lock, write_task_atomic, E2eOutcome, Task, TaskStatus};

/// The review session's verdict on a finished P-item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewVerdict {
    /// `true` when the worker's output actually met the acceptance criteria.
    pub achieved: bool,
    /// When `!achieved`, the concrete gaps the review found (fed back to the
    /// user / future redispatch). Empty when achieved.
    pub gaps: Vec<String>,
}

/// The `/goal`-style review seam. P6 implements this by spawning an isolated
/// review session; tests inject a deterministic fake.
pub trait ReviewGate {
    fn review(&self, task: &Task, p_item: &PItem) -> Result<ReviewVerdict, String>;
}

/// Side-effecting operations the orchestrator delegates to its host
/// (`fleet-task`'s `LocalHost`): worker spawn, liveness poll, and merge+reap.
pub trait OrchestratorHost {
    /// Provision the P-item's worktree and spawn its worker session. Returns
    /// the worker session id (stamped onto the P-item as `agent_session_id`).
    fn dispatch_worker(&self, task: &Task, p_item_id: &str) -> Result<String, String>;
    /// Has the spawned session's subprocess exited?
    fn session_finished(&self, session_id: &str) -> bool;
    /// Merge the P-item's worktree branch back into the task branch and reap
    /// the worktree. Called only after the review passed.
    fn merge_and_reap(&self, task: &Task, p_item_id: &str) -> Result<(), String>;
    /// Run the task-level e2e command (from `fleet.yaml`'s `verify.e2e`) against
    /// the finished task branch. `Ok(None)` when no e2e command is configured
    /// (→ task proceeds straight to `AwaitingAcceptance`); `Ok(Some(outcome))`
    /// when it ran (outcome carries pass/fail). Default: no e2e configured.
    fn run_task_e2e(&self, _task: &Task) -> Result<Option<E2eOutcome>, String> {
        Ok(None)
    }
}

/// Outcome of a single [`Orchestrator::step`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrchestratorStep {
    /// Spawned a worker for the named P-item (now `Running`).
    Dispatched(PItemId),
    /// The named P-item's worker exited; it is now `Reviewing`.
    WorkerFinished(PItemId),
    /// Review passed and the P-item was merged + flipped `Done`.
    Merged(PItemId),
    /// Review passed but the P-item has a human gate → parked `WaitHumanGate`.
    Gated(PItemId),
    /// Review rejected the P-item → `Failed(ReviewRejected)`; downstream
    /// P-items were skipped. Carries the gaps the review reported.
    Rejected(PItemId, Vec<String>),
    /// Nothing to do this tick (a worker is still running, or the task is not
    /// in a drivable state). Caller should sleep and retry.
    Idle,
    /// Every P-item is terminal; the task moved to `AwaitingAcceptance` (P7).
    PlanComplete,
    /// Plan finished but the task-level e2e command failed. The task is kept
    /// OUT of `AwaitingAcceptance` (stays `Running`) with the failure recorded
    /// on `task.e2e`. Carries the e2e gaps.
    E2eFailed(Vec<String>),
}

/// Drives one task. Cheap to construct; holds only the task id.
pub struct Orchestrator {
    task_id: String,
}

impl Orchestrator {
    pub fn new(task_id: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
        }
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    /// Advance the task by one transition. Non-blocking: when a worker is still
    /// running it returns [`OrchestratorStep::Idle`] rather than waiting.
    pub fn step(
        &self,
        host: &dyn OrchestratorHost,
        gate: &dyn ReviewGate,
    ) -> Result<OrchestratorStep, String> {
        // Serialise the whole transition so a concurrent reader never observes
        // a half-applied state change.
        let lock = task_write_lock(&self.task_id);
        let _g = lock.lock().expect("task write mutex poisoned");

        let mut task = get_task(&self.task_id)?;
        if !matches!(task.status, TaskStatus::Running) {
            return Ok(OrchestratorStep::Idle);
        }

        // 1. Is a P-item mid-review? Run the review gate and resolve it.
        if let Some(id) = active_with_status(&task, PItemStatus::Reviewing) {
            return self.resolve_review(&mut task, &id, host, gate);
        }

        // 2. Is a P-item Running? Check whether its worker has exited.
        if let Some(id) = active_with_status(&task, PItemStatus::Running) {
            let finished = task
                .plan
                .get(&id)
                .and_then(|p| p.agent_session_id.as_deref())
                .map(|sid| host.session_finished(sid))
                // No session id stamped → treat as finished so we don't wedge.
                .unwrap_or(true);
            if !finished {
                return Ok(OrchestratorStep::Idle);
            }
            if let Some(item) = task.plan.get_mut(&id) {
                item.status = PItemStatus::Reviewing;
            }
            write_task_atomic(&task)?;
            return Ok(OrchestratorStep::WorkerFinished(id));
        }

        // 3. Nothing active. Dispatch the next unblocked P-item, if any.
        if let Some(id) = task.plan.unblocked_items().into_iter().next() {
            let session_id = host.dispatch_worker(&task, &id)?;
            if let Some(item) = task.plan.get_mut(&id) {
                item.status = PItemStatus::Running;
                item.agent_session_id = Some(session_id);
                item.started_at = Some(now());
            }
            write_task_atomic(&task)?;
            return Ok(OrchestratorStep::Dispatched(id));
        }

        // 4. Nothing active, nothing dispatchable. If the plan is finished, run
        //    the task-level e2e gate (if configured) before parking the task in
        //    AwaitingAcceptance (P7 routes the final user gate).
        if task.is_plan_finished() {
            // If a prior tick already recorded a FAILED e2e, don't re-run and
            // don't auto-accept — the task stays Running with the failure
            // visible. (A passing e2e falls through to AwaitingAcceptance.)
            if matches!(&task.e2e, Some(o) if !o.passed) {
                return Ok(OrchestratorStep::Idle);
            }
            // Run e2e once (only when not yet recorded).
            if task.e2e.is_none() {
                if let Some(outcome) = host.run_task_e2e(&task)? {
                    let passed = outcome.passed;
                    let gaps = outcome.gaps.clone();
                    task.e2e = Some(outcome);
                    write_task_atomic(&task)?;
                    if !passed {
                        // Stay Running; the failure is recorded on task.e2e.
                        return Ok(OrchestratorStep::E2eFailed(gaps));
                    }
                }
            }
            task.status = TaskStatus::AwaitingAcceptance;
            write_task_atomic(&task)?;
            return Ok(OrchestratorStep::PlanComplete);
        }

        // Everything left is blocked (e.g. all remaining items poisoned by a
        // failure that propagate_skip already settled, or an empty plan).
        Ok(OrchestratorStep::Idle)
    }

    /// Run the review gate for a `Reviewing` P-item and apply the verdict.
    fn resolve_review(
        &self,
        task: &mut Task,
        id: &str,
        host: &dyn OrchestratorHost,
        gate: &dyn ReviewGate,
    ) -> Result<OrchestratorStep, String> {
        let item = task
            .plan
            .get(id)
            .ok_or_else(|| format!("p-item {id} vanished mid-review"))?
            .clone();
        let verdict = gate.review(task, &item)?;

        if verdict.achieved {
            // Merge the worker's branch back + reap its worktree, then settle
            // the P-item: gated items wait for the user, the rest go Done.
            host.merge_and_reap(task, id)?;
            let gated = item.human_gate;
            if let Some(p) = task.plan.get_mut(id) {
                p.completed_at = Some(now());
                p.status = if gated {
                    PItemStatus::WaitHumanGate
                } else {
                    PItemStatus::Done
                };
            }
            write_task_atomic(task)?;
            return Ok(if gated {
                OrchestratorStep::Gated(id.to_string())
            } else {
                OrchestratorStep::Merged(id.to_string())
            });
        }

        // Rejected: fail the P-item and skip everything it poisons. Persist the
        // gaps onto the P-item (covers both an LLM verdict and a mechanical-gate
        // rejection — both arrive here as `verdict.gaps`) so the UI can explain
        // the red node instead of dropping the reason into a transient step.
        if let Some(p) = task.plan.get_mut(id) {
            p.status = PItemStatus::Failed(FailReason::ReviewRejected);
            p.completed_at = Some(now());
            p.failure_gaps = verdict.gaps.clone();
        }
        task.plan.propagate_skip();
        write_task_atomic(task)?;
        Ok(OrchestratorStep::Rejected(id.to_string(), verdict.gaps))
    }
}

/// Id of the first P-item (sorted) currently in `status`. The orchestrator
/// drives one P-item at a time, so at most one is ever Running/Reviewing.
fn active_with_status(task: &Task, status: PItemStatus) -> Option<PItemId> {
    let mut ids: Vec<&PItemId> = task
        .plan
        .items
        .iter()
        .filter(|(_, p)| p.status == status)
        .map(|(id, _)| id)
        .collect();
    ids.sort();
    ids.first().map(|s| (*s).clone())
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::fleet_home_lock;
    use crate::pitem::AcceptanceCriterion;
    use crate::plan::DagPlan;
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::path::Path;

    // ── FLEET_HOME isolation so get_task/write_task hit a temp ~/.fleet ────────
    struct HomeOverride {
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }
    impl HomeOverride {
        fn new(tmp: &Path) -> Self {
            let lock = fleet_home_lock();
            let prev = std::env::var_os("FLEET_HOME");
            std::env::set_var("FLEET_HOME", tmp);
            HomeOverride { prev, _lock: lock }
        }
    }
    impl Drop for HomeOverride {
        fn drop(&mut self) {
            match &self.prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
    }

    /// Fake host: scripts which sessions are "finished" and records dispatch /
    /// merge calls so tests can assert the orchestration drove real effects.
    struct FakeHost {
        finished: RefCell<HashSet<String>>,
        dispatched: RefCell<Vec<String>>,
        merged: RefCell<Vec<String>>,
        next_sid: RefCell<u32>,
        /// What `run_task_e2e` returns. `None` → no e2e configured.
        e2e: RefCell<Option<E2eOutcome>>,
    }
    impl FakeHost {
        fn new() -> Self {
            Self {
                finished: RefCell::new(HashSet::new()),
                dispatched: RefCell::new(Vec::new()),
                merged: RefCell::new(Vec::new()),
                next_sid: RefCell::new(0),
                e2e: RefCell::new(None),
            }
        }
        fn mark_finished(&self, sid: &str) {
            self.finished.borrow_mut().insert(sid.to_string());
        }
        fn set_e2e(&self, outcome: E2eOutcome) {
            *self.e2e.borrow_mut() = Some(outcome);
        }
    }
    impl OrchestratorHost for FakeHost {
        fn dispatch_worker(&self, _task: &Task, p_item_id: &str) -> Result<String, String> {
            let mut n = self.next_sid.borrow_mut();
            *n += 1;
            let sid = format!("sid-{p_item_id}-{n}");
            self.dispatched.borrow_mut().push(p_item_id.to_string());
            Ok(sid)
        }
        fn session_finished(&self, session_id: &str) -> bool {
            self.finished.borrow().contains(session_id)
        }
        fn merge_and_reap(&self, _task: &Task, p_item_id: &str) -> Result<(), String> {
            self.merged.borrow_mut().push(p_item_id.to_string());
            Ok(())
        }
        fn run_task_e2e(&self, _task: &Task) -> Result<Option<E2eOutcome>, String> {
            Ok(self.e2e.borrow().clone())
        }
    }

    /// Review gate that returns a fixed verdict, optionally keyed per P-item.
    struct FakeGate {
        default_achieved: bool,
        reject: HashSet<String>,
    }
    impl FakeGate {
        fn always_pass() -> Self {
            Self { default_achieved: true, reject: HashSet::new() }
        }
        fn reject_only(ids: &[&str]) -> Self {
            Self {
                default_achieved: true,
                reject: ids.iter().map(|s| s.to_string()).collect(),
            }
        }
    }
    impl ReviewGate for FakeGate {
        fn review(&self, _task: &Task, p_item: &PItem) -> Result<ReviewVerdict, String> {
            let achieved = if self.reject.contains(&p_item.id) {
                false
            } else {
                self.default_achieved
            };
            Ok(ReviewVerdict {
                achieved,
                gaps: if achieved { vec![] } else { vec!["missing tests".into()] },
            })
        }
    }

    fn pitem(id: &str, deps: &[&str], human_gate: bool) -> PItem {
        PItem {
            id: id.into(),
            desc: id.into(),
            touches: vec![],
            depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
            acceptance: vec![AcceptanceCriterion::Builds],
            human_gate,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
            failure_gaps: Vec::new(),
        }
    }

    fn running_task(items: Vec<PItem>) -> Task {
        let mut t = Task::drafting("t1".into(), "proj".into(), "demo".into(), 0);
        t.status = TaskStatus::Running;
        t.task_branch = Some("fleet/demo".into());
        t.plan = DagPlan::from_items(items);
        write_task_atomic(&t).unwrap();
        t
    }

    fn status_of(id: &str) -> PItemStatus {
        get_task("t1").unwrap().plan.get(id).unwrap().status.clone()
    }

    #[test]
    fn happy_path_dispatch_review_merge_to_done() {
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = HomeOverride::new(tmp.path());
        running_task(vec![pitem("a", &[], false)]);
        let host = FakeHost::new();
        let gate = FakeGate::always_pass();
        let orch = Orchestrator::new("t1");

        // 1. dispatch the worker.
        assert_eq!(orch.step(&host, &gate).unwrap(), OrchestratorStep::Dispatched("a".into()));
        assert_eq!(status_of("a"), PItemStatus::Running);
        assert_eq!(host.dispatched.borrow().as_slice(), ["a"]);

        // 2. worker still running → Idle.
        assert_eq!(orch.step(&host, &gate).unwrap(), OrchestratorStep::Idle);
        assert_eq!(status_of("a"), PItemStatus::Running);

        // 3. worker exits → Reviewing.
        let sid = get_task("t1").unwrap().plan.get("a").unwrap().agent_session_id.clone().unwrap();
        host.mark_finished(&sid);
        assert_eq!(orch.step(&host, &gate).unwrap(), OrchestratorStep::WorkerFinished("a".into()));
        assert_eq!(status_of("a"), PItemStatus::Reviewing);

        // 4. review passes → merge + Done.
        assert_eq!(orch.step(&host, &gate).unwrap(), OrchestratorStep::Merged("a".into()));
        assert_eq!(status_of("a"), PItemStatus::Done);
        assert_eq!(host.merged.borrow().as_slice(), ["a"]);

        // 5. plan finished → AwaitingAcceptance.
        assert_eq!(orch.step(&host, &gate).unwrap(), OrchestratorStep::PlanComplete);
        assert!(matches!(get_task("t1").unwrap().status, TaskStatus::AwaitingAcceptance));
    }

    /// Drive a single P-item "a" through dispatch → review → merge → Done so the
    /// next `step()` hits the plan-finished (e2e) branch.
    fn drive_single_item_to_done(orch: &Orchestrator, host: &FakeHost, gate: &FakeGate) {
        orch.step(host, gate).unwrap(); // dispatch
        let sid = get_task("t1").unwrap().plan.get("a").unwrap().agent_session_id.clone().unwrap();
        host.mark_finished(&sid);
        orch.step(host, gate).unwrap(); // → Reviewing
        orch.step(host, gate).unwrap(); // → Merged + Done
        assert_eq!(status_of("a"), PItemStatus::Done);
    }

    #[test]
    fn e2e_pass_proceeds_to_awaiting_acceptance() {
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = HomeOverride::new(tmp.path());
        running_task(vec![pitem("a", &[], false)]);
        let host = FakeHost::new();
        host.set_e2e(E2eOutcome {
            command: "e2e.sh".into(),
            passed: true,
            gaps: vec![],
            ran_at: 1,
        });
        let gate = FakeGate::always_pass();
        let orch = Orchestrator::new("t1");

        drive_single_item_to_done(&orch, &host, &gate);

        // Plan finished → e2e runs + passes → AwaitingAcceptance, outcome recorded.
        assert_eq!(orch.step(&host, &gate).unwrap(), OrchestratorStep::PlanComplete);
        let t = get_task("t1").unwrap();
        assert!(matches!(t.status, TaskStatus::AwaitingAcceptance));
        assert_eq!(t.e2e.as_ref().map(|o| o.passed), Some(true));
    }

    #[test]
    fn e2e_fail_keeps_task_out_of_acceptance() {
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = HomeOverride::new(tmp.path());
        running_task(vec![pitem("a", &[], false)]);
        let host = FakeHost::new();
        host.set_e2e(E2eOutcome {
            command: "e2e.sh".into(),
            passed: false,
            gaps: vec!["e2e exited 1".into()],
            ran_at: 1,
        });
        let gate = FakeGate::always_pass();
        let orch = Orchestrator::new("t1");

        drive_single_item_to_done(&orch, &host, &gate);

        // Plan finished → e2e runs + FAILS → E2eFailed, task stays Running.
        assert_eq!(
            orch.step(&host, &gate).unwrap(),
            OrchestratorStep::E2eFailed(vec!["e2e exited 1".into()])
        );
        let t = get_task("t1").unwrap();
        assert!(matches!(t.status, TaskStatus::Running), "must NOT auto-accept on e2e fail");
        assert_eq!(t.e2e.as_ref().map(|o| o.passed), Some(false));

        // A subsequent tick must NOT re-run e2e nor auto-accept — it idles.
        assert_eq!(orch.step(&host, &gate).unwrap(), OrchestratorStep::Idle);
        assert!(matches!(get_task("t1").unwrap().status, TaskStatus::Running));
    }

    #[test]
    fn human_gate_parks_instead_of_done() {
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = HomeOverride::new(tmp.path());
        running_task(vec![pitem("a", &[], true)]);
        let host = FakeHost::new();
        let gate = FakeGate::always_pass();
        let orch = Orchestrator::new("t1");

        orch.step(&host, &gate).unwrap(); // dispatch
        let sid = get_task("t1").unwrap().plan.get("a").unwrap().agent_session_id.clone().unwrap();
        host.mark_finished(&sid);
        orch.step(&host, &gate).unwrap(); // → Reviewing
        assert_eq!(orch.step(&host, &gate).unwrap(), OrchestratorStep::Gated("a".into()));
        assert_eq!(status_of("a"), PItemStatus::WaitHumanGate);
        // Merge still happened — the gate is about the user's sign-off, not the merge.
        assert_eq!(host.merged.borrow().as_slice(), ["a"]);
    }

    #[test]
    fn review_rejection_fails_and_skips_downstream() {
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = HomeOverride::new(tmp.path());
        // b depends on a; a will be rejected so b must be skipped.
        running_task(vec![pitem("a", &[], false), pitem("b", &["a"], false)]);
        let host = FakeHost::new();
        let gate = FakeGate::reject_only(&["a"]);
        let orch = Orchestrator::new("t1");

        orch.step(&host, &gate).unwrap(); // dispatch a
        let sid = get_task("t1").unwrap().plan.get("a").unwrap().agent_session_id.clone().unwrap();
        host.mark_finished(&sid);
        orch.step(&host, &gate).unwrap(); // a → Reviewing
        let rejected = orch.step(&host, &gate).unwrap(); // review rejects a
        assert_eq!(rejected, OrchestratorStep::Rejected("a".into(), vec!["missing tests".into()]));
        assert_eq!(status_of("a"), PItemStatus::Failed(FailReason::ReviewRejected));
        // The rejection gaps are persisted onto the P-item (A: UI can explain it).
        assert_eq!(
            get_task("t1").unwrap().plan.get("a").unwrap().failure_gaps,
            vec!["missing tests".to_string()]
        );
        // a never merged.
        assert!(host.merged.borrow().is_empty());
        // b was poisoned → Skipped, never dispatched.
        assert_eq!(status_of("b"), PItemStatus::Skipped);
        // Plan is now all-terminal → next step completes.
        assert_eq!(orch.step(&host, &gate).unwrap(), OrchestratorStep::PlanComplete);
    }

    #[test]
    fn dispatches_in_dependency_order() {
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = HomeOverride::new(tmp.path());
        running_task(vec![pitem("a", &[], false), pitem("b", &["a"], false)]);
        let host = FakeHost::new();
        let gate = FakeGate::always_pass();
        let orch = Orchestrator::new("t1");

        // Only 'a' is unblocked first.
        assert_eq!(orch.step(&host, &gate).unwrap(), OrchestratorStep::Dispatched("a".into()));
        // While 'a' runs, nothing else dispatches.
        assert_eq!(orch.step(&host, &gate).unwrap(), OrchestratorStep::Idle);
        // Finish + pass 'a'.
        let sa = get_task("t1").unwrap().plan.get("a").unwrap().agent_session_id.clone().unwrap();
        host.mark_finished(&sa);
        orch.step(&host, &gate).unwrap(); // a → Reviewing
        orch.step(&host, &gate).unwrap(); // a Merged
        // Now 'b' unblocks and dispatches.
        assert_eq!(orch.step(&host, &gate).unwrap(), OrchestratorStep::Dispatched("b".into()));
        assert_eq!(host.dispatched.borrow().as_slice(), ["a", "b"]);
    }

    #[test]
    fn paused_task_is_idle() {
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = HomeOverride::new(tmp.path());
        let mut t = running_task(vec![pitem("a", &[], false)]);
        t.status = TaskStatus::Paused;
        write_task_atomic(&t).unwrap();
        let host = FakeHost::new();
        let gate = FakeGate::always_pass();
        let orch = Orchestrator::new("t1");
        assert_eq!(orch.step(&host, &gate).unwrap(), OrchestratorStep::Idle);
        // Nothing dispatched while paused.
        assert!(host.dispatched.borrow().is_empty());
    }
}
