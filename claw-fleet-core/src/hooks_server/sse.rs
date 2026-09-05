//! Server-Sent Events broadcaster shared between the `fleet serve` /
//! `fleet-hooks-server` HTTP entry points and any background poller that
//! wants to push updates to connected clients.
//!
//! Moved out of `fleet-cli/src/main.rs` in Phase 4 P1a so the hooks server
//! can live as a reusable lib function (`claw_fleet_core::hooks_server::serve`)
//! that both binaries call instead of trampolining through `execvp`.

use std::io::Write;
use std::sync::{Arc, Mutex};

use tiny_http::{Header, Request, Response};

pub struct SseClient {
    pub stream: Box<dyn Write + Send>,
    pub alive: bool,
}

impl SseClient {
    pub fn send_event(&mut self, event_type: &str, data: &str) {
        let msg = format!("event: {event_type}\ndata: {data}\n\n");
        if self.stream.write_all(msg.as_bytes()).is_err() {
            self.alive = false;
        }
        let _ = self.stream.flush();
    }
}

#[derive(Clone)]
pub struct SseBroadcaster {
    clients: Arc<Mutex<Vec<SseClient>>>,
}

impl SseBroadcaster {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn add_client(&self, stream: Box<dyn Write + Send>) {
        self.clients.lock().unwrap().push(SseClient {
            stream,
            alive: true,
        });
    }

    pub fn broadcast(&self, event_type: &str, data: &str) {
        let mut clients = self.clients.lock().unwrap();
        for client in clients.iter_mut() {
            client.send_event(event_type, data);
        }
        clients.retain(|c| c.alive);
    }

    /// Write a keepalive comment frame to every client.
    ///
    /// `: ping\n\n` is an SSE comment: the spec has EventSource ignore it, so it
    /// dispatches no event and costs the page nothing. What it buys is bytes on
    /// the wire. Without them a deployment behind a reverse proxy silently loses
    /// idle streams — nginx's `proxy_read_timeout` defaults to 60s and this
    /// server only writes when something actually happens, so a quiet minute is
    /// indistinguishable from a hung upstream. The client then sees an error it
    /// may not be able to recover from (a 502 body makes EventSource *fail the
    /// connection* rather than retry), which is how decision cards stopped
    /// arriving on Boss's server until the page was reloaded.
    ///
    /// It doubles as liveness detection: a write to a client that has gone away
    /// fails, and the retain below drops it — otherwise a departed reader stays
    /// counted until the next real broadcast, keeping `client_count` (and with
    /// it the consumer heartbeat) lying about who is listening.
    pub fn ping(&self) {
        let mut clients = self.clients.lock().unwrap();
        for client in clients.iter_mut() {
            if client.stream.write_all(b": ping\n\n").is_err() {
                client.alive = false;
            }
            let _ = client.stream.flush();
        }
        clients.retain(|c| c.alive);
    }

    pub fn client_count(&self) -> usize {
        self.clients.lock().unwrap().len()
    }
}

impl Default for SseBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

/// Upgrade an incoming HTTP request to a long-lived SSE stream and register
/// it with the broadcaster.
/// `cors` 是这条部署该发的跨源头(见 hooks_server::cors)。空 vec = 不开放跨源,
/// 那时这条流只有同源页面能读 —— 同源本来就不需要 CORS 头。
pub fn handle_sse_upgrade(request: Request, sse: &SseBroadcaster, cors: Vec<Header>) {
    let mut response = Response::empty(200)
        .with_header(
            "Content-Type: text/event-stream"
                .parse::<Header>()
                .unwrap(),
        )
        .with_header("Cache-Control: no-cache".parse::<Header>().unwrap())
        .with_header("Connection: keep-alive".parse::<Header>().unwrap());
    for h in cors {
        response.add_header(h);
    }

    let mut stream = request.upgrade("sse", response);
    let _ = std::io::Write::write_all(&mut stream, b": connected\n\n");
    let _ = std::io::Write::flush(&mut stream);
    sse.add_client(Box::new(stream));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A writer the test can read back, and optionally make fail — stands in for
    /// a client socket that is still there / has gone away.
    #[derive(Clone)]
    struct Sink {
        buf: Arc<Mutex<Vec<u8>>>,
        broken: bool,
    }

    impl Write for Sink {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            if self.broken {
                return Err(std::io::Error::other("peer gone"));
            }
            self.buf.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    // Idle streams must still put bytes on the wire, or a reverse proxy reaps
    // them on its read timeout and the page loses its decision-card channel.
    #[test]
    fn ping_writes_a_comment_frame() {
        let sse = SseBroadcaster::new();
        let buf = Arc::new(Mutex::new(Vec::new()));
        sse.add_client(Box::new(Sink {
            buf: buf.clone(),
            broken: false,
        }));

        sse.ping();

        let written = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        // A comment frame: no `event:`/`data:`, so EventSource dispatches nothing.
        assert_eq!(written, ": ping\n\n");
    }

    #[test]
    fn ping_drops_clients_that_have_gone_away() {
        let sse = SseBroadcaster::new();
        sse.add_client(Box::new(Sink {
            buf: Arc::new(Mutex::new(Vec::new())),
            broken: true,
        }));
        assert_eq!(sse.client_count(), 1);

        sse.ping();

        assert_eq!(sse.client_count(), 0);
    }
}
