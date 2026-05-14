//! Worktree per item — provisions one isolated git worktree per dispatched
//! P-item so workers can build/test without trampling each other, and the
//! master can serialize fast-forward merges back to the task branch.
//!
//! V2 (worktree-and-auto-merge PRD §Phase 1) replaces V1's "all workers
//! share `project.workspace`, ordered by `touches`" model.
//!
//! Layout: `<FLEET_HOME>/.fleet/worktrees/<task_id>/<p_item_id>/`
//! Branch: `fleet/<task_id>/<p_item_id>` (created by `git worktree add -b`).

use std::path::{Path, PathBuf};
use std::process::Command;

/// Root directory under `~/.fleet/` for all P-item worktrees.
pub fn worktree_root() -> Result<PathBuf, String> {
    crate::session::get_fleet_dir()
        .map(|h| h.join("worktrees"))
        .ok_or_else(|| "home directory not resolvable".to_string())
}

/// Computed path for a P-item's worktree. Does not check existence.
pub fn worktree_path(task_id: &str, p_item_id: &str) -> Result<PathBuf, String> {
    Ok(worktree_root()?.join(task_id).join(p_item_id))
}

/// Fleet's branch name for this P-item's worktree.
pub fn worktree_branch(task_id: &str, p_item_id: &str) -> String {
    format!("fleet/{task_id}/{p_item_id}")
}

/// Provision a fresh worktree for `(task_id, p_item_id)` rooted at
/// `task_branch` inside `project_workspace`. Returns the worktree path.
///
/// Idempotent: if the worktree path already exists and is registered with
/// git, returns it unchanged. If the path is an orphan (exists on disk but
/// not registered) it gets wiped and re-provisioned.
pub fn provision(
    project_workspace: &Path,
    task_branch: &str,
    task_id: &str,
    p_item_id: &str,
) -> Result<PathBuf, String> {
    let target = worktree_path(task_id, p_item_id)?;
    if target.exists() {
        // Compare via canonicalized paths — git records absolute, symlink-
        // resolved paths (e.g. macOS `/var/folders/...` ↔ `/private/var/...`).
        let target_canon = std::fs::canonicalize(&target).unwrap_or_else(|_| target.clone());
        let listed = git_worktree_list(project_workspace)?;
        let already_registered = listed.iter().any(|p| {
            std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) == target_canon
        });
        if already_registered {
            return Ok(target);
        }
        std::fs::remove_dir_all(&target)
            .map_err(|e| format!("remove orphan worktree dir {}: {e}", target.display()))?;
    }
    // A previous failed provision may have left the branch behind without a
    // worktree. Wipe it so `worktree add -b` doesn't fail with "already exists".
    let branch = worktree_branch(task_id, p_item_id);
    let _ = Command::new("git")
        .arg("-C")
        .arg(project_workspace)
        .args(["branch", "-D", &branch])
        .output();
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create worktree parent {}: {e}", parent.display()))?;
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(project_workspace)
        .args(["worktree", "add", "-b", &branch])
        .arg(&target)
        .arg(task_branch)
        .output()
        .map_err(|e| format!("git worktree add: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(target)
}

/// Tear down a P-item's worktree and delete its fleet branch. Best effort —
/// missing worktree or missing branch is not an error.
pub fn reap(
    project_workspace: &Path,
    task_id: &str,
    p_item_id: &str,
) -> Result<(), String> {
    let target = worktree_path(task_id, p_item_id)?;
    let branch = worktree_branch(task_id, p_item_id);
    if target.exists() {
        let out = Command::new("git")
            .arg("-C")
            .arg(project_workspace)
            .args(["worktree", "remove", "--force"])
            .arg(&target)
            .output()
            .map_err(|e| format!("git worktree remove: {e}"))?;
        if !out.status.success() {
            // git may refuse if the path isn't registered. Fall back to
            // plain rm so we don't leak disk.
            std::fs::remove_dir_all(&target).ok();
        }
    }
    // Always sweep stale .git/worktrees metadata.
    let _ = Command::new("git")
        .arg("-C")
        .arg(project_workspace)
        .args(["worktree", "prune"])
        .output();
    let _ = Command::new("git")
        .arg("-C")
        .arg(project_workspace)
        .args(["branch", "-D", &branch])
        .output();
    Ok(())
}

/// List the worktree directories fleet has provisioned for `task_id`. Order
/// is filesystem-sorted; callers don't rely on it semantically.
pub fn list_for_task(task_id: &str) -> Result<Vec<PathBuf>, String> {
    let root = worktree_root()?.join(task_id);
    if !root.exists() {
        return Ok(vec![]);
    }
    let mut out = vec![];
    for entry in std::fs::read_dir(&root)
        .map_err(|e| format!("read {}: {e}", root.display()))?
    {
        let entry = entry.map_err(|e| format!("read worktree entry: {e}"))?;
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            out.push(entry.path());
        }
    }
    out.sort();
    Ok(out)
}

