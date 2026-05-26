//! Minimal MCP (Model Context Protocol) stdio server exposing `fleet__ask`.
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
        "tools/list" => Ok(json!({
            "tools": [fleet_ask_tool_def(), a2ui_render_tool_def()]
        })),
        "tools/call" => handle_tool_call(params),
        other => Err(JsonRpcError {
            code: -32601,
            message: format!("Method not found: {}", other),
        }),
    }
}

fn fleet_ask_tool_def() -> Value {
    json!({
        "name": "fleet__ask",
        "description": "Ask the user one or more questions through Fleet's Decision Panel. Schema mirrors Claude Code's native AskUserQuestion plus two optional fields: `html` (HTML preview, rendered in a sandboxed iframe) and `formFields` (structured input fields).",
        "inputSchema": crate::mcp_ipc::fleet_ask_input_schema(),
    })
}

fn a2ui_render_tool_def() -> Value {
    json!({
        "name": "fleet__render_a2ui",
        "description": "Render an A2UI v0.9 agent-to-client message tree in Fleet's Decision Panel via the official @a2ui/react renderer, then return the user's Action payload. Reach for this when the UI needs richer layout than fleet__ask's flat formFields (Tabs / Modal / Card / Video / etc.). For simple form or option asks, prefer fleet__ask.",
        "inputSchema": crate::mcp_a2ui_ipc::a2ui_render_input_schema(),
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
        other => Err(JsonRpcError {
            code: -32602,
            message: format!("Unknown tool: {}", other),
        }),
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

    // Heartbeat check — if no Fleet consumer is alive, refuse the call so
    // Claude Code's agent can choose to fall back to AskUserQuestion.
    let status = crate::consumer_heartbeat::consumer_status(heartbeat_window());
    if !status.is_alive() {
        return Ok(tool_error(format!(
            "Fleet consumer not running (status: {status}). Start the Fleet desktop app or `fleet serve`, or use the native AskUserQuestion tool."
        )));
    }

    let request_id = crate::guard::new_request_id();
    let session_id = std::env::var("CLAUDE_SESSION_ID").unwrap_or_default();
    let workspace_name = std::env::var("CLAUDE_PROJECT_DIR")
        .ok()
        .and_then(|p| {
            std::path::PathBuf::from(p)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
        })
        .unwrap_or_default();

    let req = crate::mcp_ipc::FleetAskRequest {
        id: request_id.clone(),
        session_id,
        workspace_name,
        ai_title: None,
        timestamp: chrono::Utc::now().to_rfc3339(),
        questions,
    };

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
            return Ok(tool_error(
                "Fleet consumer heartbeat lost while waiting for your answer.".into(),
            ));
        }
        std::thread::sleep(poll);
    };

    crate::mcp_ipc::cleanup(&request_id);

    let Some(resp) = response else {
        return Ok(tool_error(format!(
            "No response from Fleet after {}s.",
            timeout.as_secs()
        )));
    };

    if resp.cancelled {
        return Ok(tool_error(
            "User cancelled the fleet__ask Decision Card.".into(),
        ));
    }

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

    let request_id = crate::guard::new_request_id();
    let session_id = std::env::var("CLAUDE_SESSION_ID").unwrap_or_default();
    let workspace_name = std::env::var("CLAUDE_PROJECT_DIR")
        .ok()
        .and_then(|p| {
            std::path::PathBuf::from(p)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
        })
        .unwrap_or_default();

    let req = crate::mcp_a2ui_ipc::A2uiRenderRequest {
        id: request_id.clone(),
        session_id,
        workspace_name,
        ai_title: None,
        timestamp: chrono::Utc::now().to_rfc3339(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn call(line: &str) -> Option<Value> {
        let raw = handle_line(line)?;
        serde_json::from_str(&raw).ok()
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
    fn tools_list_returns_both_tools() {
        let resp = call(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#)
            .expect("response");
        let tools = resp["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 2, "expected fleet__ask + fleet__render_a2ui");
        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"fleet__ask"));
        assert!(names.contains(&"fleet__render_a2ui"));
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
                        "multiSelect": false
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
}
