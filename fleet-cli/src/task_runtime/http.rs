//! Local HTTP server bound to `127.0.0.1` on an OS-assigned port. Endpoints:
//!
//! - `GET /health`               → `{ "task_id": ..., "version": ... }`
//! - `GET /state`                → `{ "task": <task json>, "running_p_items": [...] }`
//! - `GET /events`               → text/event-stream; subscribes to runtime events
//! - `GET /p-items/:id`          → `{ "p_item": <pitem json>, "agent_session_id": ... }`
//! - `POST /p-items/:id/dispatch`→ force `TaskRunner::step`; 503 if no dispatcher wired yet
//!
//! The server polls `recv_timeout(200ms)` on a dedicated `std::thread` so the
//! `HttpHandle` can break the loop on Drop / `shutdown()` within ~200ms.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde::Serialize;
use tiny_http::{Header, Method, Response, Server};

use crate::task_runtime::sse::SseBroadcaster;

/// Wired by Phase 3 P8: `fleet-task`'s runtime supplies a dispatcher that
/// routes incoming `POST /p-items/<id>/dispatch` to
/// `claw_fleet_task::actions::dispatch_pitem` with its own LocalHost so the
/// worker subprocess gets tracked by fleet-task's pid HashMap.
pub trait DispatchTrigger: Send + Sync {
    fn trigger(&self, p_item_id: &str) -> Result<(), String>;
}

#[derive(Clone)]
pub struct ServerConfig {
    pub task_id: String,
    pub broadcaster: SseBroadcaster,
    pub dispatcher: Option<Arc<dyn DispatchTrigger>>,
}

#[derive(Serialize)]
struct HealthBody<'a> {
    task_id: &'a str,
    version: &'a str,
}

#[derive(Serialize)]
struct StateBody {
    task: claw_fleet_task::task::Task,
    running_p_items: Vec<String>,
}

#[derive(Serialize)]
struct PItemBody {
    p_item: claw_fleet_task::pitem::PItem,
    agent_session_id: Option<String>,
}

pub struct HttpHandle {
    pub port: u16,
    stop: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl HttpHandle {
    pub fn shutdown(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for HttpHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

pub fn spawn(config: ServerConfig) -> std::io::Result<HttpHandle> {
    let server = Server::http("127.0.0.1:0")
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "no bound port"))?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let join = thread::Builder::new()
        .name("fleet-task-http".into())
        .spawn(move || loop {
            if stop_thread.load(Ordering::SeqCst) {
                return;
            }
            match server.recv_timeout(Duration::from_millis(200)) {
                Ok(Some(req)) => handle_request(&config, req),
                Ok(None) => continue,
                Err(_) => return,
            }
        })?;
    Ok(HttpHandle {
        port,
        stop,
        join: Some(join),
    })
}

fn json_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        .expect("valid header")
}

fn respond_json<T: Serialize>(req: tiny_http::Request, body: &T) {
    let bytes = match serde_json::to_vec(body) {
        Ok(b) => b,
        Err(e) => {
            let _ = req.respond(
                Response::from_string(format!("serialize: {e}")).with_status_code(500),
            );
            return;
        }
    };
    let resp = Response::from_data(bytes).with_header(json_header());
    let _ = req.respond(resp);
}

fn parse_p_item_path(url: &str) -> Option<(&str, bool)> {
    let rest = url.strip_prefix("/p-items/")?;
    let (id, action) = match rest.split_once('/') {
        Some((id, action)) => (id, Some(action)),
        None => (rest, None),
    };
    if id.is_empty() {
        return None;
    }
    match action {
        None => Some((id, false)),
        Some("dispatch") => Some((id, true)),
        _ => None,
    }
}

