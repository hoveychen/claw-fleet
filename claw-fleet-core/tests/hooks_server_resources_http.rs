//! HTTP-level coverage of `fleet serve`'s `/resources*` routes against a real
//! in-process `hooks_server::serve`: the request bodies' field names, the
//! status codes (200 + table, 409 + error for a refused write, 400 for a bad
//! body) and that a write answers with the table as it stands afterwards.
//!
//! Requests go out as HTTP/1.0 over a raw `TcpStream`, so the response is never
//! chunked and the body is everything after the head.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use claw_fleet_core::paths::fleet_home_lock;
use claw_fleet_core::resource_queue::{self as rq, AcquireError, AcquireSpec, RequestKind};
use claw_fleet_core::routes;
use serde_json::{json, Value};

const TOKEN: &str = "resources-http-test-token";

fn call(port: u16, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let req = format!(
        "{method} {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {TOKEN}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("head/body split");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).expect("status");
    let value = serde_json::from_slice(&raw[split + 4..]).unwrap_or_else(|e| {
        panic!("non-JSON body ({e}): {}", String::from_utf8_lossy(&raw[split + 4..]))
    });
    (status, value)
}

fn spec(id: &str) -> AcquireSpec {
    AcquireSpec {
        id: id.into(),
        kind: RequestKind::Foreground,
        needs: rq::parse_needs(&["db"]).unwrap(),
        priority: Default::default(),
        max_wait: None,
        command: format!("cmd-{id}"),
        workspace: None,
        session_id: None,
    }
}

#[test]
fn resource_routes_read_adjust_and_refuse_over_http() {
    let _guard = fleet_home_lock();
    let home = tempfile::TempDir::new().unwrap();
    // SAFETY: every test touching FLEET_HOME holds the lock above.
    unsafe { std::env::set_var("FLEET_HOME", home.path()) };
    let port_file = home.path().join("port");
    {
        let pf = port_file.clone();
        std::thread::spawn(move || {
            claw_fleet_core::hooks_server::serve(claw_fleet_core::hooks_server::ServeOptions {
                port: 0,
                token: TOKEN.to_string(),
                port_file: Some(pf),
                ..Default::default()
            })
        });
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let port: u16 = loop {
        assert!(Instant::now() < deadline, "serve never wrote its port file");
        if let Some(p) = std::fs::read_to_string(&port_file).ok().and_then(|t| t.trim().parse().ok()) {
            if p != 0 {
                break p;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let (status, snap) = call(port, "GET", routes::RESOURCES, None);
    assert_eq!(status, 200);
    assert_eq!(snap["resources"], json!([]));

    let (status, snap) = call(port, "POST", routes::RESOURCES_CAPACITY, Some(&json!({"name": "db", "capacity": 2})));
    assert_eq!(status, 200, "{snap}");
    assert_eq!(snap["resources"][0]["capacity"], 2, "the write answers with the new table");
    let (status, _) = call(port, "POST", routes::RESOURCES_CAPACITY, Some(&json!({"name": "db", "capacity": 1})));
    assert_eq!(status, 200);

    // One holder (this process) and two waiters behind it.
    let holder = rq::acquire(spec("holder"), |_| false).unwrap();
    let waiters: Vec<_> = ["w1", "w2"]
        .into_iter()
        .map(|id| std::thread::spawn(move || rq::acquire(spec(id), |_| false).map(|g| g.release())))
        .collect();
    let deadline = Instant::now() + Duration::from_secs(10);
    let queue_ids = || -> Vec<String> {
        let (_, snap) = call(port, "GET", routes::RESOURCES, None);
        snap["queue"].as_array().unwrap().iter().map(|q| q["request"]["id"].as_str().unwrap().to_string()).collect()
    };
    while queue_ids().len() < 2 {
        assert!(Instant::now() < deadline, "the waiters never queued");
        std::thread::sleep(Duration::from_millis(20));
    }
    let last = queue_ids()[1].clone();

    // Move to the head with the wire shape of MoveTarget.
    let (status, snap) = call(port, "POST", routes::RESOURCES_MOVE, Some(&json!({"id": last, "target": {"index": 1}})));
    assert_eq!(status, 200, "{snap}");
    assert_eq!(snap["queue"][0]["request"]["id"], json!(last));
    assert_eq!(snap["queue"][0]["request"]["manual"], true);
    // db has no free unit, so both wait on its holder (not on each other).
    assert_eq!(snap["queue"][0]["waitOn"][0]["reason"], "busy");
    assert_eq!(snap["queue"][0]["waitOn"][0]["by"], json!(["holder"]));

    // Refusals are 409 with a message; a malformed body is 400.
    let (status, err) = call(port, "POST", routes::RESOURCES_KICK, Some(&json!({"id": last})));
    assert_eq!(status, 409);
    assert!(err["error"].as_str().unwrap().contains("cancel it instead"), "{err}");
    let (status, err) = call(port, "POST", routes::RESOURCES_REMOVE, Some(&json!({"name": "db"})));
    assert_eq!(status, 409);
    assert!(err["error"].as_str().unwrap().contains("only an idle resource"), "{err}");
    let (status, _) = call(port, "POST", routes::RESOURCES_CANCEL, Some(&json!({"nope": 1})));
    assert_eq!(status, 400);

    // Cancel both waiters over HTTP; each acquire ends as cancelled.
    for id in queue_ids() {
        let (status, _) = call(port, "POST", routes::RESOURCES_CANCEL, Some(&json!({"id": id})));
        assert_eq!(status, 200);
    }
    for w in waiters {
        assert_eq!(w.join().unwrap().unwrap_err(), AcquireError::Cancelled);
    }

    holder.release();
    let (status, snap) = call(port, "POST", routes::RESOURCES_REMOVE, Some(&json!({"name": "db"})));
    assert_eq!(status, 200, "{snap}");
    assert_eq!(snap["resources"], json!([]));

    // The history kept what the table forgot.
    let (status, h) = call(port, "POST", routes::RESOURCES_HISTORY, Some(&json!({"resource": "db", "limit": 500})));
    assert_eq!(status, 200, "{h}");
    let kinds: Vec<&str> = h["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
    for k in ["enqueued", "granted", "released", "cancelled", "capacity_set", "resource_removed"] {
        assert!(kinds.contains(&k), "{k} missing from {kinds:?}");
    }
    assert_eq!(kinds[0], "resource_removed", "newest first");
    let (status, h) = call(port, "POST", routes::RESOURCES_HISTORY, None);
    assert_eq!(status, 200, "an empty body reads the newest page: {h}");
    let (status, _) = call(port, "POST", routes::RESOURCES_HISTORY, Some(&json!({"limit": "x"})));
    assert_eq!(status, 400);
}
