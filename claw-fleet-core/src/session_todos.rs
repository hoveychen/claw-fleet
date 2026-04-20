//! Extract the current TodoWrite state from a session's parsed JSONL messages.
//!
//! TodoWrite is a full-replace tool: every invocation overwrites the complete
//! list.  The latest `tool_use` block named "TodoWrite" therefore carries the
//! session's current todo state.  Callers feed the messages returned by
//! `Backend::get_messages` into [`extract_latest_todos`] to obtain that list.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub content: String,
    pub active_form: String,
    /// One of: "pending", "in_progress", "completed".
    pub status: String,
}

/// Aggregate view of a session's current TodoWrite list — suitable for the
/// compact progress row on a session card.  `None` on `SessionInfo` means the
/// session has never invoked TodoWrite; a populated summary with all zeros
/// means the user cleared the list explicitly.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TodoSummary {
    pub completed: u32,
    pub in_progress: u32,
    pub pending: u32,
    /// `activeForm` of the first in-progress item (falls back to its `content`
    /// when `activeForm` is empty).  `None` when nothing is in progress.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_active: Option<String>,
}

impl TodoSummary {
    pub fn total(&self) -> u32 {
        self.completed + self.in_progress + self.pending
    }
}

fn parse_todo_block(block: &Value) -> Option<Vec<TodoItem>> {
    if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
        return None;
    }
    if block.get("name").and_then(|n| n.as_str()) != Some("TodoWrite") {
        return None;
    }
    let todos = block
        .get("input")
        .and_then(|i| i.get("todos"))
        .and_then(|t| t.as_array())?;
    Some(
        todos
            .iter()
            .filter_map(|t| {
                let content = t.get("content")?.as_str()?.to_string();
                let active_form = t
                    .get("activeForm")
                    .and_then(|a| a.as_str())
                    .unwrap_or("")
                    .to_string();
                let status = t
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("pending")
                    .to_string();
                Some(TodoItem {
                    content,
                    active_form,
                    status,
                })
            })
            .collect(),
    )
}

/// Scan `messages` (as returned by `Backend::get_messages`) from newest to
/// oldest and return the todos carried by the most recent TodoWrite tool_use.
/// Returns an empty vec when the session has never invoked TodoWrite or when
/// the last call set an empty list.
pub fn extract_latest_todos(messages: &[Value]) -> Vec<TodoItem> {
    for msg in messages.iter().rev() {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(content) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        for block in content.iter().rev() {
            if let Some(todos) = parse_todo_block(block) {
                return todos;
            }
        }
    }
    Vec::new()
}

pub fn summarize(todos: &[TodoItem]) -> TodoSummary {
    let mut s = TodoSummary::default();
    for t in todos {
        match t.status.as_str() {
            "completed" => s.completed += 1,
            "in_progress" => {
                s.in_progress += 1;
                if s.current_active.is_none() {
                    let label = if t.active_form.is_empty() {
                        t.content.clone()
                    } else {
                        t.active_form.clone()
                    };
                    s.current_active = Some(label);
                }
            }
            _ => s.pending += 1,
        }
    }
    s
}

