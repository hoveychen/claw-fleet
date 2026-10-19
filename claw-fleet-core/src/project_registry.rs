//! The persistent list of projects: one entry per code directory an agent has
//! worked in.
//!
//! A project is a code directory. Deriving the list from the session scan
//! alone is not enough: the scan drops transcripts older than seven days, so a
//! project nobody touched for a week would vanish from the project page. So
//! every directory the scan reports is written here ([`discover`], called
//! whenever a client lists projects), whether Fleet started the session or the
//! user ran an agent by hand; Fleet's own spawns register theirs on the spot
//! ([`ensure`]). History from before this existed comes in once: the launch
//! notes via [`crate::task_migration`], and every Claude Code project directory
//! and Codex thread cwd via [`discover`]'s first run. Projects never expire; the user archives the
//! ones they no longer want listed ([`set_archived`]).
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
    /// When the user archived it (epoch ms). Archived projects stay registered,
    /// so the scan does not bring them back, but the project page hides them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at_ms: Option<u64>,
}

const FILE_NAME: &str = "projects.json";

/// Present once [`discover`] has registered every Claude Code project
/// directory and Codex thread cwd. Without it, the first list after an upgrade
/// would only know the last seven days of hand-run sessions. v1 covered Claude
/// Code only; registering is idempotent, so v2 simply runs both again.
const BACKFILL_MARKER: &str = "projects-backfill-v2";

fn registry_path() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join(FILE_NAME))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Every registered project, archived ones included, in registration order.
pub fn list() -> Vec<Project> {
    registry_path().map(|p| load(&p)).unwrap_or_default()
}

fn load(path: &Path) -> Vec<Project> {
    match crate::atomic_json::load_preserving::<Vec<Project>>(path) {
        crate::atomic_json::JsonLoad::Loaded(v) => v,
        _ => Vec::new(),
    }
}

/// The registry for a read-modify-write. Missing and corrupt files start over;
/// an unreadable one is an error, since writing now would drop every entry.
fn load_for_write(path: &Path) -> Result<Vec<Project>, String> {
    match crate::atomic_json::load_preserving::<Vec<Project>>(path) {
        crate::atomic_json::JsonLoad::Loaded(v) => Ok(v),
        crate::atomic_json::JsonLoad::Missing | crate::atomic_json::JsonLoad::Corrupt => Ok(Vec::new()),
        crate::atomic_json::JsonLoad::Unreadable => Err(format!("read {}: unreadable", path.display())),
    }
}

fn save(path: &Path, projects: &[Project]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_vec_pretty(projects).map_err(|e| format!("serialize: {e}"))?;
    crate::atomic_json::write_atomic(path, &json).map_err(|e| format!("write {}: {e}", path.display()))
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
    // Stat-ing a path under a TCC-protected folder (~/Documents, ~/Desktop, …)
    // is what raises macOS's permission dialog. An agent worked there, so take
    // its word that the directory exists rather than prompt from a list call.
    if crate::tcc::is_tcc_protected(Path::new(root)) {
        return Some(root.to_string());
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
/// skipped; archived projects stay archived.
pub fn ensure_many<'a>(workspaces: impl IntoIterator<Item = &'a str>) -> Result<usize, String> {
    let path = registry_path().ok_or("cannot determine fleet dir")?;
    ensure_many_in(&path, workspaces, is_temp, false)
}

/// Register a workspace Fleet is starting a task in. Called on every spawn; one
/// file read when the project is already known. Starting new work in an
/// archived project un-archives it: the user just picked it.
pub fn ensure(workspace: &str) -> Result<bool, String> {
    let path = registry_path().ok_or("cannot determine fleet dir")?;
    ensure_many_in(&path, [workspace], is_temp, true).map(|n| n > 0)
}

/// Register every directory the session scan saw an agent in (`workspaces`),
/// and on the first run every Claude Code project directory under
/// `~/.claude/projects` and every thread cwd in Codex's SQLite index, so
/// history older than the scan's seven days is not lost. Returns how many
/// projects were added.
pub fn discover<'a>(workspaces: impl IntoIterator<Item = &'a str>) -> Result<usize, String> {
    let path = registry_path().ok_or("cannot determine fleet dir")?;
    discover_in(&path, workspaces, history_workspaces, is_temp)
}

/// Every directory an agent has a record of working in, however old: Claude
/// Code's project directories and the cwd of every thread in Codex's index.
fn history_workspaces() -> Vec<String> {
    let mut history = crate::memory::claude_project_workspaces();
    history.extend(crate::codex_source::all_thread_rollout_cwds().into_values().map(|(_, cwd)| cwd));
    history.sort();
    history.dedup();
    history
}

fn discover_in<'a>(
    path: &Path,
    workspaces: impl IntoIterator<Item = &'a str>,
    history: impl FnOnce() -> Vec<String>,
    is_temp: impl Fn(&str) -> bool,
) -> Result<usize, String> {
    let mut added = 0;
    let marker = path.with_file_name(BACKFILL_MARKER);
    if !marker.exists() {
        let history = history();
        added += ensure_many_in(path, history.iter().map(String::as_str), &is_temp, false)?;
        std::fs::write(&marker, b"").map_err(|e| format!("write {}: {e}", marker.display()))?;
    }
    added += ensure_many_in(path, workspaces, &is_temp, false)?;
    Ok(added)
}

