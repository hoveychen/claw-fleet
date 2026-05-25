//! Minimal MCP (Model Context Protocol) stdio server exposing `fleet__ask`.
//!
//! Implements JSON-RPC 2.0 over stdin/stdout (line-delimited JSON).
//! Methods: `initialize`, `notifications/initialized`, `tools/list`, `tools/call`.
//!
//! P1 scope: skeleton with a `fleet__ask` placeholder that echoes its arguments.
//! P2 will wire `tools/call` to the Fleet desktop backend over IPC.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

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
        "tools/list" => Ok(json!({ "tools": [fleet_ask_tool_def()] })),
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
        "description": "Ask the user one or more questions through Fleet's Decision Panel. Schema mirrors Claude Code's native AskUserQuestion plus two optional fields: `html` (HTML preview, rendered in a sandboxed iframe) and `formFields` (structured input fields). [P1: dummy echo]",
        "inputSchema": crate::mcp_ipc::fleet_ask_input_schema(),
    })
}

fn handle_tool_call(params: &Value) -> Result<Value, JsonRpcError> {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name != "fleet__ask" {
        return Err(JsonRpcError {
            code: -32602,
            message: format!("Unknown tool: {}", name),
        });
    }
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    Ok(json!({
        "content": [{
            "type": "text",
            "text": format!(
                "[P1 dummy] fleet__ask received: {}",
                serde_json::to_string(&args).unwrap_or_default()
            )
        }],
        "isError": false
    }))
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
    fn tools_list_returns_fleet_ask() {
        let resp = call(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#)
            .expect("response");
        let tools = resp["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "fleet__ask");
        let schema = &tools[0]["inputSchema"]["properties"]["questions"]["items"]["properties"];
        assert!(schema.get("html").is_some(), "html field present in schema");
        assert!(
            schema.get("formFields").is_some(),
            "formFields field present in schema (camelCase)"
        );
    }

    #[test]
    fn tools_call_echoes_arguments() {
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
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("[P1 dummy]"));
        assert!(text.contains("hi"));
        assert_eq!(resp["result"]["isError"], false);
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
