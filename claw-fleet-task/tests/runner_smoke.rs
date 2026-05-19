//! Integration smoke test for `TaskRunner`. Drives a fixture task through
//! dispatch with a mock `TaskLifecycleHost` so we don't depend on the
//! shared fleet-session table — proves the library is self-contained
//! enough for Phase 2's binary to drop in its own host.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use claw_fleet_task::master::MasterSpawnSpec;
use claw_fleet_task::paths::fleet_home_lock;
use claw_fleet_task::pitem::{PItem, PItemStatus};
use claw_fleet_task::plan::DagPlan;
use claw_fleet_task::runner::{RunnerStep, TaskLifecycleHost, TaskRunner};
use claw_fleet_task::task::{
    create_task, get_task, task_json_path, write_task_atomic, TaskInput, TaskStatus,
};
use claw_fleet_task::worker::WorkerSpawnSpec;

struct FleetHomeOverride {
    prev: Option<std::ffi::OsString>,
}

impl FleetHomeOverride {
    fn new(tmp: &std::path::Path) -> Self {
        let prev = std::env::var_os("FLEET_HOME");
        unsafe { std::env::set_var("FLEET_HOME", tmp) };
        FleetHomeOverride { prev }
    }
}

impl Drop for FleetHomeOverride {
    fn drop(&mut self) {
        unsafe {
            match &self.prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
    }
}

/// Records every host call so tests can assert dispatch order and content.
struct MockHost {
    workspace: PathBuf,
    spawned_workers: Mutex<Vec<(String, String)>>, // (task_id, p_item_id)
    next_worker_sid: Mutex<u32>,
}

impl MockHost {
    fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            spawned_workers: Mutex::new(Vec::new()),
            next_worker_sid: Mutex::new(0),
        }
    }
}

impl TaskLifecycleHost for MockHost {
    fn workspace_for_task(
        &self,
        _task: &claw_fleet_task::task::Task,
    ) -> Result<PathBuf, String> {
        Ok(self.workspace.clone())
    }

    fn enqueue_master(&self, _spec: &MasterSpawnSpec) -> Result<String, String> {
        unreachable!("smoke test does not exercise master enqueue")
    }

    fn enqueue_worker(&self, spec: &WorkerSpawnSpec) -> Result<String, String> {
        let mut sid = self.next_worker_sid.lock().unwrap();
        *sid += 1;
        let id = format!("worker-{}", *sid);
        self.spawned_workers
            .lock()
            .unwrap()
            .push((spec.task_id.clone(), spec.p_item_id.clone()));
        Ok(id)
    }

    fn pause_task_sessions(&self, _task_id: &str) -> Result<usize, String> {
        Ok(0)
    }

    fn resume_task_sessions(&self, _task_id: &str) -> Result<usize, String> {
        Ok(0)
    }

    fn terminate_task_sessions(&self, _task_id: &str) -> Result<usize, String> {
        Ok(0)
    }
}

