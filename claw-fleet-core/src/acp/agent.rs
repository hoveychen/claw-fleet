//! The ACP agent: dispatches ACP methods onto Fleet's spawn/resume machinery.
//!
//! Transport-free by construction — a [`jsonrpc::Peer`] is handed in, so the
//! same agent serves the `fleet acp` stdio subcommand and the `/acp` WebSocket
//! endpoint.
//!
//! # Session identity
//!
//! An ACP `sessionId` is opaque to the client, which lets us keep it **stable**
//! even when the underlying agent renames itself. Claude accepts a pre-minted
//! `--session-id`, but Codex mints its own thread id and reports it back; the
//! Responses surface handled that by swapping the id it had already handed the
//! caller. Here the ACP id is minted once at `session/new` and never changes —
//! [`SessionState::internal_id`] absorbs whatever the source actually used.
//!
//! # Deferred spawn
//!
//! ACP's `session/new` carries no prompt, but Fleet cannot start an agent
//! process without one. So `session/new` only registers the session, and the
//! first `session/prompt` performs the real spawn. Later prompts resume.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::jsonrpc::{self, Peer, RpcError};
use super::types::*;
use crate::agent_source::{AgentSource, ResumeSpec, SpawnSpec};

/// How long a single prompt turn may run before we stop tailing and report
/// back. Generous: an agentic turn legitimately runs for many minutes.
const TURN_TIMEOUT: Duration = Duration::from_secs(60 * 30);

/// How often the turn loop re-reads the transcript.
const TAIL_INTERVAL: Duration = Duration::from_millis(300);

/// Per-session bookkeeping.
struct SessionState {
    /// The session id the agent source actually uses. `None` until the first
    /// prompt spawns a process.
    internal_id: Option<String>,
    /// `claude` or `codex`.
    tool: &'static str,
    model: Option<String>,
    /// Set by `session/cancel` so the turn loop can stop and report
    /// `StopReason::Cancelled`.
    cancelled: bool,
}

pub struct AcpAgent {
    peer: Arc<Peer>,
    sources: Arc<Vec<Box<dyn AgentSource>>>,
    sessions: Mutex<HashMap<String, SessionState>>,
    /// Captured at `initialize`. Decides whether questions can be asked as
    /// elicitations at all (see [`ClientCapabilities::supports_elicitation_form`]).
    client_caps: Mutex<ClientCapabilities>,
    /// JSON-RPC request id → the ACP session its turn is running for, so
    /// `$/cancel_request` (which names a request, not a session) can stop it.
    in_flight: Mutex<HashMap<String, String>>,
    /// Sessions hidden from `session/list` by `session/delete`.
    ///
    /// Deliberately in-memory and not a transcript deletion: the schema only
    /// says delete removes a session "from `session/list`", and a transcript is
    /// the user's data. Actually destroying it is a irreversible operation that
    /// needs explicit sign-off, not something a client can trigger.
    deleted: Mutex<std::collections::HashSet<String>>,
}

/// Unregisters an in-flight request when the turn returns, by any path.
struct InFlightGuard<'a> {
    agent: &'a AcpAgent,
    key: String,
}
impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.agent.in_flight.lock().unwrap().remove(&self.key);
    }
}

