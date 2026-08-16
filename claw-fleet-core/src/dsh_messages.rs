//! Normalise dsh's durable session events into the transcript shape Fleet's UI
//! reads.
//!
//! Every Fleet client — the desktop `MessageList`, the mobile web app, the
//! search index — consumes one message vocabulary: Claude Code's JSONL records
//! (`{"type":"user"|"assistant","message":{role,content:[…]}}`, tool results as
//! `tool_result` blocks nested in a *user* record and keyed by `tool_use_id`).
//! `codex_source` already converts its rollout into that vocabulary; without the
//! same step a dsh session lists fine and then opens empty, because its native
//! records are typed `user/message` / `assistant/chunk` / `tool/result` and
//! match nothing the renderer looks for.
//!
//! ## What is dropped, and why that is not data loss
//!
//! * `assistant/chunk` — the streaming deltas. One turn here produced 97 chunks
//!   against 3 `assistant/message` records; the message is the authoritative
//!   assembled form and the chunks are how it arrived. (Live status *does* read
//!   the chunks — see [`crate::dsh_events`] — but that is a phase, not a
//!   transcript.)
//! * `tool/call` — a duplicate. The `assistant/message` that precedes it already
//!   carries the same call as a `tool-call` content block (verified across
//!   captured sessions: seq 32 `assistant/message` then seq 33 `tool/call`,
//!   identical `callId`/`name`/`arguments`). Emitting both would render every
//!   tool twice.
//! * Turn/step lifecycle, `session/title*`, `request/*`, `permission/preset`,
//!   `sandbox/mode`, `approval/policy`, `agent/inbox/spliced` — session
//!   bookkeeping with no place in a conversation.

use serde_json::{json, Value};

/// Convert a session's durable events into Claude-shaped transcript records.
pub fn normalize(events: &[Value]) -> Vec<Value> {
    events.iter().filter_map(normalize_event).collect()
}

/// dsh stamps events with epoch milliseconds; the renderer wants what Claude
/// Code writes, an RFC3339 instant.
fn timestamp_of(event: &Value) -> Value {
    match event.get("time").and_then(Value::as_i64) {
        Some(ms) => match chrono::DateTime::from_timestamp_millis(ms) {
            Some(dt) => json!(dt.to_rfc3339()),
            None => Value::Null,
        },
        None => Value::Null,
    }
}

/// Map dsh's usage block onto Claude's field names, so the per-turn token line
/// has something to read.
fn usage_of(data: &Value) -> Option<Value> {
    let usage = data.get("usage")?;
    let get = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    Some(json!({
        "input_tokens": get("inputTokens"),
        "output_tokens": get("outputTokens"),
        "cache_read_input_tokens": get("cacheReadTokens"),
        "cache_creation_input_tokens": get("cacheWriteTokens"),
    }))
}

/// Convert one assistant content block.
///
/// Text passes through unchanged (dsh and Claude agree on that shape). A
/// `tool-call` becomes a `tool_use`, and its `arguments` — a JSON *string* on
/// the wire — is parsed into the object the tool cards expect. A string that
/// does not parse is preserved under `raw` rather than dropped: a malformed
/// argument blob is exactly what someone reading the transcript needs to see.
fn assistant_block(block: &Value) -> Option<Value> {
    match block.get("type").and_then(Value::as_str)? {
        "text" => Some(block.clone()),
        "tool-call" => {
            let arguments = block.get("arguments").and_then(Value::as_str).unwrap_or("");
            let input = serde_json::from_str::<Value>(arguments)
                .ok()
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({ "raw": arguments }));
            Some(json!({
                "type": "tool_use",
                "id": block.get("id").cloned().unwrap_or(Value::Null),
                "name": block.get("name").cloned().unwrap_or(Value::Null),
                "input": input,
            }))
        }
        // Reasoning blocks and anything a later dsh release adds: keep the
        // block rather than silently swallowing the turn's content.
        _ => Some(block.clone()),
    }
}

/// Flatten a `tool-result`'s content blocks into the string the result cards
/// parse. dsh nests `[{type:"text",text}]`; Claude allows a bare string, and
/// `toolResults.ts` documents that it treats the payload as one blob.
fn tool_result_text(content: Option<&Value>) -> String {
    let Some(blocks) = content.and_then(Value::as_array) else {
        return content
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
    };
    blocks
        .iter()
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One notice rendered as an assistant line, for the events that are neither
/// speech nor tool traffic but still change what the session is doing.
fn notice(event: &Value, text: String) -> Value {
    json!({
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [{ "type": "text", "text": text }],
        },
        "timestamp": timestamp_of(event),
    })
}

