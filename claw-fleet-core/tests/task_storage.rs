//! Integration test for the task storage layer (P3 acceptance).
//!
//! Sets `FLEET_HOME` to a temp dir so `~/.fleet/tasks/` resolves there, and
//! initializes a temp git repo so `start_task` has a real working tree to
//! create the `fleet/<slug>` branch in.
//!
//! All tests in this file serialize on a local `FLEET_HOME_LOCK` because
//! `real_home_dir()` reads the global env. The same pattern is used in
//! `claw-fleet-core/src/supervisor.rs::tests`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use claw_fleet_core::pitem::{AcceptanceCriterion, ArtifactKind, PItem, PItemStatus};
use claw_fleet_core::plan::DagPlan;
use claw_fleet_core::project::{self, ProjectInput};
use claw_fleet_core::task::{
    self, MediaKind, Task, TaskInput, TaskStatus,
};

static FLEET_HOME_LOCK: Mutex<()> = Mutex::new(());

struct FleetHomeOverride {
    prev: Option<std::ffi::OsString>,
}

impl FleetHomeOverride {
    fn new(tmp: &Path) -> Self {
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

fn fresh_tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "fleet-task-test-{}-{}-{}",
        tag,
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn git_init_repo(path: &Path) {
    // Best-effort: init + initial commit so we have a `main` ref and can
    // checkout new branches off it.
    Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(path)
        .status()
        .expect("git init");
    // Don't pollute user identity; set per-repo.
    Command::new("git")
        .args(["config", "user.email", "fleet@example.com"])
        .current_dir(path)
        .status()
        .expect("git config email");
    Command::new("git")
        .args(["config", "user.name", "Fleet Test"])
        .current_dir(path)
        .status()
        .expect("git config name");
    std::fs::write(path.join("README.md"), "test\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .status()
        .expect("git add");
    Command::new("git")
        .args(["commit", "-q", "-m", "init"])
        .current_dir(path)
        .status()
        .expect("git commit");
}

fn current_branch(repo: &Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(repo)
        .output()
        .expect("git rev-parse");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn full_lifecycle_create_update_material_start() {
    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("lifecycle");
    let _override = FleetHomeOverride::new(&home);

    // Create a project pointing at a fresh git workspace.
    let workspace = fresh_tmp("lifecycle-ws");
    git_init_repo(&workspace);
    let proj = project::create_project(ProjectInput {
        name: "demo".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(1),
            manual_review_all: None,
    })
    .expect("create_project");

    // create_task: file appears on disk.
    let task = task::create_task(TaskInput {
        project_id: proj.id.clone(),
        title: "Add Bookmarks UI".into(),
        description: "see Inbox materials".into(),
    })
    .expect("create_task");
    let json_path = task::task_json_path(&task.id).unwrap();
    assert!(json_path.exists(), "task json should exist at {json_path:?}");
    let materials_dir = task::task_materials_dir(&task.id).unwrap();
    assert!(materials_dir.exists(), "materials dir should be created");

    // get_task round-trips.
    let read_back = task::get_task(&task.id).expect("get_task");
    assert_eq!(read_back.id, task.id);
    assert_eq!(read_back.title, "Add Bookmarks UI");
    assert!(matches!(read_back.status, TaskStatus::Drafting));

    // list_tasks returns it; project filter works.
    let listed = task::list_tasks(Some(&proj.id));
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, task.id);
    assert!(task::list_tasks(Some("nonexistent")).is_empty());

    // add_task_material: file appears under materials/, task json references path.
    let bytes = b"hello\n";
    let stored = task::add_task_material(&task.id, "screenshot.png", bytes, MediaKind::Screenshot)
        .expect("add_task_material");
    assert!(stored.exists(), "material file should be persisted");
    assert!(
        stored.starts_with(&materials_dir),
        "material must live under materials dir: {stored:?}"
    );
    let after_material = task::get_task(&task.id).unwrap();
    assert_eq!(after_material.inbox_materials.len(), 1);

    // update_plan: persisted across get_task.
    let plan = DagPlan::from_items(vec![
        PItem {
            id: "p1".into(),
            desc: "first".into(),
            touches: vec![],
            depends_on: vec![],
            resources: vec![],
            estimate_secs: None,
            acceptance: vec![AcceptanceCriterion::Custom("true".into())],
            artifacts: vec![ArtifactKind::GitDiff],
            skippable: None,
            human_gate: false,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        },
        PItem {
            id: "p2".into(),
            desc: "second".into(),
            touches: vec![],
            depends_on: vec!["p1".into()],
            resources: vec![],
            estimate_secs: None,
            acceptance: vec![AcceptanceCriterion::Custom("true".into())],
            artifacts: vec![ArtifactKind::GitDiff],
            skippable: None,
            human_gate: false,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        },
    ]);
    task::update_plan(&task.id, plan.clone()).expect("update_plan");
    let after_plan = task::get_task(&task.id).unwrap();
    assert_eq!(after_plan.plan.items.len(), 2);
    assert!(after_plan.plan.items.contains_key("p1"));

    // start_task: branch is auto-created + task_branch set + status flips to Running.
    task::start_task(&task.id).expect("start_task");
    let after_start: Task = task::get_task(&task.id).unwrap();
    assert!(matches!(after_start.status, TaskStatus::Running));
    let branch = after_start.task_branch.as_ref().expect("task_branch set");
    assert!(branch.starts_with("fleet/"), "branch should be fleet/<slug>, got {branch}");
    assert_eq!(current_branch(&workspace), *branch);
    assert!(after_start.started_at.is_some());

    // Idempotency: a second start_task does not error.
    task::start_task(&task.id).expect("start_task idempotent");
}

#[test]
fn master_mutations_flip_p_item_statuses_correctly() {
    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("master-muts");
    let _override = FleetHomeOverride::new(&home);

    let workspace = fresh_tmp("master-muts-ws");
    git_init_repo(&workspace);
    let proj = project::create_project(ProjectInput {
        name: "demo".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(1),
            manual_review_all: None,
    })
    .unwrap();
    let t = task::create_task(TaskInput {
        project_id: proj.id,
        title: "demo".into(),
        description: String::new(),
    })
    .unwrap();
    let plan = DagPlan::from_items(vec![
        PItem {
            id: "p1".into(),
            desc: "first".into(),
            touches: vec![],
            depends_on: vec![],
            resources: vec![],
            estimate_secs: None,
            acceptance: vec![AcceptanceCriterion::Custom("true".into())],
            artifacts: vec![ArtifactKind::GitDiff],
            skippable: None,
            human_gate: false,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        },
        PItem {
            id: "p2".into(),
            desc: "second".into(),
            touches: vec![],
            depends_on: vec!["p1".into()],
            resources: vec![],
            estimate_secs: None,
            acceptance: vec![AcceptanceCriterion::Custom("true".into())],
            artifacts: vec![ArtifactKind::GitDiff],
            skippable: None,
            human_gate: false,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        },
    ]);
    task::update_plan(&t.id, plan).unwrap();

    // V2 worktree: dispatch_pitem requires a task_branch from start_task.
    let _ = task::start_task(&t.id);

    // dispatch_pitem flips WaitDeps → Running
    task::dispatch_pitem(&t.id, "p1").unwrap();
    let after_dispatch = task::get_task(&t.id).unwrap();
    assert!(matches!(after_dispatch.plan.get("p1").unwrap().status, PItemStatus::Running));
    assert!(after_dispatch.plan.get("p1").unwrap().started_at.is_some());

    // mark_done flips → Done with summary. Worker made no commits in its
    // worktree → MergeOutcome::NoChanges, but status still flips Done.
    let outcome = task::mark_done(&t.id, "p1", "OK shipped").unwrap();
    assert_eq!(outcome, claw_fleet_core::worktree::MergeOutcome::NoChanges);
    let after_done = task::get_task(&t.id).unwrap();
    assert!(matches!(after_done.plan.get("p1").unwrap().status, PItemStatus::Done));
    assert_eq!(
        after_done.plan.get("p1").unwrap().output_summary.as_deref(),
        Some("OK shipped")
    );

    // dispatch_pitem on Done item errors
    assert!(task::dispatch_pitem(&t.id, "p1").is_err());

    // mark_failed on p2 → Failed, propagate_skip empty (no downstream)
    let skipped = task::mark_failed(&t.id, "p2", claw_fleet_core::pitem::FailReason::BuildFailed)
        .unwrap();
    assert!(skipped.is_empty());
    let after_fail = task::get_task(&t.id).unwrap();
    assert!(matches!(
        after_fail.plan.get("p2").unwrap().status,
        PItemStatus::Failed(_)
    ));
}

#[test]
fn pause_resume_clear_full_cycle() {
    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("pause-cycle");
    let _override = FleetHomeOverride::new(&home);
    let workspace = fresh_tmp("pause-ws");
    git_init_repo(&workspace);
    let proj = project::create_project(ProjectInput {
        name: "demo".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(1),
            manual_review_all: None,
    })
    .unwrap();
    let t = task::create_task(TaskInput {
        project_id: proj.id,
        title: "demo".into(),
        description: String::new(),
    })
    .unwrap();
    task::start_task(&t.id).unwrap();

    // Running → Paused
    task::pause_task(&t.id).unwrap();
    assert!(matches!(task::get_task(&t.id).unwrap().status, TaskStatus::Paused));

    // Pause on Paused is idempotent
    task::pause_task(&t.id).unwrap();

    // Paused → Running
    task::resume_task(&t.id).unwrap();
    assert!(matches!(task::get_task(&t.id).unwrap().status, TaskStatus::Running));

    // Clear removes the json + materials dir
    task::clear_task(&t.id).unwrap();
    assert!(task::get_task(&t.id).is_err());
}

#[test]
fn start_task_enqueues_master_session_and_records_master_session_id() {
    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("master-spawn");
    let _override = FleetHomeOverride::new(&home);

    let workspace = fresh_tmp("master-spawn-ws");
    git_init_repo(&workspace);
    let proj = project::create_project(ProjectInput {
        name: "demo".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(1),
            manual_review_all: None,
    })
    .unwrap();
    let t = task::create_task(TaskInput {
        project_id: proj.id,
        title: "master spawn check".into(),
        description: "".into(),
    })
    .unwrap();

    task::start_task(&t.id).expect("start_task");
    let after = task::get_task(&t.id).unwrap();
    let master_sid = after
        .master_session_id
        .as_ref()
        .expect("master_session_id must be set after start_task");

    // Persisted FleetSession should be the Master kind, queued, with task linkage.
    let sessions = project::list_fleet_sessions();
    let s = sessions
        .iter()
        .find(|s| s.id == *master_sid)
        .expect("master session persisted");
    assert!(matches!(s.session_kind, project::SessionKind::Master));
    assert_eq!(s.task_id.as_deref(), Some(t.id.as_str()));
    assert_eq!(s.status, project::DEFAULT_COLUMN_QUEUED);
    assert!(s.system_prompt.is_some(), "system_prompt must be filled");
    assert_eq!(s.model.as_deref(), Some("claude-sonnet-4-6"));
    // Master is expedited so it doesn't compete with regular kanban queue.
    assert!(s.expedited);
}

#[test]
fn dispatch_pitem_enqueues_worker_session_and_records_agent_session_id() {
    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("worker-spawn");
    let _override = FleetHomeOverride::new(&home);

    let workspace = fresh_tmp("worker-spawn-ws");
    git_init_repo(&workspace);
    let proj = project::create_project(ProjectInput {
        name: "demo".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(1),
            manual_review_all: None,
    })
    .unwrap();
    let t = task::create_task(TaskInput {
        project_id: proj.id,
        title: "worker spawn check".into(),
        description: "".into(),
    })
    .unwrap();
    let plan = DagPlan::from_items(vec![PItem {
        id: "p1".into(),
        desc: "wire it up".into(),
        touches: vec![],
        depends_on: vec![],
        resources: vec![],
        estimate_secs: None,
        acceptance: vec![AcceptanceCriterion::Custom("true".into())],
        artifacts: vec![ArtifactKind::GitDiff],
        skippable: None,
        human_gate: false,
        status: PItemStatus::WaitDeps,
        agent_session_id: None,
        started_at: None,
        completed_at: None,
        output_summary: None,
    }]);
    task::update_plan(&t.id, plan).unwrap();

    // V2 worktree: dispatch_pitem requires a task_branch, set up by start_task.
    let _ = task::start_task(&t.id); // master enqueue may error in test env; branch + Running is what we need
    task::dispatch_pitem(&t.id, "p1").unwrap();
    let after = task::get_task(&t.id).unwrap();
    let worker_sid = after
        .plan
        .get("p1")
        .unwrap()
        .agent_session_id
        .as_ref()
        .expect("agent_session_id must be set after dispatch");

    let sessions = project::list_fleet_sessions();
    let s = sessions
        .iter()
        .find(|s| s.id == *worker_sid)
        .expect("worker session persisted");
    assert!(matches!(s.session_kind, project::SessionKind::Worker));
    assert_eq!(s.task_id.as_deref(), Some(t.id.as_str()));
    assert_eq!(s.p_item_id.as_deref(), Some("p1"));
    assert!(s.system_prompt.is_some());
    assert_eq!(s.model.as_deref(), Some("claude-sonnet-4-6"));
}

#[test]
fn clear_task_terminates_linked_sessions_and_marks_complete() {
    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("clear-terminate");
    let _override = FleetHomeOverride::new(&home);

    let workspace = fresh_tmp("clear-terminate-ws");
    git_init_repo(&workspace);
    let proj = project::create_project(ProjectInput {
        name: "demo".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(1),
            manual_review_all: None,
    })
    .unwrap();
    let t = task::create_task(TaskInput {
        project_id: proj.id,
        title: "terminate test".into(),
        description: "".into(),
    })
    .unwrap();
    task::start_task(&t.id).expect("start_task");
    let master_sid = task::get_task(&t.id).unwrap().master_session_id.unwrap();

    task::clear_task(&t.id).expect("clear_task");

    // task json gone.
    assert!(task::get_task(&t.id).is_err());
    // master FleetSession marked complete with task-cleared note.
    let sessions = project::list_fleet_sessions();
    let s = sessions.iter().find(|s| s.id == master_sid).expect("session still recorded");
    assert_eq!(s.status, project::DEFAULT_COLUMN_COMPLETE);
    assert!(s.note.as_deref().unwrap_or("").contains("task cleared"));
    assert!(s.pid.is_none());
}

#[test]
fn reconcile_flips_task_to_done_when_master_exited_and_plan_terminal() {
    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("reconcile");
    let _override = FleetHomeOverride::new(&home);

    let workspace = fresh_tmp("reconcile-ws");
    git_init_repo(&workspace);
    let proj = project::create_project(ProjectInput {
        name: "demo".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(1),
            manual_review_all: None,
    })
    .unwrap();
    let t = task::create_task(TaskInput {
        project_id: proj.id,
        title: "done check".into(),
        description: "".into(),
    })
    .unwrap();
    let plan = DagPlan::from_items(vec![PItem {
        id: "p1".into(),
        desc: "x".into(),
        touches: vec![],
        depends_on: vec![],
        resources: vec![],
        estimate_secs: None,
        acceptance: vec![AcceptanceCriterion::Custom("true".into())],
        artifacts: vec![ArtifactKind::GitDiff],
        skippable: None,
        human_gate: false,
        status: PItemStatus::WaitDeps,
        agent_session_id: None,
        started_at: None,
        completed_at: None,
        output_summary: None,
    }]);
    task::update_plan(&t.id, plan).unwrap();
    task::start_task(&t.id).expect("start_task");
    // Pretend the worker ran and master finished its audit.
    task::mark_done(&t.id, "p1", "ok").unwrap();
    let master_sid = task::get_task(&t.id).unwrap().master_session_id.unwrap();

    // Reconcile is a no-op while master FleetSession is still queued.
    let n = task::reconcile_task_completion().expect("reconcile");
    assert_eq!(n, 0);
    assert!(matches!(task::get_task(&t.id).unwrap().status, TaskStatus::Running));

    // Simulate master subprocess exit: mark its FleetSession complete + pid None.
    let mut sessions = project::list_fleet_sessions();
    for s in sessions.iter_mut() {
        if s.id == master_sid {
            s.status = project::DEFAULT_COLUMN_COMPLETE.into();
            s.pid = None;
        }
    }
    project::save_fleet_sessions(&sessions).unwrap();

    let n = task::reconcile_task_completion().expect("reconcile");
    assert_eq!(n, 1);
    let after = task::get_task(&t.id).unwrap();
    assert!(matches!(after.status, TaskStatus::Done));
    assert!(after.completed_at.is_some());
}

#[test]
fn start_task_picks_unique_branch_when_slug_collides() {
    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("dedupe");
    let _override = FleetHomeOverride::new(&home);

    let workspace = fresh_tmp("dedupe-ws");
    git_init_repo(&workspace);
    let proj = project::create_project(ProjectInput {
        name: "demo".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(1),
            manual_review_all: None,
    })
    .unwrap();

    // Pre-create `fleet/demo-task` so start_task must fall back to `-1`.
    let pre_status = Command::new("git")
        .args(["-C", workspace.to_str().unwrap(), "branch", "fleet/demo-task"])
        .status()
        .unwrap();
    assert!(pre_status.success(), "pre-create branch");

    let t = task::create_task(TaskInput {
        project_id: proj.id,
        title: "demo task".into(),
        description: String::new(),
    })
    .unwrap();
    task::start_task(&t.id).expect("start_task");
    let after = task::get_task(&t.id).unwrap();
    let branch = after.task_branch.unwrap();
    assert_eq!(branch, "fleet/demo-task-1");
}

// PRD `worktree-and-auto-merge` P6 — Concurrent dispatch e2e.
//
// Exercises the V2 worktree story end-to-end: dispatch two non-conflicting
// P-items, simulate workers committing to disjoint files in their own
// worktrees, then mark_done both. The task branch should pick up both
// commits via fast-forward, both worktrees should be reaped, and both
// `fleet/<task>/<p>` branches should be gone.
#[test]
fn concurrent_dispatch_isolates_workers_and_merges_back() {
    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("worktree-concurrent");
    let _override = FleetHomeOverride::new(&home);

    let workspace = fresh_tmp("worktree-concurrent-ws");
    git_init_repo(&workspace);
    let proj = project::create_project(ProjectInput {
        name: "concurrency".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(2),
        manual_review_all: None,
    })
    .unwrap();
    let t = task::create_task(TaskInput {
        project_id: proj.id,
        title: "two parallel pitems".into(),
        description: "".into(),
    })
    .unwrap();
    let plan = DagPlan::from_items(vec![
        PItem {
            id: "p1".into(),
            desc: "file_a".into(),
            touches: vec!["file_a.txt".into()],
            depends_on: vec![],
            resources: vec![],
            estimate_secs: None,
            acceptance: vec![AcceptanceCriterion::Custom("true".into())],
            artifacts: vec![ArtifactKind::GitDiff],
            skippable: None,
            human_gate: false,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        },
        PItem {
            id: "p2".into(),
            desc: "file_b".into(),
            touches: vec!["file_b.txt".into()],
            depends_on: vec![],
            resources: vec![],
            estimate_secs: None,
            acceptance: vec![AcceptanceCriterion::Custom("true".into())],
            artifacts: vec![ArtifactKind::GitDiff],
            skippable: None,
            human_gate: false,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        },
    ]);
    task::update_plan(&t.id, plan).unwrap();
    let _ = task::start_task(&t.id);

    // Dispatch both P-items. Worktrees provisioned in parallel.
    task::dispatch_pitem(&t.id, "p1").unwrap();
    task::dispatch_pitem(&t.id, "p2").unwrap();

    let wt1 = claw_fleet_core::worktree::worktree_path(&t.id, "p1").unwrap();
    let wt2 = claw_fleet_core::worktree::worktree_path(&t.id, "p2").unwrap();
    assert!(wt1.exists(), "p1 worktree must exist");
    assert!(wt2.exists(), "p2 worktree must exist");
    assert_ne!(wt1, wt2, "worktrees must be isolated paths");

    // Simulate worker 1 committing file_a in its worktree.
    fn run_git_in(path: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?} in {} failed: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    run_git_in(&wt1, &["config", "user.email", "p1@test"]);
    run_git_in(&wt1, &["config", "user.name", "P1"]);
    std::fs::write(wt1.join("file_a.txt"), "a\n").unwrap();
    run_git_in(&wt1, &["add", "file_a.txt"]);
    run_git_in(&wt1, &["commit", "-q", "-m", "p1: add file_a"]);

    // Simulate worker 2 committing file_b in its worktree.
    run_git_in(&wt2, &["config", "user.email", "p2@test"]);
    run_git_in(&wt2, &["config", "user.name", "P2"]);
    std::fs::write(wt2.join("file_b.txt"), "b\n").unwrap();
    run_git_in(&wt2, &["add", "file_b.txt"]);
    run_git_in(&wt2, &["commit", "-q", "-m", "p2: add file_b"]);

    // Master serializes the merges. p1 first — clean fast-forward.
    let outcome1 = task::mark_done(&t.id, "p1", "p1 done").unwrap();
    assert_eq!(
        outcome1,
        claw_fleet_core::worktree::MergeOutcome::FastForwarded { commits_merged: 1 }
    );
    assert!(!wt1.exists(), "p1 worktree should be reaped");

    // p2 next. p2 was branched off the same base; after p1 landed, p2's
    // branch is now diverged from task_branch but touches a disjoint file.
    // V2 merge_back falls back from ff-only to a real 3-way merge, which
    // git auto-resolves cleanly into an AutoMerged commit.
    let outcome2 = task::mark_done(&t.id, "p2", "p2 done").unwrap();
    assert!(
        matches!(
            outcome2,
            claw_fleet_core::worktree::MergeOutcome::AutoMerged { .. }
        ),
        "expected AutoMerged for disjoint-file divergence, got: {outcome2:?}"
    );
    assert!(!wt2.exists(), "p2 worktree should be reaped");

    let after = task::get_task(&t.id).unwrap();
    assert!(matches!(after.plan.get("p1").unwrap().status, PItemStatus::Done));
    assert!(matches!(after.plan.get("p2").unwrap().status, PItemStatus::Done));

    // Both files now live on the task branch.
    assert!(workspace.join("file_a.txt").exists());
    assert!(workspace.join("file_b.txt").exists());
}

// PRD `worktree-and-auto-merge` P13 — End-to-end real-conflict mediation.
//
// Forces a 3-way merge conflict between two workers, then drives the full
// Phase 2 flow: merge_back → mediate (with a fake LLM provider so the test
// doesn't need network) → apply_resolutions → verify task branch lands
// the mediated content. This is the path mark_done would take on a real
// run with Claude CLI installed.
#[test]
fn real_conflict_mediated_end_to_end_with_fake_provider() {
    use claw_fleet_core::llm_provider::{Completion, CompletionUsage, LlmModel, LlmProvider};
    use claw_fleet_core::merge_mediator::{mediate_with, MediationError};
    use claw_fleet_core::worktree::{
        apply_resolutions, merge_back, provision, MergeOutcome,
    };
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;

    struct FakeProvider {
        canned: Mutex<String>,
    }
    impl LlmProvider for FakeProvider {
        fn name(&self) -> &str { "fake" }
        fn display_name(&self) -> &str { "Fake" }
        fn is_available(&self) -> bool { true }
        fn list_models(&self) -> Vec<LlmModel> { vec![] }
        fn default_fast_model(&self) -> &str { "fast" }
        fn default_standard_model(&self) -> &str { "std" }
        fn complete(&self, _prompt: &str, _model: &str, _timeout: Duration) -> Option<Completion> {
            Some(Completion {
                text: format!(
                    "<resolved>\n{}\n</resolved>",
                    self.canned.lock().unwrap()
                ),
                usage: Some(CompletionUsage::default()),
            })
        }
    }

    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("mediator-e2e");
    let _override = FleetHomeOverride::new(&home);
    let workspace = fresh_tmp("mediator-e2e-ws");
    git_init_repo(&workspace);

    // Seed a shared file so both sides have a base ancestor.
    std::fs::write(workspace.join("shared.txt"), "v0\n").unwrap();
    Command::new("git").arg("-C").arg(&workspace).args(["add", "shared.txt"]).status().unwrap();
    Command::new("git").arg("-C").arg(&workspace).args(["commit", "-q", "-m", "seed shared"]).status().unwrap();

    let proj = project::create_project(ProjectInput {
        name: "mediator".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(2),
        manual_review_all: None,
    })
    .unwrap();
    let t = task::create_task(TaskInput {
        project_id: proj.id,
        title: "mediated conflict".into(),
        description: "".into(),
    })
    .unwrap();
    let _ = task::start_task(&t.id);
    let task_branch = task::get_task(&t.id).unwrap().task_branch.unwrap();

    // Worker p1 modifies shared.txt in its worktree.
    let wt = provision(&workspace, &task_branch, &t.id, "p1").unwrap();
    Command::new("git").arg("-C").arg(&wt).args(["config", "user.email", "p1@test"]).status().unwrap();
    Command::new("git").arg("-C").arg(&wt).args(["config", "user.name", "P1"]).status().unwrap();
    std::fs::write(wt.join("shared.txt"), "worker version\n").unwrap();
    Command::new("git").arg("-C").arg(&wt).args(["add", "shared.txt"]).status().unwrap();
    Command::new("git").arg("-C").arg(&wt).args(["commit", "-q", "-m", "worker"]).status().unwrap();

    // Task branch independently diverges on the SAME line.
    std::fs::write(workspace.join("shared.txt"), "main version\n").unwrap();
    Command::new("git").arg("-C").arg(&workspace).args(["add", "shared.txt"]).status().unwrap();
    Command::new("git").arg("-C").arg(&workspace).args(["commit", "-q", "-m", "main"]).status().unwrap();

    // Step 1 — merge_back returns Conflict with the 3-way specs.
    let outcome = merge_back(&workspace, &task_branch, &t.id, "p1").unwrap();
    let files = match outcome {
        MergeOutcome::Conflict { files, .. } => files,
        other => panic!("expected Conflict, got {other:?}"),
    };
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, PathBuf::from("shared.txt"));
    assert_eq!(files[0].base, "v0\n");
    assert_eq!(files[0].ours, "main version\n");
    assert_eq!(files[0].theirs, "worker version\n");

    // Step 2 — mediate via fake provider (substitutes Sonnet).
    let provider = FakeProvider {
        canned: Mutex::new("mediated: main + worker".to_string()),
    };
    let resolutions: Result<Vec<_>, MediationError> = mediate_with(&provider, &files);
    let resolutions = resolutions.unwrap();
    assert_eq!(resolutions.len(), 1);

    // Step 3 — apply resolutions; should produce an AutoMerged commit.
    let pairs: Vec<(PathBuf, String)> = resolutions
        .into_iter()
        .map(|m| (m.path, m.resolved_content))
        .collect();
    let final_outcome = apply_resolutions(&workspace, &t.id, "p1", &pairs).unwrap();
    assert!(matches!(final_outcome, MergeOutcome::AutoMerged { .. }));

    // Verify: file holds mediated content; HEAD is a merge commit.
    let content = std::fs::read_to_string(workspace.join("shared.txt")).unwrap();
    assert_eq!(content, "mediated: main + worker");
    let parents = Command::new("git")
        .arg("-C")
        .arg(&workspace)
        .args(["log", "-1", "--format=%P"])
        .output()
        .unwrap();
    let parent_count = String::from_utf8_lossy(&parents.stdout)
        .trim()
        .split_whitespace()
        .count();
    assert_eq!(parent_count, 2, "should be a merge commit");
}
