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
        // The block exists because the agent decided to call it; execution is
        // under way by the time we see it in the transcript.
        status: ToolCallStatus::InProgress,
        content,
        locations: locations_for(name, &input),
        raw_input: (!input.is_null()).then_some(input),
    })
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

    Some(ToolCallUpdate {
        tool_call_id: id,
        status: Some(if failed { ToolCallStatus::Failed } else { ToolCallStatus::Completed }),
        content,
        raw_output: None,
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
