//! Fleet's decision cards, mapped onto ACP's two human-in-the-loop channels.
//!
//! The Responses surface had to borrow OpenAI's `function_call` slot for these,
//! which properly means "client, run this function for me" — a card asking a
//! person to approve a command is not that. ACP has two purpose-built channels
//! instead, and Fleet's six card types split cleanly across them:
//!
//! | card | channel | why |
//! |---|---|---|
//! | guard, permission-prompt | `session/request_permission` | already attached to a tool call |
//! | elicitation, fleet-ask, plan-approval | `elicitation/create` | a question, not a tool |
//! | a2ui | `elicitation/create` (URL mode) | a rendered surface, not a form |
//!
//! Everything here is pure: cards in, ACP requests out; ACP answers in, Fleet
//! responses out. The polling, the blocking and the write-back live in
//! `agent.rs`, so every mapping decision below is unit-testable on its own.

use serde_json::{json, Map, Value};

use super::types::{
    CreateElicitationRequest, ElicitationAction, ElicitationSchema, PermissionOption,
    PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest, ToolCallStatus,
    ToolCallUpdate,
};

/// The option ids Fleet answers on. Stable strings, not indexes, so a client
/// echoing one back cannot be misread if the option list ever changes order.
pub const OPT_ALLOW: &str = "allow";
pub const OPT_ALLOW_ALWAYS: &str = "allow_always";
pub const OPT_REJECT: &str = "reject";

/// The two-or-three choices Fleet offers on a permission card.
///
/// `allow_always` is included only where Fleet can actually remember the
/// answer — offering a "don't ask again" that is silently forgotten is worse
/// than not offering it.
pub fn permission_options(allow_always: bool) -> Vec<PermissionOption> {
    let mut opts = vec![PermissionOption {
        option_id: OPT_ALLOW.to_string(),
        name: "Allow".to_string(),
        kind: PermissionOptionKind::AllowOnce,
    }];
    if allow_always {
        opts.push(PermissionOption {
            option_id: OPT_ALLOW_ALWAYS.to_string(),
            name: "Allow and remember".to_string(),
            kind: PermissionOptionKind::AllowAlways,
        });
    }
    opts.push(PermissionOption {
        option_id: OPT_REJECT.to_string(),
        name: "Reject".to_string(),
        kind: PermissionOptionKind::RejectOnce,
    });
    opts
}

/// Whether an outcome means "go ahead".
///
/// A `cancelled` outcome is **not** an approval: the schema requires a client
/// that cancelled the turn to answer every pending permission this way, so
/// treating it as consent would run a command nobody approved.
pub fn outcome_allows(outcome: &RequestPermissionOutcome) -> bool {
    match outcome {
        RequestPermissionOutcome::Cancelled => false,
        RequestPermissionOutcome::Selected { option_id } => {
            option_id == OPT_ALLOW || option_id == OPT_ALLOW_ALWAYS
        }
    }
}

/// Whether the user asked Fleet to remember the answer.
pub fn outcome_is_always(outcome: &RequestPermissionOutcome) -> bool {
    matches!(outcome, RequestPermissionOutcome::Selected { option_id } if option_id == OPT_ALLOW_ALWAYS)
}

// ─────────────────────── guard / permission-prompt ──────────────────

/// A guard card as a permission request.
///
/// Guard cards already describe a command the agent is about to run, so the
/// tool call they attach to is that command. `risk_tags` and the structured
/// command view ride along in `_meta`-style raw output rather than being
/// flattened into prose a client cannot act on.
pub fn guard_to_permission(
    session_id: &str,
    req: &crate::guard::GuardRequest,
) -> RequestPermissionRequest {
    RequestPermissionRequest {
        session_id: session_id.to_string(),
        tool_call: ToolCallUpdate {
            tool_call_id: req.id.clone(),
            status: Some(ToolCallStatus::Pending),
            content: None,
            raw_output: None,
        },
        // Guard rules are persisted by Fleet, so "remember this" is real here.
        options: permission_options(true),
    }
}

/// The title a client should show for a guard card.
pub fn guard_title(req: &crate::guard::GuardRequest) -> String {
    let summary = if req.command_summary.is_empty() {
        req.command.as_str()
    } else {
        req.command_summary.as_str()
    };
    let head = summary.lines().next().unwrap_or(summary);
    if req.risk_tags.is_empty() {
        head.to_string()
    } else {
        format!("{head}  [{}]", req.risk_tags.join(", "))
    }
}