/// Archive or un-archive a project, registering it first if the list only had
/// it from the scan. Returns the updated registry.
pub fn set_archived(project: &str, archived: bool) -> Result<Vec<Project>, String> {
    let path = registry_path().ok_or("cannot determine fleet dir")?;
    set_archived_in(&path, project, archived, now_ms())
}

fn set_archived_in(path: &Path, project: &str, archived: bool, now: u64) -> Result<Vec<Project>, String> {
    let root = project.trim().trim_end_matches('/');
    if root.is_empty() {
        return Err("empty project path".into());
    }
    let mut projects = load_for_write(path)?;
    let at = archived.then_some(now);
    match projects.iter_mut().find(|p| p.path == root) {
        Some(p) => p.archived_at_ms = at,
        None => projects.push(Project {
            name: crate::session::workspace_name(root),
            path: root.to_string(),
            created_at_ms: now,
            archived_at_ms: at,
        }),
    }
    save(path, &projects)?;
    Ok(projects)
}

fn ensure_many_in<'a>(
    path: &Path,
    workspaces: impl IntoIterator<Item = &'a str>,
    is_temp: impl Fn(&str) -> bool,
    unarchive: bool,
) -> Result<usize, String> {
    let mut projects = load_for_write(path)?;
    let now = now_ms();
    let mut added = 0;
    let mut changed = false;
    for ws in workspaces {
        let Some(root) = project_path_with(ws, &is_temp) else {
            continue;
        };
        if let Some(p) = projects.iter_mut().find(|p| p.path == root) {
            if unarchive && p.archived_at_ms.take().is_some() {
                changed = true;
            }
            continue;
        }
        projects.push(Project {
            name: crate::session::workspace_name(&root),
            path: root,
            created_at_ms: now,
            archived_at_ms: None,
        });
        added += 1;
        changed = true;
    }
    if changed {
        save(path, &projects)?;
    }
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
            false,
        )
        .unwrap();
        assert_eq!(added, 1);
        let list = load(&file);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].path, repo_s);
        assert_eq!(list[0].name, "repo");

        // Idempotent: a second pass adds nothing.
        assert_eq!(ensure_many_in(&file, [repo_s.as_str()], fake_temp, false).unwrap(), 0);
    }

    #[test]
    fn trailing_slash_is_the_same_project() {
        let base = tempfile::tempdir().unwrap();
        let file = base.path().join("projects.json");
        let p = base.path().to_str().unwrap().to_string();
        let with_slash = format!("{p}/");
        assert_eq!(
            ensure_many_in(&file, [p.as_str(), with_slash.as_str()], fake_temp, false).unwrap(),
            1
        );
    }

    #[test]
    fn archive_survives_rediscovery_and_a_spawn_unarchives() {
        let base = tempfile::tempdir().unwrap();
        let file = base.path().join("projects.json");
        let repo = base.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_s = repo.to_str().unwrap().to_string();
        ensure_many_in(&file, [repo_s.as_str()], fake_temp, false).unwrap();

        let list = set_archived_in(&file, &format!("{repo_s}/"), true, 42).unwrap();
        assert_eq!(list[0].archived_at_ms, Some(42));

        // The scan seeing it again changes nothing.
        assert_eq!(ensure_many_in(&file, [repo_s.as_str()], fake_temp, false).unwrap(), 0);
        assert_eq!(load(&file)[0].archived_at_ms, Some(42));

        // A Fleet spawn into it brings it back.
        ensure_many_in(&file, [repo_s.as_str()], fake_temp, true).unwrap();
        assert_eq!(load(&file)[0].archived_at_ms, None);
    }

    #[test]
    fn archiving_an_unregistered_project_registers_it_archived() {
        let base = tempfile::tempdir().unwrap();
        let file = base.path().join("projects.json");
        let list = set_archived_in(&file, "/Users/me/workspace/old", true, 7).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].path, "/Users/me/workspace/old");
        assert_eq!(list[0].name, "old");
        assert_eq!(list[0].archived_at_ms, Some(7));
        // Pre-archive files have no field and still load.
        std::fs::write(&file, r#"[{"path":"/a","name":"a","createdAtMs":1}]"#).unwrap();
        assert_eq!(load(&file)[0].archived_at_ms, None);
    }

    #[test]
    fn temp_dirs_are_not_projects() {
        assert!(is_temp("/tmp/x"));
        assert!(is_temp("/private/tmp/claude-501/scratchpad"));
        assert!(is_temp("/private/var/folders/ab/T/x"));
        assert!(!is_temp("/Users/me/workspace/proj"));
        assert!(!is_temp("/tmpfoo"));
    }

    #[test]
    fn first_discover_backfills_history_once() {
        let base = tempfile::tempdir().unwrap();
        let codex_repo = base.path().join("codex-repo");
        let live = base.path().join("live");
        std::fs::create_dir_all(&codex_repo).unwrap();
        std::fs::create_dir_all(&live).unwrap();
        let file = base.path().join("projects.json");
        let codex_s = codex_repo.to_str().unwrap().to_string();

        let history = || vec![codex_s.clone(), "/scratch-not-a-project/t".to_string()];
        let added = discover_in(&file, [live.to_str().unwrap()], history, fake_temp).unwrap();
        assert_eq!(added, 2);
        assert!(file.with_file_name(BACKFILL_MARKER).exists());

        // The marker stops a second backfill: history is not read again.
        let added = discover_in(&file, [], || panic!("history read twice"), fake_temp).unwrap();
        assert_eq!(added, 0);
        assert_eq!(load(&file).len(), 2);
    }
}
