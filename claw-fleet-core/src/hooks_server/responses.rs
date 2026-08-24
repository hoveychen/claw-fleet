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
    /// `input_image` / `input_file` referencing an upload from `POST /v1/files`.
    #[serde(default)]
    pub file_id: Option<String>,
    /// Inline image data (`data:` URL or http URL). Captured only so the
    /// request can be **rejected**: the agent reads attachments from disk, so
    /// bytes have to be uploaded first. Silently dropping them — which is what
    /// this surface did before — makes a caller think the image arrived.
    #[serde(default)]
    pub image_url: Option<serde_json::Value>,
    /// Inline file data, rejected for the same reason as `image_url`.
    #[serde(default)]
    pub file_data: Option<serde_json::Value>,
}

/// What went wrong with an attachment part, as an HTTP-shaped error.
#[derive(Debug)]
pub struct AttachmentError {
    pub message: String,
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

    /// Every part of the input, flattened. Helper for the attachment passes.
    fn parts(&self) -> Vec<&InputPart> {
        let mut out = Vec::new();
        if let ResponseInput::Items(items) = &self.input {
            for it in items {
                if let InputItem::Message { content: InputContent::Parts(parts), .. } = it {
                    out.extend(parts.iter());
                }
            }
        }
        out
    }

    /// Attachment `file_id`s in request order, after rejecting the shapes this
    /// surface cannot deliver.
    ///
    /// Inline bytes (`image_url`, `file_data`) are an explicit error rather than
    /// a silent drop: the agent reads attachments as files, so there is nothing
    /// to hand it, and a caller who gets a 200 back reasonably assumes the image
    /// was seen. An attachment part with no `file_id` at all is the same class
    /// of mistake and gets the same answer.
    pub fn attachment_file_ids(&self) -> Result<Vec<String>, AttachmentError> {
        let mut ids = Vec::new();
        for p in self.parts() {
            let is_attachment = p.kind == "input_image" || p.kind == "input_file";
            if p.image_url.is_some() || p.file_data.is_some() {
                return Err(AttachmentError {
                    message: format!(
                        "inline attachment data is not supported on {} — upload the bytes with POST /v1/files and reference the returned file_id",
                        p.kind
                    ),
                });
            }
            if !is_attachment {
                continue;
            }
            match p.file_id.as_deref().filter(|s| !s.is_empty()) {
                Some(id) => ids.push(id.to_string()),
                None => {
                    return Err(AttachmentError {
                        message: format!("{} part has no file_id", p.kind),
                    })
                }
            }
        }
        Ok(ids)
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

/// Effective OpenAI status for a projected run. Adjustments over the raw
/// [`map_status`]:
/// - A pending decision card means the turn yielded to await the caller's
///   answer — OpenAI's `completed`-with-tool-calls contract.
/// - A headless (`-p`) turn that reached `waitingInput` is idle. If it has
///   written new content **since this turn started** (`has_new_content`) the
///   turn is done → `completed`; if not, the turn hasn't produced its answer
///   yet (e.g. the brief window right after a resume, before the agent picks
///   up) → keep it `in_progress` so a fast poll doesn't read the *previous*
///   turn's stale `completed`.
pub fn effective_status(
    raw_status: &str,
    has_pending_calls: bool,
    turn_has_output: bool,
) -> ResponseStatus {
    if has_pending_calls {
        return ResponseStatus::Completed;
    }
    // `waitingInput` maps to in_progress but is really "idle awaiting input" —
    // for a headless turn that means done. Complete it only once the turn has
    // produced output.
    if raw_status == "waitingInput" {
        return if turn_has_output {
            ResponseStatus::Completed
        } else {
            ResponseStatus::InProgress
        };
    }
    let base = map_status(raw_status);
    // A genuine completed mapping (done / idle / succeeded) with no output for
    // THIS turn is a stale completion from before the turn engaged — e.g. the
    // brief idle window right after a resume, before the agent picks up. Hold
    // it in progress until the turn delivers, so a poll can't read empty.
    if base == ResponseStatus::Completed && !turn_has_output {
        return ResponseStatus::InProgress;
    }
    base
}

// ── Per-turn projection (multi-turn correctness) ──────────────────────
//
// A v2 response id is `resp_<session uuid>` and a follow-up (`previous_response
// _id`) reuses it, so one evolving session backs every turn. To make each
// projection reflect only the *current* turn — not the whole accumulated
// conversation — we record, per session, the transcript message count at the
// moment a turn starts (a new spawn starts at 0; a resume / decision-answer
// starts at the count of messages already present). The projection then reads
// only messages after that offset.

/// `~/.fleet/cloud-responses/<session_id>.turn` — the message-count offset at
/// which the session's current turn began.
fn turn_marker_path(session_id: &str) -> Option<std::path::PathBuf> {
    crate::session::real_home_dir().map(|h| {
        h.join(".fleet")
            .join("cloud-responses")
            .join(format!("{session_id}.turn"))
    })
}

/// Record the message count at which the current turn starts. `None` session
/// dir is a no-op (projection then falls back to offset 0 = whole transcript).
fn write_turn_offset(session_id: &str, offset: usize) {
    if let Some(p) = turn_marker_path(session_id) {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&p, offset.to_string());
    }
}

/// The current turn's start offset (0 when unset — projects the whole
/// transcript, correct for a fresh single-turn session).
fn read_turn_offset(session_id: &str) -> usize {
    turn_marker_path(session_id)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Assistant text from `messages[offset..]` — i.e. only the current turn's
/// output. Pure. `offset` past the end yields "".
fn project_turn_text(messages: &[serde_json::Value], offset: usize) -> String {
    let start = offset.min(messages.len());
    project_output_text(&messages[start..])
}

/// The number of transcript messages a session currently has (0 if the session
/// or its transcript can't be found). Used to stamp a turn's start offset.
fn current_message_count(
    sources: &[Box<dyn crate::agent_source::AgentSource>],
    session_id: &str,
) -> usize {
    let sessions = crate::session::scan_all_sources(sources);
    let Some(s) = sessions.iter().find(|s| s.id == session_id) else {
        return 0;
    };
    let Some(src) = crate::agent_source::find_source_for_path(sources, &s.jsonl_path) else {
        return 0;
    };
    src.get_messages(&s.jsonl_path).map(|m| m.len()).unwrap_or(0)
}

/// Current byte length of a session's transcript file (0 if absent). Used to
/// seed the streaming tail so a resumed stream emits only the new turn.
fn current_transcript_bytes(
    sources: &[Box<dyn crate::agent_source::AgentSource>],
    session_id: &str,
) -> u64 {
    let sessions = crate::session::scan_all_sources(sources);
    let Some(s) = sessions.iter().find(|s| s.id == session_id) else {
        return 0;
    };
    std::fs::metadata(&s.jsonl_path).map(|m| m.len()).unwrap_or(0)
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

// ──────────────────── Decision cards ⇆ function_call ─────────────────
//
// Projection: each pending Fleet decision card for a session becomes an OpenAI
// `function_call` output item whose `call_id` IS the decision request id, so the
// caller answers by echoing that `call_id` in a `function_call_output`. The
// `arguments` string carries the card payload (the module's request struct,
// serialized) so the client has everything needed to decide.
//
// Answer: a `function_call_output` carries only `call_id` + `output` (no name),
// so we reverse-look up the owning channel by probing each store for that id,
// then build the channel's response struct from `output` and write it back
// through the same path the desktop / mobile respond handlers use — a direct
// `write_response` for guard/permission (their producer is still polling), or
// `parked::deliver` for the four parkable channels (resumes the session when
// the producer has already timed out).

/// Build a `function_call` output item from a decision card payload. Pure.
fn fc_item<T: Serialize>(kind: &str, call_id: &str, payload: &T) -> OutputItem {
    OutputItem::FunctionCall {
        id: format!("fc_{call_id}"),
        call_id: call_id.to_string(),
        name: decision_function_name(kind).to_string(),
        arguments: serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string()),
    }
}

/// Treat an answer verb as approval. Anything else is a denial. Pure.
fn keyword_allow(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "allow" | "approve" | "approved" | "accept" | "yes" | "true" | "ok"
    )
}

/// Parse `output` as a JSON object, or an empty map if it isn't one. Pure.
fn output_obj(output: &str) -> serde_json::Map<String, serde_json::Value> {
    match serde_json::from_str::<serde_json::Value>(output) {
        Ok(serde_json::Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    }
}

/// Extract an answers map from a decision answer. Accepts `{"answers":{…}}`,
/// a bare object (minus reserved keys) used directly as the map, or a plain
/// string stored under `"answer"`. Pure. Shared by fleet-ask and elicitation.
fn answers_from(
    obj: &serde_json::Map<String, serde_json::Value>,
    output: &str,
    reserved: &[&str],
) -> std::collections::BTreeMap<String, String> {
    if let Some(a) = obj.get("answers").and_then(|v| v.as_object()) {
        return a
            .iter()
            .filter_map(|(k, v)| stringify_value(v).map(|s| (k.clone(), s)))
            .collect();
    }
    // A JSON object (even one holding only reserved flags like `declined`) is
    // authoritative: use its non-reserved keys as the answers map and never
    // fall through to the plain-string branch, which would otherwise stuff the
    // whole `{...}` string under "answer".
    let output_is_object = matches!(
        serde_json::from_str::<serde_json::Value>(output),
        Ok(serde_json::Value::Object(_))
    );
    if output_is_object {
        return obj
            .iter()
            .filter(|(k, _)| !reserved.contains(&k.as_str()))
            .filter_map(|(k, v)| stringify_value(v).map(|s| (k.clone(), s)))
            .collect();
    }
    let mut m = std::collections::BTreeMap::new();
    if !output.trim().is_empty() {
        m.insert("answer".to_string(), output.to_string());
    }
    m
}

/// JSON scalar → String (strings verbatim, others via `to_string`). Pure.
fn stringify_value(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

// ── Pure output → response-struct builders (unit-tested) ──────────────

fn build_guard_response(call_id: &str, output: &str) -> crate::guard::GuardResponse {
    let obj = output_obj(output);
    let verb = obj
        .get("decision")
        .and_then(|v| v.as_str())
        .unwrap_or(output);
    crate::guard::GuardResponse {
        id: call_id.to_string(),
        decision: if keyword_allow(verb) {
            crate::guard::GuardDecision::Allow
        } else {
            crate::guard::GuardDecision::Block
        },
        reason: obj.get("reason").and_then(|v| v.as_str()).map(String::from),
    }
}

fn build_permission_response(
    call_id: &str,
    output: &str,
) -> crate::permission_prompt_ipc::PermissionPromptResponse {
    let obj = output_obj(output);
    let verb = obj
        .get("decision")
        .and_then(|v| v.as_str())
        .unwrap_or(output);
    crate::permission_prompt_ipc::PermissionPromptResponse {
        id: call_id.to_string(),
        decision: if keyword_allow(verb) {
            crate::permission_prompt_ipc::PermissionPromptDecision::Allow
        } else {
            crate::permission_prompt_ipc::PermissionPromptDecision::Deny
        },
        reason: obj.get("reason").and_then(|v| v.as_str()).map(String::from),
    }
}

fn build_plan_response(call_id: &str, output: &str) -> crate::plan_approval::PlanApprovalResponse {
    let obj = output_obj(output);
    let verb = obj
        .get("decision")
        .and_then(|v| v.as_str())
        .unwrap_or(output);
    crate::plan_approval::PlanApprovalResponse {
        id: call_id.to_string(),
        decision: if keyword_allow(verb) { "approve" } else { "reject" }.to_string(),
        edited_plan: obj
            .get("edited_plan")
            .and_then(|v| v.as_str())
            .map(String::from),
        feedback: obj.get("feedback").and_then(|v| v.as_str()).map(String::from),
    }
}

fn build_elicitation_response(
    call_id: &str,
    output: &str,
) -> crate::elicitation::ElicitationResponse {
    let obj = output_obj(output);
    let declined = obj.get("declined").and_then(|v| v.as_bool()).unwrap_or(false);
    crate::elicitation::ElicitationResponse {
        id: call_id.to_string(),
        declined,
        answers: answers_from(&obj, output, &["declined", "reason"])
            .into_iter()
            .collect(),
    }
}

fn build_fleet_ask_response(call_id: &str, output: &str) -> crate::mcp_ipc::FleetAskResponse {
    let obj = output_obj(output);
    crate::mcp_ipc::FleetAskResponse {
        id: call_id.to_string(),
        cancelled: obj.get("cancelled").and_then(|v| v.as_bool()).unwrap_or(false),
        answers: answers_from(&obj, output, &["cancelled"]),
    }
}

fn build_a2ui_response(call_id: &str, output: &str) -> crate::mcp_a2ui_ipc::A2uiRenderResponse {
    let obj = output_obj(output);
    let action_context = obj
        .get("action_context")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| stringify_value(v).map(|s| (k.clone(), s)))
                .collect()
        })
        .unwrap_or_default();
    // action_name from the field, or a bare non-JSON string used directly.
    let action_name = obj
        .get("action_name")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| {
            let t = output.trim();
            if !t.is_empty() && output_obj(output).is_empty() && t != "null" {
                Some(t.to_string())
            } else {
                None
            }
        });
    crate::mcp_a2ui_ipc::A2uiRenderResponse {
        id: call_id.to_string(),
        action_name,
        action_context,
        cancelled: obj.get("cancelled").and_then(|v| v.as_bool()).unwrap_or(false),
    }
}