/// Outcome of `merge_back` for a P-item's worktree branch into the task
/// branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// Fast-forward applied `commits_merged` commits from the worker.
    FastForwarded { commits_merged: usize },
    /// Branches diverged but git's auto-merger resolved cleanly into a
    /// new merge commit on `task_branch`.
    AutoMerged { commits_merged: usize },
    /// Worker made no commits relative to `task_branch`. Nothing to merge.
    NoChanges,
    /// Real 3-way merge conflict — git left conflict markers in the
    /// listed files; merge was aborted to keep the workspace clean. The
    /// LLM mediator (Phase 2) takes `files` and produces resolved content.
    Conflict {
        files: Vec<ConflictSpec>,
        reason: String,
    },
}

/// 3-way merge inputs for a single conflicted file. `path` is workspace-
/// relative. `base` is the common ancestor blob; `ours` is the task-branch
/// version at merge time; `theirs` is the worker branch version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictSpec {
    pub path: PathBuf,
    pub base: String,
    pub ours: String,
    pub theirs: String,
}

/// Merge the P-item's worktree branch back into `task_branch` inside
/// `project_workspace`. Phase 1: fast-forward only.
pub fn merge_back(
    project_workspace: &Path,
    task_branch: &str,
    task_id: &str,
    p_item_id: &str,
) -> Result<MergeOutcome, String> {
    let branch = worktree_branch(task_id, p_item_id);
    // If the worker branch doesn't exist (e.g. the master marked a P-item
    // Done without ever dispatching a worker), there's nothing to merge.
    let exists = Command::new("git")
        .arg("-C")
        .arg(project_workspace)
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .status()
        .map_err(|e| format!("git show-ref: {e}"))?;
    if !exists.success() {
        return Ok(MergeOutcome::NoChanges);
    }
    let count = git_revlist_count(project_workspace, task_branch, &branch)?;
    if count == 0 {
        return Ok(MergeOutcome::NoChanges);
    }
    // Ensure task_branch is the checked-out HEAD in the main workspace.
    let co = Command::new("git")
        .arg("-C")
        .arg(project_workspace)
        .args(["checkout", task_branch])
        .output()
        .map_err(|e| format!("git checkout: {e}"))?;
    if !co.status.success() {
        return Err(format!(
            "git checkout {task_branch} failed: {}",
            String::from_utf8_lossy(&co.stderr).trim()
        ));
    }
    // First try a fast-forward. Cheapest, most common path: worker branched
    // off task_branch and nothing else moved.
    let ff = Command::new("git")
        .arg("-C")
        .arg(project_workspace)
        .args(["merge", "--ff-only", &branch])
        .output()
        .map_err(|e| format!("git merge --ff-only: {e}"))?;
    if ff.status.success() {
        return Ok(MergeOutcome::FastForwarded {
            commits_merged: count,
        });
    }
    // Fall back to a real 3-way merge — let git try to auto-resolve.
    let merge = Command::new("git")
        .arg("-C")
        .arg(project_workspace)
        .args([
            "merge",
            "--no-ff",
            "--no-edit",
            "-m",
            &format!("Merge worktree {branch}"),
            &branch,
        ])
        .output()
        .map_err(|e| format!("git merge: {e}"))?;
    if merge.status.success() {
        return Ok(MergeOutcome::AutoMerged {
            commits_merged: count,
        });
    }
    // Conflict — collect specs then abort to leave workspace clean.
    let files = collect_conflict_specs(project_workspace)?;
    let stderr_msg = String::from_utf8_lossy(&merge.stderr).trim().to_string();
    let _ = Command::new("git")
        .arg("-C")
        .arg(project_workspace)
        .args(["merge", "--abort"])
        .output();
    Ok(MergeOutcome::Conflict {
        files,
        reason: stderr_msg,
    })
}