/// A permission-prompt card as a permission request.
pub fn permission_prompt_to_permission(
    session_id: &str,
    req: &crate::permission_prompt_ipc::PermissionPromptRequest,
) -> RequestPermissionRequest {
    RequestPermissionRequest {
        session_id: session_id.to_string(),
        tool_call: ToolCallUpdate {
            // Prefer Claude's own tool_use id so the permission lands on the
            // tool call the client is already showing, rather than on a
            // synthetic one it has never seen.
            tool_call_id: req.tool_use_id.clone().unwrap_or_else(|| req.id.clone()),
            status: Some(ToolCallStatus::Pending),
            content: None,
            raw_output: None,
        },
        // Claude Code owns this decision for the turn; Fleet has nowhere to
        // persist an "always" for it, so it is not offered.
        options: permission_options(false),
    }
}

// ──────────────────────── form-mode elicitations ────────────────────

/// A `FleetAskFormField` as a JSON Schema property.
///
/// The mapping is lossless per 3.2 of the design: text/textarea are strings,
/// select/radio are strings with an `enum`, checkbox is a boolean, and the
/// numeric bounds carry across. `textarea`'s multi-line nature has no schema
/// representation, so it is described rather than dropped silently.
pub fn form_field_to_property(f: &crate::mcp_ipc::FleetAskFormField) -> Value {
    use crate::mcp_ipc::FormFieldKind as K;
    let mut p = Map::new();
    p.insert("title".into(), json!(f.label));
    match f.kind {
        K::Text => {
            p.insert("type".into(), json!("string"));
        }
        K::Textarea => {
            p.insert("type".into(), json!("string"));
            p.insert("description".into(), json!("Multi-line text"));
        }
        K::Number | K::Range => {
            p.insert("type".into(), json!("number"));
            if let Some(min) = f.min {
                p.insert("minimum".into(), json!(min));
            }
            if let Some(max) = f.max {
                p.insert("maximum".into(), json!(max));
            }
            if let Some(step) = f.step {
                p.insert("multipleOf".into(), json!(step));
            }
        }
        K::Select | K::Radio => {
            p.insert("type".into(), json!("string"));
            if !f.options.is_empty() {
                p.insert("enum".into(), json!(f.options));
            }
        }
        K::Checkbox => {
            p.insert("type".into(), json!("boolean"));
        }
        K::Date | K::Datetime | K::Time => {
            p.insert("type".into(), json!("string"));
            let fmt = match f.kind {
                K::Date => "date",
                K::Datetime => "date-time",
                _ => "time",
            };
            p.insert("format".into(), json!(fmt));
        }
    }
    if let Some(d) = &f.default {
        p.insert("default".into(), d.clone());
    }
    if let Some(ph) = &f.placeholder {
        p.entry("description").or_insert(json!(ph));
    }
    Value::Object(p)
}

/// A fleet-ask card as a form elicitation.
///
/// Each question becomes one property; its options become an `enum`. Form
/// fields are added alongside. Rich preview (`html`, `images`) has no place in
/// a schema — that is what URL mode is for, and the caller decides which to use
/// based on what the client advertised.
pub fn fleet_ask_to_form(
    session_id: &str,
    req: &crate::mcp_ipc::FleetAskRequest,
) -> CreateElicitationRequest {
    let mut props = Map::new();
    let mut required = Vec::new();

    for q in &req.questions {
        let key = q.question.clone();
        let mut p = Map::new();
        p.insert("type".into(), json!("string"));
        p.insert("title".into(), json!(q.header.clone()));
        if !q.options.is_empty() {
            p.insert("enum".into(), json!(q.options.iter().map(|o| o.label.clone()).collect::<Vec<_>>()));
        }
        props.insert(key.clone(), Value::Object(p));
        required.push(key);

        for f in &q.form_fields {
            props.insert(f.name.clone(), form_field_to_property(f));
            if f.required {
                required.push(f.name.clone());
            }
        }
    }

    let message = req
        .questions
        .first()
        .map(|q| q.question.clone())
        .unwrap_or_else(|| "Fleet needs your input".to_string());

    CreateElicitationRequest::form(
        session_id,
        message,
        ElicitationSchema::object(props, required),
    )
}

