//! The persistent list of projects: one entry per code directory Fleet has
//! started a task in.
//!
//! A project is a code directory. Deriving the list from the session scan
//! alone is not enough: the scan drops transcripts older than seven days, so a
//! project nobody touched for a week would vanish from the project page. The
//! registry is seeded once from the launch notes by [`crate::task_migration`]
//! and kept current by [`ensure`] on every spawn.
//!
//! Stored at `~/.fleet/projects.json` as a JSON array of [`Project`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// The code directory — a repo root, never a `.worktrees/<id>` checkout.
    pub path: String,
    /// Display name, from [`crate::session::workspace_name`].
    pub name: String,
    /// When Fleet first registered it (epoch ms).
    pub created_at_ms: u64,
}

const FILE_NAME: &str = "projects.json";

fn registry_path() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join(FILE_NAME))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Every registered project, in registration order.
pub fn list() -> Vec<Project> {
    registry_path().map(|p| load(&p)).unwrap_or_default()
}

fn load(path: &Path) -> Vec<Project> {
    match crate::atomic_json::load_preserving::<Vec<Project>>(path) {
        crate::atomic_json::JsonLoad::Loaded(v) => v,
        _ => Vec::new(),
    }
}

/// The project a workspace path belongs to: its repo root, when that is a
/// directory worth calling a project. `None` for temp dirs, Fleet's chat
/// workspace, and directories that no longer exist.
pub fn project_path_of(workspace: &str) -> Option<String> {
    project_path_with(workspace, is_temp)
}

fn project_path_with(workspace: &str, is_temp: impl Fn(&str) -> bool) -> Option<String> {
    let root = crate::session::repo_root_path(workspace.trim());
    let root = root.trim_end_matches('/');
    if root.is_empty() || crate::chat_workspace::is_chat_workspace(root) || is_temp(root) {
        return None;
    }
    Path::new(root).is_dir().then(|| root.to_string())
}

fn is_temp(path: &str) -> bool {
    let p = Path::new(path);
    let mut temp_roots = vec![
        PathBuf::from("/tmp"),
        PathBuf::from("/private/tmp"),
        PathBuf::from("/var/folders"),
        PathBuf::from("/private/var/folders"),
    ];
    temp_roots.push(std::env::temp_dir());
    temp_roots.iter().any(|root| p.starts_with(root))
}

/// Register the project of each workspace that is not registered yet. Returns
/// how many were added. Workspaces with no project ([`project_path_of`]) are
/// skipped.
pub fn ensure_many<'a>(workspaces: impl IntoIterator<Item = &'a str>) -> Result<usize, String> {
    let path = registry_path().ok_or("cannot determine fleet dir")?;
    ensure_many_in(&path, workspaces, is_temp)
}

/// [`ensure_many`] for one workspace. Called on every spawn; one file read
/// when the project is already known.
pub fn ensure(workspace: &str) -> Result<bool, String> {
    ensure_many([workspace]).map(|n| n > 0)
}

fn ensure_many_in<'a>(
    path: &Path,
    workspaces: impl IntoIterator<Item = &'a str>,
    is_temp: impl Fn(&str) -> bool,
) -> Result<usize, String> {
    let mut projects = match crate::atomic_json::load_preserving::<Vec<Project>>(path) {
        crate::atomic_json::JsonLoad::Loaded(v) => v,
        crate::atomic_json::JsonLoad::Missing | crate::atomic_json::JsonLoad::Corrupt => Vec::new(),
        // The file may be intact; writing now would drop every entry in it.
        crate::atomic_json::JsonLoad::Unreadable => {
            return Err(format!("read {}: unreadable", path.display()))
        }
    };
    let now = now_ms();
    let mut added = 0;
    for ws in workspaces {
        let Some(root) = project_path_with(ws, &is_temp) else {
            continue;
        };
        if projects.iter().any(|p| p.path == root) {
            continue;
        }
        projects.push(Project {
            name: crate::session::workspace_name(&root),
            path: root,
            created_at_ms: now,
        });
        added += 1;
    }
    if added == 0 {
        return Ok(0);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_vec_pretty(&projects).map_err(|e| format!("serialize: {e}"))?;
    crate::atomic_json::write_atomic(path, &json)
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixtures live in the system temp dir, which the real filter treats
    /// as "not a project"; tests swap in a filter for a single fake temp root.
    fn fake_temp(path: &str) -> bool {
        path.starts_with("/scratch-not-a-project")
    }

    #[test]
    fn folds_worktrees_dedupes_and_skips_non_projects() {
        let base = tempfile::tempdir().unwrap();
        let repo = base.path().join("repo");
        let wt = repo.join(".worktrees").join("task-a");
        std::fs::create_dir_all(&wt).unwrap();
        let file = base.path().join("projects.json");
        let repo_s = repo.to_str().unwrap().to_string();
        let gone = base.path().join("deleted");

        let added = ensure_many_in(
            &file,
            [
                repo_s.as_str(),
                wt.to_str().unwrap(),
                gone.to_str().unwrap(),
                "/scratch-not-a-project/x",
            ],
            fake_temp,
        )
        .unwrap();
        assert_eq!(added, 1);
        let list = load(&file);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].path, repo_s);
        assert_eq!(list[0].name, "repo");

        // Idempotent: a second pass adds nothing.
        assert_eq!(ensure_many_in(&file, [repo_s.as_str()], fake_temp).unwrap(), 0);
    }

    #[test]
    fn trailing_slash_is_the_same_project() {
        let base = tempfile::tempdir().unwrap();
        let file = base.path().join("projects.json");
        let p = base.path().to_str().unwrap().to_string();
        let with_slash = format!("{p}/");
        assert_eq!(
            ensure_many_in(&file, [p.as_str(), with_slash.as_str()], fake_temp).unwrap(),
            1
        );
    }

    #[test]
    fn temp_dirs_are_not_projects() {
        assert!(is_temp("/tmp/x"));
        assert!(is_temp("/private/tmp/claude-501/scratchpad"));
        assert!(is_temp("/private/var/folders/ab/T/x"));
        assert!(!is_temp("/Users/me/workspace/proj"));
        assert!(!is_temp("/tmpfoo"));
    }
}
