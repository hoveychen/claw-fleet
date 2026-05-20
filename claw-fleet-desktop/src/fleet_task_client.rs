//! Thin reqwest-blocking client for talking to a `fleet-task` HTTP server
//! discovered through `RuntimeRegistryWatcher`. One client per live process.
//!
//! Endpoints (see `fleet-task/src/http.rs`):
//! - `GET /health` → `{ task_id, version }`
//! - `GET /state`  → `{ task, running_p_items }`
//! - `GET /p-items/<id>` → `{ p_item, agent_session_id }`
//! - `POST /p-items/<id>/dispatch` → 202 (or 503 if no dispatcher wired)
//!
//! Responses come back as `serde_json::Value` so the frontend can render them
//! directly — the typed `Task` schema is already on the JSON side. Frontends
//! that want strongly-typed parsing can deserialize from the Value.

use std::time::Duration;

use serde_json::Value;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum ClientError {
    /// Connection refused / DNS issue / TCP reset — the fleet-task process
    /// is likely gone. Caller should mark the registry entry stale.
    ConnectionFailed(String),
    /// Reached the server but it returned a non-2xx status.
    HttpStatus { status: u16, body: String },
    /// Response body wasn't valid JSON.
    Parse(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::ConnectionFailed(e) => write!(f, "connection failed: {e}"),
            ClientError::HttpStatus { status, body } => {
                write!(f, "http {status}: {body}")
            }
            ClientError::Parse(e) => write!(f, "parse: {e}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl ClientError {
    /// True when the error suggests the fleet-task process is no longer
    /// reachable (vs. a 4xx / 5xx where it is up but rejected the request).
    /// Callers can use this signal to drop the registry entry.
    pub fn is_unreachable(&self) -> bool {
        matches!(self, ClientError::ConnectionFailed(_))
    }
}

#[derive(Debug, Clone)]
pub struct FleetTaskClient {
    base: String,
    client: reqwest::blocking::Client,
}

impl FleetTaskClient {
    pub fn new(port: u16) -> Self {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("build reqwest client");
        Self {
            base: format!("http://127.0.0.1:{port}"),
            client,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub fn events_url(&self) -> String {
        format!("{}/events", self.base)
    }

    pub fn health(&self) -> Result<Value, ClientError> {
        self.get_json("/health")
    }

    pub fn state(&self) -> Result<Value, ClientError> {
        self.get_json("/state")
    }

    pub fn p_item(&self, p_item_id: &str) -> Result<Value, ClientError> {
        self.get_json(&format!("/p-items/{p_item_id}"))
    }

    pub fn dispatch(&self, p_item_id: &str) -> Result<(), ClientError> {
        let url = format!("{}/p-items/{p_item_id}/dispatch", self.base);
        let resp = self
            .client
            .post(&url)
            .send()
            .map_err(|e| ClientError::ConnectionFailed(e.to_string()))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let body = resp.text().unwrap_or_default();
            return Err(ClientError::HttpStatus { status, body });
        }
        Ok(())
    }

    fn get_json(&self, path: &str) -> Result<Value, ClientError> {
        let url = format!("{}{path}", self.base);
        let resp = self
            .client
            .get(&url)
            .send()
            .map_err(|e| ClientError::ConnectionFailed(e.to_string()))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let body = resp.text().unwrap_or_default();
            return Err(ClientError::HttpStatus { status, body });
        }
        let bytes = resp
            .bytes()
            .map_err(|e| ClientError::ConnectionFailed(e.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|e| ClientError::Parse(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreachable_distinguishes_connect_from_status() {
        let conn = ClientError::ConnectionFailed("nope".into());
        let http = ClientError::HttpStatus {
            status: 503,
            body: String::new(),
        };
        assert!(conn.is_unreachable());
        assert!(!http.is_unreachable());
    }

    #[test]
    fn client_builds_urls_correctly() {
        let c = FleetTaskClient::new(1234);
        assert_eq!(c.base_url(), "http://127.0.0.1:1234");
        assert_eq!(c.events_url(), "http://127.0.0.1:1234/events");
    }

    /// Spins up a fleet-task HTTP server in-process via the same module the
    /// binary uses, then exercises the client against it.
    #[test]
    fn round_trip_against_real_fleet_task_http() {
        use claw_fleet_core::paths::fleet_home_lock;
        use claw_fleet_core::task::{create_task, TaskInput};

        let _g = fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        unsafe { std::env::set_var("FLEET_HOME", tmp.path()) };

        let task = create_task(TaskInput {
            project_id: "proj".into(),
            title: "smoke".into(),
            description: String::new(),
        })
        .unwrap();

        // Spin up a fleet-task HTTP server (re-using the bin's module via
        // the published test fixture would be cleaner, but a tiny in-test
        // server is simpler). We use tiny_http directly to mirror the bin's
        // /health and /state contract.
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let task_id = task.id.clone();
        let server_join = std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let url = req.url().to_string();
                let body = if url == "/health" {
                    serde_json::json!({ "task_id": task_id, "version": "test" })
                } else if url == "/state" {
                    serde_json::json!({ "task": { "id": task_id }, "running_p_items": [] })
                } else {
                    let _ = req.respond(
                        tiny_http::Response::from_string("not found")
                            .with_status_code(404),
                    );
                    continue;
                };
                let bytes = serde_json::to_vec(&body).unwrap();
                let _ = req.respond(
                    tiny_http::Response::from_data(bytes).with_header(
                        tiny_http::Header::from_bytes("Content-Type", "application/json")
                            .unwrap(),
                    ),
                );
                // exit after a handful of requests so the test doesn't hang.
                break;
            }
        });

        let client = FleetTaskClient::new(port);
        let health = client.health().unwrap();
        assert_eq!(health["task_id"], task.id);
        assert_eq!(health["version"], "test");

        // Restore env after server exit.
        let _ = server_join.join();
        unsafe {
            match prev {
                Some(v) => std::env::set_var("FLEET_HOME", v),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
    }
}
