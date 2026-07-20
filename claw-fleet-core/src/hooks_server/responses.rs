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

/// Entry point for every `/v1/...` request. P1 skeleton: parses the route and
/// returns `501 Not Implemented`; P2+ replace each arm with real handlers that
/// call spawn/tail/decision and project via the types above.
pub(crate) fn dispatch(
    _ctx: &super::ServeCtx,
    request: tiny_http::Request,
    _query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let method = request.method().as_str().to_string();
    let route = parse_v1_route(&method, path);
    let (status, code) = match route {
        V1Route::NotFound => (404u16, "not_found"),
        _ => (501u16, "not_implemented"),
    };
    let body = serde_json::json!({
        "error": { "code": code, "message": format!("{route:?} not yet implemented") }
    })
    .to_string();
    let _ = request.respond(
        tiny_http::Response::from_string(body)
            .with_status_code(status)
            .with_header(json_header),
    );
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
