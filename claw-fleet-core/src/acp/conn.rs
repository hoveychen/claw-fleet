//! The connection loop, shared by both transports.
//!
//! # Why requests get their own thread
//!
//! `session/prompt` blocks for the whole turn — minutes, in an agentic run.
//! Meanwhile three things must keep flowing on the same connection:
//!
//! - `session/cancel` from the client, which is the only way to stop that turn;
//! - the **responses** to requests the agent originated (`request_permission`,
//!   `elicitation/create`), which a decision card is parked waiting for;
//! - `$/ping` keep-alives, which stop the peer from tearing the socket down.
//!
//! Handling requests inline on the reader thread would block all three behind
//! the turn — and the second one deadlocks outright: the turn waits for a
//! permission answer that the reader can no longer read. So the reader only
//! ever *dispatches*, and anything slow runs elsewhere.
//!
//! Responses and notifications are handled on the reader thread on purpose:
//! both are O(1) (a map lookup, a flag) and routing a response is precisely
//! what unblocks a parked request.

use std::io::BufRead;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::agent::{handle_frame, AcpAgent};

/// How long to let in-flight requests finish after the peer stops sending.
///
/// A blocking request runs on its own thread, so returning the moment stdin
/// ends would drop whatever it was about to answer. Bounded, because a turn can
/// legitimately run for half an hour and a departed client is not worth waiting
/// that long for.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(30);

/// Drive one ACP connection to completion. Returns when the peer hangs up.
///
/// `lines` yields one JSON-RPC frame per item: newline-delimited for stdio,
/// one WebSocket text frame per item for `/acp`.
pub fn run_connection<R: BufRead>(agent: Arc<AcpAgent>, lines: R) {
    let inflight = Arc::new(AtomicUsize::new(0));
    for line in lines.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        super::stdio::trace("->", &line);
        dispatch_tracked(&agent, line, &inflight);
    }

    // Wait for the requests still running on their own threads.
    //
    // Without this, a client that sends a request and closes stdin gets
    // nothing: `session/load` replays on a spawned thread, and the loop used
    // to return — ending the process — before that thread wrote a single
    // update.
    let deadline = Instant::now() + SHUTDOWN_GRACE;
    while inflight.load(Ordering::Acquire) > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }

    // The peer is gone. Fail every parked request so decision-card threads
    // unwind now instead of sitting until their timeout.
    agent.disconnect();
}

/// Route one frame: slow work off-thread, fast work inline.
pub fn dispatch_frame(agent: &Arc<AcpAgent>, line: String) {
    dispatch_inner(agent, line, None)
}

/// [`dispatch_frame`], counting the request so [`run_connection`] can wait for
/// it before shutting down.
fn dispatch_tracked(agent: &Arc<AcpAgent>, line: String, inflight: &Arc<AtomicUsize>) {
    dispatch_inner(agent, line, Some(inflight))
}

