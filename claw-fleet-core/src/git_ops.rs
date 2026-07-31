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

use std::path::{Path, PathBuf};

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
    /// Fetch URL of the remote this root talks to — the branch's own remote when
    /// it tracks one, else `origin`. `None` when the repo has no such remote.
    /// This is the URL (`git@github.com:owner/repo.git`), not the ref name that
    /// `upstream` carries.
    pub remote_url: Option<String>,
    /// Commits ahead of upstream — i.e. unpushed. `None` without an upstream.
    pub ahead: Option<usize>,
    /// Commits behind upstream. `None` without an upstream.
    pub behind: Option<usize>,
    /// Working-tree entries not yet committed: staged + unstaged + untracked.
    /// Always equal to `dirty_files.len()` — kept as its own field so callers
    /// that only need the badge count don't have to look at the list.
    pub dirty_count: usize,
    /// The uncommitted entries themselves (path + a one-char status code), so
    /// the badge can expand into a file list. Sorted by path.
    pub dirty_files: Vec<DirtyFile>,
}

/// One uncommitted working-tree entry.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DirtyFile {
    /// Path relative to the repository work directory.
    pub path: String,
    /// One-char status code, git-porcelain flavoured: `M` modified, `A` staged
    /// add, `D` deleted, `R` renamed, `T` typechange, `?` untracked, `U` merge
    /// conflict.
    pub status: String,
}

