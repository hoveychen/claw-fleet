//! Make a message that was injected into a *running* turn visible in the
//! transcript.
//!
//! Fleet can write a follow-up straight into a live session's socket instead of
//! waiting for the turn to end (see [`crate::live_inject`]). The receiving CLI
//! does record it — but not as a user record. It lands as an `attachment` row:
//!
//! ```text
//! {"type":"attachment","attachment":{"type":"queued_command","prompt":"…",
//!  "origin":{"kind":"peer","from":"unknown"},"isMeta":true}, …}
//! ```
//!
//! Nothing in Fleet used to look at that row, so a message the user typed into
//! the composer of a running session produced *no* visible effect anywhere:
//! the input cleared, no bubble appeared, and no "queued" chip either (chips
//! only exist for the fallback path, and a direct injection never queues).
//!
//! So rewrite the row into the plain user record it should have been. Every
//! client reads its transcript through `get_messages` / `get_messages_tail`, so
//! doing it here reaches the desktop, `fleet serve` and the phone at once.
//!
//! **Only peer-origin prompts.** `queued_command` is also how the CLI records
//! its own internal wake-ups — `<task-notification>` blocks from finished
//! subagents are by far the most common, outnumbering real messages ~30:1 in
//! this machine's transcripts. Those carry no `origin` at all, and rendering
//! them would bury the conversation under tool bookkeeping. The gate is
//! therefore `origin.kind == "peer"`, which is what the socket path hardcodes
//! for every message it delivers.

use serde_json::{json, Value};

/// Rewrite a `queued_command` attachment row into a user record, in place.
///
/// A no-op for every other row, including a `queued_command` that the harness
/// raised for itself rather than one a person or a peer session sent.
pub fn unfold(message: &mut Value) {
    if message.get("type").and_then(Value::as_str) != Some("attachment") {
        return;
    }
    let attachment = &message["attachment"];
    if attachment.get("type").and_then(Value::as_str) != Some("queued_command") {
        return;
    }
    if attachment["origin"].get("kind").and_then(Value::as_str) != Some("peer") {
        return;
    }
    let Some(prompt) = attachment.get("prompt").and_then(Value::as_str) else {
        return;
    };
    // Fleet signs the messages it injects on the user's behalf, to undo the
    // CLI's "not typed by your user" framing (see `live_inject::USER_SIGNATURE`).
    // That line is addressed to the agent, not to the reader — the bubble shows
    // what the user actually typed.
    let prompt = crate::live_inject::strip_user_signature(prompt.trim_end()).to_string();
    if prompt.trim().is_empty() {
        return;
    }

    let Some(obj) = message.as_object_mut() else {
        return;
    };
    obj.remove("attachment");
    obj.insert("type".into(), json!("user"));
    obj.insert(
        "message".into(),
        json!({"role": "user", "content": [{"type": "text", "text": prompt}]}),
    );
    // Not `isMeta`: this is the user speaking, and the frontends fold meta rows
    // into a collapsed "System Context" card instead of drawing a bubble — which
    // is exactly the invisibility this module exists to undo. The flag below is
    // what lets a client mark the bubble as mid-turn delivered.
    obj.remove("isMeta");
    obj.insert("fleetMidTurn".into(), json!(true));
}

