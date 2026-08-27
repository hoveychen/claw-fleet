//! ACP over stdio — what a desktop editor spawns.
//!
//! This is ACP's primary transport: the client launches the agent as a
//! subprocess and speaks newline-delimited JSON-RPC over its stdin/stdout. Zed,
//! JetBrains, VS Code, Neovim, Emacs and Obsidian all connect this way, so it
//! is the path that makes Fleet reachable from an editor at all.
//!
//! Two modes:
//!
//! - [`serve_local`] runs the agent in-process against this machine's workspace.
//! - [`serve_proxy`] pipes the same stdio conversation to a **remote** Fleet
//!   over the `/acp` WebSocket. This is what lets an editor on a laptop drive an
//!   agent running in a Fleet Cloud container: the editor still spawns a local
//!   subprocess (all it knows how to do), and that subprocess is a pipe.
//!
//! The proxy deliberately does **not** parse ACP. Both sides speak the same
//! JSON-RPC, so it only has to translate framing — newline-delimited on stdio,
//! one text frame per message on WebSocket — and stay out of the way. That
//! keeps it correct for protocol features it has never heard of.

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::agent::AcpAgent;
use super::conn;
use super::jsonrpc::{Peer, Sink};

/// The subprotocol every ACP WebSocket connection negotiates.
pub const WS_SUBPROTOCOL: &str = "acp.v1";

/// Auth is folded into the subprotocol list as `bearer.<token>`.
///
/// Not a header, because the browser WebSocket API cannot set one — and a
/// client that can (a native app) still uses this form so servers only need to
/// implement one scheme. Established by the shipping `acp-ui` client.
pub const WS_BEARER_PREFIX: &str = "bearer.";

/// Give up on a stalled handshake rather than parking the editor forever.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Idle keep-alive cadence. 25s clears the shortest common idle window (60s:
/// nginx's `proxy_read_timeout` default, most home NAT evictions) even if one
/// tick is lost.
const HEARTBEAT: Duration = Duration::from_secs(25);

/// Writes frames to stdout, one per line.
///
/// The mutex is load-bearing, not defensive: the turn thread emits
/// `session/update` notifications while the reader thread writes replies, and
/// two interleaved `write_all`s would produce a corrupt line that the client
/// cannot parse.
struct StdoutSink(Mutex<std::io::Stdout>);

impl Sink for StdoutSink {
    fn send(&self, frame: &str) -> bool {
        let mut out = self.0.lock().unwrap();
        out.write_all(frame.as_bytes())
            .and_then(|_| out.write_all(b"\n"))
            .and_then(|_| out.flush())
            .is_ok()
    }
}

/// Serve ACP on stdin/stdout against the local workspace. Returns at EOF.
pub fn serve_local() -> std::io::Result<()> {
    let peer = Arc::new(Peer::new(Box::new(StdoutSink(Mutex::new(std::io::stdout())))));
    let sources = Arc::new(crate::agent_source::build_sources());
    let agent = Arc::new(AcpAgent::new(peer, sources));
    let stdin = std::io::stdin();
    conn::run_connection(agent, stdin.lock());
    Ok(())
}

/// Pipe stdio to a remote Fleet's `/acp` WebSocket.
///
/// `url` is the remote endpoint (`ws://` or `wss://`); `token` is presented as
/// the `bearer.<token>` subprotocol entry.
pub fn serve_proxy(url: &str, token: Option<&str>) -> Result<(), String> {
    // Its own current-thread runtime, matching how `mobile_relay` drives
    // tokio-tungstenite from otherwise-synchronous code.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;
    rt.block_on(proxy_loop(url, token))
}

async fn proxy_loop(url: &str, token: Option<&str>) -> Result<(), String> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let request = build_ws_request(url, token)?;
    let attempt = tokio_tungstenite::connect_async(request);
    let (ws, _) = match tokio::time::timeout(CONNECT_TIMEOUT, attempt).await {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => return Err(format!("connect {url}: {e}")),
        Err(_) => {
            return Err(format!("connect {url}: timed out after {}s", CONNECT_TIMEOUT.as_secs()))
        }
    };
    let (mut ws_tx, mut ws_rx) = ws.split();

    // stdin is blocking, so it gets a real thread and hands lines over a
    // channel; reading it on the async side would stall the whole runtime.
    let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::Builder::new()
        .name("acp-proxy-stdin".into())
        .spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                if stdin_tx.send(line).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| format!("stdin thread: {e}"))?;

    let mut stdout = std::io::stdout();
    let mut heartbeat = tokio::time::interval(HEARTBEAT);
    // The first tick fires immediately; skip it so we do not ping before the
    // client has said anything.
    heartbeat.tick().await;

    loop {
        tokio::select! {
            // Editor → cloud.
            line = stdin_rx.recv() => match line {
                Some(line) => {
                    if ws_tx.send(Message::Text(line.into())).await.is_err() {
                        return Err("remote closed while sending".to_string());
                    }
                }
                // The editor closed stdin: shut the socket down cleanly.
                None => {
                    let _ = ws_tx.send(Message::Close(None)).await;
                    return Ok(());
                }
            },
            // Cloud → editor.
            msg = ws_rx.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    // One frame per line, matching what the editor expects.
                    if writeln!(stdout, "{text}").and_then(|_| stdout.flush()).is_err() {
                        return Ok(()); // editor went away
                    }
                }
                // ACP is a text protocol; a binary frame is not ours to forward.
                Some(Ok(Message::Binary(_))) => {
                    return Err("remote sent a binary frame; ACP is text-only".to_string())
                }
                Some(Ok(Message::Close(_))) | None => return Ok(()),
                // Ping/Pong are handled by tungstenite itself.
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(format!("remote: {e}")),
            },
            // Keep the socket alive through NATs and reverse proxies.
            _ = heartbeat.tick() => {
                if ws_tx.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return Err("remote closed while pinging".to_string());
                }
            }
        }
    }
}

