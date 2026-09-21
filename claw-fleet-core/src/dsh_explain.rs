//! Side questions on a dsh session, answered in a **child session forked by
//! dsh itself** (the dsh backend of [`crate::session_explain`]).
//!
//! dsh has the fork Fleet wants: `session/fork {sessionId}` seeds a new
//! session with the source's events up to its last completed turn, and a
//! prompt to the child replays that prefix — measured 2026-09-20 on
//! 0.1.5-rc.1, the child's first request read 18,560 tokens from cache and
//! missed 4,058. The source log is not opened for writing. Three things the
//! RPC does not do, and how each is handled here:
//!
//! - **The child mounts dsh's *default* model, not the parent's**
//!   (`agentDefaultModel.currentSelection()` in the controller). A different
//!   model is a different cache, so the child is pointed at the parent's route
//!   with `session/selectModel` first. That call also *saves* the selection as
//!   the global default (`saveSelection`), so the default is read back off
//!   `session/modelCatalog` beforehand and restored through the same call once
//!   the turn is over — on every exit path, via [`RestoreDefault`].
//! - **The child persists, and looks like a session.** There is no ephemeral
//!   fork and no delete RPC; the roster item carries `parentSessionId` but no
//!   `origin`, so it would list as a top-level session beside its parent. The
//!   child id is written to [`crate::session_explain::mark_fork_session`] the
//!   moment the fork answers, and `dsh_source::scan_sessions` skips marked ids.
//! - **Nothing caps the turn at one step.** The prompt forbids tools; if the
//!   model calls one anyway the first `tool/call` on the follow stream triggers
//!   `session/cancel`, and the turn is reported as an error rather than an
//!   answer. The plugin-side hard stop (`agent/pre-step` rejecting step ≥ 2)
//!   is a separate task.
//!
//! The child's `session/follow` items are tapped raw off the shared mux socket
//! ([`crate::dsh_events::LiveView::tap`]) and folded by [`DshFollowFold`] —
//! `assistant/chunk` text deltas if the release publishes them,
//! `assistant/message` for the settled text, per-call usage and route, and
//! `turn/end` for the outcome. Spend is priced from those same
//! `assistant/message` events through [`crate::dsh_cost::price_events`], so the
//! fork-inherited prefix (which a history read would also return) is never
//! charged to the question.
//!
//! **No token streaming on 0.1.5-rc.1.** Dumped live 2026-09-20
//! (`FLEET_EXPLAIN_DSH_DUMP`): the follow stream carried turn/start →
//! step/start → request/header → assistant/message → step/end → turn/end and
//! nothing in between; `assistant/chunk` no longer appears anywhere in that
//! release's source. The answer lands whole when `assistant/message` settles,
//! like Codex's. The in-flight partial `session/page`'s tail page carries is
//! the candidate for streaming later.

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};

use crate::agent_source::{ForkAskOutcome, ForkAskSpec};
use crate::dsh_source::{DshSource, RosterSelection, DSH_URI_PREFIX};
use crate::model_cost::TurnUsage;
use crate::session_explain::ANSWER_TIMEOUT;

/// How the child's turn ended, per `turn/end`'s `data.reason.kind`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    /// `session/cancel` cut it short (ours, on a tool call, or someone else's).
    Aborted,
    /// The agent loop failed; the message dsh recorded.
    Error(String),
}

/// Fold of one session's raw `session/follow` items — `{type:"event",
/// event:{type,seq,time,data}}` — into the answer.
#[derive(Default, Debug)]
pub struct DshFollowFold {
    /// Text deltas so far, in order.
    pub streamed: String,
    /// Text blocks of the settled `assistant/message`s, blank-line separated.
    /// Authoritative over `streamed` once present.
    pub message_text: String,
    /// `provider/model` of the last model call.
    pub model: Option<String>,
    /// Usage summed over the turn's model calls; `None` until one settles.
    pub usage: Option<TurnUsage>,
    /// Tools the model called despite being told not to.
    pub tool_names: Vec<String>,
    pub ended: Option<TurnOutcome>,
    /// The turn's `assistant/message` events, verbatim, for pricing.
    pub call_events: Vec<Value>,
}

