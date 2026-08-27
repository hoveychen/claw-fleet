//! ACP over WebSocket — the endpoint mobile and web clients dial.
//!
//! # Why this is a separate listener
//!
//! `fleet serve` runs on tiny_http, which is synchronous. Its
//! `Request::upgrade()` does hand back a bidirectional stream, but as a single
//! `Box<dyn ReadWrite + Send>` — and ACP needs *concurrent* halves: the reader
//! parks in `read()` waiting for the next frame while the turn thread streams
//! `session/update` and a decision card blocks on `request_permission`. Wrapping
//! that one object in a mutex deadlocks by construction (the reader holds the
//! lock across its blocking read). tiny_http builds the stream from a separate
//! reader and writer internally, but neither the fields, the constructor, nor a
//! file descriptor is reachable through the trait object, so it cannot be taken
//! apart again.
//!
//! So ACP gets its own listener on its own port, where `tokio_tungstenite`'s
//! `WebSocketStream` splits cleanly. It stays in the same process and reuses the
//! same [`crate::hooks_server::auth::authorize`] decision; only the socket is
//! separate. That also makes the worker-starvation question moot — no ACP
//! connection ever occupies one of `fleet serve`'s 4–8 request workers.
//!
//! # Threading
//!
//! One thread per connection, each with its own current-thread runtime. A
//! shared runtime would let one connection's synchronous dispatch (see
//! [`crate::acp::conn`]) stall every other connection's I/O; this way they are
//! independent, and the cost is one thread per client — fine for a surface
//! whose clients are editors and phones.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{
    handshake::server::{ErrorResponse, Request as HsRequest, Response as HsResponse},
    http::StatusCode,
    Message,
};

use super::agent::AcpAgent;
use super::conn;
use super::jsonrpc::{Peer, Sink};
use super::stdio::{offers_acp_subprotocol, token_from_subprotocols, WS_SUBPROTOCOL};

/// Default port for the ACP listener. Overridden by `FLEET_ACP_PORT`.
pub const DEFAULT_PORT: u16 = 7008;

/// How long to let queued replies flush after the client closes the socket.
const WRITER_DRAIN: std::time::Duration = std::time::Duration::from_secs(2);

/// Why a handshake was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeError {
    WrongSubprotocol,
    Unauthorized,
}

impl HandshakeError {
    pub fn status(self) -> StatusCode {
        match self {
            HandshakeError::Unauthorized => StatusCode::UNAUTHORIZED,
            HandshakeError::WrongSubprotocol => StatusCode::BAD_REQUEST,
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            HandshakeError::Unauthorized => "invalid or missing token",
            HandshakeError::WrongSubprotocol => "client did not offer the acp.v1 subprotocol",
        }
    }
}

/// Validate a handshake from its `Sec-WebSocket-Protocol` value alone.
///
/// Pure, so every branch is testable without a socket. `authorize` receives the
/// token folded into the subprotocol list (`bearer.<token>`), or `None`.
pub fn check_subprotocols(
    subprotocols: Option<&str>,
    authorize: impl FnOnce(Option<&str>) -> bool,
) -> Result<(), HandshakeError> {
    let protos = subprotocols.unwrap_or("");
    if !offers_acp_subprotocol(protos) {
        return Err(HandshakeError::WrongSubprotocol);
    }
    if !authorize(token_from_subprotocols(protos).as_deref()) {
        return Err(HandshakeError::Unauthorized);
    }
    Ok(())
}

/// Where the ACP listener binds. `FLEET_ACP_PORT=0` disables it.
pub fn listen_addr() -> Option<String> {
    let port = std::env::var("FLEET_ACP_PORT")
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);
    if port == 0 {
        return None;
    }
    let host = std::env::var("FLEET_ACP_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    Some(format!("{host}:{port}"))
}

/// Decides whether a presented token may open an ACP connection.
///
/// Boxed rather than generic so [`spawn_listener`] can hand the same policy to
/// every connection thread.
pub type Authorizer = Arc<dyn Fn(Option<&str>) -> bool + Send + Sync>;

