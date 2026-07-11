//! Lightweight source-control surface for the file explorer's roots.
//!
//! Read status (branch, ahead/behind upstream, dirty working tree) with git2,
//! and run `push` / `pull` by shelling out to the `git` CLI so they inherit
//! the user's configured credential helper / SSH agent — git2 is built with
//! `default-features = false`, so it has no network transport compiled in and
//! cannot push or fetch on its own.
//!
//! Every entry point reuses `file_explorer::resolve_validated_root`, so a
//! `root` must be the workspace itself or one of its git worktrees, and the
//! workspace must be one the backend already knows about. This keeps the same
//! access envelope as the read-only explorer.

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::file_explorer::resolve_validated_root;

/// Source-control snapshot for a single explorer root.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    /// True when the root is a git repository at all.
    pub is_git: bool,
    /// Current branch shorthand, or `None` on a detached HEAD / empty repo.
    pub branch: Option<String>,
    /// Upstream tracking ref (e.g. `origin/main`), when the branch has one.
    pub upstream: Option<String>,
    /// Commits ahead of upstream — i.e. unpushed. `None` without an upstream.
    pub ahead: Option<usize>,
    /// Commits behind upstream. `None` without an upstream.
    pub behind: Option<usize>,
    /// Working-tree entries not yet committed: staged + unstaged + untracked.
    pub dirty_count: usize,
}

impl GitStatus {
    fn not_git() -> Self {
        GitStatus {
            is_git: false,
            branch: None,
            upstream: None,
            ahead: None,
            behind: None,
            dirty_count: 0,
        }
    }
}

/// Result of a `push` / `pull` invocation. `output` is the trimmed combined
/// stdout+stderr from git, shown verbatim to the user.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GitOpResult {
    pub ok: bool,
    pub output: String,
}

/// Compute the source-control status of one explorer root (read-only).
pub fn git_status(
    workspace: &str,
    root: &str,
    known_workspaces: &[String],
) -> Result<GitStatus, String> {
    let root = resolve_validated_root(workspace, root, known_workspaces)?;
    let Ok(repo) = git2::Repository::open(&root) else {
        return Ok(GitStatus::not_git());
    };

    let mut status = GitStatus::not_git();
    status.is_git = true;

    // Branch + upstream ahead/behind. A detached HEAD or an unborn branch
    // (fresh repo, no commits) simply leaves branch/ahead/behind as None.
    if let Ok(head) = repo.head() {
        if head.is_branch() {
            let shorthand = head.shorthand().map(str::to_string);
            status.branch = shorthand.clone();
            if let (Some(name), Some(local_oid)) = (shorthand, head.target()) {
                if let Ok(local_branch) = repo.find_branch(&name, git2::BranchType::Local) {
                    if let Ok(up) = local_branch.upstream() {
                        status.upstream = up.name().ok().flatten().map(str::to_string);
                        if let Some(up_oid) = up.get().target() {
                            if let Ok((ahead, behind)) =
                                repo.graph_ahead_behind(local_oid, up_oid)
                            {
                                status.ahead = Some(ahead);
                                status.behind = Some(behind);
                            }
                        }
                    }
                }
            }
        }
    }

    // Dirty working tree: everything that is neither clean nor gitignored.
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    if let Ok(statuses) = repo.statuses(Some(&mut opts)) {
        status.dirty_count = statuses
            .iter()
            .filter(|e| {
                let s = e.status();
                !s.is_empty() && !s.contains(git2::Status::IGNORED)
            })
            .count();
    }

    Ok(status)
}

/// `git push` in the root's working directory.
pub fn git_push(
    workspace: &str,
    root: &str,
    known_workspaces: &[String],
) -> Result<GitOpResult, String> {
    run_git(workspace, root, known_workspaces, &["push"])
}

/// `git pull --ff-only` in the root's working directory. Fast-forward-only so
/// a pull that would need a merge fails loudly (with git's own message shown)
/// instead of silently creating a merge commit or leaving conflicts behind.
pub fn git_pull(
    workspace: &str,
    root: &str,
    known_workspaces: &[String],
) -> Result<GitOpResult, String> {
    run_git(workspace, root, known_workspaces, &["pull", "--ff-only"])
}

fn run_git(
    workspace: &str,
    root: &str,
    known_workspaces: &[String],
    args: &[&str],
) -> Result<GitOpResult, String> {
    let root = resolve_validated_root(workspace, root, known_workspaces)?;
    run_git_in(&root, args)
}