/// Scan raw JSONL lines from newest to oldest and return the summary of the
/// most recent TodoWrite tool_use.  Stops at the first match to minimise work
/// during the session scan path.  Returns `None` when the file contains no
/// TodoWrite invocation (still distinguishable from "list explicitly
/// cleared", which yields `Some(TodoSummary::default())`).
pub fn latest_todo_summary_from_lines(lines: &[&str]) -> Option<TodoSummary> {
    for line in lines.iter().rev() {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(content) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        for block in content.iter().rev() {
            if let Some(todos) = parse_todo_block(block) {
                return Some(summarize(&todos));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assistant_with_todowrite(todos: Value) -> Value {
        json!({
            "type": "assistant",
            "message": {
                "content": [
                    {
                        "type": "tool_use",
                        "name": "TodoWrite",
                        "input": { "todos": todos }
                    }
                ]
            }
        })
    }

    #[test]
    fn empty_messages_returns_empty() {
        assert!(extract_latest_todos(&[]).is_empty());
    }

    #[test]
    fn returns_todos_from_single_todowrite() {
        let msgs = vec![assistant_with_todowrite(json!([
            {"content": "A", "activeForm": "Doing A", "status": "completed"},
            {"content": "B", "activeForm": "Doing B", "status": "in_progress"}
        ]))];
        let out = extract_latest_todos(&msgs);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].content, "A");
        assert_eq!(out[0].status, "completed");
        assert_eq!(out[1].active_form, "Doing B");
    }

    #[test]
    fn last_todowrite_wins_across_messages() {
        let msgs = vec![
            assistant_with_todowrite(json!([
                {"content": "old-1", "activeForm": "doing old-1", "status": "pending"}
            ])),
            assistant_with_todowrite(json!([
                {"content": "new-1", "activeForm": "doing new-1", "status": "completed"},
                {"content": "new-2", "activeForm": "doing new-2", "status": "pending"}
            ])),
        ];
        let out = extract_latest_todos(&msgs);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].content, "new-1");
    }

    #[test]
    fn empty_todos_list_returns_empty() {
        let msgs = vec![assistant_with_todowrite(json!([]))];
        assert!(extract_latest_todos(&msgs).is_empty());
    }

    #[test]
    fn ignores_non_todowrite_tool_uses() {
        let msgs = vec![json!({
            "type": "assistant",
            "message": {
                "content": [
                    {"type": "tool_use", "name": "Bash", "input": {"cmd": "ls"}}
                ]
            }
        })];
        assert!(extract_latest_todos(&msgs).is_empty());
    }

    #[test]
    fn ignores_user_and_system_messages() {
        let msgs = vec![
            json!({"type": "user", "message": {"content": "hello"}}),
            json!({"type": "system", "message": {"content": "boot"}}),
        ];
        assert!(extract_latest_todos(&msgs).is_empty());
    }

    #[test]
    fn malformed_todo_item_missing_content_is_skipped() {
        // An item without `content` is not a valid TodoItem → drop it,
        // but keep sibling valid items.
        let msgs = vec![assistant_with_todowrite(json!([
            {"activeForm": "orphan", "status": "pending"},
            {"content": "ok", "activeForm": "doing ok", "status": "in_progress"}
        ]))];
        let out = extract_latest_todos(&msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, "ok");
    }

    #[test]
    fn missing_active_form_or_status_defaults() {
        let msgs = vec![assistant_with_todowrite(json!([
            {"content": "bare"}
        ]))];
        let out = extract_latest_todos(&msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].active_form, "");
        assert_eq!(out[0].status, "pending");
    }

    #[test]
    fn summarize_counts_statuses_and_picks_first_active() {
        let todos = vec![
            TodoItem { content: "a".into(), active_form: "doing a".into(), status: "completed".into() },
            TodoItem { content: "b".into(), active_form: "doing b".into(), status: "in_progress".into() },
            TodoItem { content: "c".into(), active_form: "doing c".into(), status: "in_progress".into() },
            TodoItem { content: "d".into(), active_form: "doing d".into(), status: "pending".into() },
        ];
        let s = summarize(&todos);
        assert_eq!(s.completed, 1);
        assert_eq!(s.in_progress, 2);
        assert_eq!(s.pending, 1);
        assert_eq!(s.total(), 4);
        assert_eq!(s.current_active.as_deref(), Some("doing b"));
    }

    #[test]
    fn summarize_falls_back_to_content_when_active_form_empty() {
        let todos = vec![TodoItem {
            content: "Bare content".into(),
            active_form: "".into(),
            status: "in_progress".into(),
        }];
        let s = summarize(&todos);
        assert_eq!(s.current_active.as_deref(), Some("Bare content"));
    }

    #[test]
    fn summarize_empty_returns_all_zero() {
        let s = summarize(&[]);
        assert_eq!(s, TodoSummary::default());
        assert_eq!(s.total(), 0);
        assert!(s.current_active.is_none());
    }

    #[test]
    fn lines_summary_none_when_no_todowrite() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#,
            r#"{"type":"user","message":{"content":"hello"}}"#,
        ];
        assert!(latest_todo_summary_from_lines(&lines).is_none());
    }

    #[test]
    fn lines_summary_picks_last_todowrite_across_lines() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"TodoWrite","input":{"todos":[{"content":"old","activeForm":"old","status":"completed"}]}}]}}"#,
            r#"{"type":"user","message":{"content":"noise"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"TodoWrite","input":{"todos":[{"content":"A","activeForm":"Doing A","status":"in_progress"},{"content":"B","activeForm":"Doing B","status":"pending"}]}}]}}"#,
        ];
        let s = latest_todo_summary_from_lines(&lines).expect("summary");
        assert_eq!(s.total(), 2);
        assert_eq!(s.in_progress, 1);
        assert_eq!(s.pending, 1);
        assert_eq!(s.current_active.as_deref(), Some("Doing A"));
    }

    #[test]
    fn lines_summary_distinguishes_cleared_from_never() {
        let cleared = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"TodoWrite","input":{"todos":[]}}]}}"#,
        ];
        let s = latest_todo_summary_from_lines(&cleared).expect("summary");
        assert_eq!(s, TodoSummary::default());
    }

    #[test]
    fn lines_summary_tolerates_malformed_lines() {
        let lines = vec![
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"TodoWrite","input":{"todos":[{"content":"A","activeForm":"Doing A","status":"completed"}]}}]}}"#,
            // truncated line (broken JSON at the tail) — should be skipped, not crash
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"broken"#,
        ];
        let s = latest_todo_summary_from_lines(&lines).expect("summary");
        assert_eq!(s.completed, 1);
    }

    #[test]
    fn multiple_todowrite_in_same_message_takes_last() {
        let msg = json!({
            "type": "assistant",
            "message": {
                "content": [
                    {"type": "tool_use", "name": "TodoWrite", "input": {"todos": [
                        {"content": "first", "activeForm": "doing first", "status": "pending"}
                    ]}},
                    {"type": "tool_use", "name": "TodoWrite", "input": {"todos": [
                        {"content": "second", "activeForm": "doing second", "status": "completed"}
                    ]}}
                ]
            }
        });
        let out = extract_latest_todos(&[msg]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, "second");
    }
}
