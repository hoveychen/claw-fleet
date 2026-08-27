//! ACP v1 wire types.
//!
//! Field names and enum spellings are taken from the published schema
//! (`schema/v1/schema.json` in `agentclientprotocol/agent-client-protocol`),
//! not from prose — ACP is camelCase on the wire, and several shapes flatten a
//! shared struct into the variant (`allOf` in the schema), which `#[serde(flatten)]`
//! reproduces.
//!
//! Only the types the agent side actually emits or accepts live here. The
//! remaining `session/update` variants (`tool_call`, `plan`, `usage_update`, …)
//! arrive with the sub-plans that produce them, so an unimplemented variant is
//! a compile error rather than a silently unsent update.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Protocol version. `uint16`, bumped only for breaking changes — everything
/// else is negotiated through capabilities.
pub const PROTOCOL_VERSION: u16 = 1;

// ─────────────────────────── Content ────────────────────────────────

/// A block of content in a prompt or an update.
///
/// `resource_link` and `resource` are how ACP expresses files; there is no REST
/// file API in the protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text or Markdown. Every agent MUST support this in prompts.
    Text { text: String },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uri: Option<String>,
    },
    Audio {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    ResourceLink {
        uri: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, rename = "mimeType", skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    },
    Resource { resource: Value },
    /// Forward compatibility: a client on a newer schema may send a block we do
    /// not know. Dropping it silently beats failing the whole prompt.
    #[serde(other)]
    Unknown,
}

impl ContentBlock {
    /// The text this block contributes to a prompt, if any.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentBlock::Text { text } => Some(text),
            _ => None,
        }
    }

    pub fn text(s: impl Into<String>) -> Self {
        ContentBlock::Text { text: s.into() }
    }
}

// ─────────────────────────── initialize ─────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeRequest {
    #[serde(default)]
    pub protocol_version: u16,
    #[serde(default)]
    pub client_capabilities: ClientCapabilities,
    #[serde(default)]
    pub client_info: Option<Implementation>,
}

/// What the *client* offers the agent.
///
/// Fleet cares about exactly one field: [`ElicitationCapabilities`]. A client
/// that does not advertise elicitation cannot be asked a free-form question, so
/// fleet-ask / elicitation cards need a fallback there (handled in the
/// decision-card sub-plan). `fs` and `terminal` let a client lend the agent its
/// filesystem and terminal — Fleet's agent has its own container, so it never
/// calls those and does not care whether they are present.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    #[serde(default)]
    pub fs: Value,
    #[serde(default)]
    pub terminal: Value,
    #[serde(default)]
    pub elicitation: Option<ElicitationCapabilities>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElicitationCapabilities {
    /// Client can render a JSON-schema form.
    #[serde(default)]
    pub form: Option<Value>,
    /// Client can open a URL we host (how rich previews and A2UI get out).
    #[serde(default)]
    pub url: Option<Value>,
}

