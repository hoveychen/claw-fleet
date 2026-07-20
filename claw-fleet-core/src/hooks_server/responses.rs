//! OpenAI **Responses API**-compatible public surface for Fleet Cloud (v2).
//!
//! This is the ONLY API external customers talk to. It maps the OpenAI
//! Responses shape onto Fleet's existing spawn / tail / decision machinery and
//! projects everything back into clean `response` objects — no raw host paths,
//! no `SessionInfo`, no internal route contract. Compared with v1's
//! scoped-token-over-raw-`fleet serve` whitelist, two properties hold *by
//! construction*:
//!
//! - **Confinement.** The request has no `workspace_path`; the workspace is
//!   bound server-side to the container ([`public_workspace`]). A customer
//!   cannot point an agent at the credential directory.
//! - **Projection.** Responses are built from these types only, so `pid`,
//!   `jsonlPath` and host paths never leave the container.
//!
//! Field names are **snake_case** to match OpenAI (`input_tokens`,
//! `output_text`, `previous_response_id`, `call_id`, `created_at`), so a
//! stock OpenAI SDK pointed at `<host>/v1` with `api_key=$FLEET_PUBLIC_TOKEN`
//! works unmodified.
//!
//! Fleet's six decision-card types map to OpenAI `function_call` output items
//! (`fleet_guard`, `fleet_elicitation`, …); the caller answers with a
//! `function_call_output` input item, which routes to the matching decision
//! `/*/respond`. See [`decision_function_name`].
//!
//! P1 defines the wire contract, id/workspace/status helpers and the dispatch
//! skeleton. P2+ wire the handlers to real spawn/projection.

use serde::{Deserialize, Serialize};

// ─────────────────────────── Request types ───────────────────────────

