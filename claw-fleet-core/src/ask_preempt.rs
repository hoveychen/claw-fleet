//! Let a message that reaches a session blocked on a `fleet__ask` card be
//! handled first, instead of sitting behind the card until the boss answers.
//!
//! While a card waits, Claude Code is inside one tool call: a message injected
//! over the session socket ([`crate::live_inject`]) sits in the CLI's queue and
//! is only absorbed after the tool returns. So a watch event, a follow-up the
//! boss typed, or a `fleet send` all wait for the card — and before this module,
//! a watch would SIGINT the turn to get through.
//!
//! The protocol has three pieces:
//!
//! 1. **Inbox marker** — [`touch_inbox`]: every successful live inject stamps
//!    `~/.fleet/session-inbox/<sid>.json`. The blocked producer polls it.
//! 2. **Early return** — when the marker moves past the wait's start
//!    ([`should_preempt`]), the producer returns a plain-text result telling the
//!    agent to deal with the message first, then `wait` on the card again or
//!    `withdraw` it. The card itself stays open on the boss's screen; the
//!    **detached store** (`~/.fleet/fleet-ask-detached/<request_id>.json`) is the
//!    record that no producer is polling it right now.
//! 3. **Answer while detached** — `mcp_ipc::write_response` tells a live
//!    session (without the answer's content) that the card was resolved, and
//!    [`sweep_idle`] covers a session whose turn ended with the card detached:
//!    the answer resumes it through the ordinary parked path.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A marker older than this cannot have triggered the "arrived while the model
/// was still writing the ask call" clause of [`should_preempt`].
pub const RECENT_INBOX_MS: u64 = 10 * 60 * 1000;

/// Slack between the marker stamp and CLI's own `queue-operation` timestamp:
/// the inject writes the socket first and stamps the marker after, so the
/// CLI's enqueue can precede the marker by a little.
const QUEUE_CLOCK_SLACK_MS: u64 = 2_000;

/// How long [`sweep_idle`] leaves a fresh response alone, so it never races the
/// write-time recovery in `mcp_ipc::write_response`.
const SWEEP_SETTLE_MS: u64 = 10_000;

/// How much of the transcript tail [`transcript_queue_pending`] reads.
const TRANSCRIPT_TAIL_BYTES: u64 = 512 * 1024;

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

/// Same sanitisation as the inflight marker: no id may escape its directory.
fn safe_name(id: &str) -> Option<String> {
    if id.trim().is_empty() {
        return None;
    }
    Some(
        id.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect(),
    )
}

fn fleet_subdir(name: &str) -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join(name))
}

// ── Inbox marker ─────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InboxMarker {
    pub at_ms: u64,
    /// `"user"` or `"agent"` — who the last injected message came from.
    pub sender: String,
    /// Set once this marker has cut a wait short through the transcript clause
    /// of [`should_preempt`], so a misread transcript can never make every
    /// later ask return early on the same message.
    #[serde(default)]
    pub handled: bool,
}

fn inbox_path(session_id: &str) -> Option<PathBuf> {
    let name = safe_name(session_id)?;
    fleet_subdir("session-inbox").map(|d| d.join(format!("{name}.json")))
}

/// Stamp "a message was just injected into this session's live turn".
/// Best-effort: a failed write only means a blocked card does not step aside.
pub fn touch_inbox(session_id: &str, sender: crate::pending_message::Sender) {
    let Some(path) = inbox_path(session_id) else {
        return;
    };
    let marker = InboxMarker {
        at_ms: now_ms(),
        sender: match sender {
            crate::pending_message::Sender::User => "user",
            crate::pending_message::Sender::Agent => "agent",
        }
        .to_string(),
        handled: false,
    };
    write_json(&path, &marker);
}

pub fn read_inbox(session_id: &str) -> Option<InboxMarker> {
    read_json(&inbox_path(session_id)?)
}

/// Mark the marker stamped at `at_ms` as handled. A newer marker is left alone.
fn mark_inbox_handled(session_id: &str, at_ms: u64) {
    let Some(path) = inbox_path(session_id) else {
        return;
    };
    if let Some(mut m) = read_json::<InboxMarker>(&path) {
        if m.at_ms == at_ms && !m.handled {
            m.handled = true;
            write_json(&path, &m);
        }
    }
}

// ── Detached store ───────────────────────────────────────────────────────────

/// A card that is still open on the boss's screen while no producer polls it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DetachedCard {
    pub request_id: String,
    pub session_id: String,
    pub detached_at: u64,
}