fn run_git_in(root: &Path, args: &[&str]) -> Result<GitOpResult, String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok(GitOpResult {
        ok: out.status.success(),
        output: text.trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn known(ws: &Path) -> Vec<String> {
        vec![ws.to_string_lossy().to_string()]
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repo with deterministic identity + a `main` branch and one commit.
    fn init_repo(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q", "-b", "main"]);
        git(dir, &["config", "user.email", "t@t"]);
        git(dir, &["config", "user.name", "t"]);
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn status_reports_clean_repo_with_no_upstream() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        init_repo(&ws);
        let w = ws.to_str().unwrap();

        let st = git_status(w, w, &known(&ws)).unwrap();
        assert!(st.is_git);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.upstream, None);
        assert_eq!(st.ahead, None);
        assert_eq!(st.behind, None);
        assert_eq!(st.dirty_count, 0);
    }

    #[test]
    fn status_counts_dirty_staged_unstaged_and_untracked() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        init_repo(&ws);
        let w = ws.to_str().unwrap();

        // modified tracked (unstaged), staged new file, untracked file → 3.
        fs::write(ws.join("a.txt"), "one\ntwo\n").unwrap();
        fs::write(ws.join("staged.txt"), "s\n").unwrap();
        git(&ws, &["add", "staged.txt"]);
        fs::write(ws.join("untracked.txt"), "u\n").unwrap();

        let st = git_status(w, w, &known(&ws)).unwrap();
        assert_eq!(st.dirty_count, 3, "status: {st:?}");
    }

    #[test]
    fn status_reports_ahead_and_push_clears_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Bare "remote" + a clone that tracks it.
        let remote = tmp.path().join("remote.git");
        fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "-q", "--bare"]);

        let ws = tmp.path().join("ws");
        init_repo(&ws);
        git(&ws, &["remote", "add", "origin", remote.to_str().unwrap()]);
        git(&ws, &["push", "-q", "-u", "origin", "main"]);
        let w = ws.to_str().unwrap();

        // In sync right after the initial push.
        let st = git_status(w, w, &known(&ws)).unwrap();
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!(st.ahead, Some(0));
        assert_eq!(st.behind, Some(0));

        // One local commit → ahead by 1 (unpushed).
        fs::write(ws.join("b.txt"), "b\n").unwrap();
        git(&ws, &["add", "-A"]);
        git(&ws, &["commit", "-q", "-m", "second"]);
        let st = git_status(w, w, &known(&ws)).unwrap();
        assert_eq!(st.ahead, Some(1), "status: {st:?}");

        // Push via the real code path clears the ahead count.
        let res = git_push(w, w, &known(&ws)).unwrap();
        assert!(res.ok, "push failed: {}", res.output);
        let st = git_status(w, w, &known(&ws)).unwrap();
        assert_eq!(st.ahead, Some(0), "status after push: {st:?}");
    }

    #[test]
    fn status_reports_behind_and_pull_clears_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let remote = tmp.path().join("remote.git");
        fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "-q", "--bare"]);

        // First clone seeds the remote with two commits.
        let seed = tmp.path().join("seed");
        init_repo(&seed);
        git(&seed, &["remote", "add", "origin", remote.to_str().unwrap()]);
        git(&seed, &["push", "-q", "-u", "origin", "main"]);

        // Second clone tracks origin/main at the first commit only.
        let ws = tmp.path().join("ws");
        git(
            tmp.path(),
            &["clone", "-q", remote.to_str().unwrap(), ws.to_str().unwrap()],
        );
        git(&ws, &["config", "user.email", "t@t"]);
        git(&ws, &["config", "user.name", "t"]);
        let w = ws.to_str().unwrap();

        // Seed pushes a new commit; ws is now behind by 1.
        fs::write(seed.join("c.txt"), "c\n").unwrap();
        git(&seed, &["add", "-A"]);
        git(&seed, &["commit", "-q", "-m", "third"]);
        git(&seed, &["push", "-q"]);
        git(&ws, &["fetch", "-q"]);

        let st = git_status(w, w, &known(&ws)).unwrap();
        assert_eq!(st.behind, Some(1), "status: {st:?}");

        let res = git_pull(w, w, &known(&ws)).unwrap();
        assert!(res.ok, "pull failed: {}", res.output);
        let st = git_status(w, w, &known(&ws)).unwrap();
        assert_eq!(st.behind, Some(0), "status after pull: {st:?}");
    }

    #[test]
    fn non_git_root_reports_is_git_false() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        let w = ws.to_str().unwrap();

        let st = git_status(w, w, &known(&ws)).unwrap();
        assert!(!st.is_git);
        assert_eq!(st.dirty_count, 0);
    }

    #[test]
    fn status_rejects_unknown_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        init_repo(&ws);
        let w = ws.to_str().unwrap();
        let err = git_status(w, w, &[]).unwrap_err();
        assert!(err.contains("not a known session workspace"), "got: {err}");
    }
}
