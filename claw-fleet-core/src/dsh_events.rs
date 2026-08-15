//! Live observation of a `dsh web` instance over its two downlink WebSockets.
//!
//! `session.list` (polled by [`crate::dsh_source`]) carries one liveness bit per
//! session — `running` — which collapses every phase of a turn into "Active".
//! The fine phases Fleet shows for the other two sources (Thinking / Streaming /
//! Executing / Processing) exist in dsh only as events, and events are only
//! published on the sockets:
//!
//! * `ws://127.0.0.1:<port>/api/events.mux` — every session's turn lifecycle
//!   (`turn/start`, `step/start`, `assistant/chunk`, `tool/call`, `tool/result`,
//!   `step/end`, `turn/end`) plus projection and queue updates.
//! * `ws://127.0.0.1:<port>/api/events.host` — host-wide facts, of which Fleet
//!   uses `host/session-status` (the `running` bit, pushed instead of polled).
//!
//! Both are **downlink-only**: the client opens them without parameters and
//! sends nothing. Frames are not bare events — each one is a `server-request`
//! envelope whose `method` names the frame and whose `payload` carries it:
//!
//! ```text
//! {"type":"server-request","rpcId":"<uuid>","method":"session/event",
//!  "payload":{"type":"session/event","sessionId":"session-…",
//!             "event":{"type":"tool/call","seq":62,"time":…,"data":{…}}}}
//! ```
//!
//! This module keeps the derived per-session phase in memory and hands it to
//! `scan_sessions`, which overlays it on the polled list. The poll still decides
//! *which* sessions exist and what their token totals are; the socket only
//! sharpens their status.
//!
//! ## Both sockets are scoped to their own server process
//!
//! Measured against two concurrent `dsh web` instances sharing one `~/.dsh`
//! home: while instance A ran a full turn, instance B's `events.mux` and
//! `events.host` published **nothing** about it, and B's `session.list` reported
//! `running: false` for that session throughout — A's reported `true`. Sessions
//! are shared through the on-disk log; the *live* view is not.
//!
//! So this watcher observes turns Fleet drives through Fleet's own server, which
//! is what the spawn/resume path will do. A session someone runs in their own
//! `dsh` TUI still appears in the list with its history and token totals, but it
//! has no live phase for Fleet to show — and no `running` bit either, so that
//! limit predates this module rather than being introduced by it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use serde_json::Value;

use crate::session::SessionStatus;

/// How long a pushed phase stays authoritative after the last frame that set it.
///
/// A finished turn leaves `WaitingInput` behind; that is genuine information for
/// a while, but a session nobody has touched in half a minute is Idle by Fleet's
/// convention (`Active // file written < 30s ago`), so the overlay expires and
/// the polled status stands again.
const LIVE_STATUS_TTL_MS: u64 = 30_000;

/// Give up on a handshake that never completes, so a half-open socket cannot
/// park the reconnect loop forever (the failure mode `mobile_relay` hit live).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Wait between reconnect attempts. `dsh web` is on loopback, so a failure here
/// means the server is down or restarting, not that the network is congested.
const RECONNECT_BACKOFF: Duration = Duration::from_secs(2);

/// How long a read may block before the loop re-checks the stop flag. Bounds how
/// long `Drop` waits for the thread to notice it is finished.
const READ_TICK: Duration = Duration::from_secs(1);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Frame decoding ──────────────────────────────────────────────────────────

/// One decoded downlink frame, reduced to what Fleet acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DshFrame {
    /// `session/event` — one entry of a session's durable event log.
    ///
    /// `block_type` is only populated for `assistant/chunk`, whose phase depends
    /// on what kind of block is streaming (text vs reasoning vs tool call).
    Event {
        session_id: String,
        kind: String,
        block_type: Option<String>,
        /// `turn/end`'s `data.reason.kind` — observed as `completed` when the
        /// agent finished on its own and `aborted` when `session.cancel` cut it
        /// short. Absent on every other event.
        reason_kind: Option<String>,
    },
    /// `host/session-status` — the coarse running bit, pushed.
    Status { session_id: String, running: bool },
    /// Everything Fleet does not act on: projections (already carried by
    /// `session.list`), queue snapshots, `session/subscribed`, host commands.
    Ignored,
}