fn handle_request(cfg: &ServerConfig, req: tiny_http::Request) {
    let method = req.method().clone();
    let url = req.url().to_string();
    match (method, url.as_str()) {
        (Method::Get, "/health") => {
            let body = HealthBody {
                task_id: &cfg.task_id,
                version: env!("CARGO_PKG_VERSION"),
            };
            respond_json(req, &body);
        }
        (Method::Get, "/state") => {
            let task = match claw_fleet_task::task::get_task(&cfg.task_id) {
                Ok(t) => t,
                Err(e) => {
                    let _ = req.respond(
                        Response::from_string(format!("task not found: {e}"))
                            .with_status_code(404),
                    );
                    return;
                }
            };
            let running: Vec<String> = task
                .plan
                .items
                .values()
                .filter(|p| {
                    matches!(p.status, claw_fleet_task::pitem::PItemStatus::Running)
                })
                .map(|p| p.id.clone())
                .collect();
            let body = StateBody {
                task,
                running_p_items: running,
            };
            respond_json(req, &body);
        }
        (Method::Get, "/events") => {
            let resp = Response::empty(200)
                .with_header(
                    "Content-Type: text/event-stream"
                        .parse::<Header>()
                        .unwrap(),
                )
                .with_header("Cache-Control: no-cache".parse::<Header>().unwrap())
                .with_header("Connection: keep-alive".parse::<Header>().unwrap());
            let mut stream = req.upgrade("sse", resp);
            let _ = std::io::Write::write_all(&mut stream, b": connected\n\n");
            let _ = std::io::Write::flush(&mut stream);
            cfg.broadcaster.add_client(Box::new(stream));
        }
        (method, url) => {
            if let Some((p_id, is_dispatch)) = parse_p_item_path(url) {
                match (method.clone(), is_dispatch) {
                    (Method::Get, false) => {
                        let task = match claw_fleet_task::task::get_task(&cfg.task_id) {
                            Ok(t) => t,
                            Err(e) => {
                                let _ = req.respond(
                                    Response::from_string(format!("task not found: {e}"))
                                        .with_status_code(404),
                                );
                                return;
                            }
                        };
                        match task.plan.get(p_id) {
                            Some(p) => {
                                let body = PItemBody {
                                    p_item: p.clone(),
                                    agent_session_id: p.agent_session_id.clone(),
                                };
                                respond_json(req, &body);
                            }
                            None => {
                                let _ = req.respond(
                                    Response::from_string("p-item not found")
                                        .with_status_code(404),
                                );
                            }
                        }
                    }
                    (Method::Post, true) => match &cfg.dispatcher {
                        Some(d) => match d.trigger(p_id) {
                            Ok(()) => {
                                let _ = req.respond(
                                    Response::from_string("ok").with_status_code(202),
                                );
                            }
                            Err(e) => {
                                let _ = req.respond(
                                    Response::from_string(format!("dispatch: {e}"))
                                        .with_status_code(500),
                                );
                            }
                        },
                        None => {
                            let _ = req.respond(
                                Response::from_string(
                                    "runner not wired yet (Phase 2 P7)",
                                )
                                .with_status_code(503),
                            );
                        }
                    },
                    _ => {
                        let _ = req.respond(
                            Response::from_string("method not allowed")
                                .with_status_code(405),
                        );
                    }
                }
            } else {
                let _ = req.respond(
                    Response::from_string("not found").with_status_code(404),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claw_fleet_task::paths::fleet_home_lock;
    use claw_fleet_task::pitem::{PItem, PItemStatus};
    use claw_fleet_task::plan::DagPlan;
    use claw_fleet_task::task::{create_task, write_task_atomic, TaskInput};

    struct FleetHomeOverride {
        prev: Option<std::ffi::OsString>,
    }
    impl FleetHomeOverride {
        fn new(tmp: &std::path::Path) -> Self {
            let prev = std::env::var_os("FLEET_HOME");
            unsafe { std::env::set_var("FLEET_HOME", tmp) };
            FleetHomeOverride { prev }
        }
    }
    impl Drop for FleetHomeOverride {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(p) => std::env::set_var("FLEET_HOME", p),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
        }
    }

    fn http_get(port: u16, path: &str) -> (u16, String) {
        let url = format!("http://127.0.0.1:{port}{path}");
        match ureq::get(&url).call() {
            Ok(r) => {
                let code = r.status();
                (code, r.into_string().unwrap_or_default())
            }
            Err(ureq::Error::Status(code, r)) => {
                (code, r.into_string().unwrap_or_default())
            }
            Err(e) => panic!("http GET {url} failed: {e}"),
        }
    }

    fn http_post(port: u16, path: &str) -> (u16, String) {
        let url = format!("http://127.0.0.1:{port}{path}");
        match ureq::post(&url).call() {
            Ok(r) => {
                let code = r.status();
                (code, r.into_string().unwrap_or_default())
            }
            Err(ureq::Error::Status(code, r)) => {
                (code, r.into_string().unwrap_or_default())
            }
            Err(e) => panic!("http POST {url} failed: {e}"),
        }
    }

    fn cfg_for(task_id: &str) -> ServerConfig {
        ServerConfig {
            task_id: task_id.into(),
            broadcaster: SseBroadcaster::new(),
            dispatcher: None,
        }
    }

    fn pitem(id: &str, deps: &[&str]) -> PItem {
        PItem {
            id: id.into(),
            desc: format!("do {id}"),
            touches: vec![],
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            acceptance: vec![],
            human_gate: false,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
            failure_gaps: Vec::new(),
        }
    }

    #[test]
    fn health_endpoint_returns_task_id_and_version() {
        let _g = fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        let handle = spawn(cfg_for("task-x")).unwrap();
        let (code, body) = http_get(handle.port, "/health");
        assert_eq!(code, 200);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["task_id"], "task-x");
        assert_eq!(parsed["version"], env!("CARGO_PKG_VERSION"));
        handle.shutdown();
    }

    #[test]
    fn state_endpoint_returns_task_and_running_ids() {
        let _g = fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        let task = create_task(TaskInput {
            model: None,
            project_id: "proj".into(),
            title: "smoke".into(),
            description: String::new(),
        })
        .unwrap();

        let handle = spawn(cfg_for(&task.id)).unwrap();
        let (code, body) = http_get(handle.port, "/state");
        assert_eq!(code, 200);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["task"]["id"], task.id);
        assert!(parsed["running_p_items"].is_array());
        handle.shutdown();
    }

    #[test]
    fn state_endpoint_404_for_unknown_task() {
        let _g = fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        let handle = spawn(cfg_for("missing")).unwrap();
        let (code, _) = http_get(handle.port, "/state");
        assert_eq!(code, 404);
        handle.shutdown();
    }

    #[test]
    fn unknown_path_returns_404() {
        let _g = fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        let handle = spawn(cfg_for("t")).unwrap();
        let (code, _) = http_get(handle.port, "/nope");
        assert_eq!(code, 404);
        handle.shutdown();
    }

    #[test]
    fn p_item_get_returns_p_item_detail() {
        let _g = fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        let mut task = create_task(TaskInput {
            model: None,
            project_id: "proj".into(),
            title: "smoke".into(),
            description: String::new(),
        })
        .unwrap();
        let mut p = pitem("p1", &[]);
        p.agent_session_id = Some("worker-abc".into());
        task.plan = DagPlan::from_items(vec![p]);
        write_task_atomic(&task).unwrap();

        let handle = spawn(cfg_for(&task.id)).unwrap();
        let (code, body) = http_get(handle.port, "/p-items/p1");
        assert_eq!(code, 200);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["p_item"]["id"], "p1");
        assert_eq!(parsed["agent_session_id"], "worker-abc");
        handle.shutdown();
    }

    #[test]
    fn p_item_get_404_for_missing_p_id() {
        let _g = fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        let task = create_task(TaskInput {
            model: None,
            project_id: "proj".into(),
            title: "smoke".into(),
            description: String::new(),
        })
        .unwrap();
        let handle = spawn(cfg_for(&task.id)).unwrap();
        let (code, _) = http_get(handle.port, "/p-items/nope");
        assert_eq!(code, 404);
        handle.shutdown();
    }

    #[test]
    fn dispatch_returns_503_when_dispatcher_missing() {
        let _g = fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        let handle = spawn(cfg_for("t")).unwrap();
        let (code, _) = http_post(handle.port, "/p-items/p1/dispatch");
        assert_eq!(code, 503);
        handle.shutdown();
    }

    #[test]
    fn dispatch_invokes_trigger_when_wired() {
        use std::sync::atomic::{AtomicU32, Ordering as AOrdering};

        struct CountTrigger(Arc<AtomicU32>);
        impl DispatchTrigger for CountTrigger {
            fn trigger(&self, _p_item_id: &str) -> Result<(), String> {
                self.0.fetch_add(1, AOrdering::SeqCst);
                Ok(())
            }
        }

        let _g = fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        let counter = Arc::new(AtomicU32::new(0));
        let cfg = ServerConfig {
            task_id: "t".into(),
            broadcaster: SseBroadcaster::new(),
            dispatcher: Some(Arc::new(CountTrigger(counter.clone()))),
        };
        let handle = spawn(cfg).unwrap();
        let (code, _) = http_post(handle.port, "/p-items/p1/dispatch");
        assert_eq!(code, 202);
        assert_eq!(counter.load(AOrdering::SeqCst), 1);
        handle.shutdown();
    }

    #[test]
    fn events_endpoint_streams_broadcast_messages() {
        use std::io::{BufRead, BufReader};

        let _g = fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(tmp.path());

        let cfg = cfg_for("t");
        let broadcaster = cfg.broadcaster.clone();
        let handle = spawn(cfg).unwrap();

        // Open a raw TCP connection so we can leave the body open for reads.
        let stream = std::net::TcpStream::connect(("127.0.0.1", handle.port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let mut stream2 = stream.try_clone().unwrap();
        std::io::Write::write_all(
            &mut stream2,
            b"GET /events HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: keep-alive\r\n\r\n",
        )
        .unwrap();

        // Wait until the broadcaster picked up the client (the upgrade is async
        // to our test thread).
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while broadcaster.client_count() == 0 {
            if std::time::Instant::now() > deadline {
                panic!("broadcaster never registered the upgraded client");
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        broadcaster.broadcast("p_item.spawned", "{\"id\":\"p1\"}");

        let mut reader = BufReader::new(stream);
        let mut saw_event = false;
        let mut saw_data = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if line.starts_with("event: p_item.spawned") {
                        saw_event = true;
                    }
                    if line.starts_with("data: {\"id\":\"p1\"}") {
                        saw_data = true;
                    }
                    if saw_event && saw_data {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        assert!(saw_event, "did not see event header");
        assert!(saw_data, "did not see data line");
        handle.shutdown();
    }
}