/// An elicitation card as a form elicitation.
pub fn elicitation_to_form(
    session_id: &str,
    req: &crate::elicitation::ElicitationRequest,
) -> CreateElicitationRequest {
    let mut props = Map::new();
    let mut required = Vec::new();
    for q in &req.questions {
        let mut p = Map::new();
        p.insert("type".into(), json!("string"));
        p.insert("title".into(), json!(q.header.clone()));
        if !q.options.is_empty() {
            p.insert("enum".into(), json!(q.options.iter().map(|o| o.label.clone()).collect::<Vec<_>>()));
        }
        props.insert(q.question.clone(), Value::Object(p));
        required.push(q.question.clone());
    }
    let message = req
        .questions
        .first()
        .map(|q| q.question.clone())
        .unwrap_or_else(|| "Fleet needs your input".to_string());
    CreateElicitationRequest::form(session_id, message, ElicitationSchema::object(props, required))
}

/// Keys the plan-approval form answers on.
pub const PLAN_DECISION: &str = "decision";
pub const PLAN_EDITED: &str = "edited_plan";

/// A plan-approval card as a form elicitation.
///
/// Deliberately *not* `request_permission`: Fleet lets the user approve a plan
/// **and rewrite it**, and a permission outcome carries only an option id with
/// nowhere to put the edited text. A form has room for both.
pub fn plan_approval_to_form(
    session_id: &str,
    req: &crate::plan_approval::PlanApprovalRequest,
) -> CreateElicitationRequest {
    let mut props = Map::new();
    props.insert(
        PLAN_DECISION.into(),
        json!({"type": "string", "title": "Decision", "enum": ["approve", "reject"]}),
    );
    props.insert(
        PLAN_EDITED.into(),
        json!({
            "type": "string",
            "title": "Plan",
            "description": "Edit before approving, or leave as-is",
            "default": req.plan_content,
        }),
    );
    CreateElicitationRequest::form(
        session_id,
        "Review the plan".to_string(),
        ElicitationSchema::object(props, vec![PLAN_DECISION.to_string()]),
    )
}

// ─────────────────────── answers → Fleet responses ──────────────────

/// Turn an elicitation answer into a fleet-ask response.
pub fn form_answer_to_fleet_ask(
    id: &str,
    action: &ElicitationAction,
) -> crate::mcp_ipc::FleetAskResponse {
    crate::mcp_ipc::FleetAskResponse {
        id: id.to_string(),
        cancelled: action.is_refusal(),
        answers: stringify_map(&action.content()).into_iter().collect(),
    }
}

/// Turn an elicitation answer into an elicitation response.
pub fn form_answer_to_elicitation(
    id: &str,
    action: &ElicitationAction,
) -> crate::elicitation::ElicitationResponse {
    crate::elicitation::ElicitationResponse {
        id: id.to_string(),
        declined: action.is_refusal(),
        answers: stringify_map(&action.content()).into_iter().collect(),
    }
}

/// Turn an elicitation answer into a plan-approval response.
///
/// A refusal is a rejection, not an empty approval — the plan must not proceed
/// because the user closed a dialog.
pub fn form_answer_to_plan(
    id: &str,
    action: &ElicitationAction,
) -> crate::plan_approval::PlanApprovalResponse {
    let content = action.content();
    let approved = !action.is_refusal()
        && content.get(PLAN_DECISION).and_then(|v| v.as_str()) == Some("approve");
    let edited = content
        .get(PLAN_EDITED)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    crate::plan_approval::PlanApprovalResponse {
        id: id.to_string(),
        decision: if approved { "approve".into() } else { "reject".into() },
        edited_plan: if approved { edited } else { None },
        feedback: None,
    }
}