/// Reverse-look up the decision channel that owns `call_id` by probing each
/// store (live request files, then the four parkable stores). Returns the
/// canonical kind string used by [`decision_function_name`].
fn kind_for_call_id(call_id: &str) -> Option<&'static str> {
    if crate::guard::read_request(call_id).is_some() {
        return Some("guard");
    }
    if crate::permission_prompt_ipc::read_request(call_id).is_some() {
        return Some("permission-prompt");
    }
    if crate::elicitation::read_request(call_id).is_some() {
        return Some("elicitation");
    }
    if crate::plan_approval::read_request(call_id).is_some() {
        return Some("plan-approval");
    }
    if crate::mcp_ipc::read_request(call_id).is_some() {
        return Some("fleet-ask");
    }
    if crate::mcp_a2ui_ipc::read_request(call_id).is_some() {
        return Some("a2ui");
    }
    // Parkable channels whose request file may have moved to the parked store.
    if crate::parked::list_requests::<crate::elicitation::ElicitationRequest>(
        crate::parked::ParkedKind::Elicitation,
    )
    .iter()
    .any(|r| r.id == call_id)
    {
        return Some("elicitation");
    }
    if crate::parked::list_requests::<crate::plan_approval::PlanApprovalRequest>(
        crate::parked::ParkedKind::PlanApproval,
    )
    .iter()
    .any(|r| r.id == call_id)
    {
        return Some("plan-approval");
    }
    if crate::parked::list_requests::<crate::mcp_ipc::FleetAskRequest>(
        crate::parked::ParkedKind::FleetAsk,
    )
    .iter()
    .any(|r| r.id == call_id)
    {
        return Some("fleet-ask");
    }
    if crate::parked::list_requests::<crate::mcp_a2ui_ipc::A2uiRenderRequest>(
        crate::parked::ParkedKind::A2uiRender,
    )
    .iter()
    .any(|r| r.id == call_id)
    {
        return Some("a2ui");
    }
    None
}

/// The session id that owns a decision card (needed to project the answered
/// run when the caller omitted `previous_response_id`). Checks the live request
/// then the parked store for the parkable channels.
fn session_for_call(call_id: &str, kind: &str) -> Option<String> {
    match kind {
        "guard" => crate::guard::read_request(call_id).map(|r| r.session_id),
        "permission-prompt" => {
            crate::permission_prompt_ipc::read_request(call_id).map(|r| r.session_id)
        }
        "elicitation" => crate::elicitation::read_request(call_id)
            .map(|r| r.session_id)
            .or_else(|| {
                crate::parked::list_requests::<crate::elicitation::ElicitationRequest>(
                    crate::parked::ParkedKind::Elicitation,
                )
                .into_iter()
                .find(|r| r.id == call_id)
                .map(|r| r.session_id)
            }),
        "plan-approval" => crate::plan_approval::read_request(call_id)
            .map(|r| r.session_id)
            .or_else(|| {
                crate::parked::list_requests::<crate::plan_approval::PlanApprovalRequest>(
                    crate::parked::ParkedKind::PlanApproval,
                )
                .into_iter()
                .find(|r| r.id == call_id)
                .map(|r| r.session_id)
            }),
        "fleet-ask" => crate::mcp_ipc::read_request(call_id)
            .map(|r| r.session_id)
            .or_else(|| {
                crate::parked::list_requests::<crate::mcp_ipc::FleetAskRequest>(
                    crate::parked::ParkedKind::FleetAsk,
                )
                .into_iter()
                .find(|r| r.id == call_id)
                .map(|r| r.session_id)
            }),
        "a2ui" => crate::mcp_a2ui_ipc::read_request(call_id)
            .map(|r| r.session_id)
            .or_else(|| {
                crate::parked::list_requests::<crate::mcp_a2ui_ipc::A2uiRenderRequest>(
                    crate::parked::ParkedKind::A2uiRender,
                )
                .into_iter()
                .find(|r| r.id == call_id)
                .map(|r| r.session_id)
            }),
        _ => None,
    }
}

/// Build the channel's response from `output` and write it back through the
/// same path the desktop/mobile respond handlers use.
fn answer_decision(kind: &str, call_id: &str, output: &str) -> Result<(), String> {
    match kind {
        "guard" => crate::guard::write_response(&build_guard_response(call_id, output)),
        "permission-prompt" => {
            crate::permission_prompt_ipc::write_response(&build_permission_response(call_id, output))
        }
        "elicitation" => {
            let resp = build_elicitation_response(call_id, output);
            crate::parked::deliver(&resp.id, &resp, resp.declined, crate::elicitation::write_response)
        }
        "plan-approval" => {
            let resp = build_plan_response(call_id, output);
            crate::parked::deliver(&resp.id, &resp, false, crate::plan_approval::write_response)
        }
        "fleet-ask" => {
            let resp = build_fleet_ask_response(call_id, output);
            crate::parked::deliver(&resp.id, &resp, resp.cancelled, crate::mcp_ipc::write_response)
        }
        "a2ui" => {
            let resp = build_a2ui_response(call_id, output);
            crate::parked::deliver(
                &resp.id,
                &resp,
                resp.cancelled,
                crate::mcp_a2ui_ipc::write_response,
            )
        }
        other => Err(format!("unknown decision kind {other}")),
    }
}