/// Which block an `assistant/chunk` belongs to, across the chunk shapes dsh
/// emits. `block-start` names it outright; the deltas only imply it.
fn chunk_block_type(chunk: &Value) -> Option<String> {
    if let Some(bt) = chunk.get("blockType").and_then(Value::as_str) {
        return Some(bt.to_string());
    }
    if let Some(bt) = chunk
        .get("block")
        .and_then(|b| b.get("type"))
        .and_then(Value::as_str)
    {
        return Some(bt.to_string());
    }
    match chunk.get("type").and_then(Value::as_str) {
        Some("text-delta") => Some("text".into()),
        Some("tool-call-delta") => Some("tool-call".into()),
        Some("reasoning-delta") | Some("thinking-delta") => Some("reasoning".into()),
        // `usage` and `finish` are bookkeeping chunks with no block of their own.
        _ => None,
    }
}

/// Decode one text frame off either socket.
///
/// Anything that is not a `server-request` envelope — or is one Fleet has no use
/// for — decodes to [`DshFrame::Ignored`] rather than an error: dsh is a
/// developer preview whose frame set grows between releases, and an unknown
/// frame is not a failure.
pub fn parse_frame(text: &str) -> DshFrame {
    let Ok(parsed) = serde_json::from_str::<Value>(text) else {
        return DshFrame::Ignored;
    };
    if parsed.get("type").and_then(Value::as_str) != Some("server-request") {
        return DshFrame::Ignored;
    }
    let method = parsed.get("method").and_then(Value::as_str).unwrap_or("");
    let Some(payload) = parsed.get("payload") else {
        return DshFrame::Ignored;
    };
    let session_id = payload
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if session_id.is_empty() {
        return DshFrame::Ignored;
    }

    match method {
        "session/event" => {
            let event = payload.get("event");
            let kind = event
                .and_then(|e| e.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if kind.is_empty() {
                return DshFrame::Ignored;
            }
            let data = event.and_then(|e| e.get("data"));
            let block_type = data.and_then(|d| d.get("chunk")).and_then(chunk_block_type);
            let reason_kind = data
                .and_then(|d| d.get("reason"))
                .and_then(|r| r.get("kind"))
                .and_then(Value::as_str)
                .map(str::to_string);
            DshFrame::Event {
                session_id,
                kind,
                block_type,
                reason_kind,
            }
        }
        "host/session-status" => {
            let running = payload
                .get("running")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            DshFrame::Status {
                session_id,
                running,
            }
        }
        _ => DshFrame::Ignored,
    }
}

// ── Phase derivation ────────────────────────────────────────────────────────

/// Map one event onto Fleet's phase vocabulary, or `None` to keep the current
/// phase (bookkeeping events that say nothing about what the agent is doing).
///
/// The mapping follows [`SessionStatus`]'s own definitions rather than the event
/// names: `Executing` means a tool is the thing in flight, `Processing` means
/// Fleet is waiting on the model with nothing streaming yet, `Streaming` means
/// visible text is arriving.
fn phase_of(kind: &str, block_type: Option<&str>) -> Option<SessionStatus> {
    match kind {
        // A turn or step has been admitted but nothing is streaming yet, and a
        // finished tool hands control back to the model the same way.
        "turn/start" | "step/start" | "step/end" | "tool/result" => Some(SessionStatus::Processing),
        // The tool itself is now running.
        "tool/call" => Some(SessionStatus::Executing),
        "turn/end" => Some(SessionStatus::WaitingInput),
        "assistant/chunk" => match block_type {
            Some("reasoning") | Some("thinking") => Some(SessionStatus::Thinking),
            Some("text") => Some(SessionStatus::Streaming),
            Some("tool-call") => Some(SessionStatus::Executing),
            _ => None,
        },
        _ => None,
    }
}

/// What the sockets currently know about one session.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveSession {
    /// Last pushed `host/session-status`.
    pub running: bool,
    /// Phase derived from the most recent event that carried one.
    pub phase: SessionStatus,
    /// When that phase was set — drives [`LIVE_STATUS_TTL_MS`].
    pub phase_at_ms: u64,
}

