//! IPC protocol between the `fleet mcp` server (a Claude Code child process)
//! and the local `fleet` process for the `fleet__render_a2ui` tool.
//!
//! Wire format: file-based IPC at `~/.fleet/fleet-render-a2ui/<id>.json`
//! (request) and `~/.fleet/fleet-render-a2ui/<id>.response.json` (response).
//! Direct mirror of [`crate::mcp_ipc`] (fleet__ask) so the desktop's watcher,
//! orphan filter and cleanup can stay parallel structures.
//!
//! These types are the single source of truth for the `fleet__render_a2ui`
//! MCP schema; [`a2ui_render_input_schema`] mirrors the request shape in
//! JSONSchema form for the MCP `tools/list` response. `message_tree` is left
//! as a passthrough `serde_json::Value` — the A2UI v0.9 schema is large and
//! evolving upstream, so we defer validation to the renderer.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2uiRenderRequest {
    pub id: String,
    /// Originating Claude Code session id (same role as `FleetAskRequest::session_id`).
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub workspace_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_title: Option<String>,
    #[serde(default)]
    pub timestamp: String,
    /// Timed out and parked — see [`crate::mcp_ipc::FleetAskRequest::parked`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub parked: bool,
    /// The A2UI v0.9 agent-to-client message tree, passed through verbatim
    /// to `@a2ui/web_core`'s `MessageProcessor` on the desktop side.
    pub message_tree: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2uiRenderResponse {
    pub id: String,
    /// `userAction.name` from the rendered surface — `None` when the user
    /// closed the card without firing an Action component.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_name: Option<String>,
    /// Flattened `userAction.context` after BoundValue resolution. Non-string
    /// values are stringified by the frontend so the wire stays as a
    /// `BTreeMap<String, String>` — same shape `FleetAskResponse::answers`
    /// uses, so agents that learned one tool's wire can read both.
    #[serde(default)]
    pub action_context: BTreeMap<String, String>,
    #[serde(default)]
    pub cancelled: bool,
}

// ── File-based IPC (mirror of `crate::mcp_ipc`) ──────────────────────────────

fn fleet_a2ui_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("fleet-render-a2ui"))
}

fn request_path(id: &str) -> Option<PathBuf> {
    fleet_a2ui_dir().map(|d| d.join(format!("{id}.json")))
}

fn response_path(id: &str) -> Option<PathBuf> {
    fleet_a2ui_dir().map(|d| d.join(format!("{id}.response.json")))
}

/// Write a fleet__render_a2ui request file. Called by the MCP server when it
/// handles a `tools/call` for `fleet__render_a2ui`.
pub fn write_request(req: &A2uiRenderRequest) -> Result<(), String> {
    let dir = fleet_a2ui_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create fleet-render-a2ui dir: {e}"))?;
    let path = request_path(&req.id).unwrap();
    let json = serde_json::to_string_pretty(req).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write fleet-render-a2ui request: {e}"))
}

/// Read a pending request file. Called by the desktop watcher when it
/// notices a new id in the directory listing.
pub fn read_request(id: &str) -> Option<A2uiRenderRequest> {
    let path = request_path(id)?;
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Non-blocking response read.
pub fn try_read_response(id: &str) -> Option<A2uiRenderResponse> {
    let path = response_path(id)?;
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str::<A2uiRenderResponse>(&content).ok()
}

/// Blocking poll for a response. Returns `None` on timeout.
pub fn poll_response(id: &str, timeout: Duration) -> Option<A2uiRenderResponse> {
    let start = std::time::Instant::now();
    let interval = Duration::from_millis(200);
    loop {
        if let Some(r) = try_read_response(id) {
            return Some(r);
        }
        if start.elapsed() > timeout {
            return None;
        }
        std::thread::sleep(interval);
    }
}

/// Write a response file. Called by the desktop / fleet serve after the user
/// resolves the rendered A2UI surface.
pub fn write_response(resp: &A2uiRenderResponse) -> Result<(), String> {
    // Only a live pending request may be answered — see guard::write_response.
    if !request_path(&resp.id).map(|p| p.exists()).unwrap_or(false) {
        return Err(format!("no pending request for id {}", resp.id));
    }
    let path = response_path(&resp.id).ok_or("cannot determine home dir")?;
    let json = serde_json::to_string(resp).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write fleet-render-a2ui response: {e}"))
}

/// Remove the request + response pair. Called by the MCP server once it has
/// consumed the response.
pub fn cleanup(id: &str) {
    if let Some(p) = request_path(id) {
        let _ = fs::remove_file(p);
    }
    if let Some(p) = response_path(id) {
        let _ = fs::remove_file(p);
    }
}

/// Soft list of pending request ids; returns empty on any failure.
pub fn list_pending_requests() -> Vec<String> {
    list_pending_requests_checked().unwrap_or_default()
}

/// Strict list — same Ok/Err contract as
/// [`crate::mcp_ipc::list_pending_requests_checked`].
pub fn list_pending_requests_checked() -> std::io::Result<Vec<String>> {
    let Some(dir) = fleet_a2ui_dir() else {
        return Ok(Vec::new());
    };
    list_pending_in_dir(&dir)
}

fn list_pending_in_dir(dir: &std::path::Path) -> std::io::Result<Vec<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut request_ids: Vec<String> = Vec::new();
    let mut response_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(id) = name.strip_suffix(".response.json") {
            response_ids.insert(id.to_string());
        } else if let Some(id) = name.strip_suffix(".json") {
            request_ids.push(id.to_string());
        }
    }
    Ok(request_ids
        .into_iter()
        .filter(|id| !response_ids.contains(id))
        .collect())
}