fn normalize_event(event: &Value) -> Option<Value> {
    let kind = event.get("type").and_then(Value::as_str)?;
    let data = event.get("data").unwrap_or(&Value::Null);
    let timestamp = timestamp_of(event);

    match kind {
        "user/message" => {
            // Not every `user/message` came from a human. dsh injects its
            // runtime-context snapshot as one, tagged
            // `source.kind = "plugin"` (`@deepseek-ai/dsh-system-prompt`) —
            // a wall of environment text that would read as if the user had
            // typed it. Captured live: seq 7 `kind:"user"` is the prompt, seq 8
            // `kind:"plugin"` is the snapshot.
            //
            // Only the injection is dropped, rather than keeping only
            // `kind:"user"`: an unrecognised kind from a later dsh release is
            // better shown as noise than silently swallowed, because the one
            // thing that must never disappear is something a human said.
            if data.get("source").and_then(|s| s.get("kind")).and_then(Value::as_str)
                == Some("plugin")
            {
                return None;
            }
            Some(json!({
                "type": "user",
                "message": {
                    "role": "user",
                    // dsh's user content blocks are already `{type:"text",text}`.
                    "content": data.get("content").cloned().unwrap_or_else(|| json!([])),
                },
                "timestamp": timestamp,
            }))
        }

        "assistant/message" => {
            let message = data.get("message")?;
            let content: Vec<Value> = message
                .get("content")
                .and_then(Value::as_array)
                .map(|blocks| blocks.iter().filter_map(assistant_block).collect())
                .unwrap_or_default();
            let mut out = json!({
                "role": "assistant",
                "content": content,
            });
            if let Some(usage) = usage_of(data) {
                out["usage"] = usage;
            }
            Some(json!({
                "type": "assistant",
                "message": out,
                "timestamp": timestamp,
            }))
        }

        "tool/result" => {
            let message = data.get("message")?;
            let blocks = message.get("content").and_then(Value::as_array)?;
            let results: Vec<Value> = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool-result"))
                .map(|b| {
                    json!({
                        "type": "tool_result",
                        // The renderer pairs results to calls by this id; a
                        // result that loses it renders as an orphan card.
                        "tool_use_id": b.get("toolCallId").cloned().unwrap_or(Value::Null),
                        "content": tool_result_text(b.get("content")),
                        "is_error": b.get("isError").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect();
            if results.is_empty() {
                return None;
            }
            Some(json!({
                "type": "user",
                "message": { "role": "user", "content": results },
                "timestamp": timestamp,
            }))
        }

        // The durable audit pair around a human decision. Worth a line: without
        // it a transcript jumps from a refused tool call to a retry with no
        // explanation of who said no.
        "approval/asked" => {
            let tool = data.get("toolName").and_then(Value::as_str).unwrap_or("?");
            let reason = data.get("reason").and_then(Value::as_str).unwrap_or("");
            Some(notice(
                event,
                if reason.is_empty() {
                    format!("⏳ Waiting for approval: {tool}")
                } else {
                    format!("⏳ Waiting for approval: {tool} — {reason}")
                },
            ))
        }
        "approval/decided" => {
            let outcome = data.get("outcome").and_then(Value::as_str).unwrap_or("?");
            Some(notice(event, format!("Approval {outcome}")))
        }

        // Streaming deltas, the duplicate tool/call, and session bookkeeping.
        // See the module header for why each is dropped.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim events captured from `session.history` of a real dsh run whose
    /// tool call was refused by the sandbox and then escalated to an approval.
    fn user_message() -> Value {
        json!({
            "type": "user/message",
            "seq": 7,
            "time": 1786756088873i64,
            "data": {
                "content": [{ "type": "text", "text": "Run this exact shell command" }],
                "source": { "kind": "user", "rpcId": "edae11e6" },
                "role": "user",
                "id": "6a8b430b"
            },
            "surfaceOp": "append"
        })
    }

    fn assistant_with_tool_call() -> Value {
        json!({
            "type": "assistant/message",
            "seq": 32,
            "time": 1786756092485i64,
            "data": {
                "turn": 1,
                "step": 1,
                "message": {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "I'll run that exact command for you." },
                        {
                            "type": "tool-call",
                            "id": "toolu_bdrk_01MUz",
                            "name": "bash",
                            "arguments": "{\"command\":\"touch /tmp/x\",\"description\":\"Run it\"}"
                        }
                    ]
                },
                "usage": {
                    "inputTokens": 3,
                    "outputTokens": 103,
                    "cacheReadTokens": 6756,
                    "cacheWriteTokens": 1789
                }
            }
        })
    }

    fn tool_result() -> Value {
        json!({
            "type": "tool/result",
            "seq": 34,
            "time": 1786756092511i64,
            "data": {
                "message": {
                    "source": { "kind": "tool", "callId": "toolu_bdrk_01MUz" },
                    "content": [{
                        "type": "tool-result",
                        "toolCallId": "toolu_bdrk_01MUz",
                        "content": [{ "type": "text", "text": "[exit code: 1]" }],
                        "isError": false
                    }],
                    "role": "user"
                }
            }
        })
    }

    #[test]
    fn a_user_message_keeps_its_content_blocks() {
        let out = normalize(&[user_message()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["type"], "user");
        assert_eq!(out[0]["message"]["role"], "user");
        assert_eq!(out[0]["message"]["content"][0]["text"], "Run this exact shell command");
    }

    /// dsh stamps epoch millis; the renderer formats an RFC3339 instant.
    #[test]
    fn timestamps_become_rfc3339() {
        let out = normalize(&[user_message()]);
        let ts = out[0]["timestamp"].as_str().expect("a timestamp");
        assert!(ts.starts_with("2026-"), "not an RFC3339 instant: {ts}");
        assert!(chrono::DateTime::parse_from_rfc3339(ts).is_ok(), "{ts}");
    }

    /// The whole point of the module: a `tool-call` block has to become a
    /// `tool_use` with its arguments *parsed*, or the tool card has no input to
    /// show.
    #[test]
    fn a_tool_call_becomes_a_parsed_tool_use() {
        let out = normalize(&[assistant_with_tool_call()]);
        assert_eq!(out.len(), 1);
        let content = &out[0]["message"]["content"];
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(content[1]["id"], "toolu_bdrk_01MUz");
        assert_eq!(content[1]["name"], "bash");
        // Parsed, not the raw JSON string dsh puts on the wire.
        assert_eq!(content[1]["input"]["command"], "touch /tmp/x");
    }

    /// A malformed argument blob is what a reader most needs to see, so it is
    /// preserved rather than dropped on the floor.
    #[test]
    fn unparseable_tool_arguments_survive_as_raw() {
        let mut event = assistant_with_tool_call();
        event["data"]["message"]["content"][1]["arguments"] = json!("{not json");
        let out = normalize(&[event]);
        assert_eq!(out[0]["message"]["content"][1]["input"]["raw"], "{not json");
    }

    #[test]
    fn usage_is_renamed_onto_claudes_field_names() {
        let out = normalize(&[assistant_with_tool_call()]);
        let usage = &out[0]["message"]["usage"];
        assert_eq!(usage["input_tokens"], 3);
        assert_eq!(usage["output_tokens"], 103);
        assert_eq!(usage["cache_read_input_tokens"], 6756);
        assert_eq!(usage["cache_creation_input_tokens"], 1789);
    }

    /// `MessageList.buildResultMap` only looks at `tool_result` blocks inside a
    /// **user** record — a top-level `{"type":"tool_result"}` would be invisible
    /// to it and every tool card would render "no result".
    #[test]
    fn a_tool_result_is_nested_in_a_user_record_and_keyed_by_call_id() {
        let out = normalize(&[tool_result()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["type"], "user");
        let block = &out[0]["message"]["content"][0];
        assert_eq!(block["type"], "tool_result");
        assert_eq!(block["tool_use_id"], "toolu_bdrk_01MUz");
        assert_eq!(block["content"], "[exit code: 1]");
        assert_eq!(block["is_error"], false);
    }

    /// The call and its result must agree on the id, or the pairing breaks.
    #[test]
    fn a_call_and_its_result_share_one_id() {
        let out = normalize(&[assistant_with_tool_call(), tool_result()]);
        let call_id = out[0]["message"]["content"][1]["id"].clone();
        let result_id = out[1]["message"]["content"][0]["tool_use_id"].clone();
        assert_eq!(call_id, result_id);
    }

    /// `tool/call` repeats the call the preceding `assistant/message` already
    /// carried; rendering both would show every tool twice.
    #[test]
    fn the_duplicate_tool_call_event_is_dropped() {
        let dup = json!({
            "type": "tool/call",
            "seq": 33,
            "time": 1786756092486i64,
            "data": { "callId": "toolu_bdrk_01MUz", "name": "bash", "arguments": "{}" }
        });
        assert!(normalize(&[dup]).is_empty());
    }

    /// One captured turn carried 97 chunks against 3 assembled messages.
    #[test]
    fn streaming_chunks_are_dropped() {
        let chunk = json!({
            "type": "assistant/chunk",
            "seq": 13,
            "time": 1786756122957i64,
            "data": { "chunk": { "type": "text-delta", "index": 0, "text": "hi" } }
        });
        assert!(normalize(&[chunk]).is_empty());
    }

    #[test]
    fn session_bookkeeping_is_dropped() {
        for kind in [
            "turn/start",
            "turn/end",
            "step/start",
            "step/end",
            "session/title",
            "request/header",
            "request/context",
            "permission/preset",
            "sandbox/mode",
            "approval/policy",
            "agent/inbox/spliced",
        ] {
            let event = json!({ "type": kind, "seq": 1, "time": 1i64, "data": {} });
            assert!(normalize(&[event]).is_empty(), "{kind} should be dropped");
        }
    }

    /// Without these two lines a transcript jumps from a refused tool call to a
    /// retry with nothing saying who refused it.
    #[test]
    fn the_approval_audit_pair_becomes_readable_lines() {
        let asked = json!({
            "type": "approval/asked",
            "seq": 87,
            "time": 1786756095106i64,
            "data": {
                "id": "38ccec53",
                "toolName": "bash",
                "callId": "toolu_bdrk_01XyY",
                "reason": "escalate sandbox to danger-full-access"
            }
        });
        let decided = json!({
            "type": "approval/decided",
            "seq": 88,
            "time": 1786756095374i64,
            "data": { "id": "38ccec53", "outcome": "rejected" }
        });
        let out = normalize(&[asked, decided]);
        assert_eq!(out.len(), 2);
        let first = out[0]["message"]["content"][0]["text"].as_str().unwrap();
        assert!(first.contains("bash") && first.contains("danger-full-access"), "{first}");
        assert_eq!(out[1]["message"]["content"][0]["text"], "Approval rejected");
    }

    /// A whole captured turn, in order: the renderer sees prose, a tool card
    /// with its result, and nothing else.
    #[test]
    fn a_whole_turn_reduces_to_the_records_the_renderer_reads() {
        let events = vec![
            json!({ "type": "turn/start", "seq": 4, "time": 1i64, "data": {} }),
            user_message(),
            json!({ "type": "assistant/chunk", "seq": 13, "time": 2i64, "data": {} }),
            assistant_with_tool_call(),
            json!({ "type": "tool/call", "seq": 33, "time": 3i64, "data": {} }),
            tool_result(),
            json!({ "type": "turn/end", "seq": 35, "time": 4i64, "data": {} }),
        ];
        let out = normalize(&events);
        let types: Vec<&str> = out.iter().filter_map(|m| m["type"].as_str()).collect();
        assert_eq!(types, vec!["user", "assistant", "user"]);
    }

    /// Verbatim: dsh injects its runtime-context snapshot as a `user/message`
    /// too. Rendering it would put a wall of environment text in the transcript
    /// as if the user had typed it.
    #[test]
    fn the_injected_runtime_context_snapshot_is_not_shown_as_user_speech() {
        let injected = json!({
            "type": "user/message",
            "seq": 8,
            "time": 1786756088900i64,
            "data": {
                "content": [{ "type": "text", "text": "Current runtime context. This snapshot…" }],
                "source": {
                    "kind": "plugin",
                    "plugin": "@deepseek-ai/dsh-system-prompt",
                    "form": "snapshot"
                },
                "role": "user"
            }
        });
        assert!(normalize(&[injected]).is_empty());
        // …while the human's own prompt, one seq earlier, survives.
        assert_eq!(normalize(&[user_message()]).len(), 1);
    }

    /// A kind this release has never seen stays visible: silently swallowing
    /// something a human might have said is the worse failure.
    #[test]
    fn an_unrecognised_source_kind_is_still_shown() {
        let mut event = user_message();
        event["data"]["source"]["kind"] = json!("some-future-kind");
        assert_eq!(normalize(&[event]).len(), 1);
    }

    #[test]
    fn an_event_without_a_type_is_ignored() {
        assert!(normalize(&[json!({ "seq": 1 })]).is_empty());
        assert!(normalize(&[json!("not an object")]).is_empty());
    }
}
