//! Minimal MCP (Model Context Protocol) stdio server exposing Fleet tools.
//!
//! Implements JSON-RPC 2.0 over stdin/stdout (line-delimited JSON).
//! Methods: `initialize`, `notifications/initialized`, `tools/list`, `tools/call`.
//!
//! `tools/call` for `fleet__ask` is bridged to Fleet's Decision Panel via the
//! file-IPC pattern in [`crate::mcp_ipc`]: the request goes to
//! `~/.fleet/fleet-ask/<id>.json`, the desktop watcher emits a card, the user
//! resolves it, and the response lands at `<id>.response.json`. The MCP server
//! polls for that response and converts it into a JSON-RPC tool result.
//!
//! Heartbeat-aware: if the Fleet consumer (desktop or `fleet serve`) is not
//! alive when `tools/call` arrives, the call fails fast with a structured
//! error so the agent can fall back to the native `AskUserQuestion` flow.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::time::Instant;

pub const PROTOCOL_VERSION: &str = "2024-11-05";
pub const SERVER_NAME: &str = "fleet";

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

/// Run the MCP server on stdin/stdout until EOF.
pub fn run() -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(resp) = handle_line(&line) {
            writeln!(out, "{}", resp)?;
            out.flush()?;
        }
    }
    Ok(())
}

/// Process one JSON-RPC line. Returns `None` for notifications (no id).
fn handle_line(line: &str) -> Option<String> {
    let req: JsonRpcRequest = serde_json::from_str(line).ok()?;
    let id = req.id?; // notification → no response
    let resp = match dispatch(&req.method, &req.params) {
        Ok(result) => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        },
        Err(error) => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(error),
        },
    };
    serde_json::to_string(&resp).ok()
}

fn dispatch(method: &str, params: &Value) -> Result<Value, JsonRpcError> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": env!("CARGO_PKG_VERSION"),
            },
        })),
        "tools/list" => Ok(tools_list_result(session_is_fleet_owned())),
        "tools/call" => handle_tool_call(params),
        other => Err(JsonRpcError {
            code: -32601,
            message: format!("Method not found: {}", other),
        }),
    }
}

/// Build the `tools/list` result. The four UI tools are always present; the six
/// control tools (plan / handoff / watch / loop / schedule / wiki) are appended
/// ONLY for Fleet-owned sessions — a user's hand-launched `claude` never sees
/// them and keeps using the `fleet` CLI. This makes "Fleet sessions go through
/// MCP, non-Fleet through the CLI" hold at the tool-visibility layer, and is
/// what lets rca remote-workspace sessions drive these commands (a Bash
/// `fleet …` is routed to a remote executor with no `fleet`; an MCP call reaches
/// this local server that owns the local state). Split from `dispatch` so the
/// gating is testable without mutating process env.
fn tools_list_result(fleet_owned: bool) -> Value {
    let mut tools = vec![
        fleet_ask_tool_def(),
        a2ui_render_tool_def(),
        set_session_title_tool_def(),
        permission_prompt_tool_def(),
    ];
    if fleet_owned {
        tools.extend(crate::mcp_control::control_tool_defs());
    }
    json!({ "tools": tools })
}

fn fleet_ask_tool_def() -> Value {
    json!({
        "name": "fleet__ask",
        "description": "Ask the user one or more questions through Fleet's Decision Panel. Schema mirrors Claude Code's native AskUserQuestion plus three optional fields: `html` (HTML preview, rendered in a sandboxed iframe), `formFields` (structured input fields), and `images` (local image files shown WITHOUT base64-inlining — pass file paths, reference them from `html` by name). Every question must have an answer surface: at least 2 `options`, OR `html`, OR `formFields` — a question with none of the three is rejected. To display an image, ALWAYS use `images` + a relative `<img src=\"name\">`; never base64-inline it into `html` (that wastes output tokens).",
        "inputSchema": crate::mcp_ipc::fleet_ask_input_schema(),
    })
}

fn permission_prompt_tool_def() -> Value {
    json!({
        "name": "fleet__permission_prompt",
        "description": "INTERNAL — Claude Code's --permission-prompt-tool handler for Fleet-launched headless sessions. Routes native permission prompts to Fleet's Decision Panel and returns the user's allow/deny verdict. Do not call this tool directly from agent code; it is invoked automatically by the Claude Code permission machinery.",
        "inputSchema": crate::permission_prompt_ipc::permission_prompt_input_schema(),
    })
}

fn a2ui_render_tool_def() -> Value {
    json!({
        "name": "fleet__render_a2ui",
        "description": "Render an A2UI v0.9 agent-to-client message tree in Fleet's Decision Panel via the official @a2ui/react renderer, then return the user's Action payload. Reach for this when the UI needs richer layout than fleet__ask's flat formFields (Tabs / Modal / Card / Video / etc.). For simple form or option asks, prefer fleet__ask.",
        "inputSchema": crate::mcp_a2ui_ipc::a2ui_render_input_schema(),
    })
}

fn set_session_title_tool_def() -> Value {
    json!({
        "name": "fleet__set_session_title",
        "description": "Set a concise, descriptive title for the current Fleet session after its topic is clear. The current session is resolved automatically; do not include a session id. Call again only if the conversation's topic materially changes.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "A concise title that names the concrete task or topic."
                }
            },
            "required": ["title"],
            "additionalProperties": false
        }
    })
}

