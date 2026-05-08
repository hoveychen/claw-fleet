//! Project / FleetSession data model.
//!
//! Persisted in `~/.claude/fleet/projects.json` and `~/.claude/fleet/fleet-sessions.json`.
//! Project = workspace directory + concurrency + kanban columns.
//! FleetSession = a Claude session that Fleet itself spawned (vs. observed
//! passively from another tool).
//!
//! P2 ships read-only `list_fleet_sessions()`; P3 adds spawn/queue/state-machine
//! writes into the same file.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::session::real_home_dir;

// ── Default kanban columns ──────────────────────────────────────────────────

pub const DEFAULT_COLUMN_QUEUED: &str = "queued";
pub const DEFAULT_COLUMN_RUNNING: &str = "running";
pub const DEFAULT_COLUMN_PENDING: &str = "pending";
pub const DEFAULT_COLUMN_COMPLETE: &str = "complete";

/// The four hard-coded default kanban columns.
///
/// `is_default = true` columns cannot be renamed, reordered, deleted, or
/// recoloured — UI enforces that. User-added columns are appended after
/// `complete` with `is_default = false`.
pub fn default_kanban_columns() -> Vec<KanbanColumn> {
    vec![
        KanbanColumn { id: DEFAULT_COLUMN_QUEUED.into(),   name: "Queued".into(),   color: Some("#94a3b8".into()), is_default: true, order: 0 },
        KanbanColumn { id: DEFAULT_COLUMN_RUNNING.into(),  name: "Running".into(),  color: Some("#22c55e".into()), is_default: true, order: 1 },
        KanbanColumn { id: DEFAULT_COLUMN_PENDING.into(),  name: "Pending".into(),  color: Some("#f59e0b".into()), is_default: true, order: 2 },
        KanbanColumn { id: DEFAULT_COLUMN_COMPLETE.into(), name: "Complete".into(), color: Some("#3b82f6".into()), is_default: true, order: 3 },
    ]
}

// ── Types ───────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct KanbanColumn {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub is_default: bool,
    pub order: i32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    /// Absolute path to the workspace directory.
    pub workspace: String,
    /// Max number of fleet-managed sessions allowed to run concurrently.
    pub concurrency: u32,
    pub kanban_columns: Vec<KanbanColumn>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInput {
    pub name: String,
    pub workspace: String,
    pub concurrency: Option<u32>,
}

/// Frontend → spawn API input. Used by `crate::supervisor::enqueue`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LauncherForm {
    pub project_id: String,
    pub prompt: String,
    pub context_files: Vec<String>,
    /// `true` to bypass the project's concurrency limit and start immediately.
    pub expedited: bool,
    /// Absolute directory where the `.fleetsession` virtual-file should land.
    /// `None` = no `.fleetsession` file is written (e.g. when launched from
    /// the top-nav launcher). `Some(dir)` = write `<dir>/<sid>.fleetsession`.
    #[serde(default)]
    pub fleetsession_dir: Option<String>,
}

/// One entry returned by [`list_directory`]. `is_fleetsession` is `true` for
/// regular files whose extension is `.fleetsession` so the UI can render them
/// with a session-status icon.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size_bytes: u64,
    pub modified_ms: u64,
    /// `true` for regular files ending in `.fleetsession`.
    pub is_fleetsession: bool,
    /// For `.fleetsession` files, the session id stored inside (parsed from
    /// the JSON body). `None` if the file isn't parseable.
    pub fleet_session_id: Option<String>,
}

/// List entries in a directory under a project's workspace. Returns sorted
/// (dirs first, then files) entries. Hidden files (starting with `.`) are
/// included EXCEPT for typical noise like `.DS_Store`.
pub fn list_directory(dir_path: &str) -> Result<Vec<FileEntry>, String> {
    let path = std::path::Path::new(dir_path);
    if !path.is_dir() {
        return Err(format!("not a directory: {dir_path}"));
    }
    let entries = fs::read_dir(path).map_err(|e| e.to_string())?;
    let mut out: Vec<FileEntry> = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".DS_Store" {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let is_dir = metadata.is_dir();
        let is_fleetsession = !is_dir && name.ends_with(".fleetsession");
        let fleet_session_id = if is_fleetsession {
            fs::read_to_string(&p)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v["id"].as_str().map(|s| s.to_string()))
        } else {
            None
        };
        let modified_ms = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        out.push(FileEntry {
            name,
            path: p.to_string_lossy().to_string(),
            is_dir,
            size_bytes: if is_dir { 0 } else { metadata.len() },
            modified_ms,
            is_fleetsession,
            fleet_session_id,
        });
    }
    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(out)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FleetSession {
    /// Claude session UUID — pre-generated by supervisor and passed to
    /// `claude --session-id` at spawn time.
    pub id: String,
    pub project_id: String,
    pub workspace: String,
    /// Absolute path to the `.fleetsession` file on disk (filled by P7).
    pub fleetsession_path: Option<String>,
    pub prompt: String,
    pub context_files: Vec<String>,
    /// Must match a `KanbanColumn.id` of the owning project. Defaults to
    /// `DEFAULT_COLUMN_QUEUED` on creation.
    pub status: String,
    /// Free-text status reported by the agent that did not match any
    /// `kanban_columns[*].id` (kept for display, does not move the card).
    pub note: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub pid: Option<u32>,
    pub expedited: bool,
}