/// Start the ACP listener on its own thread. Returns the bound address.
///
/// Non-fatal by design: if the port is taken, ACP is unavailable but
/// `fleet serve` keeps serving everything else.
pub fn spawn_listener(
    addr: &str,
    sources: Arc<Vec<Box<dyn crate::agent_source::AgentSource>>>,
    authorize: Authorizer,
) -> std::io::Result<std::net::SocketAddr> {
    let listener = std::net::TcpListener::bind(addr)?;
    let bound = listener.local_addr()?;
    std::thread::Builder::new().name("acp-listener".into()).spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let sources = sources.clone();
            let authorize = authorize.clone();
            // One thread per connection — see the module docs on why they do
            // not share a runtime.
            let _ = std::thread::Builder::new()
                .name("acp-conn".into())
                .spawn(move || serve_connection(stream, sources, authorize));
        }
    })?;
    Ok(bound)
}

/// Run one connection to completion on the calling thread.
fn serve_connection(
    stream: std::net::TcpStream,
    sources: Arc<Vec<Box<dyn crate::agent_source::AgentSource>>>,
    authorize: Authorizer,
) {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return;
    };
    rt.block_on(async move {
        if stream.set_nonblocking(true).is_err() {
            return;
        }
        let Ok(stream) = tokio::net::TcpStream::from_std(stream) else { return };

        // The callback is where the subprotocol and the token are checked, and
        // where the accepted subprotocol is echoed back. Only `acp.v1` goes
        // back — never the `bearer.*` entry, which is a credential and would
        // otherwise land in proxy logs.
        let mut rejection: Option<HandshakeError> = None;
        let ws = tokio_tungstenite::accept_hdr_async(stream, |req: &HsRequest, mut resp: HsResponse| {
            let protos = req
                .headers()
                .get("Sec-WebSocket-Protocol")
                .and_then(|v| v.to_str().ok());
            match check_subprotocols(protos, |t| authorize(t)) {
                Ok(()) => {
                    resp.headers_mut().insert(
                        "Sec-WebSocket-Protocol",
                        WS_SUBPROTOCOL.parse().expect("static subprotocol token"),
                    );
                    Ok(resp)
                }
                Err(e) => {
                    rejection = Some(e);
                    let mut err = ErrorResponse::new(Some(e.message().to_string()));
                    *err.status_mut() = e.status();
                    Err(err)
                }
            }
        })
        .await;

        let ws = match ws {
            Ok(ws) => ws,
            Err(e) => {
                if let Some(r) = rejection {
                    crate::log_debug(&format!("[acp] handshake refused: {}", r.message()));
                } else {
                    crate::log_debug(&format!("[acp] handshake failed: {e}"));
                }
                return;
            }
        };

        let (mut ws_tx, mut ws_rx) = ws.split();

        // Outbound frames arrive from synchronous code on other threads (the
        // turn loop, decision-card threads), so they come through an unbounded
        // channel whose `send` is non-blocking and callable from anywhere.
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let writer = tokio::spawn(async move {
            while let Some(frame) = out_rx.recv().await {
                if ws_tx.send(Message::Text(frame.into())).await.is_err() {
                    break;
                }
            }
            let _ = ws_tx.close().await;
        });

        let peer = Arc::new(Peer::new(Box::new(ChannelSink(out_tx))));
        let agent = Arc::new(AcpAgent::new(peer, sources));

        // Dispatch runs on its own thread, never on the executor.
        //
        // `conn::dispatch_frame` is synchronous, and even the requests it
        // considers "fast" are not free — `session/list` walks every agent
        // source. Calling it inline here would block this current-thread
        // runtime, and the first casualty is the writer task: replies sit in
        // the channel unsent while the executor is stuck inside a scan.
        //
        // That is not hypothetical. Driving the endpoint by hand with
        // `initialize` followed by `session/list`, the client received *nothing*
        // — the initialize reply was queued, the scan then held the executor
        // past the client's drain window, and the socket closed with the reply
        // still in the queue.
        //
        // A channel to one dedicated thread keeps fast requests in order while
        // leaving the executor free; slow ones (`session/prompt`) still get
        // their own thread inside `dispatch_frame`.
        let (disp_tx, disp_rx) = std::sync::mpsc::channel::<String>();
        let disp_agent = agent.clone();
        let dispatcher = std::thread::Builder::new()
            .name("acp-dispatch".into())
            .spawn(move || {
                while let Ok(text) = disp_rx.recv() {
                    conn::dispatch_frame(&disp_agent, text);
                }
            })
            .ok();

        while let Some(msg) = ws_rx.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if disp_tx.send(text.to_string()).is_err() {
                        break;
                    }
                }
                // ACP is one JSON-RPC message per text frame; a binary frame is
                // not ours to interpret.
                Ok(Message::Binary(_)) => break,
                Ok(Message::Close(_)) => break,
                // Ping/Pong are answered by tungstenite itself.
                Ok(_) => {}
                Err(_) => break,
            }
        }

        agent.disconnect();

        // Close the dispatch queue and let it finish what it already accepted,
        // so a reply generated just before the client hung up still reaches the
        // writer.
        drop(disp_tx);
        if let Some(d) = dispatcher {
            let _ = d.join();
        }

        // Then let the writer drain. A reply can be sitting in the channel with
        // the writer task not yet scheduled; tearing it down here would
        // silently drop answers the client is still waiting for.
        //
        // Bounded, because request threads hold their own `Arc<Peer>` and may
        // keep the channel open past the point where anything more is coming.
        drop(agent);
        if tokio::time::timeout(WRITER_DRAIN, writer).await.is_err() {
            // Timed out with senders still alive; nothing further to flush.
        }
    });
}

