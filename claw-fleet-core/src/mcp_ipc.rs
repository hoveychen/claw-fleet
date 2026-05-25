//! IPC protocol between the `fleet mcp` server (a Claude Code child process)
//! and the local `fleet` process (Fleet desktop or `fleet serve`).
//!
//! Wire format: line-delimited JSON over a unix socket (macOS/Linux) or
//! named pipe (Windows) at `~/.fleet/mcp-ipc.sock`. The request/response
//! shape is identical on Local and Remote backends — what differs is how
//! the answer gets routed back to Boss's Decision Panel (Local: in-process
//! emit; Remote: HTTP-pull from `fleet serve` via RemoteBackend).
//!
//! These types are the single source of truth for the `fleet__ask` schema
//! advertised over MCP; [`fleet_ask_input_schema`] mirrors them in JSONSchema
//! form for the MCP `tools/list` response.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetAskRequest {
    pub id: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetAskResponse {
    pub id: String,
    #[serde(default)]
    pub answers: BTreeMap<String, String>,
    #[serde(default)]
    pub cancelled: bool,
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
                                        "enum": ["text", "textarea", "number", "select", "radio", "checkbox"]
                                    },
                                    "label": { "type": "string" },
                                    "placeholder": { "type": "string" },
                                    "options": {
                                        "type": "array",
                                        "items": { "type": "string" }
                                    },
                                    "required": { "type": "boolean" },
                                    "default": {}
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

    #[test]
    fn round_trip_minimal_request() {
        let req = FleetAskRequest {
            id: "abc".into(),
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
            }],
        };
        let v: serde_json::Value = serde_json::to_value(&q).unwrap();
        assert!(v.get("multiSelect").is_some(), "multiSelect (camelCase)");
        assert!(v.get("formFields").is_some(), "formFields (camelCase)");
        assert!(v.get("multi_select").is_none(), "no snake_case bleed");
        assert!(v.get("form_fields").is_none(), "no snake_case bleed");
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
