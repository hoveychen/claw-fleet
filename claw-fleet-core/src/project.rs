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
        KanbanColumn { id: DEFAULT_COLUMN_QUEUED.into(),   name: "Queued".into(),   color: Some("#94a3b8".into()), is_default: true, is_terminal: false, order: 0 },
        KanbanColumn { id: DEFAULT_COLUMN_RUNNING.into(),  name: "Running".into(),  color: Some("#22c55e".into()), is_default: true, is_terminal: false, order: 1 },
        KanbanColumn { id: DEFAULT_COLUMN_PENDING.into(),  name: "Pending".into(),  color: Some("#f59e0b".into()), is_default: true, is_terminal: false, order: 2 },
        KanbanColumn { id: DEFAULT_COLUMN_COMPLETE.into(), name: "Complete".into(), color: Some("#3b82f6".into()), is_default: true, is_terminal: true,  order: 3 },
    ]
}

/// True if `status_id` corresponds to a terminal column on `project` (i.e. the
/// session is "done" and should not surface as a wait-for-input card). The
/// default `complete` column is always terminal even when older persisted
/// projects.json was written before the `is_terminal` field existed.
pub fn is_terminal_status(project: &Project, status_id: &str) -> bool {
    project.kanban_columns.iter().any(|c| {
        c.id == status_id && (c.is_terminal || c.id == DEFAULT_COLUMN_COMPLETE)
    })
}

// ── Types ───────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct KanbanColumn {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub is_default: bool,
    /// `true` for columns that mark a session as finished — used by the
    /// supervisor to suppress the wait-for-input DecisionPanel card and to
    /// stamp `completed_at` on `set_status`. Default `complete` column is
    /// always terminal (see `is_terminal_status` for the backward-compat
    /// fallback applied to legacy projects.json data).
    #[serde(default)]
    pub is_terminal: bool,
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

// ── File operations (move / copy / rename / delete / mkdir / duplicate) ────

/// Move `from` to `to`. If both live on the same filesystem this is a `rename`.
/// Cross-device: falls back to recursive copy + delete-source. Refuses to
/// overwrite an existing destination, and refuses to move a directory into
/// itself or one of its descendants.
pub fn move_path(from: &str, to: &str) -> Result<(), String> {
    let from_p = Path::new(from);
    let to_p = Path::new(to);
    if !from_p.exists() {
        return Err(format!("source not found: {from}"));
    }
    if to_p.exists() {
        return Err(format!("destination already exists: {to}"));
    }
    if let Some(parent) = to_p.parent() {
        if !parent.is_dir() {
            return Err(format!("destination parent not a directory: {}", parent.display()));
        }
    }
    if from_p.is_dir() && is_ancestor(from_p, to_p) {
        return Err("cannot move a directory into itself".into());
    }
    match fs::rename(from_p, to_p) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc_xdev()) => {
            // Cross-device; fall back to recursive copy + delete.
            copy_recursive(from_p, to_p)?;
            delete_recursive(from_p)?;
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Recursively copy `from` to `to`. Refuses to overwrite an existing
/// destination, and refuses to copy a directory into itself.
pub fn copy_path(from: &str, to: &str) -> Result<(), String> {
    let from_p = Path::new(from);
    let to_p = Path::new(to);
    if !from_p.exists() {
        return Err(format!("source not found: {from}"));
    }
    if to_p.exists() {
        return Err(format!("destination already exists: {to}"));
    }
    if from_p.is_dir() && is_ancestor(from_p, to_p) {
        return Err("cannot copy a directory into itself".into());
    }
    copy_recursive(from_p, to_p)
}

/// Rename a file/dir in place (keeps the same parent directory). `new_name`
/// must be a single path component (no `/`). Returns the new absolute path.
pub fn rename_path(path: &str, new_name: &str) -> Result<String, String> {
    if new_name.is_empty() || new_name.contains('/') || new_name == "." || new_name == ".." {
        return Err(format!("invalid name: {new_name}"));
    }
    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("path not found: {path}"));
    }
    let parent = p.parent().ok_or_else(|| "path has no parent".to_string())?;
    let new_path = parent.join(new_name);
    if new_path.exists() {
        return Err(format!("name already taken: {}", new_path.display()));
    }
    fs::rename(p, &new_path).map_err(|e| e.to_string())?;
    Ok(new_path.to_string_lossy().to_string())
}

/// Delete a file or directory. When `to_trash = true` and the platform is
/// macOS, routes through Finder's Trash via `osascript`. Otherwise removes
/// directly with `remove_file` / `remove_dir_all`.
pub fn delete_path(path: &str, to_trash: bool) -> Result<(), String> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("path not found: {path}"));
    }
    if to_trash {
        #[cfg(target_os = "macos")]
        {
            return trash_via_osascript(p);
        }
        #[cfg(not(target_os = "macos"))]
        {
            // No portable trash on Linux/Windows in v1; fall through to hard delete.
        }
    }
    delete_recursive(p)
}