impl ClientCapabilities {
    pub fn supports_elicitation_form(&self) -> bool {
        self.elicitation.as_ref().is_some_and(|e| e.form.is_some())
    }
    pub fn supports_elicitation_url(&self) -> bool {
        self.elicitation.as_ref().is_some_and(|e| e.url.is_some())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Implementation {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponse {
    pub protocol_version: u16,
    pub agent_capabilities: AgentCapabilities,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub auth_methods: Vec<Value>,
    pub agent_info: Implementation,
}

/// What Fleet offers the client.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    /// Fleet's sessions are long-lived processes in a container, so they
    /// outlive any one connection — a reconnecting client can pick one back up.
    /// Most ACP agents pin a session to its connection and cannot.
    pub load_session: bool,
    pub prompt_capabilities: PromptCapabilities,
    pub session_capabilities: SessionCapabilities,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCapabilities {
    pub image: bool,
    pub audio: bool,
    pub embedded_context: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCapabilities {
    pub list: bool,
    pub delete: bool,
    pub resume: bool,
    pub close: bool,
    /// One container, one workspace — there is no second directory to add.
    pub additional_directories: bool,
}

// ─────────────────────────── session/new ────────────────────────────

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionRequest {
    /// Absolute path, required by the schema. Fleet binds the workspace
    /// server-side, so a client-supplied value cannot be honoured — see
    /// `agent.rs` for what happens to it.
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub mcp_servers: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionResponse {
    pub session_id: String,
}

// ─────────────────────────── session/prompt ─────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRequest {
    pub session_id: String,
    #[serde(default)]
    pub prompt: Vec<ContentBlock>,
}

impl PromptRequest {
    /// Flatten the prompt's text blocks into the string handed to the agent.
    pub fn prompt_text(&self) -> String {
        self.prompt
            .iter()
            .filter_map(ContentBlock::as_text)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Why a prompt turn ended. Returned as the `session/prompt` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResponse {
    pub stop_reason: StopReason,
}

// ────────── session/load · resume · close · delete · list ───────────

/// `session/load` — reattach **and replay** the prior conversation.
///
/// The replay is what separates it from [`ResumeSessionRequest`]; the schema
/// says resume "resumes an existing session without returning previous
/// messages". Fleet can serve both because the transcript on disk is the
/// source of truth — most ACP agents keep the session in the connection and
/// can only offer resume.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadSessionRequest {
    pub session_id: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub mcp_servers: Vec<Value>,
}

/// `session/resume` — reattach **without** replaying.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeSessionRequest {
    pub session_id: String,
    #[serde(default)]
    pub cwd: String,
}

/// Shared shape of `session/close` and `session/delete`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionIdRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsRequest {
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub cursor: Option<String>,
}

/// One entry in `session/list`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// ISO 8601, per the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionModeRequest {
    pub session_id: String,
    pub mode_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionConfigOptionRequest {
    pub session_id: String,
    pub config_id: String,
}

/// `$/cancel_request` params.
///
/// Note this carries **`requestId`**, not `sessionId` — it cancels one in-flight
/// JSON-RPC request. `session/cancel` is the session-scoped one.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelRequestNotification {
    pub request_id: Value,
}

/// `session/cancel` params.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelNotification {
    pub session_id: String,
}

// ─────────────────────────── Tool calls ─────────────────────────────

/// What a tool does, so clients can pick an icon and a progress treatment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

/// Where a tool call is in its lifecycle.
///
/// The schema defines `pending` as "the input is either streaming or we're
/// awaiting approval" — which is exactly the state a Fleet guard card puts a
/// command in, and why a decision card and a tool call can share one lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// A file edit, rendered by the client as a diff rather than as raw text.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diff {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_text: Option<String>,
    pub new_text: String,
}

/// Output attached to a tool call.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolCallContent {
    Content { content: ContentBlock },
    Diff(Diff),
    Terminal { terminal_id: String },
}

impl ToolCallContent {
    pub fn text(s: impl Into<String>) -> Self {
        ToolCallContent::Content { content: ContentBlock::text(s) }
    }
}

/// A file the call touched. Lets a client follow along in its own editor.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallLocation {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// A tool call as first reported.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub tool_call_id: String,
    pub title: String,
    pub kind: ToolKind,
    pub status: ToolCallStatus,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<ToolCallContent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub locations: Vec<ToolCallLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<Value>,
}

/// A change to an already-reported call.
///
/// Every field but the id is optional, and absent fields mean "unchanged" —
/// the schema is explicit that only what changed needs sending.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallUpdate {
    pub tool_call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<ToolCallStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ToolCallContent>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_output: Option<Value>,
}

impl ToolCallUpdate {
    pub fn status(id: impl Into<String>, status: ToolCallStatus) -> Self {
        Self { tool_call_id: id.into(), status: Some(status), ..Default::default() }
    }
}

/// Cumulative token usage for the session's context window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct UsageUpdate {
    /// Tokens consumed so far.
    pub used: u64,
    /// Total context window.
    pub size: u64,
}

/// A change to the session's own metadata.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfoUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

// ──────────────────── session/request_permission ────────────────────

/// What a permission choice means, so the client can style it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: PermissionOptionKind,
}

/// `session/request_permission` params.
///
/// The permission is attached to a tool call — this channel is for "may I run
/// this", not for open questions. Free-form questions go through
/// [`CreateElicitationRequest`] instead.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPermissionRequest {
    pub session_id: String,
    pub tool_call: ToolCallUpdate,
    pub options: Vec<PermissionOption>,
}

