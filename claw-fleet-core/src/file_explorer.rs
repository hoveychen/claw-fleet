//! Read-only workspace file explorer.
//!
//! Lets the desktop app browse the files of a session's workspace (and its
//! linked git worktrees) without opening an IDE. Everything here is
//! read-only; there is deliberately no write/delete surface.
//!
//! Security model — all three entry points take a `known_workspaces` list and
//! reject anything else. Backends build that list with
//! [`browsable_workspaces`]: the workspace paths of sessions the backend
//! already knows about, plus the directories the user explicitly added or
//! cloned ([`crate::browse_paths`]). Both halves are decided server-side; a
//! client can ask the backend to register a path, but cannot widen a read.
//!   • `workspace` must canonicalize to a member of `known_workspaces`.
//!   • `root` must be that workspace itself or one of its linked git
//!     worktrees (enumerated live via git2, never trusted from the client).
//!   • `rel_path` is rejected on absolute paths and `..` components, then
//!     canonicalized and required to stay under the root — so symlinks
//!     pointing outside the root are refused too.
//! This matters for RemoteBackend: `fleet serve` is token-authed, but the
//! token grants session monitoring, not arbitrary filesystem reads.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// Text previews are cut at this many bytes; the frontend shows a
/// "truncated" notice past it.
pub const TEXT_PREVIEW_CAP: u64 = 1024 * 1024; // 1 MiB
/// Images larger than this are reported as plain binary instead of being
/// base64-inlined into the response.
pub const IMAGE_PREVIEW_CAP: u64 = 10 * 1024 * 1024; // 10 MiB

// ── Types ─────────────────────────────────────────────────────────────────────

/// A browsable root: the workspace checkout itself or a linked git worktree.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerRoot {
    /// Display label — branch name for worktrees, directory name otherwise.
    pub label: String,
    /// Absolute path; pass back as `root` to `list_dir` / `read_file`.
    pub path: String,
    /// Checked-out branch, when resolvable.
    pub branch: Option<String>,
    pub is_worktree: bool,
}

