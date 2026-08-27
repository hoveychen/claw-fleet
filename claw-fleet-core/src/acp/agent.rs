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
}

impl AcpAgent {
    pub fn new(peer: Arc<Peer>, sources: Arc<Vec<Box<dyn AgentSource>>>) -> Self {
        Self {
            peer,
            sources,
            sessions: Mutex::new(HashMap::new()),
            client_caps: Mutex::new(ClientCapabilities::default()),
        }
    }

    /// Route one inbound request. Notifications are handled by the connection
    /// loop, which never calls this.
    pub fn dispatch(&self, method: &str, params: &Value) -> Result<Value, RpcError> {
        match method {
            "initialize" => self.initialize(params),
            "session/new" => self.session_new(params),
            "session/prompt" => self.session_prompt(params),
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

    fn session_prompt(&self, params: &Value) -> Result<Value, RpcError> {
        let req: PromptRequest = serde_json::from_value(params.clone())
            .map_err(|e| RpcError::invalid_params(e.to_string()))?;

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
                jsonrpc::METHOD_CANCEL_REQUEST => {
                    if let Some(sid) = params.get("sessionId").and_then(|v| v.as_str()) {
                        agent.cancel(sid);
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
            let result = agent.dispatch(&method, &params);
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
        let v = a.dispatch("initialize", &json!({"protocolVersion": 1})).unwrap();
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
        let v = a.dispatch("session/new", &json!({"cwd": ws, "mcpServers": []})).unwrap();
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
            .dispatch("session/new", &json!({"cwd": "/somewhere/else", "mcpServers": []}))
            .unwrap_err();
        assert_eq!(err.code, jsonrpc::codes::INVALID_PARAMS);
        assert!(err.message.contains("/somewhere/else"), "the rejection names the bad path");
    }

    #[test]
    fn prompting_an_unknown_session_is_invalid_params_not_a_panic() {
        let a = agent();
        let err = a
            .dispatch(
                "session/prompt",
                &json!({"sessionId": "nope", "prompt": [{"type": "text", "text": "hi"}]}),
            )
            .unwrap_err();
        assert_eq!(err.code, jsonrpc::codes::INVALID_PARAMS);
    }

    #[test]
    fn unknown_methods_report_method_not_found() {
        let a = agent();
        let err = a.dispatch("session/does_not_exist", &json!({})).unwrap_err();
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
        let v = a.dispatch("session/new", &json!({"cwd": ws})).unwrap();
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
}