impl GitStatus {
    fn not_git() -> Self {
        GitStatus {
            is_git: false,
            branch: None,
            upstream: None,
            remote_url: None,
            ahead: None,
            behind: None,
            dirty_count: 0,
            dirty_files: Vec::new(),
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

    let (branch, upstream, ahead, behind) = head_tracking(&repo);
    let dirty_files = dirty_files(&repo);
    Ok(GitStatus {
        is_git: true,
        branch,
        upstream,
        remote_url: remote_url(&repo),
        ahead,
        behind,
        dirty_count: dirty_files.len(),
        dirty_files,
    })
}

/// Branch shorthand + upstream tracking ref + ahead/behind counts for a repo's
/// current HEAD. A detached HEAD or an unborn branch (fresh repo, no commits)
/// leaves everything as `None`; ahead/behind are `None` without an upstream.
fn head_tracking(
    repo: &git2::Repository,
) -> (Option<String>, Option<String>, Option<usize>, Option<usize>) {
    let (mut branch, mut upstream, mut ahead, mut behind) = (None, None, None, None);
    if let Ok(head) = repo.head() {
        if head.is_branch() {
            let shorthand = head.shorthand().map(str::to_string);
            branch = shorthand.clone();
            if let (Some(name), Some(local_oid)) = (shorthand, head.target()) {
                if let Ok(local_branch) = repo.find_branch(&name, git2::BranchType::Local) {
                    if let Ok(up) = local_branch.upstream() {
                        upstream = up.name().ok().flatten().map(str::to_string);
                        if let Some(up_oid) = up.get().target() {
                            if let Ok((a, b)) = repo.graph_ahead_behind(local_oid, up_oid) {
                                ahead = Some(a);
                                behind = Some(b);
                            }
                        }
                    }
                }
            }
        }
    }
    (branch, upstream, ahead, behind)
}

/// Fetch URL of the remote a repo talks to: the current branch's own remote
/// when it tracks one, else `origin`. `None` on a repo with no matching remote
/// (a purely local `git init`, or a branch tracking a remote that was removed).
fn remote_url(repo: &git2::Repository) -> Option<String> {
    let name = repo
        .head()
        .ok()
        .filter(git2::Reference::is_branch)
        .and_then(|h| h.name().map(str::to_string))
        .and_then(|refname| repo.branch_upstream_remote(&refname).ok())
        .and_then(|buf| buf.as_str().map(str::to_string))
        .unwrap_or_else(|| "origin".to_string());
    repo.find_remote(&name)
        .ok()
        .and_then(|r| r.url().map(str::to_string))
}

/// Count of working-tree entries that are neither clean nor gitignored. Kept
/// for the repo-overview surface (`list_repos` / `repo_detail` / worktree
/// health), which needs only the number, not the paths.
fn dirty_count(repo: &git2::Repository) -> usize {
    dirty_files(repo).len()
}

/// Working-tree entries that are neither clean nor gitignored, each with a
/// one-char status code. Sorted by path for a stable list.
fn dirty_files(repo: &git2::Repository) -> Vec<DirtyFile> {
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    let Ok(statuses) = repo.statuses(Some(&mut opts)) else {
        return Vec::new();
    };
    let mut out: Vec<DirtyFile> = statuses
        .iter()
        .filter_map(|e| {
            let s = e.status();
            if s.is_empty() || s.contains(git2::Status::IGNORED) {
                return None;
            }
            Some(DirtyFile {
                path: e.path().unwrap_or_default().to_string(),
                status: status_code(s).to_string(),
            })
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Reduce a git2 status bitset to a single representative porcelain-style code.
/// A file can be dirty in several ways at once (e.g. staged + modified again);
/// we surface the most salient one in a fixed priority order.
fn status_code(s: git2::Status) -> &'static str {
    use git2::Status as St;
    if s.contains(St::CONFLICTED) {
        "U"
    } else if s.contains(St::WT_NEW) {
        "?"
    } else if s.contains(St::INDEX_NEW) {
        "A"
    } else if s.intersects(St::INDEX_DELETED | St::WT_DELETED) {
        "D"
    } else if s.intersects(St::INDEX_RENAMED | St::WT_RENAMED) {
        "R"
    } else if s.intersects(St::INDEX_TYPECHANGE | St::WT_TYPECHANGE) {
        "T"
    } else {
        "M"
    }
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

/// `git clone <url> <dest>` — the one git entry point that does NOT take a
/// known workspace, because a fresh clone by definition isn't one yet.
///
/// That means `resolve_validated_root`'s access envelope can't apply here, so
/// the guard rails are structural instead: `dest` must be absolute, its parent
/// must already exist as a directory, and `dest` itself must be absent or an
/// empty directory. Nothing existing can be written over or deleted — the worst
/// a caller can do is create one new directory somewhere the process could
/// already write (the same envelope as "launch a session in any cwd").
///
/// Runs from `dest`'s parent so a relative-path git config / credential helper
/// resolves the way the user's shell would.
pub fn git_clone(url: &str, dest: &str) -> Result<GitOpResult, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("clone url is empty".into());
    }
    // A leading dash would be parsed as an option (`--upload-pack=...`), so
    // reject it outright rather than relying on git's `--` handling alone.
    if url.starts_with('-') {
        return Err("clone url must not start with '-'".into());
    }
    let dest_path = Path::new(dest);
    if !dest_path.is_absolute() {
        return Err(format!("clone destination must be an absolute path: {dest}"));
    }
    let parent = dest_path
        .parent()
        .ok_or_else(|| format!("clone destination has no parent directory: {dest}"))?;
    if !parent.is_dir() {
        return Err(format!(
            "clone destination's parent directory does not exist: {}",
            parent.display()
        ));
    }
    if dest_path.exists() {
        if !dest_path.is_dir() {
            return Err(format!("clone destination already exists: {dest}"));
        }
        let empty = std::fs::read_dir(dest_path)
            .map_err(|e| format!("clone destination: {e}"))?
            .next()
            .is_none();
        if !empty {
            return Err(format!("clone destination is not empty: {dest}"));
        }
    }
    run_git_in(parent, &["clone", "--", url, dest])
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
    // `process_util::command` applies CREATE_NO_WINDOW on Windows so a push /
    // pull fired from the desktop GUI doesn't flash a conhost window. No-op on
    // Unix. (Raw `Command::new` here would pop a black console box per op.)
    let out = crate::process_util::command("git")
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

// ── Repository overview (mobile "仓库" surface) ──────────────────────────────
//
// A "loose ends" view over every git repo reachable from a known session
// workspace: which linked worktrees still hold commits not merged back into the
// main checkout (forgotten merges) and whether the main branch is ahead of its
// upstream (forgotten pushes). Unlike the per-root status bar above, this
// enumerates the repos itself and dedups worktree checkouts back to their main
// so the same repository never appears twice.

/// One repository row in the list view.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RepoSummary {
    /// Canonical path of the main checkout — pass back as `root` to the other
    /// repo entry points.
    pub root: String,
    /// Directory name of the main checkout.
    pub label: String,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    /// Commits on the current branch not yet pushed to upstream — the "forgot
    /// to push" count. `None` when the branch has no upstream.
    pub unpushed: Option<usize>,
    pub behind: Option<usize>,
    /// Uncommitted entries in the main checkout.
    pub dirty_count: usize,
    /// Linked worktrees, excluding the main checkout.
    pub worktree_count: usize,
    /// Worktrees carrying unmerged commits or uncommitted changes — the "forgot
    /// to merge" count surfaced as a badge.
    pub pending_worktrees: usize,
    /// True when anything above is a loose end worth the user's attention.
    pub needs_attention: bool,
}

/// Health of one linked git worktree.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeHealth {
    pub path: String,
    pub branch: Option<String>,
    /// Commits on this worktree's branch not reachable from the main checkout's
    /// HEAD — i.e. work not yet merged back.
    pub unmerged: usize,
    /// Uncommitted entries in this worktree. Always `dirty_files.len()`.
    pub dirty_count: usize,
    /// The uncommitted entries themselves, so the "脏 N" badge can expand into
    /// a file list on the mobile repo surface.
    pub dirty_files: Vec<DirtyFile>,
    pub last_commit_summary: Option<String>,
    /// Author date of the tip commit, unix seconds.
    pub last_commit_time: Option<i64>,
}

/// One recent commit on the main checkout's current branch.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CommitInfo {
    /// Short (8-char) hash.
    pub hash: String,
    pub summary: String,
    pub author: String,
    /// Author date, unix seconds.
    pub time: i64,
}

/// Full detail for one repo: its main-branch push state, every linked worktree,
/// and a slice of recent commits.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RepoDetail {
    pub root: String,
    pub label: String,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    /// Fetch URL of the repo's remote (branch's own remote, else `origin`).
    /// Same field as `GitStatus::remote_url` — a URL, not a ref name.
    pub remote_url: Option<String>,
    pub unpushed: Option<usize>,
    pub behind: Option<usize>,
    /// Uncommitted entries in the main checkout. Always `dirty_files.len()`.
    pub dirty_count: usize,
    /// The uncommitted entries themselves, so the main-checkout "脏 N" badge can
    /// expand into a file list on the mobile repo surface.
    pub dirty_files: Vec<DirtyFile>,
    pub worktrees: Vec<WorktreeHealth>,
    pub commits: Vec<CommitInfo>,
}

