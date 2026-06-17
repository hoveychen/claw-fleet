//! Thin wrappers around `claw_fleet_task::actions::*` that bind the
//! supervisor-backed `SupervisorHost` so the existing
//! `claw_fleet_core::task::start_task(&id)` import paths from desktop /
//! fleet-cli continue to work without changes.
//!
//! Phase 2 moved the actual business logic into `claw_fleet_task::actions`
//! so the standalone `fleet-task` binary can share it via a binary-local
//! host. `reconcile_task_completion` stays here because it reads the legacy
//! `fleet-sessions.json` table that fleet-task explicitly doesn't use.

use std::path::PathBuf;

use claw_fleet_task::actions;
use claw_fleet_task::task::{
    get_task, list_tasks, task_write_lock, write_task_atomic, AcceptMode, TaskStatus,
};
use claw_fleet_task::worktree::{self, ConflictSpec, MergeOutcome};

use crate::lifecycle_host::SupervisorHost;

/// A merge-conflict resolver: maps each conflicting file to its resolved
/// content. Injectable so tests can avoid spawning a real LLM.
pub type MergeMediatorFn<'a> =
    dyn Fn(&[ConflictSpec]) -> Result<Vec<(PathBuf, String)>, String> + 'a;

pub fn start_task(task_id: &str) -> Result<(), String> {
    // Look up the project once so we can honour `manual_review_all` — the
    // host trait doesn't carry project metadata.
    let task = get_task(task_id)?;
    let project = crate::project::list_projects()
        .into_iter()
        .find(|p| p.id == task.project_id)
        .ok_or_else(|| format!("project {} not found for task {task_id}", task.project_id))?;
    let opts = actions::StartOpts {
        force_human_gate_all: project.manual_review_all,
    };
    actions::start_task(task_id, &SupervisorHost, opts)
}

pub fn dispatch_pitem(task_id: &str, p_item_id: &str) -> Result<(), String> {
    actions::dispatch_pitem(task_id, p_item_id, &SupervisorHost)
}

/// Reset a failed task-level e2e so the live orchestrator re-runs it.
pub fn rerun_task_e2e(task_id: &str) -> Result<(), String> {
    actions::rerun_task_e2e(task_id)
}

#[cfg(test)]
mod accept_merge_tests {
    use super::*;
    use claw_fleet_task::task::Task;
    use std::path::Path;
    use std::process::Command;

