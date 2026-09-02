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
use crate::today_usage::TodayUsage;
use crate::skills::{SkillFileEntry, SkillItem};

// ── Shared types ─────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DetectedTools {
    pub cli: bool,
    pub vscode: bool,
    pub jetbrains: bool,
    pub desktop: bool,
    pub codex: bool,
}

impl Default for DetectedTools {
    fn default() -> Self {
        DetectedTools {
            cli: false,
            vscode: false,
            jetbrains: false,
            desktop: false,
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
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct WaitingAlert {
    pub session_id: String,
    pub workspace_name: String,
    pub summary: String,
    pub detected_at_ms: u64,
    pub jsonl_path: String,
    /// Originating agent source id (e.g. "claude-code", "codex").
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
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
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
            // Prefer the human/agent title override — for Codex sessions it's the
            // only real title (ai_title is the raw first prompt).
            .map(|s| {
                (
                    s.workspace_name.clone(),
                    s.title_override.clone().or_else(|| s.ai_title.clone()),
                )
            })
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
    /// Source identifier: "claude", "codex".
    pub source: String,
    /// Plan / tier label (e.g. "Max 5x", "pro", "Plus").
    pub plan: Option<String>,
    /// Rate-limit windows, each with a utilization bar.
    pub bars: Vec<UsageBar>,
    /// Where the numbers came from — `"foxy-switcher"` when read from the local
    /// foxy daemon, else the provider's own path (`"anthropic"` /
    /// `"codex-app-server"`). `None` when the source reported nothing.
    /// `#[serde(default)]` keeps older serialized payloads deserializable.
    #[serde(default)]
    pub usage_source: Option<String>,
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
        for sc in &info.seven_day_scoped {
            bars.push(UsageBar {
                label: format!("7d {}", sc.model_label),
                utilization: sc.utilization,
                resets_at: Some(sc.resets_at.clone()),
            });
        }
        SourceUsageSummary {
            source: "claude".into(),
            plan: if info.plan.is_empty() { None } else { Some(info.plan.clone()) },
            bars,
            usage_source: if info.usage_source.is_empty() {
                None
            } else {
                Some(info.usage_source.clone())
            },
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
            usage_source: val["usageSource"].as_str().map(|s| s.to_string()),
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
    /// Full, untrimmed tool output for one `tool_use_id` in `path`. The tail
    /// payload from `get_messages_tail` truncates oversized tool output for
    /// transport (see [`crate::message_trim`]); the frontend calls this when the
    /// reader expands a card flagged `_fleetTruncated`. Default impl reads the
    /// transcript directly; RemoteBackend overrides to go over HTTP.
    fn get_tool_result_full(&self, path: &str, tool_use_id: &str) -> Result<Value, String> {
        crate::message_trim::extract_full_tool_result(std::path::Path::new(path), tool_use_id)
    }
    /// Gracefully interrupt the agent at `pid`: tears down the in-flight tool
    /// call but leaves the transcript resumable. See
    /// [`crate::session::interrupt_pid_impl`] for the signal semantics.
    fn interrupt_pid(&self, pid: u32) -> Result<(), String>;
    /// Interrupt the session at `path` through its own agent source, for the
    /// sources with no per-session process to signal.
    ///
    /// dsh is the case that needs it: every dsh session's turn runs inside one
    /// shared `dsh web`, so the pid on its `SessionInfo` is that server's and
    /// [`Self::interrupt_pid`] would stop every dsh session at once. See
    /// [`crate::agent_source::interrupt_session_at`].
    fn interrupt_agent_session(&self, path: String) -> Result<(), String>;
    fn kill_pid(&self, pid: u32) -> Result<(), String>;
    fn kill_workspace(&self, workspace_path: String) -> Result<(), String>;
    /// Headlessly resume a session, routing by `agent_source`: a claude-code
    /// session spawns `claude --resume <session_id> -p <prompt> [--model <m>]
    /// [--effort <e>] [--permission-mode <pm>]`, a codex session spawns
    /// `codex exec resume <session_id> --json -- <prompt>`, both detached in the
    /// given workspace directory. `prompt` of `None`/empty means "continue"
    /// (the rate-limit auto-resume shape); the history panel passes the user's
    /// follow-up text plus optional per-turn overrides. `agent_source` is the
    /// session's `SessionInfo::agent_source` ("claude-code" / "codex"); blank
    /// defaults to claude for older callers.
    fn resume_session(
        &self,
        session_id: String,
        workspace_path: String,
        prompt: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        permission_mode: Option<String>,
        agent_source: String,
    ) -> Result<(), String>;
    /// Queue a follow-up message for a session that is still mid-turn. Fleet
    /// sessions are one-shot `-p` processes with no live stdin, so a message
    /// typed while the turn runs can't be delivered now; it is stored and the
    /// session-refresh tick fires it via `claude --resume` the moment the turn
    /// ends (see [`claw_fleet_core::pending_message`]). Rejects sessions Fleet
    /// didn't launch (they can't be headlessly resumed).
    fn enqueue_message(
        &self,
        session_id: String,
        workspace_path: String,
        text: String,
    ) -> Result<(), String>;
    /// Cancel a single queued follow-up by chip position (`index` 0-based, in the
    /// order [`enqueue_message`] queued them). Used by the "×" on a queued chip.
    /// Out-of-range/missing-queue is a no-op success — the queue can shrink under
    /// a concurrent drain between the user seeing the chip and the click landing.
    fn cancel_pending_message(&self, session_id: String, index: usize) -> Result<(), String>;
    /// Start a brand-new headless Claude Code session: spawns
    /// `claude -p "<prompt>" --session-id <uuid> [--model <m>] [--effort <e>]
    /// [--permission-mode <pm>]` detached in `workspace_path`. The session is
    /// discovered by the scanner via its own JSONL. Returns the spawned pid
    /// plus the pre-assigned session id (None only from an older remote probe).
    fn spawn_new_session(
        &self,
        workspace_path: String,
        prompt: String,
        model: Option<String>,
        effort: Option<String>,
        permission_mode: Option<String>,
        // Which agent tool to launch: "claude" (or None) / "codex".
        tool: Option<String>,
    ) -> Result<crate::session_launch::SpawnSessionResponse, String>;
    /// Absolute path of the pure-chat workspace, creating it if absent. The
    /// launcher pins this as a fixed entry because, unlike a project, it has no
    /// prior sessions to be discovered from — and the path must come from the
    /// backend, since under a remote connection it lives in the *probe host's*
    /// home directory, not the desktop's.
    fn chat_workspace(&self) -> Result<String, String>;
    /// List the directories one level under `path` (`None` = the backend host's
    /// home), so the launcher can pick a workspace that has never had a session.
    ///
    /// Must go through the backend for the same reason `chat_workspace` does:
    /// under a remote connection the directory the user is choosing lives on the
    /// *probe host*, not on this desktop's disk. That is why the launcher's
    /// native OS dialog cannot serve this case — it can only ever browse the
    /// machine the desktop runs on.
    ///
    /// Unlike the explorer methods above, this is deliberately NOT restricted to
    /// known session workspaces (see `workspace_browse` for its own boundary).
    fn browse_dir(
        &self,
        path: Option<String>,
    ) -> Result<crate::workspace_browse::BrowseDirResponse, String>;
    /// Create one directory under `path` (`None` = the backend host's home) and
    /// answer with the *new* directory's listing, so the picker lands inside it.
    ///
    /// Browsing alone is not enough to choose a workspace on a host whose tree
    /// is empty — and it always is on a freshly provisioned one. Same backend
    /// reasoning as `browse_dir`: the directory is created where the session
    /// will spawn, which under a remote connection is the probe host.
    fn create_dir(
        &self,
        path: Option<String>,
        name: String,
    ) -> Result<crate::workspace_browse::BrowseDirResponse, String>;
    /// List directories on an rca executor host, one ssh hop past the backend.
    ///
    /// `browse_dir` above lists the backend host's own disk; this lists a host
    /// that host can ssh into. Both answer with the same shape so one picker
    /// component renders either. Must go through the backend because the ssh
    /// must originate wherever the session will spawn — that machine holds the
    /// keys and the `~/.ssh/config` aliases, and it is the one whose local
    /// mirror directory has to match.
    fn remote_browse_dir(
        &self,
        ssh_target: String,
        path: Option<String>,
    ) -> Result<crate::workspace_browse::BrowseDirResponse, String>;
    /// Probe one rca executor host: reachable, rca installed, `serve --stdio`
    /// supported. Same backend reasoning as [`Backend::remote_browse_dir`].
    /// Infallible by design — an unreachable host is a renderable status, not
    /// an error the caller has to branch on.
    fn remote_host_health(&self, ssh_target: String) -> crate::remote_host::HostHealth;
    /// The remote-workspace registry (workspaces executed through rca; see
    /// [`crate::remote_workspace`]). Lives on the backend host — that is where
    /// sessions spawn, so that is whose `~/.fleet/remote-workspaces.json`
    /// governs the wrap.
    /// The ssh host book — machines Fleet can reach, each carrying its
    /// capabilities (Fleet backend / rca executor).
    ///
    /// Through the backend for the same reason the workspace registry is:
    /// `remote_workspace::transport` resolves a workspace's `hostId` through
    /// this book at spawn time, and sessions spawn on the backend host. A
    /// settings page reading the desktop's own book under a remote connection
    /// would be editing a file no session ever consults.
    fn list_ssh_hosts(&self) -> Vec<crate::remote_host::SshHost>;
    /// Insert or replace one host by id; returns the updated book.
    fn upsert_ssh_host(
        &self,
        host: crate::remote_host::SshHost,
    ) -> Result<Vec<crate::remote_host::SshHost>, String>;
    /// Remove one host by id; returns the updated book.
    fn remove_ssh_host(&self, id: String) -> Result<Vec<crate::remote_host::SshHost>, String>;
    fn list_remote_workspaces(&self) -> crate::remote_workspace::RemoteWorkspacesConfig;
    /// Register or update a remote workspace; returns the updated registry.
    fn upsert_remote_workspace(
        &self,
        entry: crate::remote_workspace::RemoteWorkspace,
    ) -> Result<crate::remote_workspace::RemoteWorkspacesConfig, String>;
    /// Remove a remote workspace by path; returns the updated registry.
    fn remove_remote_workspace(
        &self,
        path: String,
    ) -> Result<crate::remote_workspace::RemoteWorkspacesConfig, String>;
    /// Read the auto-resume scheduler config.
    fn get_auto_resume_config(&self) -> crate::auto_resume::AutoResumeConfig;
    /// Persist the auto-resume scheduler config.
    fn set_auto_resume_config(
        &self,
        config: crate::auto_resume::AutoResumeConfig,
    ) -> Result<(), String>;
    /// Persist the human's manual review mark for a session. `Some(mark)`
    /// sets/overwrites it; `None` clears it (back to "new / needs review").
    fn set_session_mark(
        &self,
        session_id: String,
        workspace_path: String,
        mark: Option<crate::session_mark::SessionMark>,
    ) -> Result<(), String>;
    /// Persist the human's manual title override for a session. A non-empty
    /// `title` sets/overwrites it; `None` or an empty/whitespace title clears it
    /// (back to the auto-derived title).
    fn set_session_title(
        &self,
        session_id: String,
        workspace_path: String,
        title: Option<String>,
    ) -> Result<(), String>;
    /// Mark a batch of sessions read as of now. "Unread" is derived
    /// (`last_activity_ms > last_read_ms`), so this only stamps the read time;
    /// a single mark is a batch of one, "mark all read" a batch of many.
    fn mark_sessions_read(
        &self,
        items: Vec<crate::session_read::SessionReadItem>,
    ) -> Result<(), String>;

    // ── Workspace command runner (文件 page) ────────────────────────────────
    // Commands launched at a workspace's cwd, each hosted by a detached
    // `fleet-proc-host` process so it survives the app/probe restarting. See
    // `crate::proc_runner`. Defaults are the NullBackend behaviour.
    /// All known workspace procs (running and exited), newest first.
    fn list_procs(&self) -> Vec<crate::proc_runner::ProcRecord> {
        Vec::new()
    }
    /// Launch `command` at `workspace_path` under a detached pty host.
    fn spawn_proc(
        &self,
        _workspace_path: String,
        _command: String,
        _cols: u16,
        _rows: u16,
    ) -> Result<crate::proc_runner::ProcRecord, String> {
        Err("backend not ready".into())
    }
    /// Kill the proc's whole process group (SIGTERM → SIGKILL escalation).
    fn kill_proc(&self, _id: String, _force: bool) -> Result<(), String> {
        Err("backend not ready".into())
    }
    /// Incremental pty output read; `offset = None` tails the recent output.
    fn proc_output(
        &self,
        _id: String,
        _offset: Option<u64>,
    ) -> Result<crate::proc_runner::ProcOutputChunk, String> {
        Err("backend not ready".into())
    }
    /// Forward raw stdin bytes (base64) to the proc's pty.
    fn proc_input(&self, _id: String, _data_b64: String) -> Result<(), String> {
        Err("backend not ready".into())
    }
    /// Resize the proc's pty.
    fn proc_resize(&self, _id: String, _cols: u16, _rows: u16) -> Result<(), String> {
        Err("backend not ready".into())
    }
    /// Clear one exited proc (`id = Some`) or every exited proc, optionally
    /// scoped to a workspace. Returns how many were cleared.
    fn clear_procs(
        &self,
        _id: Option<String>,
        _workspace_path: Option<String>,
    ) -> Result<u32, String> {
        Err("backend not ready".into())
    }

    fn account_info(&self) -> AccountInfoFuture;
    /// Fetch account/profile info for the given source (e.g. "claude", "codex").
    fn source_account(&self, source: &str) -> SourceDataFuture;
    /// Fetch usage info for the given source (e.g. "claude", "codex").
    fn source_usage(&self, source: &str) -> SourceDataFuture;
    /// Fetch usage summaries for all detected sources (for tray menu display).
    fn usage_summaries(&self) -> Vec<SourceUsageSummary>;
    /// Today's cumulative token/cost counter (desktop nav-bar / mobile header).
    /// Sums every agent turn timestamped today, whenever its session started —
    /// agent spend only, Fleet's own LLM overhead excluded. See
    /// [`crate::today_usage`].
    fn today_usage(&self) -> TodayUsage;
    /// Per-model "receipt" breakdown behind the [`Self::today_usage`] counter:
    /// one line per (source, model) with token counts, official unit prices and
    /// line cost; the total reconciles with `today_usage` to the cent.
    fn today_usage_breakdown(&self) -> crate::today_usage::TodayUsageBreakdown;
    /// Per-model receipt + per-day trend over an arbitrary inclusive
    /// `[from_ms, to_ms]` window (epoch ms). The longer-range generalisation of
    /// [`Self::today_usage_breakdown`] for the Settings usage view: both Claude
    /// and Codex turns bucket by their own timestamp (Codex's recovered by
    /// differencing its cumulative snapshots). See
    /// [`crate::today_usage::usage_range_breakdown`].
    fn usage_range_breakdown(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> crate::today_usage::UsageRangeBreakdown;
    fn check_setup(&self) -> SetupStatus;
    /// Per-harness environment probe (claude-code / codex / dsh): installed,
    /// path, version, install channel, login state — the environment wizard's
    /// data source. Default empty keeps placeholder backends (NullBackend)
    /// buildable; Local and Remote both override (drift-guard check A).
    fn harness_statuses(&self) -> Vec<crate::harness_status::HarnessStatus> {
        Vec::new()
    }
    /// Start tailing a session file for new lines.
    /// Returns the initial byte offset (file size at call time).
    /// New lines are delivered as `session-tail` Tauri events.
    fn start_watch(&self, path: String) -> Result<u64, String>;
    fn stop_watch(&self);

    // ── Memory ───────────────────────────────────────────────────────────────
    fn list_memories(&self) -> Vec<WorkspaceMemory>;
    fn get_memory_content(&self, path: &str) -> Result<String, String>;
    fn get_memory_history(&self, path: &str) -> Vec<MemoryHistoryEntry>;

    // ── Scheduled + looped future tasks ────────────────────────────────────────
    /// Registered agent loops (`fleet loop`) — recurring future tasks. Default
    /// reads this machine's `~/.fleet/loops`; RemoteBackend overrides to fetch
    /// the remote host's over HTTP.
    fn list_loops(&self) -> Vec<crate::agent_loop::LoopRecord> {
        crate::agent_loop::list()
    }
    /// Registered one-shot schedules (`fleet schedule`), pending + fired history.
    /// Default reads local; RemoteBackend overrides.
    fn list_schedules(&self) -> Vec<crate::schedule::ScheduleRecord> {
        crate::schedule::list()
    }
    /// Cancel a loop by id. Default acts on this machine; RemoteBackend overrides.
    fn cancel_loop(&self, id: String) -> Result<(), String> {
        if crate::agent_loop::stop(&id) {
            Ok(())
        } else {
            Err(format!("no loop with id {id}"))
        }
    }
    /// Cancel (or forget-history-of) a schedule by id. Default acts on this
    /// machine; RemoteBackend overrides.
    fn cancel_schedule(&self, id: String) -> Result<(), String> {
        if crate::schedule::cancel(&id) {
            Ok(())
        } else {
            Err(format!("no schedule with id {id}"))
        }
    }
    /// Edit a still-`Pending` schedule (prompt / fire time / model / effort /
    /// agent source) and re-arm its timer so the new time+generation takes
    /// effect. Default acts on this machine — it must both persist *and* re-arm,
    /// because the detached timer lives here. RemoteBackend overrides to POST the
    /// update to the host, whose route re-arms on that machine instead.
    fn update_schedule(
        &self,
        update: crate::schedule::ScheduleUpdate,
    ) -> Result<crate::schedule::ScheduleRecord, String> {
        let rec = crate::schedule::update(&update)?;
        let _ = crate::schedule::arm_timer(&rec);
        Ok(rec)
    }

    // ── Live thinking ────────────────────────────────────────────────────────
    /// Token-level reasoning for a Fleet-spawned session that is streaming
    /// right now, reconstructed from its live-thinking sidecar. `None` when the
    /// session isn't Fleet-spawned or has no live sidecar. See
    /// [`crate::live_thinking`].
    fn read_live_thinking(&self, session_id: &str) -> Option<crate::live_thinking::LiveThinking>;

    // ── Handoff relay chains ─────────────────────────────────────────────────
    /// The relay chain a session sits on, if any (`fleet handoff`). Default
    /// errs so placeholder backends need no override; real backends implement.
    fn get_handoff_chain(
        &self,
        _session_id: &str,
    ) -> Result<Option<crate::handoff::HandoffChain>, String> {
        Err("backend not ready".into())
    }

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
    /// Re-key a doc, which is how it moves between virtual directories. Version
    /// history rides along; errors when `to` is already taken.
    fn move_wiki_doc(&self, from: &str, to: &str) -> Result<crate::wiki::WikiDoc, String>;
    /// Re-key every doc under folder `from` to sit under `to`; `to` may be `""`
    /// (the tree root), which dissolves the folder. All-or-nothing.
    fn move_wiki_folder(&self, from: &str, to: &str) -> Result<Vec<crate::wiki::WikiDoc>, String>;
    /// Delete every doc under folder `prefix`. Returns how many were removed.
    fn delete_wiki_folder(&self, prefix: &str) -> Result<usize, String>;
    /// Full-text search over doc metadata and current-version entry files.
    fn search_wiki_docs(&self, query: &str) -> Vec<crate::wiki::WikiSearchHit>;
    /// Exportable artifact of one version: the entry file for single-file
    /// docs, a zip of the version dir for "htmlDir".
    fn export_wiki_doc(&self, slug: &str, version: &str)
        -> Result<crate::wiki::WikiExport, String>;
    /// Publish markdown that has no file behind it — how the desktop reader
    /// sends one message into the knowledge base. `title` empty derives it from
    /// the body; `mode` `Append` folds the text onto the doc's current version
    /// (a running note) instead of superseding it.
    fn publish_wiki_text(
        &self,
        slug: &str,
        title: &str,
        text: &str,
        workspace_path: &str,
        mode: crate::wiki::TextPublishMode,
    ) -> Result<crate::wiki::WikiDoc, String>;

    /// True when this backend talks to another host over the probe API.
    ///
    /// Almost nothing should branch on this — the whole point of the trait is
    /// that callers don't. It exists for the narrow class of actions that are
    /// meaningless off-host: handing a path to "reveal in Finder" or "open with
    /// the system app" when the file lives on the probe's machine would either
    /// fail or, because artifact ids are timestamps and two machines can mint
    /// the same one, open an unrelated local file. Those actions ask here and
    /// offer 导出 instead.
    fn is_remote(&self) -> bool {
        false
    }

    // ── Artifact store (产出) ────────────────────────────────────────────────
    /// Every artifact under `~/.fleet/artifacts`, newest first.
    fn list_artifacts(&self) -> Vec<crate::artifacts::Artifact>;
    fn get_artifact(&self, id: &str) -> Result<crate::artifacts::Artifact, String>;
    /// Ingest `source_path` into the store. `workspace_path` is the directory
    /// the producing session ran in; empty `title`/`note` mean "no opinion".
    fn add_artifact(
        &self,
        source_path: &str,
        title: &str,
        note: &str,
        workspace_path: &str,
        session_id: Option<&str>,
    ) -> Result<crate::artifacts::Artifact, String>;
    /// Patch the user-editable fields; `None` leaves one alone.
    fn update_artifact(
        &self,
        id: &str,
        title: Option<&str>,
        note: Option<&str>,
        starred: Option<bool>,
    ) -> Result<crate::artifacts::Artifact, String>;
    fn delete_artifact(&self, id: &str) -> Result<(), String>;
    /// Blob bytes, whole or by inclusive byte range — the one Backend method
    /// that takes a range, because it backs `fleet-artifact://` and a `<video>`
    /// element seeks by asking for ranges. Every other bytes-returning method
    /// here ([`Backend::get_wiki_file`], [`Backend::get_decision_asset`])
    /// answers whole files, which is fine for a page but not for a 400 MB
    /// render. `None` means the whole blob.
    fn read_artifact_bytes(
        &self,
        id: &str,
        range: Option<(u64, u64)>,
    ) -> Result<crate::artifacts::ArtifactBytes, String>;
    /// What the store occupies, for the cleanup UI.
    fn artifact_usage(&self) -> crate::artifacts::StoreUsage;

    // ── Decision-card assets (fleet__ask images) ─────────────────────────────
    /// Raw bytes + mime of one file in a `fleet__ask` question's decision-asset
    /// dir (`~/.fleet/decision-assets/<id>/q<idx>/<relpath>`). Backs the
    /// `fleet-decision://` protocol, so it must work for both local and remote
    /// backends. `relpath` is typically `index.html` or a bare image name.
    fn get_decision_asset(
        &self,
        id: &str,
        qidx: &str,
        relpath: &str,
    ) -> Result<crate::mcp_ipc::DecisionAssetBytes, String>;

    /// Resolve a [`crate::mcp_ipc::ReviewDoc`] (a `.md` file or wiki entry the
    /// agent attached to a `fleet__ask` card) to its current body, so the card's
    /// side panel can render it. Fetched live rather than snapshotted, so it must
    /// work for both local and remote backends.
    fn read_review_doc(
        &self,
        doc: &crate::mcp_ipc::ReviewDoc,
    ) -> Result<crate::mcp_ipc::ReviewDocContent, String>;

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

    /// The workspace's whole execution chain: the `parent`-linked plan forest
    /// with each plan's handoff relay chains folded onto its node.
    ///
    /// Deliberately abstract (no default body) so the compiler — not the drift
    /// guard — forces every transport to implement it.
    fn get_plan_forest(&self, workspace_path: &str) -> crate::plan_forest::PlanForest;

    // ── User-added browse paths ───────────────────────────────────────────────
    // Directories the user added by hand ("添加路径") or cloned from the 仓库
    // page. They have no sessions of their own, so without this registry the
    // explorer's session-derived gate rejects them — see `browse_paths`.
    // Every method returns the updated list so the caller never has to re-fetch.
    fn list_browse_paths(&self) -> Vec<String>;
    fn add_browse_path(&self, path: &str) -> Result<Vec<String>, String>;
    fn remove_browse_path(&self, path: &str) -> Result<Vec<String>, String>;

    // ── File explorer ─────────────────────────────────────────────────────────
    // Read-only browsing of a workspace the backend knows about — one with
    // sessions, or one registered above — and its linked git worktrees. `root`
    // must be one of the paths returned by `list_explorer_roots`.
    fn list_explorer_roots(
        &self,
        workspace: &str,
    ) -> Result<Vec<crate::file_explorer::ExplorerRoot>, String>;
    fn list_explorer_dir(
        &self,
        workspace: &str,
        root: &str,
        rel_path: &str,
        show_ignored: bool,
    ) -> Result<Vec<crate::file_explorer::ExplorerEntry>, String>;
    fn read_explorer_file(
        &self,
        workspace: &str,
        root: &str,
        rel_path: &str,
    ) -> Result<crate::file_explorer::ExplorerFileContent, String>;
    /// Root-relative paths of every file under `root` ending with `rel_suffix`.
    /// The explorer's fallback when a path clicked in agent prose does not
    /// exist where the literal join put it — see `file_explorer::find_by_suffix`.
    fn find_explorer_path(
        &self,
        workspace: &str,
        root: &str,
        rel_suffix: &str,
    ) -> Result<Vec<String>, String>;
    /// Read one absolute path that belongs to no workspace — the `/tmp/foo.md`
    /// an agent just wrote and named in prose. Single file, no listing: see
    /// `file_explorer::read_external_file` for why that is the whole surface.
    fn read_external_file(
        &self,
        path: &str,
    ) -> Result<crate::file_explorer::ExplorerFileContent, String>;

    // ── Session scratchpad ────────────────────────────────────────────────────
    // Read-only browsing of the private temp dir Claude Code hands a session.
    // The root is derived server-side from `workspace` + `session_id`; an Err
    // means the session has no scratchpad, and the UI hides the tab.
    fn list_scratchpad_dir(
        &self,
        workspace: &str,
        session_id: &str,
        rel_path: &str,
    ) -> Result<Vec<crate::file_explorer::ExplorerEntry>, String>;
    fn read_scratchpad_file(
        &self,
        workspace: &str,
        session_id: &str,
        rel_path: &str,
    ) -> Result<crate::file_explorer::ExplorerFileContent, String>;

    // ── Source control ──────────────────────────────────────────────────────────
    // Simple git status + push/pull for an explorer `root`. `workspace` / `root`
    // are validated the same way as the explorer methods above.
    fn git_status(
        &self,
        workspace: &str,
        root: &str,
    ) -> Result<crate::git_ops::GitStatus, String>;
    fn git_push(
        &self,
        workspace: &str,
        root: &str,
    ) -> Result<crate::git_ops::GitOpResult, String>;
    fn git_pull(
        &self,
        workspace: &str,
        root: &str,
    ) -> Result<crate::git_ops::GitOpResult, String>;
    /// Start `git clone` as a streaming workspace command and return its
    /// record, so the caller can tail git's own progress counters through
    /// `proc_output` instead of staring at a spinner for the whole clone.
    ///
    /// Runs on the backend's host, and is validated there — the destination's
    /// parent has to exist on the machine doing the cloning, which is the probe
    /// (not this desktop) on a remote connection.
    fn start_git_clone(
        &self,
        url: &str,
        dest: &str,
    ) -> Result<crate::proc_runner::ProcRecord, String>;

    /// `git clone <url> <dest>` on the backend's host, blocking until it
    /// finishes. Unlike the three above,
    /// this takes no `workspace`: a fresh clone isn't a known workspace yet, so
    /// `git_ops::git_clone` guards it structurally instead (absolute dest,
    /// existing parent, empty-or-absent dest — never overwrites anything).
    fn git_clone(&self, url: &str, dest: &str) -> Result<crate::git_ops::GitOpResult, String>;

    // ── Skills ────────────────────────────────────────────────────────────────
    fn list_skills(&self) -> Vec<SkillItem>;
    fn skill_sync_inventory(&self) -> Result<Vec<crate::skill_sync::SkillSyncEntry>, String>;
    fn skill_sync_apply(&self) -> Result<crate::skill_sync::SkillSyncReport, String>;
    fn skill_sync_adopt(&self, path: &str) -> Result<crate::skill_sync::SkillSyncReport, String>;
    fn skill_sync_unlink(
        &self,
        slug: &str,
        target: crate::skill_sync::SkillTarget,
    ) -> Result<crate::skill_sync::SkillSyncAction, String>;
    /// Whether automatic cross-runtime skill reconciliation is enabled.
    fn get_skill_autosync(&self) -> Result<bool, String>;
    /// Toggle automatic cross-runtime skill reconciliation on/off.
    fn set_skill_autosync(&self, enabled: bool) -> Result<(), String>;
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

    /// Codex-native token breakdown for a single `codex://` rollout. Codex
    /// rollouts have none of the source-attribution buckets
    /// `get_task_token_breakdown` reports, so the desktop "Token" tab routes
    /// codex sessions here instead. See `codex_source::codex_token_breakdown`.
    fn get_codex_token_breakdown(
        &self,
        jsonl_path: &str,
    ) -> Result<crate::codex_source::CodexTokenBreakdown, String>;

    /// dsh-native token breakdown for a single `dsh://` session. dsh has no
    /// transcript file for the other two breakdowns to parse, so the desktop
    /// "Token" tab routes dsh sessions here, where the numbers come off the
    /// server's own projections. See `dsh_source::dsh_token_breakdown`.
    fn get_dsh_token_breakdown(
        &self,
        uri: &str,
    ) -> Result<crate::dsh_source::DshTokenBreakdown, String>;

    /// Real spend for a single `dsh://` session, asked of the provider rather
    /// than inferred from token counts. Separate from the breakdown above
    /// because it needs the full `session.history` and may hit the network, so
    /// the panel renders tokens first and fills the cost in after. See
    /// `dsh_cost::dsh_session_cost`.
    fn get_dsh_session_cost(&self, uri: &str)
        -> Result<crate::dsh_cost::DshSessionCost, String>;

    /// dsh's model catalogue, so the launcher can offer real `provider/model`
    /// pairs and each model's own effort scale. Session-independent — dsh
    /// describes the host's configured providers, so there is nothing to key
    /// on. See `dsh_source::dsh_models`.
    fn dsh_models(&self) -> Result<crate::dsh_source::DshModelCatalog, String>;

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

    // ── Wiki guidance (global CLAUDE.md block) ──────────────────────────────
    fn apply_wiki_guidance(&self, locale: &str) -> Result<(), String>;
    fn remove_wiki_guidance(&self) -> Result<(), String>;

    // ── Model guidance (global CLAUDE.md block) ─────────────────────────────
    fn apply_model_guidance(&self, locale: &str) -> Result<(), String>;
    fn remove_model_guidance(&self) -> Result<(), String>;
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

    // ── Non-Claude guidance carriers (global AGENTS.md blocks) ──────────────
    /// Mirror the Claude-side concept toggles (interaction / PRD / wiki / model)
    /// onto every non-Claude carrier: codex's `~/.codex/AGENTS.md` **and** dsh's
    /// `$DSH_HOME/AGENTS.md`. One concept toggle drives all carriers: the desktop
    /// calls this after any concept toggle and on startup, so each harness's
    /// per-concept blocks always match the Claude sentinels. Reads the enabled
    /// set from the Claude carriers, so it is idempotent and order-independent.
    /// Default-bodied so unsupported backends fail loudly; LocalBackend and
    /// RemoteBackend override it.
    ///
    /// The name stays codex-specific because it is also the Tauri command name,
    /// the `/reconcile_codex_guidance` route, and the string the frontend
    /// `invoke`s — renaming it would break a desktop talking to an older
    /// `fleet serve`. Read it as "reconcile the non-Claude guidance carriers".
    fn reconcile_codex_guidance(&self, _user_title: &str, _locale: &str) -> Result<(), String> {
        Err("agent guidance not supported by this backend".into())
    }

    // ── Agent sources config ─────────────────────────────────────────────────
    fn get_sources_config(&self) -> Vec<crate::agent_source::SourceInfo>;
    fn set_source_enabled(&self, name: &str, enabled: bool) -> Result<(), String>;

    /// Codex profile-v2 files on the **host that will run Codex** — the model
    /// picker's source of non-official models.
    ///
    /// Must go through the backend rather than reading `$CODEX_HOME` directly:
    /// for a remote workspace the profiles that matter live on the probe host,
    /// not on the machine running the desktop app, and a local read there would
    /// offer models the remote Codex cannot resolve. See
    /// [`crate::codex_launch::list_codex_profiles`] for the file convention.
    ///
    /// Defaults to empty so a not-yet-initialised backend simply shows no
    /// custom profiles instead of failing the picker.
    fn list_codex_profiles(&self) -> Vec<crate::codex_launch::CodexProfile> {
        Vec::new()
    }

    // ── Claude binary discovery & override ───────────────────────────────────
    /// Enumerate every Claude CLI binary fleet can find on the host, ranked by
    /// priority. The first entry is what fleet would auto-pick if no override
    /// is set. Multiple entries cover the case where the user has the official
    /// installer **and** the VS Code extension bundle installed
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
    /// List lessons currently recorded in the managed `~/.claude/fleet-lessons.md`.
    fn list_managed_lessons(&self)
        -> Result<Vec<crate::lessons_store::ManagedLesson>, String>;
    /// Remove a managed lesson by its stable id.
    fn remove_managed_lesson(&self, id: &str) -> Result<(), String>;

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

    /// Return persisted codex primary/secondary occupancy snapshots in
    /// [from_ms, to_ms] (epoch ms) for the codex 24h occupancy-trend chart.
    /// Sorted ascending by ts. The codex parallel of [`Self::usage_history`].
    fn codex_usage_history(
        &self,
        from_ms: i64,
        to_ms: i64,
    ) -> Vec<crate::codex_usage_history::CodexUsageHistoryPoint>;

    // ── User-direction attachments ──────────────────────────────────────────
    /// Make a local file available to the agent process as an absolute path.
    /// UI layers splice the returned path into the prompt (`Context files:`) or
    /// the decision answer (`@<path>`) so Claude Code picks it up.
    ///
    /// `from_clipboard` distinguishes the two kinds of attachment, which need
    /// opposite treatment:
    ///
    /// * `true` — pasted/dropped bytes. They have no home of their own, so they
    ///   are ingested into the persistent store ([`crate::user_attachments`]) and
    ///   the store path is returned. That path is what lands in the transcript,
    ///   so history can still resolve (and render) the image later.
    /// * `false` — a file the user *picked*, which already has a meaningful path.
    ///   It is returned unchanged: copying it into the store would point the
    ///   agent at a snapshot instead of the file the user meant.
    ///
    /// RemoteBackend uploads the bytes to the probe host either way, since the
    /// agent runs there and cannot see the desktop's filesystem at all.
    fn upload_attachment(
        &self,
        source_path: &std::path::Path,
        from_clipboard: bool,
    ) -> Result<String, String>;

    /// Raw bytes + mime of one file in the user-attachment store
    /// (`~/.fleet/user-attachments/<key>/<name>`). Backs the
    /// `fleet-attachment://` protocol that renders thumbnails in history, so it
    /// must work for both local and remote backends.
    fn get_user_attachment(
        &self,
        key: &str,
        name: &str,
    ) -> Result<crate::mcp_ipc::DecisionAssetBytes, String>;

    // ── Mobile relay (移动端 mobile web channel) ────────────────────────────
    fn get_mobile_relay_config(&self) -> Result<crate::mobile_relay::MobileRelayConfig, String> {
        Ok(crate::mobile_relay::MobileRelayConfig::default())
    }
    /// Persist the config; an enabled config with an empty secret keeps the
    /// existing one or generates it (see `mobile_relay::set_config_normalized`),
    /// so the frontend never round-trips the secret. Returns the stored config.
    fn set_mobile_relay_config(
        &self,
        _cfg: crate::mobile_relay::MobileRelayConfig,
    ) -> Result<crate::mobile_relay::MobileRelayConfig, String> {
        Err("mobile relay not configured".into())
    }
    /// Rotate the pairing secret — invalidates the previous QR code.
    fn rotate_mobile_relay_secret(&self) -> Result<crate::mobile_relay::MobileRelayConfig, String> {
        Err("mobile relay not configured".into())
    }
    fn mobile_relay_status(&self) -> Result<crate::mobile_relay::MobileRelayStatus, String> {
        Err("mobile relay not configured".into())
    }
    /// SVG QR code of the pairing URL (`<relay>/#k=<secret>`). `lang` carries
    /// the viewer's current UI language so a fresh scan opens the phone in the
    /// same language (`&lang=zh|en`); `None` omits it.
    fn mobile_relay_qr_svg(&self, _lang: Option<&str>) -> Result<String, String> {
        Err("mobile relay not configured".into())
    }
    /// The same pairing URL as text, for copy-to-clipboard. Needed because a
    /// self-hosted relay cannot be reached by scanning: Android App Links only
    /// fire for hosts baked into the manifest, so the scan opens a browser and
    /// the phone app never sees the link. Pasting is that user's way in.
    fn mobile_relay_pairing_url(&self, _lang: Option<&str>) -> Result<String, String> {
        Err("mobile relay not configured".into())
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
    use crate::account::{AccountInfo, ScopedUsage, UsageStats};

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
            fleet_spawned: false,
            parent_session_id: None,
            agent_type: None,
            agent_description: None,
            slug: None,
            ai_title: ai_title.map(|s| s.to_string()),
            status: SessionStatus::Idle,
            token_speed: 0.0,
            agent_token_speed: 0.0,
            total_output_tokens: 0,
            reasoning_output_tokens: 0,
            total_input_tokens: 0,
            total_cost_usd: 0.0,
            agent_total_cost_usd: 0.0,
            cost_speed_usd_per_min: 0.0,
            last_message_preview: None,
            last_activity_ms: 0,
            agent_last_activity_ms: 0,
            running_subagent_count: 0,
            created_at_ms: 0,
            jsonl_path: format!("/tmp/{id}.jsonl"),
            model: None,
            thinking_level: None,
            pid: None,
            pid_precise: false,
            proc_alive: false,
            pending_tool_batch: false,
            last_skill: None,
            context_percent: None,
            agent_source: "claude-code".into(),
            last_outcome: None,
            rate_limit: None,
            todos: None,
            background_tasks: Vec::new(),
            task_plan: None, handoff: None, user_mark: None, title_override: None, last_read_ms: None,            compact_count: 0,
            compact_pre_tokens: 0,
            compact_post_tokens: 0,
            compact_cost_usd: 0.0,
            pending_messages: Vec::new(),
            watches: Vec::new(),
        }
    }

    #[test]
    fn resolve_pending_display_fills_empty_workspace_and_title() {
        use crate::elicitation::{ElicitationQuestion, ElicitationRequest};
        let mut pending = PendingDecisions::default();
        pending.elicitation.push(ElicitationRequest {
            parked: false,
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
    fn resolve_pending_display_prefers_title_override_over_ai_title() {
        use crate::elicitation::ElicitationRequest;
        let mut pending = PendingDecisions::default();
        pending.elicitation.push(ElicitationRequest {
            parked: false,
            id: "e1".into(),
            session_id: "s1".into(),
            workspace_name: String::new(),
            ai_title: None,
            questions: vec![],
            timestamp: "t".into(),
        });
        // Codex-like session: `ai_title` is the raw first prompt; the real,
        // human/agent-set title lives in the override. The decision card must
        // show the override, not the raw prompt.
        let mut s = mk_session("s1", "ws", Some("raw first prompt"));
        s.title_override = Some("Renamed nicely".into());
        resolve_pending_display(&mut pending, &[s]);
        assert_eq!(pending.elicitation[0].ai_title.as_deref(), Some("Renamed nicely"));
    }

    #[test]
    fn resolve_pending_display_preserves_existing_and_handles_unknown_session() {
        use crate::elicitation::ElicitationRequest;
        let mut pending = PendingDecisions::default();
        // Already-populated workspace must be preserved.
        pending.elicitation.push(ElicitationRequest {
            parked: false,
            id: "e1".into(),
            session_id: "s1".into(),
            workspace_name: "preset-ws".into(),
            ai_title: Some("preset-title".into()),
            questions: vec![],
            timestamp: "t".into(),
        });
        // Unknown session → left as-is (empty), no panic.
        pending.elicitation.push(ElicitationRequest {
            parked: false,
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
            seven_day_scoped: vec![ScopedUsage {
                model_label: "Fable".into(),
                utilization: 0.1,
                resets_at: "2026-01-07T00:00:00Z".into(),
                prev_utilization: None,
            }],
            ..Default::default()
        };
        let s = SourceUsageSummary::from_claude(&info);
        assert_eq!(s.source, "claude");
        assert_eq!(s.plan, Some("Max 5x".into()));
        assert_eq!(s.bars.len(), 3);
        assert_eq!(s.bars[0].label, "5h");
        assert!((s.bars[0].utilization - 0.3).abs() < f64::EPSILON);
        assert_eq!(s.bars[1].label, "7d Opus");
        assert_eq!(s.bars[2].label, "7d Fable");
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
    fn from_codex_carries_the_usage_source_through() {
        // The normalised summary feeds the tray menu and the mobile usage view;
        // dropping the label there would leave those two surfaces unable to say
        // whether the numbers came from foxy.
        let val = json!({
            "planType": "team",
            "primary": {"usedPercent": 1, "resetsAt": 1_787_622_736_i64},
            "usageSource": "foxy-switcher"
        });
        let s = SourceUsageSummary::from_codex(&val);
        assert_eq!(s.usage_source.as_deref(), Some("foxy-switcher"));
    }

    #[test]
    fn from_codex_without_a_usage_source_reports_none() {
        // An older backend (or a payload predating the field) must not surface
        // an empty-string source the UI would try to translate.
        let s = SourceUsageSummary::from_codex(&json!({ "planType": "team" }));
        assert_eq!(s.usage_source, None);
    }

    #[test]
    fn from_claude_carries_the_usage_source_through() {
        let info = AccountInfo {
            usage_source: "foxy-switcher".into(),
            ..AccountInfo::default()
        };
        let s = SourceUsageSummary::from_claude(&info);
        assert_eq!(s.usage_source.as_deref(), Some("foxy-switcher"));
    }

    #[test]
    fn from_claude_empty_usage_source_reports_none() {
        let s = SourceUsageSummary::from_claude(&AccountInfo::default());
        assert_eq!(s.usage_source, None);
    }

    #[test]
    fn from_codex_missing_plan() {
        let val = json!({});
        let s = SourceUsageSummary::from_codex(&val);
        assert_eq!(s.plan, None);
        assert!(s.bars.is_empty());
    }

}