fn detached_dir() -> Option<PathBuf> {
    fleet_subdir("fleet-ask-detached")
}

fn detached_path(request_id: &str) -> Option<PathBuf> {
    let name = safe_name(request_id)?;
    detached_dir().map(|d| d.join(format!("{name}.json")))
}

pub fn mark_detached(request_id: &str, session_id: &str) {
    if let Some(path) = detached_path(request_id) {
        write_json(
            &path,
            &DetachedCard {
                request_id: request_id.to_string(),
                session_id: session_id.to_string(),
                detached_at: now_ms(),
            },
        );
    }
}

/// Idempotent. `mcp_ipc::cleanup` calls it, so every way a card closes (answer
/// consumed, orphan recovery, withdraw, timeout) drops the record too.
pub fn clear_detached(request_id: &str) {
    if let Some(path) = detached_path(request_id) {
        let _ = fs::remove_file(path);
    }
}

pub fn get_detached(request_id: &str) -> Option<DetachedCard> {
    read_json(&detached_path(request_id)?)
}

pub fn is_detached(request_id: &str) -> bool {
    detached_path(request_id).is_some_and(|p| p.is_file())
}

/// [`is_detached`], also true while [`sweep_idle`] holds the record under its
/// claim name — the sweep calls the orphan recovery from inside that claim.
pub fn was_detached(request_id: &str) -> bool {
    detached_path(request_id)
        .is_some_and(|p| p.is_file() || p.with_extension("sweeping").is_file())
}

fn list_detached() -> Vec<DetachedCard> {
    let Some(dir) = detached_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| read_json(&e.path()))
        .collect()
}

/// The open detached card of `session_id`, if any. A record whose request file
/// is gone (answered and consumed elsewhere, or removed by hand) is pruned.
pub fn detached_for_session(session_id: &str) -> Option<String> {
    if session_id.trim().is_empty() {
        return None;
    }
    let mut found = None;
    for card in list_detached().into_iter().filter(|c| c.session_id == session_id) {
        if crate::mcp_ipc::read_request(&card.request_id).is_some() {
            found.get_or_insert(card.request_id);
        } else {
            clear_detached(&card.request_id);
        }
    }
    found
}

// ── Early-return decision ────────────────────────────────────────────────────

/// Whether a wait that started at `wait_start_ms` should step aside for an
/// injected message.
///
/// - A marker stamped after the wait started always wins: while the tool call
///   blocks, the CLI cannot have absorbed that message.
/// - A marker stamped shortly *before* the wait started (the message arrived
///   while the model was still writing the call) wins only when it has not cut
///   a wait short already and `queue_pending_since` — the transcript check —
///   says the CLI still holds an unabsorbed message from that moment on.
pub fn should_preempt(
    marker: Option<&InboxMarker>,
    wait_start_ms: u64,
    queue_pending_since: impl FnOnce(u64) -> bool,
) -> bool {
    let Some(m) = marker else {
        return false;
    };
    if m.at_ms > wait_start_ms {
        return true;
    }
    !m.handled
        && wait_start_ms - m.at_ms <= RECENT_INBOX_MS
        && queue_pending_since(m.at_ms.saturating_sub(QUEUE_CLOCK_SLACK_MS))
}

/// Does the transcript show a message the CLI queued at or after `since_ms`
/// that it has not taken up yet?
///
/// Claude Code logs `{"type":"queue-operation","operation":…}` lines: `enqueue`
/// when a message arrives, `remove` when it is absorbed mid-turn and `dequeue`
/// when a new turn takes it. Counting from `since_ms` on, a positive balance is
/// an unabsorbed message.
pub fn cc_queue_pending(bytes: &[u8], since_ms: u64) -> bool {
    let mut balance: i64 = 0;
    for line in bytes.split(|b| *b == b'\n') {
        // Cheap prefilter before a JSON parse of every transcript line.
        if !line.windows(15).any(|w| w == b"queue-operation") {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("queue-operation") {
            continue;
        }
        let at = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|t| t.timestamp_millis().max(0) as u64);
        if at.is_none_or(|at| at < since_ms) {
            continue;
        }
        match v.get("operation").and_then(|o| o.as_str()) {
            Some("enqueue") => balance += 1,
            Some("remove") | Some("dequeue") => balance = (balance - 1).max(0),
            _ => {}
        }
    }
    balance > 0
}

