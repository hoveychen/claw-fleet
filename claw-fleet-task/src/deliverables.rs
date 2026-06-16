//! Task deliverables — the concrete output of a task: the per-file diff stat
//! of the task's branch against the point it forked from the default branch.
//! Powers the detail page's "Deliverables" section (tasks-ui-rebuild P4/P5).
//!
//! Tasks that haven't started (no branch / no workspace yet) yield an empty
//! result, not an error, so the UI can render "no deliverables yet" cleanly.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeliverableFile {
    pub path: String,
    /// "added" | "modified" | "deleted" | "renamed" | "copied" | "other"
    pub change: String,
    pub additions: u64,
    pub deletions: u64,
    /// True for binary files (no line stats available).
    pub binary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskDeliverables {
    /// The task branch the diff is computed against; `None` when the task has
    /// not been started (no branch yet).
    pub branch: Option<String>,
    /// The base ref whose fork-point the diff is taken from ("main"/"master").
    /// `None` when no such branch exists (then the diff is against the empty
    /// tree — the whole branch tree counts as additions).
    pub base: Option<String>,
    pub files: Vec<DeliverableFile>,
    pub total_additions: u64,
    pub total_deletions: u64,
}

/// Resolve a task's workspace + branch, then compute its deliverables.
pub fn compute_task_deliverables(task_id: &str) -> Result<TaskDeliverables, String> {
    let task = crate::task::get_task(task_id)?;
    let (Some(workspace), Some(branch)) = (task.workspace.clone(), task.task_branch.clone())
    else {
        // Not started yet — no branch to diff.
        return Ok(TaskDeliverables::default());
    };
    deliverables_for_branch(&workspace, &branch)
}

/// Diff `branch` against its fork point off the default branch and summarise
/// the per-file line changes.
pub fn deliverables_for_branch(workspace: &Path, branch: &str) -> Result<TaskDeliverables, String> {
    let repo = git2::Repository::open(workspace)
        .map_err(|e| format!("open git repo at {}: {}", workspace.display(), e.message()))?;

    let branch_commit = repo
        .revparse_single(branch)
        .and_then(|o| o.peel_to_commit())
        .map_err(|e| format!("resolve branch {branch}: {}", e.message()))?;

    // Pick the base branch to fork against; first existing of main / master.
    let base_name = ["main", "master"]
        .into_iter()
        .find(|b| repo.find_branch(b, git2::BranchType::Local).is_ok());
    let base_commit = base_name
        .and_then(|b| repo.revparse_single(b).ok())
        .and_then(|o| o.peel_to_commit().ok());

    // `from` tree is the merge-base (fork point) of base..branch, so the diff
    // is exactly the task's own changes regardless of where the base has moved.
    let from_tree = match &base_commit {
        Some(bc) => match repo.merge_base(bc.id(), branch_commit.id()) {
            Ok(mb) => repo.find_commit(mb).ok().and_then(|c| c.tree().ok()),
            Err(_) => bc.tree().ok(),
        },
        None => None, // no base branch → diff against the empty tree
    };
    let to_tree = branch_commit
        .tree()
        .map_err(|e| format!("branch tree: {}", e.message()))?;

    let diff = repo
        .diff_tree_to_tree(from_tree.as_ref(), Some(&to_tree), None)
        .map_err(|e| format!("diff: {}", e.message()))?;

    let mut files = Vec::new();
    let mut total_additions = 0u64;
    let mut total_deletions = 0u64;
    for idx in 0..diff.deltas().len() {
        let delta = match diff.get_delta(idx) {
            Some(d) => d,
            None => continue,
        };
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let change = match delta.status() {
            git2::Delta::Added => "added",
            git2::Delta::Deleted => "deleted",
            git2::Delta::Renamed => "renamed",
            git2::Delta::Copied => "copied",
            git2::Delta::Modified => "modified",
            _ => "other",
        }
        .to_string();
        let (additions, deletions, binary) = match git2::Patch::from_diff(&diff, idx) {
            Ok(Some(patch)) => {
                let (_ctx, add, del) = patch.line_stats().unwrap_or((0, 0, 0));
                (add as u64, del as u64, false)
            }
            // No patch object → treated as binary (line stats unavailable).
            _ => (0, 0, true),
        };
        total_additions += additions;
        total_deletions += deletions;
        files.push(DeliverableFile { path, change, additions, deletions, binary });
    }
    // Biggest changes first.
    files.sort_by(|a, b| (b.additions + b.deletions).cmp(&(a.additions + a.deletions)));

    Ok(TaskDeliverables {
        branch: Some(branch.to_string()),
        base: base_name.map(|s| s.to_string()),
        files,
        total_additions,
        total_deletions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sig() -> git2::Signature<'static> {
        git2::Signature::now("Test", "test@example.com").unwrap()
    }

    /// Stage the given files in the working tree and commit on HEAD.
    fn commit(repo: &git2::Repository, dir: &Path, files: &[(&str, &str)], msg: &str) -> git2::Oid {
        for (name, content) in files {
            fs::write(dir.join(name), content).unwrap();
        }
        let mut index = repo.index().unwrap();
        for (name, _) in files {
            index.add_path(Path::new(name)).unwrap();
        }
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let parents: Vec<git2::Commit> = match repo.head().ok().and_then(|h| h.peel_to_commit().ok()) {
            Some(c) => vec![c],
            None => vec![],
        };
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig(), &sig(), msg, &tree, &parent_refs)
            .unwrap()
    }

    #[test]
    fn deliverables_capture_branch_changes_against_main() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();
        let repo = git2::Repository::init(dir).unwrap();

        // Base commit on `main`.
        let base = commit(&repo, dir, &[("a.txt", "l1\nl2\n")], "base");
        repo.branch("main", &repo.find_commit(base).unwrap(), true).unwrap();
        repo.set_head("refs/heads/main").unwrap();

        // Feature branch off main: modify a.txt (+1) and add b.txt (+3).
        let main_commit = repo.find_commit(base).unwrap();
        repo.branch("fleet/demo", &main_commit, false).unwrap();
        repo.set_head("refs/heads/fleet/demo").unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        commit(
            &repo,
            dir,
            &[("a.txt", "l1\nl2\nl3\n"), ("b.txt", "x\ny\nz\n")],
            "feature work",
        );

        let d = deliverables_for_branch(dir, "fleet/demo").unwrap();
        assert_eq!(d.branch.as_deref(), Some("fleet/demo"));
        assert_eq!(d.base.as_deref(), Some("main"));
        let paths: Vec<&str> = d.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"a.txt"), "a.txt modified, got {paths:?}");
        assert!(paths.contains(&"b.txt"), "b.txt added, got {paths:?}");
        let b = d.files.iter().find(|f| f.path == "b.txt").unwrap();
        assert_eq!(b.change, "added");
        assert_eq!(b.additions, 3);
        assert_eq!(d.total_additions, 4); // a.txt +1, b.txt +3
    }

    #[test]
    fn compute_task_deliverables_empty_for_unknown_or_unstarted() {
        // A task id that does not exist resolves to an Err from get_task; an
        // unstarted task (no branch) would yield an empty result. We can only
        // exercise the unknown path here without a task store fixture.
        let r = compute_task_deliverables("definitely-not-a-real-task-id");
        assert!(r.is_err());
    }
}
