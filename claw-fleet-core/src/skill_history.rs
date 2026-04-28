//! Per-session Skill invocation history.
//!
//! Walks a session's main jsonl plus any sibling
//! `<session-id>/subagents/agent-*.jsonl` files and extracts every
//! `tool_use` block whose `name` is `Skill`. Subagent invocations are
//! flagged with `is_subagent: true`; sidechain entries that happen to
//! appear in the main jsonl are flagged the same way.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SkillInvocation {
    pub skill: String,
    pub args: Option<String>,
    pub timestamp: String,
    pub is_subagent: bool,
}

/// Pull every Skill `tool_use` block out of `messages`. When `force_subagent`
/// is true, every entry is flagged as a subagent invocation regardless of
/// `isSidechain`. Otherwise the per-message `isSidechain` flag is honored.
pub fn extract_from_messages(messages: &[Value], force_subagent: bool) -> Vec<SkillInvocation> {
    let mut out = Vec::new();
    for msg in messages {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let is_subagent = force_subagent
            || msg
                .get("isSidechain")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
        let timestamp = msg
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        let Some(blocks) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                continue;
            }
            if block.get("name").and_then(|n| n.as_str()) != Some("Skill") {
                continue;
            }
            let Some(skill) = block
                .get("input")
                .and_then(|i| i.get("skill"))
                .and_then(|s| s.as_str())
            else {
                continue;
            };
            let args = block
                .get("input")
                .and_then(|i| i.get("args"))
                .and_then(|a| a.as_str())
                .map(|s| s.to_string());
            out.push(SkillInvocation {
                skill: skill.to_string(),
                args,
                timestamp: timestamp.clone(),
                is_subagent,
            });
        }
    }
    out
}

/// Given the path to a main session jsonl, return the sibling subagent
/// jsonl files (`<sid>/subagents/agent-*.jsonl`). Empty when no such
/// directory exists.
pub fn subagent_jsonl_paths(main_jsonl: &Path) -> Vec<PathBuf> {
    let Some(parent) = main_jsonl.parent() else {
        return vec![];
    };
    let Some(stem) = main_jsonl.file_stem().and_then(|s| s.to_str()) else {
        return vec![];
    };
    let dir = parent.join(stem).join("subagents");
    let Ok(entries) = fs::read_dir(&dir) else {
        return vec![];
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    paths.sort();
    paths
}

/// Stable ordering: by timestamp ascending; entries without a timestamp
/// keep their original relative order at the tail.
pub fn sort_by_timestamp(items: &mut [SkillInvocation]) {
    items.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
}
