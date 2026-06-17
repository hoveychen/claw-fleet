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

use git2::{
    build::CheckoutBuilder, BranchType, MergeAnalysis, Repository, WorktreeAddOptions,
    WorktreePruneOptions,
};

// libgit2 encodes the merge stage in bits 12-13 of `IndexEntry.flags`.
const GIT_INDEX_STAGE_SHIFT: u16 = 12;
const GIT_INDEX_STAGE_MASK: u16 = 0x3000;

fn entry_stage(e: &git2::IndexEntry) -> u16 {
    (e.flags & GIT_INDEX_STAGE_MASK) >> GIT_INDEX_STAGE_SHIFT
}

/// Root directory under `~/.fleet/` for all P-item worktrees.
pub fn worktree_root() -> Result<PathBuf, String> {
    crate::paths::get_fleet_dir()
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

/// libgit2-internal name for the worktree's `.git/worktrees/<name>/` metadata
/// dir. Must be unique per repo and contain no path separators.
fn worktree_meta_name(task_id: &str, p_item_id: &str) -> String {
    let sanitize = |s: &str| s.replace(['/', '\\', ':'], "_");
    format!("{}__{}", sanitize(task_id), sanitize(p_item_id))
}

/// Provision a fresh worktree for `(task_id, p_item_id)` rooted at
/// `task_branch` inside `project_workspace`. Returns the worktree path.
///
/// Idempotent: if the worktree path already exists and is registered with
/// git, returns it unchanged. If the path is an orphan (exists on disk but
/// not registered) it gets wiped and re-provisioned.
///
/// `provision` is the worktree-isolation entry point: it creates
/// one `git worktree add`-style isolated checkout per P-item, recovers from
/// orphaned dirs/branches left by a crashed prior run, and pairs with
/// `merge_back` (3-way merge back to the task branch) and `reap` (worktree
/// teardown) to manage the full lifecycle. Two P-items get independent
/// worktrees that don't tread on each other.
pub fn provision(
    project_workspace: &Path,
    task_branch: &str,
    task_id: &str,
    p_item_id: &str,
) -> Result<PathBuf, String> {
    let target = worktree_path(task_id, p_item_id)?;
    let branch = worktree_branch(task_id, p_item_id);
    if target.exists() {
        // Compare via canonicalized paths — libgit2 records absolute, symlink-
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
    let repo = open_repo(project_workspace)?;
    // A previous failed provision may have left the branch behind without a
    // worktree. Wipe it so the new worktree creation doesn't trip on a name
    // collision. Best-effort: not-found is fine.
    if let Ok(mut existing) = repo.find_branch(&branch, BranchType::Local) {
        existing
            .delete()
            .map_err(|e| format!("delete stale branch {branch}: {}", e.message()))?;
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create worktree parent {}: {e}", parent.display()))?;
    }
    // Create the worker branch off the task branch, then point a new worktree
    // at that branch's reference.
    let task_commit = repo
        .revparse_single(task_branch)
        .map_err(|e| format!("revparse {task_branch}: {}", e.message()))?
        .peel_to_commit()
        .map_err(|e| format!("peel {task_branch} to commit: {}", e.message()))?;
    let new_branch = repo
        .branch(&branch, &task_commit, false)
        .map_err(|e| format!("create branch {branch}: {}", e.message()))?;
    let new_ref = new_branch.into_reference();
    // libgit2 requires a worktree "name" usable as a directory name under
    // `.git/worktrees/`. It must be unique per repo and contain no slashes,
    // so derive it from `<task_id>__<p_item_id>` (sanitized).
    let wt_name = worktree_meta_name(task_id, p_item_id);
    let mut opts = WorktreeAddOptions::new();
    opts.reference(Some(&new_ref));
    repo.worktree(&wt_name, &target, Some(&opts))
        .map_err(|e| format!("create worktree {}: {}", target.display(), e.message()))?;
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
    let repo = open_repo(project_workspace)?;
    // Remove the working directory contents first; libgit2's prune doesn't
    // touch the working tree the way `git worktree remove --force` does.
    if target.exists() {
        std::fs::remove_dir_all(&target).ok();
    }
    // Drop the `.git/worktrees/<name>/` metadata. Try the canonical name we
    // set during provision; fall back to scanning all worktrees and matching
    // by path so legacy entries still get cleaned.
    let wt_name = worktree_meta_name(task_id, p_item_id);
    let mut pruned = false;
    if let Ok(wt) = repo.find_worktree(&wt_name) {
        wt.prune(Some(
            WorktreePruneOptions::new()
                .valid(true)
                .locked(true)
                .working_tree(true),
        ))
        .map_err(|e| format!("prune worktree {wt_name}: {}", e.message()))?;
        pruned = true;
    }
    if !pruned {
        if let Ok(names) = repo.worktrees() {
            let target_canon =
                std::fs::canonicalize(&target).unwrap_or_else(|_| target.clone());
            for name in names.iter().flatten() {
                if let Ok(wt) = repo.find_worktree(name) {
                    let wt_canon = std::fs::canonicalize(wt.path())
                        .unwrap_or_else(|_| wt.path().to_path_buf());
                    if wt_canon == target_canon {
                        let _ = wt.prune(Some(
                            WorktreePruneOptions::new()
                                .valid(true)
                                .locked(true)
                                .working_tree(true),
                        ));
                        break;
                    }
                }
            }
        }
    }
    // Drop the fleet branch if present; best-effort.
    if let Ok(mut b) = repo.find_branch(&branch, BranchType::Local) {
        let _ = b.delete();
    }
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

/// Build the identifying message for a fleet merge commit so the Auditor
/// can trace which P-item / session / worker produced it.
///
/// Every fleet-authored merge commit must carry identification in
/// the form `fleet: merged <p_item_id> from session <sid> worker <worker>`,
/// so `git log --oneline | grep fleet` surfaces all fleet merges and each
/// line is traceable back to its origin.
///
/// `worktree::merge_back` / `apply_resolutions` only receive
/// `(task_id, p_item_id)` from their callers — the worker/session id is not
/// threaded through those signatures (callers live outside this module's
/// touches). We therefore source the session id from the worker runtime's
/// `FLEET_SESSION_ID` env var and the worker id from `FLEET_WORKER_ID`,
/// falling back to the deterministic worktree branch name when neither is
/// set (e.g. unit tests, manual merges). The `<p_item_id>` and the worker
/// branch are always present, so the commit stays grep-able + traceable
/// even outside a live worker context.
fn fleet_merge_message(task_id: &str, p_item_id: &str, kind: &str) -> String {
    let sid = std::env::var("FLEET_SESSION_ID").unwrap_or_else(|_| "unknown".to_string());
    let worker = std::env::var("FLEET_WORKER_ID")
        .unwrap_or_else(|_| worktree_branch(task_id, p_item_id));
    format!("fleet: merged {p_item_id} from session {sid} worker {worker} ({kind})")
}

/// Outcome of `merge_back` for a P-item's worktree branch into the task
/// branch.
///
/// Exactly three "something happened" outcomes are modelled:
/// `FastForwarded` (linear advance), `AutoMerged` (git's auto-merger
/// resolved a divergence into a merge commit), and `Conflict` (a real
/// 3-way conflict that needs LLM mediation or human intervention). The
/// fourth `NoChanges` variant is the degenerate "worker committed nothing"
/// case, not a merge strategy. There is no recursive retry: `merge_back`
/// attempts FF→auto-merge once, and on `Conflict` the caller runs at most
/// one round of LLM mediation via `apply_resolutions`.
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
    let repo = open_repo(project_workspace)?;
    // If the worker branch doesn't exist (e.g. the master marked a P-item
    // Done without ever dispatching a worker), there's nothing to merge.
    let worker_branch = match repo.find_branch(&branch, BranchType::Local) {
        Ok(b) => b,
        Err(_) => return Ok(MergeOutcome::NoChanges),
    };
    let count = git_revlist_count(project_workspace, task_branch, &branch)?;
    if count == 0 {
        return Ok(MergeOutcome::NoChanges);
    }
    // Ensure task_branch is the checked-out HEAD in the main workspace.
    checkout_branch(&repo, task_branch)?;
    // Annotated commit for the merge analysis / 3-way merge.
    let worker_ref = worker_branch.into_reference();
    let annotated = repo
        .reference_to_annotated_commit(&worker_ref)
        .map_err(|e| format!("annotate {branch}: {}", e.message()))?;
    let (analysis, _pref) = repo
        .merge_analysis(&[&annotated])
        .map_err(|e| format!("merge analysis: {}", e.message()))?;
    // First try a fast-forward. Cheapest, most common path: worker branched
    // off task_branch and nothing else moved.
    if analysis.contains(MergeAnalysis::ANALYSIS_FASTFORWARD) {
        fast_forward_to(&repo, task_branch, annotated.id())?;
        return Ok(MergeOutcome::FastForwarded {
            commits_merged: count,
        });
    }
    // Fall back to a real 3-way merge — let libgit2 try to auto-resolve.
    // Stamp the merge commit with fleet identification so the
    // Auditor can trace it via `git log | grep fleet`.
    let msg = fleet_merge_message(task_id, p_item_id, "auto-merge");
    repo.merge(&[&annotated], None, None)
        .map_err(|e| format!("merge {branch}: {}", e.message()))?;
    let mut index = repo
        .index()
        .map_err(|e| format!("read index after merge: {}", e.message()))?;
    if index.has_conflicts() {
        // Conflict — collect specs then abort to leave workspace clean.
        let files = collect_conflict_specs(project_workspace)?;
        let reason = format!("3-way merge of {branch} has conflicts");
        abort_merge(&repo)?;
        return Ok(MergeOutcome::Conflict { files, reason });
    }
    finish_merge_commit(&repo, &mut index, &msg, annotated.id())?;
    Ok(MergeOutcome::AutoMerged {
        commits_merged: count,
    })
}

/// Commit the worker's uncommitted changes in the P-item worktree onto its
/// `fleet/<task>/<p>` branch so [`merge_back`] can carry them. The worker never
/// commits (its contract forbids it — see `worker.rs`); the deterministic
/// orchestrator owns the commit, right after the worker exits and the review
/// passes, before merging. Returns `Ok(true)` if a commit was made, `Ok(false)`
/// if the worktree had no changes to commit.
///
/// Without this step `merge_back` would `rev-list task_branch..worker_branch`
/// to 0 (the worker's edits live only in the working tree, never on the branch)
/// and silently return `NoChanges`, losing the work.
pub fn commit_worktree(task_id: &str, p_item_id: &str, message: &str) -> Result<bool, String> {
    let wt = worktree_path(task_id, p_item_id)?;
    if !wt.exists() {
        return Err(format!("worktree missing: {}", wt.display()));
    }
    let git = |args: &[&str]| -> Result<std::process::Output, String> {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&wt)
            .args(args)
            .output()
            .map_err(|e| format!("git {}: {e}", args.join(" ")))
    };
    let add = git(&["add", "-A"])?;
    if !add.status.success() {
        return Err(String::from_utf8_lossy(&add.stderr).trim().to_string());
    }
    // Nothing staged → nothing to commit.
    let status = git(&["status", "--porcelain"])?;
    if String::from_utf8_lossy(&status.stdout).trim().is_empty() {
        return Ok(false);
    }
    // Pass an explicit identity so the commit succeeds even when the repo has
    // no user.name/user.email configured.
    let commit = git(&[
        "-c",
        "user.email=fleet@local",
        "-c",
        "user.name=fleet",
        "commit",
        "-q",
        "-m",
        message,
    ])?;
    if !commit.status.success() {
        return Err(String::from_utf8_lossy(&commit.stderr).trim().to_string());
    }
    Ok(true)
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
    let repo = open_repo(project_workspace)?;
    let worker_branch = repo
        .find_branch(&branch, BranchType::Local)
        .map_err(|e| format!("find branch {branch}: {}", e.message()))?;
    let worker_ref = worker_branch.into_reference();
    let annotated = repo
        .reference_to_annotated_commit(&worker_ref)
        .map_err(|e| format!("annotate {branch}: {}", e.message()))?;
    // Re-do the merge. Conflicts will surface in the index; we expect them.
    repo.merge(&[&annotated], None, None)
        .map_err(|e| format!("merge --no-commit {branch}: {}", e.message()))?;
    // Write each resolution onto the working tree, then stage it (which
    // clears its stage 1/2/3 conflict slots).
    let mut index = repo
        .index()
        .map_err(|e| format!("read index: {}", e.message()))?;
    for (rel_path, content) in resolutions {
        let abs = project_workspace.join(rel_path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create dir {}: {e}", parent.display()))?;
        }
        std::fs::write(&abs, content.as_bytes())
            .map_err(|e| format!("write {}: {e}", rel_path.display()))?;
        if let Err(e) =
            index.add_path(rel_path).map_err(|e| format!("index add {}: {}", rel_path.display(), e.message()))
        {
            abort_merge(&repo)?;
            return Err(e);
        }
    }
    index
        .write()
        .map_err(|e| format!("write index: {}", e.message()))?;
    // Any conflicts still unmerged? Abort and tell the caller.
    if index.has_conflicts() {
        let unresolved: std::collections::HashSet<String> = index
            .iter()
            .filter(|e| entry_stage(e) > 0)
            .map(|e| String::from_utf8_lossy(&e.path).into_owned())
            .collect();
        let count = unresolved.len();
        abort_merge(&repo)?;
        return Err(format!(
            "mediation incomplete — {count} file(s) still unmerged after apply"
        ));
    }
    // Mediated merges also carry fleet identification (kind marks
    // them as LLM-mediated for the Auditor's benefit).
    let msg = fleet_merge_message(task_id, p_item_id, "via Sonnet mediation");
    finish_merge_commit(&repo, &mut index, &msg, annotated.id())?;
    Ok(MergeOutcome::AutoMerged { commits_merged: 1 })
}

/// Merge `feature_branch` into `into_branch` (whole-branch). Used by
/// `accept_task`'s merge-back: feature = `fleet/<slug>`, into = base branch.
/// Mirrors [`merge_back`]'s FF→auto-merge→Conflict logic but between two
/// arbitrary named branches. Checks out `into_branch` first so the merge lands
/// there; on conflict the merge is aborted (workspace left clean) and the
/// `ConflictSpec`s returned for LLM mediation.
pub fn merge_branch_into(
    workspace: &Path,
    feature_branch: &str,
    into_branch: &str,
    commit_message: &str,
) -> Result<MergeOutcome, String> {
    let repo = open_repo(workspace)?;
    let feature = repo
        .find_branch(feature_branch, BranchType::Local)
        .map_err(|e| format!("find branch {feature_branch}: {}", e.message()))?;
    let count = git_revlist_count(workspace, into_branch, feature_branch)?;
    if count == 0 {
        return Ok(MergeOutcome::NoChanges);
    }
    checkout_branch(&repo, into_branch)?;
    let feature_ref = feature.into_reference();
    let annotated = repo
        .reference_to_annotated_commit(&feature_ref)
        .map_err(|e| format!("annotate {feature_branch}: {}", e.message()))?;
    let (analysis, _pref) = repo
        .merge_analysis(&[&annotated])
        .map_err(|e| format!("merge analysis: {}", e.message()))?;
    if analysis.contains(MergeAnalysis::ANALYSIS_FASTFORWARD) {
        fast_forward_to(&repo, into_branch, annotated.id())?;
        return Ok(MergeOutcome::FastForwarded { commits_merged: count });
    }
    repo.merge(&[&annotated], None, None)
        .map_err(|e| format!("merge {feature_branch}: {}", e.message()))?;
    let mut index = repo
        .index()
        .map_err(|e| format!("read index after merge: {}", e.message()))?;
    if index.has_conflicts() {
        let files = collect_conflict_specs(workspace)?;
        let reason = format!("3-way merge of {feature_branch} into {into_branch} has conflicts");
        abort_merge(&repo)?;
        return Ok(MergeOutcome::Conflict { files, reason });
    }
    finish_merge_commit(&repo, &mut index, commit_message, annotated.id())?;
    Ok(MergeOutcome::AutoMerged { commits_merged: count })
}

/// LLM-mediated re-merge of `feature_branch` into `into_branch` using
/// `(path, resolved_content)` tuples. Two-branch analogue of
/// [`apply_resolutions`]; the caller supplies the mediator output.
pub fn apply_branch_merge_resolutions(
    workspace: &Path,
    feature_branch: &str,
    into_branch: &str,
    commit_message: &str,
    resolutions: &[(PathBuf, String)],
) -> Result<MergeOutcome, String> {
    let repo = open_repo(workspace)?;
    checkout_branch(&repo, into_branch)?;
    let feature = repo
        .find_branch(feature_branch, BranchType::Local)
        .map_err(|e| format!("find branch {feature_branch}: {}", e.message()))?;
    let feature_ref = feature.into_reference();
    let annotated = repo
        .reference_to_annotated_commit(&feature_ref)
        .map_err(|e| format!("annotate {feature_branch}: {}", e.message()))?;
    repo.merge(&[&annotated], None, None)
        .map_err(|e| format!("merge {feature_branch}: {}", e.message()))?;
    let mut index = repo
        .index()
        .map_err(|e| format!("read index: {}", e.message()))?;
    for (rel_path, content) in resolutions {
        let abs = workspace.join(rel_path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create dir {}: {e}", parent.display()))?;
        }
        std::fs::write(&abs, content.as_bytes())
            .map_err(|e| format!("write {}: {e}", rel_path.display()))?;
        if let Err(e) = index
            .add_path(rel_path)
            .map_err(|e| format!("index add {}: {}", rel_path.display(), e.message()))
        {
            abort_merge(&repo)?;
            return Err(e);
        }
    }
    index
        .write()
        .map_err(|e| format!("write index: {}", e.message()))?;
    if index.has_conflicts() {
        let count = index.iter().filter(|e| entry_stage(e) > 0).count();
        abort_merge(&repo)?;
        return Err(format!(
            "mediation incomplete — {count} file(s) still unmerged after apply"
        ));
    }
    finish_merge_commit(&repo, &mut index, commit_message, annotated.id())?;
    Ok(MergeOutcome::AutoMerged { commits_merged: 1 })
}

/// Delete a local branch — used after a successful merge-back to clean up
/// `fleet/<slug>`. Errors if the branch is currently checked out or missing.
pub fn delete_local_branch(workspace: &Path, branch: &str) -> Result<(), String> {
    let repo = open_repo(workspace)?;
    let mut b = repo
        .find_branch(branch, BranchType::Local)
        .map_err(|e| format!("find branch {branch}: {}", e.message()))?;
    b.delete()
        .map_err(|e| format!("delete branch {branch}: {}", e.message()))?;
    Ok(())
}

fn checkout_branch(repo: &Repository, branch: &str) -> Result<(), String> {
    let refname = format!("refs/heads/{branch}");
    repo.find_reference(&refname)
        .map_err(|e| format!("find ref {refname}: {}", e.message()))?;
    repo.set_head(&refname)
        .map_err(|e| format!("set HEAD to {refname}: {}", e.message()))?;
    let mut co = CheckoutBuilder::new();
    co.force();
    repo.checkout_head(Some(&mut co))
        .map_err(|e| format!("checkout {branch}: {}", e.message()))?;
    Ok(())
}

fn fast_forward_to(repo: &Repository, branch: &str, target: git2::Oid) -> Result<(), String> {
    let refname = format!("refs/heads/{branch}");
    let mut reference = repo
        .find_reference(&refname)
        .map_err(|e| format!("find ref {refname}: {}", e.message()))?;
    reference
        .set_target(target, "fleet: fast-forward")
        .map_err(|e| format!("ff {refname}: {}", e.message()))?;
    repo.set_head(&refname)
        .map_err(|e| format!("set HEAD {refname}: {}", e.message()))?;
    let mut co = CheckoutBuilder::new();
    co.force();
    repo.checkout_head(Some(&mut co))
        .map_err(|e| format!("checkout fast-forwarded {refname}: {}", e.message()))?;
    Ok(())
}

fn abort_merge(repo: &Repository) -> Result<(), String> {
    // Equivalent to `git merge --abort`: drop MERGE_HEAD etc., then hard-
    // reset working tree + index to HEAD.
    repo.cleanup_state()
        .map_err(|e| format!("cleanup merge state: {}", e.message()))?;
    let head_obj = repo
        .head()
        .and_then(|h| h.peel(git2::ObjectType::Commit))
        .map_err(|e| format!("read HEAD for abort: {}", e.message()))?;
    repo.reset(&head_obj, git2::ResetType::Hard, None)
        .map_err(|e| format!("reset --hard during abort: {}", e.message()))?;
    Ok(())
}

fn finish_merge_commit(
    repo: &Repository,
    index: &mut git2::Index,
    msg: &str,
    theirs_oid: git2::Oid,
) -> Result<(), String> {
    let tree_oid = index
        .write_tree()
        .map_err(|e| format!("write tree: {}", e.message()))?;
    let tree = repo
        .find_tree(tree_oid)
        .map_err(|e| format!("find tree {tree_oid}: {}", e.message()))?;
    let head_commit = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| format!("peel HEAD: {}", e.message()))?;
    let theirs_commit = repo
        .find_commit(theirs_oid)
        .map_err(|e| format!("find their commit {theirs_oid}: {}", e.message()))?;
    let sig = repo
        .signature()
        .or_else(|_| git2::Signature::now("Fleet", "fleet@local"))
        .map_err(|e| format!("build signature: {}", e.message()))?;
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        msg,
        &tree,
        &[&head_commit, &theirs_commit],
    )
    .map_err(|e| format!("commit merge: {}", e.message()))?;
    repo.cleanup_state()
        .map_err(|e| format!("cleanup after merge commit: {}", e.message()))?;
    Ok(())
}

