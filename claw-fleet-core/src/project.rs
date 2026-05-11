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
    /// When `true`, every P-item of every task in this project gets
    /// `human_gate = true` on task start, forcing the Master to call
    /// `AskUserQuestion` before flipping any P-item to Done. PRD §5.x /
    /// TASKS P14 — "manual review all" project-wide toggle. Per-task
    /// override happens at planner time. Defaults to `false` (legacy
    /// behavior).
    #[serde(default)]
    pub manual_review_all: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInput {
    pub name: String,
    pub workspace: String,
    pub concurrency: Option<u32>,
    #[serde(default)]
    pub manual_review_all: Option<bool>,
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

/// What flavour of Claude Code session this is. `Regular` is the legacy
/// fleet-managed session (one-shot prompt → kanban). `Master` and `Worker`
/// are the task-as-unit V1 cluster: Master coordinates a Task's lifecycle and
/// runs `fleet task *` tools; Worker is dispatched per-P-item with a
/// scoped SYSTEM prompt and a `touches`-gated edit hook.
///
/// `#[serde(default)]` on the FleetSession field keeps legacy persisted
/// sessions (no `sessionKind`) deserialising as `Regular`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum SessionKind {
    #[default]
    Regular,
    Master,
    Worker,
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
    /// Discriminator for the task-as-unit cluster (Master / Worker) vs the
    /// legacy fleet-managed launcher sessions (Regular).
    #[serde(default)]
    pub session_kind: SessionKind,
    /// For Master/Worker: the owning `Task.id`. None for Regular sessions.
    #[serde(default)]
    pub task_id: Option<String>,
    /// For Worker: the `PItemId` this worker is executing. None otherwise.
    #[serde(default)]
    pub p_item_id: Option<String>,
    /// `--append-system-prompt` payload for Master/Worker sessions. The
    /// composed Layer-1/2/3 prompt lives here at spawn time so we don't have
    /// to re-derive it across supervisor restarts.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Override model for Master/Worker (`claude-sonnet-4-6`). None →
    /// supervisor uses the CLI default.
    #[serde(default)]
    pub model: Option<String>,
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
        manual_review_all: input.manual_review_all.unwrap_or(false),
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
            manual_review_all: false,
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
            manual_review_all: None,
        });
        assert!(r.is_err());
    }

    #[test]
    fn relative_workspace_rejected() {
        let r = create_project(ProjectInput {
            name: "x".into(),
            workspace: "relative/path".into(),
            concurrency: None,
            manual_review_all: None,
        });
        assert!(r.is_err());
    }
}
