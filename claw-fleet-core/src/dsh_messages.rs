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
//! ## What is folded rather than shown
//!
//! A `user/message` whose `source.kind` is anything but `user` is not speech —
//! dsh uses that record type for its agent-instructions baseline, its
//! runtime-context snapshot and its skill catalogue. Those keep their record but
//! gain `isMeta: true`, which the frontend collapses into one expandable card.
//! Fleet's own prepended active-plans `<system-reminder>` is peeled off the
//! human's bubble for the same reason (see [`strip_prepended_reminder`]).
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
        // dsh's reasoning block carries `text`; the renderer keys off Claude's
        // `thinking`. Untranslated it falls through `ContentBlocks`' unknown-block
        // shell and renders as a wrench-icon "REASONING" tool card. An empty
        // payload has no summary to show, which is what `redacted_thinking`
        // means — the same mapping `codex_source` uses for a missing summary.
        "reasoning" => {
            let text = block.get("text").and_then(Value::as_str).unwrap_or("").trim();
            Some(if text.is_empty() {
                json!({ "type": "redacted_thinking", "reason": "summary_unavailable" })
            } else {
                json!({ "type": "thinking", "thinking": text })
            })
        }
        // Anything a later dsh release adds: keep the block rather than silently
        // swallowing the turn's content.
        _ => Some(block.clone()),
    }
}

