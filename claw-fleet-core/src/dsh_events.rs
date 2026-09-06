//! Live observation of a `dsh web` instance over its two downlink WebSockets.
//!
//! `session/list` (polled by [`crate::dsh_source`]) carries one liveness bit per
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
//! `events.host` published **nothing** about it, and B's `session/list` reported
//! `running: false` for that session throughout — A's reported `true`. Sessions
//! are shared through the on-disk log; the *live* view is not.
//!
//! So this watcher observes turns Fleet drives through Fleet's own server, which
//! is what the spawn/resume path will do. A session someone runs in their own
//! `dsh` TUI still appears in the list with its history and token totals, but it
//! has no live phase for Fleet to show — and no `running` bit either, so that
//! limit predates this module rather than being introduced by it.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use tokio::sync::mpsc;
use serde_json::Value;

use crate::dsh_client::DshClient;
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

/// The one WebSocket route dsh 0.1.2 publishes on. Every logical stream — the
/// host-wide `$events` and one `session/follow` per followed session — is
/// multiplexed over it (`REMOTE_STREAM_MUX_PATH` in
/// `@deepseek-ai/dsh-api-gateway`). 0.1.1's `/api/events.mux` and
/// `/api/events.host` are both gone.
const REMOTE_MUX_PATH: &str = "/api/remote.mux";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Frame decoding ──────────────────────────────────────────────────────────

/// One logical stream Fleet opens on the mux.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamKind {
    /// `$events` — the host-wide forwarded-event stream. Exactly one per socket.
    Events,
    /// `session/follow` — one session's durable log, opened on demand.
    Follow(String),
}

/// One decoded mux envelope, before its `value` is interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MuxEnvelope {
    /// `{"type":"item","streamId":…,"value":…}` — `value` may be absent.
    Item { stream_id: String, value: Value },
    /// `{"type":"end","streamId":…}` — the stream completed normally.
    End { stream_id: String },
    /// `{"type":"error","streamId":…,"error":{code,message,details}}`.
    Error {
        stream_id: String,
        code: String,
        message: String,
    },
    /// Anything this build does not recognise. dsh is a developer preview whose
    /// frame set grows between releases, so an unknown frame is not a failure.
    Ignored,
}