/// What the client answered.
///
/// `cancelled` is not a denial: the schema requires a client that sends
/// `session/cancel` to answer every pending permission request this way, so it
/// means "the turn went away", not "the user said no".
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RequestPermissionOutcome {
    Cancelled,
    Selected {
        #[serde(rename = "optionId")]
        option_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RequestPermissionResponse {
    pub outcome: RequestPermissionOutcome,
}

// ─────────────────────────── elicitation ────────────────────────────

/// A JSON-Schema-ish description of the form to render.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ElicitationSchema {
    #[serde(rename = "type")]
    pub kind: &'static str, // always "object"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub properties: serde_json::Map<String, Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
}

impl ElicitationSchema {
    pub fn object(properties: serde_json::Map<String, Value>, required: Vec<String>) -> Self {
        Self { kind: "object", title: None, properties, required }
    }
}

/// `elicitation/create` params.
///
/// The wire form is flat: a `mode` discriminator, the scope (`sessionId` here —
/// Fleet's questions always belong to a session), and whichever fields that
/// mode needs. Form mode carries `requestedSchema`; URL mode carries
/// `elicitationId` + `url`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateElicitationRequest {
    pub session_id: String,
    pub mode: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_schema: Option<ElicitationSchema>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elicitation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl CreateElicitationRequest {
    pub fn form(
        session_id: impl Into<String>,
        message: impl Into<String>,
        schema: ElicitationSchema,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            mode: "form",
            message: message.into(),
            requested_schema: Some(schema),
            elicitation_id: None,
            url: None,
        }
    }

    pub fn url(
        session_id: impl Into<String>,
        message: impl Into<String>,
        elicitation_id: impl Into<String>,
        url: impl Into<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            mode: "url",
            message: message.into(),
            requested_schema: None,
            elicitation_id: Some(elicitation_id.into()),
            url: Some(url.into()),
        }
    }
}

/// How the user answered an elicitation.
///
/// `content` is meaningful only on `accept`; the spec says receivers ignore it
/// for the other two.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ElicitationAction {
    Accept {
        #[serde(default)]
        content: Option<serde_json::Map<String, Value>>,
    },
    Decline,
    Cancel,
}

impl ElicitationAction {
    /// The submitted values, empty unless the user accepted with content.
    pub fn content(&self) -> serde_json::Map<String, Value> {
        match self {
            ElicitationAction::Accept { content: Some(c) } => c.clone(),
            _ => serde_json::Map::new(),
        }
    }

    /// True when the user did not answer — declined or dismissed.
    ///
    /// Both map to Fleet's "declined"/"cancelled" flags, which is what lets a
    /// parked card resume the session instead of hanging.
    pub fn is_refusal(&self) -> bool {
        !matches!(self, ElicitationAction::Accept { .. })
    }
}

// ─────────────────────────── session/update ─────────────────────────

/// `session/update` params.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNotification {
    pub session_id: String,
    pub update: SessionUpdate,
}

/// The update variants Fleet emits.
///
/// The schema models these as a `sessionUpdate` discriminator plus an `allOf`
/// of the payload struct, i.e. the payload fields sit at the same level as the
/// discriminator — which is what an internally-tagged serde enum produces.
///
/// Only the variants that are implemented appear here; the rest arrive with the
/// sub-plans that emit them.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub enum SessionUpdate {
    /// A chunk of the agent's reply.
    AgentMessageChunk { content: ContentBlock },
    /// A chunk of the *user's* message. Only emitted while replaying history
    /// for `session/load`, so the client can rebuild both sides of the
    /// conversation rather than a monologue.
    UserMessageChunk { content: ContentBlock },
    /// A chunk of the agent's reasoning. Clients usually render it collapsed.
    AgentThoughtChunk { content: ContentBlock },
    /// A tool call has started.
    ToolCall(ToolCall),
    /// A tool call changed — status, output, or both.
    ToolCallUpdate(ToolCallUpdate),
    /// Cumulative token usage.
    UsageUpdate(UsageUpdate),
    /// The session's title or timestamp changed.
    SessionInfoUpdate(SessionInfoUpdate),
}