/// Peel Fleet's own prepended `<system-reminder>` block off a human prompt's
/// content blocks.
///
/// Returns `None` when nothing was prepended, `Some(blocks)` with the peeled
/// content otherwise. The block only ever lands at the very front of the prompt
/// string (`format!("{reminder}\n\n{prompt}")`), so only the first text block is
/// examined; an empty remainder means the turn carried no human speech at all
/// and the caller folds the whole record.
fn strip_prepended_reminder(content: &[Value]) -> Option<Vec<Value>> {
    let (idx, text) = content.iter().enumerate().find_map(|(i, b)| {
        (b.get("type").and_then(Value::as_str) == Some("text"))
            .then(|| b.get("text").and_then(Value::as_str).map(|t| (i, t)))
            .flatten()
    })?;
    let stripped = crate::codex_source::strip_leading_system_reminder(text);
    if stripped.len() == text.len() {
        return None;
    }
    let mut out = content.to_vec();
    if stripped.is_empty() {
        out.remove(idx);
    } else {
        out[idx] = json!({ "type": "text", "text": stripped });
    }
    Some(out)
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
            // Not every `user/message` came from a human. Captured across 70
            // sessions, `source.kind` is one of four values: `user` (the human's
            // prompt), `agent-instructions` (all of `$DSH_HOME/AGENTS.md` plus the
            // workspace memory file — 19,397 characters, already wrapped by dsh in
            // its own `<system-reminder>`), `plugin` (the runtime-context snapshot
            // `@deepseek-ai/dsh-system-prompt` splices in), and `skill-catalog`.
            //
            // Only `user` is speech. The rest are flagged `isMeta`, the same flag
            // Claude Code stamps on harness-injected records and `codex_source`
            // stamps on codex's boilerplate: the frontend folds a run of them into
            // one collapsed, expandable card (`metaGrouping.ts` / `MetaFoldBlock`)
            // instead of a full-width bubble. Folded rather than dropped so the
            // text stays readable on demand, and so an unrecognised kind from a
            // later dsh release degrades into the fold rather than disappearing —
            // the one thing that must never vanish is something a human said.
            let kind = data
                .get("source")
                .and_then(|s| s.get("kind"))
                .and_then(Value::as_str);
            let content = data.get("content").cloned().unwrap_or_else(|| json!([]));
            let mut out = json!({
                "type": "user",
                "message": {
                    "role": "user",
                    // dsh's user content blocks are already `{type:"text",text}`.
                    "content": content,
                },
                "timestamp": timestamp,
            });
            if kind != Some("user") {
                out["isMeta"] = json!(true);
                return Some(out);
            }
            // A human prompt, but Fleet prepended the TASKS.md active-plans block
            // into the prompt string itself (no additional-context channel into
            // dsh; see `dsh_source::maybe_prepend_active_plans`). Left in place the
            // transcript opens with two `<system-reminder>` walls back to back.
            // Peel Fleet's off the bubble; a prompt that was *only* the reminder
            // carries no speech and folds like the injections above.
            if let Some(blocks) = out["message"]["content"]
                .as_array()
                .and_then(|c| strip_prepended_reminder(c))
            {
                if blocks.is_empty() {
                    out["isMeta"] = json!(true);
                } else {
                    out["message"]["content"] = json!(blocks);
                }
            }
            Some(out)
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

    /// Verbatim seq 8 of every captured session: dsh's own agent-instructions
    /// baseline, `$DSH_HOME/AGENTS.md` + the workspace memory file, wrapped by
    /// dsh in one `<system-reminder>`. 19,397 characters in the real capture.
    fn agent_instructions() -> Value {
        json!({
            "type": "user/message",
            "seq": 8,
            "time": 1786756088880i64,
            "data": {
                "content": [{
                    "type": "text",
                    "text": "<system-reminder>\n# Fleet PRD Discipline for dsh\n…\n</system-reminder>",
                }],
                "source": { "kind": "agent-instructions" },
                "role": "user"
            }
        })
    }

    /// Verbatim seq 9: the runtime-context snapshot
    /// `@deepseek-ai/dsh-system-prompt` splices in as a `user/message`.
    fn plugin_snapshot() -> Value {
        json!({
            "type": "user/message",
            "seq": 9,
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
        })
    }

    /// Verbatim seq 86 of a captured chat session: reasoning and prose in one
    /// assembled message. dsh's reasoning block carries `text`, not `thinking`.
    fn assistant_with_reasoning() -> Value {
        json!({
            "type": "assistant/message",
            "seq": 86,
            "time": 1786756092485i64,
            "data": {
                "turn": 1,
                "step": 1,
                "message": {
                    "role": "assistant",
                    "content": [
                        { "type": "reasoning", "text": "The user just said \"hi\"." },
                        { "type": "text", "text": "嗨，老板！" }
                    ]
                }
            }
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

    /// Every assembled reply names the route that produced it
    /// (`data.message.source` = `{kind:"model", provider, model}`, verbatim off
    /// a live `session.history`). `MessageList` renders `message.model` on the
    /// usage line of each assistant row, so dropping it is why a dsh transcript
    /// showed `↑45 ↓1328` with no model while a Claude one names it.
    #[test]
    fn an_assistant_record_names_the_route_that_produced_it() {
        let mut event = assistant_with_tool_call();
        event["data"]["message"]["source"] = json!({
            "kind": "model",
            "provider": "deepseek-official",
            "model": "deepseek-v4-pro"
        });
        let out = normalize(&[event]);
        assert_eq!(
            out[0]["message"]["model"], "deepseek-official/deepseek-v4-pro",
            "the row's model comes from message.model"
        );
    }

    /// A reply with no `source` (an older log, or a future shape) must still
    /// render — the model slot simply stays empty rather than reading `null`.
    #[test]
    fn a_reply_without_a_source_carries_no_model_field() {
        let out = normalize(&[assistant_with_tool_call()]);
        assert!(
            out[0]["message"].get("model").is_none(),
            "absent evidence must not become a rendered `null`"
        );
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
    /// too. Rendering it as a bubble would put a wall of environment text in the
    /// transcript as if the user had typed it — so it is folded, not dropped:
    /// the collapsed card keeps it readable on demand.
    #[test]
    fn the_injected_runtime_context_snapshot_is_folded_not_shown_as_user_speech() {
        let out = normalize(&[plugin_snapshot()]);
        assert_eq!(out.len(), 1, "the snapshot must survive as a folded record");
        assert_eq!(out[0]["type"], "user");
        assert_eq!(
            out[0]["isMeta"],
            json!(true),
            "the runtime snapshot must be flagged isMeta so the frontend folds it",
        );
        // …while the human's own prompt, one seq earlier, stays a real bubble.
        let human = normalize(&[user_message()]);
        assert_eq!(human.len(), 1);
        assert!(
            human[0].get("isMeta").is_none(),
            "a human prompt must NOT be folded",
        );
    }

    /// dsh's own agent-instructions message is the whole of `$DSH_HOME/AGENTS.md`
    /// plus the workspace memory file — 19,397 characters in a captured chat
    /// session — already wrapped by dsh in one `<system-reminder>`. Rendered as a
    /// bubble it buries the conversation.
    #[test]
    fn the_agent_instructions_baseline_is_folded() {
        let out = normalize(&[agent_instructions()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["type"], "user");
        assert_eq!(out[0]["isMeta"], json!(true));
        // Folded, not truncated: expanding the card must still show the text.
        assert!(out[0]["message"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("<system-reminder>"));
    }

    /// A kind this release has never seen stays visible — silently swallowing
    /// something a human might have said is the worse failure — but it is folded
    /// rather than presented as speech, because only `kind:"user"` is speech.
    #[test]
    fn an_unrecognised_source_kind_is_folded_not_dropped() {
        let mut event = user_message();
        event["data"]["source"]["kind"] = json!("some-future-kind");
        let out = normalize(&[event]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["isMeta"], json!(true));
    }

    /// Fleet has no additional-context channel into dsh, so
    /// `dsh_source::maybe_prepend_active_plans` prepends the TASKS.md
    /// active-plans block into the prompt string itself. Left in place the
    /// transcript opens with two `<system-reminder>` walls back to back — Fleet's
    /// inside the human's own bubble, then dsh's agent-instructions record.
    /// Strip Fleet's from the bubble, exactly as `codex_source` already does.
    #[test]
    fn a_prompt_has_fleets_prepended_active_plans_reminder_stripped() {
        let mut event = user_message();
        event["data"]["content"] = json!([{
            "type": "text",
            "text": "<system-reminder>\nThe workspace `TASKS.md` holds 14 active plans.\n\
                     ## Plan: foo\n- [ ] **P1** — bar\n</system-reminder>\n\n\
                     Run this exact shell command",
        }]);
        let out = normalize(&[event]);
        assert_eq!(out.len(), 1);
        assert!(out[0].get("isMeta").is_none(), "still a human bubble");
        assert_eq!(
            out[0]["message"]["content"][0]["text"], "Run this exact shell command",
            "the prepended Fleet reminder must not reach the bubble",
        );
    }

    /// A turn whose prompt is *nothing but* the reminder has no human speech in
    /// it at all, so the whole record folds instead of rendering an empty bubble.
    #[test]
    fn a_prompt_that_is_only_the_reminder_folds_entirely() {
        let mut event = user_message();
        event["data"]["content"] = json!([{
            "type": "text",
            "text": "<system-reminder>\nplans\n</system-reminder>",
        }]);
        let out = normalize(&[event]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["isMeta"], json!(true));
    }

    /// A prompt that merely *mentions* the tag mid-sentence is a genuine
    /// question about it and must survive byte-for-byte.
    #[test]
    fn a_prompt_merely_mentioning_the_tag_is_untouched() {
        let mut event = user_message();
        let text = "why does dsh show the <system-reminder> block in my session?";
        event["data"]["content"] = json!([{ "type": "text", "text": text }]);
        let out = normalize(&[event]);
        assert_eq!(out[0]["message"]["content"][0]["text"], text);
        assert!(out[0].get("isMeta").is_none());
    }

    /// dsh's reasoning block is `{type:"reasoning",text}`; the renderer keys off
    /// Claude's `{type:"thinking",thinking}`. Passed through untranslated it
    /// falls to `ContentBlocks`' unknown-block shell and renders as a wrench-icon
    /// "REASONING" tool card instead of a thinking fold.
    #[test]
    fn a_reasoning_block_becomes_a_thinking_block() {
        let out = normalize(&[assistant_with_reasoning()]);
        assert_eq!(out.len(), 1);
        let content = &out[0]["message"]["content"];
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "The user just said \"hi\".");
        // The prose that followed it in the same message is untouched.
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "嗨，老板！");
    }

    /// An empty reasoning payload has no summary to show; Claude's own
    /// vocabulary for that is `redacted_thinking`, which the renderer already
    /// labels "reasoning summary unavailable".
    #[test]
    fn an_empty_reasoning_block_becomes_redacted_thinking() {
        let mut event = assistant_with_reasoning();
        event["data"]["message"]["content"][0]["text"] = json!("   ");
        let out = normalize(&[event]);
        assert_eq!(out[0]["message"]["content"][0]["type"], "redacted_thinking");
    }

    #[test]
    fn an_event_without_a_type_is_ignored() {
        assert!(normalize(&[json!({ "seq": 1 })]).is_empty());
        assert!(normalize(&[json!("not an object")]).is_empty());
    }
}
