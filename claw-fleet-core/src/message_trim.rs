//! Transport-side trimming of transcript messages.
//!
//! A session's `tool_result` output and Claude Code's sibling `toolUseResult`
//! payload are, by byte count, ~98% of a transcript's tail (measured: a 50 MB /
//! 866-line session ships a 43 MB payload for its last 500 messages, of which
//! ~42 MB is tool output). Both live inside tool cards that render **collapsed**
//! by default, so shipping them in full on every SessionDetail open moves tens
//! of megabytes across the Tauri IPC / `fleet serve` HTTP boundary that the UI
//! never draws until the reader expands the card.
//!
//! [`trim_messages_for_transport`] rewrites those payloads before they cross the
//! wire: every **large string leaf** (a shell `stdout`, an agent `content`, a
//! file body, a `tool_result` text block) over a threshold is replaced with a
//! short preview plus a marker suffix, and the message is flagged
//! `_fleetTruncated`. Small structural fields are left untouched — crucially
//! `structuredPatch` (an array of short per-line strings) survives intact, so
//! the collapsed header's `+N/−N` diff chips, Bash `stderr` markers, Grep match
//! counts, etc. still render without a refetch.
//!
//! When the reader expands a flagged card, the frontend calls back for the full
//! payload via [`extract_full_tool_result`], keyed by the `tool_use_id`.

use std::path::Path;

use serde_json::{json, Value};

/// String leaves longer than this many bytes are truncated for transport.
/// Below it, the leaf ships inline — most tool outputs are small, and keeping
/// them avoids a refetch round-trip on expand for the common case.
pub const DEFAULT_STRING_THRESHOLD: usize = 4096;

/// How many bytes of a truncated leaf to keep as an inline preview. Enough to
/// give context if the card is somehow shown before the full-fetch resolves;
/// the authoritative content always comes from [`extract_full_tool_result`].
const PREVIEW_BYTES: usize = 1024;

/// Marker key set on a message whose tool output was trimmed. The frontend uses
/// it to decide whether an expanded card must fetch the full payload.
pub const TRUNCATED_FLAG: &str = "_fleetTruncated";

/// Truncate oversized tool output in-place across `msgs`, using
/// [`DEFAULT_STRING_THRESHOLD`]. Returns the number of messages trimmed.
pub fn trim_messages_for_transport(msgs: &mut [Value]) -> usize {
    trim_messages_with_threshold(msgs, DEFAULT_STRING_THRESHOLD)
}

/// [`trim_messages_for_transport`] with an explicit threshold (for tests).
pub fn trim_messages_with_threshold(msgs: &mut [Value], threshold: usize) -> usize {
    let mut trimmed = 0;
    for msg in msgs.iter_mut() {
        if trim_one_message(msg, threshold) {
            trimmed += 1;
        }
    }
    trimmed
}

/// Returns true when anything in this message was truncated.
fn trim_one_message(msg: &mut Value, threshold: usize) -> bool {
    let Some(obj) = msg.as_object_mut() else {
        return false;
    };

    let mut truncated = false;

    // 1. The structured duplicate Claude Code hangs off the message top level.
    if let Some(tur) = obj.get_mut("toolUseResult") {
        truncated |= truncate_large_strings(tur, threshold);
    }

    // 2. `tool_result` blocks inside message.content — the raw output the
    //    expanded card actually displays.
    if let Some(content) = obj.get_mut("message").and_then(|m| m.get_mut("content")) {
        if let Some(blocks) = content.as_array_mut() {
            for block in blocks {
                let is_tool_result = block
                    .get("type")
                    .and_then(|t| t.as_str())
                    .map(|t| t == "tool_result")
                    .unwrap_or(false);
                if !is_tool_result {
                    continue;
                }
                if let Some(c) = block.as_object_mut().and_then(|b| b.get_mut("content")) {
                    truncated |= truncate_large_strings(c, threshold);
                }
            }
        }
    }

    if truncated {
        obj.insert(TRUNCATED_FLAG.to_string(), Value::Bool(true));
    }
    truncated
}

/// Recursively replace every string leaf longer than `threshold` bytes with a
/// preview + marker suffix. Returns true if any leaf was replaced. Non-string
/// leaves and short strings are left untouched; arrays and objects are walked.
fn truncate_large_strings(v: &mut Value, threshold: usize) -> bool {
    match v {
        Value::String(s) => {
            if s.len() > threshold {
                *s = preview_of(s);
                true
            } else {
                false
            }
        }
        Value::Array(arr) => {
            let mut any = false;
            for item in arr {
                any |= truncate_large_strings(item, threshold);
            }
            any
        }
        Value::Object(map) => {
            let mut any = false;
            for (_, item) in map.iter_mut() {
                any |= truncate_large_strings(item, threshold);
            }
            any
        }
        _ => false,
    }
}

/// First [`PREVIEW_BYTES`] (UTF-8-safe) of `s`, plus a marker telling the reader
/// and the frontend that the full body must be fetched on expand.
fn preview_of(s: &str) -> String {
    let mut end = PREVIEW_BYTES.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let dropped = s.len() - end;
    format!(
        "{}\n\n…[Fleet truncated {} bytes — expand to load full output]",
        &s[..end],
        dropped
    )
}