/// One row of a single directory level. `list_dir` is deliberately lazy
/// (one level per call): dev workspaces hold `node_modules`/`target` trees
/// with hundreds of thousands of files, so recursive walks are off the table.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerEntry {
    pub name: String,
    /// Path relative to the root, forward slashes.
    pub relative_path: String,
    pub size_bytes: u64,
    pub is_dir: bool,
    pub modified_ms: u64,
    /// Matched by the repo's gitignore rules. Only present in results when
    /// the caller asked for ignored entries.
    pub is_ignored: bool,
    pub is_symlink: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ExplorerFileContent {
    #[serde(rename_all = "camelCase")]
    Text {
        content: String,
        truncated: bool,
        size_bytes: u64,
    },
    #[serde(rename_all = "camelCase")]
    Image {
        base64: String,
        mime: String,
        size_bytes: u64,
    },
    #[serde(rename_all = "camelCase")]
    Binary { size_bytes: u64 },
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Enumerate the browsable roots of `workspace`: the checkout itself plus
/// every linked git worktree of its repository.
pub fn list_roots(workspace: &str, known_workspaces: &[String]) -> Result<Vec<ExplorerRoot>, String> {
    let ws = validate_workspace(workspace, known_workspaces)?;
    Ok(discover_roots(&ws))
}

/// List one directory level under `root` at `rel_path` ("" = the root).
pub fn list_dir(
    workspace: &str,
    root: &str,
    rel_path: &str,
    show_ignored: bool,
    known_workspaces: &[String],
) -> Result<Vec<ExplorerEntry>, String> {
    let ws = validate_workspace(workspace, known_workspaces)?;
    let root = resolve_root(&ws, root)?;
    list_dir_in(&root, rel_path, show_ignored)
}

/// Read a file for preview. Kind is decided by content, not extension:
/// image extensions become base64 previews, NUL bytes in the head mean
/// binary, everything else is (possibly truncated) text.
pub fn read_file(
    workspace: &str,
    root: &str,
    rel_path: &str,
    known_workspaces: &[String],
) -> Result<ExplorerFileContent, String> {
    let ws = validate_workspace(workspace, known_workspaces)?;
    let root = resolve_root(&ws, root)?;
    read_file_at(&root, rel_path)
}

// ── Root-agnostic core ────────────────────────────────────────────────────────
// `root` must already be canonical and vetted by the caller. The scratchpad
// surface below reuses these with a root derived from the session id instead
// of one of the workspace's git checkouts.

fn list_dir_in(
    root: &Path,
    rel_path: &str,
    show_ignored: bool,
) -> Result<Vec<ExplorerEntry>, String> {
    let root = root.to_path_buf();
    let dir = resolve_rel(&root, rel_path)?;
    if !dir.is_dir() {
        return Err("not a directory".into());
    }

    let repo = git2::Repository::open(&root).ok();
    // A workspace can live anywhere the user keeps it, including under a
    // TCC-protected folder; log a backtrace if this read is denied.
    let rd = crate::tcc::guarded_read_dir(&dir).map_err(|e| e.to_string())?;
    let mut out = Vec::new();

    for entry in rd.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };
        if name == ".git" {
            continue;
        }
        let is_symlink = entry
            .file_type()
            .map(|t| t.is_symlink())
            .unwrap_or(false);
        // Follow symlinks for size/kind; fall back to the link's own metadata
        // when the target is dangling.
        let Ok(metadata) = crate::tcc::guarded_metadata(&path).or_else(|_| entry.metadata()) else {
            continue;
        };
        let is_dir = metadata.is_dir();
        let rel = if rel_path.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", rel_path.trim_end_matches('/'), name)
        };
        let is_ignored = repo
            .as_ref()
            .map(|r| r.is_path_ignored(Path::new(&rel)).unwrap_or(false))
            .unwrap_or(false);
        if !show_ignored && is_ignored {
            continue;
        }
        let modified_ms = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        out.push(ExplorerEntry {
            name,
            relative_path: rel,
            size_bytes: if is_dir { 0 } else { metadata.len() },
            is_dir,
            modified_ms,
            is_ignored,
            is_symlink,
        });
    }

    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

fn read_file_at(root: &Path, rel_path: &str) -> Result<ExplorerFileContent, String> {
    let path = resolve_rel(root, rel_path)?;
    if !path.is_file() {
        return Err("not a file".into());
    }
    let metadata = fs::metadata(&path).map_err(|e| e.to_string())?;
    let size_bytes = metadata.len();

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if let Some(mime) = image_mime(&ext) {
        if size_bytes > IMAGE_PREVIEW_CAP {
            return Ok(ExplorerFileContent::Binary { size_bytes });
        }
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        return Ok(ExplorerFileContent::Image {
            base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            mime: mime.to_string(),
            size_bytes,
        });
    }

    let bytes = read_head(&path, TEXT_PREVIEW_CAP as usize)?;
    let sniff = &bytes[..bytes.len().min(8192)];
    if sniff.contains(&0) {
        return Ok(ExplorerFileContent::Binary { size_bytes });
    }
    Ok(ExplorerFileContent::Text {
        content: String::from_utf8_lossy(&bytes).into_owned(),
        truncated: size_bytes > TEXT_PREVIEW_CAP,
        size_bytes,
    })
}

// ── Scratchpad ────────────────────────────────────────────────────────────────
// Claude Code hands each session a private scratch directory and tells it (in
// the system prompt) to put temp files there rather than in /tmp. The layout is
// not a documented contract — it is read off the path CC injects:
//
//     /tmp/claude-<uid>/<workspace-slug>/<session-id>/scratchpad
//
// so we probe for it instead of asserting it. `scratchpad_root` returns None
// when nothing matches and the UI then hides the tab, which keeps a future CC
// layout change a missing tab rather than a wrong directory read.