/// Default deadline for waiting on a user response — matches the
/// `decision_panel_config` `wait_seconds` default so `fleet__ask` and the
/// elicitation bridge time out on the same clock.
fn fleet_ask_timeout() -> std::time::Duration {
    crate::decision_panel_config::load().wait_duration()
}

/// Default heartbeat window for verifying that a consumer (desktop or
/// `fleet serve`) is alive before we even bother writing the request.
fn heartbeat_window() -> std::time::Duration {
    crate::decision_panel_config::load().heartbeat_window()
}

fn handle_tool_call(params: &Value) -> Result<Value, JsonRpcError> {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    match name {
        "fleet__ask" => handle_fleet_ask_call(params),
        "fleet__render_a2ui" => handle_a2ui_render_call(params),
        "fleet__set_session_title" => handle_set_session_title_call(params),
        "fleet__permission_prompt" => handle_permission_prompt_call(params),
        n if crate::mcp_control::is_control_tool(n) => handle_control_tool_call(n, params),
        other => Err(JsonRpcError {
            code: -32602,
            message: format!("Unknown tool: {}", other),
        }),
    }
}

/// True when the invoking session was spawned by Fleet — the gate for exposing
/// and running the control tools. Reads the session id from the same env the UI
/// tools use ([`current_session_id`]); an unresolvable id is treated as
/// not-Fleet-owned (the tools stay hidden and refuse), which is the safe default.
fn session_is_fleet_owned() -> bool {
    let sid = current_session_id();
    !sid.is_empty() && crate::launch_spec::was_fleet_spawned(&sid)
}