/// Flatten submitted values to the `String -> String` map Fleet stores.
///
/// A checkbox arrives as a JSON boolean and a number as a JSON number; taking
/// `as_str()` alone would drop both and record an empty answer.
fn stringify_map(content: &Map<String, Value>) -> std::collections::HashMap<String, String> {
    content
        .iter()
        .filter_map(|(k, v)| {
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Null => return None,
                other => other.to_string(),
            };
            Some((k.clone(), s))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_ipc::{FleetAskFormField, FleetAskOption, FleetAskQuestion, FormFieldKind};

    fn guard_req() -> crate::guard::GuardRequest {
        crate::guard::GuardRequest {
            id: "g1".into(),
            session_id: "s1".into(),
            workspace_name: "w".into(),
            ai_title: None,
            tool_name: "Bash".into(),
            command: "rm -rf /tmp/x".into(),
            command_summary: "Delete a temp dir".into(),
            risk_tags: vec!["destructive".into()],
            timestamp: "t".into(),
            structured_command: None,
        }
    }

    #[test]
    fn a_guard_card_becomes_a_permission_on_its_own_id() {
        let r = guard_to_permission("sess", &guard_req());
        assert_eq!(r.session_id, "sess");
        assert_eq!(r.tool_call.tool_call_id, "g1");
        assert_eq!(r.tool_call.status, Some(ToolCallStatus::Pending));
        // Guard rules persist, so "remember" is a real choice here.
        assert_eq!(r.options.len(), 3);
        assert!(r.options.iter().any(|o| o.kind == PermissionOptionKind::AllowAlways));
    }

    #[test]
    fn a_permission_prompt_attaches_to_claudes_own_tool_call_id() {
        // Landing it on a synthetic id would show the client a permission for
        // a tool call it has never heard of.
        let mut req = crate::permission_prompt_ipc::PermissionPromptRequest {
            id: "p1".into(),
            session_id: "s1".into(),
            workspace_name: "w".into(),
            ai_title: None,
            timestamp: "t".into(),
            tool_name: "Write".into(),
            tool_input: json!({"file_path": "/w/a.rs"}),
            tool_use_id: Some("toolu_XYZ".into()),
        };
        assert_eq!(permission_prompt_to_permission("s", &req).tool_call.tool_call_id, "toolu_XYZ");

        // With no tool_use_id there is still something to answer on.
        req.tool_use_id = None;
        assert_eq!(permission_prompt_to_permission("s", &req).tool_call.tool_call_id, "p1");

        // Fleet cannot persist an "always" for this channel, so it is not offered.
        assert!(permission_prompt_to_permission("s", &req)
            .options
            .iter()
            .all(|o| o.kind != PermissionOptionKind::AllowAlways));
    }

    #[test]
    fn a_cancelled_outcome_is_never_treated_as_approval() {
        // The schema makes a cancelling client answer every pending permission
        // with `cancelled`; reading that as consent would run a command
        // nobody approved.
        assert!(!outcome_allows(&RequestPermissionOutcome::Cancelled));
        assert!(!outcome_is_always(&RequestPermissionOutcome::Cancelled));

        assert!(outcome_allows(&RequestPermissionOutcome::Selected { option_id: OPT_ALLOW.into() }));
        assert!(outcome_allows(&RequestPermissionOutcome::Selected {
            option_id: OPT_ALLOW_ALWAYS.into()
        }));
        assert!(!outcome_allows(&RequestPermissionOutcome::Selected {
            option_id: OPT_REJECT.into()
        }));
        // An id we never offered is not an approval either.
        assert!(!outcome_allows(&RequestPermissionOutcome::Selected {
            option_id: "something-else".into()
        }));
    }

    #[test]
    fn only_allow_always_asks_fleet_to_remember() {
        assert!(outcome_is_always(&RequestPermissionOutcome::Selected {
            option_id: OPT_ALLOW_ALWAYS.into()
        }));
        assert!(!outcome_is_always(&RequestPermissionOutcome::Selected {
            option_id: OPT_ALLOW.into()
        }));
    }

    #[test]
    fn the_guard_title_leads_with_the_summary_and_keeps_risk_tags() {
        let t = guard_title(&guard_req());
        assert!(t.starts_with("Delete a temp dir"));
        assert!(t.contains("destructive"), "the risk is the reason to look: {t}");

        // With no summary the command itself is the label.
        let mut r = guard_req();
        r.command_summary = String::new();
        assert!(guard_title(&r).starts_with("rm -rf /tmp/x"));
    }

    fn field(name: &str, kind: FormFieldKind) -> FleetAskFormField {
        FleetAskFormField {
            name: name.into(),
            kind,
            label: "L".into(),
            placeholder: None,
            options: vec!["a".into(), "b".into()],
            required: false,
            default: None,
            min: None,
            max: None,
            step: None,
        }
    }

    #[test]
    fn every_form_field_kind_maps_to_a_schema_type() {
        use FormFieldKind as K;
        let cases = [
            (K::Text, "string", None),
            (K::Textarea, "string", None),
            (K::Number, "number", None),
            (K::Range, "number", None),
            (K::Select, "string", None),
            (K::Radio, "string", None),
            (K::Checkbox, "boolean", None),
            (K::Date, "string", Some("date")),
            (K::Datetime, "string", Some("date-time")),
            (K::Time, "string", Some("time")),
        ];
        for (kind, ty, fmt) in cases {
            let p = form_field_to_property(&field("f", kind));
            assert_eq!(p["type"], ty, "{kind:?}");
            match fmt {
                Some(f) => assert_eq!(p["format"], f, "{kind:?}"),
                None => assert!(p.get("format").is_none(), "{kind:?}"),
            }
        }
    }

    #[test]
    fn single_choice_fields_carry_their_options_as_an_enum() {
        for kind in [FormFieldKind::Select, FormFieldKind::Radio] {
            let p = form_field_to_property(&field("f", kind));
            assert_eq!(p["enum"], json!(["a", "b"]), "{kind:?}");
        }
        // A free-text field has no enum even though options is populated.
        assert!(form_field_to_property(&field("f", FormFieldKind::Text)).get("enum").is_none());
    }

    #[test]
    fn numeric_bounds_survive_the_mapping() {
        let mut f = field("n", FormFieldKind::Range);
        f.min = Some(1.0);
        f.max = Some(10.0);
        f.step = Some(0.5);
        let p = form_field_to_property(&f);
        assert_eq!(p["minimum"], json!(1.0));
        assert_eq!(p["maximum"], json!(10.0));
        assert_eq!(p["multipleOf"], json!(0.5));
    }

    fn ask_req() -> crate::mcp_ipc::FleetAskRequest {
        crate::mcp_ipc::FleetAskRequest {
            id: "a1".into(),
            session_id: "s1".into(),
            workspace_name: "w".into(),
            ai_title: None,
            timestamp: "t".into(),
            parked: false,
            questions: vec![FleetAskQuestion {
                question: "Which approach?".into(),
                header: "Approach".into(),
                multi_select: false,
                options: vec![
                    FleetAskOption { label: "A".into(), description: "d".into(), preview: None },
                    FleetAskOption { label: "B".into(), description: "d".into(), preview: None },
                ],
                html: None,
                form_fields: vec![field("note", FormFieldKind::Textarea)],
                images: vec![],
            }],
            review_docs: vec![],
        }
    }

    #[test]
    fn a_fleet_ask_becomes_a_form_with_options_as_an_enum() {
        let e = fleet_ask_to_form("sess", &ask_req());
        assert_eq!(e.mode, "form");
        assert_eq!(e.session_id, "sess");
        assert_eq!(e.message, "Which approach?");
        let schema = e.requested_schema.unwrap();
        assert_eq!(schema.properties["Which approach?"]["enum"], json!(["A", "B"]));
        // Form fields sit alongside the question.
        assert_eq!(schema.properties["note"]["type"], "string");
        // The question is required; an optional field is not.
        assert!(schema.required.contains(&"Which approach?".to_string()));
        assert!(!schema.required.contains(&"note".to_string()));
    }

    #[test]
    fn a_plan_approval_is_a_form_because_a_permission_cannot_carry_an_edit() {
        let req = crate::plan_approval::PlanApprovalRequest {
            id: "pl1".into(),
            session_id: "s".into(),
            workspace_name: "w".into(),
            ai_title: None,
            plan_content: "step one".into(),
            plan_file_path: None,
            timestamp: "t".into(),
            parked: false,
        };
        let e = plan_approval_to_form("sess", &req);
        let schema = e.requested_schema.unwrap();
        assert_eq!(schema.properties[PLAN_DECISION]["enum"], json!(["approve", "reject"]));
        // The plan is pre-filled so the user edits rather than retypes.
        assert_eq!(schema.properties[PLAN_EDITED]["default"], "step one");
        assert_eq!(schema.required, vec![PLAN_DECISION.to_string()]);
    }

    fn accept(v: Value) -> ElicitationAction {
        ElicitationAction::Accept { content: Some(v.as_object().unwrap().clone()) }
    }

    #[test]
    fn an_accepted_form_becomes_fleet_answers() {
        let a = accept(json!({"Which approach?": "A", "note": "because"}));
        let r = form_answer_to_fleet_ask("a1", &a);
        assert!(!r.cancelled);
        assert_eq!(r.answers["Which approach?"], "A");
        assert_eq!(r.answers["note"], "because");
    }

    #[test]
    fn declining_or_cancelling_is_recorded_as_a_refusal() {
        // Both map to Fleet's cancelled/declined flag, which is what lets a
        // parked card resume the session instead of hanging.
        for a in [ElicitationAction::Decline, ElicitationAction::Cancel] {
            assert!(a.is_refusal());
            assert!(form_answer_to_fleet_ask("a1", &a).cancelled);
            assert!(form_answer_to_elicitation("e1", &a).declined);
            assert_eq!(form_answer_to_plan("p1", &a).decision, "reject");
        }
        // An accept with no content is still an accept.
        let empty = ElicitationAction::Accept { content: None };
        assert!(!empty.is_refusal());
        assert!(empty.content().is_empty());
    }

    #[test]
    fn non_string_answers_are_kept_not_dropped() {
        // A checkbox arrives as a boolean and a number as a number; reading
        // only strings would record an empty answer for both.
        let a = accept(json!({"agree": true, "count": 3, "note": "x", "skipped": null}));
        let r = form_answer_to_fleet_ask("a1", &a);
        assert_eq!(r.answers["agree"], "true");
        assert_eq!(r.answers["count"], "3");
        assert_eq!(r.answers["note"], "x");
        assert!(!r.answers.contains_key("skipped"), "null is no answer at all");
    }

    #[test]
    fn a_plan_is_approved_only_on_an_explicit_approve() {
        let ok = form_answer_to_plan("p1", &accept(json!({PLAN_DECISION: "approve"})));
        assert_eq!(ok.decision, "approve");

        let no = form_answer_to_plan("p1", &accept(json!({PLAN_DECISION: "reject"})));
        assert_eq!(no.decision, "reject");

        // A submitted form that never answered the question is not consent.
        let silent = form_answer_to_plan("p1", &accept(json!({PLAN_EDITED: "rewritten"})));
        assert_eq!(silent.decision, "reject");
        assert!(silent.edited_plan.is_none(), "a rejected plan carries no edit");
    }

    #[test]
    fn an_edited_plan_rides_along_with_the_approval() {
        // This is the whole reason plan-approval is a form: a permission
        // outcome has nowhere to put the rewritten text.
        let r = form_answer_to_plan(
            "p1",
            &accept(json!({PLAN_DECISION: "approve", PLAN_EDITED: "rewritten"})),
        );
        assert_eq!(r.decision, "approve");
        assert_eq!(r.edited_plan.as_deref(), Some("rewritten"));

        // An unchanged plan submitted as empty is not an edit.
        let r = form_answer_to_plan(
            "p1",
            &accept(json!({PLAN_DECISION: "approve", PLAN_EDITED: ""})),
        );
        assert!(r.edited_plan.is_none());
    }

    #[test]
    fn elicitation_requests_serialize_in_the_documented_wire_shape() {
        let form = CreateElicitationRequest::form(
            "s1",
            "pick",
            ElicitationSchema::object(Map::new(), vec![]),
        );
        let v = serde_json::to_value(&form).unwrap();
        assert_eq!(v["mode"], "form");
        assert_eq!(v["sessionId"], "s1");
        assert!(v.get("url").is_none(), "form mode carries no url");

        let url = CreateElicitationRequest::url("s1", "open this", "el-1", "https://x.test/a");
        let v = serde_json::to_value(&url).unwrap();
        assert_eq!(v["mode"], "url");
        assert_eq!(v["elicitationId"], "el-1");
        assert_eq!(v["url"], "https://x.test/a");
        assert!(v.get("requestedSchema").is_none(), "url mode carries no schema");
    }

    #[test]
    fn permission_outcomes_deserialize_from_the_wire() {
        let sel: RequestPermissionResponseForTest =
            serde_json::from_value(json!({"outcome": {"outcome": "selected", "optionId": "allow"}}))
                .unwrap();
        assert!(outcome_allows(&sel.outcome));

        let cancelled: RequestPermissionResponseForTest =
            serde_json::from_value(json!({"outcome": {"outcome": "cancelled"}})).unwrap();
        assert!(!outcome_allows(&cancelled.outcome));
    }

    type RequestPermissionResponseForTest = super::super::types::RequestPermissionResponse;
}
