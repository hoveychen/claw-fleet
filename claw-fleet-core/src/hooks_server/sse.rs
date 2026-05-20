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
pub fn handle_sse_upgrade(request: Request, sse: &SseBroadcaster) {
    let response = Response::empty(200)
        .with_header(
            "Content-Type: text/event-stream"
                .parse::<Header>()
                .unwrap(),
        )
        .with_header("Cache-Control: no-cache".parse::<Header>().unwrap())
        .with_header("Connection: keep-alive".parse::<Header>().unwrap())
        .with_header(
            "Access-Control-Allow-Origin: *"
                .parse::<Header>()
                .unwrap(),
        );

    let mut stream = request.upgrade("sse", response);
    let _ = std::io::Write::write_all(&mut stream, b": connected\n\n");
    let _ = std::io::Write::flush(&mut stream);
    sse.add_client(Box::new(stream));
}