/// Bridge a control-tool `tools/call` to [`crate::mcp_control::handle`]. Resolves
/// the session id and workspace cwd the same way the UI handlers do, then maps
/// its `Ok(text)` / `Err(text)` onto the MCP tool-result envelope. Defense in
/// depth: even though the tool is hidden from non-Fleet sessions, a direct call
/// is refused unless the session is Fleet-owned.
fn handle_control_tool_call(name: &str, params: &Value) -> Result<Value, JsonRpcError> {
    if !session_is_fleet_owned() {
        return Ok(tool_error(format!(
            "{name} is only available in Fleet-launched sessions; use the `fleet` CLI instead."
        )));
    }
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    let sid = current_session_id();
    let session_id = (!sid.is_empty()).then_some(sid.as_str());
    // Workspace: the agent's CLAUDE_PROJECT_DIR (what the UI handlers chip on),
    // falling back to the server's cwd (Fleet spawns claude with
    // `.current_dir(workspace)`, which the MCP child inherits).
    let cwd = std::env::var("CLAUDE_PROJECT_DIR")
        .ok()
        .filter(|d| !d.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
    match crate::mcp_control::handle(name, &args, session_id, &cwd) {
        Ok(text) => Ok(json!({
            "content": [{ "type": "text", "text": text }],
            "isError": false,
        })),
        Err(text) => Ok(tool_error(text)),
    }
}

fn handle_set_session_title_call(params: &Value) -> Result<Value, JsonRpcError> {
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    let title = args
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .ok_or_else(|| JsonRpcError {
            code: -32602,
            message: "Missing or blank `title` argument".into(),
        })?;
    let session_id = current_session_id();
    if session_id.is_empty() {
        return Ok(tool_error(
            "Fleet could not resolve the current session id; continue without setting a title."
                .into(),
        ));
    }
    let workspace_path = std::env::current_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let outcome = match crate::session_title::set_agent_title(
        &session_id,
        &workspace_path,
        title.to_string(),
    ) {
        Ok(outcome) => outcome,
        Err(error) => return Ok(tool_error(format!("Failed to set Fleet session title: {error}"))),
    };

    if outcome == crate::session_title::AgentTitleOutcome::ManualTitlePreserved {
        return Ok(json!({
            "content": [{
                "type": "text",
                "text": "Session already has a manual title; kept the user's title unchanged.",
            }],
            "structuredContent": { "updated": false, "reason": "manual_title" },
            "isError": false,
        }));
    }

    Ok(json!({
        "content": [{
            "type": "text",
            "text": format!("Session title set to: {title}"),
        }],
        "structuredContent": { "title": title, "updated": true },
        "isError": false,
    }))
}

/// The session id of the agent that invoked this MCP tool, across agent sources.
///
/// For **Claude Code** the id arrives via `CLAUDE_CODE_SESSION_ID` (verified
/// empirically: the value equals the transcript UUID, e.g.
/// `~/.claude/projects/<proj>/<uuid>.jsonl`, which is exactly what
/// `session::parse_session_info` keys `SessionInfo.id` on). For **Codex** — which
/// exposes no session-id env of its own — Fleet stamps the MCP server's env at
/// spawn: a resume carries `FLEET_SESSION_ID` (the thread id, known up front), a
/// new spawn carries `FLEET_CODEX_LAUNCH_TOKEN` (resolved to the thread id once
/// `thread.started` lands). All three collapse into the one precedence in
/// [`crate::codex_launch::resolve_fleet_session_id_from_env`], shared with the
/// `fleet` CLI. Returning a real id lets the desktop decision panel resolve which
/// session a `fleet__ask` / `fleet__render_a2ui` card belongs to so its inline
/// SessionDetail side column can open.
fn current_session_id() -> String {
    crate::codex_launch::resolve_fleet_session_id_from_env().unwrap_or_default()
}

/// Workspace-name chip for the agent's `CLAUDE_PROJECT_DIR`. Empty when unset.
fn project_dir_workspace_name() -> String {
    workspace_name_from_project_dir(std::env::var("CLAUDE_PROJECT_DIR").ok().as_deref())
}

/// Pure core of [`project_dir_workspace_name`]: route the project dir through
/// the shared helper so `.worktrees/<task-id>` collapses to the repo name (and
/// the chat workspace is renamed) — matching the session list, not the raw
/// basename. Split out so it is testable without mutating process env.
fn workspace_name_from_project_dir(dir: Option<&str>) -> String {
    dir.map(crate::session::workspace_name).unwrap_or_default()
}

/// RAII clear for the in-flight-ask marker: registered at the top of a card
/// producer, dropped (cleared) on every normal return so the session frees up as
/// soon as the card resolves. On the timeout→park path the process is SIGINT'd
/// before Drop runs — that is fine: the parked card then guards re-entry, and a
/// marker left by a dead pid is treated as stale on the next register.
struct InflightGuard {
    session_id: String,
    request_id: String,
}

impl InflightGuard {
    fn new(session_id: &str, request_id: &str) -> Self {
        Self { session_id: session_id.to_string(), request_id: request_id.to_string() }
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        crate::parked::clear_inflight_ask(&self.session_id, &self.request_id);
    }
}

fn handle_fleet_ask_call(params: &Value) -> Result<Value, JsonRpcError> {
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    let questions: Vec<crate::mcp_ipc::FleetAskQuestion> =
        match args.get("questions").cloned() {
            Some(q) => serde_json::from_value(q).map_err(|e| JsonRpcError {
                code: -32602,
                message: format!("Invalid `questions` payload: {e}"),
            })?,
            None => {
                return Err(JsonRpcError {
                    code: -32602,
                    message: "Missing `questions` argument".into(),
                });
            }
        };
    if questions.is_empty() {
        return Err(JsonRpcError {
            code: -32602,
            message: "`questions` must not be empty".into(),
        });
    }

    // Answerability guard: `header`/`question` are the only serde-required
    // fields, so a question with no `options`, no `html` and no `formFields`
    // deserialises fine yet renders as a free-text-only card — no clickable or
    // fillable answer surface. The native AskUserQuestion tool forbids this (it
    // marks `options` required with `minItems: 2`); restore that contract here
    // while still allowing `fleet__ask`'s pure-html and pure-form extension
    // cards. A single lone option is likewise unanswerable-by-choice, so
    // require at least two when options is the only surface.
    if let Some(idx) = questions.iter().position(|q| {
        q.options.len() < 2 && q.html.is_none() && q.form_fields.is_empty()
    }) {
        return Err(JsonRpcError {
            code: -32602,
            message: format!(
                "Question {} has no answer surface: give it at least 2 `options`, or `html`, or `formFields`.",
                idx + 1
            ),
        });
    }

    // Heartbeat check — if no Fleet consumer is alive, refuse the call so
    // Claude Code's agent can choose to fall back to AskUserQuestion.
    let status = crate::consumer_heartbeat::consumer_status(heartbeat_window());
    if !status.is_alive() {
        return Ok(tool_error(format!(
            "Fleet consumer not running (status: {status}). Start the Fleet desktop app or `fleet serve`, or use the native AskUserQuestion tool."
        )));
    }

    // Re-entry guard: this session already has a question parked and waiting for
    // the user. An agent that got here anyway ignored the stop notice from that
    // one — hand it straight back rather than stacking a second card the user
    // has to dismiss.
    let session_id = current_session_id();
    if crate::parked::has_parked_for_session(&session_id) {
        return Ok(tool_error(crate::parked::STOP_NOTICE.to_string()));
    }

    let request_id = crate::guard::new_request_id();

    // In-flight guard: `has_parked_for_session` only catches a card that already
    // timed out into the parked store — it does nothing while a card is actively
    // waiting. A double-submitted prompt can briefly put a second `claude`
    // process on this session, each blocking on its own `fleet__ask`, stacking
    // two live cards before either parks. Register ourselves; if another live
    // process already holds an in-flight ask for this session, hand back the stop
    // notice instead of a duplicate card. `_inflight` clears our marker on every
    // normal return (Drop); the park path below SIGINTs us, but then the parked
    // card guards re-entry and a dead-pid marker self-heals.
    if !crate::parked::register_inflight_ask(&session_id, &request_id) {
        return Ok(tool_error(crate::parked::STOP_NOTICE.to_string()));
    }
    let _inflight = InflightGuard::new(&session_id, &request_id);

    let workspace_name = project_dir_workspace_name();

    // Optional review docs (`.md` files / wiki entries to show next to the card).
    // A malformed array is non-fatal: drop it rather than failing the whole ask.
    let review_docs: Vec<crate::mcp_ipc::ReviewDoc> = args
        .get("reviewDocs")
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    let mut req = crate::mcp_ipc::FleetAskRequest {
        id: request_id.clone(),
        session_id,
        workspace_name,
        ai_title: None,
        timestamp: chrono::Utc::now().to_rfc3339(),
        parked: false,
        questions,
        review_docs,
    };

    // Resolve `file` doc refs against the agent's cwd so the desktop (a separate
    // process) can read them back live when the card renders.
    let project_dir = std::env::var("CLAUDE_PROJECT_DIR").ok();
    crate::mcp_ipc::resolve_review_docs(&mut req, project_dir.as_deref().map(std::path::Path::new));

    // Give every `html` preview the height-reporting script before it is staged
    // anywhere, so the card's sandboxed iframe can size itself to the content
    // instead of clipping it into a fixed-height box. Must precede
    // `ingest_images`, which copies `q.html` verbatim into the served index.html.
    crate::mcp_ipc::normalize_html(&mut req);

    // Copy any referenced images into Fleet's persistent decision-asset store
    // and blank their local paths, so the card can load them through the
    // `fleet-decision://` protocol instead of base64-inlined `html`.
    if let Err(e) = crate::mcp_ipc::ingest_images(&mut req) {
        return Ok(tool_error(format!("Failed to stage fleet__ask images: {e}")));
    }

    if let Err(e) = crate::mcp_ipc::write_request(&req) {
        return Ok(tool_error(format!("Failed to queue fleet__ask request: {e}")));
    }

    // Poll for the response, watching the consumer heartbeat so we exit
    // promptly if the desktop / fleet serve dies mid-flight.
    let timeout = fleet_ask_timeout();
    let liveness = heartbeat_window();
    let poll = std::time::Duration::from_millis(200);
    let started = Instant::now();
    let response = loop {
        if let Some(r) = crate::mcp_ipc::try_read_response(&request_id) {
            break Some(r);
        }
        if started.elapsed() > timeout {
            break None;
        }
        if !crate::consumer_heartbeat::consumer_status(liveness).is_alive() {
            crate::mcp_ipc::cleanup(&request_id);
            persist_fleet_ask_history(
                &req,
                crate::decision_history::FleetAskOutcome::HeartbeatLost,
                std::collections::BTreeMap::new(),
            );
            return Ok(tool_error(
                "Fleet consumer heartbeat lost while waiting for your answer.".into(),
            ));
        }
        std::thread::sleep(poll);
    };

    crate::mcp_ipc::cleanup(&request_id);

    let Some(resp) = response else {
        persist_fleet_ask_history(
            &req,
            crate::decision_history::FleetAskOutcome::Timeout,
            std::collections::BTreeMap::new(),
        );
        // Fleet-owned session: keep the question alive as a parked card and cut
        // this turn short, instead of handing the agent an error it will "route
        // around" by pressing on without an answer. `park_and_stop` SIGINTs the
        // CLI, so control may never come back from this call.
        if crate::parked::park_and_stop(
            &request_id,
            crate::parked::ParkedKind::FleetAsk,
            &req.session_id,
            &req,
        ) {
            return Ok(tool_error(crate::parked::STOP_NOTICE.to_string()));
        }
        return Ok(tool_error(format!(
            "No response from Fleet after {}s.",
            timeout.as_secs()
        )));
    };

    if resp.cancelled {
        persist_fleet_ask_history(
            &req,
            crate::decision_history::FleetAskOutcome::Cancelled,
            std::collections::BTreeMap::new(),
        );
        return Ok(tool_error(
            "User cancelled the fleet__ask Decision Card.".into(),
        ));
    }

    persist_fleet_ask_history(
        &req,
        crate::decision_history::FleetAskOutcome::Answered,
        resp.answers.clone(),
    );

    // Pack answers as JSON so the agent can parse structured form values.
    let answers_json = serde_json::to_string(&resp.answers).unwrap_or_else(|_| "{}".into());
    Ok(json!({
        "content": [{
            "type": "text",
            "text": answers_json,
        }],
        "structuredContent": { "answers": resp.answers },
        "isError": false,
    }))
}

/// Best-effort persistence of a resolved fleet__ask card into the per-session
/// decision history JSONL. Failures only log — the agent's tool response is
/// the authoritative output and must not block on a disk hiccup.
fn persist_fleet_ask_history(
    req: &crate::mcp_ipc::FleetAskRequest,
    outcome: crate::decision_history::FleetAskOutcome,
    answers: std::collections::BTreeMap<String, String>,
) {
    let rec = crate::decision_history::build_fleet_ask_record(
        req,
        outcome,
        answers,
        chrono::Utc::now().to_rfc3339(),
    );
    if let Err(e) =
        crate::decision_history::append_record(&crate::decision_history::DecisionHistoryRecord::FleetAsk(rec))
    {
        eprintln!("decision_history append (fleet__ask): {e}");
    }
}

fn handle_a2ui_render_call(params: &Value) -> Result<Value, JsonRpcError> {
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    let message_tree = match args.get("messageTree").cloned() {
        Some(t) if t.is_object() => t,
        Some(_) => {
            return Err(JsonRpcError {
                code: -32602,
                message: "`messageTree` must be a JSON object".into(),
            });
        }
        None => {
            return Err(JsonRpcError {
                code: -32602,
                message: "Missing `messageTree` argument".into(),
            });
        }
    };

    // Heartbeat — same fall-back hint as fleet__ask so the agent can pick
    // AskUserQuestion or a degraded text response when no consumer is up.
    let status = crate::consumer_heartbeat::consumer_status(heartbeat_window());
    if !status.is_alive() {
        return Ok(tool_error(format!(
            "Fleet consumer not running (status: {status}). Start the Fleet desktop app or `fleet serve`, or fall back to fleet__ask / AskUserQuestion."
        )));
    }

    // Same re-entry guard as fleet__ask: one parked question per session.
    let session_id = current_session_id();
    if crate::parked::has_parked_for_session(&session_id) {
        return Ok(tool_error(crate::parked::STOP_NOTICE.to_string()));
    }

    let request_id = crate::guard::new_request_id();

    // Same in-flight guard as fleet__ask: a second live process must not stack a
    // concurrent A2UI card on a session that already has one waiting.
    if !crate::parked::register_inflight_ask(&session_id, &request_id) {
        return Ok(tool_error(crate::parked::STOP_NOTICE.to_string()));
    }
    let _inflight = InflightGuard::new(&session_id, &request_id);

    let workspace_name = project_dir_workspace_name();

    let req = crate::mcp_a2ui_ipc::A2uiRenderRequest {
        id: request_id.clone(),
        session_id,
        workspace_name,
        ai_title: None,
        timestamp: chrono::Utc::now().to_rfc3339(),
        parked: false,
        message_tree,
    };

    if let Err(e) = crate::mcp_a2ui_ipc::write_request(&req) {
        return Ok(tool_error(format!(
            "Failed to queue fleet__render_a2ui request: {e}"
        )));
    }

    let timeout = fleet_ask_timeout();
    let liveness = heartbeat_window();
    let poll = std::time::Duration::from_millis(200);
    let started = Instant::now();
    let response = loop {
        if let Some(r) = crate::mcp_a2ui_ipc::try_read_response(&request_id) {
            break Some(r);
        }
        if started.elapsed() > timeout {
            break None;
        }
        if !crate::consumer_heartbeat::consumer_status(liveness).is_alive() {
            crate::mcp_a2ui_ipc::cleanup(&request_id);
            return Ok(tool_error(
                "Fleet consumer heartbeat lost while waiting for your A2UI action.".into(),
            ));
        }
        std::thread::sleep(poll);
    };

    crate::mcp_a2ui_ipc::cleanup(&request_id);

    let Some(resp) = response else {
        if crate::parked::park_and_stop(
            &request_id,
            crate::parked::ParkedKind::A2uiRender,
            &req.session_id,
            &req,
        ) {
            return Ok(tool_error(crate::parked::STOP_NOTICE.to_string()));
        }
        return Ok(tool_error(format!(
            "No A2UI action from Fleet after {}s.",
            timeout.as_secs()
        )));
    };

    if resp.cancelled {
        return Ok(tool_error(
            "User cancelled the fleet__render_a2ui Decision Card.".into(),
        ));
    }

    let payload = json!({
        "actionName": resp.action_name,
        "actionContext": resp.action_context,
    });
    let text = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
    Ok(json!({
        "content": [{
            "type": "text",
            "text": text,
        }],
        "structuredContent": payload,
        "isError": false,
    }))
}

fn tool_error(message: String) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true,
    })
}