/// `POST /v1/responses` body.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateResponseRequest {
    /// Fleet model id (e.g. `claude-opus-4-8`, `gpt-5.6-sol`). Optional; the
    /// container's default is used when absent.
    #[serde(default)]
    pub model: Option<String>,
    /// The task input: a plain string or an array of typed items.
    pub input: ResponseInput,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub background: bool,
    /// Continue a prior response (multi-turn / follow-up / decision answer).
    #[serde(default)]
    pub previous_response_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ResponseInput {
    Text(String),
    Items(Vec<InputItem>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputItem {
    Message {
        #[serde(default = "default_user_role")]
        role: String,
        content: InputContent,
    },
    /// The caller's answer to a `function_call` we emitted (a decision card).
    FunctionCallOutput { call_id: String, output: String },
    /// Tolerate unknown item types (forward-compat with OpenAI clients).
    #[serde(other)]
    Other,
}

fn default_user_role() -> String {
    "user".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum InputContent {
    Text(String),
    Parts(Vec<InputPart>),
}

#[derive(Debug, Clone, Deserialize)]
pub struct InputPart {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub text: Option<String>,
}

impl CreateResponseRequest {
    /// Flatten the input into the prompt text handed to the agent. Message
    /// items contribute their text; `function_call_output` items are handled
    /// separately via [`Self::function_call_outputs`].
    pub fn prompt_text(&self) -> String {
        match &self.input {
            ResponseInput::Text(s) => s.clone(),
            ResponseInput::Items(items) => {
                let mut out: Vec<String> = Vec::new();
                for it in items {
                    if let InputItem::Message { content, .. } = it {
                        match content {
                            InputContent::Text(s) => out.push(s.clone()),
                            InputContent::Parts(parts) => {
                                for p in parts {
                                    if let Some(t) = &p.text {
                                        out.push(t.clone());
                                    }
                                }
                            }
                        }
                    }
                }
                out.join("\n")
            }
        }
    }

    /// `(call_id, output)` pairs the caller submitted as decision answers.
    pub fn function_call_outputs(&self) -> Vec<(String, String)> {
        match &self.input {
            ResponseInput::Text(_) => Vec::new(),
            ResponseInput::Items(items) => items
                .iter()
                .filter_map(|it| match it {
                    InputItem::FunctionCallOutput { call_id, output } => {
                        Some((call_id.clone(), output.clone()))
                    }
                    _ => None,
                })
                .collect(),
        }
    }
}

// ─────────────────────────── Response types ──────────────────────────

/// The `response` object returned by create / retrieve.
#[derive(Debug, Clone, Serialize)]
pub struct ResponseObject {
    pub id: String,
    pub object: &'static str, // always "response"
    pub created_at: i64,
    pub status: ResponseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
    pub output: Vec<OutputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
}

impl ResponseObject {
    pub fn new(id: String, status: ResponseStatus) -> Self {
        Self {
            id,
            object: "response",
            created_at: 0,
            status,
            model: None,
            previous_response_id: None,
            output: Vec::new(),
            usage: None,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Queued,
    InProgress,
    Completed,
    Failed,
    Cancelled,
    Incomplete,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputItem {
    Message {
        id: String,
        role: String,
        content: Vec<OutputContent>,
    },
    /// A pending Fleet decision card, surfaced as an OpenAI tool call.
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        /// JSON-encoded arguments (the decision card payload).
        arguments: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputContent {
    OutputText { text: String },
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseError {
    pub code: String,
    pub message: String,
}

// ─────────────────────────── Helpers ─────────────────────────────────

/// Mint a new opaque response id and its backing session UUID. The wire id is
/// `resp_<uuid>`; the UUID is pre-assigned to the spawn via `--session-id` so
/// the response is correlatable synchronously. The internal session id is
/// never exposed except wrapped in the opaque `resp_` form.
pub fn new_resp_id() -> (String, String) {
    let uuid = uuid::Uuid::new_v4().to_string();
    (format!("resp_{uuid}"), uuid)
}

/// `resp_<uuid>` → `<uuid>` (the internal session id). `None` if malformed.
pub fn session_id_from_resp(resp_id: &str) -> Option<&str> {
    resp_id.strip_prefix("resp_").filter(|s| !s.is_empty())
}

/// `<session id>` → `resp_<session id>`.
pub fn to_resp_id(session_id: &str) -> String {
    format!("resp_{session_id}")
}

/// The workspace an external response runs in. One customer per container, so
/// this is the container's single mounted workspace — bound server-side, never
/// taken from the request. `FLEET_PUBLIC_WORKSPACE` overrides the `/workspace`
/// default.
pub fn public_workspace() -> String {
    std::env::var("FLEET_PUBLIC_WORKSPACE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/workspace".to_string())
}

/// Map a Fleet session status string to an OpenAI response status.
pub fn map_status(fleet_status: &str) -> ResponseStatus {
    match fleet_status {
        "queued" | "starting" => ResponseStatus::Queued,
        "running" | "waitingInput" | "thinking" => ResponseStatus::InProgress,
        "done" | "completed" | "succeeded" | "idle" => ResponseStatus::Completed,
        "failed" | "error" => ResponseStatus::Failed,
        "cancelled" | "canceled" | "stopped" | "killed" => ResponseStatus::Cancelled,
        _ => ResponseStatus::InProgress,
    }
}

/// The OpenAI `function_call.name` a Fleet decision-card kind maps to.
pub fn decision_function_name(kind: &str) -> &'static str {
    match kind {
        "guard" => "fleet_guard",
        "elicitation" => "fleet_elicitation",
        "fleet-ask" | "fleet_ask" => "fleet_ask",
        "plan-approval" | "plan_approval" => "fleet_plan_approval",
        "permission-prompt" | "permission_prompt" => "fleet_permission",
        "a2ui" | "a2ui-render" => "fleet_a2ui",
        _ => "fleet_decision",
    }
}

/// The Fleet decision kind a `function_call.name` maps back to (for routing a
/// `function_call_output` to the right `/*/respond`). Inverse of
/// [`decision_function_name`].
pub fn decision_kind_from_function(name: &str) -> Option<&'static str> {
    match name {
        "fleet_guard" => Some("guard"),
        "fleet_elicitation" => Some("elicitation"),
        "fleet_ask" => Some("fleet-ask"),
        "fleet_plan_approval" => Some("plan-approval"),
        "fleet_permission" => Some("permission-prompt"),
        "fleet_a2ui" => Some("a2ui"),
        _ => None,
    }
}

// ─────────────────────────── Dispatch skeleton ───────────────────────

/// Parsed `/v1/...` route target.
#[derive(Debug, PartialEq, Eq)]
pub enum V1Route {
    /// `POST /v1/responses`
    CreateResponse,
    /// `GET /v1/responses/{id}`
    GetResponse(String),
    /// `POST /v1/responses/{id}/cancel`
    CancelResponse(String),
    /// `GET /v1/responses/{id}/files`
    ListFiles(String),
    /// `GET /v1/files/{id}/content`
    FileContent(String),
    /// Unknown `/v1/...` path.
    NotFound,
}

/// Route a `/v1/...` path (method + path) to a [`V1Route`]. Pure; unit-tested.
pub fn parse_v1_route(method: &str, path: &str) -> V1Route {
    let rest = match path.strip_prefix("/v1/") {
        Some(r) => r,
        None => return V1Route::NotFound,
    };
    let segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    match segs.as_slice() {
        ["responses"] if method == "POST" => V1Route::CreateResponse,
        ["responses", id] if method == "GET" => V1Route::GetResponse((*id).to_string()),
        ["responses", id, "cancel"] if method == "POST" => {
            V1Route::CancelResponse((*id).to_string())
        }
        ["responses", id, "files"] if method == "GET" => V1Route::ListFiles((*id).to_string()),
        ["files", id, "content"] if method == "GET" => V1Route::FileContent((*id).to_string()),
        _ => V1Route::NotFound,
    }
}

/// Pick the agent tool from the requested model id (`gpt-*`/`codex` → codex,
/// else claude). Keeps the OpenAI `model` field meaningful without exposing
/// Fleet's tool concept.
pub fn tool_for_model(model: Option<&str>) -> &'static str {
    match model {
        Some(m) if m.starts_with("gpt") || m.starts_with("codex") => "codex",
        _ => "claude",
    }
}

/// Concatenate assistant text out of a transcript, handling both Claude
/// (`{type:"assistant", message.content[].text}`) and Codex
/// (`{type:"response_item", payload.content[].output_text}`) shapes. Pure;
/// unit-tested.
pub fn project_output_text(messages: &[serde_json::Value]) -> String {
    let mut out = String::new();
    let mut push = |t: &str| {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(t);
    };
    for msg in messages {
        let ty = msg.get("type").and_then(|t| t.as_str());
        if ty == Some("assistant") {
            if let Some(content) = msg
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                for b in content {
                    if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                        if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                            push(t);
                        }
                    }
                }
            }
        } else if ty == Some("response_item") {
            let payload = msg.get("payload");
            let is_assistant = payload
                .and_then(|p| p.get("role"))
                .and_then(|r| r.as_str())
                == Some("assistant");
            if is_assistant {
                if let Some(content) = payload
                    .and_then(|p| p.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for b in content {
                        if b.get("type").and_then(|t| t.as_str()) == Some("output_text") {
                            if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                                push(t);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// Project a live Fleet session (found by internal id) into a `response`
/// object: status, usage from token totals, and the assistant output text.
/// `None` if no session with that id exists yet.
fn build_response(ctx: &super::ServeCtx, session_id: &str) -> Option<ResponseObject> {
    let sessions = crate::session::scan_all_sources(ctx.sources);
    let s = sessions.iter().find(|s| s.id == session_id)?;
    let status_str = serde_json::to_value(&s.status)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    let mut resp = ResponseObject::new(to_resp_id(session_id), map_status(&status_str));
    resp.model = s.model.clone();
    resp.usage = Some(Usage {
        input_tokens: s.total_input_tokens,
        output_tokens: s.total_output_tokens,
        total_tokens: s.total_input_tokens.saturating_add(s.total_output_tokens),
    });
    if let Some(src) = crate::agent_source::find_source_for_path(ctx.sources, &s.jsonl_path) {
        if let Ok(messages) = src.get_messages(&s.jsonl_path) {
            let text = project_output_text(&messages);
            if !text.is_empty() {
                resp.output.push(OutputItem::Message {
                    id: format!("msg_{session_id}"),
                    role: "assistant".to_string(),
                    content: vec![OutputContent::OutputText { text }],
                });
            }
        }
    }
    Some(resp)
}

fn respond_value(request: tiny_http::Request, status: u16, body: &serde_json::Value, json_header: tiny_http::Header) {
    let _ = request.respond(
        tiny_http::Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(json_header),
    );
}

fn respond_error(request: tiny_http::Request, status: u16, code: &str, message: impl AsRef<str>, json_header: tiny_http::Header) {
    let body = serde_json::json!({"error": {"code": code, "message": message.as_ref()}});
    respond_value(request, status, &body, json_header);
}

/// Entry point for every `/v1/...` request.
pub(crate) fn dispatch(
    ctx: &super::ServeCtx,
    request: tiny_http::Request,
    _query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let method = request.method().as_str().to_string();
    match parse_v1_route(&method, path) {
        V1Route::CreateResponse => create_response(ctx, request, json_header),
        V1Route::GetResponse(id) => get_response(ctx, request, json_header, &id),
        V1Route::CancelResponse(id) => cancel_response(ctx, request, json_header, &id),
        // Files land in P5.
        V1Route::ListFiles(_) | V1Route::FileContent(_) => {
            respond_error(request, 501, "not_implemented", "files endpoints arrive in P5", json_header)
        }
        V1Route::NotFound => {
            respond_error(request, 404, "not_found", "unknown /v1 route", json_header)
        }
    }
}

/// `POST /v1/responses` — spawn a new run, or (with `previous_response_id`)
/// resume one with a follow-up. Answering a decision via `function_call_output`
/// is wired in P4.
fn create_response(_ctx: &super::ServeCtx, mut request: tiny_http::Request, json_header: tiny_http::Header) {
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
    let req: CreateResponseRequest = match serde_json::from_str(&buf) {
        Ok(r) => r,
        Err(e) => return respond_error(request, 400, "invalid_request", e.to_string(), json_header),
    };
    let model = req.model.clone();
    let tool = tool_for_model(model.as_deref());

    if let Some(prev) = req.previous_response_id.clone() {
        let Some(sid) = session_id_from_resp(&prev).map(|s| s.to_string()) else {
            return respond_error(request, 400, "invalid_request", "malformed previous_response_id", json_header);
        };
        let spec = crate::agent_source::ResumeSpec {
            session_id: sid.clone(),
            workspace_path: public_workspace(),
            prompt: req.prompt_text(),
            model,
            ..Default::default()
        };
        match crate::agent_source::resume_session(tool, &spec, Box::new(|_| {})) {
            Ok(()) => {
                let mut r = ResponseObject::new(to_resp_id(&sid), ResponseStatus::InProgress);
                r.previous_response_id = Some(prev);
                respond_value(request, 200, &serde_json::to_value(&r).unwrap_or_default(), json_header)
            }
            Err(e) => respond_error(request, 500, "resume_failed", e, json_header),
        }
    } else {
        let (resp_id, uuid) = new_resp_id();
        let spec = crate::agent_source::SpawnSpec {
            workspace_path: public_workspace(),
            prompt: req.prompt_text(),
            model: model.clone(),
            effort: None,
            permission_mode: None,
            session_id: Some(uuid),
            entrypoint: String::new(),
        };
        match crate::agent_source::spawn_session(tool, &spec) {
            Ok(_) => {
                let mut r = ResponseObject::new(resp_id, ResponseStatus::Queued);
                r.model = model;
                respond_value(request, 200, &serde_json::to_value(&r).unwrap_or_default(), json_header)
            }
            Err(e) => respond_error(request, 500, "spawn_failed", e, json_header),
        }
    }
}

/// `GET /v1/responses/{id}` — project current session state.
fn get_response(ctx: &super::ServeCtx, request: tiny_http::Request, json_header: tiny_http::Header, resp_id: &str) {
    let Some(sid) = session_id_from_resp(resp_id) else {
        return respond_error(request, 404, "not_found", "malformed response id", json_header);
    };
    match build_response(ctx, sid) {
        Some(r) => respond_value(request, 200, &serde_json::to_value(&r).unwrap_or_default(), json_header),
        None => respond_error(request, 404, "not_found", "no such response", json_header),
    }
}

/// `POST /v1/responses/{id}/cancel` — interrupt the run, best-effort.
fn cancel_response(ctx: &super::ServeCtx, request: tiny_http::Request, json_header: tiny_http::Header, resp_id: &str) {
    let Some(sid) = session_id_from_resp(resp_id) else {
        return respond_error(request, 404, "not_found", "malformed response id", json_header);
    };
    let sessions = crate::session::scan_all_sources(ctx.sources);
    let Some(s) = sessions.iter().find(|s| s.id == sid) else {
        return respond_error(request, 404, "not_found", "no such response", json_header);
    };
    if let Some(pid) = s.pid {
        let _ = crate::session::interrupt_pid_impl(pid);
    }
    let mut r = build_response(ctx, sid)
        .unwrap_or_else(|| ResponseObject::new(to_resp_id(sid), ResponseStatus::Cancelled));
    r.status = ResponseStatus::Cancelled;
    respond_value(request, 200, &serde_json::to_value(&r).unwrap_or_default(), json_header);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resp_id_round_trips() {
        let (resp, uuid) = new_resp_id();
        assert!(resp.starts_with("resp_"));
        assert_eq!(session_id_from_resp(&resp), Some(uuid.as_str()));
        assert_eq!(to_resp_id(&uuid), resp);
        assert_eq!(session_id_from_resp("resp_"), None);
        assert_eq!(session_id_from_resp("nope"), None);
    }

    #[test]
    fn public_workspace_defaults_and_overrides() {
        std::env::remove_var("FLEET_PUBLIC_WORKSPACE");
        assert_eq!(public_workspace(), "/workspace");
    }

    #[test]
    fn status_mapping() {
        assert_eq!(map_status("running"), ResponseStatus::InProgress);
        assert_eq!(map_status("done"), ResponseStatus::Completed);
        assert_eq!(map_status("failed"), ResponseStatus::Failed);
        assert_eq!(map_status("stopped"), ResponseStatus::Cancelled);
        assert_eq!(map_status("queued"), ResponseStatus::Queued);
    }

    #[test]
    fn decision_name_round_trips_all_six() {
        for (kind, name) in [
            ("guard", "fleet_guard"),
            ("elicitation", "fleet_elicitation"),
            ("fleet-ask", "fleet_ask"),
            ("plan-approval", "fleet_plan_approval"),
            ("permission-prompt", "fleet_permission"),
            ("a2ui", "fleet_a2ui"),
        ] {
            assert_eq!(decision_function_name(kind), name);
            assert_eq!(decision_kind_from_function(name), Some(kind));
        }
    }

    #[test]
    fn parse_routes() {
        assert_eq!(parse_v1_route("POST", "/v1/responses"), V1Route::CreateResponse);
        assert_eq!(
            parse_v1_route("GET", "/v1/responses/resp_abc"),
            V1Route::GetResponse("resp_abc".into())
        );
        assert_eq!(
            parse_v1_route("POST", "/v1/responses/resp_abc/cancel"),
            V1Route::CancelResponse("resp_abc".into())
        );
        assert_eq!(
            parse_v1_route("GET", "/v1/responses/resp_abc/files"),
            V1Route::ListFiles("resp_abc".into())
        );
        assert_eq!(
            parse_v1_route("GET", "/v1/files/file_xyz/content"),
            V1Route::FileContent("file_xyz".into())
        );
        assert_eq!(parse_v1_route("GET", "/v1/responses"), V1Route::NotFound); // wrong method
        assert_eq!(parse_v1_route("GET", "/v1/bogus"), V1Route::NotFound);
    }

    #[test]
    fn request_deserializes_string_and_item_forms() {
        // String input
        let r: CreateResponseRequest =
            serde_json::from_str(r#"{"model":"claude-opus-4-8","input":"fix bug"}"#).unwrap();
        assert_eq!(r.prompt_text(), "fix bug");
        assert!(!r.stream && !r.background);

        // Message-array input + a function_call_output (decision answer)
        let r2: CreateResponseRequest = serde_json::from_str(
            r#"{"input":[
                {"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]},
                {"type":"function_call_output","call_id":"call_1","output":"allow"}
            ],"previous_response_id":"resp_x","stream":true}"#,
        )
        .unwrap();
        assert_eq!(r2.prompt_text(), "hello");
        assert!(r2.stream);
        assert_eq!(r2.previous_response_id.as_deref(), Some("resp_x"));
        assert_eq!(r2.function_call_outputs(), vec![("call_1".into(), "allow".into())]);
    }

    #[test]
    fn tool_routing_by_model() {
        assert_eq!(tool_for_model(Some("claude-opus-4-8")), "claude");
        assert_eq!(tool_for_model(Some("gpt-5.6-sol")), "codex");
        assert_eq!(tool_for_model(Some("codex")), "codex");
        assert_eq!(tool_for_model(None), "claude");
    }

    #[test]
    fn project_output_text_claude_and_codex() {
        let claude = serde_json::json!({
            "type": "assistant",
            "message": {"content": [
                {"type": "text", "text": "hello"},
                {"type": "tool_use", "name": "Read"},
                {"type": "text", "text": "world"}
            ]}
        });
        let codex = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "message", "role": "assistant", "content": [
                {"type": "output_text", "text": "codex reply"}
            ]}
        });
        let user = serde_json::json!({"type": "user", "message": {"content": "ignored"}});
        assert_eq!(project_output_text(&[claude.clone()]), "hello\nworld");
        assert_eq!(project_output_text(&[codex.clone()]), "codex reply");
        assert_eq!(project_output_text(&[user, claude, codex]), "hello\nworld\ncodex reply");
        assert_eq!(project_output_text(&[]), "");
    }

    #[test]
    fn response_serializes_openai_snake_case() {
        let mut resp = ResponseObject::new("resp_1".into(), ResponseStatus::Completed);
        resp.output.push(OutputItem::Message {
            id: "msg_1".into(),
            role: "assistant".into(),
            content: vec![OutputContent::OutputText { text: "done".into() }],
        });
        resp.usage = Some(Usage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
        });
        let v: serde_json::Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["object"], "response");
        assert_eq!(v["status"], "completed");
        assert_eq!(v["output"][0]["type"], "message");
        assert_eq!(v["output"][0]["content"][0]["type"], "output_text");
        assert_eq!(v["output"][0]["content"][0]["text"], "done");
        assert_eq!(v["usage"]["input_tokens"], 10);
        assert_eq!(v["usage"]["total_tokens"], 15);
    }
}