/// Build the WebSocket handshake request, carrying the ACP subprotocol and, if
/// present, the bearer token.
fn build_ws_request(
    url: &str,
    token: Option<&str>,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, String> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let mut req = url.into_client_request().map_err(|e| format!("bad url {url}: {e}"))?;
    let protocols = ws_subprotocol_header(token);
    req.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        protocols.parse().map_err(|_| "token is not header-safe".to_string())?,
    );
    Ok(req)
}

/// The `Sec-WebSocket-Protocol` value: the ACP subprotocol, plus the token
/// folded in as a second entry when there is one. Pure; unit-tested.
pub fn ws_subprotocol_header(token: Option<&str>) -> String {
    match token.filter(|t| !t.is_empty()) {
        Some(t) => format!("{WS_SUBPROTOCOL}, {WS_BEARER_PREFIX}{t}"),
        None => WS_SUBPROTOCOL.to_string(),
    }
}

/// Extract a bearer token from a client's `Sec-WebSocket-Protocol` value.
///
/// Lives here beside its producer so the two spellings cannot drift; the
/// `/acp` endpoint is the consumer. Pure; unit-tested.
pub fn token_from_subprotocols(header: &str) -> Option<String> {
    header
        .split(',')
        .map(str::trim)
        .find_map(|p| p.strip_prefix(WS_BEARER_PREFIX))
        .filter(|t| !t.is_empty())
        .map(String::from)
}

/// Whether the client offered the ACP subprotocol. Pure; unit-tested.
pub fn offers_acp_subprotocol(header: &str) -> bool {
    header.split(',').map(str::trim).any(|p| p == WS_SUBPROTOCOL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subprotocol_header_round_trips_a_token() {
        let h = ws_subprotocol_header(Some("secret123"));
        assert_eq!(h, "acp.v1, bearer.secret123");
        assert!(offers_acp_subprotocol(&h));
        assert_eq!(token_from_subprotocols(&h).as_deref(), Some("secret123"));
    }

    #[test]
    fn a_tokenless_client_offers_only_the_protocol() {
        let h = ws_subprotocol_header(None);
        assert_eq!(h, "acp.v1");
        assert!(offers_acp_subprotocol(&h));
        assert_eq!(token_from_subprotocols(&h), None);

        // An empty token is the same as none — otherwise we would advertise a
        // bare `bearer.` entry and the server would read an empty secret.
        assert_eq!(ws_subprotocol_header(Some("")), "acp.v1");
    }

    #[test]
    fn subprotocol_parsing_tolerates_spacing_and_ordering() {
        assert!(offers_acp_subprotocol("bearer.t,acp.v1"));
        assert_eq!(token_from_subprotocols("bearer.t,acp.v1").as_deref(), Some("t"));
        assert_eq!(token_from_subprotocols("acp.v1,   bearer.t  ").as_deref(), Some("t"));
    }

    #[test]
    fn a_client_without_the_acp_protocol_is_not_accepted() {
        // The endpoint uses this to reject a socket that is not speaking ACP.
        assert!(!offers_acp_subprotocol("chat, superchat"));
        assert!(!offers_acp_subprotocol(""));
        // A near-miss must not pass: subprotocol matching is exact.
        assert!(!offers_acp_subprotocol("acp.v2"));
        assert!(!offers_acp_subprotocol("acp"));
    }

    #[test]
    fn an_empty_bearer_entry_yields_no_token() {
        assert_eq!(token_from_subprotocols("acp.v1, bearer."), None);
    }

    #[test]
    fn ws_request_carries_the_subprotocol_header() {
        let req = build_ws_request("ws://example.test/acp", Some("tok")).unwrap();
        let got = req.headers().get("Sec-WebSocket-Protocol").unwrap().to_str().unwrap();
        assert_eq!(got, "acp.v1, bearer.tok");
    }

    #[test]
    fn a_malformed_url_is_reported_not_panicked() {
        assert!(build_ws_request("not a url", None).is_err());
    }
}