/// Handle `fleet__permission_prompt` — Claude Code's `--permission-prompt-tool`
/// bridge for headless sessions.
///
/// Unlike the other tools, EVERY exit path must return a *valid contract
/// payload* (`{"behavior":"allow"|"deny",...}`) as plain text content, never
/// `tool_error`: the caller is Claude Code's permission machinery, and a
/// malformed result would surface as an opaque tool failure instead of a
/// clean denial the agent can read and route around. Verified empirically on
/// CLI 2.1.181: `{"behavior":"allow","updatedInput":{...}}` executes the
/// tool, `{"behavior":"deny","message":"..."}` blocks it with the message.
fn handle_permission_prompt_call(params: &Value) -> Result<Value, JsonRpcError> {
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    let Some(tool_name) = args.get("tool_name").and_then(|v| v.as_str()) else {
        return Err(JsonRpcError {
            code: -32602,
            message: "Missing `tool_name` argument".into(),
        });
    };
    let tool_input = args.get("input").cloned().unwrap_or(json!({}));
    let tool_use_id = args
        .get("tool_use_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // No live Fleet consumer → nobody can approve. Deny with a reason the
    // agent can act on (there is no native UI to fall back to in headless
    // `-p` mode, so a fast deny beats a doomed 600s wait).
    let status = crate::consumer_heartbeat::consumer_status(heartbeat_window());
    if !status.is_alive() {
        return Ok(permission_deny(format!(
            "Fleet is not running to approve this action (consumer status: {status}); denied by default."
        )));
    }

    let request_id = crate::guard::new_request_id();
    let req = crate::permission_prompt_ipc::PermissionPromptRequest {
        id: request_id.clone(),
        session_id: current_session_id(),
        workspace_name: project_dir_workspace_name(),
        ai_title: None,
        timestamp: chrono::Utc::now().to_rfc3339(),
        tool_name: tool_name.to_string(),
        tool_input: tool_input.clone(),
        tool_use_id,
    };

    if let Err(e) = crate::permission_prompt_ipc::write_request(&req) {
        return Ok(permission_deny(format!(
            "Fleet could not queue the permission request ({e}); denied by default."
        )));
    }

    let timeout = fleet_ask_timeout();
    let liveness = heartbeat_window();
    let poll = std::time::Duration::from_millis(200);
    let started = Instant::now();
    let response = loop {
        if let Some(r) = crate::permission_prompt_ipc::try_read_response(&request_id) {
            break Some(r);
        }
        if started.elapsed() > timeout {
            break None;
        }
        if !crate::consumer_heartbeat::consumer_status(liveness).is_alive() {
            crate::permission_prompt_ipc::cleanup(&request_id);
            return Ok(permission_deny(
                "Fleet went away while waiting for permission approval; denied by default.".into(),
            ));
        }
        std::thread::sleep(poll);
    };

    crate::permission_prompt_ipc::cleanup(&request_id);

    let Some(resp) = response else {
        return Ok(permission_deny(format!(
            "No permission decision from the user after {}s; denied by default.",
            timeout.as_secs()
        )));
    };

    match resp.decision {
        crate::permission_prompt_ipc::PermissionPromptDecision::Allow => {
            let payload = json!({ "behavior": "allow", "updatedInput": tool_input });
            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into()),
                }],
                "isError": false,
            }))
        }
        crate::permission_prompt_ipc::PermissionPromptDecision::Deny => {
            let msg = resp
                .reason
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|r| format!("Denied by user via Fleet Decision Panel: {r}"))
                .unwrap_or_else(|| "Denied by user via Fleet Decision Panel.".to_string());
            Ok(permission_deny(msg))
        }
    }
}