/// Every git repo reachable from a known session workspace, deduped, with a
/// loose-ends health summary. Non-git workspaces are skipped; worktree
/// checkouts collapse onto their main checkout so each repo appears once.
/// Needs-attention repos sort first, then alphabetically by label.
pub fn list_repos(known_workspaces: &[String]) -> Vec<RepoSummary> {
    let mut out = Vec::new();
    for root in known_repo_roots(known_workspaces) {
        let Ok(repo) = git2::Repository::open(&root) else {
            continue;
        };
        let (branch, upstream, unpushed, behind) = head_tracking(&repo);
        let main_head = repo.head().ok().and_then(|h| h.target());
        let worktrees = worktree_healths(&repo, main_head);
        let dc = dirty_count(&repo);
        let pending = worktrees
            .iter()
            .filter(|w| w.unmerged > 0 || w.dirty_count > 0)
            .count();
        let needs_attention = unpushed.unwrap_or(0) > 0 || dc > 0 || pending > 0;
        out.push(RepoSummary {
            label: repo_label(&root),
            root: root.to_string_lossy().to_string(),
            branch,
            upstream,
            unpushed,
            behind,
            dirty_count: dc,
            worktree_count: worktrees.len(),
            pending_worktrees: pending,
            needs_attention,
        });
    }
    out.sort_by(|a, b| {
        b.needs_attention
            .cmp(&a.needs_attention)
            .then_with(|| a.label.cmp(&b.label))
    });
    out
}

/// Full detail for one repo. `root` must be a repository reachable from a known
/// workspace (the same access envelope as `list_repos`).
pub fn repo_detail(root: &str, known_workspaces: &[String]) -> Result<RepoDetail, String> {
    let root = validate_repo_root(root, known_workspaces)?;
    let repo = git2::Repository::open(&root).map_err(|e| format!("open repo: {e}"))?;
    let (branch, upstream, unpushed, behind) = head_tracking(&repo);
    let main_head = repo.head().ok().and_then(|h| h.target());
    let files = dirty_files(&repo);
    Ok(RepoDetail {
        label: repo_label(&root),
        root: root.to_string_lossy().to_string(),
        branch,
        upstream,
        remote_url: remote_url(&repo),
        unpushed,
        behind,
        dirty_count: files.len(),
        dirty_files: files,
        worktrees: worktree_healths(&repo, main_head),
        commits: recent_commits(&repo, 30),
    })
}