impl AcpAgent {
    pub fn new(peer: Arc<Peer>, sources: Arc<Vec<Box<dyn AgentSource>>>) -> Self {
        Self {
            peer,
            sources,
            sessions: Mutex::new(HashMap::new()),
            client_caps: Mutex::new(ClientCapabilities::default()),
            in_flight: Mutex::new(HashMap::new()),
            deleted: Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Route one inbound request. Notifications are handled by the connection
    /// loop, which never calls this.
    ///
    /// `request_id` is the JSON-RPC id of this call, needed so `$/cancel_request`
    /// (which names a *request*, not a session) can find the turn to stop.
    pub fn dispatch(
        &self,
        request_id: &Value,
        method: &str,
        params: &Value,
    ) -> Result<Value, RpcError> {
        match method {
            "initialize" => self.initialize(params),
            "session/new" => self.session_new(params),
            "session/prompt" => self.session_prompt(request_id, params),
            "session/load" => self.session_load(params),
            "session/resume" => self.session_resume(params),
            "session/close" => self.session_close(params),
            "session/delete" => self.session_delete(params),
            "session/list" => self.session_list(params),
            "session/set_mode" => self.set_session_mode(params),
            "session/set_config_option" => self.set_session_config_option(params),
            other => Err(RpcError::method_not_found(other)),
        }
    }

    // ── initialize ──────────────────────────────────────────────────

    fn initialize(&self, params: &Value) -> Result<Value, RpcError> {
        let req: InitializeRequest = serde_json::from_value(params.clone())
            .map_err(|e| RpcError::invalid_params(e.to_string()))?;
        *self.client_caps.lock().unwrap() = req.client_capabilities;

        let resp = InitializeResponse {
            // The schema bumps this only for breaking changes, so answering
            // with our own version (rather than echoing the client's) is what
            // lets an older client notice the mismatch.
            protocol_version: PROTOCOL_VERSION,
            agent_capabilities: AgentCapabilities {
                // Fleet sessions are container processes with a transcript on
                // disk; they outlive the connection.
                load_session: true,
                prompt_capabilities: PromptCapabilities {
                    image: true,
                    audio: false,
                    embedded_context: false,
                },
                session_capabilities: SessionCapabilities {
                    list: true,
                    delete: true,
                    resume: true,
                    close: true,
                    // One container, one workspace.
                    additional_directories: false,
                },
            },
            auth_methods: Vec::new(),
            agent_info: Implementation {
                name: "fleet".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                title: Some("Fleet".to_string()),
            },
        };
        serde_json::to_value(resp).map_err(|e| RpcError::internal(e.to_string()))
    }

    pub fn client_supports_elicitation_form(&self) -> bool {
        self.client_caps.lock().unwrap().supports_elicitation_form()
    }

    // ── session/new ─────────────────────────────────────────────────

    fn session_new(&self, params: &Value) -> Result<Value, RpcError> {
        let req: NewSessionRequest = serde_json::from_value(params.clone())
            .map_err(|e| RpcError::invalid_params(e.to_string()))?;

        // The schema requires `cwd`, but Fleet binds the workspace server-side
        // (one container, one customer) — honouring a client-supplied path is
        // exactly the confinement hole the public surface exists to close. A
        // mismatch is reported rather than silently ignored, so a client cannot
        // believe it is running against its own checkout.
        let workspace = crate::hooks_server::responses::public_workspace();
        if !req.cwd.is_empty() && req.cwd != workspace {
            return Err(RpcError::invalid_params(format!(
                "this agent is bound to {workspace}; it cannot run in {}",
                req.cwd
            )));
        }

        let session_id = uuid::Uuid::new_v4().to_string();
        self.sessions.lock().unwrap().insert(
            session_id.clone(),
            SessionState { internal_id: None, tool: "claude", model: None, cancelled: false },
        );
        serde_json::to_value(NewSessionResponse { session_id })
            .map_err(|e| RpcError::internal(e.to_string()))
    }

    // ── session/prompt ──────────────────────────────────────────────

    fn session_prompt(&self, request_id: &Value, params: &Value) -> Result<Value, RpcError> {
        let req: PromptRequest = serde_json::from_value(params.clone())
            .map_err(|e| RpcError::invalid_params(e.to_string()))?;

        // `$/cancel_request` names a request id, so the turn has to be findable
        // by it for the duration of the call.
        self.in_flight
            .lock()
            .unwrap()
            .insert(request_id.to_string(), req.session_id.clone());
        let _unregister = InFlightGuard { agent: self, key: request_id.to_string() };

        let (internal_id, tool, model) = {
            let sessions = self.sessions.lock().unwrap();
            let st = sessions
                .get(&req.session_id)
                .ok_or_else(|| RpcError::invalid_params("unknown sessionId"))?;
            (st.internal_id.clone(), st.tool, st.model.clone())
        };

        let prompt = req.prompt_text();
        let workspace = crate::hooks_server::responses::public_workspace();

        let internal_id = match internal_id {
            // Follow-up turn: resume the existing process.
            Some(id) => {
                let spec = ResumeSpec {
                    session_id: id.clone(),
                    workspace_path: workspace,
                    prompt,
                    model,
                    ..Default::default()
                };
                crate::agent_source::resume_session(tool, &spec, Box::new(|_| {}))
                    .map_err(RpcError::internal)?;
                id
            }
            // First turn: this is where the process actually starts.
            None => {
                let spec = SpawnSpec {
                    workspace_path: workspace,
                    prompt,
                    model,
                    effort: None,
                    permission_mode: None,
                    // Claude honours this; Codex ignores it and reports its own.
                    session_id: Some(req.session_id.clone()),
                    entrypoint: String::new(),
                    images: Vec::new(),
                };
                let resp =
                    crate::agent_source::spawn_session(tool, &spec).map_err(RpcError::internal)?;
                let actual = resp
                    .session_id
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| req.session_id.clone());
                if let Some(st) = self.sessions.lock().unwrap().get_mut(&req.session_id) {
                    st.internal_id = Some(actual.clone());
                }
                actual
            }
        };

        let stop_reason = self.run_turn(&req.session_id, &internal_id);
        serde_json::to_value(PromptResponse { stop_reason })
            .map_err(|e| RpcError::internal(e.to_string()))
    }

    // ── load · resume · close · delete · list ───────────────────────

    /// `session/load` — reattach to an existing session **and replay it**.
    ///
    /// Fleet can offer this (most ACP agents cannot) because the transcript on
    /// disk outlives the connection. That is also the answer to ACP v1's
    /// "server→client messages are not replayed after a reconnect": the client
    /// reconnects, calls `session/load`, and gets the conversation back.
    fn session_load(&self, params: &Value) -> Result<Value, RpcError> {
        let req: LoadSessionRequest = serde_json::from_value(params.clone())
            .map_err(|e| RpcError::invalid_params(e.to_string()))?;
        self.check_cwd(&req.cwd)?;
        let internal = self.adopt(&req.session_id)?;
        self.replay_history(&req.session_id, &internal);
        Ok(json!({}))
    }

    /// `session/resume` — reattach without replaying.
    fn session_resume(&self, params: &Value) -> Result<Value, RpcError> {
        let req: ResumeSessionRequest = serde_json::from_value(params.clone())
            .map_err(|e| RpcError::invalid_params(e.to_string()))?;
        self.check_cwd(&req.cwd)?;
        self.adopt(&req.session_id)?;
        Ok(json!({}))
    }

    /// `session/close` — the schema requires cancelling in-flight work first,
    /// then releasing resources.
    fn session_close(&self, params: &Value) -> Result<Value, RpcError> {
        let req: SessionIdRequest = serde_json::from_value(params.clone())
            .map_err(|e| RpcError::invalid_params(e.to_string()))?;
        self.cancel(&req.session_id);
        self.sessions.lock().unwrap().remove(&req.session_id);
        Ok(json!({}))
    }

    /// `session/delete` — hide from `session/list`. See [`AcpAgent::deleted`]
    /// for why this does not touch the transcript.
    fn session_delete(&self, params: &Value) -> Result<Value, RpcError> {
        let req: SessionIdRequest = serde_json::from_value(params.clone())
            .map_err(|e| RpcError::invalid_params(e.to_string()))?;
        self.cancel(&req.session_id);
        self.sessions.lock().unwrap().remove(&req.session_id);
        self.deleted.lock().unwrap().insert(req.session_id);
        Ok(json!({}))
    }

    /// `session/list` — the sessions in this container's workspace.
    ///
    /// The workspace filter is a security boundary, not a convenience: a scan
    /// sees every session on the host, and this surface may only ever show the
    /// one workspace the container is bound to.
    fn session_list(&self, params: &Value) -> Result<Value, RpcError> {
        let req: ListSessionsRequest =
            serde_json::from_value(params.clone()).unwrap_or_default();
        if let Some(cwd) = req.cwd.as_deref().filter(|c| !c.is_empty()) {
            self.check_cwd(cwd)?;
        }
        let workspace = crate::hooks_server::responses::public_workspace();
        let deleted = self.deleted.lock().unwrap().clone();

        let sessions = crate::session::scan_all_sources(&self.sources)
            .into_iter()
            .filter(|s| s.workspace_path == workspace)
            .filter(|s| !s.is_subagent)
            .filter(|s| !deleted.contains(&s.id))
            .map(|s| SessionInfo {
                session_id: s.id,
                cwd: workspace.clone(),
                title: s.ai_title,
                updated_at: iso8601_from_unix_ms(s.last_activity_ms),
            })
            .collect();

        serde_json::to_value(ListSessionsResponse { sessions, next_cursor: None })
            .map_err(|e| RpcError::internal(e.to_string()))
    }

    /// `session/set_mode` — Fleet advertises no modes, so any id is unknown.
    ///
    /// Answering with an error rather than a silent `{}` matters: a client that
    /// got `{}` would believe the mode took effect. Fleet *does* have a
    /// `permission_mode` internally (`SpawnSpec::permission_mode`); exposing it
    /// as an ACP mode is a real feature, not a rename, so it is left out until
    /// someone asks for it.
    fn set_session_mode(&self, params: &Value) -> Result<Value, RpcError> {
        let req: SetSessionModeRequest = serde_json::from_value(params.clone())
            .map_err(|e| RpcError::invalid_params(e.to_string()))?;
        Err(RpcError::invalid_params(format!(
            "this agent advertises no session modes; unknown modeId {}",
            req.mode_id
        )))
    }

    /// `session/set_config_option` — same reasoning as [`Self::set_session_mode`].
    fn set_session_config_option(&self, params: &Value) -> Result<Value, RpcError> {
        let req: SetSessionConfigOptionRequest = serde_json::from_value(params.clone())
            .map_err(|e| RpcError::invalid_params(e.to_string()))?;
        Err(RpcError::invalid_params(format!(
            "this agent advertises no config options; unknown configId {}",
            req.config_id
        )))
    }

    /// Reject a client-supplied cwd that is not the bound workspace.
    fn check_cwd(&self, cwd: &str) -> Result<(), RpcError> {
        let workspace = crate::hooks_server::responses::public_workspace();
        if !cwd.is_empty() && cwd != workspace {
            return Err(RpcError::invalid_params(format!(
                "this agent is bound to {workspace}; it cannot run in {cwd}"
            )));
        }
        Ok(())
    }

    /// Register an existing on-disk session under its own id so this connection
    /// can drive it. Errors when no such session exists in the workspace.
    fn adopt(&self, session_id: &str) -> Result<String, RpcError> {
        let workspace = crate::hooks_server::responses::public_workspace();
        let found = crate::session::scan_all_sources(&self.sources)
            .into_iter()
            .find(|s| s.id == session_id && s.workspace_path == workspace)
            .ok_or_else(|| RpcError::invalid_params("unknown sessionId"))?;

        let tool = if found.agent_type.as_deref() == Some("codex") { "codex" } else { "claude" };
        self.sessions.lock().unwrap().insert(
            session_id.to_string(),
            SessionState {
                internal_id: Some(found.id.clone()),
                tool,
                model: None,
                cancelled: false,
            },
        );
        Ok(found.id)
    }

    /// Replay a loaded session's conversation as `user_message_chunk` /
    /// `agent_message_chunk` updates.
    fn replay_history(&self, acp_session_id: &str, internal_id: &str) {
        let Some((path, src)) = self.resolve_source(internal_id) else {
            return;
        };
        let Ok(messages) = src.get_messages(&path) else {
            return;
        };
        for msg in &messages {
            match msg.get("type").and_then(|t| t.as_str()) {
                Some("user") => {
                    if let Some(text) = user_message_text(msg) {
                        self.notify_update(acp_session_id, SessionUpdate::user_text(text));
                    }
                }
                _ => {
                    let text = crate::hooks_server::responses::project_output_text(
                        std::slice::from_ref(msg),
                    );
                    if !text.is_empty() {
                        self.notify_update(acp_session_id, SessionUpdate::agent_text(text));
                    }
                }
            }
        }
    }

    /// Tail the transcript, streaming `agent_message_chunk` updates until the
    /// turn finishes.
    ///
    /// The completion rule mirrors the Responses surface's `effective_status`:
    /// a headless turn parks at `waitingInput`, but that only means "done" once
    /// the turn has actually produced assistant text — on resume the agent
    /// writes bookkeeping lines before it generates, so "any new content" would
    /// end the turn with nothing to show.
    fn run_turn(&self, acp_session_id: &str, internal_id: &str) -> StopReason {
        let deadline = Instant::now() + TURN_TIMEOUT;
        let mut offset = self.transcript_len(internal_id);
        let mut emitted = false;

        loop {
            if self.take_cancelled(acp_session_id) {
                return StopReason::Cancelled;
            }
            if Instant::now() >= deadline {
                return StopReason::MaxTurnRequests;
            }

            if let Some((path, src)) = self.resolve_source(internal_id) {
                if let Ok((msgs, new_offset)) = src.tail_incremental(&path, offset) {
                    offset = new_offset;
                    let delta =
                        crate::hooks_server::responses::project_output_text(&msgs);
                    if !delta.is_empty() {
                        self.notify_update(acp_session_id, SessionUpdate::agent_text(delta));
                        emitted = true;
                    }
                }
                if emitted && self.turn_is_idle(internal_id) {
                    return StopReason::EndTurn;
                }
            }
            std::thread::sleep(TAIL_INTERVAL);
        }
    }

    fn notify_update(&self, session_id: &str, update: SessionUpdate) {
        let params = SessionNotification { session_id: session_id.to_string(), update };
        if let Ok(v) = serde_json::to_value(params) {
            self.peer.notify("session/update", v);
        }
    }

    /// Cancel whatever turn is serving JSON-RPC request `request_id`.
    ///
    /// Silently does nothing for an id with no in-flight turn — the request may
    /// have finished between the client sending the cancel and us reading it,
    /// which is a race, not an error.
    pub fn cancel_request(&self, request_id: &Value) {
        let session = self.in_flight.lock().unwrap().get(&request_id.to_string()).cloned();
        if let Some(sid) = session {
            self.cancel(&sid);
        }
    }

    /// Mark a session cancelled; the turn loop notices on its next tick.
    pub fn cancel(&self, acp_session_id: &str) {
        if let Some(st) = self.sessions.lock().unwrap().get_mut(acp_session_id) {
            st.cancelled = true;
        }
    }

    fn take_cancelled(&self, acp_session_id: &str) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        match sessions.get_mut(acp_session_id) {
            Some(st) if st.cancelled => {
                st.cancelled = false;
                true
            }
            _ => false,
        }
    }