/// Build a `--permission-prompt-tool` deny payload. The `behavior`/`message`
/// text contract is what Claude Code parses; `isError` stays false because
/// the tool itself succeeded — the *permission* was denied.
fn permission_deny(message: String) -> Value {
    let payload = json!({ "behavior": "deny", "message": message });
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into()),
        }],
        "isError": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(line: &str) -> Option<Value> {
        let raw = handle_line(line)?;
        serde_json::from_str(&raw).ok()
    }

    #[test]
    fn project_dir_workspace_name_collapses_worktree_to_repo() {
        // A worktree CLAUDE_PROJECT_DIR must chip the decision/permission card
        // with the repo name, not the task-id leaf — same contract as the
        // session list. Buggy behaviour took only PathBuf::file_name().
        assert_eq!(
            workspace_name_from_project_dir(Some(
                "/Users/x/claude-fleet/.worktrees/some-plan"
            )),
            "claude-fleet"
        );
        // Non-worktree path still yields its basename; unset stays empty.
        assert_eq!(
            workspace_name_from_project_dir(Some("/Users/x/claude-fleet")),
            "claude-fleet"
        );
        assert_eq!(workspace_name_from_project_dir(None), "");
    }

    #[test]
    fn initialize_returns_protocol_version() {
        let resp = call(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
            .expect("response");
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(resp["result"]["serverInfo"]["name"], SERVER_NAME);
    }

    #[test]
    fn tools_list_returns_all_tools() {
        // Non-Fleet session: only the four UI tools, regardless of ambient env
        // (the JSON-RPC path gates on `session_is_fleet_owned`, but the pure
        // builder lets us assert deterministically).
        let result = tools_list_result(false);
        let tools = result["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 4, "non-Fleet session sees only the UI tools");
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"fleet__ask"));
        assert!(names.contains(&"fleet__render_a2ui"));
        assert!(names.contains(&"fleet__permission_prompt"));
        let pp = tools
            .iter()
            .find(|t| t["name"] == "fleet__permission_prompt")
            .unwrap();
        // Claude Code sends snake_case arguments — the schema must advertise
        // the exact field names observed on the wire (CLI 2.1.181).
        assert!(pp["inputSchema"]["properties"].get("tool_name").is_some());
        assert!(pp["inputSchema"]["properties"].get("input").is_some());
        let ask = tools.iter().find(|t| t["name"] == "fleet__ask").unwrap();
        let schema = &ask["inputSchema"]["properties"]["questions"]["items"]["properties"];
        assert!(schema.get("html").is_some(), "html field present in fleet__ask schema");
        assert!(
            schema.get("formFields").is_some(),
            "formFields field present in fleet__ask schema (camelCase)"
        );
        let a2ui = tools
            .iter()
            .find(|t| t["name"] == "fleet__render_a2ui")
            .unwrap();
        assert_eq!(a2ui["inputSchema"]["required"][0], "messageTree");

        let set_title = tools
            .iter()
            .find(|t| t["name"] == "fleet__set_session_title")
            .expect("session-title tool must be advertised");
        assert_eq!(set_title["inputSchema"]["required"][0], "title");
        assert_eq!(
            set_title["inputSchema"]["properties"]["title"]["type"],
            "string"
        );
    }

    #[test]
    fn fleet_owned_session_also_sees_the_control_tools() {
        // Conditional registration: a Fleet-owned session gets the four UI tools
        // PLUS the six control tools; a non-Fleet session (tested above) does not.
        let result = tools_list_result(true);
        let tools = result["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 10, "Fleet-owned session sees UI + control tools");
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for control in crate::mcp_control::CONTROL_TOOL_NAMES {
            assert!(names.contains(&control), "{control} must be advertised to Fleet sessions");
        }
        // The UI tools remain present alongside them.
        assert!(names.contains(&"fleet__ask"));
    }

    #[test]
    fn set_session_title_call_persists_for_current_session() {
        let _guard = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet-mcp-session-title-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let previous_home = std::env::var_os("FLEET_HOME");
        let previous_session = std::env::var_os("CLAUDE_CODE_SESSION_ID");
        unsafe {
            std::env::set_var("FLEET_HOME", &tmp);
            std::env::set_var("CLAUDE_CODE_SESSION_ID", "session-title-test");
        }

        let req = json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/call",
            "params": {
                "name": "fleet__set_session_title",
                "arguments": { "title": "  MCP 自动命名会话  " }
            }
        });
        let resp = call(&req.to_string()).expect("response");
        assert!(resp.get("error").is_none(), "expected ok envelope, got {resp}");
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(
            crate::session_title::read("session-title-test").as_deref(),
            Some("MCP 自动命名会话")
        );

        unsafe {
            match previous_home {
                Some(value) => std::env::set_var("FLEET_HOME", value),
                None => std::env::remove_var("FLEET_HOME"),
            }
            match previous_session {
                Some(value) => std::env::set_var("CLAUDE_CODE_SESSION_ID", value),
                None => std::env::remove_var("CLAUDE_CODE_SESSION_ID"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn set_session_title_rejects_blank_title() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "tools/call",
            "params": {
                "name": "fleet__set_session_title",
                "arguments": { "title": "   " }
            }
        });
        let resp = call(&req.to_string()).expect("response");
        assert_eq!(resp["error"]["code"], -32602);
        assert!(
            resp["error"]["message"]
                .as_str()
                .unwrap()
                .contains("blank")
        );
    }

    #[test]
    fn tools_call_without_consumer_returns_structured_error() {
        // Without a live Fleet consumer, tools/call returns isError=true with
        // a textual hint pointing at the native AskUserQuestion fallback.
        // Force an empty FLEET_HOME (no heartbeat file) so the consumer-check
        // resolves to "not alive" regardless of the dev's machine state.
        let _guard = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet-ask-no-consumer-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&tmp);
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialised by `fleet_home_lock`; no other thread is
        // touching FLEET_HOME for the duration of this test.
        unsafe { std::env::set_var("FLEET_HOME", &tmp) };

        let req = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "fleet__ask",
                "arguments": {
                    "questions": [{
                        "question": "hi",
                        "header": "H",
                        "multiSelect": false,
                        "options": [
                            {"label": "yes", "description": "affirm"},
                            {"label": "no", "description": "deny"}
                        ]
                    }]
                }
            }
        });
        let resp = call(&req.to_string()).expect("response");

        // SAFETY: restore prior state under the same lock.
        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);

        // JSON-RPC envelope succeeded (no `error` member); the tool itself
        // reports the failure via `isError: true` per MCP spec.
        assert!(resp.get("error").is_none(), "expected ok envelope, got {resp}");
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("Fleet consumer"),
            "expected hint about Fleet consumer, got: {text}"
        );
    }

    #[test]
    fn tools_call_with_missing_questions_field_errors() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {
                "name": "fleet__ask",
                "arguments": {}
            }
        });
        let resp = call(&req.to_string()).expect("response");
        assert_eq!(resp["error"]["code"], -32602);
    }

    #[test]
    fn tools_call_with_empty_questions_array_errors() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "tools/call",
            "params": {
                "name": "fleet__ask",
                "arguments": { "questions": [] }
            }
        });
        let resp = call(&req.to_string()).expect("response");
        assert_eq!(resp["error"]["code"], -32602);
    }

    #[test]
    fn tools_call_with_unanswerable_question_errors() {
        // A question with no options, no html and no formFields has no answer
        // surface — the card would render as free-text-only. The native
        // AskUserQuestion tool forbids this (options is required, minItems 2);
        // `fleet__ask` must restore that contract for the non-form/non-html
        // case. Force FLEET_HOME empty so that, absent the guard, the call would
        // otherwise fall through to the (not-alive) consumer check and return an
        // `isError` envelope rather than a JSON-RPC error — proving the guard,
        // not the heartbeat, is what rejects it.
        let _guard = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir()
            .join(format!("fleet-ask-unanswerable-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialised by `fleet_home_lock`.
        unsafe { std::env::set_var("FLEET_HOME", &tmp) };

        let req = json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "tools/call",
            "params": {
                "name": "fleet__ask",
                "arguments": {
                    "questions": [{
                        "question": "要不要一并清掉?",
                        "header": "清库完成",
                        "multiSelect": false
                    }]
                }
            }
        });
        let resp = call(&req.to_string()).expect("response");

        // SAFETY: restore under the same lock.
        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);

        assert_eq!(resp["error"]["code"], -32602);
        let msg = resp["error"]["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("options") && msg.contains("html") && msg.contains("formFields"),
            "error should name the three answer surfaces, got: {msg}"
        );
    }

    #[test]
    fn permission_prompt_without_consumer_denies_with_contract_json() {
        // The permission tool must NEVER return isError — every failure path
        // has to be a parseable {"behavior":"deny"} payload, because the
        // caller is Claude Code's permission machinery, not the agent.
        let _guard = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet-pp-no-consumer-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&tmp);
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialised by `fleet_home_lock` (matches fleet__ask sibling test).
        unsafe { std::env::set_var("FLEET_HOME", &tmp) };

        let req = json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "tools/call",
            "params": {
                "name": "fleet__permission_prompt",
                "arguments": {
                    "tool_name": "Write",
                    "input": { "file_path": "/tmp/x", "content": "y" },
                    "tool_use_id": "toolu_test"
                }
            }
        });
        let resp = call(&req.to_string()).expect("response");

        // SAFETY: restore prior state under the same lock.
        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);

        assert!(resp.get("error").is_none(), "expected ok envelope, got {resp}");
        assert_eq!(resp["result"]["isError"], false);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(text).expect("contract JSON");
        assert_eq!(payload["behavior"], "deny");
        assert!(
            payload["message"].as_str().unwrap().contains("denied by default"),
            "unexpected message: {payload}"
        );
    }

    #[test]
    fn permission_prompt_missing_tool_name_errors() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 12,
            "method": "tools/call",
            "params": {
                "name": "fleet__permission_prompt",
                "arguments": { "input": {} }
            }
        });
        let resp = call(&req.to_string()).expect("response");
        assert_eq!(resp["error"]["code"], -32602);
    }

    #[test]
    fn tools_call_a2ui_without_consumer_returns_structured_error() {
        let _guard = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet-a2ui-no-consumer-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&tmp);
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialised by `fleet_home_lock` (matches fleet__ask sibling test).
        unsafe { std::env::set_var("FLEET_HOME", &tmp) };

        let req = json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/call",
            "params": {
                "name": "fleet__render_a2ui",
                "arguments": {
                    "messageTree": {
                        "surfaceUpdate": {
                            "surfaceId": "s",
                            "root": { "Card": { "id": "c" } }
                        }
                    }
                }
            }
        });
        let resp = call(&req.to_string()).expect("response");

        // SAFETY: restore prior state under the same lock.
        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);

        assert!(resp.get("error").is_none(), "expected ok envelope, got {resp}");
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("Fleet consumer"),
            "expected hint about Fleet consumer, got: {text}"
        );
    }

    #[test]
    fn tools_call_a2ui_with_missing_message_tree_errors() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "tools/call",
            "params": {
                "name": "fleet__render_a2ui",
                "arguments": {}
            }
        });
        let resp = call(&req.to_string()).expect("response");
        assert_eq!(resp["error"]["code"], -32602);
    }

    #[test]
    fn tools_call_a2ui_with_non_object_message_tree_errors() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 11,
            "method": "tools/call",
            "params": {
                "name": "fleet__render_a2ui",
                "arguments": { "messageTree": "not-an-object" }
            }
        });
        let resp = call(&req.to_string()).expect("response");
        assert_eq!(resp["error"]["code"], -32602);
    }

    #[test]
    fn tools_call_unknown_tool_errors() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": { "name": "bogus", "arguments": {} }
        });
        let resp = call(&req.to_string()).expect("response");
        assert_eq!(resp["error"]["code"], -32602);
    }

    #[test]
    fn unknown_method_returns_method_not_found() {
        let resp = call(r#"{"jsonrpc":"2.0","id":5,"method":"unknown/x","params":{}}"#)
            .expect("response");
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[test]
    fn notification_returns_no_response() {
        let resp = handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#);
        assert!(resp.is_none());
    }

    #[test]
    fn malformed_json_silently_ignored() {
        assert!(handle_line("not json").is_none());
        assert!(handle_line("").is_none());
    }

    /// Regression: `current_session_id` must read the env var Claude Code
    /// actually injects into MCP stdio servers — `CLAUDE_CODE_SESSION_ID` —
    /// not the look-alike `CLAUDE_SESSION_ID`. When it read the wrong name the
    /// id came back empty, so every `fleet__ask` / `fleet__render_a2ui` card
    /// carried `sessionId: ""`, which gated off the decision panel's inline
    /// SessionDetail side column (DecisionPanel.tsx `PastHistoryStrip` only
    /// renders when `active.request.sessionId` is truthy).
    ///
    /// `set_var`/`remove_var` mutate process-global state; no other test reads
    /// `CLAUDE_CODE_SESSION_ID`, so this stays self-contained. The wrong-name
    /// var is also cleared to prove the value comes from the right source.
    #[test]
    fn current_session_id_reads_claude_code_session_id() {
        unsafe {
            std::env::remove_var("CLAUDE_SESSION_ID");
            std::env::set_var("CLAUDE_CODE_SESSION_ID", "sess-abc-123");
        }
        let got = current_session_id();
        unsafe {
            std::env::remove_var("CLAUDE_CODE_SESSION_ID");
        }
        assert_eq!(
            got, "sess-abc-123",
            "current_session_id must read CLAUDE_CODE_SESSION_ID, not CLAUDE_SESSION_ID"
        );
    }
}