// ── Storage paths ───────────────────────────────────────────────────────────

fn fleet_dir() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".claude").join("fleet"))
}

fn projects_file() -> Option<PathBuf> {
    fleet_dir().map(|d| d.join("projects.json"))
}

fn sessions_file() -> Option<PathBuf> {
    fleet_dir().map(|d| d.join("fleet-sessions.json"))
}

fn ensure_fleet_dir() -> std::io::Result<PathBuf> {
    let dir = fleet_dir().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no home directory"))?;
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ── Project CRUD ────────────────────────────────────────────────────────────

pub fn list_projects() -> Vec<Project> {
    let Some(path) = projects_file() else { return vec![]; };
    let Ok(s) = fs::read_to_string(&path) else { return vec![]; };
    serde_json::from_str(&s).unwrap_or_default()
}

fn save_projects(projects: &[Project]) -> Result<(), String> {
    let dir = ensure_fleet_dir().map_err(|e| e.to_string())?;
    let path = dir.join("projects.json");
    let s = serde_json::to_string_pretty(projects).map_err(|e| e.to_string())?;
    let mut f = fs::File::create(&path).map_err(|e| e.to_string())?;
    f.write_all(s.as_bytes()).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn create_project(input: ProjectInput) -> Result<Project, String> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err("project name is required".into());
    }
    let workspace = input.workspace.trim();
    if workspace.is_empty() {
        return Err("workspace is required".into());
    }
    if !Path::new(workspace).is_absolute() {
        return Err("workspace must be an absolute path".into());
    }
    let mut projects = list_projects();
    if projects.iter().any(|p| p.workspace == workspace) {
        return Err("workspace already used by another project".into());
    }
    let now = now_ms();
    let new_project = Project {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.into(),
        workspace: workspace.into(),
        concurrency: input.concurrency.unwrap_or(1).max(1),
        kanban_columns: default_kanban_columns(),
        created_at: now,
        updated_at: now,
    };
    projects.push(new_project.clone());
    save_projects(&projects)?;
    Ok(new_project)
}

pub fn update_project(mut updated: Project) -> Result<(), String> {
    let mut projects = list_projects();
    let idx = projects.iter().position(|p| p.id == updated.id)
        .ok_or_else(|| format!("project not found: {}", updated.id))?;
    if projects.iter().enumerate().any(|(i, p)| i != idx && p.workspace == updated.workspace) {
        return Err("workspace already used by another project".into());
    }
    updated.updated_at = now_ms();
    projects[idx] = updated;
    save_projects(&projects)
}

pub fn delete_project(project_id: &str) -> Result<(), String> {
    let mut projects = list_projects();
    let before = projects.len();
    projects.retain(|p| p.id != project_id);
    if projects.len() == before {
        return Err(format!("project not found: {}", project_id));
    }
    save_projects(&projects)
}

// ── FleetSession persistence ────────────────────────────────────────────────

pub fn list_fleet_sessions() -> Vec<FleetSession> {
    let Some(path) = sessions_file() else { return vec![]; };
    let Ok(s) = fs::read_to_string(&path) else { return vec![]; };
    serde_json::from_str(&s).unwrap_or_default()
}

/// Write fleet sessions atomically (write to .tmp, then rename).
/// Used by `crate::supervisor::*` to persist queue / state-machine transitions.
pub fn save_fleet_sessions(sessions: &[FleetSession]) -> Result<(), String> {
    let dir = ensure_fleet_dir().map_err(|e| e.to_string())?;
    let target = dir.join("fleet-sessions.json");
    let tmp = dir.join(".fleet-sessions.json.tmp");
    let s = serde_json::to_string_pretty(sessions).map_err(|e| e.to_string())?;
    fs::write(&tmp, s).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &target).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_columns_are_locked_and_ordered() {
        let cols = default_kanban_columns();
        assert_eq!(cols.len(), 4);
        assert!(cols.iter().all(|c| c.is_default));
        let mut prev = -1;
        for c in &cols {
            assert!(c.order > prev, "columns must be strictly ordered");
            prev = c.order;
        }
        assert_eq!(cols[0].id, DEFAULT_COLUMN_QUEUED);
        assert_eq!(cols[3].id, DEFAULT_COLUMN_COMPLETE);
    }

    #[test]
    fn empty_workspace_rejected() {
        let r = create_project(ProjectInput {
            name: "x".into(),
            workspace: "  ".into(),
            concurrency: None,
        });
        assert!(r.is_err());
    }

    #[test]
    fn relative_workspace_rejected() {
        let r = create_project(ProjectInput {
            name: "x".into(),
            workspace: "relative/path".into(),
            concurrency: None,
        });
        assert!(r.is_err());
    }
}