    fn transcript_len(&self, internal_id: &str) -> u64 {
        self.resolve_source(internal_id)
            .and_then(|(path, _)| std::fs::metadata(path).ok())
            .map(|m| m.len())
            .unwrap_or(0)
    }

    fn resolve_source(&self, internal_id: &str) -> Option<(String, &dyn AgentSource)> {
        let sessions = crate::session::scan_all_sources(&self.sources);
        let s = sessions.iter().find(|s| s.id == internal_id)?;
        let src = crate::agent_source::find_source_for_path(&self.sources, &s.jsonl_path)?;
        Some((s.jsonl_path.clone(), src))
    }

    /// True once the source reports the session is parked awaiting input.
    fn turn_is_idle(&self, internal_id: &str) -> bool {
        let sessions = crate::session::scan_all_sources(&self.sources);
        let Some(s) = sessions.iter().find(|s| s.id == internal_id) else {
            return false;
        };
        let status = serde_json::to_value(&s.status)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        matches!(status.as_str(), "waitingInput" | "done" | "idle" | "completed" | "succeeded")
    }
}

/// Unix millis → ISO 8601, the format `SessionInfo.updatedAt` requires.
fn iso8601_from_unix_ms(ms: u64) -> Option<String> {
    if ms == 0 {
        return None;
    }
    chrono::DateTime::from_timestamp_millis(ms as i64).map(|dt| dt.to_rfc3339())
}