/// Decode one text message off `/api/remote.mux`.
pub fn parse_envelope(text: &str) -> MuxEnvelope {
    let Ok(parsed) = serde_json::from_str::<Value>(text) else {
        return MuxEnvelope::Ignored;
    };
    let stream_id = parsed
        .get("streamId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if stream_id.is_empty() {
        return MuxEnvelope::Ignored;
    }
    match parsed.get("type").and_then(Value::as_str) {
        Some("item") => MuxEnvelope::Item {
            stream_id,
            value: parsed.get("value").cloned().unwrap_or(Value::Null),
        },
        Some("end") => MuxEnvelope::End { stream_id },
        Some("error") => {
            let error = parsed.get("error");
            MuxEnvelope::Error {
                stream_id,
                code: error
                    .and_then(|e| e.get("code"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                message: error
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            }
        }
        _ => MuxEnvelope::Ignored,
    }
}

/// One decoded stream item, reduced to what Fleet acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DshFrame {
    /// The `$events` opening item. `client_id` is required by every later
    /// waterfall answer, so it is published rather than folded into state.
    Ready { client_id: String },
    /// One entry of a session's durable event log, off its `session/follow`.
    ///
    /// `block_type` is only populated for `assistant/chunk`, whose phase depends
    /// on what kind of block is streaming (text vs reasoning vs tool call).
    Event {
        session_id: String,
        kind: String,
        /// The event's position in the session log. The cursor a history read
        /// needs is the newest one seen, so every event advances it.
        seq: u64,
        block_type: Option<String>,
        /// `turn/end`'s `data.reason.kind` — observed as `completed` when the
        /// agent finished on its own and `aborted` when `session/cancel` cut it
        /// short. Absent on every other event.
        reason_kind: Option<String>,
    },
    /// `api-session/status` — the coarse running bit, pushed on `$events`.
    Status { session_id: String, running: bool },
    /// A follow stream's opening `snapshot`, reduced to its `cursor`.
    ///
    /// That cursor is the only legitimate source of `session/page`'s required
    /// `throughSeq` ("Inclusive log cut obtained from the corresponding follow
    /// opening frame"). `session/list`'s `projections.asOfSeq` looks like it but
    /// is not: measured live, a settled session reported `asOfSeq` 2 against a
    /// real cursor of 135, so paging through it would have truncated the
    /// history to its first two events.
    Cursor { session_id: String, seq: u64 },
    /// `approval/request` — a tool call the session's policy will not run
    /// unattended, delivered as a waterfall. Answerable: [`crate::dsh_decisions`]
    /// raises a card and answers on `event_id` through `$events/result`.
    ///
    /// Not to be confused with the `approval/asked` *session event*, which the
    /// follow stream carries at the same moment: that one is the durable audit
    /// record, it has no event id of its own, and answering it is impossible.
    ApprovalRequested {
        event_id: String,
        session_id: String,
        tool_name: String,
        call_id: Option<String>,
        reason: Option<String>,
    },
    /// `user-questions/request` — the agent called `ask_user_question`.
    QuestionRequested {
        event_id: String,
        session_id: String,
        questions: Vec<crate::dsh_decisions::DshQuestion>,
    },
    /// The host withdrew a pending waterfall: `{"type":"cancel","eventId":…}`.
    /// One shape for both kinds — a cancellation names only the event id, and
    /// the bridge knows which card that id belongs to.
    Withdrawn { event_id: String },
    /// Everything Fleet does not act on: `api-session/activity`, settings and
    /// adapter notices, follow snapshots (their projections already reach Fleet
    /// through `session/list`).
    Ignored,
}

impl DshFrame {
    /// Whether this frame belongs to the decision bridge rather than to the
    /// phase state machine.
    pub fn is_decision(&self) -> bool {
        matches!(
            self,
            Self::ApprovalRequested { .. } | Self::QuestionRequested { .. } | Self::Withdrawn { .. }
        )
    }
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

/// Interpret one `item` value against the stream it arrived on.
///
/// The two streams carry different vocabularies — `$events` carries
/// ready/emit/waterfall/cancel, a follow carries snapshot/event — and nothing in
/// the value itself says which, so the stream identity has to be passed in.
pub fn decode_item(kind: &StreamKind, value: &Value) -> DshFrame {
    match kind {
        StreamKind::Events => decode_event_stream_item(value),
        StreamKind::Follow(session_id) => decode_follow_item(session_id, value),
    }
}

/// One item off `$events`.
fn decode_event_stream_item(value: &Value) -> DshFrame {
    match value.get("type").and_then(Value::as_str) {
        Some("ready") => match value.get("clientId").and_then(Value::as_str) {
            Some(client_id) if !client_id.is_empty() => DshFrame::Ready {
                client_id: client_id.to_string(),
            },
            // A generation with no client id can never answer a waterfall, so it
            // is worth naming rather than silently dropping.
            _ => DshFrame::Ignored,
        },
        Some("emit") => decode_emit(value),
        Some("waterfall") => decode_waterfall(value),
        Some("cancel") => match value.get("eventId").and_then(Value::as_str) {
            Some(event_id) if !event_id.is_empty() => DshFrame::Withdrawn {
                event_id: event_id.to_string(),
            },
            _ => DshFrame::Ignored,
        },
        _ => DshFrame::Ignored,
    }
}

/// `{"type":"emit","event":…,"args":[…]}` — positional args, per the Cordis
/// listener signature the host forwards without renaming.
fn decode_emit(value: &Value) -> DshFrame {
    let event = value.get("event").and_then(Value::as_str).unwrap_or("");
    let args = value.get("args").and_then(Value::as_array);
    let arg = |i: usize| args.and_then(|a| a.get(i));
    match event {
        // `api-session/status(sessionId, running)`.
        "api-session/status" => {
            let Some(session_id) = arg(0).and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                return DshFrame::Ignored;
            };
            DshFrame::Status {
                session_id: session_id.to_string(),
                running: arg(1).and_then(Value::as_bool).unwrap_or(false),
            }
        }
        _ => DshFrame::Ignored,
    }
}

/// `{"type":"waterfall","event":…,"eventId":…,"agentId":…,"request":{…}}`.
///
/// `agentId` is the session id for the session-scoped agents Fleet drives —
/// verified live against `approval/request` raised by a Fleet-created session.
fn decode_waterfall(value: &Value) -> DshFrame {
    let event = value.get("event").and_then(Value::as_str).unwrap_or("");
    let event_id = value
        .get("eventId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    // Without the event id there is no way to answer, and a card nobody can
    // answer is worse than no card: it would block the turn behind a button
    // whose answer the host would refuse.
    if event_id.is_empty() {
        return DshFrame::Ignored;
    }
    let session_id = value
        .get("agentId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let request = value.get("request");
    let field = |key: &str| {
        request
            .and_then(|r| r.get(key))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };

    match event {
        "approval/request" => match field("toolName") {
            Some(tool_name) => DshFrame::ApprovalRequested {
                event_id,
                session_id,
                tool_name,
                call_id: field("callId"),
                reason: field("reason"),
            },
            None => DshFrame::Ignored,
        },
        "user-questions/request" => {
            let questions: Vec<crate::dsh_decisions::DshQuestion> = request
                .and_then(|r| r.get("questions"))
                .and_then(Value::as_array)
                .map(|qs| {
                    qs.iter()
                        .filter_map(crate::dsh_decisions::DshQuestion::from_value)
                        .collect()
                })
                .unwrap_or_default();
            if questions.is_empty() {
                return DshFrame::Ignored;
            }
            DshFrame::QuestionRequested {
                event_id,
                session_id,
                questions,
            }
        }
        _ => DshFrame::Ignored,
    }
}

/// One item off a `session/follow`.
///
/// The opening `snapshot`'s *records* are deliberately not folded: they are
/// history Fleet reads through `session/page`, and replaying them as live events
/// would drive the phase machine from a log that finished minutes ago. Its
/// `cursor` is kept, because that page read cannot be made without it.
fn decode_follow_item(session_id: &str, value: &Value) -> DshFrame {
    if value.get("type").and_then(Value::as_str) == Some("snapshot") {
        return match value.get("cursor").and_then(Value::as_u64) {
            Some(seq) => DshFrame::Cursor {
                session_id: session_id.to_string(),
                seq,
            },
            None => DshFrame::Ignored,
        };
    }
    if value.get("type").and_then(Value::as_str) != Some("event") {
        return DshFrame::Ignored;
    }
    let event = value.get("event");
    let kind = event
        .and_then(|e| e.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if kind.is_empty() {
        return DshFrame::Ignored;
    }
    let data = event.and_then(|e| e.get("data"));
    DshFrame::Event {
        session_id: session_id.to_string(),
        kind,
        seq: event
            .and_then(|e| e.get("seq"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        block_type: data.and_then(|d| d.get("chunk")).and_then(chunk_block_type),
        reason_kind: data
            .and_then(|d| d.get("reason"))
            .and_then(|r| r.get("kind"))
            .and_then(Value::as_str)
            .map(str::to_string),
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
    /// Last pushed `api-session/status`.
    pub running: bool,
    /// Phase derived from the most recent event that carried one.
    pub phase: SessionStatus,
    /// When that phase was set — drives [`LIVE_STATUS_TTL_MS`].
    pub phase_at_ms: u64,
    /// Newest log position this follow stream has reported — the opening
    /// snapshot's `cursor`, then every event's `seq`. `session/page` refuses a
    /// `throughSeq` past the real cursor, so this only ever tracks positions the
    /// server has already published.
    pub cursor: Option<u64>,
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
                seq,
                block_type,
                reason_kind,
            } => {
                {
                    let mut guard = self
                        .sessions
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let entry = guard.entry(session_id.clone()).or_default();
                    if let Some(phase) = phase_of(&kind, block_type.as_deref()) {
                        entry.phase = phase;
                        entry.phase_at_ms = now_ms;
                    }
                    // Every event advances the cut a history read may ask for,
                    // whether or not it means anything to the phase machine.
                    entry.cursor = Some(entry.cursor.map_or(seq, |c| c.max(seq)));
                }
                if kind == "turn/end" {
                    // `aborted` (session/cancel) is the one other kind observed
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
            DshFrame::Cursor { session_id, seq } => {
                let mut guard = self
                    .sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let entry = guard.entry(session_id).or_default();
                entry.cursor = Some(entry.cursor.map_or(seq, |c| c.max(seq)));
            }
            // The answerable frames and their resolutions carry no phase; the
            // pump routes them to `dsh_decisions` instead of here.
            _ => {}
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

    /// The newest log position this session's follow stream has reported.
    pub fn cursor_of(&self, session_id: &str) -> Option<u64> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .and_then(|s| s.cursor)
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

/// A background follower of one `dsh web` instance's mux socket.
///
/// Bound to a single port: a restarted server lands on a fresh OS-assigned port
/// (and mints a fresh launch token), so the owner drops this watcher and starts
/// another rather than reusing it.
pub struct DshEventWatcher {
    port: u16,
    live: SharedLive,
    stop: Arc<AtomicBool>,
    /// Asks the socket thread to open a `session/follow` for one session.
    ///
    /// Unbounded and lossy on purpose: a send that fails means the thread is
    /// gone, which the caller cannot fix and must not block on.
    follow_tx: mpsc::UnboundedSender<String>,
    /// Kept alive for as long as the watcher is: dropping it is what tells the
    /// bridge's worker to withdraw whatever it is still holding.
    _decisions: Arc<crate::dsh_decisions::DecisionBridge>,
}

impl DshEventWatcher {
    /// Open the mux in a background thread and start folding frames.
    ///
    /// Returns immediately; the socket connects (and reconnects) on its own, so
    /// a server that is not answering yet costs nothing but a retry.
    pub fn start(port: u16, launch_token: &str) -> Self {
        let live: SharedLive = Arc::new(LiveView::default());
        let stop = Arc::new(AtomicBool::new(false));
        let decisions = Arc::new(crate::dsh_decisions::DecisionBridge::start(port, launch_token));
        let (follow_tx, follow_rx) = mpsc::unbounded_channel();

        let thread_states = live.clone();
        let thread_stop = stop.clone();
        let thread_decisions = decisions.clone();
        let token = launch_token.to_string();
        let spawned = std::thread::Builder::new()
            .name("dsh-events".into())
            .spawn(move || {
                // The cookie is minted off the runtime: the exchange is one
                // blocking HTTP round trip, and it has to succeed before the
                // handshake is worth attempting at all.
                let cookie = match DshClient::new(port, &token) {
                    Ok(client) => client.cookie().to_string(),
                    Err(e) => {
                        crate::log_debug(&format!("dsh events: no session cookie: {e}"));
                        thread_stop.store(true, Ordering::SeqCst);
                        return;
                    }
                };
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
                rt.block_on(run_mux(
                    port,
                    cookie,
                    thread_states,
                    thread_decisions,
                    thread_stop,
                    follow_rx,
                ));
            });
        if let Err(e) = spawned {
            crate::log_debug(&format!("dsh events: cannot spawn follower: {e}"));
            stop.store(true, Ordering::SeqCst);
        }

        Self {
            port,
            live,
            stop,
            follow_tx,
            _decisions: decisions,
        }
    }

    /// The port this watcher follows.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Start following one session's log, if it is not already followed.
    ///
    /// 0.1.1 published every session's turn lifecycle on one global socket;
    /// 0.1.2 publishes it only per session, on a `session/follow` stream the
    /// client opens by address. So a phase Fleet wants to show has to be asked
    /// for — by the scan, for sessions the host reports running, and by the
    /// launch path, which needs `turn/end` for a session that may not have
    /// started running yet.
    pub fn follow(&self, session_id: &str) {
        let _ = self.follow_tx.send(session_id.to_string());
    }

    /// The phase to show for `session_id`, or `None` when the socket has
    /// nothing fresher than the poll.
    pub fn phase_of(&self, session_id: &str) -> Option<SessionStatus> {
        self.live.phase_of(session_id, now_ms())
    }

    /// The log cut a history read may ask `session/page` for, opening the
    /// follow stream that publishes it if nobody has yet.
    ///
    /// Blocking, up to `budget`: the cursor arrives in the follow stream's
    /// opening frame, so a session Fleet has never followed cannot answer
    /// without one round trip. Returns `None` when the socket is down or the
    /// server does not answer in time — the caller then has no safe cut to ask
    /// for, and asking with a guess would either truncate the history (too low)
    /// or be refused outright (too high).
    pub fn cursor_for_history(&self, session_id: &str, budget: Duration) -> Option<u64> {
        if let Some(seq) = self.live.cursor_of(session_id) {
            return Some(seq);
        }
        self.follow(session_id);
        let deadline = std::time::Instant::now() + budget;
        while std::time::Instant::now() < deadline {
            if let Some(seq) = self.live.cursor_of(session_id) {
                return Some(seq);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }

    /// Call `cb` when this session's next turn ends. See [`LiveView::on_turn_end`].
    ///
    /// Also opens the follow stream that will carry that `turn/end`: registering
    /// a waiter for a session nobody is following would wait forever.
    pub fn on_turn_end(&self, session_id: &str, cb: TurnEndCallback) {
        self.follow(session_id);
        self.live.on_turn_end(session_id, cb);
    }

    /// How many sessions the socket has reported on. Diagnostics and tests.
    pub fn tracked(&self) -> usize {
        self.live.tracked()
    }
}

impl Drop for DshEventWatcher {
    fn drop(&mut self) {
        // The follower notices within one `READ_TICK` and closes the socket.
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

/// Build one `{"type":"open",…}` request for a logical stream.
///
/// The host validates these keys exactly — an extra or missing one is refused
/// before the endpoint is looked at — and `payload` carries the same
/// `{"args": …}` wrapper every unary call does.
fn open_frame(stream_id: &str, kind: &StreamKind) -> String {
    let (endpoint, args) = match kind {
        StreamKind::Events => ("$events", serde_json::json!({})),
        StreamKind::Follow(session_id) => (
            "session/follow",
            serde_json::json!({
                "request": {
                    "address": { "kind": "session", "sessionId": session_id },
                    // Only the snapshot's `cursor` is kept — its records are
                    // history `session/page` reads on demand — so ask for the
                    // smallest window dsh will build rather than one nobody
                    // reads. The cursor is the log's cut, not the window's, so
                    // shrinking this does not shorten the history a page read
                    // can then reach.
                    "maxMessages": 1
                }
            }),
        ),
    };
    serde_json::json!({
        "type": "open",
        "streamId": stream_id,
        "endpoint": endpoint,
        "payload": { "args": args },
    })
    .to_string()
}

/// Register one logical stream and render its `open` request.
fn open_stream(streams: &mut HashMap<String, StreamKind>, kind: StreamKind) -> String {
    let stream_id = uuid::Uuid::new_v4().to_string();
    let frame = open_frame(&stream_id, &kind);
    streams.insert(stream_id, kind);
    frame
}

/// Keep one mux socket connected for as long as the watcher lives.
///
/// Sessions asked for while the socket is down are remembered and re-opened on
/// the next generation: a logical stream does not survive its carrier, and the
/// launch path's `turn/end` waiter must not be lost to a server restart.
async fn run_mux(
    port: u16,
    cookie: String,
    states: SharedLive,
    decisions: Arc<crate::dsh_decisions::DecisionBridge>,
    stop: Arc<AtomicBool>,
    mut follow_rx: mpsc::UnboundedReceiver<String>,
) {
    let mut wanted: HashSet<String> = HashSet::new();
    while !stop.load(Ordering::SeqCst) {
        // Anything asked for while disconnected is picked up here rather than
        // dropped on the floor.
        while let Ok(session_id) = follow_rx.try_recv() {
            wanted.insert(session_id);
        }
        match connect(port, &cookie).await {
            Ok(ws) => {
                pump(
                    ws,
                    &states,
                    &decisions,
                    &stop,
                    &mut follow_rx,
                    &mut wanted,
                )
                .await
            }
            Err(e) => crate::log_debug(&format!("dsh events: {e}")),
        }
        sleep_interruptible(RECONNECT_BACKOFF, &stop).await;
    }
}

async fn connect(port: u16, cookie: &str) -> Result<DshWs, String> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let url = format!("ws://127.0.0.1:{port}{REMOTE_MUX_PATH}");
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|e| format!("mux request {url}: {e}"))?;
    // The mux sits behind the same browser-authentication gate as `/api`: an
    // unauthenticated handshake is answered 401 and never becomes a socket.
    let header = cookie
        .parse()
        .map_err(|e| format!("mux cookie header: {e}"))?;
    request
        .headers_mut()
        .insert(tokio_tungstenite::tungstenite::http::header::COOKIE, header);

    match tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(request)).await {
        Ok(Ok((ws, _))) => Ok(ws),
        Ok(Err(e)) => Err(format!("connect {url}: {e}")),
        Err(_) => Err(format!(
            "connect {url}: timed out after {}s",
            CONNECT_TIMEOUT.as_secs()
        )),
    }
}

/// Read frames until the socket closes or the watcher is dropped.
async fn pump(
    ws: DshWs,
    states: &SharedLive,
    decisions: &Arc<crate::dsh_decisions::DecisionBridge>,
    stop: &Arc<AtomicBool>,
    follow_rx: &mut mpsc::UnboundedReceiver<String>,
    wanted: &mut HashSet<String>,
) {
    use futures::SinkExt;
    use tokio_tungstenite::tungstenite::Message;

    let (mut write, mut read) = ws.split();
    // Logical streams live and die with this physical socket, so the registry
    // is rebuilt per generation rather than carried across reconnects.
    let mut streams: HashMap<String, StreamKind> = HashMap::new();
    let hello = open_stream(&mut streams, StreamKind::Events);
    if let Err(e) = write.send(Message::Text(hello.into())).await {
        crate::log_debug(&format!("dsh events: open $events: {e}"));
        return;
    }
    for session_id in wanted.iter() {
        let frame = open_stream(&mut streams, StreamKind::Follow(session_id.clone()));
        if let Err(e) = write.send(Message::Text(frame.into())).await {
            crate::log_debug(&format!("dsh events: reopen follow: {e}"));
            return;
        }
    }

    while !stop.load(Ordering::SeqCst) {
        tokio::select! {
            biased;

            asked = follow_rx.recv() => {
                let Some(session_id) = asked else { return };
                if !wanted.insert(session_id.clone()) {
                    continue;
                }
                let frame = open_stream(&mut streams, StreamKind::Follow(session_id));
                if let Err(e) = write.send(Message::Text(frame.into())).await {
                    crate::log_debug(&format!("dsh events: open follow: {e}"));
                    return;
                }
            }

            // The timeout is the only reason this loop re-checks `stop` on an
            // idle socket: dsh publishes nothing between turns, so a bare
            // `next()` would park here until the next session ran.
            read = tokio::time::timeout(READ_TICK, read.next()) => {
                let message = match read {
                    Err(_) => continue,
                    Ok(None) => return,
                    Ok(Some(Err(e))) => {
                        crate::log_debug(&format!("dsh events: read: {e}"));
                        return;
                    }
                    Ok(Some(Ok(m))) => m,
                };
                match message {
                    Message::Text(text) => {
                        match parse_envelope(&text) {
                            MuxEnvelope::Item { stream_id, value } => {
                                let Some(kind) = streams.get(&stream_id) else { continue };
                                let frame = decode_item(kind, &value);
                                // Answerable frames go to the bridge's own
                                // thread: raising a card and answering it are
                                // blocking file + HTTP work, and this is a
                                // tokio worker.
                                if frame.is_decision() {
                                    decisions.offer(frame);
                                } else if let DshFrame::Ready { client_id } = frame {
                                    // Every waterfall answer is scoped to the
                                    // generation that delivered it, so the
                                    // bridge cannot answer before this arrives.
                                    decisions.set_client_id(client_id);
                                } else {
                                    // A session the host reports running gets
                                    // followed here, so its phases show up
                                    // without anyone having asked in advance —
                                    // the poll learns of a session at most one
                                    // tick later, and by then its first chunks
                                    // are gone.
                                    if let DshFrame::Status { session_id, running: true } = &frame {
                                        if wanted.insert(session_id.clone()) {
                                            let open = open_stream(
                                                &mut streams,
                                                StreamKind::Follow(session_id.clone()),
                                            );
                                            if let Err(e) =
                                                write.send(Message::Text(open.into())).await
                                            {
                                                crate::log_debug(&format!(
                                                    "dsh events: follow a running session: {e}"
                                                ));
                                                return;
                                            }
                                        }
                                    }
                                    states.apply(frame, now_ms());
                                }
                            }
                            MuxEnvelope::End { stream_id } => {
                                streams.remove(&stream_id);
                            }
                            MuxEnvelope::Error { stream_id, code, message } => {
                                if let Some(kind) = streams.remove(&stream_id) {
                                    crate::log_debug(&format!(
                                        "dsh events: stream {kind:?} failed: {code}: {message}"
                                    ));
                                    // A follow that failed is no longer wanted
                                    // under this generation's id; the next
                                    // reconnect reopens it from `wanted`.
                                }
                            }
                            MuxEnvelope::Ignored => {}
                        }
                    }
                    Message::Close(_) => return,
                    // Ping/Pong are answered by the stream itself; binary frames
                    // are not part of this protocol.
                    _ => {}
                }
            }
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
     /// Every frame in this block is verbatim from a live capture: a node `ws`
    /// client on `/api/remote.mux` against dsh 0.1.2-rc.1, driving one real turn
    /// and one real approval.
    fn item(stream_id: &str, value: Value) -> String {
        json!({ "type": "item", "streamId": stream_id, "value": value }).to_string()
    }

    fn follow(session_id: &str) -> StreamKind {
        StreamKind::Follow(session_id.to_string())
    }

    /// Decode one captured `item` the way the pump does: envelope first, then
    /// the value against the stream it arrived on.
    fn decode(kind: &StreamKind, value: Value) -> DshFrame {
        match parse_envelope(&item("s-1", value)) {
            MuxEnvelope::Item { value, .. } => decode_item(kind, &value),
            other => panic!("expected an item envelope, got {other:?}"),
        }
    }

    #[test]
    fn decodes_a_live_tool_call_frame() {
        let value = json!({
            "type": "event",
            "event": {
                "type": "tool/call",
                "seq": 389,
                "time": 1788723346552u64,
                "data": {
                    "turn": 1, "step": 1,
                    "callId": "call_00_Mxllh1ma05OXI5WLpwSy1032",
                    "name": "bash"
                }
            }
        });
        assert_eq!(
            decode(&follow("session-ef76dbb8"), value),
            DshFrame::Event {
                session_id: "session-ef76dbb8".into(),
                kind: "tool/call".into(),
                seq: 389,
                block_type: None,
                reason_kind: None,
            }
        );
    }

    /// Verbatim `turn/end` values: one that finished, one cut short by
    /// `session/cancel`. The nested `reason.kind` is the only thing telling the
    /// two apart, and it decides what `on_turn_end` reports.
    #[test]
    fn decodes_the_outcome_of_a_finished_turn() {
        let end = |reason: Value| {
            json!({
                "type": "event",
                "event": { "type": "turn/end", "seq": 48, "data": { "turn": 1, "reason": reason } }
            })
        };
        let completed = decode(&follow("session-a"), end(json!({ "kind": "completed" })));
        let aborted = decode(
            &follow("session-a"),
            end(json!({ "kind": "aborted", "reason": { "kind": "user" } })),
        );
        assert!(matches!(
            completed,
            DshFrame::Event { ref reason_kind, .. } if reason_kind.as_deref() == Some("completed")
        ));
        assert!(matches!(
            aborted,
            DshFrame::Event { ref reason_kind, .. } if reason_kind.as_deref() == Some("aborted")
        ));
    }

    /// The two streams speak different vocabularies and nothing in a value says
    /// which one it came from, so the same bytes must decode differently per
    /// stream. A follow item read as an `$events` item would be a silent
    /// mis-decode rather than an error.
    #[test]
    fn the_stream_identity_decides_how_a_value_reads() {
        let value = json!({ "type": "event", "event": { "type": "turn/start", "data": {} } });
        assert!(matches!(
            decode(&follow("session-a"), value.clone()),
            DshFrame::Event { .. }
        ));
        assert_eq!(decode(&StreamKind::Events, value), DshFrame::Ignored);
    }

    /// The opening item of `$events`. Its `clientId` is the only way to answer a
    /// waterfall, so losing it would take every decision card with it.
    #[test]
    fn decodes_the_opening_ready_frame() {
        let value = json!({
            "type": "ready",
            "clientId": "dd879fb9-23c1-4eb0-8b5e-1745d2ce4b51",
            "host": { "home": "/Users/hoveychen" }
        });
        assert_eq!(
            decode(&StreamKind::Events, value),
            DshFrame::Ready {
                client_id: "dd879fb9-23c1-4eb0-8b5e-1745d2ce4b51".into()
            }
        );
    }

    /// `api-session/status` is an `emit` with *positional* args, mirroring the
    /// Cordis listener signature the host forwards unchanged.
    #[test]
    fn decodes_the_host_running_bit() {
        for running in [true, false] {
            let value = json!({
                "type": "emit",
                "event": "api-session/status",
                "args": ["session-63035897-e9ff-45be-a076-89e50847b7a8", running]
            });
            assert_eq!(
                decode(&StreamKind::Events, value),
                DshFrame::Status {
                    session_id: "session-63035897-e9ff-45be-a076-89e50847b7a8".into(),
                    running,
                }
            );
        }
    }

    #[test]
    fn ignores_emits_fleet_does_not_act_on() {
        let value = json!({
            "type": "emit",
            "event": "api-session/activity",
            "args": ["session-a", 1788723263490u64]
        });
        assert_eq!(decode(&StreamKind::Events, value), DshFrame::Ignored);
    }

    /// Verbatim `approval/request` waterfall, captured while a
    /// `workspace-write` session tried to touch a file outside its workspace.
    #[test]
    fn decodes_a_live_approval_request() {
        let value = json!({
            "type": "waterfall",
            "event": "approval/request",
            "eventId": "04eccea2-3c00-416e-955c-2f0a802a5a62",
            "agentId": "session-0304f53d-19db-4e4f-b240-ff4961c16e2d",
            "request": {
                "toolName": "bash",
                "callId": "call_00_7ENJ63IKfC3CFTIrrETU9309",
                "reason": "escalate sandbox to danger-full-access: writes outside the workspace"
            }
        });
        match decode(&StreamKind::Events, value) {
            DshFrame::ApprovalRequested {
                event_id,
                session_id,
                tool_name,
                call_id,
                reason,
            } => {
                assert_eq!(event_id, "04eccea2-3c00-416e-955c-2f0a802a5a62");
                assert_eq!(session_id, "session-0304f53d-19db-4e4f-b240-ff4961c16e2d");
                assert_eq!(tool_name, "bash");
                assert_eq!(call_id.as_deref(), Some("call_00_7ENJ63IKfC3CFTIrrETU9309"));
                assert!(reason.unwrap().contains("danger-full-access"));
            }
            other => panic!("expected ApprovalRequested, got {other:?}"),
        }
    }

    /// The session's follow stream carries a durable `approval/asked` event at
    /// the same moment as the waterfall. It has no event id of its own, so
    /// answering it is impossible — it must not be mistaken for the request.
    #[test]
    fn the_durable_approval_audit_event_is_not_the_answerable_frame() {
        let value = json!({
            "type": "event",
            "event": {
                "type": "approval/asked",
                "seq": 40,
                "data": { "id": "ap-1", "toolName": "bash", "reason": "why" }
            }
        });
        assert!(matches!(
            decode(&follow("session-a"), value),
            DshFrame::Event { ref kind, .. } if kind == "approval/asked"
        ));
    }

    /// A waterfall with no event id cannot be answered, and a card nobody can
    /// answer would park the turn behind a permanently refused button.
    #[test]
    fn an_unanswerable_approval_is_dropped() {
        let value = json!({
            "type": "waterfall",
            "event": "approval/request",
            "agentId": "session-a",
            "request": { "toolName": "bash" }
        });
        assert_eq!(decode(&StreamKind::Events, value), DshFrame::Ignored);
    }

    #[test]
    fn decodes_a_withdrawn_waterfall() {
        let value = json!({ "type": "cancel", "eventId": "04eccea2-3c00-416e-955c-2f0a802a5a62" });
        assert_eq!(
            decode(&StreamKind::Events, value),
            DshFrame::Withdrawn {
                event_id: "04eccea2-3c00-416e-955c-2f0a802a5a62".into()
            }
        );
    }

    #[test]
    fn decodes_a_question_request() {
        let value = json!({
            "type": "waterfall",
            "event": "user-questions/request",
            "eventId": "q-event-1",
            "agentId": "session-a",
            "request": {
                "questions": [{
                    "id": "pick",
                    "question": "Which database?",
                    "header": "Database",
                    "options": [{ "label": "Postgres" }, { "label": "SQLite" }]
                }]
            }
        });
        match decode(&StreamKind::Events, value) {
            DshFrame::QuestionRequested {
                event_id,
                session_id,
                questions,
            } => {
                assert_eq!(event_id, "q-event-1");
                assert_eq!(session_id, "session-a");
                assert_eq!(questions.len(), 1);
                assert_eq!(questions[0].id, "pick");
                assert_eq!(questions[0].options.len(), 2);
            }
            other => panic!("expected QuestionRequested, got {other:?}"),
        }
    }

    /// The opening snapshot's records are history `session/page` reads; folding
    /// them would replay a finished turn through the phase machine and light up
    /// a session idle for minutes. Its `cursor` is the one thing kept — it is
    /// the only legitimate `throughSeq` for that page read.
    #[test]
    fn the_follow_snapshot_yields_its_cursor_and_nothing_else() {
        let value = json!({
            "type": "snapshot",
            "header": { "version": 0, "id": "session-a", "cwd": "/tmp" },
            "cursor": 2,
            "records": [{ "type": "event", "event": { "type": "turn/end", "seq": 1, "data": {} } }],
            "hasMore": false,
            "projections": { "asOfSeq": 2, "values": {} }
        });
        assert_eq!(
            decode(&follow("session-a"), value),
            DshFrame::Cursor {
                session_id: "session-a".into(),
                seq: 2,
            }
        );
    }

    /// The envelope layer: three terminal shapes, each naming its stream.
    #[test]
    fn decodes_every_envelope_shape() {
        assert_eq!(
            parse_envelope(r#"{"type":"end","streamId":"s-1"}"#),
            MuxEnvelope::End {
                stream_id: "s-1".into()
            }
        );
        assert_eq!(
            parse_envelope(
                r#"{"type":"error","streamId":"s-1","error":{"code":"gateway/cancelled","message":"gone","details":{}}}"#
            ),
            MuxEnvelope::Error {
                stream_id: "s-1".into(),
                code: "gateway/cancelled".into(),
                message: "gone".into(),
            }
        );
        // An item may arrive without a value at all.
        assert_eq!(
            parse_envelope(r#"{"type":"item","streamId":"s-1"}"#),
            MuxEnvelope::Item {
                stream_id: "s-1".into(),
                value: Value::Null,
            }
        );
    }

    #[test]
    fn ignores_malformed_and_stream_less_envelopes() {
        for text in [
            "",
            "not json",
            r#"{"type":"item"}"#,
            r#"{"streamId":"s-1"}"#,
            r#"{"type":"open","streamId":"s-1","endpoint":"$events","payload":{}}"#,
        ] {
            assert_eq!(parse_envelope(text), MuxEnvelope::Ignored, "text {text:?}");
        }
    }

    /// The `open` request the pump sends. The host validates these keys exactly
    /// — one extra or missing key is refused before the endpoint is looked at.
    #[test]
    fn opens_each_stream_with_the_shape_the_gateway_validates() {
        let events: Value = serde_json::from_str(&open_frame("s-1", &StreamKind::Events)).unwrap();
        assert_eq!(events["type"], "open");
        assert_eq!(events["streamId"], "s-1");
        assert_eq!(events["endpoint"], "$events");
        assert_eq!(events["payload"], json!({ "args": {} }));
        assert_eq!(events.as_object().unwrap().len(), 4);

        let follow: Value =
            serde_json::from_str(&open_frame("s-2", &follow("session-a"))).unwrap();
        assert_eq!(follow["endpoint"], "session/follow");
        assert_eq!(
            follow["payload"]["args"]["request"]["address"],
            json!({ "kind": "session", "sessionId": "session-a" })
        );
    }

    #[test]
    fn only_the_answerable_frames_route_to_the_bridge() {
        assert!(DshFrame::Withdrawn {
            event_id: "e".into()
        }
        .is_decision());
        assert!(!event("session-a", "tool/call").is_decision());
        assert!(!DshFrame::Ready {
            client_id: "c".into()
        }
        .is_decision());
        assert!(!DshFrame::Status {
            session_id: "session-a".into(),
            running: true
        }
        .is_decision());
        assert!(!DshFrame::Ignored.is_decision());
    }

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
            seq: 0,
            block_type: None,
            reason_kind: None,
        }
    }

    fn turn_end(sid: &str, reason: &str) -> DshFrame {
        DshFrame::Event {
            session_id: sid.into(),
            kind: "turn/end".into(),
            seq: 0,
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
            cursor: None,
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
            cursor: None,
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

    /// The cursor a history read asks for must never run ahead of what the
    /// server has published — `session/page` refuses a `throughSeq` past the
    /// real cut — and must never run backwards either, or a later read would
    /// silently truncate the history it just walked.
    #[test]
    fn the_cursor_advances_with_the_log_and_never_retreats() {
        let live = LiveView::default();
        assert_eq!(live.cursor_of("session-a"), None);

        live.apply(
            DshFrame::Cursor {
                session_id: "session-a".into(),
                seq: 48,
            },
            1_000,
        );
        assert_eq!(live.cursor_of("session-a"), Some(48));

        live.apply(
            DshFrame::Event {
                session_id: "session-a".into(),
                kind: "turn/start".into(),
                seq: 49,
                block_type: None,
                reason_kind: None,
            },
            1_000,
        );
        assert_eq!(live.cursor_of("session-a"), Some(49));

        // A late frame from an older cut (a reconnect replays the snapshot)
        // must not pull the cursor back.
        live.apply(
            DshFrame::Cursor {
                session_id: "session-a".into(),
                seq: 12,
            },
            1_000,
        );
        assert_eq!(live.cursor_of("session-a"), Some(49));
    }

    #[test]
    fn ignored_frames_do_not_create_entries() {
        let live = LiveView::default();
        live.apply(DshFrame::Ignored, 1_000);
        assert_eq!(live.tracked(), 0);
    }
}