/// Where CC roots its per-uid scratch trees. Unix is a hardcoded `/tmp` — CC
/// does not honour `$TMPDIR` here, which on macOS is a per-user `/var/folders`
/// path. Windows has no such convention, so fall back to the system temp dir.
fn scratch_tmp_base() -> PathBuf {
    if cfg!(windows) {
        std::env::temp_dir()
    } else {
        PathBuf::from("/tmp")
    }
}

/// Session ids are uuids; refusing anything else keeps the id from ever
/// contributing a `..` or a path separator to the derived root.
fn is_safe_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Locate the scratchpad of `session_id` under `workspace`, or None when there
/// is none — the session never wrote a temp file, it ran on another machine, or
/// CC changed its layout.
pub fn scratchpad_root(workspace: &str, session_id: &str) -> Option<PathBuf> {
    scratchpad_root_in(&scratch_tmp_base(), workspace, session_id)
}

fn scratchpad_root_in(base: &Path, workspace: &str, session_id: &str) -> Option<PathBuf> {
    if !is_safe_session_id(session_id) {
        return None;
    }
    let ws = fs::canonicalize(workspace).ok()?;
    let slug = crate::session::encode_workspace_path(&ws.to_string_lossy());
    // The uid in `claude-<uid>` is whoever ran CC. Scanning for it beats
    // hardcoding getuid(): a remote `fleet serve` may well run as another user.
    for entry in fs::read_dir(base).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("claude-") {
            continue;
        }
        let candidate = entry.path().join(&slug).join(session_id).join("scratchpad");
        if candidate.is_dir() {
            if let Ok(canon) = fs::canonicalize(&candidate) {
                return Some(canon);
            }
        }
    }
    None
}

/// List one level of a session's scratchpad. Errors when the session has no
/// scratchpad — the caller treats that as "hide the tab", not as a failure.
pub fn list_scratchpad_dir(
    workspace: &str,
    session_id: &str,
    rel_path: &str,
    known_workspaces: &[String],
) -> Result<Vec<ExplorerEntry>, String> {
    validate_workspace(workspace, known_workspaces)?;
    let root = scratchpad_root(workspace, session_id).ok_or("no scratchpad for this session")?;
    // Nothing here is a git checkout, so gitignore filtering has no meaning.
    list_dir_in(&root, rel_path, true)
}

pub fn read_scratchpad_file(
    workspace: &str,
    session_id: &str,
    rel_path: &str,
    known_workspaces: &[String],
) -> Result<ExplorerFileContent, String> {
    validate_workspace(workspace, known_workspaces)?;
    let root = scratchpad_root(workspace, session_id).ok_or("no scratchpad for this session")?;
    read_file_at(&root, rel_path)
}

// ── Validation ────────────────────────────────────────────────────────────────

/// Validate `workspace` is known and `root` is one of its browsable roots,
/// returning the canonical root path. Shared with the `git_ops`
/// source-control surface so it inherits the exact same access checks.
pub(crate) fn resolve_validated_root(
    workspace: &str,
    root: &str,
    known_workspaces: &[String],
) -> Result<PathBuf, String> {
    let ws = validate_workspace(workspace, known_workspaces)?;
    resolve_root(&ws, root)
}

/// The full set of directories the explorer may browse: the workspaces the
/// backend derived from session transcripts, plus the ones the user added by
/// hand or cloned from the 仓库 page ([`crate::browse_paths`]).
///
/// Backends call this to build the `known_workspaces` argument the entry points
/// above take, so both halves of the boundary are decided server-side — a
/// freshly cloned repo is browsable because the backend recorded the clone, not
/// because a client asked nicely.
pub fn browsable_workspaces(session_workspaces: &[String]) -> Vec<String> {
    let mut paths: Vec<String> = session_workspaces.to_vec();
    paths.extend(crate::browse_paths::list());
    paths.sort();
    paths.dedup();
    paths
}