/// Text of a transcript `user` record.
///
/// Claude writes `message.content` as either a bare string or an array of
/// blocks, so both shapes have to be read — a replay that only handled one
/// would drop half the user's side of the conversation.
fn user_message_text(msg: &Value) -> Option<String> {
    let content = msg.get("message")?.get("content")?;
    if let Some(s) = content.as_str() {
        return (!s.is_empty()).then(|| s.to_string());
    }
    let joined: Vec<String> = content
        .as_array()?
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()).map(String::from))
        .collect();
    (!joined.is_empty()).then(|| joined.join("\n"))
}

/// Handle one inbound frame against an agent. Returns the frame to write back,
/// or `None` for a notification (JSON-RPC 2.0 §4.1 forbids replying to those).
///
/// Split out from the connection loops so stdio and WebSocket share it.
pub fn handle_frame(agent: &AcpAgent, line: &str) -> Option<String> {
    match jsonrpc::parse(line) {
        Err((id, err)) => Some(jsonrpc::response_err(id.as_ref(), &err)),
        Ok(jsonrpc::Incoming::Response { id, result }) => {
            agent.peer.resolve(&id, result);
            None
        }
        Ok(jsonrpc::Incoming::Notification { method, params }) => {
            match method.as_str() {
                // Keep-alive from real clients; answering it would violate
                // "MUST NOT reply to a Notification".
                jsonrpc::METHOD_PING => {}
                // Cancels one in-flight *request*, named by `requestId` — not
                // a session. Resolving it through the in-flight table is what
                // makes it different from `session/cancel`.
                jsonrpc::METHOD_CANCEL_REQUEST => {
                    if let Some(rid) = params.get("requestId") {
                        agent.cancel_request(rid);
                    }
                }
                "session/cancel" => {
                    if let Some(sid) = params.get("sessionId").and_then(|v| v.as_str()) {
                        agent.cancel(sid);
                    }
                }
                _ => {}
            }
            None
        }
        Ok(jsonrpc::Incoming::Request { id, method, params }) => {
            let result = agent.dispatch(&id, &method, &params);
            Some(match result {
                Ok(v) => jsonrpc::response_ok(&id, v),
                Err(e) => jsonrpc::response_err(Some(&id), &e),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::jsonrpc::Sink;

    struct NullSink;
    impl Sink for NullSink {
        fn send(&self, _frame: &str) -> bool {
            true
        }
    }

    fn agent() -> AcpAgent {
        AcpAgent::new(Arc::new(Peer::new(Box::new(NullSink))), Arc::new(Vec::new()))
    }

    #[test]
    fn initialize_advertises_the_capabilities_fleet_actually_has() {
        let a = agent();
        let v = a.dispatch(&json!(1), "initialize", &json!({"protocolVersion": 1})).unwrap();
        assert_eq!(v["protocolVersion"], PROTOCOL_VERSION);
        // Sessions outlive connections — this is the capability that lets a
        // reconnecting client pick a run back up.
        assert_eq!(v["agentCapabilities"]["loadSession"], true);
        assert_eq!(v["agentCapabilities"]["sessionCapabilities"]["resume"], true);
        // One container, one workspace.
        assert_eq!(v["agentCapabilities"]["sessionCapabilities"]["additionalDirectories"], false);
        assert_eq!(v["agentInfo"]["name"], "fleet");
    }

    #[test]
    fn initialize_records_client_elicitation_support() {
        let a = agent();
        assert!(!a.client_supports_elicitation_form());
        a.dispatch(
            &json!(1),
            "initialize",
            &json!({"protocolVersion": 1, "clientCapabilities": {"elicitation": {"form": {}}}}),
        )
        .unwrap();
        assert!(a.client_supports_elicitation_form());
    }

    #[test]
    fn session_new_mints_an_id_and_defers_the_spawn() {
        let a = agent();
        let ws = crate::hooks_server::responses::public_workspace();
        let v = a.dispatch(&json!(1), "session/new", &json!({"cwd": ws, "mcpServers": []})).unwrap();
        let sid = v["sessionId"].as_str().expect("a session id");
        assert!(!sid.is_empty());
        // Deferred: no process exists yet, so no internal id.
        let sessions = a.sessions.lock().unwrap();
        assert!(sessions.get(sid).unwrap().internal_id.is_none());
    }

    #[test]
    fn session_new_rejects_a_foreign_cwd_instead_of_silently_rebinding() {
        // Honouring a client-supplied cwd would defeat the confinement the
        // public surface exists to provide; silently ignoring it would let the
        // client believe it is running against its own checkout.
        let a = agent();
        let err = a
            .dispatch(&json!(1), "session/new", &json!({"cwd": "/somewhere/else", "mcpServers": []}))
            .unwrap_err();
        assert_eq!(err.code, jsonrpc::codes::INVALID_PARAMS);
        assert!(err.message.contains("/somewhere/else"), "the rejection names the bad path");
    }

    #[test]
    fn prompting_an_unknown_session_is_invalid_params_not_a_panic() {
        let a = agent();
        let err = a
            .dispatch(
                &json!(1),
                "session/prompt",
                &json!({"sessionId": "nope", "prompt": [{"type": "text", "text": "hi"}]}),
            )
            .unwrap_err();
        assert_eq!(err.code, jsonrpc::codes::INVALID_PARAMS);
    }

    #[test]
    fn unknown_methods_report_method_not_found() {
        let a = agent();
        let err = a.dispatch(&json!(1), "session/does_not_exist", &json!({})).unwrap_err();
        assert_eq!(err.code, jsonrpc::codes::METHOD_NOT_FOUND);
    }

    #[test]
    fn notifications_are_never_answered() {
        let a = agent();
        assert!(handle_frame(&a, r#"{"jsonrpc":"2.0","method":"$/ping"}"#).is_none());
        assert!(handle_frame(
            &a,
            r#"{"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":"s"}}"#
        )
        .is_none());
        // Even an unknown notification stays silent — replying would break
        // JSON-RPC 2.0 §4.1.
        assert!(handle_frame(&a, r#"{"jsonrpc":"2.0","method":"who/knows"}"#).is_none());
    }

    #[test]
    fn a_cancel_notification_marks_the_session_and_is_consumed_once() {
        let a = agent();
        let ws = crate::hooks_server::responses::public_workspace();
        let v = a.dispatch(&json!(1), "session/new", &json!({"cwd": ws})).unwrap();
        let sid = v["sessionId"].as_str().unwrap().to_string();

        let frame = format!(
            r#"{{"jsonrpc":"2.0","method":"session/cancel","params":{{"sessionId":"{sid}"}}}}"#
        );
        assert!(handle_frame(&a, &frame).is_none());
        assert!(a.take_cancelled(&sid), "the turn loop sees the cancel");
        assert!(!a.take_cancelled(&sid), "and it is consumed, not sticky");
    }

    #[test]
    fn a_broken_frame_is_answered_with_an_error_not_dropped() {
        let a = agent();
        let out = handle_frame(&a, "{ not json").expect("a parse error is answerable");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], jsonrpc::codes::PARSE_ERROR);
        assert_eq!(v["id"], Value::Null);
    }

    #[test]
    fn inbound_responses_route_to_the_peer_and_produce_no_frame() {
        let a = agent();
        // Nobody is waiting on id 1, so this is a no-op — but it must not be
        // mistaken for a request and answered.
        assert!(handle_frame(&a, r#"{"jsonrpc":"2.0","id":1,"result":{}}"#).is_none());
    }

    // ── P3: the rest of the session lifecycle ───────────────────────

    #[test]
    fn cancel_request_names_a_request_id_not_a_session_id() {
        // `$/cancel_request` carries `requestId`; `session/cancel` carries
        // `sessionId`. Reading the wrong field would make the cancel a no-op.
        let a = agent();
        let ws = crate::hooks_server::responses::public_workspace();
        let v = a.dispatch(&json!(1), "session/new", &json!({"cwd": ws})).unwrap();
        let sid = v["sessionId"].as_str().unwrap().to_string();

        // Pretend request 42 is the in-flight prompt turn for this session.
        a.in_flight.lock().unwrap().insert(json!(42).to_string(), sid.clone());

        assert!(handle_frame(
            &a,
            r#"{"jsonrpc":"2.0","method":"$/cancel_request","params":{"requestId":42}}"#
        )
        .is_none());
        assert!(a.take_cancelled(&sid), "the turn behind request 42 is cancelled");
    }

    #[test]
    fn cancelling_an_unknown_request_id_is_a_race_not_an_error() {
        let a = agent();
        assert!(handle_frame(
            &a,
            r#"{"jsonrpc":"2.0","method":"$/cancel_request","params":{"requestId":999}}"#
        )
        .is_none());
    }

    #[test]
    fn load_and_resume_reject_a_foreign_cwd() {
        let a = agent();
        for method in ["session/load", "session/resume"] {
            let err = a
                .dispatch(&json!(1), method, &json!({"sessionId": "s", "cwd": "/elsewhere"}))
                .unwrap_err();
            assert_eq!(err.code, jsonrpc::codes::INVALID_PARAMS, "{method} must check cwd");
            assert!(err.message.contains("/elsewhere"));
        }
    }

    #[test]
    fn load_and_resume_of_an_unknown_session_are_invalid_params() {
        let a = agent();
        let ws = crate::hooks_server::responses::public_workspace();
        for method in ["session/load", "session/resume"] {
            let err = a
                .dispatch(&json!(1), method, &json!({"sessionId": "nope", "cwd": ws}))
                .unwrap_err();
            assert_eq!(err.code, jsonrpc::codes::INVALID_PARAMS);
        }
    }

    #[test]
    fn close_cancels_the_turn_and_drops_the_session() {
        let a = agent();
        let ws = crate::hooks_server::responses::public_workspace();
        let v = a.dispatch(&json!(1), "session/new", &json!({"cwd": ws})).unwrap();
        let sid = v["sessionId"].as_str().unwrap().to_string();

        a.dispatch(&json!(2), "session/close", &json!({"sessionId": sid})).unwrap();
        assert!(!a.sessions.lock().unwrap().contains_key(&sid), "resources are freed");
        // Prompting it again is now an unknown session, not a silent no-op.
        let err = a
            .dispatch(&json!(3), "session/prompt", &json!({"sessionId": sid, "prompt": []}))
            .unwrap_err();
        assert_eq!(err.code, jsonrpc::codes::INVALID_PARAMS);
    }

    #[test]
    fn delete_hides_from_list_without_destroying_the_transcript() {
        // The schema only says delete removes a session "from session/list".
        // A transcript is the user's data; destroying it is not something a
        // client gets to trigger.
        let a = agent();
        let ws = crate::hooks_server::responses::public_workspace();
        let v = a.dispatch(&json!(1), "session/new", &json!({"cwd": ws})).unwrap();
        let sid = v["sessionId"].as_str().unwrap().to_string();

        a.dispatch(&json!(2), "session/delete", &json!({"sessionId": sid})).unwrap();
        assert!(a.deleted.lock().unwrap().contains(&sid));

        let listed = a.dispatch(&json!(3), "session/list", &json!({})).unwrap();
        let ids: Vec<&str> = listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s["sessionId"].as_str())
            .collect();
        assert!(!ids.contains(&sid.as_str()), "a deleted session is hidden from list");
    }

    #[test]
    fn list_tolerates_missing_params_and_rejects_a_foreign_cwd() {
        let a = agent();
        // No params at all is legal — every field of ListSessionsRequest is
        // optional.
        let v = a.dispatch(&json!(1), "session/list", &json!(null)).unwrap();
        assert!(v["sessions"].is_array());

        let err = a
            .dispatch(&json!(2), "session/list", &json!({"cwd": "/elsewhere"}))
            .unwrap_err();
        assert_eq!(err.code, jsonrpc::codes::INVALID_PARAMS);
    }

    #[test]
    fn set_mode_and_set_config_report_unknown_ids_rather_than_pretending() {
        // Answering `{}` would let a client believe the mode took effect.
        let a = agent();
        let err = a
            .dispatch(&json!(1), "session/set_mode", &json!({"sessionId": "s", "modeId": "plan"}))
            .unwrap_err();
        assert_eq!(err.code, jsonrpc::codes::INVALID_PARAMS);
        assert!(err.message.contains("plan"), "the rejection names the id it did not know");

        let err = a
            .dispatch(
                &json!(2),
                "session/set_config_option",
                &json!({"sessionId": "s", "configId": "effort"}),
            )
            .unwrap_err();
        assert_eq!(err.code, jsonrpc::codes::INVALID_PARAMS);
        assert!(err.message.contains("effort"));
    }

    #[test]
    fn user_message_text_reads_both_transcript_shapes() {
        // Claude writes `message.content` as a bare string or as blocks;
        // handling only one would drop half the replayed conversation.
        assert_eq!(
            user_message_text(&json!({"message": {"content": "plain"}})).as_deref(),
            Some("plain")
        );
        assert_eq!(
            user_message_text(&json!({"message": {"content": [
                {"type": "text", "text": "a"},
                {"type": "tool_result", "content": "ignored"},
                {"type": "text", "text": "b"}
            ]}}))
            .as_deref(),
            Some("a\nb")
        );
        assert_eq!(user_message_text(&json!({"message": {"content": []}})), None);
        assert_eq!(user_message_text(&json!({"message": {"content": ""}})), None);
        assert_eq!(user_message_text(&json!({"nothing": true})), None);
    }

    #[test]
    fn updated_at_is_iso8601_and_absent_when_unknown() {
        assert_eq!(iso8601_from_unix_ms(0), None, "0 means unknown, not the epoch");
        let s = iso8601_from_unix_ms(1_700_000_000_000).expect("a timestamp");
        assert!(s.starts_with("2023-11-"), "unexpected rendering: {s}");
        assert!(s.contains('T'), "ISO 8601 needs the date/time separator");
    }

    #[test]
    fn in_flight_registration_is_released_when_the_turn_returns() {
        // The guard must fire on the error path too, or a failed prompt leaks
        // its entry and a later cancel hits the wrong session.
        let a = agent();
        let _ = a.dispatch(&json!(7), "session/prompt", &json!({"sessionId": "nope", "prompt": []}));
        assert!(a.in_flight.lock().unwrap().is_empty());
    }
}