/// [`cc_queue_pending`] over the tail of the session's transcript. False when
/// the transcript cannot be found or read.
pub fn transcript_queue_pending(session_id: &str, since_ms: u64) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Some(path) = crate::session::find_session_jsonl(session_id) else {
        return false;
    };
    let Ok(mut f) = fs::File::open(&path) else {
        return false;
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TRANSCRIPT_TAIL_BYTES);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return false;
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return false;
    }
    cc_queue_pending(&buf, since_ms)
}

/// Per-wait preemption probe for the `fleet__ask` producer. The transcript
/// clause is evaluated once, on the first poll.
pub struct Preempter {
    session_id: String,
    wait_start_ms: u64,
    first_poll: bool,
}

impl Preempter {
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            wait_start_ms: now_ms(),
            first_poll: true,
        }
    }

    /// True when the wait should step aside now.
    pub fn poll(&mut self) -> bool {
        let first = std::mem::replace(&mut self.first_poll, false);
        let marker = read_inbox(&self.session_id);
        let hit = should_preempt(marker.as_ref(), self.wait_start_ms, |since| {
            first && transcript_queue_pending(&self.session_id, since)
        });
        if hit {
            if let Some(m) = marker {
                mark_inbox_handled(&self.session_id, m.at_ms);
            }
        }
        hit
    }
}

/// What the agent reads when its card steps aside. Chinese: agent-facing
/// product text, like the rest of Fleet's injected guidance.
pub fn detached_notice(request_id: &str) -> String {
    format!(
        "[fleet] 有新消息进来了，决策卡 `{request_id}` 先让路——它没有撤回，仍然显示在老板的屏幕上，老板随时可以回答。\n\
         紧接着这条结果之后就是那条新消息：先处理它。\n\
         处理完后：\n\
         - 这张卡仍然需要答复 → 调用 `fleet__ask`，参数 `{{\"action\":\"wait\",\"request_id\":\"{request_id}\"}}` 继续等老板的回答；\n\
         - 新消息已经让这张卡过时 → 调用 `fleet__ask`，参数 `{{\"action\":\"withdraw\",\"request_id\":\"{request_id}\"}}` 把它从老板的屏幕上撤下。\n\
         这张卡没 wait 或 withdraw 之前，不要再发新卡。"
    )
}

/// What a live session is told when its detached card gets resolved. Never
/// carries the answer itself: the agent fetches it with `wait`, which also
/// writes the decision history.
pub fn resolved_notice(request_id: &str) -> String {
    format!(
        "[fleet] 决策卡 `{request_id}` 老板已经处理了。调用 `fleet__ask`，参数 `{{\"action\":\"wait\",\"request_id\":\"{request_id}\"}}` 取回结果，再按结果继续。"
    )
}

// ── Answer while detached ────────────────────────────────────────────────────

/// Called by `mcp_ipc::write_response` once the write-time orphan recovery has
/// declined the response (the session is still running). A detached card has no
/// producer to read it, so tell the live turn to fetch it.
pub fn notify_resolved_if_detached(request_id: &str) {
    let Some(card) = get_detached(request_id) else {
        return;
    };
    if !crate::parked::session_alive(&card.session_id) {
        // The turn is over: `sweep_idle` resumes it with the answer.
        return;
    }
    let Some(workspace) = crate::parked::parkable_workspace(&card.session_id) else {
        return;
    };
    match crate::pending_message::enqueue(
        &card.session_id,
        &workspace,
        &resolved_notice(request_id),
        crate::pending_message::Sender::Agent,
    ) {
        Ok(how) => crate::log_debug(&format!(
            "ask_preempt: detached card {request_id} resolved -> notice {how:?} to {}",
            card.session_id
        )),
        Err(e) => crate::log_debug(&format!(
            "ask_preempt: detached card {request_id} resolved, notice failed: {e}"
        )),
    }
}

/// Fallback for a detached card whose session is no longer running and whose
/// answer is in: hand it to the parked path, which resumes the session with
/// the answer. Covers a turn that ended without calling `wait` (it ignored the
/// resolved notice, crashed, or lost the race against the write-time recovery).
///
/// Runs on the session-refresh tick; one directory listing, normally empty.
pub fn sweep_idle() {
    for card in list_detached() {
        sweep_one(&card, crate::parked::session_alive, |id| {
            crate::mcp_ipc::recover_orphaned_response(id)
        });
    }
}