impl LiveSession {
    /// The phase to overlay on the polled session, or `None` to leave the polled
    /// status alone (nothing pushed yet, or the last push has gone stale).
    pub fn effective_phase(&self, now_ms: u64) -> Option<SessionStatus> {
        if self.phase_at_ms == 0 {
            return None;
        }
        if self.running || now_ms.saturating_sub(self.phase_at_ms) <= LIVE_STATUS_TTL_MS {
            return Some(self.phase.clone());
        }
        None
    }
}

/// Called once when a session's turn ends; `true` when it ran to completion.
pub type TurnEndCallback = Box<dyn FnOnce(bool) + Send>;

/// The per-session live view both sockets write into.
///
/// Two maps, two locks: `sessions` is read on every scan tick, while `waiters`
/// is touched only when a Fleet-driven turn starts and when it ends. Sharing one
/// lock would make every scan contend with callbacks that may run arbitrary
/// caller code.
#[derive(Default)]
pub struct LiveView {
    sessions: Mutex<HashMap<String, LiveSession>>,
    waiters: Mutex<HashMap<String, Vec<TurnEndCallback>>>,
}

/// Handle shared between the socket follower and its owner.
pub type SharedLive = Arc<LiveView>;

impl LiveView {
    /// Fold one frame in. Split out from the socket loop so the state machine is
    /// testable without a server.
    pub fn apply(&self, frame: DshFrame, now_ms: u64) {
        match frame {
            DshFrame::Event {
                session_id,
                kind,
                block_type,
                reason_kind,
            } => {
                if let Some(phase) = phase_of(&kind, block_type.as_deref()) {
                    let mut guard = self
                        .sessions
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let entry = guard.entry(session_id.clone()).or_default();
                    entry.phase = phase;
                    entry.phase_at_ms = now_ms;
                }
                if kind == "turn/end" {
                    // `aborted` (session.cancel) is the one other kind observed
                    // live; treat anything that is not an outright completion as
                    // a failed turn so the caller does not record it as success.
                    self.settle(&session_id, reason_kind.as_deref() == Some("completed"));
                }
            }
            DshFrame::Status {
                session_id,
                running,
            } => {
                let mut guard = self
                    .sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.entry(session_id).or_default().running = running;
            }
            DshFrame::Ignored => {}
        }
    }

    /// Run (and forget) every callback waiting on this session's turn.
    ///
    /// Callbacks run outside the lock: they are caller-supplied and may take
    /// their own locks, which would otherwise be a deadlock waiting to happen.
    fn settle(&self, session_id: &str, success: bool) {
        let waiting = {
            let mut guard = self
                .waiters
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.remove(session_id).unwrap_or_default()
        };
        for cb in waiting {
            cb(success);
        }
    }

    /// Call `cb` when this session's next `turn/end` arrives.
    ///
    /// Fleet's spawn/resume contract wants a completion signal, and dsh has no
    /// per-session process whose exit could provide one — the turn runs inside
    /// the shared server. `turn/end` is that signal.
    pub fn on_turn_end(&self, session_id: &str, cb: TurnEndCallback) {
        self.waiters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(session_id.to_string())
            .or_default()
            .push(cb);
    }

    /// The phase to overlay for `session_id`, if a fresh one exists.
    pub fn phase_of(&self, session_id: &str, now_ms: u64) -> Option<SessionStatus> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .and_then(|s| s.effective_phase(now_ms))
    }

    /// How many sessions the sockets have reported on.
    pub fn tracked(&self) -> usize {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Fail every outstanding waiter. Used when the follower is torn down: a
    /// turn whose completion Fleet can no longer observe must not leave the
    /// caller (the auto-resume scheduler) holding its slot forever.
    fn abandon_all(&self) {
        let waiting: Vec<TurnEndCallback> = {
            let mut guard = self
                .waiters
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.drain().flat_map(|(_, cbs)| cbs).collect()
        };
        for cb in waiting {
            cb(false);
        }
    }
}