/// `git push` on a repo's main checkout (pushes the current branch to its
/// upstream). `root` is validated against the same envelope as `repo_detail`.
pub fn repo_push(root: &str, known_workspaces: &[String]) -> Result<GitOpResult, String> {
    let root = validate_repo_root(root, known_workspaces)?;
    run_git_in(&root, &["push"])
}

/// `git pull --ff-only` on a repo's main checkout — fast-forward-only so a pull
/// that would need a merge fails loudly instead of creating a merge commit.
pub fn repo_pull(root: &str, known_workspaces: &[String]) -> Result<GitOpResult, String> {
    let root = validate_repo_root(root, known_workspaces)?;
    run_git_in(&root, &["pull", "--ff-only"])
}

fn repo_label(root: &Path) -> String {
    root.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo")
        .to_string()
}

/// The canonical main-checkout path of the repo containing `path`. `None` when
/// `path` is not inside a git repository. A linked worktree collapses onto its
/// main checkout: git2 reports a worktree's `path()` as
/// `<main>/.git/worktrees/<name>`, so the main checkout is three parents up.
fn main_checkout(path: &Path) -> Option<PathBuf> {
    let repo = git2::Repository::open(path).ok()?;
    let workdir = if repo.is_worktree() {
        repo.path().ancestors().nth(3).map(Path::to_path_buf)
    } else {
        repo.workdir().map(Path::to_path_buf)
    }?;
    std::fs::canonicalize(workdir).ok()
}

/// Canonical main-checkout paths of every git repo reachable from a known
/// workspace, deduped. This is the access envelope for the repo surface:
/// exactly the repositories the read-only explorer could already reach.
fn known_repo_roots(known_workspaces: &[String]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for ws in known_workspaces {
        if let Some(main) = main_checkout(Path::new(ws)) {
            if !roots.contains(&main) {
                roots.push(main);
            }
        }
    }
    roots
}

fn validate_repo_root(root: &str, known_workspaces: &[String]) -> Result<PathBuf, String> {
    let rc = std::fs::canonicalize(root).map_err(|e| format!("root: {e}"))?;
    if known_repo_roots(known_workspaces).into_iter().any(|r| r == rc) {
        Ok(rc)
    } else {
        Err("root is not a known repository".into())
    }
}