/// Create a new directory `name` inside `parent`. Returns the new absolute path.
pub fn mkdir(parent: &str, name: &str) -> Result<String, String> {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(format!("invalid name: {name}"));
    }
    let parent_p = Path::new(parent);
    if !parent_p.is_dir() {
        return Err(format!("parent not a directory: {parent}"));
    }
    let new_path = parent_p.join(name);
    if new_path.exists() {
        return Err(format!("already exists: {}", new_path.display()));
    }
    fs::create_dir(&new_path).map_err(|e| e.to_string())?;
    Ok(new_path.to_string_lossy().to_string())
}

/// Duplicate a file/dir into the same parent, picking a Finder-style suffix:
/// `name copy`, `name copy 2`, `name copy 3`, ... Returns the new absolute path.
pub fn duplicate_path(path: &str) -> Result<String, String> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("path not found: {path}"));
    }
    let parent = p.parent().ok_or_else(|| "path has no parent".to_string())?;
    let dest = pick_duplicate_dest(parent, p);
    copy_recursive(p, &dest)?;
    Ok(dest.to_string_lossy().to_string())
}

/// Read up to `max_bytes` from a file. Refuses to read directories or files
/// whose size exceeds the limit. Error strings are prefixed (`path not found:`,
/// `path is a directory:`, `file too large:`) so callers can branch on them.
pub fn read_file_bytes(path: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
    let p = Path::new(path);
    let meta = fs::metadata(p).map_err(|e| format!("path not found: {path}: {e}"))?;
    if meta.is_dir() {
        return Err(format!("path is a directory: {path}"));
    }
    let size = meta.len();
    if size > max_bytes {
        return Err(format!("file too large: {size} bytes (max {max_bytes})"));
    }
    fs::read(p).map_err(|e| e.to_string())
}

// ── helpers ────────────────────────────────────────────────────────────────

fn is_ancestor(ancestor: &Path, descendant: &Path) -> bool {
    let a = fs::canonicalize(ancestor).unwrap_or_else(|_| ancestor.to_path_buf());
    let d = match descendant.parent() {
        Some(p) => fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()),
        None => return false,
    };
    d.starts_with(&a)
}

fn copy_recursive(from: &Path, to: &Path) -> Result<(), String> {
    if from.is_dir() {
        fs::create_dir(to).map_err(|e| e.to_string())?;
        for entry in fs::read_dir(from).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let from_child = entry.path();
            let to_child = to.join(entry.file_name());
            copy_recursive(&from_child, &to_child)?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ()).map_err(|e| e.to_string())
    }
}

fn delete_recursive(p: &Path) -> Result<(), String> {
    if p.is_dir() {
        fs::remove_dir_all(p).map_err(|e| e.to_string())
    } else {
        fs::remove_file(p).map_err(|e| e.to_string())
    }
}

fn pick_duplicate_dest(parent: &Path, src: &Path) -> PathBuf {
    let (stem, ext) = match src.file_name().and_then(|n| n.to_str()) {
        Some(name) => match name.rfind('.') {
            Some(i) if i > 0 && !src.is_dir() => (name[..i].to_string(), Some(name[i..].to_string())),
            _ => (name.to_string(), None),
        },
        None => ("untitled".to_string(), None),
    };
    let base_with_copy = format!("{stem} copy");
    for n in 0..1000 {
        let candidate = if n == 0 {
            base_with_copy.clone()
        } else {
            format!("{base_with_copy} {}", n + 1)
        };
        let with_ext = match &ext {
            Some(e) => format!("{candidate}{e}"),
            None => candidate,
        };
        let p = parent.join(&with_ext);
        if !p.exists() {
            return p;
        }
    }
    parent.join(format!("{stem} copy {}", now_ms()))
}

#[cfg(target_os = "macos")]
fn trash_via_osascript(p: &Path) -> Result<(), String> {
    let posix = p.to_string_lossy().replace('"', "\\\"");
    let script = format!(
        "tell application \"Finder\" to delete (POSIX file \"{posix}\" as alias)"
    );
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(())
}