/// JSONSchema describing `fleet__render_a2ui` arguments. The MCP `tools/list`
/// response embeds this verbatim so the agent sees the same shape the Rust
/// types deserialize.
pub fn a2ui_render_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "messageTree": {
                "type": "object",
                "description": "A2UI v0.9 agent-to-client message tree (passed through to @a2ui/web_core MessageProcessor). See https://github.com/google/A2UI/tree/main/specification/v0_9 for the full schema."
            }
        },
        "required": ["messageTree"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fresh_tmp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "fleet-render-a2ui-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    #[test]
    fn request_round_trips_camel_case_with_message_tree() {
        let req = A2uiRenderRequest {
            parked: false,
            id: "abc".into(),
            session_id: "sess-1".into(),
            workspace_name: "claude-fleet".into(),
            ai_title: Some("Implement P3".into()),
            timestamp: "2026-05-26T12:00:00Z".into(),
            message_tree: json!({
                "surfaceUpdate": {
                    "surfaceId": "demo",
                    "root": { "Card": { "id": "root", "children": [] } }
                }
            }),
        };
        let s = serde_json::to_string(&req).unwrap();
        // camelCase on the wire — matches fleet__ask
        assert!(s.contains("\"sessionId\""));
        assert!(s.contains("\"workspaceName\""));
        assert!(s.contains("\"aiTitle\""));
        assert!(s.contains("\"messageTree\""));
        let back: A2uiRenderRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.id, "abc");
        assert_eq!(back.session_id, "sess-1");
        assert_eq!(back.ai_title.as_deref(), Some("Implement P3"));
        // message_tree survives round-trip as a Value, no schema coercion
        assert!(back.message_tree["surfaceUpdate"]["surfaceId"] == "demo");
    }

    #[test]
    fn response_round_trips_action_payload() {
        let mut ctx = BTreeMap::new();
        ctx.insert("level".into(), "75".into());
        ctx.insert("date".into(), "2026-05-26".into());
        let resp = A2uiRenderResponse {
            id: "abc".into(),
            action_name: Some("submit".into()),
            action_context: ctx,
            cancelled: false,
        };
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"actionName\""));
        assert!(s.contains("\"actionContext\""));
        let back: A2uiRenderResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.action_name.as_deref(), Some("submit"));
        assert_eq!(back.action_context.get("level").unwrap(), "75");
        assert!(!back.cancelled);
    }

    #[test]
    fn response_round_trips_cancelled() {
        let resp = A2uiRenderResponse {
            id: "k".into(),
            action_name: None,
            action_context: BTreeMap::new(),
            cancelled: true,
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: A2uiRenderResponse = serde_json::from_str(&s).unwrap();
        assert!(back.cancelled);
        assert!(back.action_name.is_none());
        // action_name omitted when None thanks to skip_serializing_if
        assert!(!s.contains("\"actionName\""));
    }

    #[test]
    fn parses_request_with_nested_message_tree() {
        // Verifies the messageTree passthrough survives complex nesting —
        // anything @a2ui/web_core's schema accepts must round-trip without
        // schema knowledge baked into Fleet core.
        let raw = json!({
            "id": "r1",
            "messageTree": {
                "surfaceUpdate": {
                    "surfaceId": "form",
                    "root": {
                        "Card": {
                            "id": "card",
                            "children": [
                                { "TextField": { "id": "name", "label": "Name" } },
                                { "Slider": { "id": "n", "min": 0, "max": 100, "value": 50 } },
                                { "Button": { "id": "ok", "label": "OK", "action": { "name": "submit" } } }
                            ]
                        }
                    }
                }
            }
        });
        let req: A2uiRenderRequest = serde_json::from_value(raw).unwrap();
        let children = &req.message_tree["surfaceUpdate"]["root"]["Card"]["children"];
        assert_eq!(children.as_array().map(|a| a.len()), Some(3));
        assert_eq!(children[2]["Button"]["action"]["name"], "submit");
    }

    #[test]
    fn list_pending_in_dir_missing_returns_ok_empty() {
        let dir = fresh_tmp_dir("missing");
        assert!(!dir.exists());
        let result = list_pending_in_dir(&dir);
        assert!(matches!(&result, Ok(v) if v.is_empty()), "got {result:?}");
    }

    #[test]
    fn list_pending_in_dir_filters_responses() {
        let dir = fresh_tmp_dir("filter");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("req-a.json"), "{}").unwrap();
        std::fs::write(dir.join("req-b.json"), "{}").unwrap();
        std::fs::write(dir.join("req-a.response.json"), "{}").unwrap();
        std::fs::write(dir.join("ignored.txt"), "ignore").unwrap();
        let ids = list_pending_in_dir(&dir).unwrap();
        assert_eq!(ids, vec!["req-b".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_pending_in_dir_excludes_already_answered() {
        let dir = fresh_tmp_dir("answered");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("answered.json"), "{}").unwrap();
        std::fs::write(dir.join("answered.response.json"), "{}").unwrap();
        let ids = list_pending_in_dir(&dir).unwrap();
        assert!(ids.is_empty(), "answered orphan leaked: {ids:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn input_schema_advertises_message_tree() {
        let s = a2ui_render_input_schema();
        assert_eq!(s["type"], "object");
        assert_eq!(s["required"][0], "messageTree");
        assert!(s["properties"]["messageTree"].get("type").is_some());
    }
}