/// Fetch the untrimmed tool output for a single `tool_use_id` from a transcript.
///
/// Scans `jsonl_path` for the (unique) message whose `message.content` holds a
/// `tool_result` block with `tool_use_id == id`, and returns
/// `{ "content": <that block's content>, "toolUseResult": <sibling or null> }`.
/// Reads the whole file, which is cheap (~18 ms on a 50 MB transcript) and only
/// happens once per card expand.
pub fn extract_full_tool_result(jsonl_path: &Path, tool_use_id: &str) -> Result<Value, String> {
    let content = std::fs::read_to_string(jsonl_path).map_err(|e| e.to_string())?;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let Some(blocks) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                continue;
            }
            if block.get("tool_use_id").and_then(|i| i.as_str()) != Some(tool_use_id) {
                continue;
            }
            return Ok(json!({
                "content": block.get("content").cloned().unwrap_or(Value::Null),
                "toolUseResult": msg.get("toolUseResult").cloned().unwrap_or(Value::Null),
            }));
        }
    }
    Err(format!("tool_use_id {tool_use_id} not found"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn big(n: usize) -> String {
        "x".repeat(n)
    }

    #[test]
    fn small_strings_are_left_untouched() {
        let mut msgs = vec![json!({
            "type": "user",
            "message": {"content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "short output"}
            ]},
            "toolUseResult": {"stdout": "ok", "numAdded": 3}
        })];
        let n = trim_messages_with_threshold(&mut msgs, 4096);
        assert_eq!(n, 0);
        assert!(msgs[0].get(TRUNCATED_FLAG).is_none());
        assert_eq!(msgs[0]["toolUseResult"]["numAdded"], 3);
    }

    #[test]
    fn large_tool_result_content_string_is_truncated_and_flagged() {
        let mut msgs = vec![json!({
            "type": "user",
            "message": {"content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": big(10_000)}
            ]}
        })];
        let n = trim_messages_with_threshold(&mut msgs, 4096);
        assert_eq!(n, 1);
        assert_eq!(msgs[0][TRUNCATED_FLAG], Value::Bool(true));
        let c = msgs[0]["message"]["content"][0]["content"].as_str().unwrap();
        assert!(c.len() < 10_000);
        assert!(c.contains("Fleet truncated"));
    }

    #[test]
    fn large_text_block_inside_content_array_is_truncated() {
        let mut msgs = vec![json!({
            "type": "user",
            "message": {"content": [
                {"type": "tool_result", "tool_use_id": "t1",
                 "content": [{"type": "text", "text": big(9000)}]}
            ]}
        })];
        let n = trim_messages_with_threshold(&mut msgs, 4096);
        assert_eq!(n, 1);
        let txt = msgs[0]["message"]["content"][0]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(txt.contains("Fleet truncated"));
    }

    #[test]
    fn structured_patch_lines_survive_because_each_is_short() {
        // A big diff is an array of many short line strings — none over the
        // threshold — so the header's +N/−N chips must keep working.
        let lines: Vec<Value> = (0..500)
            .map(|i| json!(format!("+ line {i}")))
            .collect();
        let mut msgs = vec![json!({
            "type": "user",
            "message": {"content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "ok"}
            ]},
            "toolUseResult": {"structuredPatch": [{"lines": lines}]}
        })];
        let n = trim_messages_with_threshold(&mut msgs, 4096);
        assert_eq!(n, 0, "short per-line strings must not be truncated");
        assert_eq!(
            msgs[0]["toolUseResult"]["structuredPatch"][0]["lines"]
                .as_array()
                .unwrap()
                .len(),
            500
        );
    }

    #[test]
    fn big_stdout_in_tooluseresult_is_truncated() {
        let mut msgs = vec![json!({
            "type": "user",
            "message": {"content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "see meta"}
            ]},
            "toolUseResult": {"stdout": big(20_000), "stderr": "", "interrupted": false}
        })];
        let n = trim_messages_with_threshold(&mut msgs, 4096);
        assert_eq!(n, 1);
        assert!(msgs[0]["toolUseResult"]["stdout"]
            .as_str()
            .unwrap()
            .contains("Fleet truncated"));
        // stderr stays "" so the header's stderr chip logic is unaffected.
        assert_eq!(msgs[0]["toolUseResult"]["stderr"], "");
    }

    #[test]
    fn extract_full_tool_result_recovers_untrimmed_payload() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("message_trim_test_{}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        let line = json!({
            "type": "user",
            "message": {"content": [
                {"type": "tool_result", "tool_use_id": "abc", "content": big(10_000)}
            ]},
            "toolUseResult": {"stdout": big(20_000)}
        });
        writeln!(f, "{}", serde_json::to_string(&line).unwrap()).unwrap();
        drop(f);

        let full = extract_full_tool_result(&path, "abc").unwrap();
        assert_eq!(full["content"].as_str().unwrap().len(), 10_000);
        assert_eq!(full["toolUseResult"]["stdout"].as_str().unwrap().len(), 20_000);

        assert!(extract_full_tool_result(&path, "missing").is_err());
        std::fs::remove_file(path).ok();
    }
}
