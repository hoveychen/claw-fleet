//! Projecting a transcript's tool activity into ACP `tool_call` updates.
//!
//! This is the capability the Responses surface could not express at all: its
//! projection kept only assistant text and dropped every `tool_use` block,
//! because OpenAI's output items have no vocabulary for "read this file, then
//! ran this command". ACP does, so the trace survives.
//!
//! # Where the mapping came from
//!
//! The `name -> ToolKind` table is ordered by a measured distribution, not by
//! guesswork: across 120 recent transcripts and 19,915 `tool_use` blocks, `Bash`
//! was 74.0%, `Edit` 9.9%, `Read` 4.9%, `Write` 3.1%. Three kinds — `execute`,
//! `edit`, `read` — therefore cover ~92% of all calls, which is why they get
//! the care and everything else falls back to `other`.

use serde_json::Value;

use super::types::{
    Diff, ToolCall, ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdate, ToolKind,
};

/// Classify a tool by name.
///
/// `other` is the honest answer for anything unrecognised: a wrong icon is
/// worse than a neutral one, and MCP tools (`mcp__*`) are arbitrary by design.
pub fn kind_for_tool(name: &str) -> ToolKind {
    match name {
        "Bash" | "BashOutput" | "KillShell" => ToolKind::Execute,
        "Edit" | "Write" | "NotebookEdit" | "MultiEdit" => ToolKind::Edit,
        "Read" | "NotebookRead" => ToolKind::Read,
        "Glob" | "Grep" | "WebSearch" | "ToolSearch" => ToolKind::Search,
        "WebFetch" => ToolKind::Fetch,
        // The agent reasoning about its own plan, not acting on anything.
        "Task" | "Agent" | "TodoWrite" | "ExitPlanMode" => ToolKind::Think,
        _ => ToolKind::Other,
    }
}

/// Whether a tool's job is to put a question in front of a human.
///
/// These are the calls that produce a Fleet decision card, so they get two
/// projections: this `tool_call`, and — once the decision channel is wired —
/// the question itself as a `request_permission` or an `elicitation/create`.
/// The client sees both "a tool is waiting" and "a question is being asked",
/// which is the honest description of what is happening.
///
/// They start at [`ToolCallStatus::Pending`] rather than `in_progress`, because
/// the schema defines pending as "the input is either streaming or we're
/// awaiting approval" — nothing is executing while a human is being asked.
pub fn is_interactive_tool(name: &str) -> bool {
    matches!(name, "AskUserQuestion" | "ExitPlanMode")
        || name.ends_with("__fleet__ask")
        || name.ends_with("__fleet__render_a2ui")
}

/// A one-line human summary of a call, for the client's tool list.
///
/// Prefers the argument a person would recognise — the command, the path, the
/// pattern — over the bare tool name, because a list of "Bash / Bash / Bash"
/// tells the reader nothing.
pub fn title_for(name: &str, input: &Value) -> String {
    let arg = |k: &str| input.get(k).and_then(|v| v.as_str());
    let summary = match name {
        "Bash" => arg("description").or_else(|| arg("command")),
        "Read" | "Edit" | "Write" | "NotebookEdit" => arg("file_path"),
        "Glob" | "Grep" => arg("pattern"),
        "WebFetch" => arg("url"),
        "WebSearch" => arg("query"),
        "Task" | "Agent" => arg("description"),
        _ => None,
    };
    match summary {
        Some(s) => {
            let one_line = s.lines().next().unwrap_or(s);
            // Long commands and prompts are common; a title is a label, not a
            // transcript.
            if one_line.chars().count() > 80 {
                let clipped: String = one_line.chars().take(79).collect();
                format!("{name}: {clipped}…")
            } else {
                format!("{name}: {one_line}")
            }
        }
        None => name.to_string(),
    }
}

/// Files a call touches, so a client can follow along in its own editor.
pub fn locations_for(name: &str, input: &Value) -> Vec<ToolCallLocation> {
    let path = match name {
        "Read" | "Edit" | "Write" | "NotebookEdit" | "NotebookRead" => input.get("file_path"),
        _ => None,
    };
    path.and_then(|p| p.as_str())
        .map(|p| {
            vec![ToolCallLocation {
                path: p.to_string(),
                line: input.get("offset").and_then(|v| v.as_u64()).map(|n| n as u32),
            }]
        })
        .unwrap_or_default()
}

