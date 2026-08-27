//! Full-duplex JSON-RPC 2.0 for the Agent Client Protocol.
//!
//! # Why this is not `mcp_server.rs`'s JSON-RPC
//!
//! `mcp_server.rs` already speaks JSON-RPC 2.0 over stdio, and its
//! `JsonRpcRequest` / `JsonRpcResponse` / `JsonRpcError` structs look almost
//! identical to the ones here. They are deliberately *not* shared, because MCP
//! and ACP need different halves of the protocol:
//!
//! - **MCP (as Fleet implements it) is half-duplex.** The client asks, the
//!   server answers, and the server never originates a request. `handle_line`
//!   can therefore be a pure `&str -> Option<String>` function.
//! - **ACP is full-duplex.** The agent receives `session/prompt`, but it also
//!   *originates* `session/update` notifications, plus `session/request_permission`
//!   and `elicitation/create` **requests that block waiting for the client's
//!   response**. That needs an outbound id allocator, a pending-response table,
//!   and a writer that serializes concurrent senders.
//!
//! Retrofitting that onto the MCP server would churn a shipped, stable surface
//! for no gain, so ACP gets its own peer. If you are looking for "the other
//! JSON-RPC in this crate", it is `mcp_server.rs`, and this note is why there
//! are two.
//!
//! # Transport independence
//!
//! Nothing here knows about stdio or WebSocket. [`Peer`] is driven by a
//! line/frame reader on one side and a `Sink` on the other, which is what lets
//! the same protocol implementation serve both the `fleet acp` stdio subcommand
//! and the `/acp` WebSocket endpoint. ACP explicitly blesses this: the v1
//! `transports.mdx` says the protocol "is transport-agnostic and can be
//! implemented over any communication channel that supports bidirectional
//! message exchange", requiring only that the JSON-RPC message format and
//! lifecycle are preserved.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use serde::Serialize;
use serde_json::{json, Value};

// ─────────────────────────── Error codes ────────────────────────────

/// JSON-RPC 2.0 §5.1 reserved codes, plus the one ACP-relevant extension.
pub mod codes {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
    /// Not reserved by JSON-RPC; used when a peer's in-flight request is
    /// cancelled via `$/cancel_request` before it produced a result.
    pub const REQUEST_CANCELLED: i32 = -32800;
}

/// The `$/`-prefixed namespace is the LSP/JSON-RPC convention for
/// implementation-defined methods. ACP defines exactly one protocol method.
pub const METHOD_CANCEL_REQUEST: &str = "$/cancel_request";

/// Keep-alive notification that real ACP clients send on an idle connection.
///
/// `acp-ui` emits one every 25s to keep NAT/proxy mappings warm (60s is the
/// shortest common idle window — nginx's `proxy_read_timeout` default and most
/// home NAT evictions). It is a **notification**, so per JSON-RPC 2.0 §4.1 we
/// MUST NOT reply to it. Recognising it by name keeps it from being answered
/// with `METHOD_NOT_FOUND`, which is what a naive dispatcher would do.
pub const METHOD_PING: &str = "$/ping";

// ─────────────────────────── Wire types ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }
    pub fn method_not_found(method: &str) -> Self {
        Self::new(codes::METHOD_NOT_FOUND, format!("Method not found: {method}"))
    }
    pub fn invalid_params(detail: impl Into<String>) -> Self {
        Self::new(codes::INVALID_PARAMS, detail)
    }
    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new(codes::INTERNAL_ERROR, detail)
    }
}

/// One decoded inbound message.
///
/// JSON-RPC distinguishes these three by which fields are present, not by a
/// tag, so this is a parse result rather than a `#[serde(untagged)]` enum —
/// untagged would silently pick the wrong arm on a malformed frame instead of
/// letting us answer with a precise error.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// Has `id` + `method`: peer wants an answer.
    Request { id: Value, method: String, params: Value },
    /// Has `method`, no `id`: peer wants nothing back. Never answer these.
    Notification { method: String, params: Value },
    /// Has `id` + (`result` | `error`): the answer to something *we* sent.
    Response { id: Value, result: Result<Value, RpcError> },
}