/// Every pending decision card for `session_id`, projected as `function_call`
/// output items. Scans all six live stores plus the four parkable stores,
/// deduping by `call_id`.
fn pending_function_calls(session_id: &str) -> Vec<OutputItem> {
    use std::collections::HashSet;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    let mut push = |kind: &str, id: &str, item: OutputItem, seen: &mut HashSet<String>| {
        if seen.insert(id.to_string()) {
            let _ = kind;
            out.push(item);
        }
    };

    for id in crate::guard::list_pending_requests() {
        if let Some(req) = crate::guard::read_request(&id) {
            if req.session_id == session_id {
                push("guard", &id, fc_item("guard", &id, &req), &mut seen);
            }
        }
    }
    for id in crate::permission_prompt_ipc::list_pending_requests() {
        if let Some(req) = crate::permission_prompt_ipc::read_request(&id) {
            if req.session_id == session_id {
                push(
                    "permission-prompt",
                    &id,
                    fc_item("permission-prompt", &id, &req),
                    &mut seen,
                );
            }
        }
    }
    for id in crate::elicitation::list_pending_requests() {
        if let Some(req) = crate::elicitation::read_request(&id) {
            if req.session_id == session_id {
                push("elicitation", &id, fc_item("elicitation", &id, &req), &mut seen);
            }
        }
    }
    for req in crate::parked::list_requests::<crate::elicitation::ElicitationRequest>(
        crate::parked::ParkedKind::Elicitation,
    ) {
        if req.session_id == session_id {
            push(
                "elicitation",
                &req.id.clone(),
                fc_item("elicitation", &req.id, &req),
                &mut seen,
            );
        }
    }
    for id in crate::plan_approval::list_pending_requests() {
        if let Some(req) = crate::plan_approval::read_request(&id) {
            if req.session_id == session_id {
                push("plan-approval", &id, fc_item("plan-approval", &id, &req), &mut seen);
            }
        }
    }
    for req in crate::parked::list_requests::<crate::plan_approval::PlanApprovalRequest>(
        crate::parked::ParkedKind::PlanApproval,
    ) {
        if req.session_id == session_id {
            push(
                "plan-approval",
                &req.id.clone(),
                fc_item("plan-approval", &req.id, &req),
                &mut seen,
            );
        }
    }
    for id in crate::mcp_ipc::list_pending_requests() {
        if let Some(req) = crate::mcp_ipc::read_request(&id) {
            if req.session_id == session_id {
                push("fleet-ask", &id, fc_item("fleet-ask", &id, &req), &mut seen);
            }
        }
    }
    for req in crate::parked::list_requests::<crate::mcp_ipc::FleetAskRequest>(
        crate::parked::ParkedKind::FleetAsk,
    ) {
        if req.session_id == session_id {
            push("fleet-ask", &req.id.clone(), fc_item("fleet-ask", &req.id, &req), &mut seen);
        }
    }
    for id in crate::mcp_a2ui_ipc::list_pending_requests() {
        if let Some(req) = crate::mcp_a2ui_ipc::read_request(&id) {
            if req.session_id == session_id {
                push("a2ui", &id, fc_item("a2ui", &id, &req), &mut seen);
            }
        }
    }
    for req in crate::parked::list_requests::<crate::mcp_a2ui_ipc::A2uiRenderRequest>(
        crate::parked::ParkedKind::A2uiRender,
    ) {
        if req.session_id == session_id {
            push("a2ui", &req.id.clone(), fc_item("a2ui", &req.id, &req), &mut seen);
        }
    }
    out
}

// ─────────────────────────── Files (artifacts) ───────────────────────
//
// A customer's run produces files in the container's single workspace
// ([`public_workspace`]). We expose them OpenAI-file-shaped:
//   GET /v1/responses/{id}/files      → { object: "list", data: [file …] }
//   GET /v1/files/{file_id}/content   → raw bytes
// The `file_id` is an opaque `file_<base64url(rel_path)>`; content reads
// canonicalize the joined path and assert it stays under the canonical
// workspace root, so a crafted id can't escape the container.

use base64::Engine as _;

/// OpenAI-shaped file object. `bytes` is the size; `id` round-trips a
/// workspace-relative path through base64url.
#[derive(Debug, Clone, Serialize)]
struct FileObject {
    id: String,
    object: &'static str, // "file"
    bytes: u64,
    created_at: i64,
    filename: String,
    /// `"output"` for artifacts the run produced; for uploads, whatever the
    /// caller declared (OpenAI's own default for non-fine-tuning use is
    /// `user_data`).
    purpose: String,
}

/// Where `POST /v1/files` puts uploads, relative to the workspace root.
///
/// Inside the workspace on purpose: the agent reads an attachment by path, so
/// the bytes have to be somewhere it can reach. Each upload gets its own
/// subdirectory, so the caller's filename survives verbatim (it shows up in the
/// prompt, and `photo.png` reads better than a hash) without one upload ever
/// overwriting another.
const UPLOADS_DIR: &str = ".fleet-uploads";

/// Cap for a single upload. Same ceiling as a desktop attachment — one number
/// for "how big a blob may a caller hand the agent".
const MAX_UPLOAD_BYTES: u64 = crate::backend::MAX_ATTACHMENT_BYTES;

/// Reduce a caller-supplied filename to a bare, safe basename. Traversal
/// (`../`, absolute paths) and empty/`.`/`..` names collapse to a default, so
/// the join below can only land inside the upload's own directory. Pure.
fn sanitize_upload_filename(raw: &str) -> String {
    let base = std::path::Path::new(raw)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cleaned: String = base
        .chars()
        .map(|c| if c == '/' || c == '\\' || c == '\0' { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        return "upload.bin".to_string();
    }
    cleaned.to_string()
}

/// True when a workspace-relative path lives under the uploads directory —
/// the only region `DELETE /v1/files/{id}` is allowed to touch. Pure.
fn is_upload_rel(rel: &str) -> bool {
    let norm = rel.replace('\\', "/");
    norm.starts_with(&format!("{UPLOADS_DIR}/"))
}

/// `file_<base64url(rel)>`. Pure.
fn encode_file_id(rel: &str) -> String {
    format!(
        "file_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rel.as_bytes())
    )
}

/// `file_<base64url(rel)>` → workspace-relative path. `None` if malformed. Pure.
fn decode_file_id(id: &str) -> Option<String> {
    let b64 = id.strip_prefix("file_").filter(|s| !s.is_empty())?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(b64)
        .ok()?;
    String::from_utf8(bytes).ok()
}

/// Directories skipped when walking the workspace — VCS metadata and
/// regenerable build output that a customer never wants to download. Pure.
fn is_ignored_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".next"
            | ".worktrees"
            | ".venv"
            | "__pycache__"
    )
}

/// Directories that must never be exposed through `/v1`, even when a deployment
/// places them inside the workspace root.
///
/// This is not hygiene, it is the confinement boundary. A host that hands out a
/// single persistent volume (muvee mounts exactly one, at `/workspace`) pushes
/// an operator to park Fleet's state and the cred store's data inside the very
/// tree `/v1` serves — and then `.fleet-state/token` is Fleet's admin token and
/// `.foxy-switcher/agent-config.json` is the vault device token. Both were
/// listed *and* downloadable with nothing but the scoped public token on the
/// first cloud deployment. Choosing a workspace subdirectory avoids the overlap;
/// this list means getting that choice wrong is not catastrophic.
///
/// `.claude` / `.codex` / `.ssh` / `.aws` / `.gnupg` are here for the same
/// reason — they are credential stores by convention, so a workspace copy of one
/// is never something a scoped caller should fetch.
fn is_internal_dir(name: &str) -> bool {
    matches!(
        name,
        ".fleet"
            | ".fleet-state"
            | ".foxy-switcher"
            | ".claude"
            | ".codex"
            | ".ssh"
            | ".aws"
            | ".gnupg"
    )
}

/// True when a workspace-relative path traverses an [`is_internal_dir`]
/// component. Checked on read as well as on list: a file id is
/// `base64url(rel_path)`, so it is guessable — skipping these on the walk alone
/// would hide them from the listing while still serving them by id. Pure.
fn is_internal_rel(rel: &str) -> bool {
    rel.replace('\\', "/").split('/').any(is_internal_dir)
}

/// Cap the walk so an enormous tree can't blow up memory / the response.
const MAX_LISTED_FILES: usize = 2000;

/// List regular files under the workspace root (recursively, skipping ignored
/// dirs and symlinks), newest first, capped at [`MAX_LISTED_FILES`].
fn list_workspace_files() -> Vec<FileObject> {
    let root = std::path::PathBuf::from(public_workspace());
    let Ok(canon_root) = root.canonicalize() else {
        return Vec::new();
    };
    let mut out: Vec<(FileObject, std::time::SystemTime)> = Vec::new();
    let mut stack = vec![canon_root.clone()];
    while let Some(dir) = stack.pop() {
        if out.len() >= MAX_LISTED_FILES {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // symlink_metadata: never follow links (escape guard + loop guard).
            let Ok(md) = entry.path().symlink_metadata() else {
                continue;
            };
            if md.file_type().is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if md.is_dir() {
                if !is_ignored_dir(&name) && !is_internal_dir(&name) {
                    stack.push(path);
                }
                continue;
            }
            if !md.is_file() {
                continue;
            }
            let Ok(rel) = path.strip_prefix(&canon_root) else {
                continue;
            };
            let rel_str = rel.to_string_lossy().to_string();
            let mtime = md.modified().unwrap_or(std::time::UNIX_EPOCH);
            let created_at = mtime
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            out.push((
                FileObject {
                    id: encode_file_id(&rel_str),
                    object: "file",
                    bytes: md.len(),
                    created_at,
                    filename: rel_str,
                    purpose: "output".to_string(),
                },
                mtime,
            ));
            if out.len() >= MAX_LISTED_FILES {
                break;
            }
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1)); // newest first
    out.into_iter().map(|(f, _)| f).collect()
}