/// Re-attempt the merge using LLM-mediated resolutions for each conflict.
///
/// Takes `(path, resolved_content)` tuples (the mediator's output, but
/// this function doesn't care where they came from — useful for tests
/// that don't want to invoke an LLM). Steps:
/// 1. Re-run `git merge --no-ff --no-commit` of the worker branch in the
///    main workspace. Expected to exit non-zero due to conflicts.
/// 2. For each resolution, overwrite the conflicted file with the
///    resolved content and `git add` it.
/// 3. Verify no unmerged entries remain (`git ls-files -u`); abort and
///    surface an error if any are left over.
/// 4. Commit the merge with a "via Sonnet mediation" message.
///
/// Returns `MergeOutcome::AutoMerged` on success. The caller (master /
/// `mark_done`) treats this exactly like a clean auto-merge.
pub fn apply_resolutions(
    project_workspace: &Path,
    task_id: &str,
    p_item_id: &str,
    resolutions: &[(PathBuf, String)],
) -> Result<MergeOutcome, String> {
    let branch = worktree_branch(task_id, p_item_id);
    // Re-do the merge. Exits non-zero on conflict — that's expected; we
    // catch it after this call and resolve.
    let _ = Command::new("git")
        .arg("-C")
        .arg(project_workspace)
        .args([
            "merge",
            "--no-ff",
            "--no-commit",
            "-m",
            &format!("Merge worktree {branch} (LLM-mediated)"),
            &branch,
        ])
        .output()
        .map_err(|e| format!("git merge --no-commit: {e}"))?;
    // Write each resolution onto the working tree, then stage it.
    for (rel_path, content) in resolutions {
        let abs = project_workspace.join(rel_path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create dir {}: {e}", parent.display()))?;
        }
        std::fs::write(&abs, content.as_bytes())
            .map_err(|e| format!("write {}: {e}", rel_path.display()))?;
        let add = Command::new("git")
            .arg("-C")
            .arg(project_workspace)
            .args(["add", "--"])
            .arg(rel_path)
            .output()
            .map_err(|e| format!("git add {}: {e}", rel_path.display()))?;
        if !add.status.success() {
            let _ = Command::new("git")
                .arg("-C")
                .arg(project_workspace)
                .args(["merge", "--abort"])
                .output();
            return Err(format!(
                "git add {} failed: {}",
                rel_path.display(),
                String::from_utf8_lossy(&add.stderr).trim()
            ));
        }
    }
    // Any conflicts still unmerged? Abort and tell the caller.
    let ls = Command::new("git")
        .arg("-C")
        .arg(project_workspace)
        .args(["ls-files", "-u"])
        .output()
        .map_err(|e| format!("git ls-files -u: {e}"))?;
    if !ls.stdout.is_empty() {
        let _ = Command::new("git")
            .arg("-C")
            .arg(project_workspace)
            .args(["merge", "--abort"])
            .output();
        return Err(format!(
            "mediation incomplete — {} file(s) still unmerged after apply",
            String::from_utf8_lossy(&ls.stdout)
                .lines()
                .filter_map(|l| l.split('\t').nth(1))
                .collect::<std::collections::HashSet<_>>()
                .len()
        ));
    }
    let commit = Command::new("git")
        .arg("-C")
        .arg(project_workspace)
        .args([
            "commit",
            "--no-edit",
            "-m",
            &format!("auto-merge: {p_item_id} via Sonnet mediation"),
        ])
        .output()
        .map_err(|e| format!("git commit: {e}"))?;
    if !commit.status.success() {
        return Err(format!(
            "git commit (mediated merge) failed: {}",
            String::from_utf8_lossy(&commit.stderr).trim()
        ));
    }
    Ok(MergeOutcome::AutoMerged { commits_merged: 1 })
}