/// Read the index's stages 1/2/3 for every conflicted path via libgit2.
/// A missing stage (e.g. file added on one side, didn't exist on the
/// other) yields an empty string for that side, matching the original
/// CLI behaviour.
fn collect_conflict_specs(workspace: &Path) -> Result<Vec<ConflictSpec>, String> {
    let repo = open_repo(workspace)?;
    let index = repo
        .index()
        .map_err(|e| format!("read index: {}", e.message()))?;
    let mut by_path: std::collections::BTreeMap<String, [Option<git2::Oid>; 3]> =
        std::collections::BTreeMap::new();
    for entry in index.iter() {
        let stage = entry_stage(&entry);
        if stage == 0 {
            continue; // not part of a conflict
        }
        let path = String::from_utf8_lossy(&entry.path).into_owned();
        let slot = by_path.entry(path).or_insert([None, None, None]);
        if (1..=3).contains(&stage) {
            slot[(stage - 1) as usize] = Some(entry.id);
        }
    }
    let mut specs = Vec::with_capacity(by_path.len());
    for (path, oids) in by_path {
        let base = blob_to_string(&repo, oids[0])?;
        let ours = blob_to_string(&repo, oids[1])?;
        let theirs = blob_to_string(&repo, oids[2])?;
        specs.push(ConflictSpec {
            path: PathBuf::from(&path),
            base,
            ours,
            theirs,
        });
    }
    Ok(specs)
}