fn validate_workspace(workspace: &str, known_workspaces: &[String]) -> Result<PathBuf, String> {
    let ws = fs::canonicalize(workspace).map_err(|e| format!("workspace: {e}"))?;
    let known = known_workspaces
        .iter()
        .any(|k| fs::canonicalize(k).map(|c| c == ws).unwrap_or(false));
    if !known {
        return Err("workspace is not a known session workspace".into());
    }
    Ok(ws)
}

fn resolve_root(ws: &Path, root: &str) -> Result<PathBuf, String> {
    let rc = fs::canonicalize(root).map_err(|e| format!("root: {e}"))?;
    if rc == ws {
        return Ok(rc);
    }
    let allowed = discover_roots(ws).iter().any(|r| {
        fs::canonicalize(&r.path)
            .map(|c| c == rc)
            .unwrap_or(false)
    });
    if !allowed {
        return Err("root is not the workspace or one of its worktrees".into());
    }
    Ok(rc)
}

/// Join + canonicalize `rel` under `root`, refusing absolute paths, `..`
/// components, and symlink escapes. `root` must already be canonical.
fn resolve_rel(root: &Path, rel: &str) -> Result<PathBuf, String> {
    if rel.is_empty() {
        return Ok(root.to_path_buf());
    }
    if Path::new(rel).is_absolute() || rel.split(['/', '\\']).any(|c| c == "..") {
        return Err("path traversal rejected".into());
    }
    let canon = fs::canonicalize(root.join(rel)).map_err(|e| e.to_string())?;
    if !canon.starts_with(root) {
        return Err("path escapes root".into());
    }
    Ok(canon)
}

// ── Roots discovery ───────────────────────────────────────────────────────────

fn discover_roots(ws: &Path) -> Vec<ExplorerRoot> {
    let ws_label = ws
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace")
        .to_string();

    let Ok(repo) = git2::Repository::open(ws) else {
        // Not a git repo — the workspace itself is the only root.
        return vec![ExplorerRoot {
            label: ws_label,
            path: ws.to_string_lossy().to_string(),
            branch: None,
            is_worktree: false,
        }];
    };

    // Main checkout: for a linked worktree, `repo.path()` is
    // `<main>/.git/worktrees/<name>` — three parents up is the main
    // checkout (git2 0.18 has no `commondir()`). Otherwise it is this
    // repo's own workdir.
    let main_workdir = if repo.is_worktree() {
        repo.path()
            .ancestors()
            .nth(3)
            .map(Path::to_path_buf)
    } else {
        repo.workdir().map(Path::to_path_buf)
    };

    let mut roots = Vec::new();
    if let Some(main) = main_workdir.filter(|p| p.is_dir()) {
        roots.push(ExplorerRoot {
            label: main
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("main")
                .to_string(),
            branch: branch_of(&main),
            path: main.to_string_lossy().to_string(),
            is_worktree: false,
        });
    }

    if let Ok(names) = repo.worktrees() {
        let mut worktree_roots: Vec<ExplorerRoot> = names
            .iter()
            .flatten()
            .filter_map(|name| {
                let wt = repo.find_worktree(name).ok()?;
                let path = wt.path().to_path_buf();
                if !path.is_dir() {
                    return None; // pruned / stale worktree
                }
                let branch = branch_of(&path);
                Some(ExplorerRoot {
                    label: branch.clone().unwrap_or_else(|| name.to_string()),
                    path: path.to_string_lossy().to_string(),
                    branch,
                    is_worktree: true,
                })
            })
            .collect();
        worktree_roots.sort_by(|a, b| a.label.cmp(&b.label));
        roots.extend(worktree_roots);
    }

    if roots.is_empty() {
        // Bare repo or unreadable workdir — fall back to the workspace path.
        roots.push(ExplorerRoot {
            label: ws_label,
            path: ws.to_string_lossy().to_string(),
            branch: None,
            is_worktree: false,
        });
    }
    roots
}