// ── The watcher ─────────────────────────────────────────────────────────────

/// A background follower of one `dsh web` instance's two downlinks.
///
/// Bound to a single port: a restarted server lands on a fresh OS-assigned port,
/// so the owner drops this watcher and starts another rather than reusing it.
pub struct DshEventWatcher {
    port: u16,
    live: SharedLive,
    stop: Arc<AtomicBool>,
}

impl DshEventWatcher {
    /// Open both downlinks in a background thread and start folding frames.
    ///
    /// Returns immediately; the sockets connect (and reconnect) on their own, so
    /// a server that is not answering yet costs nothing but a retry.
    pub fn start(port: u16) -> Self {
        let live: SharedLive = Arc::new(LiveView::default());
        let stop = Arc::new(AtomicBool::new(false));

        let thread_states = live.clone();
        let thread_stop = stop.clone();
        let spawned = std::thread::Builder::new()
            .name("dsh-events".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        crate::log_debug(&format!("dsh events: no runtime: {e}"));
                        return;
                    }
                };
                rt.block_on(async {
                    let mux = follow(
                        format!("ws://127.0.0.1:{port}/api/events.mux"),
                        thread_states.clone(),
                        thread_stop.clone(),
                    );
                    let host = follow(
                        format!("ws://127.0.0.1:{port}/api/events.host"),
                        thread_states.clone(),
                        thread_stop.clone(),
                    );
                    tokio::join!(mux, host);
                });
            });
        if let Err(e) = spawned {
            crate::log_debug(&format!("dsh events: cannot spawn follower: {e}"));
            stop.store(true, Ordering::SeqCst);
        }

        Self { port, live, stop }
    }

    /// The port this watcher follows.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The phase to show for `session_id`, or `None` when the sockets have
    /// nothing fresher than the poll.
    pub fn phase_of(&self, session_id: &str) -> Option<SessionStatus> {
        self.live.phase_of(session_id, now_ms())
    }

    /// Call `cb` when this session's next turn ends. See [`LiveView::on_turn_end`].
    pub fn on_turn_end(&self, session_id: &str, cb: TurnEndCallback) {
        self.live.on_turn_end(session_id, cb);
    }

    /// How many sessions the sockets have reported on. Diagnostics and tests.
    pub fn tracked(&self) -> usize {
        self.live.tracked()
    }
}

impl Drop for DshEventWatcher {
    fn drop(&mut self) {
        // The follower notices within one `READ_TICK` and closes both sockets.
        // Not joined: the owner drops this on a rescan path that must not block
        // on a socket read, and a detached thread with a stop flag set exits on
        // its own within the tick.
        self.stop.store(true, Ordering::SeqCst);
        // Nothing will observe those turns any more, so no caller may be left
        // waiting on a completion that can never arrive.
        self.live.abandon_all();
    }
}

type DshWs = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// Keep one downlink connected for as long as the watcher lives.
async fn follow(url: String, states: SharedLive, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        match connect(&url).await {
            Ok(ws) => pump(ws, &states, &stop).await,
            Err(e) => crate::log_debug(&format!("dsh events: {e}")),
        }
        sleep_interruptible(RECONNECT_BACKOFF, &stop).await;
    }
}

async fn connect(url: &str) -> Result<DshWs, String> {
    match tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(url)).await {
        Ok(Ok((ws, _))) => Ok(ws),
        Ok(Err(e)) => Err(format!("connect {url}: {e}")),
        Err(_) => Err(format!(
            "connect {url}: timed out after {}s",
            CONNECT_TIMEOUT.as_secs()
        )),
    }
}