fn sweep_one(
    card: &DetachedCard,
    alive: impl FnOnce(&str) -> bool,
    recover: impl FnOnce(&str) -> Result<bool, String>,
) {
    let id = card.request_id.as_str();
    if crate::mcp_ipc::read_request(id).is_none() {
        clear_detached(id);
        return;
    }
    let Some(age) = crate::mcp_ipc::response_age_ms(id) else {
        return;
    };
    if age < SWEEP_SETTLE_MS || alive(&card.session_id) {
        return;
    }
    // Claim by rename so two refresh threads (or the desktop and a `fleet
    // serve`) never both resume the session.
    let (Some(src), Some(dst)) = (detached_path(id), detached_path(id).map(|p| p.with_extension("sweeping")))
    else {
        return;
    };
    if fs::rename(&src, &dst).is_err() {
        return;
    }
    match recover(id) {
        Ok(true) => {
            crate::log_debug(&format!(
                "ask_preempt: swept detached card {id} into a resume of {}",
                card.session_id
            ));
            let _ = fs::remove_file(&dst);
        }
        other => {
            if let Err(e) = other {
                crate::log_debug(&format!("ask_preempt: sweep of {id} failed: {e}"));
            }
            let _ = fs::rename(&dst, &src);
        }
    }
}

// ── File helpers ─────────────────────────────────────────────────────────────