/// Read a workspace file by opaque id, confined to the workspace root. Returns
/// `(bytes, mime)`. Errors carry an HTTP status so the handler can map them.
fn read_workspace_file(file_id: &str) -> Result<(Vec<u8>, String), (u16, String)> {
    let rel = decode_file_id(file_id).ok_or((404, "malformed file id".to_string()))?;
    // Confinement, part one: never serve Fleet-internal or credential dirs, even
    // when they sit inside the workspace root. Checked before touching the disk
    // so the answer can't depend on whether the file happens to exist.
    if is_internal_rel(&rel) {
        return Err((403, "path is not exposed".to_string()));
    }
    let root = std::path::PathBuf::from(public_workspace());
    let canon_root = root
        .canonicalize()
        .map_err(|_| (404, "workspace not found".to_string()))?;
    let joined = canon_root.join(&rel);
    let canon_file = joined
        .canonicalize()
        .map_err(|_| (404, "file not found".to_string()))?;
    // Confinement: the resolved path must stay under the workspace root, and it
    // must be a regular file (not a dir / device).
    if !canon_file.starts_with(&canon_root) {
        return Err((403, "path escapes workspace".to_string()));
    }
    let md = canon_file
        .symlink_metadata()
        .map_err(|_| (404, "file not found".to_string()))?;
    if !md.is_file() {
        return Err((404, "not a regular file".to_string()));
    }
    let bytes = std::fs::read(&canon_file).map_err(|e| (500, format!("read: {e}")))?;
    let mime = crate::wiki::mime_for_path(&canon_file).to_string();
    Ok((bytes, mime))
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
    /// `POST /v1/files` — upload an attachment the agent can read.
    UploadFile,
    /// `GET /v1/files/{id}` — the file object without its bytes.
    FileMeta(String),
    /// `DELETE /v1/files/{id}` — drop an upload.
    DeleteFile(String),
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
        ["files"] if method == "POST" => V1Route::UploadFile,
        ["files", id] if method == "GET" => V1Route::FileMeta((*id).to_string()),
        ["files", id] if method == "DELETE" => V1Route::DeleteFile((*id).to_string()),
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
/// `None` if no session with that id exists yet. Takes an owned/borrowed
/// sources slice so both the request handlers (`ctx.sources`) and the
/// streaming thread (`build_sources()`) can call it.
fn build_response(
    sources: &[Box<dyn crate::agent_source::AgentSource>],
    session_id: &str,
) -> Option<ResponseObject> {
    let sessions = crate::session::scan_all_sources(sources);
    let s = sessions.iter().find(|s| s.id == session_id)?;
    let status_str = serde_json::to_value(&s.status)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    let mut resp = ResponseObject::new(to_resp_id(session_id), ResponseStatus::InProgress);
    resp.model = s.model.clone();
    resp.usage = Some(Usage {
        input_tokens: s.total_input_tokens,
        output_tokens: s.total_output_tokens,
        total_tokens: s.total_input_tokens.saturating_add(s.total_output_tokens),
    });
    // Project only the CURRENT turn: assistant text from messages after the
    // recorded turn-start offset. `turn_has_output` — the turn produced
    // assistant text — is the completion signal for an idle `waitingInput`
    // turn. It must key off assistant text, NOT raw message growth: on resume
    // the agent writes bookkeeping lines (queue-operation / user echo) BEFORE
    // it generates, so "any new line" would complete the turn with empty text
    // during the brief stale-idle window.
    let offset = read_turn_offset(session_id);
    let mut turn_has_output = false;
    if let Some(src) = crate::agent_source::find_source_for_path(sources, &s.jsonl_path) {
        if let Ok(messages) = src.get_messages(&s.jsonl_path) {
            let text = project_turn_text(&messages, offset);
            turn_has_output = !text.is_empty();
            if turn_has_output {
                resp.output.push(OutputItem::Message {
                    id: format!("msg_{session_id}"),
                    role: "assistant".to_string(),
                    content: vec![OutputContent::OutputText { text }],
                });
            }
        }
    }
    // Pending decision cards surface as function_call items; their presence
    // (or the turn having produced text) drives the status.
    let calls = pending_function_calls(session_id);
    let has_calls = !calls.is_empty();
    if has_calls {
        resp.output.extend(calls);
    }
    resp.status = effective_status(&status_str, has_calls, turn_has_output);
    Some(resp)
}

/// Write one SSE frame (`event: <type>\ndata: <json>\n\n`). Returns `false`
/// once the client has disconnected (write failed).
fn send_sse(stream: &mut dyn std::io::Write, event: &str, data: &serde_json::Value) -> bool {
    let msg = format!("event: {event}\ndata: {data}\n\n");
    stream.write_all(msg.as_bytes()).and_then(|_| stream.flush()).is_ok()
}

fn terminal_event(st: ResponseStatus) -> &'static str {
    match st {
        ResponseStatus::Failed => "failed",
        ResponseStatus::Cancelled => "cancelled",
        _ => "completed",
    }
}

const STREAM_POLL_MS: u64 = 500;
/// Cap the streaming thread so a stuck run can't leak a thread forever
/// (~20 min at 500ms). On timeout we emit `response.incomplete`.
const STREAM_MAX_TICKS: u32 = 2_400;