/// A `diff` content block for an edit, when the input carries enough to build
/// one.
///
/// This is what ACP offers that a text-only projection cannot: the client
/// renders a real diff instead of a paragraph describing one. `Write` has no
/// prior text, which the schema models as `oldText: null` rather than an empty
/// string — the distinction is "new file" versus "replaced everything".
pub fn diff_for(name: &str, input: &Value) -> Option<ToolCallContent> {
    let path = input.get("file_path")?.as_str()?.to_string();
    match name {
        "Edit" => Some(ToolCallContent::Diff(Diff {
            path,
            old_text: input.get("old_string").and_then(|v| v.as_str()).map(String::from),
            new_text: input.get("new_string").and_then(|v| v.as_str())?.to_string(),
        })),
        "Write" => Some(ToolCallContent::Diff(Diff {
            path,
            old_text: None,
            new_text: input.get("content").and_then(|v| v.as_str())?.to_string(),
        })),
        _ => None,
    }
}

/// Build the `tool_call` for a transcript `tool_use` block.
///
/// Returns `None` for a block with no id — without one there is nothing to
/// correlate a later result against, so reporting it would strand a call in
/// `in_progress` forever.
pub fn tool_call_from_use(block: &Value) -> Option<ToolCall> {
    let id = block.get("id")?.as_str()?.to_string();
    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
    let input = block.get("input").cloned().unwrap_or(Value::Null);

    let mut content = Vec::new();
    if let Some(d) = diff_for(name, &input) {
        content.push(d);
    }

    Some(ToolCall {
        tool_call_id: id,
        title: title_for(name, &input),
        kind: kind_for_tool(name),
        status: initial_status(name),
        content,
        locations: locations_for(name, &input),
        raw_input: (!input.is_null()).then_some(input),
    })
}

/// The status a call starts in.
///
/// Everything is executing by the time it reaches the transcript, except the
/// tools whose whole job is to wait for a person — see [`is_interactive_tool`].
pub fn initial_status(name: &str) -> ToolCallStatus {
    if is_interactive_tool(name) {
        ToolCallStatus::Pending
    } else {
        ToolCallStatus::InProgress
    }
}

/// How much of a tool's output to forward.
///
/// A `Read` of a large file or a chatty build can run to megabytes; the client
/// wants to see what happened, not to be handed the whole artifact over a
/// WebSocket frame.
const MAX_OUTPUT_CHARS: usize = 8_000;

/// Build the `tool_call_update` for a transcript `tool_result` block.
pub fn tool_call_update_from_result(block: &Value) -> Option<ToolCallUpdate> {
    let id = block.get("tool_use_id")?.as_str()?.to_string();
    let failed = block.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);

    let text = result_text(block.get("content"));
    let content = text.map(|t| vec![ToolCallContent::text(clip(&t))]);

    // A result update rides on a tool call the client has already been told
    // about, so it carries no title/kind of its own.
    Some(ToolCallUpdate {
        tool_call_id: id,
        status: Some(if failed { ToolCallStatus::Failed } else { ToolCallStatus::Completed }),
        content,
        ..Default::default()
    })
}

/// Flatten a `tool_result`'s content, which is a bare string in the common case
/// and a block array when the tool returned something structured.
fn result_text(content: Option<&Value>) -> Option<String> {
    let content = content?;
    if let Some(s) = content.as_str() {
        return (!s.is_empty()).then(|| s.to_string());
    }
    let joined: Vec<String> = content
        .as_array()?
        .iter()
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()).map(String::from))
        .collect();
    (!joined.is_empty()).then(|| joined.join("\n"))
}

/// Truncate on a character boundary, saying so. Pure; unit-tested.
fn clip(s: &str) -> String {
    if s.chars().count() <= MAX_OUTPUT_CHARS {
        return s.to_string();
    }
    let head: String = s.chars().take(MAX_OUTPUT_CHARS).collect();
    format!("{head}\n… (truncated)")
}

// ───────────────────── Transcript → session/update ──────────────────

