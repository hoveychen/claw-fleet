use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};

use chrono::Utc;
use claw_fleet_core::agent_source::SpawnSpec;
use claw_fleet_core::backend::{HarnessEvent, HarnessEventSink};
use fleet_cloud_wire::event::{DecisionCreated, DecisionKind, RunnerEvent};
use fleet_cloud_wire::runner::{CloudCommand, CommandAckStatus};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::outbox::EventOutbox;

pub const TASK_ENV: &str = "FLEET_CLOUD_TASK_ID";
pub const RUN_ENV: &str = "FLEET_CLOUD_RUN_ID";
pub const EVENT_LOG_ENV: &str = "FLEET_CLOUD_EVENT_LOG";

#[derive(Debug)]
pub struct CommandResult {
    pub status: CommandAckStatus,
    pub result: Option<Value>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct HarnessSnapshot {
    pub messages: Vec<Value>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

pub trait Harness: Send + Sync {
    fn spawn(
        &self,
        provider: &str,
        spec: &SpawnSpec,
    ) -> Result<claw_fleet_core::session_launch::SpawnSessionResponse, String>;
    fn terminate(&self, pid: u32) -> Result<(), String>;
    fn enqueue(&self, session_id: &str, workspace: &str, text: &str) -> Result<(), String>;
    fn snapshot(&self, session_id: &str) -> Result<Option<HarnessSnapshot>, String>;
    fn pending_decisions(&self) -> Result<Vec<PendingDecision>, String> {
        Ok(Vec::new())
    }
    fn respond_decision(&self, _kind: &str, _id: &str, _response: &Value) -> Result<(), String> {
        Err("decision responses are not supported by this harness".into())
    }
}

#[derive(Debug, Clone)]
pub struct PendingDecision {
    pub id: String,
    pub session_id: String,
    pub kind: String,
    pub request: Value,
}

#[derive(Default)]
pub struct CoreHarness;

impl Harness for CoreHarness {
    fn spawn(
        &self,
        provider: &str,
        spec: &SpawnSpec,
    ) -> Result<claw_fleet_core::session_launch::SpawnSessionResponse, String> {
        let tool = match provider {
            "claude_code" => "claude",
            "codex" => "codex",
            other => return Err(format!("unsupported provider {other}")),
        };
        claw_fleet_core::agent_source::spawn_session(tool, spec)
    }

    fn terminate(&self, pid: u32) -> Result<(), String> {
        claw_fleet_core::session::kill_pid_impl(pid)
    }

    fn enqueue(&self, session_id: &str, workspace: &str, text: &str) -> Result<(), String> {
        claw_fleet_core::pending_message::enqueue(session_id, workspace, text)
    }

    fn snapshot(&self, session_id: &str) -> Result<Option<HarnessSnapshot>, String> {
        let sources = claw_fleet_core::agent_source::build_sources();
        let Some((source, session)) = sources.iter().find_map(|source| {
            source
                .scan_sessions()
                .into_iter()
                .find(|session| session.id == session_id)
                .map(|session| (source.as_ref(), session))
        }) else {
            return Ok(None);
        };
        Ok(Some(HarnessSnapshot {
            messages: source.get_messages(&session.jsonl_path).unwrap_or_default(),
            input_tokens: session.total_input_tokens,
            output_tokens: session.total_output_tokens,
            cost_usd: session.total_cost_usd,
        }))
    }

    fn pending_decisions(&self) -> Result<Vec<PendingDecision>, String> {
        let mut decisions = Vec::new();
        macro_rules! collect {
            ($kind:literal, $list:path, $read:path) => {
                for id in $list() {
                    if let Some(request) = $read(&id) {
                        let value = serde_json::to_value(&request).map_err(|e| e.to_string())?;
                        decisions.push(PendingDecision {
                            id,
                            session_id: value["sessionId"].as_str().unwrap_or_default().into(),
                            kind: $kind.into(),
                            request: value,
                        });
                    }
                }
            };
        }
        collect!(
            "guard",
            claw_fleet_core::guard::list_pending_requests,
            claw_fleet_core::guard::read_request
        );
        collect!(
            "elicitation",
            claw_fleet_core::elicitation::list_pending_requests,
            claw_fleet_core::elicitation::read_request
        );
        collect!(
            "fleet_ask",
            claw_fleet_core::mcp_ipc::list_pending_requests,
            claw_fleet_core::mcp_ipc::read_request
        );
        collect!(
            "plan_approval",
            claw_fleet_core::plan_approval::list_pending_requests,
            claw_fleet_core::plan_approval::read_request
        );
        collect!(
            "a2ui",
            claw_fleet_core::mcp_a2ui_ipc::list_pending_requests,
            claw_fleet_core::mcp_a2ui_ipc::read_request
        );
        collect!(
            "permission_prompt",
            claw_fleet_core::permission_prompt_ipc::list_pending_requests,
            claw_fleet_core::permission_prompt_ipc::read_request
        );
        Ok(decisions)
    }

    fn respond_decision(&self, kind: &str, id: &str, response: &Value) -> Result<(), String> {
        let action = response["action"].as_str().unwrap_or("answer");
        let answers = response.get("answers").cloned().unwrap_or_default();
        match kind {
            "guard" => {
                claw_fleet_core::guard::write_response(&claw_fleet_core::guard::GuardResponse {
                    id: id.into(),
                    decision: if matches!(action, "allow" | "approve" | "answer") {
                        claw_fleet_core::guard::GuardDecision::Allow
                    } else {
                        claw_fleet_core::guard::GuardDecision::Block
                    },
                    reason: answers["reason"].as_str().map(str::to_owned),
                })
            }
            "elicitation" => claw_fleet_core::elicitation::write_response(
                &claw_fleet_core::elicitation::ElicitationResponse {
                    id: id.into(),
                    declined: matches!(action, "decline" | "deny" | "reject" | "cancel"),
                    answers: serde_json::from_value(answers).map_err(|e| e.to_string())?,
                },
            ),
            "fleet_ask" => claw_fleet_core::mcp_ipc::write_response(
                &claw_fleet_core::mcp_ipc::FleetAskResponse {
                    id: id.into(),
                    answers: serde_json::from_value(answers).map_err(|e| e.to_string())?,
                    cancelled: action == "cancel",
                },
            ),
            "plan_approval" => claw_fleet_core::plan_approval::write_response(
                &claw_fleet_core::plan_approval::PlanApprovalResponse {
                    id: id.into(),
                    decision: if matches!(action, "approve" | "allow" | "answer") {
                        "approve"
                    } else {
                        "reject"
                    }
                    .into(),
                    edited_plan: answers["editedPlan"].as_str().map(str::to_owned),
                    feedback: answers["feedback"].as_str().map(str::to_owned),
                },
            ),
            "a2ui" => claw_fleet_core::mcp_a2ui_ipc::write_response(
                &claw_fleet_core::mcp_a2ui_ipc::A2uiRenderResponse {
                    id: id.into(),
                    action_name: (action != "cancel").then(|| action.to_owned()),
                    action_context: serde_json::from_value(answers).map_err(|e| e.to_string())?,
                    cancelled: action == "cancel",
                },
            ),
            "permission_prompt" => claw_fleet_core::permission_prompt_ipc::write_response(
                &claw_fleet_core::permission_prompt_ipc::PermissionPromptResponse {
                    id: id.into(),
                    decision: if matches!(action, "allow" | "approve" | "answer") {
                        claw_fleet_core::permission_prompt_ipc::PermissionPromptDecision::Allow
                    } else {
                        claw_fleet_core::permission_prompt_ipc::PermissionPromptDecision::Deny
                    },
                    reason: answers["reason"].as_str().map(str::to_owned),
                },
            ),
            _ => Err(format!("unsupported decision kind {kind}")),
        }
    }
}

struct ChannelSink(mpsc::Sender<HarnessEvent>);

impl HarnessEventSink for ChannelSink {
    fn emit(&self, event: HarnessEvent) {
        let _ = self.0.send(event);
    }
}

pub struct Supervisor {
    connection: Connection,
    workspace_root: PathBuf,
    event_log: PathBuf,
    event_rx: mpsc::Receiver<HarnessEvent>,
    outbox: Arc<EventOutbox>,
    harness: Arc<dyn Harness>,
}

impl Supervisor {
    pub fn open(state_directory: &Path, outbox: Arc<EventOutbox>) -> anyhow::Result<Self> {
        Self::open_with_harness(state_directory, outbox, Arc::new(CoreHarness))
    }

    pub fn open_with_harness(
        state_directory: &Path,
        outbox: Arc<EventOutbox>,
        harness: Arc<dyn Harness>,
    ) -> anyhow::Result<Self> {
        fs::create_dir_all(state_directory)?;
        let workspace_root = state_directory.join("workspaces");
        fs::create_dir_all(&workspace_root)?;
        let event_log = state_directory.join("harness-events.jsonl");
        let connection = Connection::open(state_directory.join("supervisor.sqlite"))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS tasks(
                task_id TEXT PRIMARY KEY, active_run_id TEXT, paused INTEGER NOT NULL DEFAULT 0,
                launch_json TEXT NOT NULL, pending_message TEXT
             );
             CREATE TABLE IF NOT EXISTS runs(
                run_id TEXT PRIMARY KEY, task_id TEXT NOT NULL, provider TEXT NOT NULL,
                session_id TEXT, pid INTEGER, status TEXT NOT NULL, workspace_path TEXT NOT NULL,
                model TEXT, effort TEXT, permission_policy TEXT
             );
             CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value INTEGER NOT NULL);",
        )?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS decisions(
                decision_id TEXT PRIMARY KEY, run_id TEXT NOT NULL, kind TEXT NOT NULL,
                request_json TEXT NOT NULL, resolved INTEGER NOT NULL DEFAULT 0
             );",
        )?;
        let (event_tx, event_rx) = mpsc::channel();
        claw_fleet_core::backend::set_harness_event_sink(Arc::new(ChannelSink(event_tx)));
        Ok(Self {
            connection,
            workspace_root,
            event_log,
            event_rx,
            outbox,
            harness,
        })
    }

    pub fn active_runs(&self) -> u16 {
        self.connection
            .query_row(
                "SELECT count(*) FROM runs WHERE status IN ('starting','running')",
                [],
                |row| row.get::<_, u16>(0),
            )
            .unwrap_or(0)
    }

    pub fn execute(&mut self, command: &CloudCommand) -> CommandResult {
        let result = match command.command_type.as_str() {
            "start_run" => self.start_run(command, command.run_id.clone()),
            "append_message" => self.append_message(command),
            "cancel_task" | "cancel_run" => self.cancel(command),
            "pause_task" | "pause_runner" => self.pause(command),
            "resume_task" => self.resume(command),
            "resolve_decision" => self.resolve_decision(command),
            other => Err((
                "unsupported_command",
                format!("unsupported command {other}"),
            )),
        };
        match result {
            Ok(value) => CommandResult {
                status: CommandAckStatus::Completed,
                result: Some(value),
                error_code: None,
            },
            Err((code, message)) => CommandResult {
                status: if code == "invalid_command" {
                    CommandAckStatus::Rejected
                } else {
                    CommandAckStatus::Failed
                },
                result: Some(json!({"message":message})),
                error_code: Some(code.into()),
            },
        }
    }

    pub fn reconcile(&mut self) -> anyhow::Result<()> {
        while let Ok(event) = self.event_rx.try_recv() {
            self.apply_harness_event(event)?;
        }
        self.drain_event_log()?;
        self.sync_pending_decisions()?;
        Ok(())
    }

    fn sync_pending_decisions(&mut self) -> anyhow::Result<()> {
        for decision in self
            .harness
            .pending_decisions()
            .map_err(anyhow::Error::msg)?
        {
            let run: Option<(String, String)> = self
                .connection
                .query_row(
                    "SELECT run_id,task_id FROM runs WHERE session_id=?1 ORDER BY rowid DESC LIMIT 1",
                    [&decision.session_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((run_id, task_id)) = run else {
                continue;
            };
            let inserted = self.connection.execute(
                "INSERT OR IGNORE INTO decisions(decision_id,run_id,kind,request_json) VALUES(?1,?2,?3,?4)",
                params![decision.id,run_id,decision.kind,decision.request.to_string()],
            )?;
            if inserted == 0 {
                continue;
            }
            let kind = match decision.kind.as_str() {
                "guard" => DecisionKind::Guard,
                "elicitation" => DecisionKind::Elicitation,
                "fleet_ask" => DecisionKind::FleetAsk,
                "plan_approval" => DecisionKind::PlanApproval,
                "a2ui" => DecisionKind::A2ui,
                "permission_prompt" => DecisionKind::PermissionPrompt,
                other => anyhow::bail!("unsupported pending Decision kind {other}"),
            };
            self.emit(
                &format!("{}:created", decision.id),
                "decision.created",
                &task_id,
                Some(&run_id),
                json!({
                    "task_id":task_id,"run_id":run_id,
                    "decision": DecisionCreated {
                        source_decision_id: decision.id.clone(),
                        kind,
                        payload: decision.request,
                        response_schema: json!({}),
                        deadline: None,
                    }
                }),
            )?;
            self.emit(
                &format!("{task_id}:waiting:{}", decision.id),
                "task.status_changed",
                &task_id,
                Some(&run_id),
                json!({"task_id":task_id,"run_id":run_id,"status":"waiting_input"}),
            )?;
        }
        Ok(())
    }

    fn resolve_decision(
        &mut self,
        command: &CloudCommand,
    ) -> Result<Value, (&'static str, String)> {
        let task_id = required_task(command)?;
        let cloud_id = command.payload["decision_id"].as_str().ok_or((
            "invalid_command",
            "resolve_decision requires decision_id".into(),
        ))?;
        let source_id = command.payload["source_decision_id"].as_str().ok_or((
            "invalid_command",
            "resolve_decision requires source_decision_id".into(),
        ))?;
        let row: Option<(String, String, i64)> = self
            .connection
            .query_row(
                "SELECT d.kind,d.run_id,d.resolved FROM decisions d JOIN runs r ON r.run_id=d.run_id WHERE d.decision_id=?1 AND r.task_id=?2",
                params![source_id,task_id],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
            )
            .optional()
            .map_err(db_error)?;
        let Some((kind, run_id, resolved)) = row else {
            return Err((
                "decision_not_found",
                "decision is not pending on this Runner".into(),
            ));
        };
        if resolved == 0 {
            self.harness
                .respond_decision(&kind, source_id, &command.payload["response"])
                .map_err(harness_error)?;
            self.emit(
                &format!("{cloud_id}:delivered"),
                "decision.delivered",
                task_id,
                Some(&run_id),
                json!({
                    "task_id":task_id,"run_id":run_id,"decision_id":cloud_id,
                    "source_decision_id":source_id,"kind":kind,
                    "status":command.payload["terminal_status"]
                }),
            )
            .map_err(outbox_error)?;
            self.connection
                .execute(
                    "UPDATE decisions SET resolved=1 WHERE decision_id=?1",
                    [source_id],
                )
                .map_err(db_error)?;
        }
        Ok(json!({"decision_id":cloud_id,"status":"delivered"}))
    }

    fn start_run(
        &mut self,
        command: &CloudCommand,
        forced_run_id: Option<String>,
    ) -> Result<Value, (&'static str, String)> {
        let task_id = command
            .task_id
            .as_deref()
            .ok_or(("invalid_command", "start_run requires task_id".into()))?;
        let run_id = forced_run_id.unwrap_or_else(new_run_id);
        let goal = command.payload["goal"]
            .as_str()
            .or_else(|| command.payload["message"].as_str())
            .unwrap_or("continue")
            .trim();
        if goal.is_empty() {
            return Err(("invalid_command", "run prompt must not be empty".into()));
        }
        let provider = command.payload["agent"]["provider"]
            .as_str()
            .unwrap_or("claude_code");
        let workspace = self.prepare_workspace(task_id, &run_id, &command.payload["workspace"])?;
        let model = command.payload["agent"]["model"]
            .as_str()
            .map(str::to_owned);
        let effort = command.payload["agent"]["effort"]
            .as_str()
            .map(str::to_owned);
        let permission = command.payload["agent"]["permission_policy_id"]
            .as_str()
            .map(str::to_owned);
        let spec = SpawnSpec {
            workspace_path: workspace.to_string_lossy().into_owned(),
            prompt: goal.to_owned(),
            model: model.clone(),
            effort: effort.clone(),
            permission_mode: None,
            session_id: None,
            entrypoint: "fleet-cloud-runner".into(),
            environment: vec![
                (TASK_ENV.into(), task_id.into()),
                (RUN_ENV.into(), run_id.clone()),
                (
                    EVENT_LOG_ENV.into(),
                    self.event_log.to_string_lossy().into_owned(),
                ),
            ],
        };
        self.connection
            .execute(
                "INSERT INTO tasks(task_id,active_run_id,paused,launch_json) VALUES(?1,?2,0,?3)
                 ON CONFLICT(task_id) DO UPDATE SET active_run_id=excluded.active_run_id,paused=0,launch_json=excluded.launch_json",
                params![task_id, run_id, command.payload.to_string()],
            )
            .map_err(db_error)?;
        self.connection
            .execute(
                "INSERT OR REPLACE INTO runs(run_id,task_id,provider,status,workspace_path,model,effort,permission_policy)
                 VALUES(?1,?2,?3,'starting',?4,?5,?6,?7)",
                params![run_id, task_id, provider, workspace.to_string_lossy(), model, effort, permission],
            )
            .map_err(db_error)?;
        let response = self.harness.spawn(provider, &spec).map_err(harness_error)?;
        let session_id = response.session_id.ok_or((
            "harness_launch_failed",
            "provider returned no session reference".into(),
        ))?;
        self.connection
            .execute(
                "UPDATE runs SET session_id=?2,pid=?3,status='running' WHERE run_id=?1",
                params![run_id, session_id, i64::from(response.pid)],
            )
            .map_err(db_error)?;
        self.emit(
            &format!("{run_id}:started"),
            "run.started",
            task_id,
            Some(&run_id),
            json!({"task_id":task_id,"run_id":run_id,"provider":provider,"status":"running"}),
        )
        .map_err(outbox_error)?;
        self.emit(
            &format!("{task_id}:{run_id}:running"),
            "task.status_changed",
            task_id,
            Some(&run_id),
            json!({"task_id":task_id,"run_id":run_id,"status":"running"}),
        )
        .map_err(outbox_error)?;
        Ok(json!({"task_id":task_id,"run_id":run_id,"status":"running"}))
    }

    fn append_message(&mut self, command: &CloudCommand) -> Result<Value, (&'static str, String)> {
        let task_id = required_task(command)?;
        let text = command.payload["text"]
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(("invalid_command", "append_message requires text".into()))?;
        let row: Option<(i64, Option<String>, Option<String>, Option<String>)> = self
            .connection
            .query_row(
                "SELECT t.paused,r.session_id,r.workspace_path,r.status FROM tasks t LEFT JOIN runs r ON r.run_id=t.active_run_id WHERE t.task_id=?1",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(db_error)?;
        let Some((paused, session_id, workspace, status)) = row else {
            return Err(("task_not_found", "task is not known to this Runner".into()));
        };
        if paused != 0 {
            self.connection
                .execute(
                    "UPDATE tasks SET pending_message=?2 WHERE task_id=?1",
                    params![task_id, text],
                )
                .map_err(db_error)?;
            return Ok(json!({"queued":true,"reason":"paused"}));
        }
        if status.as_deref() == Some("running") {
            self.harness
                .enqueue(
                    session_id
                        .as_deref()
                        .ok_or(("session_missing", "active session missing".into()))?,
                    workspace
                        .as_deref()
                        .ok_or(("workspace_missing", "active workspace missing".into()))?,
                    text,
                )
                .map_err(harness_error)?;
            Ok(json!({"queued":true,"reason":"running"}))
        } else {
            self.spawn_followup(task_id, text, command)
        }
    }

    fn cancel(&mut self, command: &CloudCommand) -> Result<Value, (&'static str, String)> {
        let task_id = required_task(command)?;
        let row: Option<(String, Option<i64>)> = self
            .connection
            .query_row(
                "SELECT r.run_id,r.pid FROM tasks t JOIN runs r ON r.run_id=t.active_run_id WHERE t.task_id=?1",
                [task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(db_error)?;
        let (run_id, pid) =
            row.ok_or(("task_not_found", "task is not known to this Runner".into()))?;
        if let Some(pid) = pid {
            self.harness.terminate(pid as u32).map_err(harness_error)?;
        }
        self.connection
            .execute(
                "UPDATE runs SET status='cancelled' WHERE run_id=?1",
                [&run_id],
            )
            .map_err(db_error)?;
        self.emit(
            &format!("{run_id}:cancelled"),
            "run.finished",
            task_id,
            Some(&run_id),
            json!({"task_id":task_id,"run_id":run_id,"status":"cancelled"}),
        )
        .map_err(outbox_error)?;
        self.emit(
            &format!("{task_id}:cancelled"),
            "task.completed",
            task_id,
            Some(&run_id),
            json!({"task_id":task_id,"run_id":run_id,"status":"cancelled"}),
        )
        .map_err(outbox_error)?;
        Ok(json!({"status":"cancelled"}))
    }

    fn pause(&mut self, command: &CloudCommand) -> Result<Value, (&'static str, String)> {
        let task_id = required_task(command)?;
        let changed = self
            .connection
            .execute("UPDATE tasks SET paused=1 WHERE task_id=?1", [task_id])
            .map_err(db_error)?;
        if changed == 0 {
            return Err(("task_not_found", "task is not known to this Runner".into()));
        }
        self.emit(
            &format!("{task_id}:paused"),
            "task.status_changed",
            task_id,
            command.run_id.as_deref(),
            json!({"task_id":task_id,"run_id":command.run_id,"status":"paused"}),
        )
        .map_err(outbox_error)?;
        Ok(json!({"status":"paused","safe_boundary":true}))
    }

    fn resume(&mut self, command: &CloudCommand) -> Result<Value, (&'static str, String)> {
        let task_id = required_task(command)?;
        let launch: String = self
            .connection
            .query_row(
                "SELECT launch_json FROM tasks WHERE task_id=?1",
                [task_id],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        let mut payload: Value =
            serde_json::from_str(&launch).map_err(|error| ("state_corrupt", error.to_string()))?;
        let pending: Option<String> = self
            .connection
            .query_row(
                "SELECT pending_message FROM tasks WHERE task_id=?1",
                [task_id],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        let message = command.payload["message"]
            .as_str()
            .map(str::to_owned)
            .or(pending);
        payload["goal"] = Value::String(message.unwrap_or_else(|| "continue".into()));
        if !command.payload["agent"].is_null() {
            payload["agent"] = command.payload["agent"].clone();
        }
        self.connection
            .execute(
                "UPDATE tasks SET paused=0,pending_message=NULL WHERE task_id=?1",
                [task_id],
            )
            .map_err(db_error)?;
        let mut followup = command.clone();
        followup.payload = payload;
        followup.run_id = Some(new_run_id());
        self.start_run(&followup, followup.run_id.clone())
    }

    fn spawn_followup(
        &mut self,
        task_id: &str,
        text: &str,
        command: &CloudCommand,
    ) -> Result<Value, (&'static str, String)> {
        let launch: String = self
            .connection
            .query_row(
                "SELECT launch_json FROM tasks WHERE task_id=?1",
                [task_id],
                |row| row.get(0),
            )
            .map_err(db_error)?;
        let mut followup = command.clone();
        followup.payload =
            serde_json::from_str(&launch).map_err(|error| ("state_corrupt", error.to_string()))?;
        followup.payload["goal"] = Value::String(text.into());
        followup.run_id = Some(new_run_id());
        self.start_run(&followup, followup.run_id.clone())
    }

    fn prepare_workspace(
        &self,
        task_id: &str,
        run_id: &str,
        spec: &Value,
    ) -> Result<PathBuf, (&'static str, String)> {
        let repository = spec["repository"]
            .as_str()
            .ok_or(("invalid_command", "workspace.repository is required".into()))?;
        let reference = spec["ref"].as_str().unwrap_or("main");
        let destination = self.workspace_root.join(task_id).join(run_id);
        if destination.exists() {
            return Ok(destination);
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        let source = repository.strip_prefix("file://").unwrap_or(repository);
        if Path::new(source).exists() {
            if Path::new(source).join(".git").exists() {
                let status = std::process::Command::new("git")
                    .args(["clone", "--quiet", "--branch", reference, "--", source])
                    .arg(&destination)
                    .status();
                if status.as_ref().is_ok_and(|status| status.success()) {
                    return subdirectory(destination, spec);
                }
            }
            copy_directory(Path::new(source), &destination).map_err(io_error)?;
            return subdirectory(destination, spec);
        }
        let status = std::process::Command::new("git")
            .args(["clone", "--quiet", "--branch", reference, "--", repository])
            .arg(&destination)
            .status()
            .map_err(io_error)?;
        if !status.success() {
            return Err(("workspace_prepare_failed", "git clone failed".into()));
        }
        subdirectory(destination, spec)
    }

    fn apply_harness_event(&mut self, event: HarnessEvent) -> anyhow::Result<()> {
        let (Some(task_id), Some(run_id)) = (event.task_id.as_deref(), event.run_id.as_deref())
        else {
            return Ok(());
        };
        match event.event_type.as_str() {
            "run.handoff_created" => {
                let provider = event.data["provider"].as_str().unwrap_or("claude_code");
                let predecessor = event.data["predecessor_run_id"].as_str();
                let workspace: String = predecessor
                    .and_then(|id| {
                        self.connection
                            .query_row(
                                "SELECT workspace_path FROM runs WHERE run_id=?1",
                                [id],
                                |row| row.get(0),
                            )
                            .optional()
                            .ok()
                            .flatten()
                    })
                    .unwrap_or_else(|| {
                        self.workspace_root
                            .join(task_id)
                            .join(run_id)
                            .to_string_lossy()
                            .into_owned()
                    });
                self.connection.execute(
                    "INSERT OR IGNORE INTO runs(run_id,task_id,provider,status,workspace_path) VALUES(?1,?2,?3,'starting',?4)",
                    params![run_id,task_id,provider,workspace],
                )?;
                self.connection.execute(
                    "UPDATE tasks SET active_run_id=?2 WHERE task_id=?1",
                    params![task_id, run_id],
                )?;
                self.emit(&format!("{run_id}:handoff"), "run.assigned", task_id, Some(run_id), json!({
                    "task_id":task_id,"run_id":run_id,"predecessor_run_id":predecessor,"reason":"handoff","provider":provider
                }))?;
            }
            "run.process_started" => {
                if let Some(session_id) = event.provider_session_ref.as_deref() {
                    self.connection.execute(
                        "UPDATE runs SET session_id=?2,status='running' WHERE run_id=?1",
                        params![run_id, session_id],
                    )?;
                }
            }
            "run.process_exited" => {
                let success = event.data["success"].as_bool().unwrap_or(false);
                self.finish_run(
                    task_id,
                    run_id,
                    event.provider_session_ref.as_deref(),
                    success,
                )?;
            }
            _ => {}
        }
        Ok(())
    }

    fn finish_run(
        &mut self,
        task_id: &str,
        run_id: &str,
        session_id: Option<&str>,
        success: bool,
    ) -> anyhow::Result<()> {
        let current: Option<String> = self
            .connection
            .query_row("SELECT status FROM runs WHERE run_id=?1", [run_id], |row| {
                row.get(0)
            })
            .optional()?;
        if matches!(
            current.as_deref(),
            Some("cancelled" | "succeeded" | "failed")
        ) {
            return Ok(());
        }
        self.project_transcript(task_id, run_id, session_id)?;
        let status = if success { "succeeded" } else { "failed" };
        self.connection.execute(
            "UPDATE runs SET status=?2 WHERE run_id=?1",
            params![run_id, status],
        )?;
        self.emit(
            &format!("{run_id}:finished"),
            "run.finished",
            task_id,
            Some(run_id),
            json!({
                "task_id":task_id,"run_id":run_id,"status":status
            }),
        )?;
        let active: Option<String> = self
            .connection
            .query_row(
                "SELECT active_run_id FROM tasks WHERE task_id=?1",
                [task_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        if active.as_deref() == Some(run_id) {
            self.emit(
                &format!("{task_id}:completed:{run_id}"),
                "task.completed",
                task_id,
                Some(run_id),
                json!({
                    "task_id":task_id,"run_id":run_id,"status":status
                }),
            )?;
        }
        Ok(())
    }

    fn project_transcript(
        &self,
        task_id: &str,
        run_id: &str,
        session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let Some(session_id) = session_id else {
            return Ok(());
        };
        let Some(snapshot) = self
            .harness
            .snapshot(session_id)
            .map_err(anyhow::Error::msg)?
        else {
            return Ok(());
        };
        for (index, message) in snapshot.messages.iter().enumerate() {
            let role = message["type"].as_str().unwrap_or("message");
            let record = public_transcript_record(message.clone());
            self.emit(
                &format!("{run_id}:message:{index}"),
                "message.created",
                task_id,
                Some(run_id),
                json!({
                    "task_id":task_id,"run_id":run_id,"role":role,"record":record
                }),
            )?;
            if let Some(blocks) = message
                .pointer("/message/content")
                .and_then(Value::as_array)
            {
                for (block_index, block) in blocks.iter().enumerate() {
                    let kind = match block["type"].as_str() {
                        Some("tool_use") => Some("tool.started"),
                        Some("tool_result") => Some("tool.finished"),
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        self.emit(
                            &format!("{run_id}:{kind}:{index}:{block_index}"),
                            kind,
                            task_id,
                            Some(run_id),
                            json!({
                                "task_id":task_id,"run_id":run_id,"tool":public_transcript_record(block.clone())
                            }),
                        )?;
                    }
                }
            }
        }
        self.emit(
            &format!("{run_id}:usage"),
            "usage.updated",
            task_id,
            Some(run_id),
            json!({
                "task_id":task_id,"run_id":run_id,"input_tokens":snapshot.input_tokens,
                "output_tokens":snapshot.output_tokens,"cost_usd":snapshot.cost_usd
            }),
        )?;
        Ok(())
    }

    fn drain_event_log(&mut self) -> anyhow::Result<()> {
        let offset: i64 = self
            .connection
            .query_row(
                "SELECT value FROM meta WHERE key='event_offset'",
                [],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let Ok(file) = fs::File::open(&self.event_log) else {
            return Ok(());
        };
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(offset as u64))?;
        let mut next = offset as u64;
        loop {
            let mut line = String::new();
            let read = reader.read_line(&mut line)?;
            if read == 0 {
                break;
            }
            if !line.ends_with('\n') {
                break;
            }
            next += read as u64;
            if let Ok(event) = serde_json::from_str::<HarnessEvent>(&line) {
                self.apply_harness_event(event)?;
            }
        }
        self.connection.execute(
            "INSERT INTO meta(key,value) VALUES('event_offset',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [next as i64],
        )?;
        Ok(())
    }

    fn emit(
        &self,
        source_event_id: &str,
        event_type: &str,
        task_id: &str,
        run_id: Option<&str>,
        data: Value,
    ) -> anyhow::Result<()> {
        debug_assert_eq!(data.get("task_id").and_then(Value::as_str), Some(task_id));
        if contains_local_secret(&data) {
            anyhow::bail!("Cloud event contains a forbidden local process/path field");
        }
        self.outbox.append(RunnerEvent {
            source_event_id: source_event_id.into(),
            sequence: 0,
            event_type: event_type.into(),
            occurred_at: Utc::now(),
            data: if run_id.is_some() { data } else { data },
            schema_version: 1,
        })?;
        Ok(())
    }
}

fn required_task(command: &CloudCommand) -> Result<&str, (&'static str, String)> {
    command
        .task_id
        .as_deref()
        .ok_or(("invalid_command", "command requires task_id".into()))
}

fn new_run_id() -> String {
    format!("run_{}", uuid::Uuid::now_v7().simple())
}

fn subdirectory(destination: PathBuf, spec: &Value) -> Result<PathBuf, (&'static str, String)> {
    let path = spec["subdirectory"]
        .as_str()
        .map(|value| destination.join(value))
        .unwrap_or(destination);
    if path.is_dir() {
        Ok(path)
    } else {
        Err((
            "workspace_prepare_failed",
            "workspace subdirectory does not exist".into(),
        ))
    }
}

fn copy_directory(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn contains_local_secret(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, value)| {
            matches!(
                key.as_str(),
                "pid" | "cwd" | "jsonl_path" | "workspace_path" | "absolute_path"
            ) || contains_local_secret(value)
        }),
        Value::Array(values) => values.iter().any(contains_local_secret),
        _ => false,
    }
}

fn public_transcript_record(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(key, _)| {
                    !matches!(
                        key.as_str(),
                        "pid" | "cwd" | "jsonl_path" | "workspace_path" | "absolute_path"
                    )
                })
                .map(|(key, value)| (key, public_transcript_record(value)))
                .collect(),
        ),
        Value::Array(values) => {
            Value::Array(values.into_iter().map(public_transcript_record).collect())
        }
        other => other,
    }
}

fn db_error(error: rusqlite::Error) -> (&'static str, String) {
    ("runner_state_error", error.to_string())
}
fn io_error(error: std::io::Error) -> (&'static str, String) {
    ("workspace_prepare_failed", error.to_string())
}
fn harness_error(error: String) -> (&'static str, String) {
    ("harness_launch_failed", error)
}
fn outbox_error(error: anyhow::Error) -> (&'static str, String) {
    ("outbox_write_failed", error.to_string())
}