fn write_json<T: Serialize>(path: &Path, value: &T) {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec(value) {
        let _ = crate::atomic_json::write_atomic(path, &json);
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(at_ms: u64, handled: bool) -> InboxMarker {
        InboxMarker {
            at_ms,
            sender: "user".into(),
            handled,
        }
    }

    #[test]
    fn marker_after_wait_start_always_preempts() {
        let m = marker(1_001, true);
        assert!(should_preempt(Some(&m), 1_000, |_| panic!("transcript must not be read")));
    }

    #[test]
    fn no_marker_never_preempts() {
        assert!(!should_preempt(None, 1_000, |_| true));
    }

    #[test]
    fn recent_marker_preempts_only_with_pending_queue() {
        let start = 100 * 60 * 1000;
        let m = marker(start - 5_000, false);
        assert!(should_preempt(Some(&m), start, |since| {
            assert_eq!(since, start - 5_000 - QUEUE_CLOCK_SLACK_MS);
            true
        }));
        assert!(!should_preempt(Some(&m), start, |_| false));
    }

    #[test]
    fn handled_or_stale_marker_does_not_use_the_transcript_clause() {
        let start = 100 * 60 * 1000;
        assert!(!should_preempt(Some(&marker(start - 5_000, true)), start, |_| true));
        assert!(!should_preempt(
            Some(&marker(start - RECENT_INBOX_MS - 1, false)),
            start,
            |_| true
        ));
    }

    fn op(op: &str, ts: &str) -> String {
        format!(
            r#"{{"type":"queue-operation","operation":"{op}","timestamp":"{ts}","sessionId":"s"}}"#
        )
    }

    fn ms(ts: &str) -> u64 {
        chrono::DateTime::parse_from_rfc3339(ts).unwrap().timestamp_millis() as u64
    }

    #[test]
    fn unmatched_enqueue_is_pending() {
        let t = [
            r#"{"type":"assistant","message":{}}"#.to_string(),
            op("enqueue", "2026-10-25T10:00:00.000Z"),
            r#"{"type":"user","message":"queue-operation mentioned in text"}"#.to_string(),
        ]
        .join("\n");
        assert!(cc_queue_pending(t.as_bytes(), ms("2026-10-25T09:59:59.000Z")));
    }

    #[test]
    fn removed_or_dequeued_message_is_not_pending() {
        for closing in ["remove", "dequeue"] {
            let t = [
                op("enqueue", "2026-10-25T10:00:00.000Z"),
                op(closing, "2026-10-25T10:00:03.000Z"),
            ]
            .join("\n");
            assert!(!cc_queue_pending(t.as_bytes(), ms("2026-10-25T09:59:59.000Z")), "{closing}");
        }
    }

    #[test]
    fn operations_before_since_are_ignored() {
        let t = [
            op("enqueue", "2026-10-25T09:00:00.000Z"),
            op("enqueue", "2026-10-25T10:00:00.000Z"),
            op("remove", "2026-10-25T10:00:01.000Z"),
        ]
        .join("\n");
        assert!(!cc_queue_pending(t.as_bytes(), ms("2026-10-25T09:59:00.000Z")));
        // A stray close with no enqueue in range must not go negative and
        // swallow a later enqueue.
        let t = [
            op("dequeue", "2026-10-25T10:00:00.000Z"),
            op("enqueue", "2026-10-25T10:00:01.000Z"),
        ]
        .join("\n");
        assert!(cc_queue_pending(t.as_bytes(), ms("2026-10-25T09:59:00.000Z")));
    }

    struct TempHome {
        dir: PathBuf,
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl TempHome {
        fn new(tag: &str) -> Self {
            let lock = crate::session::fleet_home_lock();
            let dir = std::env::temp_dir().join(format!(
                "fleet-ask-preempt-{tag}-{}-{}",
                std::process::id(),
                now_ms()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let prev = std::env::var_os("FLEET_HOME");
            // SAFETY: serialised by fleet_home_lock.
            unsafe { std::env::set_var("FLEET_HOME", &dir) };
            Self { dir, prev, _lock: lock }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("FLEET_HOME", v),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn write_request(id: &str, session_id: &str) {
        let req = crate::mcp_ipc::FleetAskRequest {
            id: id.into(),
            session_id: session_id.into(),
            workspace_name: String::new(),
            ai_title: None,
            task: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
            parked: false,
            task_complete: false,
            questions: Vec::new(),
            review_docs: Vec::new(),
        };
        crate::mcp_ipc::write_request(&req).unwrap();
    }

    #[test]
    fn inbox_marker_round_trips_and_handled_flag_sticks() {
        let _home = TempHome::new("inbox");
        assert!(read_inbox("s1").is_none());
        touch_inbox("s1", crate::pending_message::Sender::User);
        let m = read_inbox("s1").unwrap();
        assert_eq!(m.sender, "user");
        assert!(!m.handled);
        mark_inbox_handled("s1", m.at_ms + 1);
        assert!(!read_inbox("s1").unwrap().handled, "a different stamp is left alone");
        mark_inbox_handled("s1", m.at_ms);
        assert!(read_inbox("s1").unwrap().handled);
        // Ids that could escape the directory never touch the disk.
        touch_inbox("", crate::pending_message::Sender::Agent);
        assert!(read_inbox("").is_none());
    }

    #[test]
    fn preempter_fires_on_a_marker_stamped_during_the_wait() {
        let _home = TempHome::new("preempter");
        let mut p = Preempter::new("s1");
        assert!(!p.poll());
        std::thread::sleep(std::time::Duration::from_millis(5));
        touch_inbox("s1", crate::pending_message::Sender::Agent);
        assert!(p.poll());
    }

    #[test]
    fn detached_for_session_prunes_records_without_a_request() {
        let _home = TempHome::new("detached");
        write_request("r-live", "s1");
        mark_detached("r-live", "s1");
        mark_detached("r-gone", "s1");
        mark_detached("r-other", "s2");
        assert_eq!(detached_for_session("s1").as_deref(), Some("r-live"));
        assert!(!is_detached("r-gone"), "a record whose request is gone is pruned");
        assert!(detached_for_session("s3").is_none());
        // `mcp_ipc::cleanup` is the single close path and drops the record.
        crate::mcp_ipc::cleanup("r-live");
        assert!(!is_detached("r-live"));
        assert!(detached_for_session("s1").is_none());
    }

    #[test]
    fn sweep_waits_for_settle_and_a_dead_session_then_claims_once() {
        let _home = TempHome::new("sweep");
        write_request("r1", "s1");
        mark_detached("r1", "s1");
        let card = get_detached("r1").unwrap();
        // No response yet: nothing to do.
        sweep_one(&card, |_| false, |_| panic!("no response yet"));
        crate::mcp_ipc::write_response_file_for_test("r1", SWEEP_SETTLE_MS + 1_000);
        // Session still running: leave it to the live turn.
        sweep_one(&card, |_| true, |_| panic!("session is alive"));
        // Recovery declined: the record is restored for the next tick.
        sweep_one(&card, |_| false, |_| Ok(false));
        assert!(is_detached("r1"));
        let mut calls = 0;
        sweep_one(&card, |_| false, |_| {
            calls += 1;
            Ok(true)
        });
        assert_eq!(calls, 1);
        assert!(!is_detached("r1"));
    }

    #[test]
    fn sweep_leaves_a_fresh_response_to_the_write_time_recovery() {
        let _home = TempHome::new("sweep-fresh");
        write_request("r1", "s1");
        mark_detached("r1", "s1");
        let card = get_detached("r1").unwrap();
        crate::mcp_ipc::write_response_file_for_test("r1", 0);
        sweep_one(&card, |_| false, |_| panic!("response is too fresh"));
        assert!(is_detached("r1"));
    }
}
