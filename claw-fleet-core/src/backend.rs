//! Backend trait — abstraction over local (file-based) and remote (HTTP probe)
//! data sources.  Both `LocalBackend` and `RemoteBackend` implement this trait
//! so that all Tauri command handlers can be written as simple delegations with
//! no `if remote { … } else { … }` branching.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::account::{AccountInfo, UsageHistoryPoint};
use crate::audit::{AuditRuleInfo, AuditSummary, SuggestedRule};
use crate::daily_report::{DailyReport, DailyReportStats, Lesson};
use crate::llm_provider::{LlmConfig, LlmProviderInfo};
use crate::llm_usage::FleetLlmUsageDailyBucket;
use crate::memory::{MemoryHistoryEntry, WorkspaceMemory};
use crate::search_index::SearchHit;
use crate::session::SessionInfo;
use crate::skill_history::SkillInvocation;
use crate::skills::{SkillFileEntry, SkillItem};

// ── Shared types ─────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DetectedTools {
    pub cli: bool,
    pub vscode: bool,
    pub jetbrains: bool,
    pub desktop: bool,
    pub cursor: bool,
    pub openclaw: bool,
    pub codex: bool,
}

impl Default for DetectedTools {
    fn default() -> Self {
        DetectedTools {
            cli: false,
            vscode: false,
            jetbrains: false,
            desktop: false,
            cursor: false,
            openclaw: false,
            codex: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SetupStatus {
    pub cli_installed: bool,
    pub cli_path: Option<String>,
    pub claude_dir_exists: bool,
    pub detected_tools: DetectedTools,
    pub logged_in: bool,
    pub has_sessions: bool,
    pub credentials_valid: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WaitingAlert {
    pub session_id: String,
    pub workspace_name: String,
    pub summary: String,
    pub detected_at_ms: u64,
    pub jsonl_path: String,
    /// Originating agent source id (e.g. "claude-code", "cursor", "codex").
    /// Used by the UI to suppress audible alerts for sources whose waits are
    /// already surfaced through the Decision Panel (AskUserQuestion bridge).
    pub source: String,
}

// ── Pending decisions snapshot (mount catch-up) ──────────────────────────────

/// Snapshot of every decision-panel request currently awaiting a response,
/// across all five file-IPC channels. Returned by [`Backend::list_pending_decisions`]
/// so the frontend can pull outstanding decisions on mount instead of relying
/// solely on the one-shot watcher emit (which is lost if no Tauri listener is
/// attached at emit time — e.g. on a cold app restart while a `fleet
/// elicitation` / `fleet mcp` child process is still blocking on its poll).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PendingDecisions {
    pub guard: Vec<crate::guard::GuardRequest>,
    pub elicitation: Vec<crate::elicitation::ElicitationRequest>,
    pub fleet_ask: Vec<crate::mcp_ipc::FleetAskRequest>,
    pub a2ui_render: Vec<crate::mcp_a2ui_ipc::A2uiRenderRequest>,
    pub plan_approval: Vec<crate::plan_approval::PlanApprovalRequest>,
    /// Native permission prompts from headless sessions, routed through the
    /// `fleet__permission_prompt` MCP tool (`--permission-prompt-tool`).
    /// `#[serde(default)]` keeps older `fleet serve` probes (whose
    /// `/pending_decisions` payload predates this field) deserializable.
    #[serde(default)]
    pub permission_prompt: Vec<crate::permission_prompt_ipc::PermissionPromptRequest>,
}

/// Fill in each pending request's `workspace_name` / `ai_title` from the
/// session cache, mirroring `local_backend::resolve_session_display` so
/// mount-catch-up cards show the same workspace/title labels the live watcher
/// would have stamped. Only fills empty `workspace_name` and `None` `ai_title`
/// so a request that already carries display info is left untouched.
pub fn resolve_pending_display(pending: &mut PendingDecisions, sessions: &[SessionInfo]) {
    let lookup = |session_id: &str| -> Option<(String, Option<String>)> {
        sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| (s.workspace_name.clone(), s.ai_title.clone()))
    };
    macro_rules! resolve_vec {
        ($v:expr) => {
            for req in $v.iter_mut() {
                if let Some((ws, ai)) = lookup(&req.session_id) {
                    if req.workspace_name.is_empty() {
                        req.workspace_name = ws;
                    }
                    if req.ai_title.is_none() {
                        req.ai_title = ai;
                    }
                }
            }
        };
    }
    resolve_vec!(pending.guard);
    resolve_vec!(pending.elicitation);
    resolve_vec!(pending.fleet_ask);
    resolve_vec!(pending.a2ui_render);
    resolve_vec!(pending.plan_approval);
    resolve_vec!(pending.permission_prompt);
}

// ── Unified usage summary for tray / overview ───────────────────────────────

/// A single rate-limit bar (e.g. "5h", "7d Opus", "Premium requests").
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UsageBar {
    pub label: String,
    /// 0.0–1.0
    pub utilization: f64,
    pub resets_at: Option<String>,
}

/// Normalised usage snapshot for one agent source.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SourceUsageSummary {
    /// Source identifier: "claude", "cursor", "codex", "openclaw".
    pub source: String,
    /// Plan / tier label (e.g. "Max 5x", "pro", "Plus").
    pub plan: Option<String>,
    /// Rate-limit windows, each with a utilization bar.
    pub bars: Vec<UsageBar>,
}

impl SourceUsageSummary {
    /// Convert Claude's `AccountInfo` into a unified summary.
    pub fn from_claude(info: &AccountInfo) -> Self {
        let mut bars = Vec::new();
        if let Some(ref fh) = info.five_hour {
            bars.push(UsageBar {
                label: "5h".into(),
                utilization: fh.utilization,
                resets_at: Some(fh.resets_at.clone()),
            });
        }
        if let Some(ref sd) = info.seven_day {
            bars.push(UsageBar {
                label: "7d Opus".into(),
                utilization: sd.utilization,
                resets_at: Some(sd.resets_at.clone()),
            });
        }
        if let Some(ref ss) = info.seven_day_sonnet {
            bars.push(UsageBar {
                label: "7d Sonnet".into(),
                utilization: ss.utilization,
                resets_at: Some(ss.resets_at.clone()),
            });
        }
        SourceUsageSummary {
            source: "claude".into(),
            plan: if info.plan.is_empty() { None } else { Some(info.plan.clone()) },
            bars,
        }
    }