fn branch_of(workdir: &Path) -> Option<String> {
    let repo = git2::Repository::open(workdir).ok()?;
    let head = repo.head().ok()?;
    head.shorthand().map(str::to_string)
}

// ── Content helpers ───────────────────────────────────────────────────────────

fn image_mime(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        _ => return None,
    })
}

fn read_head(path: &Path, cap: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    file.take(cap as u64)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Init a git repo with one commit so worktrees can be added.
    fn init_repo(dir: &Path) -> git2::Repository {
        let repo = git2::Repository::init(dir).unwrap();
        {
            let sig = git2::Signature::now("t", "t@t").unwrap();
            let tree_id = {
                let mut index = repo.index().unwrap();
                index.write_tree().unwrap()
            };
            let tree = repo.find_tree(tree_id).unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }
        repo
    }

    fn known(ws: &Path) -> Vec<String> {
        vec![ws.to_string_lossy().to_string()]
    }

    #[test]
    fn rejects_unknown_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = list_dir(tmp.path().to_str().unwrap(), tmp.path().to_str().unwrap(), "", false, &[])
            .unwrap_err();
        assert!(err.contains("not a known session workspace"), "got: {err}");
    }

    /// Regression: a repo cloned from the 仓库 page has no sessions of its own,
    /// so the session-derived list can never contain it and the file tree
    /// answered "workspace is not a known session workspace" — indistinguishable
    /// from a failed clone. Once the backend has recorded the clone destination
    /// in `browse_paths`, `browsable_workspaces` must let it through.
    #[test]
    fn a_user_added_path_is_browsable_even_with_no_sessions() {
        let _lock = crate::session::fleet_home_lock();
        let home = tempfile::TempDir::new().unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        unsafe { std::env::set_var("FLEET_HOME", home.path()) };

        let repo = home.path().join("cloned-repo");
        fs::create_dir_all(repo.join("src")).unwrap();
        let repo_s = fs::canonicalize(&repo).unwrap().to_string_lossy().to_string();
        crate::browse_paths::add(&repo_s).unwrap();

        // No sessions anywhere — exactly the state right after a clone.
        let known = browsable_workspaces(&[]);
        let listed = list_dir(&repo_s, &repo_s, "", false, &known);

        unsafe {
            match prev {
                Some(v) => std::env::set_var("FLEET_HOME", v),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }

        let entries = listed.expect("a user-added path must be browsable");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "src");
    }

    #[test]
    fn rejects_root_outside_workspace_and_worktrees() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        let other = tmp.path().join("other");
        fs::create_dir_all(&ws).unwrap();
        fs::create_dir_all(&other).unwrap();
        let err = list_dir(
            ws.to_str().unwrap(),
            other.to_str().unwrap(),
            "",
            false,
            &known(&ws),
        )
        .unwrap_err();
        assert!(err.contains("not the workspace"), "got: {err}");
    }

    #[test]
    fn rejects_path_traversal() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        fs::create_dir_all(ws.join("sub")).unwrap();
        fs::write(tmp.path().join("victim.txt"), "secret").unwrap();
        let w = ws.to_str().unwrap();

        let err = read_file(w, w, "../victim.txt", &known(&ws)).unwrap_err();
        assert!(err.contains("traversal"), "got: {err}");

        let err = read_file(w, w, "/etc/hosts", &known(&ws)).unwrap_err();
        assert!(err.contains("traversal"), "got: {err}");
    }

    #[test]
    fn rejects_symlink_escaping_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        fs::write(tmp.path().join("victim.txt"), "secret").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(tmp.path().join("victim.txt"), ws.join("link.txt"))
                .unwrap();
            let w = ws.to_str().unwrap();
            let err = read_file(w, w, "link.txt", &known(&ws)).unwrap_err();
            assert!(err.contains("escapes root"), "got: {err}");
        }
    }

    #[test]
    fn lists_single_level_dirs_first() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        fs::create_dir_all(ws.join("src").join("nested")).unwrap();
        fs::write(ws.join("a.txt"), "a").unwrap();
        fs::write(ws.join("src").join("main.rs"), "fn main() {}").unwrap();
        let w = ws.to_str().unwrap();

        let entries = list_dir(w, w, "", false, &known(&ws)).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["src", "a.txt"], "dirs first, then files");
        // Lazy: nested content is NOT included at this level.
        assert!(entries.iter().all(|e| !e.relative_path.contains("nested")));

        let sub = list_dir(w, w, "src", false, &known(&ws)).unwrap();
        let sub_names: Vec<&str> = sub.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(sub_names, vec!["nested", "main.rs"]);
        assert_eq!(sub[1].relative_path, "src/main.rs");
    }

    #[test]
    fn gitignored_entries_hidden_unless_requested() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        init_repo(&ws);
        fs::write(ws.join(".gitignore"), "target/\n").unwrap();
        fs::create_dir_all(ws.join("target")).unwrap();
        fs::write(ws.join("kept.txt"), "x").unwrap();
        let w = ws.to_str().unwrap();

        let hidden = list_dir(w, w, "", false, &known(&ws)).unwrap();
        assert!(hidden.iter().all(|e| e.name != "target"), "target should be filtered");
        assert!(hidden.iter().any(|e| e.name == "kept.txt"));
        assert!(hidden.iter().all(|e| e.name != ".git"), ".git never listed");

        let shown = list_dir(w, w, "", true, &known(&ws)).unwrap();
        let target = shown.iter().find(|e| e.name == "target").expect("target visible");
        assert!(target.is_ignored);
    }

    #[test]
    fn read_file_three_kinds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("hello.rs"), "fn main() {}").unwrap();
        fs::write(ws.join("blob.bin"), [0u8, 1, 2, 0, 4]).unwrap();
        // Tiny valid PNG header is unnecessary — kind is decided by extension.
        fs::write(ws.join("pic.png"), [137u8, 80, 78, 71]).unwrap();
        let w = ws.to_str().unwrap();

        match read_file(w, w, "hello.rs", &known(&ws)).unwrap() {
            ExplorerFileContent::Text { content, truncated, .. } => {
                assert_eq!(content, "fn main() {}");
                assert!(!truncated);
            }
            other => panic!("expected text, got {other:?}"),
        }
        match read_file(w, w, "blob.bin", &known(&ws)).unwrap() {
            ExplorerFileContent::Binary { size_bytes } => assert_eq!(size_bytes, 5),
            other => panic!("expected binary, got {other:?}"),
        }
        match read_file(w, w, "pic.png", &known(&ws)).unwrap() {
            ExplorerFileContent::Image { mime, .. } => assert_eq!(mime, "image/png"),
            other => panic!("expected image, got {other:?}"),
        }
    }

    #[test]
    fn roots_include_linked_worktrees_and_accept_them() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        let repo = init_repo(&ws);
        let wt_path = tmp.path().join("wt-feature");
        repo.worktree("feature", &wt_path, None).unwrap();
        fs::write(wt_path.join("only-here.txt"), "wt").unwrap();
        let w = ws.to_str().unwrap();

        let roots = list_roots(w, &known(&ws)).unwrap();
        assert_eq!(roots.len(), 2, "main + one worktree: {roots:?}");
        assert!(!roots[0].is_worktree);
        let wt_root = roots.iter().find(|r| r.is_worktree).expect("worktree root");
        assert_eq!(wt_root.branch.as_deref(), Some("feature"));

        // The worktree path is accepted as a browse root for this workspace…
        let entries = list_dir(w, &wt_root.path, "", false, &known(&ws)).unwrap();
        assert!(entries.iter().any(|e| e.name == "only-here.txt"));
        // …and file reads under it work.
        match read_file(w, &wt_root.path, "only-here.txt", &known(&ws)).unwrap() {
            ExplorerFileContent::Text { content, .. } => assert_eq!(content, "wt"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    /// Build `<base>/claude-501/<slug>/<sid>/scratchpad` for a workspace, the
    /// layout CC uses, and return the scratchpad path.
    fn make_scratchpad(base: &Path, ws: &Path, sid: &str) -> PathBuf {
        let canon = fs::canonicalize(ws).unwrap();
        let slug = crate::session::encode_workspace_path(&canon.to_string_lossy());
        let pad = base.join("claude-501").join(slug).join(sid).join("scratchpad");
        fs::create_dir_all(&pad).unwrap();
        pad
    }

    #[test]
    fn scratchpad_root_derived_from_slug_and_session_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("tmpbase");
        let ws = tmp.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        let sid = "112e4ce4-87ad-4251-99b6-cf83be9607c9";
        let pad = make_scratchpad(&base, &ws, sid);
        fs::write(pad.join("note.txt"), "hi").unwrap();

        let root = scratchpad_root_in(&base, ws.to_str().unwrap(), sid).expect("scratchpad found");
        assert_eq!(root, fs::canonicalize(&pad).unwrap());

        let entries = list_dir_in(&root, "", true).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "note.txt");
        match read_file_at(&root, "note.txt").unwrap() {
            ExplorerFileContent::Text { content, .. } => assert_eq!(content, "hi"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn scratchpad_root_none_when_absent_or_id_unsafe() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("tmpbase");
        let ws = tmp.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        let sid = "112e4ce4-87ad-4251-99b6-cf83be9607c9";
        let w = ws.to_str().unwrap();

        // Nothing on disk yet — the tab must stay hidden rather than error.
        assert!(scratchpad_root_in(&base, w, sid).is_none());

        make_scratchpad(&base, &ws, sid);
        assert!(scratchpad_root_in(&base, w, sid).is_some());
        // A session that never wrote anything has no directory of its own.
        assert!(scratchpad_root_in(&base, w, "00000000-0000-0000-0000-000000000000").is_none());
        // Ids that could inject path components are refused outright.
        for bad in ["..", "../..", "a/b", "a\\b", ""] {
            assert!(scratchpad_root_in(&base, w, bad).is_none(), "accepted {bad:?}");
        }
    }

    #[test]
    fn scratchpad_rejects_traversal_and_symlink_escape() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().join("tmpbase");
        let ws = tmp.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        fs::write(tmp.path().join("victim.txt"), "secret").unwrap();
        let sid = "112e4ce4-87ad-4251-99b6-cf83be9607c9";
        let pad = make_scratchpad(&base, &ws, sid);
        let root = scratchpad_root_in(&base, ws.to_str().unwrap(), sid).unwrap();

        let err = read_file_at(&root, "../../../victim.txt").unwrap_err();
        assert!(err.contains("traversal"), "got: {err}");
        let err = list_dir_in(&root, "..", true).unwrap_err();
        assert!(err.contains("traversal"), "got: {err}");

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(tmp.path().join("victim.txt"), pad.join("link.txt")).unwrap();
            let err = read_file_at(&root, "link.txt").unwrap_err();
            assert!(err.contains("escapes root"), "got: {err}");
        }
    }

    #[test]
    fn scratchpad_surface_requires_a_known_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        let err = list_scratchpad_dir(ws.to_str().unwrap(), "abc", "", &[]).unwrap_err();
        assert!(err.contains("not a known session workspace"), "got: {err}");
    }

    #[test]
    fn non_git_workspace_has_single_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let ws = tmp.path().join("plain");
        fs::create_dir_all(&ws).unwrap();
        let roots = list_roots(ws.to_str().unwrap(), &known(&ws)).unwrap();
        assert_eq!(roots.len(), 1);
        assert!(!roots[0].is_worktree);
        assert_eq!(roots[0].branch, None);
    }
}