/// Project transcript records into the `session/update` stream.
///
/// Handles both on-disk shapes. Claude writes `{type:"assistant", message:
/// {content:[…]}}` with `text` / `thinking` / `tool_use` blocks; Codex writes
/// `{type:"response_item", payload:{…}}` where a call is `function_call` or
/// `custom_tool_call`, correlated by `call_id` rather than `id`.
///
/// `include_user` is off during a live turn — the client sent that message and
/// does not need it echoed — and on while replaying for `session/load`, where
/// both sides of the conversation are being rebuilt.
pub fn project_updates(messages: &[Value], include_user: bool) -> Vec<super::types::SessionUpdate> {
    use super::types::SessionUpdate as U;
    let mut out = Vec::new();
    // The diff each edit announced, kept so its completion can re-send it.
    // An update's `content` *replaces* the collection (schema: "Replace the
    // content collection"), so letting the bland success text through would
    // erase the diff at the moment the edit lands.
    let mut diffs: std::collections::HashMap<String, ToolCallContent> =
        std::collections::HashMap::new();

    for msg in messages {
        match msg.get("type").and_then(|t| t.as_str()) {
            Some("assistant") => {
                for b in blocks(msg.get("message")) {
                    match b.get("type").and_then(|t| t.as_str()) {
                        Some("text") => push_text(&mut out, b.get("text"), U::agent_text),
                        // Claude keeps the reasoning under its own key, beside
                        // a signature that is not ours to forward.
                        Some("thinking") => push_text(&mut out, b.get("thinking"), U::thought),
                        Some("tool_use") => {
                            if let Some(call) = tool_call_from_use(b) {
                                if let Some(d) = call
                                    .content
                                    .iter()
                                    .find(|c| matches!(c, ToolCallContent::Diff(_)))
                                {
                                    diffs.insert(call.tool_call_id.clone(), d.clone());
                                }
                                out.push(U::ToolCall(call));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("user") => {
                for b in blocks(msg.get("message")) {
                    match b.get("type").and_then(|t| t.as_str()) {
                        Some("tool_result") => {
                            if let Some(mut up) = tool_call_update_from_result(b) {
                                // A succeeded edit: the diff says everything
                                // "the file has been updated successfully"
                                // says, and says it better. A failed one has
                                // no diff worth showing, so its error stands.
                                if up.status != Some(ToolCallStatus::Failed) {
                                    if let Some(d) = diffs.get(&up.tool_call_id) {
                                        up.content = Some(vec![d.clone()]);
                                    }
                                }
                                out.push(U::ToolCallUpdate(up));
                            }
                        }
                        Some("text") if include_user => {
                            push_text(&mut out, b.get("text"), U::user_text)
                        }
                        _ => {}
                    }
                }
                // Claude also writes a bare-string user message.
                if include_user {
                    if let Some(s) =
                        msg.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_str())
                    {
                        push_text(&mut out, Some(&Value::from(s)), U::user_text);
                    }
                }
            }
            Some("response_item") => project_codex_item(&mut out, msg.get("payload"), include_user),
            _ => {}
        }
    }
    out
}

/// One Codex `response_item` payload.
fn project_codex_item(
    out: &mut Vec<super::types::SessionUpdate>,
    payload: Option<&Value>,
    include_user: bool,
) {
    use super::types::SessionUpdate as U;
    let Some(p) = payload else { return };
    match p.get("type").and_then(|t| t.as_str()) {
        Some("message") => {
            let is_user = p.get("role").and_then(|r| r.as_str()) == Some("user");
            if is_user && !include_user {
                return;
            }
            let make = if is_user { U::user_text } else { U::agent_text };
            for b in p.get("content").and_then(|c| c.as_array()).map(|v| v.as_slice()).unwrap_or(&[])
            {
                if matches!(b.get("type").and_then(|t| t.as_str()), Some("output_text" | "input_text"))
                {
                    push_text(out, b.get("text"), make);
                }
            }
        }
        // Codex encrypts its reasoning and usually leaves `summary` empty, so
        // there is normally nothing to show. Forward a summary when there is
        // one; never touch `encrypted_content`.
        Some("reasoning") => {
            for s in p.get("summary").and_then(|s| s.as_array()).map(|v| v.as_slice()).unwrap_or(&[])
            {
                let text = s.as_str().map(Value::from).or_else(|| s.get("text").cloned());
                push_text(out, text.as_ref(), U::thought);
            }
        }
        Some("function_call" | "custom_tool_call") => {
            if let Some(call) = codex_tool_call(p) {
                out.push(U::ToolCall(call));
            }
        }
        Some("function_call_output" | "custom_tool_call_output") => {
            if let Some(up) = codex_tool_result(p) {
                out.push(U::ToolCallUpdate(up));
            }
        }
        _ => {}
    }
}

/// Build a `tool_call` from a Codex call payload.
///
/// Keyed on `call_id`, not `id`: the output payload carries only `call_id`, so
/// using `id` here would leave every Codex call stuck at `in_progress`.
fn codex_tool_call(p: &Value) -> Option<ToolCall> {
    let id = p.get("call_id")?.as_str()?.to_string();
    let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
    // `function_call` carries a JSON *string* of arguments; `custom_tool_call`
    // carries free-form `input`.
    let input = p
        .get("arguments")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .or_else(|| p.get("input").cloned())
        .unwrap_or(Value::Null);

    let mut content = Vec::new();
    if let Some(d) = diff_for(name, &input) {
        content.push(d);
    }
    Some(ToolCall {
        tool_call_id: id,
        title: title_for(name, &input),
        kind: kind_for_tool(name),
        status: initial_status(name),
        content,
        locations: locations_for(name, &input),
        raw_input: (!input.is_null()).then_some(input),
    })
}

fn codex_tool_result(p: &Value) -> Option<ToolCallUpdate> {
    let id = p.get("call_id")?.as_str()?.to_string();
    let text = p.get("output").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
    Some(ToolCallUpdate {
        tool_call_id: id,
        status: Some(ToolCallStatus::Completed),
        content: text.map(|t| vec![ToolCallContent::text(clip(t))]),
        ..Default::default()
    })
}

/// The content blocks of a `message`, or empty when it is not an array.
fn blocks(message: Option<&Value>) -> &[Value] {
    message
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
        .map(|v| v.as_slice())
        .unwrap_or(&[])
}

/// Push a non-empty string field as an update built by `make`.
fn push_text(
    out: &mut Vec<super::types::SessionUpdate>,
    field: Option<&Value>,
    make: fn(String) -> super::types::SessionUpdate,
) {
    if let Some(s) = field.and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        out.push(make(s.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_measured_hot_path_maps_to_the_right_kinds() {
        // Bash 74%, Edit 9.9%, Read 4.9%, Write 3.1% of all calls.
        assert_eq!(kind_for_tool("Bash"), ToolKind::Execute);
        assert_eq!(kind_for_tool("Edit"), ToolKind::Edit);
        assert_eq!(kind_for_tool("Read"), ToolKind::Read);
        assert_eq!(kind_for_tool("Write"), ToolKind::Edit);
    }

    #[test]
    fn the_rest_of_the_table_and_its_fallback() {
        assert_eq!(kind_for_tool("Grep"), ToolKind::Search);
        assert_eq!(kind_for_tool("Glob"), ToolKind::Search);
        assert_eq!(kind_for_tool("WebSearch"), ToolKind::Search);
        assert_eq!(kind_for_tool("WebFetch"), ToolKind::Fetch);
        assert_eq!(kind_for_tool("Task"), ToolKind::Think);
        assert_eq!(kind_for_tool("NotebookEdit"), ToolKind::Edit);
        // MCP tools are arbitrary by design; a neutral icon beats a wrong one.
        assert_eq!(kind_for_tool("mcp__fleet__fleet__plan"), ToolKind::Other);
        assert_eq!(kind_for_tool("SomethingNewNextYear"), ToolKind::Other);
    }

    #[test]
    fn titles_name_the_argument_a_person_would_recognise() {
        assert_eq!(
            title_for("Bash", &json!({"command": "ls -la", "description": "List files"})),
            "Bash: List files",
            "the human description wins over the raw command"
        );
        assert_eq!(title_for("Bash", &json!({"command": "ls -la"})), "Bash: ls -la");
        assert_eq!(title_for("Read", &json!({"file_path": "/w/a.rs"})), "Read: /w/a.rs");
        assert_eq!(title_for("Grep", &json!({"pattern": "TODO"})), "Grep: TODO");
        // Nothing recognisable is still better than an empty label.
        assert_eq!(title_for("Bash", &json!({})), "Bash");
        assert_eq!(title_for("Whatever", &json!({"x": 1})), "Whatever");
    }

    #[test]
    fn titles_stay_one_short_line() {
        let long = "x".repeat(500);
        let t = title_for("Bash", &json!({"command": long}));
        assert!(t.chars().count() < 100, "a title is a label, not a transcript: {}", t.len());
        assert!(t.ends_with('…'));

        let multi = title_for("Bash", &json!({"command": "first\nsecond\nthird"}));
        assert_eq!(multi, "Bash: first", "a multi-line command must not break the list");
    }

    #[test]
    fn an_edit_becomes_a_real_diff() {
        // The payload shape is taken from an actual transcript, not invented.
        let input = json!({
            "file_path": "/w/a.rs",
            "old_string": "let x = 1;",
            "new_string": "let x = 2;",
            "replace_all": false
        });
        match diff_for("Edit", &input).unwrap() {
            ToolCallContent::Diff(d) => {
                assert_eq!(d.path, "/w/a.rs");
                assert_eq!(d.old_text.as_deref(), Some("let x = 1;"));
                assert_eq!(d.new_text, "let x = 2;");
            }
            other => panic!("expected a diff, got {other:?}"),
        }
    }

    #[test]
    fn a_write_has_no_prior_text_rather_than_empty_text() {
        // null vs "" is the difference between "new file" and "replaced
        // everything", and clients render those differently.
        match diff_for("Write", &json!({"file_path": "/w/new.rs", "content": "fn main() {}"}))
            .unwrap()
        {
            ToolCallContent::Diff(d) => {
                assert_eq!(d.old_text, None);
                assert_eq!(d.new_text, "fn main() {}");
            }
            other => panic!("expected a diff, got {other:?}"),
        }
    }

    #[test]
    fn tools_that_are_not_edits_produce_no_diff() {
        assert!(diff_for("Bash", &json!({"command": "ls"})).is_none());
        assert!(diff_for("Read", &json!({"file_path": "/w/a.rs"})).is_none());
        // An Edit missing its replacement cannot be rendered as a diff.
        assert!(diff_for("Edit", &json!({"file_path": "/w/a.rs"})).is_none());
    }

    /// A finished edit must still be a diff.
    ///
    /// The schema says a `tool_call_update`'s content field "Replace[s] the
    /// content collection" — so sending the tool_result text as content wipes
    /// out the diff the `tool_call` carried, the instant the edit succeeds.
    /// The whole point of an editor client is that it renders that diff, and
    /// Zed showed "The file … has been updated successfully." where the diff
    /// should have been (observed 2026-08-27).
    ///
    /// Claude's success text for an edit says nothing a diff does not say
    /// better, so the diff wins. A *failed* edit is the opposite case: there
    /// is no diff worth showing and the error is the only useful content.
    #[test]
    fn a_completed_edit_keeps_its_diff_instead_of_the_result_text() {
        let messages = vec![
            json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": "t1", "name": "Edit", "input": {
                    "file_path": "/w/a.rs", "old_string": "one", "new_string": "two"
                }}
            ]}}),
            json!({"type": "user", "message": {"content": [
                {"type": "tool_result", "tool_use_id": "t1",
                 "content": "The file /w/a.rs has been updated successfully."}
            ]}}),
        ];

        let ups = project_updates(&messages, false);
        let update = ups
            .iter()
            .find_map(|u| match u {
                super::super::types::SessionUpdate::ToolCallUpdate(t) if t.tool_call_id == "t1" => {
                    Some(t)
                }
                _ => None,
            })
            .expect("the edit reports a completion");

        assert_eq!(update.status, Some(ToolCallStatus::Completed));
        let content = update.content.as_ref().expect(
            "an update that replaces content must still carry the diff, or the client loses it",
        );
        assert!(
            content.iter().any(|c| matches!(c, ToolCallContent::Diff(_))),
            "the completed edit still renders as a diff, got {content:?}"
        );

        // A failed edit keeps the error instead — there is nothing to show.
        let failed = vec![
            messages[0].clone(),
            json!({"type": "user", "message": {"content": [
                {"type": "tool_result", "tool_use_id": "t1", "is_error": true,
                 "content": "String to replace not found in file."}
            ]}}),
        ];
        let ups = project_updates(&failed, false);
        let update = ups
            .iter()
            .find_map(|u| match u {
                super::super::types::SessionUpdate::ToolCallUpdate(t) if t.tool_call_id == "t1" => {
                    Some(t)
                }
                _ => None,
            })
            .expect("the failed edit reports too");
        assert_eq!(update.status, Some(ToolCallStatus::Failed));
        let text = format!("{:?}", update.content);
        assert!(
            text.contains("not found"),
            "a failed edit surfaces why it failed, got {text}"
        );
    }

    #[test]
    fn locations_are_reported_for_file_tools_only() {
        let locs = locations_for("Read", &json!({"file_path": "/w/a.rs", "offset": 42}));
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].path, "/w/a.rs");
        assert_eq!(locs[0].line, Some(42));

        assert!(locations_for("Bash", &json!({"command": "ls"})).is_empty());
        assert!(locations_for("Read", &json!({})).is_empty());
    }

    #[test]
    fn a_tool_use_block_projects_with_its_id_preserved() {
        let block = json!({
            "type": "tool_use",
            "id": "toolu_01ABC",
            "name": "Edit",
            "input": {"file_path": "/w/a.rs", "old_string": "a", "new_string": "b"}
        });
        let call = tool_call_from_use(&block).unwrap();
        // The id must survive verbatim — it is what a later result correlates
        // against.
        assert_eq!(call.tool_call_id, "toolu_01ABC");
        assert_eq!(call.kind, ToolKind::Edit);
        assert_eq!(call.status, ToolCallStatus::InProgress);
        assert_eq!(call.locations.len(), 1);
        assert!(matches!(call.content.first(), Some(ToolCallContent::Diff(_))));
        assert!(call.raw_input.is_some());
    }

    #[test]
    fn a_tool_that_asks_a_human_starts_pending_not_in_progress() {
        // The schema's `pending` is "awaiting approval". Nothing is executing
        // while a person is being asked, and this is the tool half of the
        // double projection — the question itself follows on the decision
        // channel.
        for name in [
            "AskUserQuestion",
            "ExitPlanMode",
            "mcp__fleet__fleet__ask",
            "mcp__fleet__fleet__render_a2ui",
        ] {
            assert!(is_interactive_tool(name), "{name} asks a human");
            assert_eq!(initial_status(name), ToolCallStatus::Pending, "{name}");
        }
        // Everything else really is running by the time it hits the transcript.
        for name in ["Bash", "Edit", "Read", "mcp__fleet__fleet__plan", "WebFetch"] {
            assert!(!is_interactive_tool(name), "{name} does not ask a human");
            assert_eq!(initial_status(name), ToolCallStatus::InProgress, "{name}");
        }
    }

    #[test]
    fn the_pending_status_survives_the_projection_for_both_agents() {
        let claude = json!({"id": "t1", "name": "mcp__fleet__fleet__ask", "input": {}});
        assert_eq!(tool_call_from_use(&claude).unwrap().status, ToolCallStatus::Pending);

        let codex = json!({"type": "response_item", "payload": {
            "type": "custom_tool_call", "call_id": "c1",
            "name": "mcp__fleet__fleet__ask", "input": "{}"
        }});
        match &project_updates(std::slice::from_ref(&codex), false)[0] {
            U::ToolCall(c) => assert_eq!(c.status, ToolCallStatus::Pending),
            other => panic!("expected a tool_call, got {other:?}"),
        }
    }

    #[test]
    fn answering_an_interactive_tool_completes_it() {
        // The result arrives once the human answers, which is what closes the
        // pending call — the same transition an ordinary tool makes.
        let up = tool_call_update_from_result(&json!({
            "tool_use_id": "t1", "content": "{\"answers\":{\"Q\":\"A\"}}"
        }))
        .unwrap();
        assert_eq!(up.status, Some(ToolCallStatus::Completed));
    }

    #[test]
    fn an_id_less_block_is_skipped_rather_than_stranded() {
        // With no id there is nothing to correlate a result against, so the
        // call would sit at in_progress forever.
        assert!(tool_call_from_use(&json!({"type": "tool_use", "name": "Bash"})).is_none());
    }

    #[test]
    fn a_result_completes_the_call_it_names() {
        let block = json!({
            "type": "tool_result",
            "tool_use_id": "toolu_01ABC",
            "content": "done"
        });
        let up = tool_call_update_from_result(&block).unwrap();
        assert_eq!(up.tool_call_id, "toolu_01ABC");
        assert_eq!(up.status, Some(ToolCallStatus::Completed));
        assert_eq!(up.content, Some(vec![ToolCallContent::text("done")]));
    }

    #[test]
    fn an_errored_result_fails_the_call() {
        let up = tool_call_update_from_result(&json!({
            "tool_use_id": "t1", "content": "boom", "is_error": true
        }))
        .unwrap();
        assert_eq!(up.status, Some(ToolCallStatus::Failed));
    }

    #[test]
    fn result_content_reads_both_string_and_block_forms() {
        assert_eq!(result_text(Some(&json!("plain"))).as_deref(), Some("plain"));
        assert_eq!(
            result_text(Some(&json!([{"type": "text", "text": "a"}, {"type": "text", "text": "b"}])))
                .as_deref(),
            Some("a\nb")
        );
        assert_eq!(result_text(Some(&json!(""))), None);
        assert_eq!(result_text(Some(&json!([]))), None);
        assert_eq!(result_text(None), None);
    }

    #[test]
    fn oversized_output_is_clipped_on_a_char_boundary() {
        // A Read of a large file must not be forwarded whole.
        let big = "あ".repeat(MAX_OUTPUT_CHARS + 500);
        let out = clip(&big);
        assert!(out.chars().count() < big.chars().count());
        assert!(out.ends_with("… (truncated)"), "truncation must be visible, not silent");
        // Short output is untouched.
        assert_eq!(clip("small"), "small");
    }

    #[test]
    fn tool_call_serializes_in_the_schema_shape() {
        let call = tool_call_from_use(&json!({
            "id": "t1", "name": "Bash", "input": {"command": "ls"}
        }))
        .unwrap();
        let v = serde_json::to_value(&call).unwrap();
        assert_eq!(v["toolCallId"], "t1");
        assert_eq!(v["kind"], "execute");
        assert_eq!(v["status"], "in_progress");
        // Empty collections are omitted rather than sent as [].
        assert!(v.get("locations").is_none());
        assert!(v.get("content").is_none());
    }

    #[test]
    fn an_update_sends_only_what_changed() {
        let up = ToolCallUpdate::status("t1", ToolCallStatus::Completed);
        let v = serde_json::to_value(&up).unwrap();
        assert_eq!(v["toolCallId"], "t1");
        assert_eq!(v["status"], "completed");
        assert!(v.get("content").is_none(), "absent means unchanged");
        assert!(v.get("rawOutput").is_none());
    }

    // ── transcript projection ───────────────────────────────────────

    use super::super::types::SessionUpdate as U;

    #[test]
    fn a_claude_turn_projects_text_thinking_and_the_tool_round_trip() {
        // Shapes taken from real transcripts: thinking lives under its own
        // key beside a signature, tool_use carries `id`, tool_result carries
        // `tool_use_id`.
        let msgs = vec![
            json!({"type": "assistant", "message": {"content": [
                {"type": "thinking", "thinking": "let me look", "signature": "sig"},
                {"type": "text", "text": "checking"},
                {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}}
            ]}}),
            json!({"type": "user", "message": {"content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "a.txt"}
            ]}}),
            json!({"type": "assistant", "message": {"content": [
                {"type": "text", "text": "there is one file"}
            ]}}),
        ];
        let ups = project_updates(&msgs, false);
        assert_eq!(ups.len(), 5);
        assert!(matches!(ups[0], U::AgentThoughtChunk { .. }), "thinking is its own channel");
        assert!(matches!(ups[1], U::AgentMessageChunk { .. }));
        match &ups[2] {
            U::ToolCall(c) => {
                assert_eq!(c.tool_call_id, "t1");
                assert_eq!(c.kind, ToolKind::Execute);
            }
            other => panic!("expected a tool_call, got {other:?}"),
        }
        match &ups[3] {
            U::ToolCallUpdate(u) => {
                assert_eq!(u.tool_call_id, "t1", "the result must name the call it completes");
                assert_eq!(u.status, Some(ToolCallStatus::Completed));
            }
            other => panic!("expected a tool_call_update, got {other:?}"),
        }
        assert!(matches!(ups[4], U::AgentMessageChunk { .. }));
    }

    #[test]
    fn the_signature_beside_a_thinking_block_is_not_forwarded() {
        let msgs = vec![json!({"type": "assistant", "message": {"content": [
            {"type": "thinking", "thinking": "reasoning", "signature": "SECRET"}
        ]}})];
        let json = serde_json::to_string(&project_updates(&msgs, false)).unwrap();
        assert!(json.contains("reasoning"));
        assert!(!json.contains("SECRET"), "the signature is not ours to forward");
    }

    #[test]
    fn user_messages_are_echoed_only_when_replaying() {
        let msgs = vec![
            json!({"type": "user", "message": {"content": [{"type": "text", "text": "hello"}]}}),
            // Claude also writes a bare-string user message.
            json!({"type": "user", "message": {"content": "bare string"}}),
        ];
        // During a live turn the client sent these; echoing them would double
        // them up in its transcript.
        assert!(project_updates(&msgs, false).is_empty());

        let replayed = project_updates(&msgs, true);
        assert_eq!(replayed.len(), 2, "both shapes must survive a replay");
        assert!(replayed.iter().all(|u| matches!(u, U::UserMessageChunk { .. })));
    }

    #[test]
    fn a_codex_turn_projects_through_call_id() {
        // Codex's output payload carries only `call_id`; keying the call on
        // `id` would leave every one of them stuck at in_progress.
        let msgs = vec![
            json!({"type": "response_item", "payload": {
                "type": "function_call", "id": "fc_1", "call_id": "call_A",
                "name": "Read", "arguments": "{\"file_path\":\"/w/a.rs\"}"
            }}),
            json!({"type": "response_item", "payload": {
                "type": "function_call_output", "call_id": "call_A", "output": "fn main"
            }}),
            json!({"type": "response_item", "payload": {
                "type": "message", "role": "assistant",
                "content": [{"type": "output_text", "text": "read it"}]
            }}),
        ];
        let ups = project_updates(&msgs, false);
        assert_eq!(ups.len(), 3);
        match &ups[0] {
            U::ToolCall(c) => {
                assert_eq!(c.tool_call_id, "call_A");
                assert_eq!(c.kind, ToolKind::Read);
                // `arguments` is a JSON *string* and must be parsed, or the
                // title and locations come out empty.
                assert_eq!(c.locations.first().map(|l| l.path.as_str()), Some("/w/a.rs"));
            }
            other => panic!("expected a tool_call, got {other:?}"),
        }
        match &ups[1] {
            U::ToolCallUpdate(u) => assert_eq!(u.tool_call_id, "call_A"),
            other => panic!("expected an update, got {other:?}"),
        }
        assert!(matches!(ups[2], U::AgentMessageChunk { .. }));
    }

    #[test]
    fn a_codex_custom_tool_call_uses_its_free_form_input() {
        let msgs = vec![json!({"type": "response_item", "payload": {
            "type": "custom_tool_call", "call_id": "call_B", "name": "exec",
            "input": "console.log(1)"
        }})];
        match &project_updates(&msgs, false)[0] {
            U::ToolCall(c) => {
                assert_eq!(c.tool_call_id, "call_B");
                assert_eq!(c.raw_input, Some(json!("console.log(1)")));
            }
            other => panic!("expected a tool_call, got {other:?}"),
        }
    }

    #[test]
    fn codex_encrypted_reasoning_yields_nothing_rather_than_noise() {
        // Codex encrypts its reasoning and usually leaves `summary` empty.
        let empty = vec![json!({"type": "response_item", "payload": {
            "type": "reasoning", "summary": [], "encrypted_content": "gAAAAA..."
        }})];
        let ups = project_updates(&empty, false);
        assert!(ups.is_empty());
        let json = serde_json::to_string(&ups).unwrap();
        assert!(!json.contains("gAAAAA"), "encrypted content is never forwarded");

        // When a summary is present it is worth showing.
        let summarised = vec![json!({"type": "response_item", "payload": {
            "type": "reasoning", "summary": ["planned the edit"]
        }})];
        assert!(matches!(project_updates(&summarised, false)[0], U::AgentThoughtChunk { .. }));
    }

    #[test]
    fn malformed_records_are_skipped_not_fatal() {
        let msgs = vec![
            json!({"type": "assistant"}),
            json!({"type": "assistant", "message": {"content": "not an array"}}),
            json!({"type": "response_item"}),
            json!({"type": "something_new"}),
            json!({}),
            // Empty text must not produce an empty chunk.
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": ""}]}}),
        ];
        assert!(project_updates(&msgs, true).is_empty());
    }

    #[test]
    fn a_diff_omits_old_text_when_there_is_none() {
        let c = diff_for("Write", &json!({"file_path": "/w/n.rs", "content": "x"})).unwrap();
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["type"], "diff");
        assert_eq!(v["path"], "/w/n.rs");
        assert_eq!(v["newText"], "x");
        assert!(v.get("oldText").is_none());
    }
}