/// Detach a per-response SSE stream onto its own thread (like
/// [`super::handle_sse_upgrade`], but scoped to one run). Polls the session's
/// transcript incrementally, emitting `response.created` → repeated
/// `response.output_text.delta` → terminal `response.completed/failed/
/// cancelled`. Never touches the serve loop.
fn run_stream(
    mut stream: Box<dyn std::io::Write + Send>,
    session_id: String,
    resp_id: String,
    model: Option<String>,
) {
    std::thread::spawn(move || {
        let mut created = ResponseObject::new(resp_id.clone(), ResponseStatus::Queued);
        created.model = model;
        if !send_sse(
            &mut *stream,
            "response.created",
            &serde_json::to_value(&created).unwrap_or_default(),
        ) {
            return;
        }

        let sources = crate::agent_source::build_sources();
        // Seed the tail at the transcript's current end so a resumed stream
        // emits only THIS turn, not a replay of the whole conversation. A new
        // spawn's file doesn't exist yet → seed 0.
        let seed = current_transcript_bytes(&sources, &session_id);
        let mut offset = seed;
        // Whether THIS turn has streamed any assistant text yet — the terminal
        // guard (mirrors the polling projection's `turn_has_output`).
        let mut emitted_text = false;
        for _ in 0..STREAM_MAX_TICKS {
            let sessions = crate::session::scan_all_sources(&sources);
            if let Some(s) = sessions.iter().find(|s| s.id == session_id) {
                if let Some(src) = crate::agent_source::find_source_for_path(&sources, &s.jsonl_path) {
                    if let Ok((msgs, new_off)) = src.tail_incremental(&s.jsonl_path, offset) {
                        offset = new_off;
                        let delta = project_output_text(&msgs);
                        if !delta.is_empty() {
                            let ev = serde_json::json!({
                                "type": "response.output_text.delta",
                                "delta": delta,
                            });
                            if !send_sse(&mut *stream, "response.output_text.delta", &ev) {
                                return; // client gone
                            }
                            emitted_text = true;
                        }
                    }
                }
                // A pending decision card ends the stream turn: emit the
                // function_call carried by build_response and let the caller
                // answer via a follow-up create. Checked independent of
                // `offset` because a card can appear before any assistant text
                // (e.g. a guard on the very first command).
                let calls = pending_function_calls(&session_id);
                if !calls.is_empty() {
                    let full = build_response(&sources, &session_id)
                        .unwrap_or_else(|| ResponseObject::new(resp_id.clone(), ResponseStatus::Completed));
                    let ev = serde_json::json!({
                        "type": "response.completed",
                        "response": serde_json::to_value(&full).unwrap_or_default(),
                    });
                    let _ = send_sse(&mut *stream, "response.completed", &ev);
                    return;
                }
                // Honor a terminal status only once THIS turn has streamed
                // assistant text, so neither an initial idle scan (new spawn)
                // nor the stale idle / bookkeeping-line window right after a
                // resume completes the response prematurely.
                if emitted_text {
                    let status_str = serde_json::to_value(&s.status)
                        .ok()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_default();
                    // No pending calls here (handled above); emitted_text is
                    // true, so a finished `waitingInput` turn → completed, same
                    // as the polling projection.
                    let st = effective_status(&status_str, false, emitted_text);
                    if matches!(
                        st,
                        ResponseStatus::Completed | ResponseStatus::Failed | ResponseStatus::Cancelled
                    ) {
                        let evname = format!("response.{}", terminal_event(st));
                        let full = build_response(&sources, &session_id)
                            .unwrap_or_else(|| ResponseObject::new(resp_id.clone(), st));
                        let ev = serde_json::json!({
                            "type": evname,
                            "response": serde_json::to_value(&full).unwrap_or_default(),
                        });
                        let _ = send_sse(&mut *stream, &evname, &ev);
                        return;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(STREAM_POLL_MS));
        }
        let ev = serde_json::json!({
            "type": "response.incomplete",
            "response": { "id": resp_id, "status": "incomplete" },
        });
        let _ = send_sse(&mut *stream, "response.incomplete", &ev);
    });
}

/// Upgrade the request to an SSE stream and start [`run_stream`] on it.
fn start_stream(
    request: tiny_http::Request,
    session_id: String,
    resp_id: String,
    model: Option<String>,
) {
    let resp = tiny_http::Response::empty(200)
        .with_header("Content-Type: text/event-stream".parse::<tiny_http::Header>().unwrap())
        .with_header("Cache-Control: no-cache".parse::<tiny_http::Header>().unwrap())
        .with_header("Connection: keep-alive".parse::<tiny_http::Header>().unwrap());
    let stream = request.upgrade("sse", resp);
    run_stream(stream, session_id, resp_id, model);
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
        V1Route::ListFiles(id) => list_files(request, json_header, &id),
        V1Route::FileContent(id) => file_content(request, &id, json_header),
        V1Route::UploadFile => upload_file(request, json_header),
        V1Route::FileMeta(id) => file_meta(request, &id, json_header),
        V1Route::DeleteFile(id) => delete_file(request, &id, json_header),
        V1Route::NotFound => {
            respond_error(request, 404, "not_found", "unknown /v1 route", json_header)
        }
    }
}

/// `POST /v1/responses` — spawn a new run, (with `previous_response_id`)
/// resume one with a follow-up, or (with `function_call_output` items) answer a
/// pending decision card and let the blocked run continue.
fn create_response(ctx: &super::ServeCtx, mut request: tiny_http::Request, json_header: tiny_http::Header) {
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
    let req: CreateResponseRequest = match serde_json::from_str(&buf) {
        Ok(r) => r,
        Err(e) => return respond_error(request, 400, "invalid_request", e.to_string(), json_header),
    };
    let model = req.model.clone();
    let tool = tool_for_model(model.as_deref());
    let stream = req.stream;

    // Decision-answer path: a `function_call_output` echoes the `call_id` of a
    // card we emitted. Route each to its owning channel's respond path; this
    // unblocks the same session (no spawn/resume). The prompt text, if any, is
    // ignored here — the answer itself carries the turn forward.
    let fc_outputs = req.function_call_outputs();
    if !fc_outputs.is_empty() {
        // Resolve each call's owning channel BEFORE answering — answering may
        // remove the request file that `kind_for_call_id` / `session_for_call`
        // read.
        let mut resolved: Vec<(String, &'static str, String)> =
            Vec::with_capacity(fc_outputs.len());
        let mut session_id: Option<String> = req
            .previous_response_id
            .as_deref()
            .and_then(session_id_from_resp)
            .map(String::from);
        for (call_id, output) in &fc_outputs {
            let Some(kind) = kind_for_call_id(call_id) else {
                return respond_error(
                    request,
                    404,
                    "not_found",
                    format!("no pending decision for call_id {call_id}"),
                    json_header,
                );
            };
            if session_id.is_none() {
                session_id = session_for_call(call_id, kind);
            }
            resolved.push((call_id.clone(), kind, output.clone()));
        }
        let Some(sid) = session_id else {
            return respond_error(
                request,
                500,
                "internal",
                "cannot determine session for answered decision",
                json_header,
            );
        };
        // Stamp the turn start at the pre-answer message count so the projection
        // shows only the continuation this answer unblocks.
        write_turn_offset(&sid, current_message_count(ctx.sources, &sid));
        for (call_id, kind, output) in &resolved {
            if let Err(e) = answer_decision(kind, call_id, output) {
                let status = if e.contains("no pending request") { 404 } else { 500 };
                return respond_error(request, status, "decision_answer_failed", e, json_header);
            }
        }
        if stream {
            return start_stream(request, sid.clone(), to_resp_id(&sid), model);
        }
        let r = build_response(ctx.sources, &sid).unwrap_or_else(|| {
            let mut r = ResponseObject::new(to_resp_id(&sid), ResponseStatus::InProgress);
            r.model = model.clone();
            r
        });
        return respond_value(request, 200, &serde_json::to_value(&r).unwrap_or_default(), json_header);
    }

    // Attachments: resolve every referenced upload to a path inside the
    // workspace and append them to the prompt, so the agent reads the bytes off
    // disk. Resolved BEFORE launching — a bad file_id must be a 400, not a run
    // that starts and then can't find its input.
    let prompt = match req.attachment_file_ids() {
        Err(e) => return respond_error(request, 400, "invalid_request", e.message, json_header),
        Ok(ids) => match resolve_attachment_paths(&ids) {
            Err((status, msg)) => {
                let code = if status == 403 { "forbidden" } else { "invalid_request" };
                return respond_error(request, status, code, msg, json_header);
            }
            Ok(paths) => prompt_with_attachments(&req.prompt_text(), &paths),
        },
    };

    // Launch: resume when previous_response_id is set, else spawn a new run.
    // Both yield the internal session id + wire resp_id + initial status.
    let launched: Result<(String, String, ResponseStatus), (u16, &'static str, String)> =
        if let Some(prev) = req.previous_response_id.clone() {
            match session_id_from_resp(&prev).map(|s| s.to_string()) {
                None => Err((400, "invalid_request", "malformed previous_response_id".into())),
                Some(sid) => {
                    // Capture the pre-resume message count; the new turn's
                    // output is everything after it.
                    let turn_offset = current_message_count(ctx.sources, &sid);
                    let spec = crate::agent_source::ResumeSpec {
                        session_id: sid.clone(),
                        workspace_path: public_workspace(),
                        prompt: prompt.clone(),
                        model: model.clone(),
                        ..Default::default()
                    };
                    match crate::agent_source::resume_session(tool, &spec, Box::new(|_| {})) {
                        Ok(()) => {
                            write_turn_offset(&sid, turn_offset);
                            Ok((sid.clone(), to_resp_id(&sid), ResponseStatus::InProgress))
                        }
                        Err(e) => Err((500, "resume_failed", e)),
                    }
                }
            }
        } else {
            let (resp_id, uuid) = new_resp_id();
            let spec = crate::agent_source::SpawnSpec {
                workspace_path: public_workspace(),
                prompt: prompt.clone(),
                model: model.clone(),
                effort: None,
                permission_mode: None,
                session_id: Some(uuid.clone()),
                entrypoint: String::new(),
            };
            match crate::agent_source::spawn_session(tool, &spec) {
                Ok(_) => {
                    // Fresh session starts at turn offset 0 (whole transcript is
                    // this turn); write it explicitly so any stale marker can't
                    // hide the output.
                    write_turn_offset(&uuid, 0);
                    Ok((uuid, resp_id, ResponseStatus::Queued))
                }
                Err(e) => Err((500, "spawn_failed", e)),
            }
        };

    match launched {
        Err((status, code, msg)) => respond_error(request, status, code, msg, json_header),
        Ok((session_id, resp_id, init_status)) => {
            if stream {
                start_stream(request, session_id, resp_id, model);
            } else {
                let mut r = ResponseObject::new(resp_id, init_status);
                r.model = model;
                if req.previous_response_id.is_some() {
                    r.previous_response_id = req.previous_response_id;
                }
                respond_value(request, 200, &serde_json::to_value(&r).unwrap_or_default(), json_header);
            }
        }
    }
}

/// `GET /v1/responses/{id}` — project current session state.
fn get_response(ctx: &super::ServeCtx, request: tiny_http::Request, json_header: tiny_http::Header, resp_id: &str) {
    let Some(sid) = session_id_from_resp(resp_id) else {
        return respond_error(request, 404, "not_found", "malformed response id", json_header);
    };
    match build_response(ctx.sources, sid) {
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
    let mut r = build_response(ctx.sources, sid)
        .unwrap_or_else(|| ResponseObject::new(to_resp_id(sid), ResponseStatus::Cancelled));
    r.status = ResponseStatus::Cancelled;
    respond_value(request, 200, &serde_json::to_value(&r).unwrap_or_default(), json_header);
}

/// `GET /v1/responses/{id}/files` — list the run's workspace artifacts. One
/// customer per container means one workspace, so the listing is
/// workspace-scoped; the `{id}` only needs to be a well-formed response id.
fn list_files(request: tiny_http::Request, json_header: tiny_http::Header, resp_id: &str) {
    if session_id_from_resp(resp_id).is_none() {
        return respond_error(request, 404, "not_found", "malformed response id", json_header);
    }
    let files = list_workspace_files();
    let body = serde_json::json!({ "object": "list", "data": files });
    respond_value(request, 200, &body, json_header);
}

/// `GET /v1/files/{file_id}/content` — stream a workspace file, confined to
/// the workspace root.
fn file_content(request: tiny_http::Request, file_id: &str, json_header: tiny_http::Header) {
    match read_workspace_file(file_id) {
        Ok((bytes, mime)) => {
            let mime_header: tiny_http::Header =
                format!("Content-Type: {mime}").parse().unwrap();
            let _ = request.respond(tiny_http::Response::from_data(bytes).with_header(mime_header));
        }
        Err((status, msg)) => {
            let code = if status == 403 { "forbidden" } else { "not_found" };
            respond_error(request, status, code, msg, json_header);
        }
    }
}

/// `POST /v1/files` — accept a `multipart/form-data` upload (the shape the
/// OpenAI SDKs send) and land it in the workspace so the agent can read it.
///
/// Fields: `file` (required, the bytes + filename) and `purpose` (optional,
/// echoed back; OpenAI's non-fine-tuning default is `user_data`). The response
/// is a file object whose `id` is the same opaque
/// `file_<base64url(workspace-relative path)>` the artifact routes mint, so
/// `GET /v1/files/{id}` / `.../content` / `DELETE` all work on it for free.
fn upload_file(mut request: tiny_http::Request, json_header: tiny_http::Header) {
    let content_type = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Content-Type"))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default();
    let Some(boundary) = super::multipart::boundary_from_content_type(&content_type) else {
        return respond_error(
            request,
            400,
            "invalid_request",
            "expected multipart/form-data with a boundary",
            json_header,
        );
    };

    // Reject on the declared length first, then re-check after reading: a body
    // may lie about (or omit) Content-Length.
    if let Some(len) = request.body_length() {
        if len as u64 > MAX_UPLOAD_BYTES {
            return respond_error(
                request,
                413,
                "file_too_large",
                format!("upload too large: {len} bytes (max {MAX_UPLOAD_BYTES})"),
                json_header,
            );
        }
    }
    let mut body = Vec::new();
    let mut limited = std::io::Read::take(request.as_reader(), MAX_UPLOAD_BYTES + 1);
    let _ = std::io::Read::read_to_end(&mut limited, &mut body);
    if body.len() as u64 > MAX_UPLOAD_BYTES {
        return respond_error(
            request,
            413,
            "file_too_large",
            format!("upload too large: >{MAX_UPLOAD_BYTES} bytes"),
            json_header,
        );
    }

    let parts = match super::multipart::parse_multipart(&body, &boundary) {
        Ok(p) => p,
        Err(e) => return respond_error(request, 400, "invalid_request", e, json_header),
    };
    let Some(file_part) = parts
        .iter()
        .find(|p| p.name == "file" && p.filename.is_some())
        .or_else(|| parts.iter().find(|p| p.filename.is_some()))
    else {
        return respond_error(
            request,
            400,
            "invalid_request",
            "no file part in multipart body (expected a field named \"file\")",
            json_header,
        );
    };
    let purpose = parts
        .iter()
        .find(|p| p.name == "purpose")
        .map(|p| p.text())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "user_data".to_string());

    let filename = sanitize_upload_filename(file_part.filename.as_deref().unwrap_or(""));
    let dir_name = uuid::Uuid::new_v4().simple().to_string();
    let rel = format!("{UPLOADS_DIR}/{dir_name}/{filename}");
    let root = std::path::PathBuf::from(public_workspace());
    let dest_dir = root.join(UPLOADS_DIR).join(&dir_name);
    if let Err(e) = std::fs::create_dir_all(&dest_dir) {
        return respond_error(request, 500, "internal", format!("create upload dir: {e}"), json_header);
    }
    let dest = dest_dir.join(&filename);
    if let Err(e) = std::fs::write(&dest, &file_part.data) {
        return respond_error(request, 500, "internal", format!("write upload: {e}"), json_header);
    }

    let obj = FileObject {
        id: encode_file_id(&rel),
        object: "file",
        bytes: file_part.data.len() as u64,
        created_at: now_unix(),
        filename,
        purpose,
    };
    respond_value(request, 200, &serde_json::to_value(&obj).unwrap_or_default(), json_header);
}

/// Resolve attachment file ids to absolute paths, applying the same confinement
/// as every other file route. A missing id is a 404 and an out-of-bounds one a
/// 403 — the caller's mistake either way, so it must not reach a spawn.
fn resolve_attachment_paths(ids: &[String]) -> Result<Vec<std::path::PathBuf>, (u16, String)> {
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let (_, path, _) = stat_workspace_file(id)
            .map_err(|(status, msg)| (status, format!("attachment {id}: {msg}")))?;
        out.push(path);
    }
    Ok(out)
}

/// Append an attachment manifest to the prompt.
///
/// Paths, not bytes: `claude` reads an image or a document off disk with its own
/// file tools, so the delivery mechanism is telling it where to look. The
/// instruction is explicit ("read each") because a bare path in a prompt is
/// easy for a model to acknowledge without opening. Pure; unit-tested.
fn prompt_with_attachments(prompt: &str, paths: &[std::path::PathBuf]) -> String {
    if paths.is_empty() {
        return prompt.to_string();
    }
    let mut out = prompt.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str("Attached files (read each one from disk before answering):\n");
    for p in paths {
        out.push_str(&format!("- {}\n", p.display()));
    }
    out
}

/// Seconds since the epoch, or 0 if the clock is before it.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Resolve a file id to an existing regular file inside the workspace, applying
/// the same confinement as [`read_workspace_file`] without reading the bytes.
fn stat_workspace_file(file_id: &str) -> Result<(String, std::path::PathBuf, std::fs::Metadata), (u16, String)> {
    let rel = decode_file_id(file_id).ok_or((404, "malformed file id".to_string()))?;
    if is_internal_rel(&rel) {
        return Err((403, "path is not exposed".to_string()));
    }
    let canon_root = std::path::PathBuf::from(public_workspace())
        .canonicalize()
        .map_err(|_| (404, "workspace not found".to_string()))?;
    let canon_file = canon_root
        .join(&rel)
        .canonicalize()
        .map_err(|_| (404, "file not found".to_string()))?;
    if !canon_file.starts_with(&canon_root) {
        return Err((403, "path escapes workspace".to_string()));
    }
    let md = canon_file
        .symlink_metadata()
        .map_err(|_| (404, "file not found".to_string()))?;
    if !md.is_file() {
        return Err((404, "not a regular file".to_string()));
    }
    Ok((rel, canon_file, md))
}

/// `GET /v1/files/{file_id}` — the file object, no bytes.
fn file_meta(request: tiny_http::Request, file_id: &str, json_header: tiny_http::Header) {
    match stat_workspace_file(file_id) {
        Ok((rel, _, md)) => {
            let obj = FileObject {
                id: file_id.to_string(),
                object: "file",
                bytes: md.len(),
                created_at: md
                    .modified()
                    .ok()
                    .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
                filename: rel.clone(),
                purpose: if is_upload_rel(&rel) { "user_data".to_string() } else { "output".to_string() },
            };
            respond_value(request, 200, &serde_json::to_value(&obj).unwrap_or_default(), json_header);
        }
        Err((status, msg)) => {
            let code = if status == 403 { "forbidden" } else { "not_found" };
            respond_error(request, status, code, msg, json_header);
        }
    }
}

/// `DELETE /v1/files/{file_id}` — remove an upload.
///
/// Confined to [`UPLOADS_DIR`] on purpose: the same id space also names the
/// agent's own output files, and a delete route that reached those would hand a
/// scoped caller a way to destroy the run's results. Uploads are the caller's
/// own bytes, so those it may drop.
fn delete_file(request: tiny_http::Request, file_id: &str, json_header: tiny_http::Header) {
    match stat_workspace_file(file_id) {
        Ok((rel, path, _)) => {
            if !is_upload_rel(&rel) {
                return respond_error(
                    request,
                    403,
                    "forbidden",
                    "only uploaded files can be deleted",
                    json_header,
                );
            }
            if let Err(e) = std::fs::remove_file(&path) {
                return respond_error(request, 500, "internal", format!("delete: {e}"), json_header);
            }
            // Each upload owns its directory; drop it once empty so the uploads
            // tree doesn't fill with husks.
            if let Some(parent) = path.parent() {
                let _ = std::fs::remove_dir(parent);
            }
            let body = serde_json::json!({"id": file_id, "object": "file", "deleted": true});
            respond_value(request, 200, &body, json_header);
        }
        Err((status, msg)) => {
            let code = if status == 403 { "forbidden" } else { "not_found" };
            respond_error(request, status, code, msg, json_header);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that mutate the `FLEET_PUBLIC_WORKSPACE` env var, since
    /// cargo runs the test fns concurrently in one process.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let _g = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("FLEET_PUBLIC_WORKSPACE").ok();
        std::env::remove_var("FLEET_PUBLIC_WORKSPACE");
        assert_eq!(public_workspace(), "/workspace");
        if let Some(v) = prev {
            std::env::set_var("FLEET_PUBLIC_WORKSPACE", v);
        }
    }

    /// A scoped `/v1` caller must never be able to list or download Fleet's own
    /// state or the cred store's data, even when a deployment puts those
    /// directories inside the workspace root — which the single-persistent-volume
    /// layout (muvee mounts exactly one volume, at /workspace) invites.
    ///
    /// Regression: on the first cloud deployment, `FLEET_STATE_DIR` and
    /// `FOXY_DATA_DIR` both lived under `/workspace`, so the artifact walk listed
    /// `.fleet-state/token` (Fleet's admin token) and
    /// `.foxy-switcher/agent-config.json` (the vault device token), and
    /// `/v1/files/{id}/content` served both to a caller holding only the scoped
    /// token. Verified against the live container before this guard existed.
    #[test]
    fn internal_dirs_are_not_listed_or_readable() {
        let _g = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("FLEET_PUBLIC_WORKSPACE").ok();
        let root = std::env::temp_dir().join(format!("fleet-conf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, name) in [
            (".fleet-state", "token"),
            (".foxy-switcher", "agent-config.json"),
            ("src", "main.rs"),
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::write(root.join(dir).join(name), b"secret-or-not").unwrap();
        }
        std::env::set_var("FLEET_PUBLIC_WORKSPACE", root.to_string_lossy().to_string());

        let listed: Vec<String> = list_workspace_files().into_iter().map(|f| f.filename).collect();
        assert!(
            listed.iter().any(|f| f.ends_with("main.rs")),
            "real workspace files must still be listed, got {listed:?}"
        );
        for leaked in [".fleet-state/token", ".foxy-switcher/agent-config.json"] {
            assert!(
                !listed.iter().any(|f| f.replace('\\', "/") == leaked),
                "{leaked} must not be listed, got {listed:?}"
            );
            let id = encode_file_id(leaked);
            assert!(
                read_workspace_file(&id).is_err(),
                "{leaked} must not be readable by id"
            );
        }

        let _ = std::fs::remove_dir_all(&root);
        match prev {
            Some(v) => std::env::set_var("FLEET_PUBLIC_WORKSPACE", v),
            None => std::env::remove_var("FLEET_PUBLIC_WORKSPACE"),
        }
    }

    /// A file id is `base64url(relative path)` and therefore forgeable, so the
    /// read path — not the listing — is where escape attempts must die. Both
    /// shapes an attacker actually has: a `../` id, and a symlink planted inside
    /// the workspace that points out of it.
    #[test]
    fn crafted_ids_and_symlinks_cannot_escape_the_workspace() {
        let _g = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("FLEET_PUBLIC_WORKSPACE").ok();
        let base = std::env::temp_dir().join(format!("fleet-esc-{}", std::process::id()));
        let root = base.join("ws");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(base.join("outside.txt"), b"not yours").unwrap();
        std::fs::write(root.join("inside.txt"), b"fine").unwrap();
        std::env::set_var("FLEET_PUBLIC_WORKSPACE", root.to_string_lossy().to_string());

        // Sanity: an honest id still reads.
        assert!(read_workspace_file(&encode_file_id("inside.txt")).is_ok());

        // Traversal by crafted id.
        assert!(read_workspace_file(&encode_file_id("../outside.txt")).is_err());
        assert!(read_workspace_file(&encode_file_id("/etc/hosts")).is_err());
        assert!(read_workspace_file("file_not-base64!!").is_err());

        // Symlink planted inside the workspace, pointing out of it.
        #[cfg(unix)]
        {
            let link = root.join("escape.txt");
            std::os::unix::fs::symlink(base.join("outside.txt"), &link).unwrap();
            let err = read_workspace_file(&encode_file_id("escape.txt"))
                .err()
                .expect("symlink out of the workspace must be refused");
            assert_eq!(err.0, 403, "expected a confinement error, got {err:?}");
            // …and it must not show up in the listing either.
            let listed: Vec<String> =
                list_workspace_files().into_iter().map(|f| f.filename).collect();
            assert!(!listed.iter().any(|f| f == "escape.txt"), "got {listed:?}");
        }

        let _ = std::fs::remove_dir_all(&base);
        match prev {
            Some(v) => std::env::set_var("FLEET_PUBLIC_WORKSPACE", v),
            None => std::env::remove_var("FLEET_PUBLIC_WORKSPACE"),
        }
    }

    #[test]
    fn attachment_file_ids_collected_in_order() {
        let r: CreateResponseRequest = serde_json::from_str(
            r#"{"input":[{"type":"message","role":"user","content":[
                {"type":"input_text","text":"what is in these?"},
                {"type":"input_image","file_id":"file_AAA"},
                {"type":"input_file","file_id":"file_BBB"}
            ]}]}"#,
        )
        .unwrap();
        assert_eq!(r.prompt_text(), "what is in these?");
        assert_eq!(r.attachment_file_ids().unwrap(), vec!["file_AAA", "file_BBB"]);
    }

    /// The regression this whole surface exists for: an inline image used to
    /// deserialize fine, contribute nothing, and return 200 — so a caller
    /// believed the agent had seen it. Now it is a stated error.
    #[test]
    fn inline_attachment_data_is_rejected_not_dropped() {
        for body in [
            r#"{"input":[{"type":"message","role":"user","content":[
                {"type":"input_text","text":"hi"},
                {"type":"input_image","image_url":"data:image/png;base64,iVBORw0KGgo="}
            ]}]}"#,
            r#"{"input":[{"type":"message","role":"user","content":[
                {"type":"input_file","filename":"a.pdf","file_data":"data:application/pdf;base64,JVBER"}
            ]}]}"#,
            // Chat-Completions object form of image_url.
            r#"{"input":[{"type":"message","role":"user","content":[
                {"type":"input_image","image_url":{"url":"https://example.com/a.png"}}
            ]}]}"#,
        ] {
            let r: CreateResponseRequest = serde_json::from_str(body).unwrap();
            let err = r.attachment_file_ids().err().expect("must be rejected");
            assert!(
                err.message.contains("POST /v1/files"),
                "the error should point at the upload route, got {}",
                err.message
            );
        }
    }

    #[test]
    fn attachment_part_without_file_id_is_rejected() {
        let r: CreateResponseRequest = serde_json::from_str(
            r#"{"input":[{"type":"message","role":"user","content":[{"type":"input_image"}]}]}"#,
        )
        .unwrap();
        assert!(r.attachment_file_ids().is_err());
    }

    #[test]
    fn unknown_non_attachment_parts_stay_tolerated() {
        // Forward-compat: an OpenAI client may send part types we don't model.
        // Those still must not fail the request — only attachments we cannot
        // deliver do.
        let r: CreateResponseRequest = serde_json::from_str(
            r#"{"input":[{"type":"message","role":"user","content":[
                {"type":"input_text","text":"go"},
                {"type":"refusal","refusal":"nope"}
            ]}]}"#,
        )
        .unwrap();
        assert_eq!(r.attachment_file_ids().unwrap(), Vec::<String>::new());
        assert_eq!(r.prompt_text(), "go");
    }

    #[test]
    fn prompt_gains_an_attachment_manifest() {
        let paths = vec![
            std::path::PathBuf::from("/workspace/repo/.fleet-uploads/a/photo.png"),
            std::path::PathBuf::from("/workspace/repo/.fleet-uploads/b/spec.pdf"),
        ];
        let out = prompt_with_attachments("describe it", &paths);
        assert!(out.starts_with("describe it\n\n"));
        assert!(out.contains("read each one from disk"));
        assert!(out.contains("- /workspace/repo/.fleet-uploads/a/photo.png\n"));
        assert!(out.contains("- /workspace/repo/.fleet-uploads/b/spec.pdf\n"));
        // No attachments → the prompt is untouched, byte for byte.
        assert_eq!(prompt_with_attachments("plain", &[]), "plain");
    }

    #[test]
    fn upload_filename_sanitizing_defeats_traversal() {
        assert_eq!(sanitize_upload_filename("shot.png"), "shot.png");
        assert_eq!(sanitize_upload_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_upload_filename("/etc/shadow"), "shadow");
        assert_eq!(sanitize_upload_filename(".."), "upload.bin");
        assert_eq!(sanitize_upload_filename("."), "upload.bin");
        assert_eq!(sanitize_upload_filename(""), "upload.bin");
        assert_eq!(sanitize_upload_filename("   "), "upload.bin");
        // A Windows-style path arrives as one component on unix; the separator
        // is neutralized rather than treated as a directory break.
        assert_eq!(sanitize_upload_filename(r"C:\tmp\a.txt"), "C:_tmp_a.txt");
    }

    #[test]
    fn only_uploads_are_deletable_paths() {
        assert!(is_upload_rel(".fleet-uploads/abc/photo.png"));
        assert!(!is_upload_rel("src/main.rs"));
        assert!(!is_upload_rel("fleet-uploads/abc/photo.png"));
        // A path that merely mentions the dir deeper down is not an upload.
        assert!(!is_upload_rel("src/.fleet-uploads/x"));
    }

    #[test]
    fn file_routes_parse() {
        assert_eq!(parse_v1_route("POST", "/v1/files"), V1Route::UploadFile);
        assert_eq!(
            parse_v1_route("GET", "/v1/files/file_abc"),
            V1Route::FileMeta("file_abc".to_string())
        );
        assert_eq!(
            parse_v1_route("DELETE", "/v1/files/file_abc"),
            V1Route::DeleteFile("file_abc".to_string())
        );
        assert_eq!(
            parse_v1_route("GET", "/v1/files/file_abc/content"),
            V1Route::FileContent("file_abc".to_string())
        );
        // Unsupported verbs on a known shape stay 404 rather than silently
        // matching a neighbouring route.
        assert_eq!(parse_v1_route("PUT", "/v1/files/file_abc"), V1Route::NotFound);
        assert_eq!(parse_v1_route("DELETE", "/v1/files"), V1Route::NotFound);
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
    fn effective_status_promotes_waiting_and_pending() {
        // A finished-but-idle headless turn (waitingInput + output) → completed.
        assert_eq!(
            effective_status("waitingInput", false, true),
            ResponseStatus::Completed
        );
        // waitingInput with NO output yet (resume race window) stays in_progress.
        assert_eq!(
            effective_status("waitingInput", false, false),
            ResponseStatus::InProgress
        );
        // A completed-mapping status (idle/done) with no output yet is a stale
        // completion from before the turn engaged → held in_progress.
        assert_eq!(effective_status("idle", false, false), ResponseStatus::InProgress);
        assert_eq!(effective_status("done", false, false), ResponseStatus::InProgress);
        // …but once the turn produced output they complete.
        assert_eq!(effective_status("idle", false, true), ResponseStatus::Completed);
        assert_eq!(effective_status("done", false, true), ResponseStatus::Completed);
        // Actively generating stays in_progress regardless of output.
        assert_eq!(effective_status("running", false, true), ResponseStatus::InProgress);
        assert_eq!(effective_status("thinking", false, false), ResponseStatus::InProgress);
        // A pending decision card completes the turn regardless of raw/output.
        assert_eq!(effective_status("running", true, false), ResponseStatus::Completed);
        assert_eq!(effective_status("waitingInput", true, false), ResponseStatus::Completed);
        // Failed / cancelled pass through even with no output (failed turns may
        // produce nothing).
        assert_eq!(effective_status("failed", false, false), ResponseStatus::Failed);
        assert_eq!(effective_status("stopped", false, false), ResponseStatus::Cancelled);
        assert_eq!(effective_status("queued", false, false), ResponseStatus::Queued);
    }

    #[test]
    fn project_turn_text_slices_to_current_turn() {
        let turn1_user = serde_json::json!({"type":"user","message":{"content":"go"}});
        let turn1_asst = serde_json::json!({
            "type":"assistant","message":{"content":[{"type":"text","text":"DONE"}]}
        });
        let turn2_user = serde_json::json!({"type":"user","message":{"content":"again"}});
        let turn2_asst = serde_json::json!({
            "type":"assistant","message":{"content":[{"type":"text","text":"SECOND"}]}
        });
        let msgs = [turn1_user, turn1_asst, turn2_user, turn2_asst];
        // offset 0 → whole conversation (the old, buggy behavior if left at 0)
        assert_eq!(project_turn_text(&msgs, 0), "DONE\nSECOND");
        // offset 2 → only turn 2's assistant text
        assert_eq!(project_turn_text(&msgs, 2), "SECOND");
        // offset at/after end → empty (turn hasn't produced output yet)
        assert_eq!(project_turn_text(&msgs, 4), "");
        assert_eq!(project_turn_text(&msgs, 99), "");
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
    fn sse_framing_is_event_data_blank() {
        let mut buf: Vec<u8> = Vec::new();
        let data = serde_json::json!({"type": "response.output_text.delta", "delta": "hi"});
        assert!(send_sse(&mut buf, "response.output_text.delta", &data));
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with("event: response.output_text.delta\n"));
        assert!(s.contains("data: {"));
        assert!(s.ends_with("\n\n"));
        assert!(s.contains("\"delta\":\"hi\""));
    }

    #[test]
    fn terminal_event_names() {
        assert_eq!(terminal_event(ResponseStatus::Completed), "completed");
        assert_eq!(terminal_event(ResponseStatus::Failed), "failed");
        assert_eq!(terminal_event(ResponseStatus::Cancelled), "cancelled");
    }

    #[test]
    fn pending_card_projects_as_function_call() {
        let item = fc_item("guard", "gid_1", &serde_json::json!({"command": "rm -rf /"}));
        match item {
            OutputItem::FunctionCall { id, call_id, name, arguments } => {
                assert_eq!(id, "fc_gid_1");
                assert_eq!(call_id, "gid_1");
                assert_eq!(name, "fleet_guard");
                assert!(arguments.contains("rm -rf /"));
            }
            _ => panic!("expected function_call item"),
        }
    }

    #[test]
    fn guard_output_maps_allow_and_block() {
        assert_eq!(
            build_guard_response("c1", "allow").decision,
            crate::guard::GuardDecision::Allow
        );
        assert_eq!(
            build_guard_response("c1", "approve").decision,
            crate::guard::GuardDecision::Allow
        );
        assert_eq!(
            build_guard_response("c1", "block").decision,
            crate::guard::GuardDecision::Block
        );
        // JSON object form carries a reason and an explicit decision.
        let r = build_guard_response("c1", r#"{"decision":"block","reason":"too risky"}"#);
        assert_eq!(r.decision, crate::guard::GuardDecision::Block);
        assert_eq!(r.reason.as_deref(), Some("too risky"));
    }

    #[test]
    fn permission_output_maps_allow_and_deny() {
        use crate::permission_prompt_ipc::PermissionPromptDecision as D;
        assert_eq!(build_permission_response("c1", "yes").decision, D::Allow);
        assert_eq!(build_permission_response("c1", "deny").decision, D::Deny);
        assert_eq!(build_permission_response("c1", "").decision, D::Deny);
    }

    #[test]
    fn plan_output_maps_approve_and_reject() {
        assert_eq!(build_plan_response("c1", "approve").decision, "approve");
        assert_eq!(build_plan_response("c1", "reject").decision, "reject");
        let r = build_plan_response(
            "c1",
            r#"{"decision":"approve","edited_plan":"do X","feedback":"nit"}"#,
        );
        assert_eq!(r.decision, "approve");
        assert_eq!(r.edited_plan.as_deref(), Some("do X"));
        assert_eq!(r.feedback.as_deref(), Some("nit"));
    }

    #[test]
    fn answers_extraction_forms() {
        // nested answers object
        let r = build_fleet_ask_response("c1", r#"{"answers":{"Q1":"A","Q2":"B"}}"#);
        assert_eq!(r.answers.get("Q1").map(String::as_str), Some("A"));
        assert_eq!(r.answers.get("Q2").map(String::as_str), Some("B"));
        assert!(!r.cancelled);
        // cancelled flag honored, reserved key excluded from the map
        let r2 = build_fleet_ask_response("c1", r#"{"cancelled":true,"Q1":"A"}"#);
        assert!(r2.cancelled);
        assert_eq!(r2.answers.get("Q1").map(String::as_str), Some("A"));
        assert!(!r2.answers.contains_key("cancelled"));
        // plain string → single answer
        let r3 = build_fleet_ask_response("c1", "just this");
        assert_eq!(r3.answers.get("answer").map(String::as_str), Some("just this"));
    }

    #[test]
    fn elicitation_declined_flag() {
        let r = build_elicitation_response("c1", r#"{"declined":true}"#);
        assert!(r.declined);
        assert!(r.answers.is_empty());
        let r2 = build_elicitation_response("c1", r#"{"answers":{"name":"foo"}}"#);
        assert!(!r2.declined);
        assert_eq!(r2.answers.get("name").map(String::as_str), Some("foo"));
    }

    #[test]
    fn a2ui_action_name_forms() {
        // bare string → action_name
        let r = build_a2ui_response("c1", "submit");
        assert_eq!(r.action_name.as_deref(), Some("submit"));
        assert!(!r.cancelled);
        // structured form with context
        let r2 = build_a2ui_response(
            "c1",
            r#"{"action_name":"save","action_context":{"score":"7"},"cancelled":false}"#,
        );
        assert_eq!(r2.action_name.as_deref(), Some("save"));
        assert_eq!(r2.action_context.get("score").map(String::as_str), Some("7"));
        // cancelled with no action
        let r3 = build_a2ui_response("c1", r#"{"cancelled":true}"#);
        assert!(r3.cancelled);
        assert_eq!(r3.action_name, None);
    }

    #[test]
    fn keyword_allow_recognizes_verbs() {
        for yes in ["allow", "APPROVE", " Yes ", "ok", "true", "accept"] {
            assert!(keyword_allow(yes), "{yes} should be allow");
        }
        for no in ["block", "deny", "no", "", "reject", "maybe"] {
            assert!(!keyword_allow(no), "{no} should not be allow");
        }
    }

    #[test]
    fn function_call_output_routes_by_call_id_shape() {
        // A request carrying only a function_call_output (no message) parses,
        // exposes the (call_id, output) pair, and no prompt text.
        let r: CreateResponseRequest = serde_json::from_str(
            r#"{"previous_response_id":"resp_s1","input":[
                {"type":"function_call_output","call_id":"guard_42","output":"allow"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(r.prompt_text(), "");
        assert_eq!(
            r.function_call_outputs(),
            vec![("guard_42".into(), "allow".into())]
        );
    }

    #[test]
    fn file_id_round_trips() {
        for rel in ["report.md", "src/main.rs", "a b/c-d.txt", "深/文件.json"] {
            let id = encode_file_id(rel);
            assert!(id.starts_with("file_"));
            assert_eq!(decode_file_id(&id).as_deref(), Some(rel));
        }
        assert_eq!(decode_file_id("file_"), None);
        assert_eq!(decode_file_id("nope"), None);
        assert_eq!(decode_file_id("file_!!!not-base64"), None);
    }

    #[test]
    fn ignored_dirs_skipped() {
        assert!(is_ignored_dir(".git"));
        assert!(is_ignored_dir("node_modules"));
        assert!(is_ignored_dir("target"));
        assert!(!is_ignored_dir("src"));
        assert!(!is_ignored_dir("docs"));
    }

    #[test]
    fn workspace_walk_lists_and_confines() {
        // A private temp workspace: bound via FLEET_PUBLIC_WORKSPACE for this
        // test only. Serializes with other env-mutating tests via a mutex.
        let _g = ENV_LOCK.lock().unwrap();
        let base = std::env::temp_dir().join(format!("fleet-resp-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src")).unwrap();
        std::fs::create_dir_all(base.join("node_modules/pkg")).unwrap();
        std::fs::write(base.join("report.md"), b"hello").unwrap();
        std::fs::write(base.join("src/main.rs"), b"fn main(){}").unwrap();
        std::fs::write(base.join("node_modules/pkg/index.js"), b"junk").unwrap();
        let prev = std::env::var("FLEET_PUBLIC_WORKSPACE").ok();
        std::env::set_var("FLEET_PUBLIC_WORKSPACE", &base);

        let files = list_workspace_files();
        let names: std::collections::HashSet<&str> =
            files.iter().map(|f| f.filename.as_str()).collect();
        assert!(names.contains("report.md"));
        assert!(names.contains("src/main.rs"));
        // ignored dir excluded
        assert!(!names.iter().any(|n| n.contains("node_modules")));

        // content read is confined + correct
        let rid = files.iter().find(|f| f.filename == "report.md").unwrap();
        let (bytes, mime) = read_workspace_file(&rid.id).unwrap();
        assert_eq!(bytes, b"hello");
        assert!(mime.starts_with("text/"));

        // a crafted escaping id is rejected (403), not served
        let escape = encode_file_id("../../../etc/passwd");
        let err = read_workspace_file(&escape).unwrap_err();
        assert!(err.0 == 403 || err.0 == 404, "escape must not succeed: {err:?}");

        // restore env + cleanup
        match prev {
            Some(v) => std::env::set_var("FLEET_PUBLIC_WORKSPACE", v),
            None => std::env::remove_var("FLEET_PUBLIC_WORKSPACE"),
        }
        let _ = std::fs::remove_dir_all(&base);
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