/// Parse one JSON-RPC frame.
///
/// `Err` carries the error we should send back when the frame had an `id` we
/// could recover; a frame too broken to carry an id yields `(None, err)` and
/// the caller answers with a null-id error per JSON-RPC 2.0 §5.
pub fn parse(line: &str) -> Result<Incoming, (Option<Value>, RpcError)> {
    let v: Value = serde_json::from_str(line)
        .map_err(|e| (None, RpcError::new(codes::PARSE_ERROR, e.to_string())))?;
    let obj = match v.as_object() {
        Some(o) => o,
        None => {
            return Err((None, RpcError::new(codes::INVALID_REQUEST, "frame is not an object")))
        }
    };
    let id = obj.get("id").cloned().filter(|i| !i.is_null());

    // A response carries result/error and never a method. Check it first so a
    // peer that (wrongly) includes both can't be mistaken for a request.
    if obj.contains_key("result") || obj.contains_key("error") {
        let id = match id {
            Some(i) => i,
            None => {
                return Err((
                    None,
                    RpcError::new(codes::INVALID_REQUEST, "response without id"),
                ))
            }
        };
        let result = if let Some(err) = obj.get("error") {
            Err(RpcError {
                code: err.get("code").and_then(|c| c.as_i64()).unwrap_or(codes::INTERNAL_ERROR as i64)
                    as i32,
                message: err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error")
                    .to_string(),
                data: err.get("data").cloned(),
            })
        } else {
            Ok(obj.get("result").cloned().unwrap_or(Value::Null))
        };
        return Ok(Incoming::Response { id, result });
    }

    let method = match obj.get("method").and_then(|m| m.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return Err((id, RpcError::new(codes::INVALID_REQUEST, "missing method")));
        }
    };
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    Ok(match id {
        Some(id) => Incoming::Request { id, method, params },
        None => Incoming::Notification { method, params },
    })
}

/// Serialize a successful response.
pub fn response_ok(id: &Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

/// Serialize an error response. `id` is `null` when the frame was too broken to
/// recover one (JSON-RPC 2.0 §5).
pub fn response_err(id: Option<&Value>, err: &RpcError) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id.cloned().unwrap_or(Value::Null),
        "error": err,
    })
    .to_string()
}

/// Serialize an outbound notification.
pub fn notification(method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()
}