fn blob_to_string(repo: &Repository, oid: Option<git2::Oid>) -> Result<String, String> {
    match oid {
        None => Ok(String::new()),
        Some(o) => {
            let blob = repo
                .find_blob(o)
                .map_err(|e| format!("find blob {o}: {}", e.message()))?;
            Ok(String::from_utf8_lossy(blob.content()).to_string())
        }
    }
}

/// Count of commits in `head` that are not in `base` (equivalent to
/// `git rev-list --count base..head`). Uses a libgit2 revwalk so no
/// subprocess is spawned.
fn git_revlist_count(workspace: &Path, base: &str, head: &str) -> Result<usize, String> {
    let repo = open_repo(workspace)?;
    let head_oid = repo
        .revparse_single(head)
        .map_err(|e| format!("revparse {head}: {}", e.message()))?
        .id();
    let base_oid = repo
        .revparse_single(base)
        .map_err(|e| format!("revparse {base}: {}", e.message()))?
        .id();
    let mut walk = repo
        .revwalk()
        .map_err(|e| format!("revwalk init: {}", e.message()))?;
    walk.push(head_oid)
        .map_err(|e| format!("revwalk push {head}: {}", e.message()))?;
    walk.hide(base_oid)
        .map_err(|e| format!("revwalk hide {base}: {}", e.message()))?;
    let mut count = 0usize;
    for oid in walk {
        oid.map_err(|e| format!("revwalk iter: {}", e.message()))?;
        count += 1;
    }
    Ok(count)
}