    /// Convert Cursor's `CursorAccountInfo` JSON value into a unified summary.
    pub fn from_cursor(val: &Value) -> Self {
        let membership = val["membershipType"].as_str().unwrap_or("").to_string();
        let mut bars = Vec::new();
        if let Some(items) = val["usage"].as_array() {
            for item in items {
                let name = item["name"].as_str().unwrap_or("unknown");
                let utilization = item["utilization"].as_f64().unwrap_or_else(|| {
                    let used = item["used"].as_u64().unwrap_or(0) as f64;
                    let limit = item["limit"].as_u64().unwrap_or(1) as f64;
                    if limit > 0.0 { used / limit } else { 0.0 }
                });
                bars.push(UsageBar {
                    label: name.to_string(),
                    utilization,
                    resets_at: item["resetsAt"].as_str().map(|s| s.to_string()),
                });
            }
        }
        SourceUsageSummary {
            source: "cursor".into(),
            plan: if membership.is_empty() { None } else { Some(membership) },
            bars,
        }
    }

    /// Convert Codex's `CodexUsageItem` JSON value into a unified summary.
    pub fn from_codex(val: &Value) -> Self {
        let plan = val["planType"].as_str().map(|s| s.to_string());
        let mut bars = Vec::new();
        if let Some(primary) = val.get("primary") {
            let pct = primary["usedPercent"].as_i64().unwrap_or(0);
            let resets = primary["resetsAt"].as_i64()
                .map(|ts| chrono::DateTime::from_timestamp(ts, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default());
            bars.push(UsageBar {
                label: "Primary".into(),
                utilization: pct as f64 / 100.0,
                resets_at: resets,
            });
        }
        if let Some(secondary) = val.get("secondary") {
            let pct = secondary["usedPercent"].as_i64().unwrap_or(0);
            let resets = secondary["resetsAt"].as_i64()
                .map(|ts| chrono::DateTime::from_timestamp(ts, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default());
            bars.push(UsageBar {
                label: "Secondary".into(),
                utilization: pct as f64 / 100.0,
                resets_at: resets,
            });
        }
        SourceUsageSummary {
            source: "codex".into(),
            plan,
            bars,
        }
    }

    /// Convert OpenClaw's `OpenClawUsageInfo` JSON value into a unified summary.
    /// Shows the highest context utilisation across active sessions.
    pub fn from_openclaw(val: &Value) -> Self {
        let mut bars = Vec::new();
        if let Some(sessions) = val["sessions"].as_array() {
            // Pick the session with the highest context usage as the representative bar.
            if let Some(top) = sessions.iter()
                .filter_map(|s| s["percentUsed"].as_f64().map(|p| (p, s)))
                .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            {
                let model = top.1["model"].as_str().unwrap_or("unknown");
                bars.push(UsageBar {
                    label: format!("ctx ({})", model),
                    utilization: top.0 / 100.0,
                    resets_at: None,
                });
            }
        }
        SourceUsageSummary {
            source: "openclaw".into(),
            plan: None,
            bars,
        }
    }
}

// ── Backend trait ─────────────────────────────────────────────────────────────

/// A boxed, Send future returning `Result<T, String>`.
pub type AccountInfoFuture = Pin<Box<dyn Future<Output = Result<AccountInfo, String>> + Send>>;
/// Generic future for per-source account/usage data (returns untyped JSON).
pub type SourceDataFuture = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;

pub trait Backend: Send + Sync {
    fn list_sessions(&self) -> Vec<SessionInfo>;
    fn get_messages(&self, path: &str) -> Result<Vec<Value>, String>;
    /// Read at most the last `n` messages. Used by SessionDetail to avoid
    /// loading huge transcripts on open. Default impl delegates to
    /// `get_messages` and slices; backends should override when a faster
    /// tail-only path is available (e.g. local filesystem reverse scan,
    /// HTTP `/messages?tail=N`).
    fn get_messages_tail(&self, path: &str, n: usize) -> Result<Vec<Value>, String> {
        let all = self.get_messages(path)?;
        let start = all.len().saturating_sub(n);
        Ok(all[start..].to_vec())
    }
    fn kill_pid(&self, pid: u32) -> Result<(), String>;
    fn kill_workspace(&self, workspace_path: String) -> Result<(), String>;
    /// Headlessly resume a session: spawns `claude --resume <session_id>
    /// -p <prompt>` detached in the given workspace directory. `prompt` of
    /// `None`/empty means "continue" (the rate-limit auto-resume shape); the
    /// history panel passes the user's follow-up text.
    fn resume_session(
        &self,
        session_id: String,
        workspace_path: String,
        prompt: Option<String>,
    ) -> Result<(), String>;
    /// Start a brand-new headless Claude Code session: spawns
    /// `claude -p "<prompt>" [--model <m>] [--effort <e>]
    /// [--permission-mode <pm>]` detached in `workspace_path`. The session is
    /// discovered by the scanner via its own JSONL. Returns the spawned pid.
    fn spawn_new_session(
        &self,
        workspace_path: String,
        prompt: String,
        model: Option<String>,
        effort: Option<String>,
        permission_mode: Option<String>,
    ) -> Result<u32, String>;
    /// Read the auto-resume scheduler config.
    fn get_auto_resume_config(&self) -> crate::auto_resume::AutoResumeConfig;
    /// Persist the auto-resume scheduler config.
    fn set_auto_resume_config(
        &self,
        config: crate::auto_resume::AutoResumeConfig,
    ) -> Result<(), String>;
    fn account_info(&self) -> AccountInfoFuture;
    /// Fetch account/profile info for the given source (e.g. "claude", "cursor", "openclaw").
    fn source_account(&self, source: &str) -> SourceDataFuture;
    /// Fetch usage info for the given source (e.g. "claude", "cursor", "codex", "openclaw").
    fn source_usage(&self, source: &str) -> SourceDataFuture;
    /// Fetch usage summaries for all detected sources (for tray menu display).
    fn usage_summaries(&self) -> Vec<SourceUsageSummary>;
    fn check_setup(&self) -> SetupStatus;
    /// Start tailing a session file for new lines.
    /// Returns the initial byte offset (file size at call time).
    /// New lines are delivered as `session-tail` Tauri events.
    fn start_watch(&self, path: String) -> Result<u64, String>;
    fn stop_watch(&self);

    // ── Memory ───────────────────────────────────────────────────────────────
    fn list_memories(&self) -> Vec<WorkspaceMemory>;
    fn get_memory_content(&self, path: &str) -> Result<String, String>;
    fn get_memory_history(&self, path: &str) -> Vec<MemoryHistoryEntry>;

    // ── Wiki knowledge base ──────────────────────────────────────────────────
    /// All docs under `~/.fleet/wiki`, newest first.
    fn list_wiki_docs(&self) -> Vec<crate::wiki::WikiDoc>;
    fn get_wiki_doc(&self, slug: &str) -> Result<crate::wiki::WikiDoc, String>;
    /// Raw bytes + mime of one file of one version. `version` `""`/`"current"`
    /// resolves to the doc's `current_version`. Serves the `fleet-wiki://`
    /// protocol, so it must work for both local and remote backends.
    fn get_wiki_file(
        &self,
        slug: &str,
        version: &str,
        relpath: &str,
    ) -> Result<crate::wiki::WikiFileBytes, String>;
    fn delete_wiki_doc(&self, slug: &str) -> Result<(), String>;
    /// Delete a single non-current version.
    fn delete_wiki_version(&self, slug: &str, version: &str) -> Result<(), String>;

    // ── TASKS.md plans ─────────────────────────────────────────────────────────
    /// PRD plans (with full task items) visible to a session whose workspace
    /// root is `workspace_path` — scans the same source set the `fleet
    /// prd-context` hook injects (main checkout + sibling worktrees).
    ///
    /// `session_id` scopes the result: `Some(sid)` returns only the plan that
    /// session is focused on (empty when it has no focus record), while `None`
    /// returns every plan in the workspace.
    fn get_task_plans(
        &self,
        workspace_path: &str,
        session_id: Option<&str>,
    ) -> Vec<crate::prd_tasks::TaskPlanDetail>;

    // ── Skills ────────────────────────────────────────────────────────────────
    fn list_skills(&self) -> Vec<SkillItem>;
    fn get_skill_content(&self, path: &str) -> Result<String, String>;
    fn list_skill_files(&self, skill_path: &str) -> Result<Vec<SkillFileEntry>, String>;
    /// Delete a skill (directory or flat file) under `~/.claude/skills/`.
    fn delete_skill(&self, skill_path: &str) -> Result<(), String>;
    /// Per-session Skill invocation history. Scans the main jsonl and any
    /// sibling `<sid>/subagents/agent-*.jsonl` files; subagent entries are
    /// flagged with `is_subagent: true`.
    fn get_skill_history(&self, jsonl_path: &str) -> Result<Vec<SkillInvocation>, String>;

    /// Claude Code **Workflow** runs for a session. `jsonl_path` is the parent
    /// session's `.jsonl` transcript path; the sibling
    /// `<sid>/subagents/workflows/wf_*/` dirs are discovered and each run's
    /// `journal.jsonl` + script `meta.phases` are parsed into a
    /// [`crate::workflow::WorkflowTree`]. Empty when the session has no runs.
    /// Default impl returns empty so non-Claude sources opt out for free.
    fn get_workflow_trees(
        &self,
        _jsonl_path: &str,
    ) -> Result<Vec<crate::workflow::WorkflowTree>, String> {
        Ok(Vec::new())
    }

    // ── Token spend ──────────────────────────────────────────────────────────
    /// Token-spend attribution for the task tree rooted at `main_jsonl_path`
    /// (main session + recursive subagent jsonls). Returns billed totals from
    /// API usage + estimated source-bucket attribution (chars/4 + disk
    /// snapshot of CLAUDE.md / memory / skills / fleet reminders). See
    /// `token_analysis` module for methodology details.
    fn get_task_token_breakdown(
        &self,
        main_jsonl_path: &str,
        project_root: Option<&str>,
    ) -> Result<crate::token_analysis::TaskTokenBreakdown, String>;

    // ── Plugins ──────────────────────────────────────────────────────────────
    /// Scan `~/.claude/plugins/` for installed Claude Code plugins.
    fn list_plugins(&self) -> Vec<crate::plugins::PluginItem>;
    /// Enable or disable a downloaded plugin via `claude plugin enable|disable`.
    fn set_plugin_enabled(&self, plugin_id: &str, enabled: bool) -> Result<(), String>;
    /// Install (download) a plugin from a marketplace via `claude plugin install`.
    fn install_plugin(&self, plugin_id: &str) -> Result<(), String>;
    /// Uninstall (remove on-disk + prune from enabled set) via `claude plugin uninstall`.
    fn uninstall_plugin(&self, plugin_id: &str) -> Result<(), String>;
    /// List configured marketplaces via `claude plugin marketplace list --json`.
    fn list_marketplaces(&self) -> Vec<crate::claude_cli::CliMarketplace>;
    /// Add a marketplace via `claude plugin marketplace add <source>`.
    /// `source` may be a GitHub `owner/repo`, a git URL, or a local path.
    fn add_marketplace(&self, source: &str) -> Result<(), String>;
    /// Remove a marketplace via `claude plugin marketplace remove <name>`.
    fn remove_marketplace(&self, name: &str) -> Result<(), String>;

    // ── Projects (workspace + kanban + fleet-managed sessions) ───────────────
    /// List Fleet projects from `~/.claude/fleet/projects.json`.
    fn list_projects(&self) -> Vec<crate::project::Project>;
    /// Create a new project; returns the persisted record.
    fn create_project(&self, input: crate::project::ProjectInput) -> Result<crate::project::Project, String>;
    /// Persist updated project metadata (id must match an existing project).
    fn update_project(&self, project: crate::project::Project) -> Result<(), String>;
    /// Delete a project by id (metadata only — the workspace directory and its
    /// `.fleetsession` files are untouched per Boss's "delete metadata only"
    /// decision).
    fn delete_project(&self, project_id: &str) -> Result<(), String>;
    /// List all fleet-managed sessions across all projects (read from
    /// `~/.claude/fleet/fleet-sessions.json`).
    fn list_fleet_sessions(&self) -> Vec<crate::project::FleetSession>;
    /// Enqueue a new fleet-managed session. Spawn happens on the next
    /// supervisor tick (macOS only); other platforms return NotSupported.
    fn spawn_fleet_session(
        &self,
        form: crate::project::LauncherForm,
    ) -> Result<crate::project::FleetSession, String>;
    /// SIGTERM a running fleet session and mark it complete.
    fn cancel_fleet_session(&self, session_id: &str) -> Result<(), String>;
    /// Re-queue a completed fleet session with a follow-up prompt.
    /// Spawn becomes `claude --resume <sid> -p <prompt>` on the next tick.
    fn resume_fleet_session(&self, session_id: &str, follow_up_prompt: &str) -> Result<(), String>;
    /// Update a session's status (used by `fleet session status` self-report
    /// from agents, and by kanban drag-and-drop). If `status` doesn't match
    /// any of the project's kanban_columns, it's recorded in `note` and the
    /// column is unchanged.
    fn set_fleet_session_status(
        &self,
        session_id: &str,
        status: &str,
        note: Option<&str>,
    ) -> Result<(), String>;

    // ── Tasks (task-as-unit V1 — see design/task-as-unit-redesign.md) ────────
    //
    // Six methods front the new task system. Default impls return
    // `Err("task system not implemented in this backend yet")` so the trait
    // compiles before LocalBackend (P3) and RemoteBackend (P4) provide real
    // implementations. Once both backends override, the defaults can be
    // removed.
    //
    /// Create a new task in `Drafting` status. Materials get attached after
    /// creation via `add_task_material`.
    fn create_task(&self, _input: crate::task::TaskInput) -> Result<crate::task::Task, String> {
        Err("task system not implemented in this backend yet".into())
    }
    /// Append a file material to the task's inbox. Returns the persisted
    /// absolute path on the storage host (LocalBackend: same machine;
    /// RemoteBackend: path on the probe host). Bytes are bounded by
    /// `MAX_ATTACHMENT_BYTES` (50 MiB); larger payloads must be rejected.
    fn add_task_material(
        &self,
        _task_id: &str,
        _filename: &str,
        _bytes: Vec<u8>,
        _media: crate::task::MediaKind,
    ) -> Result<std::path::PathBuf, String> {
        Err("task system not implemented in this backend yet".into())
    }
    /// Read a single task by id. `Err` if not found.
    fn get_task(&self, _task_id: &str) -> Result<crate::task::Task, String> {
        Err("task system not implemented in this backend yet".into())
    }
    /// List tasks, optionally filtered by project. `None` returns every task.
    fn list_tasks(&self, _project_id: Option<&str>) -> Vec<crate::task::Task> {
        Vec::new()
    }
    /// Deliverables for a task: the per-file diff stat of its branch against
    /// the fork point off the default branch. Empty (not an error) for tasks
    /// that haven't started yet. Powers the detail page's Deliverables section.
    fn get_task_deliverables(
        &self,
        _task_id: &str,
    ) -> Result<crate::deliverables::TaskDeliverables, String> {
        Err("task system not implemented in this backend yet".into())
    }
    /// Replace the task's `plan` (master tool — see PRD §5.7 / P20).
    /// Caller must hold the per-task write mutex (master runtime owns this in
    /// production); the LocalBackend impl will acquire it internally.
    fn update_plan(
        &self,
        _task_id: &str,
        _plan: crate::plan::DagPlan,
    ) -> Result<(), String> {
        Err("task system not implemented in this backend yet".into())
    }
    /// Transition a `Ready` task to `Running`: auto-create `fleet/<slug>` git
    /// branch, spawn the master session, flip status. No-op once the task is
    /// already running. See PRD §5.2 / P3.
    fn start_task(&self, _task_id: &str) -> Result<(), String> {
        Err("task system not implemented in this backend yet".into())
    }
    /// P7 final acceptance: flip a task from `AwaitingAcceptance` to `Done`
    /// once the user confirms the completed work. `mode` selects whether to
    /// merge `fleet/<slug>` back into its base branch + delete it (`MergeBack`)
    /// or leave the branch for the user (`KeepBranch`).
    fn accept_task(
        &self,
        _task_id: &str,
        _mode: claw_fleet_task::task::AcceptMode,
    ) -> Result<(), String> {
        Err("task system not implemented in this backend yet".into())
    }
    /// Re-run a task's task-level e2e gate after a failed run (clears the
    /// recorded outcome so the live orchestrator re-runs `verify.e2e`).
    fn rerun_task_e2e(&self, _task_id: &str) -> Result<(), String> {
        Err("task system not implemented in this backend yet".into())
    }
    /// Delete a task: terminate its linked sessions, then remove the task json
    /// and its materials dir from disk. Used by the UI's task delete action.
    fn clear_task(&self, _task_id: &str) -> Result<(), String> {
        Err("task system not implemented in this backend yet".into())
    }
    /// Manually rename a task. Clears `title_auto` so the master-session
    /// aiTitle reconcile pass will not overwrite the new value. Trims input
    /// and rejects empty titles.
    fn set_task_title(&self, _task_id: &str, _new_title: &str) -> Result<(), String> {
        Err("task system not implemented in this backend yet".into())
    }
    /// Start an SSE stream of task events. Events arrive as Tauri events on
    /// the `task-event` channel (LocalBackend) or HTTP-relayed (RemoteBackend).
    /// Returns the channel id the caller can later pass to
    /// `unsubscribe_task_events`. P4 wires the HTTP side.
    fn subscribe_task_events(&self, _task_id: &str) -> Result<String, String> {
        Err("task system not implemented in this backend yet".into())
    }

    /// Phase 3: snapshot of live `fleet-task` processes from the runtime
    /// registry. LocalBackend serves this from its `RuntimeRegistryWatcher`;
    /// RemoteBackend returns an empty list until Phase 3 wires the SSH probe.
    fn runtime_tasks(&self) -> Vec<crate::registry::RegistryEntry> {
        Vec::new()
    }

    /// Phase 3: query a live fleet-task's `/state` endpoint. Returns the raw
    /// JSON the binary published. Errors when no fleet-task process is
    /// serving this task_id (caller can fall back to `get_task`).
    fn fleet_task_state(&self, _task_id: &str) -> Result<serde_json::Value, String> {
        Err("no fleet-task process serving this task in this backend".into())
    }

    /// Phase 3: trigger a worker dispatch on a live fleet-task's runtime
    /// loop. Returns the same error as `fleet_task_state` when the task has
    /// no associated process.
    fn fleet_task_dispatch(
        &self,
        _task_id: &str,
        _p_item_id: &str,
    ) -> Result<(), String> {
        Err("no fleet-task process serving this task in this backend".into())
    }

    /// Whether the macOS LaunchAgent plist for `fleet serve` is installed.
    /// Always returns `false` on non-macOS.
    fn is_fleet_daemon_installed(&self) -> bool;
    /// Install the LaunchAgent plist + `launchctl bootstrap`. macOS only.
    fn install_fleet_daemon(&self, fleet_path: &str, port: u16, token: &str) -> Result<(), String>;
    /// `launchctl bootout` + remove the plist. macOS only.
    fn uninstall_fleet_daemon(&self) -> Result<(), String>;
    /// Create / refresh `~/.claude/fleet/bin/fleet → <fleet_path>` so that
    /// fleet-managed agents can call `fleet session status` from PATH.
    fn ensure_fleet_cli_link(&self, fleet_path: &str) -> Result<(), String>;

    // ── Waiting alerts ────────────────────────────────────────────────────────
    fn get_waiting_alerts(&self) -> Vec<WaitingAlert>;

    // ── Hooks ─────────────────────────────────────────────────────────────────
    fn get_hooks_plan(&self) -> crate::hooks::HookSetupPlan;
    fn apply_hooks(&self) -> Result<(), String>;
    fn remove_hooks(&self) -> Result<(), String>;

    // ── Guard (real-time interception) ───────────────────────────────────────
    fn apply_guard_hook(&self) -> Result<(), String>;
    fn remove_guard_hook(&self) -> Result<(), String>;
    fn respond_to_guard(
        &self,
        id: &str,
        allow: bool,
        always_allow: Option<crate::guard::GuardAlwaysAllow>,
        reason: Option<String>,
    ) -> Result<(), String>;
    /// List persisted "always allow" rules for the Bash guard short-circuit
    /// path. Accumulated when the user clicks "Always allow" on a guard card.
    fn list_guard_allow_rules(&self) -> Vec<crate::audit::GuardAllowRule> {
        Vec::new()
    }
    /// Remove a guard allow rule by its id. Returns an error if the id is
    /// unknown or the backend doesn't support guard allow-list management.
    fn remove_guard_allow_rule(&self, _id: &str) -> Result<(), String> {
        Err("guard allow-list not supported by this backend".into())
    }
    fn analyze_guard_command(&self, command: &str, context: &str, lang: &str) -> Result<String, String>;

    // ── Elicitation (AskUserQuestion interception) ──────────────────────
    fn apply_elicitation_hook(&self) -> Result<(), String>;
    fn remove_elicitation_hook(&self) -> Result<(), String>;
    fn respond_to_elicitation(
        &self,
        id: &str,
        declined: bool,
        answers: std::collections::HashMap<String, String>,
    ) -> Result<(), String>;

    // ── fleet__ask MCP tool (mirror of elicitation for the MCP bridge) ──
    /// Write a fleet_ask response so the polling MCP server (`fleet mcp`)
    /// can pick it up and return it to the agent. `cancelled = true` signals
    /// the user dismissed the card; `answers` maps form-field name → value
    /// (or question text → option label, mirroring elicitation's contract).
    fn respond_to_fleet_ask(
        &self,
        id: &str,
        cancelled: bool,
        answers: std::collections::BTreeMap<String, String>,
    ) -> Result<(), String>;

    // ── fleet__permission_prompt MCP tool (headless native-permission bridge) ──
    /// Write a permission-prompt response so the polling MCP server
    /// (`fleet mcp`) can convert it into Claude Code's
    /// `--permission-prompt-tool` contract payload. `allow = false` denies;
    /// `reason` (deny path) is forwarded to the agent as the deny message.
    fn respond_to_permission_prompt(
        &self,
        id: &str,
        allow: bool,
        reason: Option<String>,
    ) -> Result<(), String>;

    // ── fleet__render_a2ui MCP tool (parallel channel to fleet__ask) ──
    /// Write a fleet__render_a2ui response. `action_name = None` paired with
    /// `cancelled = true` signals the user dismissed the card without firing
    /// any Action. `action_context` is the resolved BoundValue map from the
    /// A2UI surface, stringified per-value by the frontend so the wire stays
    /// `BTreeMap<String, String>` (same shape as fleet__ask's `answers`).
    fn respond_to_a2ui_render(
        &self,
        id: &str,
        cancelled: bool,
        action_name: Option<String>,
        action_context: std::collections::BTreeMap<String, String>,
    ) -> Result<(), String>;
    /// One-click fix for the `mcp_injection` diagnostic: re-acquire the
    /// `mcpServers.fleet` entry in `~/.claude.json` with the supplied
    /// fleet binary path. Mirrors `apply_elicitation_hook` for the MCP
    /// injection side. Default impl is a no-op error so RemoteBackend
    /// gets a useful message until a probe-side endpoint is wired (P-task
    /// follow-up — local-only for now is fine since the diagnostic surface
    /// only renders in the desktop window).
    fn apply_mcp_injector(&self, _fleet_path: &str) -> Result<(), String> {
        Err("apply_mcp_injector is local-only in this build".into())
    }

    // ── Plan approval (ExitPlanMode interception) ──────────────────────
    fn apply_plan_approval_hook(&self) -> Result<(), String>;
    fn remove_plan_approval_hook(&self) -> Result<(), String>;
    fn list_pending_plan_approvals(&self) -> Vec<crate::plan_approval::PlanApprovalRequest>;
    fn respond_to_plan_approval(
        &self,
        id: &str,
        decision: &str,
        edited_plan: Option<String>,
        feedback: Option<String>,
    ) -> Result<(), String>;

    // ── Pending decisions snapshot (mount catch-up) ─────────────────────
    /// Snapshot every decision-panel request currently awaiting a response so
    /// the frontend can seed its store on mount, covering the cold-restart gap
    /// where the one-shot watcher emit was lost (no Tauri listener attached
    /// yet). Default impl returns empty; both real backends override.
    fn list_pending_decisions(&self) -> PendingDecisions {
        PendingDecisions::default()
    }

    // ── Decision history (per-session log of resolved decision panels) ──
    /// Return all decision records (elicitation + plan-approval + user
    /// prompts extracted from the session jsonl) for a session, oldest-first.
    /// Empty when nothing has been recorded yet. When `jsonl_path` is
    /// provided the backend will sync any new user prompts from the jsonl
    /// into the persisted history before returning.
    fn list_session_decisions(
        &self,
        session_id: &str,
        jsonl_path: Option<&str>,
    ) -> Vec<crate::decision_history::DecisionHistoryRecord>;

    // ── Interaction mode (global CLAUDE.md guidance) ────────────────────────
    fn apply_interaction_mode(&self, user_title: &str, locale: &str) -> Result<(), String>;
    fn remove_interaction_mode(&self) -> Result<(), String>;
    /// QA diagnostics: report on the four backend-observable checkpoints in
    /// the AskUserQuestion → Decision Card pipeline. The frontend appends a
    /// fifth row (Tauri listener self-test) it owns.
    fn interaction_diagnostics(&self) -> Vec<crate::interaction_mode_diagnostics::DiagnosticCheck> {
        crate::interaction_mode_diagnostics::run_checks()
    }

    /// Active diagnostic test (depth 2): write a fake ElicitationRequest
    /// to disk so the watcher picks it up and emits it to the frontend
    /// exactly like a real AskUserQuestion. Cleanup auto-runs after 10s.
    fn test_decision_end_to_end(&self) -> Result<crate::interaction_mode_test::TestRunResult, String> {
        crate::interaction_mode_test::run_end_to_end_test(std::time::Duration::from_secs(10))
    }

    /// Active diagnostic test (depth 3): spawn `claude -p "<prompt>"` so
    /// the Agent actually exercises the AskUserQuestion injection from
    /// CLAUDE.md. Returns when the process exits or after 60s.
    fn test_decision_via_claude_cli(&self) -> Result<crate::interaction_mode_test::TestRunResult, String> {
        crate::interaction_mode_test::run_claude_cli_test(std::time::Duration::from_secs(60))
    }

    /// fleet__ask depth-2 test: write a fake `FleetAskRequest` so the new
    /// MCP-side watcher emits `fleet-ask-request` and the frontend renders
    /// a composite (html + form + options) test card. Cleanup auto-runs.
    fn test_fleet_ask_end_to_end(&self) -> Result<crate::interaction_mode_test::TestRunResult, String> {
        crate::interaction_mode_test::run_fleet_ask_end_to_end_test(std::time::Duration::from_secs(10))
    }

    /// fleet__ask depth-3 test: spawn `claude -p "<prompt>"` with
    /// `--allowed-tools "mcp__fleet__ask"` so the Agent actually invokes
    /// the new MCP tool. Mirrors `test_decision_via_claude_cli`.
    fn test_fleet_ask_via_claude_cli(&self) -> Result<crate::interaction_mode_test::TestRunResult, String> {
        crate::interaction_mode_test::run_fleet_ask_claude_cli_test(std::time::Duration::from_secs(60))
    }

    // ── PRD Discipline mode (commit guard + TASKS.md re-injection hook) ─────
    /// Enable the PRD Discipline mode: install the guidance block in
    /// CLAUDE.md AND register the UserPromptSubmit hook that re-injects
    /// the workspace's TASKS.md on every prompt. Two halves move together
    /// so the UI exposes a single toggle.
    fn apply_prd_mode(&self, user_title: &str, locale: &str) -> Result<(), String>;
    fn remove_prd_mode(&self) -> Result<(), String>;

    // ── Agent sources config ─────────────────────────────────────────────────
    fn get_sources_config(&self) -> Vec<crate::agent_source::SourceInfo>;
    fn set_source_enabled(&self, name: &str, enabled: bool) -> Result<(), String>;

    // ── Claude binary discovery & override ───────────────────────────────────
    /// Enumerate every Claude CLI binary fleet can find on the host, ranked by
    /// priority. The first entry is what fleet would auto-pick if no override
    /// is set. Multiple entries cover the case where the user has the official
    /// installer **and** the VS Code / Cursor extension bundle installed
    /// simultaneously — the Settings panel uses this list to let the user
    /// pick which binary fleet should drive.
    fn list_claude_binaries(&self) -> Vec<crate::claude_binary::ClaudeBinary>;
    /// The currently-persisted user override path, or `None` for "auto".
    fn get_claude_binary_override(&self) -> Option<String>;
    /// Persist the user's choice. `None` (or empty string) clears the override.
    fn set_claude_binary_override(&self, path: Option<String>) -> Result<(), String>;

    // ── Full-text search ─────────────────────────────────────────────────────
    fn search_sessions(&self, query: &str, limit: usize) -> Vec<SearchHit>;

    // ── Security audit ──────────────────────────────────────────────────────
    fn get_audit_events(&self) -> AuditSummary;
    fn get_audit_rules(&self) -> Vec<AuditRuleInfo>;
    fn set_audit_rule_enabled(&self, id: &str, enabled: bool) -> Result<(), String>;
    fn save_custom_audit_rule(&self, rule: AuditRuleInfo) -> Result<(), String>;
    fn delete_custom_audit_rule(&self, id: &str) -> Result<(), String>;
    fn suggest_audit_rules(&self, concern: &str, lang: &str) -> Result<Vec<SuggestedRule>, String>;

    // ── Daily reports ────────────────────────────────────────────────────────
    fn get_daily_report(&self, date: &str) -> Result<Option<DailyReport>, String>;
    fn list_daily_report_stats(&self, from: &str, to: &str) -> Vec<DailyReportStats>;
    fn generate_daily_report(&self, date: &str) -> Result<DailyReport, String>;
    fn generate_daily_report_ai_summary(&self, date: &str) -> Result<String, String>;
    fn generate_daily_report_lessons(&self, date: &str) -> Result<Vec<Lesson>, String>;
    fn append_lesson_to_claude_md(&self, lesson: &Lesson) -> Result<(), String>;

    // ── LLM provider ────────────────────────────────────────────────────────
    fn list_llm_providers(&self) -> Vec<LlmProviderInfo>;
    fn get_llm_config(&self) -> LlmConfig;
    fn set_llm_config(&self, config: LlmConfig) -> Result<(), String>;

    // ── Fleet-self LLM usage accounting ─────────────────────────────────────
    /// Return daily usage buckets (one per date × scenario) in [from_ms, to_ms].
    fn list_fleet_llm_usage_daily(&self, from_ms: u64, to_ms: u64)
        -> Vec<FleetLlmUsageDailyBucket>;

    // ── Usage occupancy history ─────────────────────────────────────────────
    /// Return persisted 5h/7d/7d-sonnet occupancy snapshots in [from_ms, to_ms]
    /// (epoch ms) for the 24h occupancy-trend chart. Sorted ascending by ts.
    fn usage_history(&self, from_ms: i64, to_ms: i64) -> Vec<UsageHistoryPoint>;

    // ── Decision-panel attachments ──────────────────────────────────────────
    /// Make a local file available to the agent process as an absolute path.
    ///
    /// LocalBackend returns the source path unchanged (agent runs on the same
    /// machine). RemoteBackend uploads the file bytes to the probe host and
    /// returns the server-side temp path. UI layers concatenate the returned
    /// path into the textarea as `@<path>` so Claude Code picks it up.
    fn upload_attachment(&self, source_path: &std::path::Path) -> Result<String, String>;

    // ── Feishu (Lark) Decision Panel mirror ─────────────────────────────────
    // Skeleton: default impls report "not configured" until the operator sets
    // the FEISHU_APP_ID / FEISHU_APP_SECRET / FEISHU_ENCRYPT_KEY env vars and
    // wires up the Feishu app per design/feishu-integration.md.
    fn start_feishu_oauth(&self) -> Result<crate::feishu::OauthHandle, String> {
        Err("feishu integration not configured".into())
    }
    fn poll_feishu_oauth(&self, _state: &str) -> Result<crate::feishu::OauthStatus, String> {
        Err("feishu integration not configured".into())
    }
    fn feishu_status(&self) -> Result<crate::feishu::FeishuConnection, String> {
        Ok(crate::feishu::FeishuConnection::NotConnected)
    }
    fn disconnect_feishu(&self) -> Result<(), String> {
        Err("feishu integration not configured".into())
    }
    fn get_feishu_creds(&self) -> Result<crate::feishu::StoredCreds, String> {
        Ok(crate::feishu::StoredCreds::default())
    }
    fn set_feishu_creds(&self, _creds: crate::feishu::StoredCreds) -> Result<(), String> {
        Err("feishu integration not configured".into())
    }
}

/// Upper bound on a single attachment payload. Enforced by both the uploader
/// (to fail fast) and `fleet serve` (to reject oversized POSTs). Kept small
/// enough that a full upload can reasonably live in memory.
pub const MAX_ATTACHMENT_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use crate::account::{AccountInfo, UsageStats};

    // ── SourceUsageSummary::from_claude tests ───────────────────────────────

    // ── resolve_pending_display ─────────────────────────────────────────────

    fn mk_session(id: &str, workspace_name: &str, ai_title: Option<&str>) -> SessionInfo {
        use crate::session::SessionStatus;
        SessionInfo {
            id: id.into(),
            workspace_path: "/tmp/test".into(),
            workspace_name: workspace_name.into(),
            ide_name: None,
            entrypoint: None,
            is_subagent: false,
            parent_session_id: None,
            agent_type: None,
            agent_description: None,
            slug: None,
            ai_title: ai_title.map(|s| s.to_string()),
            status: SessionStatus::Idle,
            token_speed: 0.0,
            agent_token_speed: 0.0,
            total_output_tokens: 0,
            total_cost_usd: 0.0,
            agent_total_cost_usd: 0.0,
            cost_speed_usd_per_min: 0.0,
            last_message_preview: None,
            last_activity_ms: 0,
            created_at_ms: 0,
            jsonl_path: format!("/tmp/{id}.jsonl"),
            model: None,
            thinking_level: None,
            pid: None,
            pid_precise: false,
            last_skill: None,
            context_percent: None,
            agent_source: "claude-code".into(),
            last_outcome: None,
            rate_limit: None,
            todos: None,
            task_plan: None,            compact_count: 0,
            compact_pre_tokens: 0,
            compact_post_tokens: 0,
            compact_cost_usd: 0.0,
        }
    }

    #[test]
    fn resolve_pending_display_fills_empty_workspace_and_title() {
        use crate::elicitation::{ElicitationQuestion, ElicitationRequest};
        let mut pending = PendingDecisions::default();
        pending.elicitation.push(ElicitationRequest {
            id: "e1".into(),
            session_id: "s1".into(),
            workspace_name: String::new(),
            ai_title: None,
            questions: vec![ElicitationQuestion {
                question: "q?".into(),
                header: "h".into(),
                options: vec![],
                multi_select: false,
            }],
            timestamp: "t".into(),
        });
        let sessions = vec![mk_session("s1", "my-workspace", Some("Fix the bug"))];
        resolve_pending_display(&mut pending, &sessions);
        assert_eq!(pending.elicitation[0].workspace_name, "my-workspace");
        assert_eq!(pending.elicitation[0].ai_title.as_deref(), Some("Fix the bug"));
    }

    #[test]
    fn resolve_pending_display_preserves_existing_and_handles_unknown_session() {
        use crate::elicitation::ElicitationRequest;
        let mut pending = PendingDecisions::default();
        // Already-populated workspace must be preserved.
        pending.elicitation.push(ElicitationRequest {
            id: "e1".into(),
            session_id: "s1".into(),
            workspace_name: "preset-ws".into(),
            ai_title: Some("preset-title".into()),
            questions: vec![],
            timestamp: "t".into(),
        });
        // Unknown session → left as-is (empty), no panic.
        pending.elicitation.push(ElicitationRequest {
            id: "e2".into(),
            session_id: "missing".into(),
            workspace_name: String::new(),
            ai_title: None,
            questions: vec![],
            timestamp: "t".into(),
        });
        let sessions = vec![mk_session("s1", "lookup-ws", Some("lookup-title"))];
        resolve_pending_display(&mut pending, &sessions);
        assert_eq!(pending.elicitation[0].workspace_name, "preset-ws");
        assert_eq!(pending.elicitation[0].ai_title.as_deref(), Some("preset-title"));
        assert_eq!(pending.elicitation[1].workspace_name, "");
        assert_eq!(pending.elicitation[1].ai_title, None);
    }

    #[test]
    fn from_claude_all_windows() {
        let info = AccountInfo {
            plan: "Max 5x".into(),
            five_hour: Some(UsageStats { utilization: 0.3, resets_at: "2026-01-01T00:00:00Z".into(), prev_utilization: None }),
            seven_day: Some(UsageStats { utilization: 0.7, resets_at: "2026-01-07T00:00:00Z".into(), prev_utilization: None }),
            seven_day_sonnet: Some(UsageStats { utilization: 0.1, resets_at: "2026-01-07T00:00:00Z".into(), prev_utilization: None }),
            ..Default::default()
        };
        let s = SourceUsageSummary::from_claude(&info);
        assert_eq!(s.source, "claude");
        assert_eq!(s.plan, Some("Max 5x".into()));
        assert_eq!(s.bars.len(), 3);
        assert_eq!(s.bars[0].label, "5h");
        assert!((s.bars[0].utilization - 0.3).abs() < f64::EPSILON);
        assert_eq!(s.bars[1].label, "7d Opus");
        assert_eq!(s.bars[2].label, "7d Sonnet");
    }

    #[test]
    fn from_claude_partial_windows() {
        let info = AccountInfo {
            plan: "".into(),
            five_hour: Some(UsageStats { utilization: 0.5, resets_at: "t".into(), prev_utilization: None }),
            ..Default::default()
        };
        let s = SourceUsageSummary::from_claude(&info);
        assert_eq!(s.plan, None); // empty plan → None
        assert_eq!(s.bars.len(), 1);
    }

    #[test]
    fn from_claude_no_windows() {
        let info = AccountInfo::default();
        let s = SourceUsageSummary::from_claude(&info);
        assert!(s.bars.is_empty());
    }

    // ── SourceUsageSummary::from_cursor tests ───────────────────────────────

    #[test]
    fn from_cursor_with_usage_items() {
        let val = json!({
            "membershipType": "pro",
            "usage": [
                {"name": "Premium", "utilization": 0.6, "resetsAt": "2026-01-01T00:00:00Z"},
                {"name": "Standard", "used": 50, "limit": 200}
            ]
        });
        let s = SourceUsageSummary::from_cursor(&val);
        assert_eq!(s.source, "cursor");
        assert_eq!(s.plan, Some("pro".into()));
        assert_eq!(s.bars.len(), 2);
        assert!((s.bars[0].utilization - 0.6).abs() < f64::EPSILON);
        // Computed from used/limit: 50/200 = 0.25
        assert!((s.bars[1].utilization - 0.25).abs() < f64::EPSILON);
        assert!(s.bars[1].resets_at.is_none());
    }

    #[test]
    fn from_cursor_empty_usage() {
        let val = json!({"membershipType": "", "usage": []});
        let s = SourceUsageSummary::from_cursor(&val);
        assert_eq!(s.plan, None); // empty → None
        assert!(s.bars.is_empty());
    }

    // ── SourceUsageSummary::from_codex tests ────────────────────────────────

    #[test]
    fn from_codex_with_primary_and_secondary() {
        let val = json!({
            "planType": "plus",
            "primary": {"usedPercent": 45, "resetsAt": 1735689600},
            "secondary": {"usedPercent": 10, "resetsAt": 1735776000}
        });
        let s = SourceUsageSummary::from_codex(&val);
        assert_eq!(s.source, "codex");
        assert_eq!(s.plan, Some("plus".into()));
        assert_eq!(s.bars.len(), 2);
        assert!((s.bars[0].utilization - 0.45).abs() < f64::EPSILON);
        assert!((s.bars[1].utilization - 0.10).abs() < f64::EPSILON);
        assert!(s.bars[0].resets_at.is_some());
    }

    #[test]
    fn from_codex_missing_plan() {
        let val = json!({});
        let s = SourceUsageSummary::from_codex(&val);
        assert_eq!(s.plan, None);
        assert!(s.bars.is_empty());
    }

    // ── SourceUsageSummary::from_openclaw tests ─────────────────────────────

    #[test]
    fn from_openclaw_picks_highest_utilization() {
        let val = json!({
            "sessions": [
                {"percentUsed": 30.0, "model": "gpt-4"},
                {"percentUsed": 80.0, "model": "claude-opus"},
                {"percentUsed": 50.0, "model": "gpt-4"}
            ]
        });
        let s = SourceUsageSummary::from_openclaw(&val);
        assert_eq!(s.source, "openclaw");
        assert_eq!(s.bars.len(), 1);
        assert!((s.bars[0].utilization - 0.8).abs() < f64::EPSILON);
        assert!(s.bars[0].label.contains("claude-opus"));
    }

    #[test]
    fn from_openclaw_empty_sessions() {
        let val = json!({"sessions": []});
        let s = SourceUsageSummary::from_openclaw(&val);
        assert!(s.bars.is_empty());
    }

}