/// Serialize an outbound request.
pub fn request(id: &Value, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

// ─────────────────────────── Outbound peer ──────────────────────────

/// Where outbound frames go. Implemented by the stdio writer and the WebSocket
/// connection thread.
///
/// `send` takes `&self` (not `&mut self`) because several threads originate
/// frames concurrently — the prompt-handling thread emits `session/update`
/// notifications while a decision-card thread blocks on
/// `session/request_permission`. Implementations serialize internally.
pub trait Sink: Send + Sync {
    /// Write one complete frame. `false` once the peer is gone.
    fn send(&self, frame: &str) -> bool;
}

/// A slot a caller blocks on until the peer answers.
///
/// `Condvar` rather than a channel because the waiter wants a timeout *and* the
/// same slot may be completed by either the reader thread (real answer) or
/// [`Peer::fail_all`] (connection dropped) — one lock, two producers.
struct Pending {
    lock: Mutex<Option<Result<Value, RpcError>>>,
    cv: Condvar,
}

/// The full-duplex half of the connection: allocates outbound request ids,
/// tracks who is waiting for what, and routes inbound responses back.
///
/// One `Peer` per connection. Clone-free — share it as `Arc<Peer>`.
pub struct Peer {
    sink: Box<dyn Sink>,
    next_id: AtomicI64,
    pending: Mutex<HashMap<String, Arc<Pending>>>,
}

impl Peer {
    pub fn new(sink: Box<dyn Sink>) -> Self {
        Self { sink, next_id: AtomicI64::new(1), pending: Mutex::new(HashMap::new()) }
    }

    /// Fire-and-forget. This is how every `session/update` goes out.
    pub fn notify(&self, method: &str, params: Value) -> bool {
        self.sink.send(&notification(method, params))
    }

    /// Write an already-serialized frame. Used by the connection loop, which
    /// gets its replies back from the dispatcher pre-encoded.
    pub fn send_raw(&self, frame: &str) -> bool {
        self.sink.send(frame)
    }

    /// Answer an inbound request.
    pub fn reply(&self, id: &Value, result: Result<Value, RpcError>) -> bool {
        let frame = match &result {
            Ok(v) => response_ok(id, v.clone()),
            Err(e) => response_err(Some(id), e),
        };
        self.sink.send(&frame)
    }

    /// Send a request and block until the peer answers or `timeout` elapses.
    ///
    /// This is the call that makes decision cards work: `request_permission`
    /// and `elicitation/create` both need the human's answer before the agent
    /// can continue, so the caller genuinely wants to park here.
    pub fn request_blocking(
        &self,
        method: &str,
        params: Value,
        timeout: std::time::Duration,
    ) -> Result<Value, RpcError> {
        let id = Value::from(self.next_id.fetch_add(1, Ordering::Relaxed));
        let key = id.to_string();
        let slot = Arc::new(Pending { lock: Mutex::new(None), cv: Condvar::new() });
        self.pending.lock().unwrap().insert(key.clone(), slot.clone());

        if !self.sink.send(&request(&id, method, params)) {
            self.pending.lock().unwrap().remove(&key);
            return Err(RpcError::internal("peer disconnected"));
        }

        let mut guard = slot.lock.lock().unwrap();
        let deadline = std::time::Instant::now() + timeout;
        while guard.is_none() {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (g, _) = slot.cv.wait_timeout(guard, remaining).unwrap();
            guard = g;
        }
        let answer = guard.take();
        drop(guard);
        // Always unregister: on timeout a late response would otherwise leak
        // the slot for the life of the connection.
        self.pending.lock().unwrap().remove(&key);
        answer.unwrap_or_else(|| {
            Err(RpcError::new(codes::REQUEST_CANCELLED, format!("{method} timed out")))
        })
    }

    /// Route an inbound [`Incoming::Response`] to whoever is blocked on it.
    ///
    /// Returns `false` for an id nobody is waiting on — a late answer after a
    /// timeout, which is normal and must not be treated as a protocol error.
    pub fn resolve(&self, id: &Value, result: Result<Value, RpcError>) -> bool {
        let slot = self.pending.lock().unwrap().remove(&id.to_string());
        match slot {
            Some(slot) => {
                *slot.lock.lock().unwrap() = Some(result);
                slot.cv.notify_all();
                true
            }
            None => false,
        }
    }

    /// Fail every waiter. Call this when the connection drops, so blocked
    /// decision-card threads unwind instead of sitting until their timeout.
    pub fn fail_all(&self, err: RpcError) {
        let slots: Vec<Arc<Pending>> =
            self.pending.lock().unwrap().drain().map(|(_, s)| s).collect();
        for slot in slots {
            *slot.lock.lock().unwrap() = Some(Err(err.clone()));
            slot.cv.notify_all();
        }
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Sink that records frames and can simulate a dead peer.
    struct VecSink {
        frames: Mutex<Vec<String>>,
        alive: bool,
    }
    impl VecSink {
        fn new(alive: bool) -> Self {
            Self { frames: Mutex::new(Vec::new()), alive }
        }
    }
    impl Sink for VecSink {
        fn send(&self, frame: &str) -> bool {
            self.frames.lock().unwrap().push(frame.to_string());
            self.alive
        }
    }

    /// Sink that forwards frames to a channel so a test thread can answer them.
    struct ChanSink(mpsc::Sender<String>);
    impl Sink for ChanSink {
        fn send(&self, frame: &str) -> bool {
            self.0.send(frame.to_string()).is_ok()
        }
    }

    #[test]
    fn parses_the_three_message_shapes() {
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","id":1,"method":"session/prompt","params":{"a":1}}"#).unwrap(),
            Incoming::Request {
                id: json!(1),
                method: "session/prompt".into(),
                params: json!({"a": 1})
            }
        );
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","method":"session/update","params":{}}"#).unwrap(),
            Incoming::Notification { method: "session/update".into(), params: json!({}) }
        );
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#).unwrap(),
            Incoming::Response { id: json!(7), result: Ok(json!({"ok": true})) }
        );
    }

    #[test]
    fn a_null_id_is_a_notification_not_a_request() {
        // JSON-RPC 2.0 §4.1: a notification is a request object *without* an
        // id. Some clients send `"id": null` to mean the same thing; answering
        // it would violate "MUST NOT reply to a Notification".
        assert_eq!(
            parse(r#"{"jsonrpc":"2.0","id":null,"method":"$/ping"}"#).unwrap(),
            Incoming::Notification { method: METHOD_PING.into(), params: Value::Null }
        );
    }

    #[test]
    fn error_responses_round_trip_code_and_message() {
        let msg = parse(
            r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"Method not found: x"}}"#,
        )
        .unwrap();
        match msg {
            Incoming::Response { id, result: Err(e) } => {
                assert_eq!(id, json!(3));
                assert_eq!(e.code, codes::METHOD_NOT_FOUND);
                assert_eq!(e.message, "Method not found: x");
            }
            other => panic!("expected an error response, got {other:?}"),
        }
    }

    #[test]
    fn a_frame_carrying_both_result_and_method_is_read_as_a_response() {
        // Guards the ordering in `parse`: checking `method` first would let a
        // malformed answer be dispatched as an inbound request.
        let msg = parse(r#"{"jsonrpc":"2.0","id":1,"method":"x","result":5}"#).unwrap();
        assert!(matches!(msg, Incoming::Response { .. }));
    }

    #[test]
    fn broken_frames_report_a_recoverable_id_when_there_is_one() {
        let (id, err) = parse("not json").unwrap_err();
        assert!(id.is_none());
        assert_eq!(err.code, codes::PARSE_ERROR);

        let (id, err) = parse(r#"{"jsonrpc":"2.0","id":9}"#).unwrap_err();
        assert_eq!(id, Some(json!(9)), "an id-bearing frame must be answerable");
        assert_eq!(err.code, codes::INVALID_REQUEST);

        let (id, _) = parse("[1,2,3]").unwrap_err();
        assert!(id.is_none());
    }

    #[test]
    fn outbound_request_blocks_until_the_answer_arrives() {
        let (tx, rx) = mpsc::channel();
        let peer = Arc::new(Peer::new(Box::new(ChanSink(tx))));

        let responder = {
            let peer = peer.clone();
            std::thread::spawn(move || {
                let frame = rx.recv().expect("a request frame");
                let sent: Value = serde_json::from_str(&frame).unwrap();
                assert_eq!(sent["method"], "session/request_permission");
                assert_eq!(sent["jsonrpc"], "2.0");
                peer.resolve(&sent["id"], Ok(json!({"outcome": "selected"})));
            })
        };

        let got = peer
            .request_blocking(
                "session/request_permission",
                json!({"sessionId": "s1"}),
                std::time::Duration::from_secs(5),
            )
            .expect("the peer answered");
        assert_eq!(got, json!({"outcome": "selected"}));
        responder.join().unwrap();
        assert_eq!(peer.pending_count(), 0, "the slot must be released");
    }

    #[test]
    fn outbound_request_times_out_without_leaking_its_slot() {
        let peer = Peer::new(Box::new(VecSink::new(true)));
        let err = peer
            .request_blocking("elicitation/create", json!({}), std::time::Duration::from_millis(30))
            .unwrap_err();
        assert_eq!(err.code, codes::REQUEST_CANCELLED);
        assert_eq!(peer.pending_count(), 0, "a timed-out slot must not leak");
    }

    #[test]
    fn a_late_answer_after_timeout_is_ignored_not_an_error() {
        let peer = Peer::new(Box::new(VecSink::new(true)));
        let _ = peer.request_blocking("x", json!({}), std::time::Duration::from_millis(10));
        assert!(!peer.resolve(&json!(1), Ok(json!("late"))));
    }

    #[test]
    fn a_dead_sink_fails_the_request_immediately() {
        let peer = Peer::new(Box::new(VecSink::new(false)));
        let err = peer
            .request_blocking("x", json!({}), std::time::Duration::from_secs(30))
            .unwrap_err();
        assert_eq!(err.code, codes::INTERNAL_ERROR);
        assert_eq!(peer.pending_count(), 0);
    }

    #[test]
    fn fail_all_unblocks_every_waiter_when_the_connection_drops() {
        let (tx, rx) = mpsc::channel();
        let peer = Arc::new(Peer::new(Box::new(ChanSink(tx))));
        let waiters: Vec<_> = (0..3)
            .map(|_| {
                let peer = peer.clone();
                std::thread::spawn(move || {
                    peer.request_blocking("x", json!({}), std::time::Duration::from_secs(30))
                })
            })
            .collect();

        for _ in 0..3 {
            rx.recv().expect("each waiter sends its frame");
        }
        peer.fail_all(RpcError::internal("connection closed"));

        for w in waiters {
            let err = w.join().unwrap().unwrap_err();
            assert_eq!(err.code, codes::INTERNAL_ERROR);
        }
        assert_eq!(peer.pending_count(), 0);
    }

    #[test]
    fn outbound_ids_are_unique_per_peer() {
        let (tx, rx) = mpsc::channel();
        let peer = Arc::new(Peer::new(Box::new(ChanSink(tx))));
        for _ in 0..4 {
            let peer = peer.clone();
            std::thread::spawn(move || {
                let _ = peer.request_blocking("x", json!({}), std::time::Duration::from_millis(80));
            });
        }
        let mut ids = std::collections::HashSet::new();
        for _ in 0..4 {
            let frame = rx.recv().unwrap();
            let v: Value = serde_json::from_str(&frame).unwrap();
            assert!(ids.insert(v["id"].to_string()), "ids must not repeat");
        }
    }

    #[test]
    fn serializers_emit_the_required_envelope() {
        assert_eq!(
            response_ok(&json!(1), json!({"a": 1})),
            r#"{"id":1,"jsonrpc":"2.0","result":{"a":1}}"#
        );
        let err = response_err(None, &RpcError::method_not_found("nope"));
        let v: Value = serde_json::from_str(&err).unwrap();
        assert_eq!(v["id"], Value::Null, "an unrecoverable id serializes as null");
        assert_eq!(v["error"]["code"], codes::METHOD_NOT_FOUND);
        assert!(v["error"].get("data").is_none(), "absent data must not serialize");

        let n: Value = serde_json::from_str(&notification(METHOD_PING, Value::Null)).unwrap();
        assert!(n.get("id").is_none(), "a notification must not carry an id");
    }
}
