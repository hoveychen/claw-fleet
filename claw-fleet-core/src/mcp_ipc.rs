//! IPC protocol between the `fleet mcp` server (a Claude Code child process)
//! and the local `fleet` process (Fleet desktop or `fleet serve`).
//!
//! Wire format: file-based IPC at `~/.fleet/fleet-ask/<id>.json` (request)
//! and `~/.fleet/fleet-ask/<id>.response.json` (response). Mirrors the
//! existing `elicitation` / `guard` / `plan_approval` Decision Card patterns
//! so all four channels can share the same watcher / cleanup / orphan-filter
//! logic and the desktop polls a single style of directory.
//!
//! These types are the single source of truth for the `fleet__ask` schema
//! advertised over MCP; [`fleet_ask_input_schema`] mirrors them in JSONSchema
//! form for the MCP `tools/list` response.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetAskRequest {
    pub id: String,
    /// Originating Claude Code session id (passed via env var
    /// `CLAUDE_CODE_SESSION_ID` when the MCP server is launched). Used by the
    /// desktop watcher to resolve workspace + AI title for the Decision
    /// Card header, exactly like `ElicitationRequest::session_id`.
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub workspace_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_title: Option<String>,
    #[serde(default)]
    pub timestamp: String,
    pub questions: Vec<FleetAskQuestion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetAskQuestion {
    pub question: String,
    pub header: String,
    #[serde(default)]
    pub multi_select: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<FleetAskOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub form_fields: Vec<FleetAskFormField>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetAskOption {
    pub label: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetAskFormField {
    pub name: String,
    pub kind: FormFieldKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FormFieldKind {
    Text,
    Textarea,
    Number,
    Select,
    Radio,
    Checkbox,
    Date,
    Datetime,
    Time,
    Range,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetAskResponse {
    pub id: String,
    #[serde(default)]
    pub answers: BTreeMap<String, String>,
    #[serde(default)]
    pub cancelled: bool,
}

// ── File-based IPC (mirror of `elicitation` module) ──────────────────────────

fn fleet_ask_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("fleet-ask"))
}

fn request_path(id: &str) -> Option<PathBuf> {
    fleet_ask_dir().map(|d| d.join(format!("{id}.json")))
}

fn response_path(id: &str) -> Option<PathBuf> {
    fleet_ask_dir().map(|d| d.join(format!("{id}.response.json")))
}

/// Write a fleet_ask request file. Called by the MCP server when it handles
/// a `tools/call` for `fleet__ask`.
pub fn write_request(req: &FleetAskRequest) -> Result<(), String> {
    let dir = fleet_ask_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create fleet-ask dir: {e}"))?;
    let path = request_path(&req.id).unwrap();
    let json = serde_json::to_string_pretty(req).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write fleet-ask request: {e}"))
}

/// Read a pending request file. Called by the desktop watcher when it
/// notices a new id in the directory listing.
pub fn read_request(id: &str) -> Option<FleetAskRequest> {
    let path = request_path(id)?;
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Non-blocking response read.
pub fn try_read_response(id: &str) -> Option<FleetAskResponse> {
    let path = response_path(id)?;
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str::<FleetAskResponse>(&content).ok()
}

/// Blocking poll for a response. Returns `None` on timeout.
pub fn poll_response(id: &str, timeout: Duration) -> Option<FleetAskResponse> {
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

/// Write a response file. Called by the desktop / fleet serve after the
/// user resolves the Decision Card.
pub fn write_response(resp: &FleetAskResponse) -> Result<(), String> {
    let path = response_path(&resp.id).ok_or("cannot determine home dir")?;
    let json = serde_json::to_string(resp).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write fleet-ask response: {e}"))
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

/// Strict list — distinguishes "no requests / missing dir" (`Ok(vec![])`)
/// from "I/O error reading the directory" (`Err`). The watcher uses the
/// `Err` to skip the dismissal-emit step so a transient APFS hiccup
/// doesn't take every active panel down with it.
pub fn list_pending_requests_checked() -> std::io::Result<Vec<String>> {
    let Some(dir) = fleet_ask_dir() else {
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

/// JSONSchema describing `fleet__ask` arguments. The MCP `tools/list` response
/// embeds this verbatim so the agent sees the same field shape the Rust types
/// would deserialize.
pub fn fleet_ask_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "questions": {
                "type": "array",
                "minItems": 1,
                "maxItems": 4,
                "items": {
                    "type": "object",
                    "properties": {
                        "question": { "type": "string" },
                        "header": { "type": "string", "maxLength": 12 },
                        "multiSelect": { "type": "boolean" },
                        "options": {
                            "type": "array",
                            "minItems": 2,
                            "maxItems": 4,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "label": { "type": "string" },
                                    "description": { "type": "string" },
                                    "preview": { "type": "string" }
                                },
                                "required": ["label", "description"]
                            }
                        },
                        "html": {
                            "type": "string",
                            "description": "HTML preview body; rendered in an iframe srcdoc with sandbox=\"\" (no scripts, no same-origin)"
                        },
                        "formFields": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string" },
                                    "kind": {
                                        "type": "string",
                                        "enum": ["text", "textarea", "number", "select", "radio", "checkbox", "date", "datetime", "time", "range"]
                                    },
                                    "label": { "type": "string" },
                                    "placeholder": { "type": "string" },
                                    "options": {
                                        "type": "array",
                                        "items": { "type": "string" }
                                    },
                                    "required": { "type": "boolean" },
                                    "default": {},
                                    "min": { "type": "number", "description": "Range slider lower bound (range kind only)" },
                                    "max": { "type": "number", "description": "Range slider upper bound (range kind only)" },
                                    "step": { "type": "number", "description": "Range slider step (range kind only)" }
                                },
                                "required": ["name", "kind", "label"]
                            }
                        }
                    },
                    "required": ["question", "header", "multiSelect"]
                }
            }
        },
        "required": ["questions"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn empty_request(id: &str) -> FleetAskRequest {
        FleetAskRequest {
            id: id.into(),
            session_id: String::new(),
            workspace_name: String::new(),
            ai_title: None,
            timestamp: String::new(),
            questions: vec![FleetAskQuestion {
                question: "q".into(),
                header: "h".into(),
                multi_select: false,
                options: vec![],
                html: None,
                form_fields: vec![],
            }],
        }
    }

    #[test]
    fn round_trip_minimal_request() {
        let req = FleetAskRequest {
            id: "abc".into(),
            session_id: String::new(),
            workspace_name: String::new(),
            ai_title: None,
            timestamp: String::new(),
            questions: vec![FleetAskQuestion {
                question: "Pick one".into(),
                header: "Choice".into(),
                multi_select: false,
                options: vec![],
                html: None,
                form_fields: vec![],
            }],
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: FleetAskRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.id, "abc");
        assert_eq!(back.questions[0].question, "Pick one");
        // Empty/None fields are skipped on the wire.
        assert!(!s.contains("\"options\""));
        assert!(!s.contains("\"html\""));
        assert!(!s.contains("\"formFields\""));
    }

    #[test]
    fn request_carries_session_envelope() {
        let mut req = empty_request("e1");
        req.session_id = "sess-123".into();
        req.workspace_name = "claude-fleet".into();
        req.ai_title = Some("Implement P2".into());
        req.timestamp = "2026-05-25T12:00:00Z".into();
        let s = serde_json::to_string(&req).unwrap();
        let back: FleetAskRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.session_id, "sess-123");
        assert_eq!(back.workspace_name, "claude-fleet");
        assert_eq!(back.ai_title.as_deref(), Some("Implement P2"));
        assert_eq!(back.timestamp, "2026-05-25T12:00:00Z");
        // camelCase on the wire — matches elicitation pattern.
        assert!(s.contains("\"sessionId\""));
        assert!(s.contains("\"workspaceName\""));
        assert!(s.contains("\"aiTitle\""));
    }

    #[test]
    fn camel_case_field_names() {
        let q = FleetAskQuestion {
            question: "q".into(),
            header: "h".into(),
            multi_select: true,
            options: vec![],
            html: None,
            form_fields: vec![FleetAskFormField {
                name: "commit_msg".into(),
                kind: FormFieldKind::Textarea,
                label: "Message".into(),
                placeholder: None,
                options: vec![],
                required: true,
                default: None,
                min: None,
                max: None,
                step: None,
            }],
        };
        let v: serde_json::Value = serde_json::to_value(&q).unwrap();
        assert!(v.get("multiSelect").is_some(), "multiSelect (camelCase)");
        assert!(v.get("formFields").is_some(), "formFields (camelCase)");
        assert!(v.get("multi_select").is_none(), "no snake_case bleed");
        assert!(v.get("form_fields").is_none(), "no snake_case bleed");
    }

    // ── File-IPC tests (mirror of elicitation::tests::list_pending_in_dir_*) ─

    fn fresh_tmp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "fleet-ask-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
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
        // Regression: orphaned `<id>.json` paired with `<id>.response.json`
        // must not re-surface, exactly like the elicitation orphan filter.
        let dir = fresh_tmp_dir("answered");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("answered.json"), "{}").unwrap();
        std::fs::write(dir.join("answered.response.json"), "{}").unwrap();
        let ids = list_pending_in_dir(&dir).unwrap();
        assert!(ids.is_empty(), "answered orphan leaked: {ids:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn form_field_kind_serializes_lowercase() {
        assert_eq!(
            serde_json::to_value(FormFieldKind::Textarea).unwrap(),
            json!("textarea")
        );
        assert_eq!(
            serde_json::to_value(FormFieldKind::Checkbox).unwrap(),
            json!("checkbox")
        );
        let back: FormFieldKind = serde_json::from_value(json!("radio")).unwrap();
        assert_eq!(back, FormFieldKind::Radio);
    }

    #[test]
    fn response_round_trip_with_form_answers() {
        let mut answers = BTreeMap::new();
        answers.insert("commit_msg".into(), "fix: typo".into());
        answers.insert("strategy".into(), "rebase".into());
        let resp = FleetAskResponse {
            id: "xyz".into(),
            answers,
            cancelled: false,
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: FleetAskResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.answers.get("commit_msg").unwrap(), "fix: typo");
        assert_eq!(back.answers.get("strategy").unwrap(), "rebase");
        assert!(!back.cancelled);
    }

    #[test]
    fn response_round_trip_cancelled() {
        let resp = FleetAskResponse {
            id: "k".into(),
            answers: BTreeMap::new(),
            cancelled: true,
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: FleetAskResponse = serde_json::from_str(&s).unwrap();
        assert!(back.cancelled);
    }

    #[test]
    fn schema_advertises_html_and_form_fields_in_camel_case() {
        let s = fleet_ask_input_schema();
        let props = &s["properties"]["questions"]["items"]["properties"];
        assert!(props.get("html").is_some(), "html field present");
        assert!(
            props.get("formFields").is_some(),
            "formFields camelCase, not snake_case"
        );
        assert!(props.get("form_fields").is_none());
        // Form-field kinds enum surfaces in schema:
        let kinds = &props["formFields"]["items"]["properties"]["kind"]["enum"];
        let arr = kinds.as_array().expect("enum array");
        assert!(arr.iter().any(|v| v == "textarea"));
        assert!(arr.iter().any(|v| v == "checkbox"));
    }

    #[test]
    fn form_field_kind_serialises_new_variants_lowercase() {
        assert_eq!(
            serde_json::to_value(FormFieldKind::Date).unwrap(),
            json!("date")
        );
        assert_eq!(
            serde_json::to_value(FormFieldKind::Datetime).unwrap(),
            json!("datetime")
        );
        assert_eq!(
            serde_json::to_value(FormFieldKind::Time).unwrap(),
            json!("time")
        );
        assert_eq!(
            serde_json::to_value(FormFieldKind::Range).unwrap(),
            json!("range")
        );
        let back: FormFieldKind = serde_json::from_value(json!("date")).unwrap();
        assert_eq!(back, FormFieldKind::Date);
        let back: FormFieldKind = serde_json::from_value(json!("range")).unwrap();
        assert_eq!(back, FormFieldKind::Range);
    }

    #[test]
    fn form_field_min_max_step_round_trip() {
        let f = FleetAskFormField {
            name: "volume".into(),
            kind: FormFieldKind::Range,
            label: "Volume".into(),
            placeholder: None,
            options: vec![],
            required: false,
            default: Some(json!(50)),
            min: Some(0.0),
            max: Some(100.0),
            step: Some(5.0),
        };
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["min"], json!(0.0));
        assert_eq!(v["max"], json!(100.0));
        assert_eq!(v["step"], json!(5.0));
        assert_eq!(v["kind"], json!("range"));
        let back: FleetAskFormField = serde_json::from_value(v).unwrap();
        assert_eq!(back.min, Some(0.0));
        assert_eq!(back.max, Some(100.0));
        assert_eq!(back.step, Some(5.0));
        assert_eq!(back.kind, FormFieldKind::Range);
    }

    #[test]
    fn form_field_min_max_step_omitted_when_none() {
        let f = FleetAskFormField {
            name: "msg".into(),
            kind: FormFieldKind::Text,
            label: "Message".into(),
            placeholder: None,
            options: vec![],
            required: false,
            default: None,
            min: None,
            max: None,
            step: None,
        };
        let v = serde_json::to_value(&f).unwrap();
        assert!(v.get("min").is_none(), "min skipped when None");
        assert!(v.get("max").is_none(), "max skipped when None");
        assert!(v.get("step").is_none(), "step skipped when None");
    }

    #[test]
    fn schema_advertises_new_kinds_and_range_bounds() {
        let s = fleet_ask_input_schema();
        let item_props =
            &s["properties"]["questions"]["items"]["properties"]["formFields"]["items"]["properties"];
        let kinds = item_props["kind"]["enum"]
            .as_array()
            .expect("enum array");
        for k in ["date", "datetime", "time", "range"] {
            assert!(kinds.iter().any(|v| v == k), "kind enum missing {k}");
        }
        assert!(item_props.get("min").is_some(), "min advertised");
        assert!(item_props.get("max").is_some(), "max advertised");
        assert!(item_props.get("step").is_some(), "step advertised");
    }

    #[test]
    fn parses_request_with_html_and_form() {
        let raw = json!({
            "id": "r1",
            "questions": [{
                "question": "Confirm commit message and rebase strategy",
                "header": "Commit",
                "multiSelect": false,
                "html": "<p>Diff preview</p>",
                "formFields": [
                    {
                        "name": "commit_msg",
                        "kind": "textarea",
                        "label": "Commit message",
                        "required": true
                    },
                    {
                        "name": "strategy",
                        "kind": "radio",
                        "label": "Strategy",
                        "options": ["merge", "rebase", "squash"]
                    }
                ]
            }]
        });
        let req: FleetAskRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(req.questions[0].html.as_deref(), Some("<p>Diff preview</p>"));
        assert_eq!(req.questions[0].form_fields.len(), 2);
        assert_eq!(req.questions[0].form_fields[1].kind, FormFieldKind::Radio);
        assert_eq!(
            req.questions[0].form_fields[1].options,
            vec!["merge", "rebase", "squash"]
        );
    }
}