fn dispatch_inner(agent: &Arc<AcpAgent>, line: String, inflight: Option<&Arc<AtomicUsize>>) {
    if !is_blocking_request(&line) {
        if let Some(reply) = handle_frame(agent.as_ref(), &line) {
            agent.send_frame(&reply);
        }
        return;
    }

    if let Some(c) = inflight {
        c.fetch_add(1, Ordering::AcqRel);
    }
    // The clones are what let the fallback below still run: a failed spawn
    // consumes the closure, so the originals have to stay behind.
    let owned_agent = agent.clone();
    let owned_line = line.clone();
    let owned_count = inflight.cloned();
    let spawned = std::thread::Builder::new().name("acp-request".into()).spawn(move || {
        if let Some(reply) = handle_frame(owned_agent.as_ref(), &owned_line) {
            owned_agent.send_frame(&reply);
        }
        if let Some(c) = owned_count {
            c.fetch_sub(1, Ordering::AcqRel);
        }
    });
    // A failed spawn must not silently drop the request — answer inline
    // instead. Slower, and it blocks the reader, but it never loses a frame.
    if spawned.is_err() {
        if let Some(reply) = handle_frame(agent.as_ref(), &line) {
            agent.send_frame(&reply);
        }
        if let Some(c) = inflight {
            c.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// Whether a frame may park for a long time and therefore must not run on the
/// reader thread.
///
/// Deliberately a cheap prefix test rather than a full parse: the reader is the
/// hot path, and being wrong in the conservative direction (spawning for
/// something quick) costs a thread, while being wrong the other way deadlocks
/// the connection. Only `session/prompt` blocks today; `session/load` joins it
/// because replaying a long transcript is not instant either.
fn is_blocking_request(line: &str) -> bool {
    line.contains(r#""session/prompt""#) || line.contains(r#""session/load""#)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::jsonrpc::{Peer, Sink};
    use serde_json::Value;
    use std::sync::Mutex;

    struct RecordingSink(Arc<Mutex<Vec<String>>>);
    impl Sink for RecordingSink {
        fn send(&self, frame: &str) -> bool {
            self.0.lock().unwrap().push(frame.to_string());
            true
        }
    }

    fn agent_with_log() -> (Arc<AcpAgent>, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let peer = Arc::new(Peer::new(Box::new(RecordingSink(log.clone()))));
        (Arc::new(AcpAgent::new(peer, Arc::new(Vec::new()))), log)
    }

    #[test]
    fn blocking_requests_are_recognised_by_method_name() {
        assert!(is_blocking_request(r#"{"id":1,"method":"session/prompt"}"#));
        assert!(is_blocking_request(r#"{"id":1,"method":"session/load"}"#));
        // Fast paths stay on the reader thread.
        assert!(!is_blocking_request(r#"{"id":1,"method":"initialize"}"#));
        assert!(!is_blocking_request(r#"{"method":"session/cancel"}"#));
        assert!(!is_blocking_request(r#"{"method":"$/ping"}"#));
        assert!(!is_blocking_request(r#"{"id":1,"result":{}}"#));
    }

    #[test]
    fn the_loop_answers_requests_and_stays_silent_on_notifications() {
        let (agent, log) = agent_with_log();
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"$/ping"}"#,
            "\n",
            "\n", // blank lines are skipped, not treated as parse errors
            r#"{"jsonrpc":"2.0","id":2,"method":"session/unknown","params":{}}"#,
            "\n",
        );
        run_connection(agent, std::io::Cursor::new(input));

        let frames = log.lock().unwrap();
        assert_eq!(frames.len(), 2, "one reply per request, nothing for the ping");
        let first: Value = serde_json::from_str(&frames[0]).unwrap();
        assert_eq!(first["id"], 1);
        assert_eq!(first["result"]["agentInfo"]["name"], "fleet");
        let second: Value = serde_json::from_str(&frames[1]).unwrap();
        assert_eq!(second["id"], 2);
        assert_eq!(second["error"]["code"], crate::acp::jsonrpc::codes::METHOD_NOT_FOUND);
    }

    #[test]
    fn a_blocking_request_is_answered_before_the_loop_returns() {
        // session/load runs on its own thread. Returning the moment stdin ends
        // used to end the process before that thread wrote anything, so a
        // client that sent a request and closed stdin got silence.
        let (agent, log) = agent_with_log();
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"session/load","params":{"sessionId":"nope","cwd":""}}"#,
            "\n",
        );
        run_connection(agent, std::io::Cursor::new(input));

        let frames = log.lock().unwrap();
        assert_eq!(frames.len(), 1, "the reply must land before the loop returns");
        let v: Value = serde_json::from_str(&frames[0]).unwrap();
        assert_eq!(v["id"], 1);
    }

    #[test]
    fn a_dropped_connection_unblocks_parked_requests() {
        // A decision card blocked on request_permission must not sit until its
        // timeout when the client is already gone.
        let (agent, _log) = agent_with_log();
        let peer = agent.peer_for_test();
        let waiter = {
            let peer = peer.clone();
            std::thread::spawn(move || {
                peer.request_blocking(
                    "session/request_permission",
                    serde_json::json!({}),
                    std::time::Duration::from_secs(30),
                )
            })
        };
        // Let the waiter register before the peer "hangs up".
        while peer.pending_count() == 0 {
            std::thread::yield_now();
        }
        run_connection(agent, std::io::Cursor::new(""));
        assert!(waiter.join().unwrap().is_err(), "the parked request is failed, not left hanging");
    }

    #[test]
    fn a_parse_error_is_answered_and_the_loop_keeps_going() {
        let (agent, log) = agent_with_log();
        let input = concat!(
            "{ broken\n",
            r#"{"jsonrpc":"2.0","id":5,"method":"initialize","params":{"protocolVersion":1}}"#,
            "\n",
        );
        run_connection(agent, std::io::Cursor::new(input));

        let frames = log.lock().unwrap();
        assert_eq!(frames.len(), 2, "the bad frame must not kill the connection");
        let bad: Value = serde_json::from_str(&frames[0]).unwrap();
        assert_eq!(bad["error"]["code"], crate::acp::jsonrpc::codes::PARSE_ERROR);
        let good: Value = serde_json::from_str(&frames[1]).unwrap();
        assert_eq!(good["id"], 5);
    }
}