/// Health of every linked worktree of `main_repo`. `main_head` is the main
/// checkout's HEAD oid, against which each worktree's unmerged-commit count is
/// measured. Pruned/stale worktree dirs are skipped. Sorted by path.
fn worktree_healths(
    main_repo: &git2::Repository,
    main_head: Option<git2::Oid>,
) -> Vec<WorktreeHealth> {
    let mut out = Vec::new();
    let Ok(names) = main_repo.worktrees() else {
        return out;
    };
    for name in names.iter().flatten() {
        let Ok(wt) = main_repo.find_worktree(name) else {
            continue;
        };
        let path = wt.path().to_path_buf();
        if !path.is_dir() {
            continue; // pruned / stale worktree
        }
        let Ok(wt_repo) = git2::Repository::open(&path) else {
            continue;
        };
        let head = wt_repo.head().ok();
        let branch = head.as_ref().and_then(|h| h.shorthand().map(str::to_string));
        let wt_oid = head.as_ref().and_then(git2::Reference::target);
        // Worktrees share the main repo's object database, so the worktree tip
        // is resolvable from `main_repo`. ahead = commits in the worktree
        // branch not reachable from main HEAD = not yet merged back.
        let unmerged = match (wt_oid, main_head) {
            (Some(w), Some(m)) => main_repo
                .graph_ahead_behind(w, m)
                .map(|(ahead, _)| ahead)
                .unwrap_or(0),
            _ => 0,
        };
        let tip = wt_oid.and_then(|o| wt_repo.find_commit(o).ok());
        let (last_commit_summary, last_commit_time) = match tip {
            Some(c) => (c.summary().map(str::to_string), Some(c.time().seconds())),
            None => (None, None),
        };
        let wt_files = dirty_files(&wt_repo);
        out.push(WorktreeHealth {
            path: path.to_string_lossy().to_string(),
            branch,
            unmerged,
            dirty_count: wt_files.len(),
            dirty_files: wt_files,
            last_commit_summary,
            last_commit_time,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Up to `limit` most-recent commits reachable from HEAD, newest first.
fn recent_commits(repo: &git2::Repository, limit: usize) -> Vec<CommitInfo> {
    let mut out = Vec::new();
    let Ok(mut walk) = repo.revwalk() else {
        return out;
    };
    if walk.push_head().is_err() {
        return out; // unborn branch / empty repo
    }
    for oid in walk.flatten().take(limit) {
        if let Ok(c) = repo.find_commit(oid) {
            out.push(CommitInfo {
                hash: oid.to_string().chars().take(8).collect(),
                summary: c.summary().unwrap_or("").to_string(),
                author: c.author().name().unwrap_or("").to_string(),
                time: c.time().seconds(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::process::Command;

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
        // dirty_files carries the paths + a one-char code, sorted by path, and
        // its length always matches dirty_count.
        assert_eq!(st.dirty_files.len(), st.dirty_count);
        let files: Vec<(&str, &str)> = st
            .dirty_files
            .iter()
            .map(|f| (f.path.as_str(), f.status.as_str()))
            .collect();
        assert_eq!(
            files,
            vec![("a.txt", "M"), ("staged.txt", "A"), ("untracked.txt", "?")],
            "dirty_files: {:?}",
            st.dirty_files
        );
    }

    #[test]
    fn status_reports_ahead_and_push_clears_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Bare "remote" + a clone that tracks it.
        let remote = tmp.path().join("remote.git");
        fs::create_dir_all(&remote).unwrap();
        // `-b main` pins the bare repo's HEAD so it doesn't default to the
        // ambient `init.defaultBranch` (often "master" on CI runners, which
        // leaves HEAD dangling after we only push `main` → clone can't check
        // out and status reports no upstream).
        git(&remote, &["init", "-q", "--bare", "-b", "main"]);

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
        // `-b main` pins the bare repo's HEAD so it doesn't default to the
        // ambient `init.defaultBranch` (often "master" on CI runners, which
        // leaves HEAD dangling after we only push `main` → clone can't check
        // out and status reports no upstream).
        git(&remote, &["init", "-q", "--bare", "-b", "main"]);

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

    // ── Clone + remote URL ──────────────────────────────────────────────────

    #[test]
    fn clone_creates_checkout_and_status_reports_its_remote_url() {
        let tmp = tempfile::TempDir::new().unwrap();
        // A bare "remote" seeded with one commit, so the clone has something to
        // check out and a real URL to report back.
        let remote = tmp.path().join("remote.git");
        fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "-q", "--bare", "-b", "main"]);
        let seed = tmp.path().join("seed");
        init_repo(&seed);
        git(&seed, &["remote", "add", "origin", remote.to_str().unwrap()]);
        git(&seed, &["push", "-q", "-u", "origin", "main"]);

        let dest = tmp.path().join("cloned");
        let res = git_clone(remote.to_str().unwrap(), dest.to_str().unwrap()).unwrap();
        assert!(res.ok, "clone failed: {}", res.output);
        assert!(dest.join(".git").exists(), "no .git in {}", dest.display());
        assert!(dest.join("a.txt").exists(), "working tree not checked out");

        // The fresh clone reports the URL it came from — the whole point of the
        // new field, and it resolves via the branch's own remote.
        let d = dest.to_str().unwrap();
        let st = git_status(d, d, &known(&dest)).unwrap();
        assert_eq!(st.remote_url.as_deref(), remote.to_str(), "status: {st:?}");
        // repo_detail carries the same field for the mobile repo surface.
        let detail = repo_detail(d, &known(&dest)).unwrap();
        assert_eq!(detail.remote_url.as_deref(), remote.to_str());
    }

    #[test]
    fn clone_into_existing_empty_dir_is_allowed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let remote = tmp.path().join("remote.git");
        fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "-q", "--bare", "-b", "main"]);
        let seed = tmp.path().join("seed");
        init_repo(&seed);
        git(&seed, &["remote", "add", "origin", remote.to_str().unwrap()]);
        git(&seed, &["push", "-q", "-u", "origin", "main"]);

        let dest = tmp.path().join("empty-dir");
        fs::create_dir_all(&dest).unwrap();
        let res = git_clone(remote.to_str().unwrap(), dest.to_str().unwrap()).unwrap();
        assert!(res.ok, "clone into empty dir failed: {}", res.output);
        assert!(dest.join(".git").exists());
    }

    #[test]
    fn clone_rejects_bad_url_and_unsafe_destinations() {
        let tmp = tempfile::TempDir::new().unwrap();
        let url = "https://example.invalid/x.git";
        let abs = tmp.path().join("ok").to_string_lossy().to_string();

        // Empty / option-looking URLs never reach git.
        assert!(git_clone("  ", &abs).unwrap_err().contains("empty"));
        assert!(git_clone("--upload-pack=evil", &abs)
            .unwrap_err()
            .contains("must not start with '-'"));

        // Relative destination.
        let err = git_clone(url, "relative/dir").unwrap_err();
        assert!(err.contains("absolute"), "got: {err}");

        // Parent directory missing.
        let missing = tmp.path().join("nope").join("child");
        let err = git_clone(url, missing.to_str().unwrap()).unwrap_err();
        assert!(err.contains("parent directory does not exist"), "got: {err}");

        // Destination is a file.
        let file = tmp.path().join("a-file");
        fs::write(&file, "x").unwrap();
        let err = git_clone(url, file.to_str().unwrap()).unwrap_err();
        assert!(err.contains("already exists"), "got: {err}");

        // Destination is a non-empty directory — refusing this is what keeps the
        // clone from ever landing on top of existing work.
        let full = tmp.path().join("full");
        fs::create_dir_all(&full).unwrap();
        fs::write(full.join("keep.txt"), "mine").unwrap();
        let err = git_clone(url, full.to_str().unwrap()).unwrap_err();
        assert!(err.contains("not empty"), "got: {err}");
        assert!(full.join("keep.txt").exists(), "existing file must survive");
    }

    #[test]
    fn remote_url_is_none_without_a_remote_and_falls_back_to_origin() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        init_repo(&ws);
        let w = ws.to_str().unwrap();

        // Plain `git init` repo: no remotes at all.
        assert_eq!(git_status(w, w, &known(&ws)).unwrap().remote_url, None);

        // An `origin` with no tracking branch still resolves via the fallback.
        git(&ws, &["remote", "add", "origin", "git@example.com:o/r.git"]);
        let st = git_status(w, w, &known(&ws)).unwrap();
        assert_eq!(st.remote_url.as_deref(), Some("git@example.com:o/r.git"));
        assert_eq!(st.upstream, None, "no tracking branch: {st:?}");
    }

    // ── Repository overview ─────────────────────────────────────────────────

    #[test]
    fn list_repos_skips_non_git_and_clean_repo_needs_no_attention() {
        let tmp = tempfile::TempDir::new().unwrap();
        // A plain (non-git) workspace contributes no repo.
        let plain = tmp.path().join("plain");
        fs::create_dir_all(&plain).unwrap();
        assert!(list_repos(&[plain.to_string_lossy().to_string()]).is_empty());

        // A clean git repo with no worktrees and no upstream is a repo, but
        // needs no attention.
        let ws = tmp.path().join("ws");
        init_repo(&ws);
        let repos = list_repos(&known(&ws));
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].branch.as_deref(), Some("main"));
        assert_eq!(repos[0].worktree_count, 0);
        assert_eq!(repos[0].pending_worktrees, 0);
        assert!(!repos[0].needs_attention, "clean repo: {:?}", repos[0]);
    }

    #[test]
    fn list_repos_flags_unmerged_worktree_and_dedups_to_main() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        init_repo(&ws);
        // A worktree on a fresh branch with one extra commit not on main.
        let wt = tmp.path().join("wt-feature");
        git(&ws, &["worktree", "add", "-b", "prd/feature", wt.to_str().unwrap()]);
        fs::write(wt.join("f.txt"), "f\n").unwrap();
        git(&wt, &["add", "-A"]);
        git(&wt, &["commit", "-q", "-m", "feature work"]);

        // `known` names BOTH the main checkout and the worktree; they must
        // collapse onto a single repo entry.
        let known = vec![
            ws.to_string_lossy().to_string(),
            wt.to_string_lossy().to_string(),
        ];
        let repos = list_repos(&known);
        assert_eq!(repos.len(), 1, "worktree dedups to its main: {repos:?}");
        let r = &repos[0];
        assert_eq!(r.worktree_count, 1);
        assert_eq!(r.pending_worktrees, 1, "unmerged worktree flagged: {r:?}");
        assert!(r.needs_attention);

        let detail = repo_detail(&r.root, &known).unwrap();
        assert_eq!(detail.worktrees.len(), 1);
        assert_eq!(detail.worktrees[0].branch.as_deref(), Some("prd/feature"));
        assert_eq!(detail.worktrees[0].unmerged, 1, "one commit not in main");
        assert_eq!(detail.worktrees[0].dirty_count, 0);
        assert!(detail.worktrees[0].last_commit_summary.is_some());
        assert!(!detail.commits.is_empty(), "recent commits populated");
    }

    #[test]
    fn repo_detail_reports_dirty_files_for_main_and_worktree() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        init_repo(&ws);
        // A worktree on a fresh branch.
        let wt = tmp.path().join("wt-feature");
        git(&ws, &["worktree", "add", "-b", "prd/feature", wt.to_str().unwrap()]);

        // Main checkout: one untracked file.
        fs::write(ws.join("scratch.txt"), "s\n").unwrap();
        // Worktree: a modified tracked file + an untracked file.
        fs::write(wt.join("a.txt"), "one\ntwo\n").unwrap();
        fs::write(wt.join("new.txt"), "n\n").unwrap();

        let known = vec![
            ws.to_string_lossy().to_string(),
            wt.to_string_lossy().to_string(),
        ];
        let detail = repo_detail(&ws.to_string_lossy(), &known).unwrap();

        // Main checkout dirty list.
        assert_eq!(detail.dirty_count, 1);
        assert_eq!(detail.dirty_files.len(), detail.dirty_count);
        assert_eq!(detail.dirty_files[0].path, "scratch.txt");
        assert_eq!(detail.dirty_files[0].status, "?");

        // Worktree dirty list, sorted by path (a.txt modified, new.txt untracked).
        assert_eq!(detail.worktrees.len(), 1);
        let wtf: Vec<(&str, &str)> = detail.worktrees[0]
            .dirty_files
            .iter()
            .map(|f| (f.path.as_str(), f.status.as_str()))
            .collect();
        assert_eq!(
            wtf,
            vec![("a.txt", "M"), ("new.txt", "?")],
            "worktree dirty_files: {:?}",
            detail.worktrees[0].dirty_files
        );
        assert_eq!(detail.worktrees[0].dirty_count, 2);
    }

    #[test]
    fn list_repos_flags_unpushed_main_branch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let remote = tmp.path().join("remote.git");
        fs::create_dir_all(&remote).unwrap();
        // `-b main` pins the bare repo's HEAD so it doesn't default to the
        // ambient `init.defaultBranch` (often "master" on CI runners, which
        // leaves HEAD dangling after we only push `main` → clone can't check
        // out and status reports no upstream).
        git(&remote, &["init", "-q", "--bare", "-b", "main"]);
        let ws = tmp.path().join("ws");
        init_repo(&ws);
        git(&ws, &["remote", "add", "origin", remote.to_str().unwrap()]);
        git(&ws, &["push", "-q", "-u", "origin", "main"]);

        // Right after the initial push: in sync, no attention needed.
        let repos = list_repos(&known(&ws));
        assert_eq!(repos[0].unpushed, Some(0));
        assert!(!repos[0].needs_attention);

        // One local commit → unpushed by 1 (forgot to push).
        fs::write(ws.join("b.txt"), "b\n").unwrap();
        git(&ws, &["add", "-A"]);
        git(&ws, &["commit", "-q", "-m", "second"]);
        let repos = list_repos(&known(&ws));
        assert_eq!(repos[0].unpushed, Some(1), "repo: {:?}", repos[0]);
        assert!(repos[0].needs_attention);
        assert_eq!(repos[0].pending_worktrees, 0);
    }

    #[test]
    fn repo_detail_and_ops_reject_unknown_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        init_repo(&ws);
        let w = ws.to_str().unwrap();
        for err in [
            repo_detail(w, &[]).unwrap_err(),
            repo_push(w, &[]).unwrap_err(),
            repo_pull(w, &[]).unwrap_err(),
        ] {
            assert!(err.contains("not a known repository"), "got: {err}");
        }
    }
}