fn pitem(id: &str, deps: &[&str]) -> PItem {
    PItem {
        id: id.into(),
        desc: format!("do {id}"),
        touches: vec![],
        depends_on: deps.iter().map(|s| s.to_string()).collect(),
        resources: vec![],
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

/// Initialise a real git repo at `path` with one empty commit + a `fleet/x`
/// branch so worktree::provision has something to work with.
fn init_repo_with_branch(path: &Path, branch: &str) {
    let repo = git2::Repository::init(path).expect("init repo");
    let sig = git2::Signature::now("Test", "test@example.com").expect("sig");
    let tree_oid = {
        let tb = repo.treebuilder(None).expect("treebuilder");
        tb.write().expect("tree write")
    };
    let tree = repo.find_tree(tree_oid).expect("find tree");
    let head_oid = repo
        .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .expect("initial commit");
    let head_commit = repo.find_commit(head_oid).expect("head commit");
    repo.branch(branch, &head_commit, false).expect("branch");
}

#[test]
fn runner_step_dispatches_unblocked_pitem_and_stamps_session_id() {
    let _g = fleet_home_lock();
    let home = tempfile::TempDir::new().unwrap();
    let ws = tempfile::TempDir::new().unwrap();
    let _override = FleetHomeOverride::new(home.path());

    init_repo_with_branch(ws.path(), "fleet/smoke");

    // Set up a Running task with one unblocked P-item.
    let mut task = create_task(TaskInput {
        project_id: "proj-1".into(),
        title: "smoke".into(),
        description: String::new(),
    })
    .unwrap();
    task.status = TaskStatus::Running;
    task.task_branch = Some("fleet/smoke".into());
    task.plan = DagPlan::from_items(vec![pitem("p1", &[]), pitem("p2", &["p1"])]);
    write_task_atomic(&task).unwrap();

    let host = MockHost::new(ws.path().to_path_buf());
    let runner = TaskRunner::new(task.id.clone(), &host);

    // First step: p1 is unblocked, p2 blocked on p1 → expect SpawnedWorker(p1).
    match runner.step().unwrap() {
        RunnerStep::SpawnedWorker(pid, sid) => {
            assert_eq!(pid, "p1");
            assert_eq!(sid, "worker-1");
        }
        other => panic!("expected SpawnedWorker(p1, ...), got {other:?}"),
    }

    // Plan should now record p1 Running + agent_session_id set.
    let reloaded = get_task(&task.id).unwrap();
    let p1 = reloaded.plan.get("p1").unwrap();
    assert!(matches!(p1.status, PItemStatus::Running));
    assert_eq!(p1.agent_session_id.as_deref(), Some("worker-1"));
    let p2 = reloaded.plan.get("p2").unwrap();
    assert!(matches!(p2.status, PItemStatus::WaitDeps));

    // Second step: p2 still blocked on p1 (which is Running, not Done) → Idle.
    assert_eq!(runner.step().unwrap(), RunnerStep::Idle);

    // Host recorded exactly one worker enqueue for (task, p1).
    let spawned = host.spawned_workers.lock().unwrap();
    assert_eq!(spawned.len(), 1);
    assert_eq!(spawned[0].0, task.id);
    assert_eq!(spawned[0].1, "p1");
}

#[test]
fn runner_step_reports_done_for_terminal_task() {
    let _g = fleet_home_lock();
    let home = tempfile::TempDir::new().unwrap();
    let ws = tempfile::TempDir::new().unwrap();
    let _override = FleetHomeOverride::new(home.path());

    let mut task = create_task(TaskInput {
        project_id: "p".into(),
        title: "t".into(),
        description: String::new(),
    })
    .unwrap();
    task.status = TaskStatus::Done;
    write_task_atomic(&task).unwrap();

    let host = MockHost::new(ws.path().to_path_buf());
    let runner = TaskRunner::new(task.id.clone(), &host);
    assert_eq!(runner.step().unwrap(), RunnerStep::Done);

    // Sanity: also ensure task_json_path resolves under the overridden FLEET_HOME.
    assert!(task_json_path(&task.id)
        .unwrap()
        .starts_with(home.path()));
}

#[test]
fn runner_step_reports_idle_for_drafting_task() {
    let _g = fleet_home_lock();
    let home = tempfile::TempDir::new().unwrap();
    let ws = tempfile::TempDir::new().unwrap();
    let _override = FleetHomeOverride::new(home.path());

    let task = create_task(TaskInput {
        project_id: "p".into(),
        title: "t".into(),
        description: String::new(),
    })
    .unwrap();
    assert!(matches!(task.status, TaskStatus::Drafting));

    let host = MockHost::new(ws.path().to_path_buf());
    let runner = TaskRunner::new(task.id.clone(), &host);
    assert_eq!(runner.step().unwrap(), RunnerStep::Idle);
}