fn open_repo(workspace: &Path) -> Result<Repository, String> {
    Repository::open(workspace)
        .map_err(|e| format!("open git repo at {}: {}", workspace.display(), e.message()))
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

/// Enumerate registered worktree paths (including the main one) via libgit2.
fn git_worktree_list(workspace: &Path) -> Result<Vec<PathBuf>, String> {
    let repo = open_repo(workspace)?;
    let mut paths = vec![repo.workdir().unwrap_or(workspace).to_path_buf()];
    let names = repo
        .worktrees()
        .map_err(|e| format!("list worktrees: {}", e.message()))?;
    for name in names.iter().flatten() {
        let wt = repo
            .find_worktree(name)
            .map_err(|e| format!("find worktree {name}: {}", e.message()))?;
        paths.push(wt.path().to_path_buf());
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
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
        let _g = crate::paths::fleet_home_lock();
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
        let _g = crate::paths::fleet_home_lock();
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
        let _g = crate::paths::fleet_home_lock();
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
        let _g = crate::paths::fleet_home_lock();
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
        let _g = crate::paths::fleet_home_lock();
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
        let _g = crate::paths::fleet_home_lock();
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
        let _g = crate::paths::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        reap(repo.path(), "ghost", "p1").unwrap();
    }

    #[test]
    fn list_for_task_returns_each_provisioned_worktree() {
        let _g = crate::paths::fleet_home_lock();
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
        let _g = crate::paths::fleet_home_lock();
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
        let _g = crate::paths::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        let _ = provision(repo.path(), "main", "task-a", "p1").unwrap();
        let outcome = merge_back(repo.path(), "main", "task-a", "p1").unwrap();
        assert_eq!(outcome, MergeOutcome::NoChanges);
    }

    /// Documents the gap the orchestrator must bridge: a real worker leaves its
    /// changes UNCOMMITTED (its contract forbids committing), and `merge_back`
    /// merges the *branch* — so without `commit_worktree` the work is invisible
    /// and silently dropped as `NoChanges`.
    #[test]
    fn merge_back_loses_uncommitted_worker_changes_without_commit() {
        let _g = crate::paths::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        let wt = provision(repo.path(), "main", "task-a", "p1").unwrap();
        // Real worker behaviour: edit a file, do NOT commit.
        fs::write(wt.join("feature.txt"), "hello\n").unwrap();
        let outcome = merge_back(repo.path(), "main", "task-a", "p1").unwrap();
        assert_eq!(
            outcome,
            MergeOutcome::NoChanges,
            "uncommitted worker changes are NOT merged — orchestrator must commit_worktree first"
        );
        assert!(!repo.path().join("feature.txt").exists());
    }

    /// The fix: `commit_worktree` commits the worker's uncommitted changes onto
    /// the worker branch, after which `merge_back` fast-forwards them into the
    /// task branch.
    #[test]
    fn commit_worktree_then_merge_back_carries_worker_changes() {
        let _g = crate::paths::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        let wt = provision(repo.path(), "main", "task-a", "p1").unwrap();
        // Real worker: edits, no commit.
        fs::write(wt.join("feature.txt"), "hello\n").unwrap();

        let committed = commit_worktree("task-a", "p1", "fleet: worker p1").unwrap();
        assert!(committed, "uncommitted changes must produce a commit");

        let outcome = merge_back(repo.path(), "main", "task-a", "p1").unwrap();
        assert_eq!(outcome, MergeOutcome::FastForwarded { commits_merged: 1 });
        assert_eq!(
            fs::read_to_string(repo.path().join("feature.txt")).unwrap(),
            "hello\n"
        );
    }

    /// `commit_worktree` is a no-op (returns false) when the worker changed
    /// nothing — the orchestrator then treats the P-item as a clean no-op merge.
    #[test]
    fn commit_worktree_noop_when_worktree_clean() {
        let _g = crate::paths::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        let _ = provision(repo.path(), "main", "task-a", "p1").unwrap();
        let committed = commit_worktree("task-a", "p1", "fleet: worker p1").unwrap();
        assert!(!committed, "a clean worktree must not produce a commit");
    }

    #[test]
    fn merge_back_fast_forwards_worker_commits() {
        let _g = crate::paths::fleet_home_lock();
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
        let _g = crate::paths::fleet_home_lock();
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
        let _g = crate::paths::fleet_home_lock();
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
        let _g = crate::paths::fleet_home_lock();
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
        let _g = crate::paths::fleet_home_lock();
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
        let _g = crate::paths::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        // worktree_root doesn't exist yet — should not error.
        let reaped = gc_stale(&[]).unwrap();
        assert!(reaped.is_empty());
    }

    #[test]
    fn apply_resolutions_resolves_a_real_conflict_and_commits() {
        let _g = crate::paths::fleet_home_lock();
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
        let _g = crate::paths::fleet_home_lock();
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
        let _g = crate::paths::fleet_home_lock();
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

    /// Read the subject line of HEAD's commit in `repo`.
    fn head_subject(repo: &Path) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["log", "-1", "--format=%s"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    // An auto-merge commit must identify the P-item and be
    // discoverable via `git log | grep fleet`.
    #[test]
    fn auto_merge_commit_carries_fleet_identification() {
        let _g = crate::paths::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        // Force a non-fast-forward auto-merge (divergent disjoint files).
        let wt = provision(repo.path(), "main", "task-a", "p7").unwrap();
        commit_in_worktree(&wt, "a.txt", "a\n", "worker a");
        commit_in_worktree(repo.path(), "b.txt", "b\n", "main b");

        let outcome = merge_back(repo.path(), "main", "task-a", "p7").unwrap();
        assert!(matches!(outcome, MergeOutcome::AutoMerged { .. }));

        let subject = head_subject(repo.path());
        assert!(
            subject.starts_with("fleet: merged p7 from session "),
            "merge subject must identify the P-item + session: {subject:?}"
        );
        assert!(subject.contains("worker "), "must name a worker: {subject:?}");
        assert!(subject.contains("auto-merge"), "must mark the merge kind: {subject:?}");

        // `git log --oneline | grep fleet` finds it.
        let log = Command::new("git")
            .arg("-C")
            .arg(repo.path())
            .args(["log", "--oneline"])
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&log.stdout);
        assert!(
            log.lines().any(|l| l.contains("fleet: merged p7")),
            "fleet merge must be grep-able in git log: {log}"
        );
    }

    // A mediated (apply_resolutions) merge commit must also carry
    // fleet identification, distinguished as the mediation kind.
    #[test]
    fn mediated_merge_commit_carries_fleet_identification() {
        let _g = crate::paths::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");
        commit_in_worktree(repo.path(), "shared.txt", "original\n", "seed");

        let wt = provision(repo.path(), "main", "task-a", "p9").unwrap();
        commit_in_worktree(&wt, "shared.txt", "from worker\n", "worker");
        commit_in_worktree(repo.path(), "shared.txt", "from main\n", "main");

        let resolutions = vec![(PathBuf::from("shared.txt"), "merged\n".to_string())];
        let outcome = apply_resolutions(repo.path(), "task-a", "p9", &resolutions).unwrap();
        assert!(matches!(outcome, MergeOutcome::AutoMerged { .. }));

        let subject = head_subject(repo.path());
        assert!(
            subject.starts_with("fleet: merged p9 from session "),
            "mediated merge subject must identify the P-item: {subject:?}"
        );
        assert!(
            subject.contains("via Sonnet mediation"),
            "must mark the merge as LLM-mediated: {subject:?}"
        );
    }

    // When the worker runtime sets FLEET_SESSION_ID / FLEET_WORKER_ID,
    // those real ids land in the merge commit (not the branch-name fallback).
    #[test]
    fn fleet_merge_message_uses_runtime_session_and_worker_ids() {
        // Guard env mutation: serialize via the same lock other tests use.
        let _g = crate::paths::fleet_home_lock();
        // Save + restore so we don't leak into sibling tests.
        let prev_sid = std::env::var_os("FLEET_SESSION_ID");
        let prev_wid = std::env::var_os("FLEET_WORKER_ID");
        unsafe {
            std::env::set_var("FLEET_SESSION_ID", "sess-xyz");
            std::env::set_var("FLEET_WORKER_ID", "worker-42");
        }
        let msg = fleet_merge_message("task-a", "p3", "auto-merge");
        unsafe {
            match prev_sid {
                Some(v) => std::env::set_var("FLEET_SESSION_ID", v),
                None => std::env::remove_var("FLEET_SESSION_ID"),
            }
            match prev_wid {
                Some(v) => std::env::set_var("FLEET_WORKER_ID", v),
                None => std::env::remove_var("FLEET_WORKER_ID"),
            }
        }
        assert_eq!(
            msg,
            "fleet: merged p3 from session sess-xyz worker worker-42 (auto-merge)"
        );
    }

    // Absent runtime ids, the message still identifies the P-item
    // and falls back to the deterministic worktree branch name for traceability.
    #[test]
    fn fleet_merge_message_falls_back_to_branch_when_env_absent() {
        let _g = crate::paths::fleet_home_lock();
        let prev_sid = std::env::var_os("FLEET_SESSION_ID");
        let prev_wid = std::env::var_os("FLEET_WORKER_ID");
        unsafe {
            std::env::remove_var("FLEET_SESSION_ID");
            std::env::remove_var("FLEET_WORKER_ID");
        }
        let msg = fleet_merge_message("task-a", "p3", "auto-merge");
        unsafe {
            if let Some(v) = prev_sid { std::env::set_var("FLEET_SESSION_ID", v); }
            if let Some(v) = prev_wid { std::env::set_var("FLEET_WORKER_ID", v); }
        }
        assert_eq!(
            msg,
            "fleet: merged p3 from session unknown worker fleet/task-a/p3 (auto-merge)"
        );
    }

    // merge_back's three real outcomes are exercised by dedicated
    // tests; this one asserts the enum models exactly the FF / auto / conflict
    // trichotomy with the Conflict variant carrying the 3-way specs the LLM
    // mediator consumes — and that there is no built-in retry loop.
    #[test]
    fn merge_outcome_models_three_strategies_with_conflict_specs() {
        let _g = crate::paths::fleet_home_lock();
        let fh = TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(fh.path());
        let repo = TempDir::new().unwrap();
        init_repo(repo.path(), "main");

        // (1) FastForwarded: linear advance.
        let wt1 = provision(repo.path(), "main", "t", "ff").unwrap();
        commit_in_worktree(&wt1, "ff.txt", "x\n", "ff work");
        assert!(matches!(
            merge_back(repo.path(), "main", "t", "ff").unwrap(),
            MergeOutcome::FastForwarded { .. }
        ));

        // (2) Conflict: same line changed both sides → carries ConflictSpec vec.
        commit_in_worktree(repo.path(), "c.txt", "base\n", "seed c");
        let wt2 = provision(repo.path(), "main", "t", "cf").unwrap();
        commit_in_worktree(&wt2, "c.txt", "worker\n", "worker c");
        commit_in_worktree(repo.path(), "c.txt", "main\n", "main c");
        match merge_back(repo.path(), "main", "t", "cf").unwrap() {
            MergeOutcome::Conflict { files, .. } => {
                assert!(!files.is_empty(), "Conflict must surface ConflictSpec vec");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    // ── task→base merge-back (P2: accept) ────────────────────────────────────

    #[test]
    fn merge_branch_into_fast_forwards() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path(), "main");
        run_git(dir.path(), &["checkout", "-q", "-b", "fleet/x"]);
        commit_in_worktree(dir.path(), "feat.txt", "feature\n", "feat work");
        run_git(dir.path(), &["checkout", "-q", "main"]);
        let out = merge_branch_into(dir.path(), "fleet/x", "main", "fleet: merge").unwrap();
        assert!(matches!(out, MergeOutcome::FastForwarded { .. }), "got {out:?}");
        // main now carries the feature file.
        assert!(dir.path().join("feat.txt").exists());
    }

    #[test]
    fn merge_branch_into_no_changes_when_feature_not_ahead() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path(), "main");
        run_git(dir.path(), &["branch", "fleet/x"]); // same commit, no divergence
        assert_eq!(
            merge_branch_into(dir.path(), "fleet/x", "main", "m").unwrap(),
            MergeOutcome::NoChanges
        );
    }

    #[test]
    fn merge_branch_into_reports_conflict() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path(), "main");
        commit_in_worktree(dir.path(), "c.txt", "base\n", "seed");
        run_git(dir.path(), &["checkout", "-q", "-b", "fleet/x"]);
        commit_in_worktree(dir.path(), "c.txt", "feature\n", "feat side");
        run_git(dir.path(), &["checkout", "-q", "main"]);
        commit_in_worktree(dir.path(), "c.txt", "mainside\n", "main side");
        match merge_branch_into(dir.path(), "fleet/x", "main", "m").unwrap() {
            MergeOutcome::Conflict { files, .. } => {
                assert!(files.iter().any(|f| f.path.ends_with("c.txt")));
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        // Conflict aborted → workspace clean (no merge in progress).
        let st = std::process::Command::new("git")
            .arg("-C").arg(dir.path()).args(["status", "--porcelain"]).output().unwrap();
        assert!(String::from_utf8_lossy(&st.stdout).trim().is_empty(), "workspace must be clean after abort");
    }

    #[test]
    fn apply_branch_merge_resolutions_resolves_and_commits() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path(), "main");
        commit_in_worktree(dir.path(), "c.txt", "base\n", "seed");
        run_git(dir.path(), &["checkout", "-q", "-b", "fleet/x"]);
        commit_in_worktree(dir.path(), "c.txt", "feature\n", "feat side");
        run_git(dir.path(), &["checkout", "-q", "main"]);
        commit_in_worktree(dir.path(), "c.txt", "mainside\n", "main side");
        // Confirm it really conflicts first.
        assert!(matches!(
            merge_branch_into(dir.path(), "fleet/x", "main", "m").unwrap(),
            MergeOutcome::Conflict { .. }
        ));
        // Now mediate with a resolved content.
        let res = vec![(PathBuf::from("c.txt"), "resolved\n".to_string())];
        let out = apply_branch_merge_resolutions(dir.path(), "fleet/x", "main", "fleet: mediated", &res).unwrap();
        assert!(matches!(out, MergeOutcome::AutoMerged { .. }), "got {out:?}");
        assert_eq!(std::fs::read_to_string(dir.path().join("c.txt")).unwrap(), "resolved\n");
    }

    #[test]
    fn delete_local_branch_removes_it() {
        let dir = TempDir::new().unwrap();
        init_repo(dir.path(), "main");
        run_git(dir.path(), &["branch", "fleet/x"]);
        delete_local_branch(dir.path(), "fleet/x").unwrap();
        // Second delete errors (already gone).
        assert!(delete_local_branch(dir.path(), "fleet/x").is_err());
    }
}