/// Surface a message Fleet injected on the user's behalf that the agent has
/// not read yet.
///
/// The CLI only drains its input queue at a tool boundary. While a long Bash
/// call runs (a ten-minute deploy poll, say), the injected message exists in
/// the transcript solely as a `queue-operation` `enqueue` row — the
/// `queued_command` attachment that [`unfold`] turns into a bubble is written
/// only when the message is absorbed. So until then no client could tell a
/// delivered-but-unread message apart from one that went nowhere.
///
/// This replays the queue over `messages` and rewrites every enqueue that is
/// still outstanding at the end — and that carries Fleet's user signature, so
/// harness wake-ups and `fleet send` peers stay hidden — into a user row
/// flagged `fleetPending`. Resolved enqueues are left as they are; the
/// frontends ignore `queue-operation` rows.
///
/// Queue semantics, from this machine's transcripts: `remove` names its entry
/// by `content`; `dequeue` carries no content and pops the oldest entry.
///
/// A window that starts after the enqueue cannot see it, and one that ends
/// before the matching `remove` reports it pending; a later incremental chunk
/// then brings the absorbed `fleetMidTurn` row, and the clients drop a pending
/// row once a real bubble with the same text follows it.
pub fn mark_pending(messages: &mut [Value]) {
    let mut queue: std::collections::VecDeque<(usize, String)> = Default::default();
    for (i, row) in messages.iter().enumerate() {
        match row.get("type").and_then(Value::as_str) {
            Some("queue-operation") => {
                let content = row.get("content").and_then(Value::as_str);
                match (row.get("operation").and_then(Value::as_str), content) {
                    (Some("enqueue"), Some(c)) => queue.push_back((i, c.to_string())),
                    (Some("enqueue"), None) => queue.push_back((i, String::new())),
                    (Some("dequeue"), _) => {
                        queue.pop_front();
                    }
                    (Some(_), Some(c)) => {
                        if let Some(pos) = queue.iter().position(|(_, q)| q == c) {
                            queue.remove(pos);
                        }
                    }
                    _ => {}
                }
            }
            // The absorbed attachment settles its entry even if the window
            // happens to cut off the `remove` that follows it.
            Some("attachment")
                if row["attachment"].get("type").and_then(Value::as_str)
                    == Some("queued_command") =>
            {
                if let Some(p) = row["attachment"].get("prompt").and_then(Value::as_str) {
                    if let Some(pos) = queue.iter().position(|(_, q)| q == p) {
                        queue.remove(pos);
                    }
                }
            }
            _ => {}
        }
    }

    for (i, content) in queue {
        let stripped = crate::live_inject::strip_user_signature(content.trim_end());
        if stripped.len() == content.trim_end().len() || stripped.trim().is_empty() {
            continue;
        }
        let text = stripped.to_string();
        let Some(obj) = messages[i].as_object_mut() else {
            continue;
        };
        let timestamp = obj.get("timestamp").cloned().unwrap_or(Value::Null);
        obj.clear();
        obj.insert("type".into(), json!("user"));
        // Stable across re-reads so a list keyed by uuid does not remount it.
        obj.insert(
            "uuid".into(),
            json!(format!("fleet-pending-{}", timestamp.as_str().unwrap_or(""))),
        );
        obj.insert("timestamp".into(), timestamp);
        obj.insert(
            "message".into(),
            json!({"role": "user", "content": [{"type": "text", "text": text}]}),
        );
        obj.insert("fleetPending".into(), json!(true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(origin: Value, prompt: &str) -> Value {
        json!({
            "type": "attachment",
            "uuid": "u1",
            "parentUuid": "p1",
            "timestamp": "2026-09-20T00:26:55.161Z",
            "attachment": {
                "type": "queued_command",
                "prompt": prompt,
                "origin": origin,
                "isMeta": true,
            },
        })
    }

    #[test]
    fn peer_prompt_becomes_a_plain_user_bubble() {
        let mut msg = row(json!({"kind": "peer", "from": "unknown"}), "make it blue");
        unfold(&mut msg);
        assert_eq!(msg["type"], "user");
        assert_eq!(msg["message"]["role"], "user");
        assert_eq!(msg["message"]["content"][0]["text"], "make it blue");
        assert_eq!(msg["fleetMidTurn"], true);
        // Identity and ordering fields are the row's own and must survive.
        assert_eq!(msg["uuid"], "u1");
        assert_eq!(msg["parentUuid"], "p1");
        assert_eq!(msg["timestamp"], "2026-09-20T00:26:55.161Z");
        // A meta row is folded away by the frontends, so the flag has to go.
        assert!(msg.get("isMeta").is_none());
        assert!(msg.get("attachment").is_none());
    }

    #[test]
    fn task_notifications_are_left_folded() {
        // The harness's own wake-ups carry no origin — they outnumber real
        // messages by a wide margin and are not conversation.
        let mut msg = json!({
            "type": "attachment",
            "attachment": {
                "type": "queued_command",
                "prompt": "<task-notification>\n<task-id>abc</task-id>\n</task-notification>",
            },
        });
        let before = msg.clone();
        unfold(&mut msg);
        assert_eq!(msg, before);
    }

    /// The signature is guidance for the agent; showing it back to the person
    /// who typed the message would be noise they never wrote.
    #[test]
    fn the_senders_signature_is_stripped_from_the_bubble() {
        let signed = crate::live_inject::sign_as_user("合并吧");
        let mut msg = row(json!({"kind": "peer", "from": "unknown"}), &signed);
        unfold(&mut msg);
        assert_eq!(msg["message"]["content"][0]["text"], "合并吧");
    }

    #[test]
    fn other_rows_are_untouched() {
        for mut msg in [
            json!({"type": "user", "message": {"role": "user", "content": "hi"}}),
            json!({"type": "attachment", "attachment": {"type": "total_tokens_reminder"}}),
            json!({"type": "queue-operation", "operation": "enqueue", "content": "hi"}),
        ] {
            let before = msg.clone();
            unfold(&mut msg);
            assert_eq!(msg, before);
        }
    }

    /// End to end through the reader every client uses, on the exact row shape
    /// the CLI wrote for a message typed into the desktop composer of a running
    /// session (captured 2026-09-20 from `e3d2092c`'s transcript).
    #[test]
    fn a_real_transcript_row_reaches_get_messages_as_a_bubble() {
        use crate::agent_source::AgentSource;
        use std::io::Write as _;

        let dir = std::env::temp_dir().join(format!("qc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"user","uuid":"a","message":{{"role":"user","content":"first"}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"parentUuid":"a","isSidechain":false,"attachment":{{"type":"queued_command","prompt":"mid-turn ask","source_uuid":"s","commandMode":"prompt","origin":{{"kind":"peer","from":"unknown"}},"timestamp":"2026-09-20T00:24:32.263Z","isMeta":true}},"type":"attachment","uuid":"b","timestamp":"2026-09-20T00:26:55.161Z"}}"#
        )
        .unwrap();
        drop(f);

        let source = crate::claude_source::ClaudeCodeSource::new();
        let msgs = source.get_messages(path.to_str().unwrap()).unwrap();
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1]["type"], "user");
        assert_eq!(msgs[1]["message"]["content"][0]["text"], "mid-turn ask");
        assert_eq!(msgs[1]["uuid"], "b");
        assert!(msgs[1].get("isMeta").is_none());
    }

    fn enqueue(content: &str, ts: &str) -> Value {
        json!({"type": "queue-operation", "operation": "enqueue", "timestamp": ts, "content": content})
    }

    fn remove(content: &str) -> Value {
        json!({"type": "queue-operation", "operation": "remove", "reason": "absorbed_mid_turn", "content": content})
    }

    /// The 2026-09-24 case: a follow-up injected while a ten-minute Bash poll
    /// ran sat in the queue for 4m43s with nothing on screen to say so.
    #[test]
    fn an_unread_signed_injection_becomes_a_pending_bubble() {
        let signed = crate::live_inject::sign_as_user("这么久的么？");
        let mut msgs = vec![
            json!({"type": "assistant", "uuid": "a"}),
            enqueue(&signed, "2026-09-24T02:54:38.351Z"),
        ];
        mark_pending(&mut msgs);
        assert_eq!(msgs[1]["type"], "user");
        assert_eq!(msgs[1]["fleetPending"], true);
        assert_eq!(msgs[1]["message"]["content"][0]["text"], "这么久的么？");
        assert_eq!(msgs[1]["timestamp"], "2026-09-24T02:54:38.351Z");
        assert_eq!(msgs[1]["uuid"], "fleet-pending-2026-09-24T02:54:38.351Z");
        assert!(msgs[1].get("operation").is_none());
    }

    #[test]
    fn an_absorbed_injection_stays_hidden() {
        let signed = crate::live_inject::sign_as_user("继续");
        let mut msgs = vec![enqueue(&signed, "t"), remove(&signed)];
        let before = msgs.clone();
        mark_pending(&mut msgs);
        assert_eq!(msgs, before);

        // The attachment alone settles it when the window ends before `remove`.
        let mut msgs = vec![enqueue(&signed, "t"), row(json!({"kind": "peer"}), &signed)];
        mark_pending(&mut msgs);
        assert_eq!(msgs[0]["type"], "queue-operation");
    }

    #[test]
    fn unsigned_enqueues_are_never_surfaced() {
        // Task notifications and `fleet send` peers: not something the user typed.
        let mut msgs = vec![enqueue("<task-notification>x</task-notification>", "t")];
        let before = msgs.clone();
        mark_pending(&mut msgs);
        assert_eq!(msgs, before);
    }

    /// `dequeue` carries no content and pops the oldest entry, so the one it
    /// takes is the unsigned one ahead of the user's message.
    #[test]
    fn dequeue_pops_the_oldest_entry() {
        let signed = crate::live_inject::sign_as_user("改成蓝色");
        let mut msgs = vec![
            enqueue("<task-notification>x</task-notification>", "t0"),
            enqueue(&signed, "t1"),
            json!({"type": "queue-operation", "operation": "dequeue"}),
        ];
        mark_pending(&mut msgs);
        assert_eq!(msgs[1]["fleetPending"], true);

        let mut msgs = vec![
            enqueue(&signed, "t1"),
            json!({"type": "queue-operation", "operation": "dequeue"}),
        ];
        mark_pending(&mut msgs);
        assert_eq!(msgs[0]["type"], "queue-operation");
    }

    #[test]
    fn a_blank_prompt_is_not_a_bubble() {
        let mut msg = row(json!({"kind": "peer"}), "   ");
        let before = msg.clone();
        unfold(&mut msg);
        assert_eq!(msg, before);
    }
}