/// `EXDEV` — "cross-device link". Used to detect when `rename` needs a
/// copy + delete fallback. Defined inline so we don't pull in `libc`.
#[cfg(unix)]
fn libc_xdev() -> i32 { 18 }
#[cfg(not(unix))]
fn libc_xdev() -> i32 { 17 } // ERROR_NOT_SAME_DEVICE on Windows

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
    /// `true` when the session has been **proactively** finalised — either
    /// the agent self-marked via `fleet session status <terminal-column>` or
    /// the user picked a terminal-column button on the wait-for-input
    /// DecisionPanel card. NOT set when the supervisor's Pass 1 auto-marks a
    /// session "complete" because the headless `claude --print` process
    /// exited at end-of-turn — that flow leaves the flag false so the card
    /// can pop and ask the user to confirm or resume. `#[serde(default)]`
    /// keeps legacy persisted records (pre-pending-decision-panel PRD)
    /// deserialising as `false`.
    #[serde(default)]
    pub final_by_agent: bool,
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
    fn default_complete_column_is_terminal_others_are_not() {
        let cols = default_kanban_columns();
        for c in &cols {
            if c.id == DEFAULT_COLUMN_COMPLETE {
                assert!(c.is_terminal, "complete column must be terminal");
            } else {
                assert!(!c.is_terminal, "{} must not be terminal", c.id);
            }
        }
    }

    fn project_with(columns: Vec<KanbanColumn>) -> Project {
        Project {
            id: "p1".into(),
            name: "p1".into(),
            workspace: "/tmp".into(),
            concurrency: 1,
            kanban_columns: columns,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn is_terminal_status_handles_default_columns() {
        let p = project_with(default_kanban_columns());
        assert!(is_terminal_status(&p, DEFAULT_COLUMN_COMPLETE));
        assert!(!is_terminal_status(&p, DEFAULT_COLUMN_RUNNING));
        assert!(!is_terminal_status(&p, DEFAULT_COLUMN_QUEUED));
        assert!(!is_terminal_status(&p, DEFAULT_COLUMN_PENDING));
        // Unknown status id never terminal.
        assert!(!is_terminal_status(&p, "nonexistent"));
    }

    #[test]
    fn is_terminal_status_respects_custom_terminal_flag() {
        let mut cols = default_kanban_columns();
        cols.push(KanbanColumn {
            id: "shipped".into(),
            name: "Shipped".into(),
            color: None,
            is_default: false,
            is_terminal: true,
            order: 4,
        });
        cols.push(KanbanColumn {
            id: "blocked".into(),
            name: "Blocked".into(),
            color: None,
            is_default: false,
            is_terminal: false,
            order: 5,
        });
        let p = project_with(cols);
        assert!(is_terminal_status(&p, "shipped"));
        assert!(!is_terminal_status(&p, "blocked"));
    }

    #[test]
    fn is_terminal_status_falls_back_for_legacy_complete_column() {
        // Legacy projects.json deserialises with `is_terminal: false` (serde default)
        // for the default complete column — fallback by id keeps it terminal.
        let legacy_complete = KanbanColumn {
            id: DEFAULT_COLUMN_COMPLETE.into(),
            name: "Complete".into(),
            color: None,
            is_default: true,
            is_terminal: false, // legacy: missing field deserialised as false
            order: 3,
        };
        let p = project_with(vec![legacy_complete]);
        assert!(
            is_terminal_status(&p, DEFAULT_COLUMN_COMPLETE),
            "default complete column must remain terminal even with legacy data"
        );
    }

    #[test]
    fn kanban_column_deserialises_legacy_json_without_is_terminal() {
        // Older projects.json was written before is_terminal existed; serde must
        // tolerate the missing field rather than refusing to parse the file.
        let legacy = r#"{"id":"complete","name":"Complete","color":null,"isDefault":true,"order":3}"#;
        let col: KanbanColumn = serde_json::from_str(legacy).unwrap();
        assert_eq!(col.id, "complete");
        assert!(!col.is_terminal, "missing field deserialises as false");
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

    // ── File operations ──────────────────────────────────────────────────────

    fn touch(path: &Path) {
        fs::write(path, b"x").unwrap();
    }

    #[test]
    fn move_file_same_parent() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        touch(&a);
        move_path(a.to_str().unwrap(), b.to_str().unwrap()).unwrap();
        assert!(!a.exists());
        assert!(b.exists());
    }

    #[test]
    fn move_refuses_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        touch(&a);
        touch(&b);
        let err = move_path(a.to_str().unwrap(), b.to_str().unwrap()).unwrap_err();
        assert!(err.contains("already exists"));
        assert!(a.exists());
    }

    #[test]
    fn move_dir_into_itself_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("parent");
        fs::create_dir(&parent).unwrap();
        let inside = parent.join("inside");
        let err = move_path(parent.to_str().unwrap(), inside.to_str().unwrap()).unwrap_err();
        assert!(err.contains("itself"));
        assert!(parent.exists());
    }

    #[test]
    fn copy_file_creates_independent_copy() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, b"hello").unwrap();
        copy_path(a.to_str().unwrap(), b.to_str().unwrap()).unwrap();
        assert_eq!(fs::read(&a).unwrap(), b"hello");
        assert_eq!(fs::read(&b).unwrap(), b"hello");
        // Mutating the source must not affect the copy.
        fs::write(&a, b"changed").unwrap();
        assert_eq!(fs::read(&b).unwrap(), b"hello");
    }

    #[test]
    fn copy_directory_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("f1.txt"), b"1").unwrap();
        let nested = src.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("f2.txt"), b"2").unwrap();
        let dst = dir.path().join("dst");
        copy_path(src.to_str().unwrap(), dst.to_str().unwrap()).unwrap();
        assert_eq!(fs::read(dst.join("f1.txt")).unwrap(), b"1");
        assert_eq!(fs::read(dst.join("nested").join("f2.txt")).unwrap(), b"2");
    }

    #[test]
    fn rename_invalid_name_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        touch(&a);
        for bad in ["", "..", ".", "with/slash"] {
            let err = rename_path(a.to_str().unwrap(), bad).unwrap_err();
            assert!(err.contains("invalid"), "want invalid for {bad:?}: {err}");
        }
        assert!(a.exists());
    }

    #[test]
    fn rename_collision_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        touch(&a);
        touch(&b);
        let err = rename_path(a.to_str().unwrap(), "b.txt").unwrap_err();
        assert!(err.contains("already taken"));
    }

    #[test]
    fn rename_returns_new_path() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        touch(&a);
        let new_path = rename_path(a.to_str().unwrap(), "z.txt").unwrap();
        assert!(new_path.ends_with("z.txt"));
        assert!(Path::new(&new_path).exists());
    }

    #[test]
    fn delete_file_no_trash() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        touch(&a);
        delete_path(a.to_str().unwrap(), false).unwrap();
        assert!(!a.exists());
    }

    #[test]
    fn delete_dir_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("d");
        fs::create_dir(&d).unwrap();
        fs::write(d.join("f.txt"), b"x").unwrap();
        delete_path(d.to_str().unwrap(), false).unwrap();
        assert!(!d.exists());
    }

    #[test]
    fn mkdir_creates_dir() {
        let dir = tempfile::tempdir().unwrap();
        let new_path = mkdir(dir.path().to_str().unwrap(), "child").unwrap();
        assert!(Path::new(&new_path).is_dir());
    }

    #[test]
    fn mkdir_invalid_name_rejected() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["", "..", "a/b"] {
            let err = mkdir(dir.path().to_str().unwrap(), bad).unwrap_err();
            assert!(err.contains("invalid"), "want invalid for {bad:?}: {err}");
        }
    }

    #[test]
    fn mkdir_existing_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let _ = mkdir(dir.path().to_str().unwrap(), "child").unwrap();
        let err = mkdir(dir.path().to_str().unwrap(), "child").unwrap_err();
        assert!(err.contains("already exists"));
    }

    #[test]
    fn duplicate_picks_finder_style_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        fs::write(&a, b"x").unwrap();
        let dup1 = duplicate_path(a.to_str().unwrap()).unwrap();
        assert!(dup1.ends_with("a copy.txt"));
        let dup2 = duplicate_path(a.to_str().unwrap()).unwrap();
        assert!(dup2.ends_with("a copy 2.txt"));
        let dup3 = duplicate_path(a.to_str().unwrap()).unwrap();
        assert!(dup3.ends_with("a copy 3.txt"));
    }

    #[test]
    fn duplicate_directory_no_extension() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().join("folder");
        fs::create_dir(&d).unwrap();
        fs::write(d.join("inside.txt"), b"y").unwrap();
        let dup = duplicate_path(d.to_str().unwrap()).unwrap();
        assert!(dup.ends_with("folder copy"));
        assert!(Path::new(&dup).is_dir());
        assert_eq!(
            fs::read(Path::new(&dup).join("inside.txt")).unwrap(),
            b"y"
        );
    }

    #[test]
    fn read_file_bytes_returns_contents() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("hello.txt");
        fs::write(&f, b"hello world").unwrap();
        let bytes = read_file_bytes(f.to_str().unwrap(), 1024).unwrap();
        assert_eq!(bytes, b"hello world");
    }

    #[test]
    fn read_file_bytes_rejects_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("big.bin");
        fs::write(&f, vec![0u8; 2048]).unwrap();
        let err = read_file_bytes(f.to_str().unwrap(), 1024).unwrap_err();
        assert!(err.starts_with("file too large:"), "got: {err}");
    }

    #[test]
    fn read_file_bytes_rejects_directory() {
        let dir = tempfile::tempdir().unwrap();
        let err = read_file_bytes(dir.path().to_str().unwrap(), 1024).unwrap_err();
        assert!(err.starts_with("path is a directory:"), "got: {err}");
    }

    #[test]
    fn read_file_bytes_rejects_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.txt");
        let err = read_file_bytes(missing.to_str().unwrap(), 1024).unwrap_err();
        assert!(err.starts_with("path not found:"), "got: {err}");
    }
}