/// Read the index's stages 1/2/3 for every conflicted path. `git ls-files
/// -u` lists unmerged entries; we then `git show :<stage>:<path>` to get
/// each side. Returns an empty vec when there are no conflicts.
fn collect_conflict_specs(workspace: &Path) -> Result<Vec<ConflictSpec>, String> {
    let ls = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["ls-files", "-u"])
        .output()
        .map_err(|e| format!("git ls-files -u: {e}"))?;
    if !ls.status.success() {
        return Err(format!(
            "git ls-files -u failed: {}",
            String::from_utf8_lossy(&ls.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&ls.stdout);
    let mut paths: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for line in stdout.lines() {
        // Format: `<mode> <sha> <stage>\t<path>`.
        if let Some(tab) = line.find('\t') {
            paths.insert(line[tab + 1..].to_string());
        }
    }
    let mut specs = Vec::with_capacity(paths.len());
    for path in paths {
        let base = git_show_stage(workspace, 1, &path).unwrap_or_default();
        let ours = git_show_stage(workspace, 2, &path).unwrap_or_default();
        let theirs = git_show_stage(workspace, 3, &path).unwrap_or_default();
        specs.push(ConflictSpec {
            path: PathBuf::from(&path),
            base,
            ours,
            theirs,
        });
    }
    Ok(specs)
}

fn git_show_stage(workspace: &Path, stage: u8, path: &str) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["show", &format!(":{stage}:{path}")])
        .output()
        .map_err(|e| format!("git show :{stage}:{path}: {e}"))?;
    if !out.status.success() {
        // A missing stage (e.g. file added on one side, didn't exist on the
        // other) is a legitimate state for delete/modify conflicts. Return
        // empty so the mediator can reason about it.
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Count of commits in `head` that are not in `base` (`git rev-list --count base..head`).
fn git_revlist_count(workspace: &Path, base: &str, head: &str) -> Result<usize, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["rev-list", "--count", &format!("{base}..{head}")])
        .output()
        .map_err(|e| format!("git rev-list: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git rev-list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    s.parse::<usize>()
        .map_err(|e| format!("parse rev-list count {s:?}: {e}"))
}

/// Garbage-collect worktree directories whose task or P-item is no longer
/// live according to the caller-supplied `alive` set. Each entry in `alive`
/// is `(task_id, running_p_item_ids)` — anything missing or absent from
/// that map gets its directory removed.
///
/// Best-effort filesystem cleanup. We don't run `git worktree prune` in
/// the parent repo here because we generally don't know which project a
/// dead task belonged to. Lingering `.git/worktrees/<name>/` metadata is
/// harmless and gets cleared by the next legitimate `git worktree remove`
/// or by `provision`'s orphan-branch recovery.
pub fn gc_stale(alive: &[(String, Vec<String>)]) -> Result<Vec<PathBuf>, String> {
    let root = worktree_root()?;
    if !root.exists() {
        return Ok(vec![]);
    }
    let alive_by_task: std::collections::HashMap<&str, &[String]> = alive
        .iter()
        .map(|(t, ps)| (t.as_str(), ps.as_slice()))
        .collect();
    let mut reaped = vec![];
    for task_entry in std::fs::read_dir(&root)
        .map_err(|e| format!("read {}: {e}", root.display()))?
    {
        let task_entry = task_entry.map_err(|e| format!("read worktree task entry: {e}"))?;
        let task_id = task_entry.file_name().to_string_lossy().into_owned();
        let task_path = task_entry.path();
        if !task_path.is_dir() {
            continue;
        }
        let running_ps = alive_by_task.get(task_id.as_str()).copied();
        let mut keep_task_dir = false;
        for p_entry in std::fs::read_dir(&task_path)
            .map_err(|e| format!("read {}: {e}", task_path.display()))?
        {
            let p_entry = p_entry.map_err(|e| format!("read p-item entry: {e}"))?;
            let p_id = p_entry.file_name().to_string_lossy().into_owned();
            let p_path = p_entry.path();
            if !p_path.is_dir() {
                continue;
            }
            let alive_here = running_ps
                .map(|ps| ps.iter().any(|s| s == &p_id))
                .unwrap_or(false);
            if alive_here {
                keep_task_dir = true;
                continue;
            }
            match std::fs::remove_dir_all(&p_path) {
                Ok(()) => reaped.push(p_path),
                Err(e) => {
                    eprintln!(
                        "gc_stale: failed to remove orphan worktree {}: {e}",
                        p_path.display()
                    );
                    keep_task_dir = true;
                }
            }
        }
        if !keep_task_dir {
            let _ = std::fs::remove_dir(&task_path);
        }
    }
    Ok(reaped)
}

/// Parse `git worktree list --porcelain` to enumerate registered worktree
/// paths (including the main one).
fn git_worktree_list(workspace: &Path) -> Result<Vec<PathBuf>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .map_err(|e| format!("git worktree list: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git worktree list failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let paths = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("worktree ").map(PathBuf::from))
        .collect();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

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

    fn run_git(path: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo(path: &Path, branch: &str) {
        fs::create_dir_all(path).unwrap();
        run_git(path, &["init", "-q", "-b", branch]);
        run_git(path, &["config", "user.email", "test@example.com"]);
        run_git(path, &["config", "user.name", "Test"]);
        fs::write(path.join("README.md"), "init\n").unwrap();
        run_git(path, &["add", "README.md"]);
        run_git(path, &["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn worktree_path_lives_under_fleet_home() {
        let _g = crate::session::fleet_home_lock();
        let tmp = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp.path());
        let p = worktree_path("task-x", "p1").unwrap();
        assert_eq!(p, tmp.path().join(".fleet/worktrees/task-x/p1"));
    }

    #[test]
    fn worktree_branch_format() {
        assert_eq!(worktree_branch("my-task", "p1"), "fleet/my-task/p1");
        assert_eq!(worktree_branch("v2-feat", "p10"), "fleet/v2-feat/p10");
    }

    #[test]
    fn provision_creates_worktree_with_branch_and_initial_content() {
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        let path = provision(repo.path(), "main", "task-a", "p1").unwrap();
        assert!(path.exists(), "worktree dir should exist");
        // In a worktree, .git is a file pointing back to the main repo.
        assert!(path.join(".git").exists(), ".git marker should exist");
        assert!(
            path.join("README.md").exists(),
            "checked-out content should be present"
        );

        // Branch should be `fleet/task-a/p1`.
        let head = Command::new("git")
            .arg("-C")
            .arg(&path)
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&head.stdout).trim(),
            "fleet/task-a/p1"
        );
    }

    #[test]
    fn provision_is_idempotent_when_worktree_already_registered() {
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        let p1 = provision(repo.path(), "main", "task-a", "p1").unwrap();
        let p2 = provision(repo.path(), "main", "task-a", "p1").unwrap();
        assert_eq!(p1, p2);
    }

    #[test]
    fn provision_recovers_from_orphan_branch() {
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        // Simulate a failed prior provision: branch exists, but no worktree.
        run_git(repo.path(), &["branch", "fleet/task-a/p1", "main"]);
        let path = provision(repo.path(), "main", "task-a", "p1").unwrap();
        assert!(path.exists());
    }

    #[test]
    fn provision_recovers_from_orphan_dir() {
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        // Simulate an orphan dir that git doesn't know about.
        let stale = worktree_path("task-a", "p1").unwrap();
        fs::create_dir_all(&stale).unwrap();
        fs::write(stale.join("garbage"), "x").unwrap();

        let path = provision(repo.path(), "main", "task-a", "p1").unwrap();
        assert_eq!(path, stale);
        assert!(path.join(".git").exists());
        assert!(!path.join("garbage").exists(), "orphan should be wiped");
    }

    #[test]
    fn reap_removes_worktree_dir_and_branch() {
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        let path = provision(repo.path(), "main", "task-a", "p1").unwrap();
        assert!(path.exists());

        reap(repo.path(), "task-a", "p1").unwrap();
        assert!(!path.exists());

        let branch_check = Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                "refs/heads/fleet/task-a/p1",
            ])
            .status()
            .unwrap();
        assert!(!branch_check.success(), "branch should be gone");
    }

    #[test]
    fn reap_is_noop_when_nothing_to_reap() {
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        reap(repo.path(), "ghost", "p1").unwrap();
    }

    #[test]
    fn list_for_task_returns_each_provisioned_worktree() {
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        let _ = provision(repo.path(), "main", "task-a", "p1").unwrap();
        let _ = provision(repo.path(), "main", "task-a", "p2").unwrap();
        let _ = provision(repo.path(), "main", "task-b", "p1").unwrap();

        let listed = list_for_task("task-a").unwrap();
        assert_eq!(listed.len(), 2, "{listed:?}");
        let listed_b = list_for_task("task-b").unwrap();
        assert_eq!(listed_b.len(), 1);
    }

    #[test]
    fn list_for_task_returns_empty_for_unknown_task() {
        let _g = crate::session::fleet_home_lock();
        let tmp = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp.path());
        let listed = list_for_task("nope").unwrap();
        assert!(listed.is_empty());
    }

    fn commit_in_worktree(path: &Path, file: &str, contents: &str, msg: &str) {
        fs::write(path.join(file), contents).unwrap();
        run_git(path, &["add", file]);
        run_git(path, &["commit", "-q", "-m", msg]);
    }

    #[test]
    fn merge_back_no_changes_when_worker_did_nothing() {
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        let _ = provision(repo.path(), "main", "task-a", "p1").unwrap();
        let outcome = merge_back(repo.path(), "main", "task-a", "p1").unwrap();
        assert_eq!(outcome, MergeOutcome::NoChanges);
    }

    #[test]
    fn merge_back_fast_forwards_worker_commits() {
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        let wt = provision(repo.path(), "main", "task-a", "p1").unwrap();
        commit_in_worktree(&wt, "feature.txt", "hello\n", "add feature");
        commit_in_worktree(&wt, "feature.txt", "hello world\n", "polish");

        let outcome = merge_back(repo.path(), "main", "task-a", "p1").unwrap();
        assert_eq!(outcome, MergeOutcome::FastForwarded { commits_merged: 2 });

        // main now has feature.txt at its tip.
        assert!(repo.path().join("feature.txt").exists());
    }

    #[test]
    fn merge_back_reports_conflict_with_specs_when_same_line_changed() {
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        // Seed a shared file so both sides have a common ancestor.
        commit_in_worktree(repo.path(), "shared.txt", "original\n", "seed");

        let wt = provision(repo.path(), "main", "task-a", "p1").unwrap();
        // Worker modifies the shared line.
        commit_in_worktree(&wt, "shared.txt", "from worker\n", "worker version");
        // Main modifies the same line.
        commit_in_worktree(repo.path(), "shared.txt", "from main\n", "main version");

        let outcome = merge_back(repo.path(), "main", "task-a", "p1").unwrap();
        match outcome {
            MergeOutcome::Conflict { files, .. } => {
                assert_eq!(files.len(), 1, "expected one conflicted file");
                let spec = &files[0];
                assert_eq!(spec.path, PathBuf::from("shared.txt"));
                assert_eq!(spec.base, "original\n");
                assert_eq!(spec.ours, "from main\n");
                assert_eq!(spec.theirs, "from worker\n");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }

        // Workspace must be clean after merge_back (we abort the merge).
        let status = Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&status.stdout).trim().is_empty(),
            "workspace should be clean after aborted merge"
        );
    }

    #[test]
    fn gc_stale_removes_unknown_tasks() {
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        // Fabricate two orphan worktree dirs on disk; no git involvement.
        let root = worktree_root().unwrap();
        let a = root.join("dead-task/p1");
        let b = root.join("dead-task/p2");
        let c = root.join("live-task/p1");
        for p in [&a, &b, &c] {
            fs::create_dir_all(p).unwrap();
        }
        // Only live-task/p1 is in the alive set.
        let alive = vec![("live-task".to_string(), vec!["p1".to_string()])];
        let reaped = gc_stale(&alive).unwrap();
        assert_eq!(reaped.len(), 2);
        assert!(!a.exists());
        assert!(!b.exists());
        assert!(c.exists(), "alive worktree must be preserved");
    }

    #[test]
    fn gc_stale_keeps_only_currently_running_p_items() {
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let root = worktree_root().unwrap();
        let p1 = root.join("task-a/p1");
        let p2 = root.join("task-a/p2"); // alive
        let p3 = root.join("task-a/p3");
        for p in [&p1, &p2, &p3] {
            fs::create_dir_all(p).unwrap();
        }
        let alive = vec![("task-a".to_string(), vec!["p2".to_string()])];
        let reaped = gc_stale(&alive).unwrap();
        assert_eq!(reaped.len(), 2);
        assert!(!p1.exists());
        assert!(p2.exists());
        assert!(!p3.exists());
    }

    #[test]
    fn gc_stale_empty_alive_set_clears_everything() {
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let root = worktree_root().unwrap();
        let p = root.join("t/p1");
        fs::create_dir_all(&p).unwrap();
        let reaped = gc_stale(&[]).unwrap();
        assert_eq!(reaped.len(), 1);
        assert!(!p.exists());
    }

    #[test]
    fn gc_stale_noop_when_root_missing() {
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        // worktree_root doesn't exist yet — should not error.
        let reaped = gc_stale(&[]).unwrap();
        assert!(reaped.is_empty());
    }

    #[test]
    fn apply_resolutions_resolves_a_real_conflict_and_commits() {
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");
        commit_in_worktree(repo.path(), "shared.txt", "original\n", "seed");

        let wt = provision(repo.path(), "main", "task-a", "p1").unwrap();
        commit_in_worktree(&wt, "shared.txt", "from worker\n", "worker");
        commit_in_worktree(repo.path(), "shared.txt", "from main\n", "main");

        // First: merge_back returns Conflict (verified in another test). Now
        // simulate the mediator handing back a clean resolution.
        let resolutions = vec![(
            PathBuf::from("shared.txt"),
            "merged: worker + main\n".to_string(),
        )];
        let outcome = apply_resolutions(repo.path(), "task-a", "p1", &resolutions).unwrap();
        assert!(matches!(outcome, MergeOutcome::AutoMerged { .. }));

        // Workspace is clean.
        let status = Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&status.stdout).trim().is_empty(),
            "clean after mediated commit"
        );

        // File has the mediated content.
        let content = std::fs::read_to_string(repo.path().join("shared.txt")).unwrap();
        assert_eq!(content, "merged: worker + main\n");

        // HEAD is a merge commit (2 parents).
        let parents = Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["log", "-1", "--format=%P"])
            .output()
            .unwrap();
        let parent_count = String::from_utf8_lossy(&parents.stdout)
            .trim()
            .split_whitespace()
            .count();
        assert_eq!(parent_count, 2, "should be a merge commit");
    }

    #[test]
    fn apply_resolutions_errors_when_resolution_set_misses_a_file() {
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");
        commit_in_worktree(repo.path(), "a.txt", "a0\n", "seed a");
        commit_in_worktree(repo.path(), "b.txt", "b0\n", "seed b");

        let wt = provision(repo.path(), "main", "task-a", "p1").unwrap();
        commit_in_worktree(&wt, "a.txt", "worker a\n", "worker a");
        commit_in_worktree(&wt, "b.txt", "worker b\n", "worker b");
        commit_in_worktree(repo.path(), "a.txt", "main a\n", "main a");
        commit_in_worktree(repo.path(), "b.txt", "main b\n", "main b");

        // Resolve only a.txt; b.txt is left dangling — should error.
        let resolutions = vec![(PathBuf::from("a.txt"), "merged a\n".to_string())];
        let err = apply_resolutions(repo.path(), "task-a", "p1", &resolutions).unwrap_err();
        assert!(err.contains("still unmerged"), "got: {err}");

        // Aborted state — workspace clean.
        let status = Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["status", "--porcelain"])
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&status.stdout).trim().is_empty());
    }

    #[test]
    fn merge_back_auto_merges_disjoint_file_diverge() {
        // main moves on a different file → not fast-forwardable but git's
        // auto-merger resolves the divergence into a merge commit.
        let _g = crate::session::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        let wt = provision(repo.path(), "main", "task-a", "p1").unwrap();
        commit_in_worktree(&wt, "a.txt", "a\n", "worker a");
        commit_in_worktree(repo.path(), "b.txt", "b\n", "main b");

        let outcome = merge_back(repo.path(), "main", "task-a", "p1").unwrap();
        match outcome {
            MergeOutcome::AutoMerged { .. } => {}
            other => panic!("expected AutoMerged, got {other:?}"),
        }
        assert!(repo.path().join("a.txt").exists());
        assert!(repo.path().join("b.txt").exists());
    }
}