    struct HomeOverride {
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }
    impl HomeOverride {
        fn new(tmp: &Path) -> Self {
            let lock = claw_fleet_task::paths::fleet_home_lock();
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

    fn git(ws: &Path, args: &[&str]) {
        let out = Command::new("git").arg("-C").arg(ws).args(args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn write_file(ws: &Path, name: &str, content: &str) {
        std::fs::write(ws.join(name), content).unwrap();
    }

    fn init_ws(ws: &Path) {
        std::fs::create_dir_all(ws).unwrap();
        git(ws, &["init", "-q", "-b", "main"]);
        git(ws, &["config", "user.email", "t@t.co"]);
        git(ws, &["config", "user.name", "t"]);
        write_file(ws, "README.md", "init\n");
        git(ws, &["add", "-A"]);
        git(ws, &["commit", "-q", "-m", "init"]);
    }

    fn branch_exists(ws: &Path, b: &str) -> bool {
        Command::new("git").arg("-C").arg(ws)
            .args(["rev-parse", "--verify", &format!("refs/heads/{b}")])
            .output().unwrap().status.success()
    }

    fn make_task(home: &Path, ws: &Path) -> String {
        let mut t = Task::drafting("t-merge".into(), "p".into(), "demo".into(), 0);
        t.status = TaskStatus::AwaitingAcceptance;
        t.workspace = Some(ws.to_path_buf());
        t.task_branch = Some("fleet/x".into());
        t.base_branch = Some("main".into());
        let _ = home; // FLEET_HOME already set by HomeOverride
        write_task_atomic(&t).unwrap();
        t.id
    }

    #[test]
    fn merge_back_fast_forward_deletes_branch() {
        let home = tempfile::TempDir::new().unwrap();
        let _o = HomeOverride::new(home.path());
        let wsd = tempfile::TempDir::new().unwrap();
        let ws = wsd.path();
        init_ws(ws);
        git(ws, &["checkout", "-q", "-b", "fleet/x"]);
        write_file(ws, "feat.txt", "feature\n");
        git(ws, &["add", "-A"]);
        git(ws, &["commit", "-q", "-m", "feat"]);
        git(ws, &["checkout", "-q", "main"]);
        let id = make_task(home.path(), ws);

        // ff merge → mediator never called.
        let never = |_: &[ConflictSpec]| -> Result<Vec<(PathBuf, String)>, String> {
            panic!("mediator must not run on a clean merge")
        };
        merge_back_to_base(&id, &never).unwrap();
        assert!(ws.join("feat.txt").exists(), "base must carry the merged file");
        assert!(!branch_exists(ws, "fleet/x"), "task branch must be deleted after merge");
    }

    #[test]
    fn merge_back_conflict_uses_mediator_then_deletes_branch() {
        let home = tempfile::TempDir::new().unwrap();
        let _o = HomeOverride::new(home.path());
        let wsd = tempfile::TempDir::new().unwrap();
        let ws = wsd.path();
        init_ws(ws);
        write_file(ws, "c.txt", "base\n");
        git(ws, &["add", "-A"]);
        git(ws, &["commit", "-q", "-m", "seed"]);
        git(ws, &["checkout", "-q", "-b", "fleet/x"]);
        write_file(ws, "c.txt", "feature\n");
        git(ws, &["add", "-A"]);
        git(ws, &["commit", "-q", "-m", "feat side"]);
        git(ws, &["checkout", "-q", "main"]);
        write_file(ws, "c.txt", "mainside\n");
        git(ws, &["add", "-A"]);
        git(ws, &["commit", "-q", "-m", "main side"]);
        let id = make_task(home.path(), ws);

        let mediator = |files: &[ConflictSpec]| -> Result<Vec<(PathBuf, String)>, String> {
            assert!(files.iter().any(|f| f.path.ends_with("c.txt")));
            Ok(vec![(PathBuf::from("c.txt"), "resolved\n".to_string())])
        };
        merge_back_to_base(&id, &mediator).unwrap();
        assert_eq!(std::fs::read_to_string(ws.join("c.txt")).unwrap(), "resolved\n");
        assert!(!branch_exists(ws, "fleet/x"));
    }

    /// REAL LLM mediation — spawns `claude` to resolve an actual conflict.
    /// `#[ignore]`d so normal runs don't need claude/quota; run explicitly with
    /// `cargo test -p claw-fleet-core --ignored accept_merge_real`. Asserts the
    /// real `merge_mediator` produces a marker-free merge, deletes the branch,
    /// and the task reaches Done — the one thing fake-mediator tests can't prove.
    #[test]
    #[ignore = "spawns real claude; run with --ignored"]
    fn accept_merge_real_llm_resolves_conflict() {
        let home = tempfile::TempDir::new().unwrap();
        let _o = HomeOverride::new(home.path());
        let wsd = tempfile::TempDir::new().unwrap();
        let ws = wsd.path();
        init_ws(ws);
        write_file(ws, "greeting.txt", "hello world\n");
        git(ws, &["add", "-A"]);
        git(ws, &["commit", "-q", "-m", "seed"]);
        git(ws, &["checkout", "-q", "-b", "fleet/x"]);
        write_file(ws, "greeting.txt", "hello fleet\n");
        git(ws, &["add", "-A"]);
        git(ws, &["commit", "-q", "-m", "feat side"]);
        git(ws, &["checkout", "-q", "main"]);
        write_file(ws, "greeting.txt", "hi world\n");
        git(ws, &["add", "-A"]);
        git(ws, &["commit", "-q", "-m", "main side"]);
        let id = make_task(home.path(), ws);

        accept_task_with_mode(&id, AcceptMode::MergeBack).unwrap();

        let merged = std::fs::read_to_string(ws.join("greeting.txt")).unwrap();
        assert!(
            !merged.contains("<<<<<<<") && !merged.contains(">>>>>>>"),
            "real mediation must leave no conflict markers; got: {merged:?}"
        );
        assert!(!branch_exists(ws, "fleet/x"), "branch deleted after mediated merge");
        assert!(matches!(get_task(&id).unwrap().status, TaskStatus::Done));
    }

    #[test]
    fn keep_branch_mode_leaves_branch_and_flips_done() {
        let home = tempfile::TempDir::new().unwrap();
        let _o = HomeOverride::new(home.path());
        let wsd = tempfile::TempDir::new().unwrap();
        let ws = wsd.path();
        init_ws(ws);
        git(ws, &["branch", "fleet/x"]);
        let id = make_task(home.path(), ws);

        accept_task_with_mode(&id, AcceptMode::KeepBranch).unwrap();
        assert!(branch_exists(ws, "fleet/x"), "KeepBranch must not delete the branch");
        assert!(matches!(get_task(&id).unwrap().status, TaskStatus::Done));
    }
}

pub fn pause_task(task_id: &str) -> Result<(), String> {
    actions::pause_task(task_id, &SupervisorHost)
}

pub fn resume_task(task_id: &str) -> Result<(), String> {
    actions::resume_task(task_id, &SupervisorHost)
}

pub fn clear_task(task_id: &str) -> Result<(), String> {
    actions::clear_task(task_id, &SupervisorHost)
}

/// P7 final acceptance: user confirms a task parked in `AwaitingAcceptance`,
/// flipping it to `Done`. No host needed — the orchestrator already finished.
/// Back-compat entry point: accept without merging back (KeepBranch). Existing
/// callers keep their old behaviour until they pass an explicit mode.
pub fn accept_task(task_id: &str) -> Result<(), String> {
    actions::accept_task(task_id)
}

/// Accept a reviewed task with an explicit finishing mode.
/// - `KeepBranch`: just flip the task `Done`, leave `fleet/<slug>` for the user.
/// - `MergeBack`: merge `fleet/<slug>` into its base branch (LLM-mediated on
///   conflict via `merge_mediator`), delete the task branch, then flip `Done`.
///   The `Done` flip happens ONLY after a successful merge — a conflict the
///   mediator can't resolve surfaces as an error with the branch left intact.
pub fn accept_task_with_mode(task_id: &str, mode: AcceptMode) -> Result<(), String> {
    match mode {
        AcceptMode::KeepBranch => actions::accept_task(task_id),
        AcceptMode::MergeBack => {
            let mediator = |files: &[ConflictSpec]| -> Result<Vec<(PathBuf, String)>, String> {
                let resolved = crate::merge_mediator::mediate(files).map_err(|e| e.to_string())?;
                Ok(resolved
                    .into_iter()
                    .map(|m| (m.path, m.resolved_content))
                    .collect())
            };
            merge_back_to_base(task_id, &mediator)?;
            actions::accept_task(task_id)
        }
    }
}

/// Merge a finished task's `fleet/<slug>` branch into its base branch and delete
/// it. On a 3-way conflict, `mediator` is asked to resolve the files and the
/// merge is re-applied. Errors (leaving the branch intact) if the base can't be
/// resolved or mediation fails — the caller must NOT flip `Done` in that case.
pub fn merge_back_to_base(task_id: &str, mediator: &MergeMediatorFn) -> Result<(), String> {
    let task = get_task(task_id)?;
    let workspace = task
        .workspace
        .clone()
        .ok_or_else(|| format!("task {task_id} has no persisted workspace"))?;
    let task_branch = task
        .task_branch
        .clone()
        .ok_or_else(|| format!("task {task_id} has no task_branch"))?;
    let base = task
        .resolve_base_branch(&workspace)
        .ok_or_else(|| format!("could not resolve base branch for task {task_id}"))?;
    if base == task_branch {
        return Err(format!(
            "base branch equals task branch ({base}); refusing to merge"
        ));
    }
    let msg = format!("fleet: merge task {} ({task_branch}) into {base}", task.id);
    match worktree::merge_branch_into(&workspace, &task_branch, &base, &msg)? {
        MergeOutcome::Conflict { files, .. } => {
            let resolutions = mediator(&files)?;
            worktree::apply_branch_merge_resolutions(
                &workspace,
                &task_branch,
                &base,
                &format!("{msg} (via mediation)"),
                &resolutions,
            )?;
        }
        // FastForwarded / AutoMerged / NoChanges — nothing more to do.
        _ => {}
    }
    // Merge landed (or there was nothing to merge); we're on `base` now, so
    // deleting the task branch is safe. Best-effort: a failure here doesn't
    // undo a successful merge.
    let _ = worktree::delete_local_branch(&workspace, &task_branch);
    Ok(())
}

/// Reconcile Task status against its master FleetSession on disk. Called by
/// the supervisor tick: if the master session has exited AND the task's plan
/// is fully terminal, flip the task to Done. Returns the number of tasks
/// that transitioned.
///
/// Stays in core (rather than migrating to claw-fleet-task::actions) because
/// it consults the legacy `fleet-sessions.json` table — fleet-task's
/// LocalHost path tracks master exit via its in-process pid HashMap, so it
/// has no use for this function.
pub fn reconcile_task_completion() -> Result<usize, String> {
    let sessions = crate::project::list_fleet_sessions();
    let tasks = list_tasks(None);
    let mut transitioned = 0usize;
    for task in tasks {
        if !matches!(task.status, TaskStatus::Running) {
            continue;
        }
        let Some(master_sid) = task.master_session_id.as_deref() else {
            continue;
        };
        let Some(master) = sessions.iter().find(|s| s.id == master_sid) else {
            continue;
        };
        let master_finished =
            master.status == crate::project::DEFAULT_COLUMN_COMPLETE && master.pid.is_none();
        if !master_finished {
            continue;
        }
        if !task.is_plan_finished() {
            continue;
        }
        let lock = task_write_lock(&task.id);
        let _g = lock.lock().expect("task write mutex poisoned");
        let Ok(mut latest) = get_task(&task.id) else { continue };
        if matches!(latest.status, TaskStatus::Running) && latest.is_plan_finished() {
            latest.status = TaskStatus::Done;
            latest.completed_at = Some(chrono::Utc::now().timestamp());
            if write_task_atomic(&latest).is_ok() {
                transitioned += 1;
            }
        }
    }
    Ok(transitioned)
}