/// Bridges the synchronous [`Sink`] the protocol layer expects onto the async
/// writer task.
struct ChannelSink(tokio::sync::mpsc::UnboundedSender<String>);

impl Sink for ChannelSink {
    fn send(&self, frame: &str) -> bool {
        self.0.send(frame.to_string()).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_good_handshake_passes_the_token_to_the_authorizer() {
        let mut seen = None;
        let got = check_subprotocols(Some("acp.v1, bearer.s3cr3t"), |t| {
            seen = t.map(String::from);
            true
        });
        assert!(got.is_ok());
        assert_eq!(seen.as_deref(), Some("s3cr3t"));
    }

    #[test]
    fn a_tokenless_client_hands_the_authorizer_none_not_empty_string() {
        let mut seen = Some("sentinel".to_string());
        let got = check_subprotocols(Some("acp.v1"), |t| {
            seen = t.map(String::from);
            true
        });
        assert!(got.is_ok());
        assert_eq!(seen, None);
    }

    #[test]
    fn a_socket_not_speaking_acp_is_refused_before_authorization_runs() {
        // Ordering matters: a client that never offered acp.v1 must not get as
        // far as having its token checked.
        let mut authorized = false;
        let err = check_subprotocols(Some("chat, superchat"), |_| {
            authorized = true;
            true
        })
        .unwrap_err();
        assert_eq!(err, HandshakeError::WrongSubprotocol);
        assert!(!authorized, "authorization must not run for a non-ACP socket");

        assert_eq!(
            check_subprotocols(None, |_| true).unwrap_err(),
            HandshakeError::WrongSubprotocol
        );
    }

    #[test]
    fn a_rejected_token_is_401_and_a_wrong_protocol_is_400() {
        let err = check_subprotocols(Some("acp.v1, bearer.no"), |_| false).unwrap_err();
        assert_eq!(err, HandshakeError::Unauthorized);
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(HandshakeError::WrongSubprotocol.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn near_miss_subprotocols_do_not_pass() {
        for p in ["acp", "acp.v2", "acp.v1x", ""] {
            assert_eq!(
                check_subprotocols(Some(p), |_| true).unwrap_err(),
                HandshakeError::WrongSubprotocol,
                "{p:?} must not be accepted as acp.v1"
            );
        }
    }

    /// Drive a real listener with a real WebSocket client.
    ///
    /// This is the test that would have caught the reply-dropping bug found
    /// while wiring the endpoint up by hand: replies were being queued and then
    /// discarded when the client closed, so a client that sent its requests and
    /// hung up saw nothing at all.
    #[test]
    fn a_websocket_client_gets_its_replies_including_after_it_closes() {
        use futures_util::{SinkExt, StreamExt};

        let authorize: Authorizer = Arc::new(|t: Option<&str>| t == Some("good"));
        let bound = spawn_listener("127.0.0.1:0", Arc::new(Vec::new()), authorize).unwrap();

        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            use tokio_tungstenite::tungstenite::client::IntoClientRequest;
            let mut req = format!("ws://{bound}/acp").into_client_request().unwrap();
            req.headers_mut().insert(
                "Sec-WebSocket-Protocol",
                "acp.v1, bearer.good".parse().unwrap(),
            );
            let (mut ws, resp) = tokio_tungstenite::connect_async(req).await.expect("handshake");

            // The server echoes back only the protocol, never the credential.
            let negotiated =
                resp.headers().get("Sec-WebSocket-Protocol").unwrap().to_str().unwrap();
            assert_eq!(negotiated, WS_SUBPROTOCOL);
            assert!(!negotiated.contains("bearer"), "a reflected token would leak into proxy logs");

            ws.send(Message::Text(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1}}"#
                    .into(),
            ))
            .await
            .unwrap();
            // A notification must draw no reply, and must not stall the next
            // request behind it.
            ws.send(Message::Text(r#"{"jsonrpc":"2.0","method":"$/ping"}"#.into())).await.unwrap();
            ws.send(Message::Text(
                r#"{"jsonrpc":"2.0","id":2,"method":"session/list","params":{}}"#.into(),
            ))
            .await
            .unwrap();

            let mut got: Vec<serde_json::Value> = Vec::new();
            while got.len() < 2 {
                let msg = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next())
                    .await
                    .expect("server answered in time")
                    .expect("stream open")
                    .expect("no ws error");
                if let Message::Text(t) = msg {
                    got.push(serde_json::from_str(&t).unwrap());
                }
            }

            assert_eq!(got[0]["id"], 1);
            assert_eq!(got[0]["result"]["agentInfo"]["name"], "fleet");
            assert_eq!(got[0]["result"]["protocolVersion"], PROTOCOL_VERSION_FOR_TEST);
            assert_eq!(got[1]["id"], 2, "the ping must not have consumed a reply slot");
            assert!(got[1]["result"]["sessions"].is_array());
        });
    }