impl SessionUpdate {
    pub fn agent_text(text: impl Into<String>) -> Self {
        SessionUpdate::AgentMessageChunk { content: ContentBlock::text(text) }
    }
    pub fn user_text(text: impl Into<String>) -> Self {
        SessionUpdate::UserMessageChunk { content: ContentBlock::text(text) }
    }
    pub fn thought(text: impl Into<String>) -> Self {
        SessionUpdate::AgentThoughtChunk { content: ContentBlock::text(text) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn content_block_text_round_trips_in_schema_shape() {
        let v = serde_json::to_value(ContentBlock::text("hi")).unwrap();
        assert_eq!(v, json!({"type": "text", "text": "hi"}));
        let back: ContentBlock = serde_json::from_value(v).unwrap();
        assert_eq!(back, ContentBlock::text("hi"));
    }

    #[test]
    fn unknown_content_blocks_deserialize_instead_of_failing() {
        // A client on a newer schema must not be able to break a whole prompt
        // by including one block we do not know yet.
        let b: ContentBlock =
            serde_json::from_value(json!({"type": "some_future_thing", "x": 1})).unwrap();
        assert_eq!(b, ContentBlock::Unknown);
        assert_eq!(b.as_text(), None);
    }

    #[test]
    fn prompt_text_joins_only_text_blocks() {
        let req = PromptRequest {
            session_id: "s1".into(),
            prompt: vec![
                ContentBlock::text("first"),
                ContentBlock::Image {
                    data: "AAAA".into(),
                    mime_type: "image/png".into(),
                    uri: None,
                },
                ContentBlock::text("second"),
            ],
        };
        assert_eq!(req.prompt_text(), "first\nsecond");
    }

    #[test]
    fn session_update_flattens_the_discriminator_with_the_payload() {
        // The schema's `allOf` puts ContentChunk's fields alongside
        // `sessionUpdate`, not nested under it.
        let v = serde_json::to_value(SessionUpdate::agent_text("hello")).unwrap();
        assert_eq!(
            v,
            json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "hello"}})
        );
    }

    #[test]
    fn session_notification_is_camel_case() {
        let n = SessionNotification {
            session_id: "sess-1".into(),
            update: SessionUpdate::agent_text("x"),
        };
        let v = serde_json::to_value(&n).unwrap();
        assert_eq!(v["sessionId"], "sess-1");
        assert!(v.get("session_id").is_none());
    }

    #[test]
    fn stop_reason_uses_the_schema_spelling() {
        assert_eq!(serde_json::to_value(StopReason::EndTurn).unwrap(), json!("end_turn"));
        assert_eq!(
            serde_json::to_value(StopReason::MaxTurnRequests).unwrap(),
            json!("max_turn_requests")
        );
        assert_eq!(serde_json::to_value(StopReason::Cancelled).unwrap(), json!("cancelled"));
    }

    #[test]
    fn client_elicitation_capability_is_opt_in() {
        let none: ClientCapabilities = serde_json::from_value(json!({})).unwrap();
        assert!(!none.supports_elicitation_form());
        assert!(!none.supports_elicitation_url());

        let form: ClientCapabilities =
            serde_json::from_value(json!({"elicitation": {"form": {}}})).unwrap();
        assert!(form.supports_elicitation_form());
        assert!(!form.supports_elicitation_url(), "form must not imply url");

        let both: ClientCapabilities =
            serde_json::from_value(json!({"elicitation": {"form": {}, "url": {}}})).unwrap();
        assert!(both.supports_elicitation_form() && both.supports_elicitation_url());
    }

    #[test]
    fn initialize_request_tolerates_a_minimal_client() {
        let req: InitializeRequest = serde_json::from_value(json!({"protocolVersion": 1})).unwrap();
        assert_eq!(req.protocol_version, 1);
        assert!(req.client_info.is_none());
        assert!(!req.client_capabilities.supports_elicitation_form());
    }

    #[test]
    fn prompt_request_accepts_a_bare_text_prompt() {
        let req: PromptRequest = serde_json::from_value(json!({
            "sessionId": "s",
            "prompt": [{"type": "text", "text": "do it"}]
        }))
        .unwrap();
        assert_eq!(req.prompt_text(), "do it");
    }
}