/// Read frames until the socket closes or the watcher is dropped.
async fn pump(mut ws: DshWs, states: &SharedLive, stop: &Arc<AtomicBool>) {
    use tokio_tungstenite::tungstenite::Message;
    while !stop.load(Ordering::SeqCst) {
        // The timeout is the only reason this loop re-checks `stop` on an idle
        // socket: dsh publishes nothing between turns, so a bare `next()` would
        // park here until the next session ran.
        match tokio::time::timeout(READ_TICK, ws.next()).await {
            Err(_) => continue,
            Ok(None) => break,
            Ok(Some(Err(e))) => {
                crate::log_debug(&format!("dsh events: read: {e}"));
                break;
            }
            Ok(Some(Ok(Message::Text(text)))) => {
                states.apply(parse_frame(&text), now_ms());
            }
            Ok(Some(Ok(Message::Close(_)))) => break,
            // Ping/Pong are answered by the stream itself; binary frames are not
            // part of this protocol.
            Ok(Some(Ok(_))) => {}
        }
    }
}

/// Sleep in `READ_TICK` slices so a dropped watcher does not wait out a full
/// backoff before its thread exits.
async fn sleep_interruptible(total: Duration, stop: &Arc<AtomicBool>) {
    let mut left = total;
    while left > Duration::ZERO && !stop.load(Ordering::SeqCst) {
        let slice = left.min(READ_TICK);
        tokio::time::sleep(slice).await;
        left -= slice;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Verbatim `tool/call` frame captured off `events.mux`.
    fn tool_call_frame() -> String {
        json!({
            "type": "server-request",
            "rpcId": "e128abf8-ed58-4710-912c-46689acc01d9",
            "method": "session/event",
            "payload": {
                "type": "session/event",
                "sessionId": "session-ef76dbb8",
                "event": {
                    "type": "tool/call",
                    "seq": 62,
                    "time": 1786753415952u64,
                    "data": { "turn": 2, "step": 1, "callId": "toolu_x", "name": "bash" }
                },
                "view": { "for": "call" }
            }
        })
        .to_string()
    }

    #[test]
    fn decodes_a_live_tool_call_frame() {
        assert_eq!(
            parse_frame(&tool_call_frame()),
            DshFrame::Event {
                session_id: "session-ef76dbb8".into(),
                kind: "tool/call".into(),
                block_type: None,
                reason_kind: None,
            }
        );
    }

    /// Verbatim `turn/end` frames: one that finished, one cut short by
    /// `session.cancel`. The nested `reason.kind` is the only thing telling the
    /// two apart, and it decides what `on_turn_end` reports.
    #[test]
    fn decodes_the_outcome_of_a_finished_turn() {
        let end = |reason: Value| {
            json!({
                "type": "server-request",
                "rpcId": "x",
                "method": "session/event",
                "payload": {
                    "sessionId": "session-a",
                    "event": { "type": "turn/end", "seq": 85, "data": { "turn": 2, "reason": reason } }
                }
            })
            .to_string()
        };
        let completed = parse_frame(&end(json!({ "kind": "completed" })));
        let aborted = parse_frame(&end(json!({ "kind": "aborted", "reason": { "kind": "user" } })));
        assert!(matches!(
            completed,
            DshFrame::Event { ref reason_kind, .. } if reason_kind.as_deref() == Some("completed")
        ));
        assert!(matches!(
            aborted,
            DshFrame::Event { ref reason_kind, .. } if reason_kind.as_deref() == Some("aborted")
        ));
    }

    /// The frame type lives on the envelope's `method`, not on the payload — a
    /// reader that matched the payload's own `type` would see every session
    /// event as the same thing.
    #[test]
    fn rejects_anything_that_is_not_a_server_request() {
        let unary = json!({
            "type": "server-response",
            "rpcId": "x",
            "result": { "ok": true, "value": {} }
        })
        .to_string();
        assert_eq!(parse_frame(&unary), DshFrame::Ignored);
    }

    #[test]
    fn ignores_frames_fleet_does_not_act_on() {
        for method in ["session/projection", "session/queue", "session/subscribed"] {
            let frame = json!({
                "type": "server-request",
                "rpcId": "x",
                "method": method,
                "payload": { "sessionId": "session-a", "key": "tokenUsage" }
            })
            .to_string();
            assert_eq!(parse_frame(&frame), DshFrame::Ignored, "method {method}");
        }
    }

    #[test]
    fn ignores_malformed_and_session_less_frames() {
        assert_eq!(parse_frame("not json"), DshFrame::Ignored);
        let no_session = json!({
            "type": "server-request",
            "method": "session/event",
            "payload": { "event": { "type": "turn/start" } }
        })
        .to_string();
        assert_eq!(parse_frame(&no_session), DshFrame::Ignored);
    }

    #[test]
    fn decodes_the_host_running_bit() {
        let frame = json!({
            "type": "server-request",
            "rpcId": "x",
            "method": "host/session-status",
            "payload": {
                "type": "host/session-status",
                "sessionId": "session-a",
                "running": true
            }
        })
        .to_string();
        assert_eq!(
            parse_frame(&frame),
            DshFrame::Status {
                session_id: "session-a".into(),
                running: true
            }
        );
    }

    /// All four chunk shapes observed live name their block one way or another;
    /// `usage` and `finish` name none.
    #[test]
    fn resolves_the_block_type_of_every_observed_chunk_shape() {
        let cases = [
            (json!({"type":"block-start","index":0,"blockType":"tool-call"}), Some("tool-call")),
            (json!({"type":"block-start","index":0,"blockType":"text"}), Some("text")),
            (json!({"type":"text-delta","index":0,"text":"hi"}), Some("text")),
            (json!({"type":"tool-call-delta","index":0,"name":"bash"}), Some("tool-call")),
            (json!({"type":"block-end","index":0,"block":{"type":"tool-call"}}), Some("tool-call")),
            (json!({"type":"usage","usage":{"outputTokens":84}}), None),
            (json!({"type":"finish","reason":{"kind":"tool-calls"}}), None),
        ];
        for (chunk, want) in cases {
            assert_eq!(
                chunk_block_type(&chunk).as_deref(),
                want,
                "chunk {chunk}"
            );
        }
    }

    #[test]
    fn maps_the_turn_lifecycle_onto_fleet_phases() {
        assert_eq!(phase_of("turn/start", None), Some(SessionStatus::Processing));
        assert_eq!(phase_of("step/start", None), Some(SessionStatus::Processing));
        assert_eq!(phase_of("tool/call", None), Some(SessionStatus::Executing));
        assert_eq!(phase_of("tool/result", None), Some(SessionStatus::Processing));
        assert_eq!(phase_of("turn/end", None), Some(SessionStatus::WaitingInput));
        assert_eq!(
            phase_of("assistant/chunk", Some("text")),
            Some(SessionStatus::Streaming)
        );
        assert_eq!(
            phase_of("assistant/chunk", Some("reasoning")),
            Some(SessionStatus::Thinking)
        );
        assert_eq!(
            phase_of("assistant/chunk", Some("tool-call")),
            Some(SessionStatus::Executing)
        );
    }

    /// A `usage` chunk arrives between a tool call and its result; letting it
    /// reset the phase would flicker the row back to "no idea".
    #[test]
    fn bookkeeping_events_leave_the_phase_alone() {
        assert_eq!(phase_of("assistant/chunk", None), None);
        assert_eq!(phase_of("session/title", None), None);
        assert_eq!(phase_of("agent/inbox/spliced", None), None);
    }

    fn event(sid: &str, kind: &str) -> DshFrame {
        DshFrame::Event {
            session_id: sid.into(),
            kind: kind.into(),
            block_type: None,
            reason_kind: None,
        }
    }

    fn turn_end(sid: &str, reason: &str) -> DshFrame {
        DshFrame::Event {
            session_id: sid.into(),
            kind: "turn/end".into(),
            block_type: None,
            reason_kind: Some(reason.into()),
        }
    }

    #[test]
    fn folds_a_whole_turn_into_the_live_view() {
        let live = LiveView::default();
        let sid = "session-a";
        live.apply(
            DshFrame::Status {
                session_id: sid.into(),
                running: true,
            },
            1_000,
        );
        for (kind, at) in [("turn/start", 1_001u64), ("step/start", 1_002), ("tool/call", 1_003)] {
            live.apply(event(sid, kind), at);
        }
        assert_eq!(live.phase_of(sid, 1_004), Some(SessionStatus::Executing));
        assert_eq!(live.tracked(), 1);
    }

    /// dsh runs turns inside the shared server, so `turn/end` is the only
    /// completion signal Fleet's spawn/resume contract can hang `on_exit` on.
    #[test]
    fn a_finished_turn_settles_its_waiters_with_success() {
        let live = LiveView::default();
        let seen = Arc::new(Mutex::new(Vec::<bool>::new()));
        for _ in 0..2 {
            let sink = seen.clone();
            live.on_turn_end("session-a", Box::new(move |ok| sink.lock().unwrap().push(ok)));
        }
        // Another session's turn ending must not settle ours.
        live.apply(turn_end("session-b", "completed"), 1_000);
        assert!(seen.lock().unwrap().is_empty());

        live.apply(turn_end("session-a", "completed"), 1_001);
        assert_eq!(*seen.lock().unwrap(), vec![true, true]);

        // Waiters fire exactly once: a second turn on the same session must not
        // re-settle callers who already got their answer.
        live.apply(turn_end("session-a", "completed"), 1_002);
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    /// A cancelled turn is not a successful one — reporting it as success would
    /// let the auto-resume scheduler record a win it never had.
    #[test]
    fn an_aborted_turn_settles_its_waiters_with_failure() {
        let live = LiveView::default();
        let seen = Arc::new(Mutex::new(Vec::<bool>::new()));
        let sink = seen.clone();
        live.on_turn_end("session-a", Box::new(move |ok| sink.lock().unwrap().push(ok)));
        live.apply(turn_end("session-a", "aborted"), 1_000);
        assert_eq!(*seen.lock().unwrap(), vec![false]);
    }

    /// Tearing the follower down abandons what it can no longer observe, so a
    /// caller waiting on a turn is never left holding its slot forever.
    #[test]
    fn dropping_the_view_fails_outstanding_waiters() {
        let live = LiveView::default();
        let seen = Arc::new(Mutex::new(Vec::<bool>::new()));
        let sink = seen.clone();
        live.on_turn_end("session-a", Box::new(move |ok| sink.lock().unwrap().push(ok)));
        live.abandon_all();
        assert_eq!(*seen.lock().unwrap(), vec![false]);
    }

    /// A running session's phase never expires — the turn is still in flight
    /// however long the tool takes.
    #[test]
    fn a_running_session_keeps_its_phase_past_the_ttl() {
        let live = LiveSession {
            running: true,
            phase: SessionStatus::Executing,
            phase_at_ms: 1_000,
        };
        assert_eq!(
            live.effective_phase(1_000 + LIVE_STATUS_TTL_MS * 10),
            Some(SessionStatus::Executing)
        );
    }

    /// A stopped session's last phase survives the TTL and then yields to the
    /// polled status, so an hour-old session does not read as "WaitingInput".
    #[test]
    fn a_stopped_session_expires_after_the_ttl() {
        let live = LiveSession {
            running: false,
            phase: SessionStatus::WaitingInput,
            phase_at_ms: 1_000,
        };
        assert_eq!(
            live.effective_phase(1_000 + LIVE_STATUS_TTL_MS),
            Some(SessionStatus::WaitingInput)
        );
        assert_eq!(live.effective_phase(1_000 + LIVE_STATUS_TTL_MS + 1), None);
    }

    /// A session the host has merely announced carries no phase, so the polled
    /// status must stand rather than being overwritten with the default.
    #[test]
    fn a_status_only_session_overlays_nothing() {
        let live = LiveView::default();
        live.apply(
            DshFrame::Status {
                session_id: "session-a".into(),
                running: false,
            },
            1_000,
        );
        assert_eq!(live.phase_of("session-a", 1_000), None);
        assert_eq!(live.tracked(), 1, "it is tracked, it just has no phase");
    }

    #[test]
    fn ignored_frames_do_not_create_entries() {
        let live = LiveView::default();
        live.apply(DshFrame::Ignored, 1_000);
        assert_eq!(live.tracked(), 0);
    }
}