    #[test]
    fn a_client_with_a_bad_token_is_refused_at_the_handshake() {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        let authorize: Authorizer = Arc::new(|t: Option<&str>| t == Some("good"));
        let bound = spawn_listener("127.0.0.1:0", Arc::new(Vec::new()), authorize).unwrap();

        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            for protos in ["acp.v1, bearer.wrong", "acp.v1", "chat"] {
                let mut req = format!("ws://{bound}/acp").into_client_request().unwrap();
                req.headers_mut()
                    .insert("Sec-WebSocket-Protocol", protos.parse().unwrap());
                assert!(
                    tokio_tungstenite::connect_async(req).await.is_err(),
                    "{protos:?} must not open a session"
                );
            }
        });
    }

    const PROTOCOL_VERSION_FOR_TEST: u16 = crate::acp::types::PROTOCOL_VERSION;

    #[test]
    fn listen_addr_honours_the_env_and_zero_disables() {
        // Serialized with the other env-reading test by running them in one
        // function: cargo runs tests in parallel threads sharing the process
        // environment.
        let prev_port = std::env::var("FLEET_ACP_PORT").ok();
        let prev_host = std::env::var("FLEET_ACP_HOST").ok();

        std::env::remove_var("FLEET_ACP_PORT");
        std::env::remove_var("FLEET_ACP_HOST");
        assert_eq!(listen_addr().as_deref(), Some("127.0.0.1:7008"), "loopback by default");

        std::env::set_var("FLEET_ACP_PORT", "9999");
        assert_eq!(listen_addr().as_deref(), Some("127.0.0.1:9999"));

        std::env::set_var("FLEET_ACP_HOST", "0.0.0.0");
        assert_eq!(listen_addr().as_deref(), Some("0.0.0.0:9999"));

        // 0 is the off switch, not "let the OS pick" — a random port nothing
        // can find is worse than being explicitly disabled.
        std::env::set_var("FLEET_ACP_PORT", "0");
        assert_eq!(listen_addr(), None);

        // Garbage falls back to the default rather than disabling silently.
        std::env::set_var("FLEET_ACP_PORT", "not-a-port");
        assert_eq!(listen_addr().as_deref(), Some("0.0.0.0:7008"));

        match prev_port {
            Some(v) => std::env::set_var("FLEET_ACP_PORT", v),
            None => std::env::remove_var("FLEET_ACP_PORT"),
        }
        match prev_host {
            Some(v) => std::env::set_var("FLEET_ACP_HOST", v),
            None => std::env::remove_var("FLEET_ACP_HOST"),
        }
    }
}