impl DshFollowFold {
    /// Feed one raw follow item. Returns the text delta it carried, if any.
    pub fn feed(&mut self, item: &Value) -> Option<String> {
        if item.get("type").and_then(Value::as_str) != Some("event") {
            return None;
        }
        let event = item.get("event")?;
        let kind = event.get("type").and_then(Value::as_str)?;
        let data = event.get("data").unwrap_or(&Value::Null);
        match kind {
            "assistant/chunk" => {
                let chunk = data.get("chunk")?;
                if chunk.get("type").and_then(Value::as_str) != Some("text-delta") {
                    return None;
                }
                let text = chunk.get("text").and_then(Value::as_str)?.to_string();
                if text.is_empty() {
                    return None;
                }
                self.streamed.push_str(&text);
                Some(text)
            }
            "tool/call" => {
                let name = data
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("<unnamed>")
                    .to_string();
                self.tool_names.push(name);
                None
            }
            "assistant/message" => {
                let source = data.pointer("/message/source")?;
                if source.get("kind").and_then(Value::as_str) != Some("model") {
                    return None;
                }
                let text = |key: &str| {
                    source
                        .get(key)
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                };
                if let (Some(provider), Some(model)) = (text("provider"), text("model")) {
                    self.model = Some(format!("{provider}/{model}"));
                }
                if let Some(u) = data.get("usage") {
                    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                    let acc = self.usage.get_or_insert_with(TurnUsage::default);
                    acc.input_tokens += n("inputTokens");
                    acc.output_tokens += n("outputTokens");
                    acc.cache_read_tokens += n("cacheReadTokens");
                    acc.cache_creation_tokens += n("cacheWriteTokens");
                }
                let blocks = data
                    .pointer("/message/content")
                    .and_then(Value::as_array)
                    .map(|c| {
                        c.iter()
                            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                            .filter_map(|b| b.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("")
                    })
                    .unwrap_or_default();
                if !blocks.trim().is_empty() {
                    if !self.message_text.is_empty() {
                        self.message_text.push_str("\n\n");
                    }
                    self.message_text.push_str(&blocks);
                }
                self.call_events.push(event.clone());
                None
            }
            "turn/end" => {
                let reason = data.get("reason");
                let reason_kind = reason
                    .and_then(|r| r.get("kind"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                self.ended = Some(match reason_kind {
                    "completed" => TurnOutcome::Completed,
                    "aborted" => TurnOutcome::Aborted,
                    _ => {
                        let message = reason
                            .and_then(|r| r.pointer("/error/message"))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .or_else(|| reason.map(Value::to_string))
                            .unwrap_or_else(|| "dsh turn ended without a reason".to_string());
                        TurnOutcome::Error(message)
                    }
                });
                None
            }
            _ => None,
        }
    }

    /// Settle into the answer. `model` fills in when no call named one
    /// (the parent's route, which is what the child was pointed at).
    pub fn into_outcome(
        self,
        fork_id: &str,
        model: Option<String>,
        cost_usd: Option<f64>,
    ) -> Result<ForkAskOutcome, String> {
        if let Some(TurnOutcome::Error(m)) = &self.ended {
            return Err(m.clone());
        }
        if !self.tool_names.is_empty() {
            return Err(format!(
                "the fork tried to call {} and was cancelled; try again",
                self.tool_names.join(", ")
            ));
        }
        let text = if self.message_text.trim().is_empty() {
            self.streamed
        } else {
            self.message_text
        };
        if text.trim().is_empty() {
            return Err(match self.ended {
                Some(TurnOutcome::Aborted) => {
                    "the fork was cancelled before it answered".to_string()
                }
                _ => "the fork produced no text".to_string(),
            });
        }
        Ok(ForkAskOutcome {
            text,
            model: self.model.or(model),
            usage: self.usage,
            cost_usd,
            fork_session_id: Some(fork_id.to_string()),
        })
    }
}

/// Sum a ledger into one figure: `Some` only when every call was priced, so an
/// unpriced OpenRouter receipt shows as "unknown" rather than as a smaller
/// number.
pub fn total_priced(calls: &[crate::dsh_cost::PricedCall]) -> Option<f64> {
    if calls.is_empty() {
        return None;
    }
    calls.iter().map(|c| c.usd).sum()
}

/// Puts dsh's default model back the way it was found, whichever way the
/// fork ended. `session/selectModel` is the only writer of that default, so
/// the restore goes through the (now idle, hidden) child.
struct RestoreDefault {
    child: String,
    default: RosterSelection,
}

impl Drop for RestoreDefault {
    fn drop(&mut self) {
        let Some(route) = self.default.route.as_deref() else {
            return;
        };
        let result = DshSource::new().with_client(|client| {
            DshSource::select_model(
                client,
                &self.child,
                Some(route),
                self.default.effort.as_deref(),
            )
        });
        if let Err(e) = result {
            crate::log_debug(&format!(
                "[dsh_explain] could not restore the default model {route}: {e}"
            ));
        }
    }
}

fn bare_session_id(id: &str) -> &str {
    id.strip_prefix(DSH_URI_PREFIX).unwrap_or(id)
}

/// Fork a dsh session for one answer; see the module docs for the route. The
/// child's follow stream is tapped before the prompt goes in, so a fast turn
/// cannot finish unobserved; the tap, the default-model restore and the
/// answer deadline ([`ANSWER_TIMEOUT`], enforced with `session/cancel`) are
/// released on every exit path.
pub(crate) fn dsh_fork_ask(
    source: &DshSource,
    spec: &ForkAskSpec,
    on_delta: &mut dyn FnMut(&str),
) -> Result<ForkAskOutcome, String> {
    let parent = bare_session_id(&spec.session_id);
    if parent.is_empty() {
        return Err("dsh fork: session id is required".to_string());
    }
    let parent_selection = source.session_selection(parent);

    let forked = source.with_client(|client| {
        client
            .call(
                "session/fork",
                json!({ "request": { "sessionId": parent } }),
            )
            .map_err(String::from)
    })?;
    let child = forked
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "session/fork answered without a sessionId".to_string())?
        .to_string();
    // Marked before anything else can happen to it, so no scan lists it.
    crate::session_explain::mark_fork_session(&child);

    let (tx, rx) = mpsc::channel::<Value>();
    // `FLEET_EXPLAIN_DSH_DUMP=<file>` appends every raw follow item, one JSON
    // line each — the way to see what a dsh release actually streams when the
    // fold stops matching it.
    let dump = std::env::var_os("FLEET_EXPLAIN_DSH_DUMP").map(std::path::PathBuf::from);
    let _tap = source.tap(
        &child,
        Arc::new(move |item: &Value| {
            if let Some(path) = &dump {
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    use std::io::Write;
                    let _ = writeln!(f, "{item}");
                }
            }
            let _ = tx.send(item.clone());
        }),
    )?;

    // Point the child at the parent's route, remembering what the default was
    // so it can be put back (selectModel saves it as the new default).
    let _restore = source.with_client(|client| {
        let default = DshSource::default_selection(client).unwrap_or_default();
        let Some(route) = parent_selection.route.as_deref() else {
            crate::log_debug(&format!(
                "[dsh_explain] parent {parent} names no model; the fork runs on dsh's default {:?}",
                default.route
            ));
            return Ok(None);
        };
        let already =
            default.route.as_deref() == Some(route) && default.effort == parent_selection.effort;
        if already {
            return Ok(None);
        }
        DshSource::select_model(
            client,
            &child,
            Some(route),
            parent_selection.effort.as_deref(),
        )?;
        Ok(Some(RestoreDefault {
            child: child.clone(),
            default,
        }))
    })?;

    let started = Instant::now();
    source.with_client(|client| DshSource::prompt(client, &child, &spec.prompt))?;

    let mut fold = DshFollowFold::default();
    let mut cancelled = false;
    let mut timed_out = false;
    let deadline = started + ANSWER_TIMEOUT;
    let cancel = |source: &DshSource| {
        let _ = source.with_client(|client| {
            client
                .call(
                    "session/cancel",
                    json!({ "request": { "sessionId": child } }),
                )
                .map(|_| ())
                .map_err(String::from)
        });
    };
    loop {
        let now = Instant::now();
        if now >= deadline {
            timed_out = true;
            if !cancelled {
                cancel(source);
            }
            break;
        }
        match rx.recv_timeout(deadline - now) {
            Ok(item) => {
                if let Some(delta) = fold.feed(&item) {
                    on_delta(&delta);
                }
                if !fold.tool_names.is_empty() && !cancelled {
                    cancelled = true;
                    cancel(source);
                }
                if fold.ended.is_some() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            // The watcher was torn down (server restart): nothing more will
            // arrive on this tap.
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    if fold.ended.is_none()
        && fold.streamed.trim().is_empty()
        && fold.message_text.trim().is_empty()
    {
        return Err(if timed_out {
            format!("dsh fork timed out after {}s", ANSWER_TIMEOUT.as_secs())
        } else {
            "dsh fork: the follow stream closed before the turn ended".to_string()
        });
    }

    let priced =
        crate::dsh_cost::price_events(&format!("{DSH_URI_PREFIX}{child}"), &fold.call_events);
    let cost_usd = total_priced(&priced);
    crate::log_debug(&format!(
        "[dsh_explain] parent={parent} child={child} model={:?} usage={:?} cost={cost_usd:?} ended={:?} in {}ms",
        fold.model.as_deref().or(parent_selection.route.as_deref()),
        fold.usage,
        fold.ended,
        started.elapsed().as_millis()
    ));
    fold.into_outcome(&child, parent_selection.route.clone(), cost_usd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str, seq: u64, data: Value) -> Value {
        json!({
            "type": "event",
            "event": { "type": kind, "seq": seq, "time": 1_758_000_000_000u64, "data": data }
        })
    }

    fn model_message(text: &str, usage: Value) -> Value {
        event(
            "assistant/message",
            40,
            json!({
                "turn": 3, "step": 1,
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": text }],
                    "source": { "kind": "model", "provider": "deepseek-official", "model": "deepseek-flash" }
                },
                "usage": usage,
                "stream": []
            }),
        )
    }

    /// Shapes follow dsh 0.1.5-rc.1's follow stream: `assistant/chunk` carries
    /// `data.chunk = {type:"text-delta", index, text}`, `assistant/message`
    /// the settled blocks with `source` + `usage`, `turn/end` a `reason.kind`.
    #[test]
    fn fold_streams_deltas_then_settles_on_the_message() {
        let mut fold = DshFollowFold::default();
        assert!(fold
            .feed(&json!({ "type": "snapshot", "cursor": 37, "records": [] }))
            .is_none());
        assert!(fold
            .feed(&event("turn/start", 38, json!({ "turn": 3 })))
            .is_none());
        assert_eq!(
            fold.feed(&event(
                "assistant/chunk",
                39,
                json!({ "turn": 3, "step": 1, "chunk": { "type": "text-delta", "index": 0, "text": "因为" } })
            )),
            Some("因为".to_string())
        );
        assert!(fold
            .feed(&event(
                "assistant/chunk",
                39,
                json!({ "chunk": { "type": "reasoning-delta", "index": 0, "text": "hmm" } })
            ))
            .is_none());
        assert_eq!(
            fold.feed(&event(
                "assistant/chunk",
                39,
                json!({ "chunk": { "type": "text-delta", "index": 0, "text": "缓存。" } })
            )),
            Some("缓存。".to_string())
        );
        assert!(fold
            .feed(&model_message(
                "因为缓存。",
                json!({ "inputTokens": 4058, "cacheReadTokens": 18560, "cacheWriteTokens": 0, "outputTokens": 57, "totalTokens": 22675 })
            ))
            .is_none());
        assert!(fold
            .feed(&event(
                "turn/end",
                41,
                json!({ "turn": 3, "reason": { "kind": "completed" } })
            ))
            .is_none());
        assert_eq!(fold.ended, Some(TurnOutcome::Completed));
        assert_eq!(fold.streamed, "因为缓存。");
        assert_eq!(fold.call_events.len(), 1);
        let out = fold
            .into_outcome("session-child", None, Some(0.001))
            .unwrap();
        assert_eq!(out.text, "因为缓存。");
        assert_eq!(
            out.model.as_deref(),
            Some("deepseek-official/deepseek-flash")
        );
        let u = out.usage.unwrap();
        assert_eq!(u.input_tokens, 4058);
        assert_eq!(u.cache_read_tokens, 18560);
        assert_eq!(u.output_tokens, 57);
        assert_eq!(out.cost_usd, Some(0.001));
        assert_eq!(out.fork_session_id.as_deref(), Some("session-child"));
    }

    #[test]
    fn fold_prefers_the_settled_message_over_the_deltas() {
        let mut fold = DshFollowFold::default();
        fold.feed(&event(
            "assistant/chunk",
            1,
            json!({ "chunk": { "type": "text-delta", "index": 0, "text": "partial" } }),
        ));
        fold.feed(&model_message("partial, then whole.", json!({})));
        fold.feed(&event(
            "turn/end",
            2,
            json!({ "reason": { "kind": "completed" } }),
        ));
        let out = fold.into_outcome("c", None, None).unwrap();
        assert_eq!(out.text, "partial, then whole.");
    }

    #[test]
    fn fold_falls_back_to_the_parent_route_when_no_call_named_one() {
        let mut fold = DshFollowFold::default();
        fold.feed(&event(
            "assistant/chunk",
            1,
            json!({ "chunk": { "type": "text-delta", "index": 0, "text": "ok" } }),
        ));
        fold.feed(&event(
            "turn/end",
            2,
            json!({ "reason": { "kind": "completed" } }),
        ));
        let out = fold
            .into_outcome("c", Some("openrouter/x/y".into()), None)
            .unwrap();
        assert_eq!(out.text, "ok");
        assert_eq!(out.model.as_deref(), Some("openrouter/x/y"));
        assert!(out.usage.is_none());
    }

    #[test]
    fn fold_reports_tool_calls_errors_and_empty_turns() {
        let mut fold = DshFollowFold::default();
        fold.feed(&event(
            "tool/call",
            5,
            json!({ "turn": 1, "step": 1, "callId": "call_1", "name": "bash" }),
        ));
        fold.feed(&event(
            "turn/end",
            6,
            json!({ "reason": { "kind": "aborted", "reason": { "kind": "user" } } }),
        ));
        assert_eq!(fold.ended, Some(TurnOutcome::Aborted));
        let err = fold.into_outcome("c", None, None).unwrap_err();
        assert!(err.contains("bash"), "{err}");

        let mut fold = DshFollowFold::default();
        fold.feed(&event(
            "turn/end",
            6,
            json!({ "reason": { "kind": "error", "error": { "message": "rate limited", "code": "429" } } }),
        ));
        assert_eq!(
            fold.into_outcome("c", None, None).unwrap_err(),
            "rate limited"
        );

        let mut fold = DshFollowFold::default();
        fold.feed(&event(
            "turn/end",
            6,
            json!({ "reason": { "kind": "completed" } }),
        ));
        assert_eq!(
            fold.into_outcome("c", None, None).unwrap_err(),
            "the fork produced no text"
        );

        let mut fold = DshFollowFold::default();
        assert!(fold.feed(&json!("garbage")).is_none());
        assert!(fold.feed(&json!({ "type": "event" })).is_none());
        assert!(fold.ended.is_none());
    }

    #[test]
    fn total_is_unknown_unless_every_call_was_priced() {
        use crate::dsh_cost::{PriceBasis, PricedCall};
        let call = |usd: Option<f64>| PricedCall {
            at_ms: 0,
            provider: "deepseek-official".into(),
            model: "deepseek-flash".into(),
            input_tokens: 1,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 1,
            usd,
            basis: usd.map(|_| PriceBasis::Table),
            peak: None,
        };
        assert_eq!(total_priced(&[]), None);
        assert_eq!(
            total_priced(&[call(Some(0.5)), call(Some(0.25))]),
            Some(0.75)
        );
        assert_eq!(total_priced(&[call(Some(0.5)), call(None)]), None);
    }

    #[test]
    fn session_ids_lose_their_scheme() {
        assert_eq!(bare_session_id("dsh://session-1"), "session-1");
        assert_eq!(bare_session_id("session-1"), "session-1");
    }
}
