//! Server-Sent Events broadcaster shared between the HTTP server and the
//! runtime loop. The runtime loop (P7+) calls `broadcast` whenever the plan
//! advances; subscribed `/events` clients receive the event as a JSON line.
//!
//! Mirrors the pattern used by `fleet serve`'s `SseBroadcaster`. We don't
//! reuse that one because it lives in `fleet-cli` and Phase 2 wants
//! `fleet-task` to stand on its own.

use std::io::Write;
use std::sync::{Arc, Mutex};

struct SseClient {
    stream: Box<dyn Write + Send>,
    alive: bool,
}

impl SseClient {
    fn send_event(&mut self, event_type: &str, data: &str) {
        let msg = format!("event: {event_type}\ndata: {data}\n\n");
        if self.stream.write_all(msg.as_bytes()).is_err() {
            self.alive = false;
        }
        let _ = self.stream.flush();
    }
}

#[derive(Clone, Default)]
pub struct SseBroadcaster {
    clients: Arc<Mutex<Vec<SseClient>>>,
}

impl SseBroadcaster {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_client(&self, stream: Box<dyn Write + Send>) {
        self.clients.lock().unwrap().push(SseClient {
            stream,
            alive: true,
        });
    }

    pub fn broadcast(&self, event_type: &str, data: &str) -> usize {
        let mut clients = self.clients.lock().unwrap();
        for c in clients.iter_mut() {
            c.send_event(event_type, data);
        }
        let before = clients.len();
        clients.retain(|c| c.alive);
        before - clients.len()
    }

    pub fn client_count(&self) -> usize {
        self.clients.lock().unwrap().len()
    }
}
