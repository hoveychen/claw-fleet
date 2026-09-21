//! HTTP-level coverage of `fleet serve`'s selection-explain routes
//! (`/session_explain`, `/session_explains`) against a real in-process
//! `hooks_server::serve` on an ephemeral port and an isolated `FLEET_HOME`.
//!
//! Only the read side and the request validation are exercised: a successful
//! `POST /session_explain` forks a real agent session and spends money, which
//! is the eyes-on check at the end of the plan, not a unit test. What this
//! pins is the wire contract the browser build and the mobile relay share —
//! query names, the 404 for an unknown record, the 400 with `{"error"}` for a
//! rejected body — since the desktop's `liveProxy.ts` mirrors it by hand.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::MutexGuard;
use std::time::Duration;

use claw_fleet_core::paths::fleet_home_lock;
use claw_fleet_core::routes;
use claw_fleet_core::session_explain::{ExplainPreset, ExplainRecord, ExplainStatus};
use serde_json::{json, Value};

const TOKEN: &str = "integration-test-token";

struct Resp {
    status: u16,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "non-JSON body (status {}): {e}\n{}",
                self.status,
                String::from_utf8_lossy(&self.body)
            )
        })
    }
}

fn request(port: u16, method: &str, path: &str, body: Option<&str>) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("set read timeout");
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {TOKEN}\r\nConnection: close\r\n"
    );
    if let Some(b) = body {
        req.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            b.len()
        ));
    }
    req.push_str("\r\n");
    if let Some(b) = body {
        req.push_str(b);
    }
    stream.write_all(req.as_bytes()).expect("write request");
    stream.flush().expect("flush");

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read response");
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("response has a header block");
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let chunked = head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked");
    let body = &raw[split + 4..];
    let body = if chunked {
        dechunk(body)
    } else {
        body.to_vec()
    };
    Resp { status, body }
}

fn dechunk(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let Some(end) = raw[i..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .map(|p| i + p)
        else {
            break;
        };
        let size_str = String::from_utf8_lossy(&raw[i..end]);
        let size = usize::from_str_radix(size_str.trim().split(';').next().unwrap_or("0"), 16)
            .unwrap_or(0);
        if size == 0 {
            break;
        }
        let start = end + 2;
        out.extend_from_slice(&raw[start..start + size]);
        i = start + size + 2;
    }
    out
}

struct Fixture {
    port: u16,
    home: tempfile::TempDir,
    _guard: MutexGuard<'static, ()>,
}

impl Fixture {
    fn get(&self, path: &str) -> Resp {
        request(self.port, "GET", path, None)
    }

    fn post(&self, path: &str, body: &Value) -> Resp {
        request(self.port, "POST", path, Some(&body.to_string()))
    }

    /// Drop a finished record straight into the sandboxed store, the way the
    /// worker thread would leave it.
    fn seed(&self, session_id: &str, id: &str, text: &str) {
        let rec = ExplainRecord {
            id: id.to_string(),
            session_id: session_id.to_string(),
            source: "claude-code".to_string(),
            created_ms: 1,
            updated_ms: 2,
            preset: ExplainPreset::Explain,
            quote: "the quoted passage".to_string(),
            question: "what does it mean?".to_string(),
            anchor: None,
            thread: Vec::new(),
            status: ExplainStatus::Done,
            text: text.to_string(),
            error: None,
            model: Some("claude-fable-5-1".to_string()),
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 30,
            cache_creation_tokens: 0,
            cost_usd: Some(0.01),
            duration_ms: 1234,
            fork_session_id: None,
            dismissed: false,
        };
        let dir = self
            .home
            .path()
            .join(".fleet")
            .join("explain")
            .join(session_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{id}.json")),
            serde_json::to_vec(&rec).unwrap(),
        )
        .unwrap();
    }
}

fn boot() -> Fixture {
    let guard = fleet_home_lock();
    let home = tempfile::TempDir::new().unwrap();
    // SAFETY: every test touching FLEET_HOME holds `guard`, so no other
    // thread reads the env while we swap it.
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
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let port = loop {
        assert!(
            std::time::Instant::now() < deadline,
            "serve never wrote its port file"
        );
        if let Ok(text) = std::fs::read_to_string(&port_file) {
            if let Ok(p) = text.trim().parse::<u16>() {
                if p != 0 {
                    break p;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Fixture {
        port,
        home,
        _guard: guard,
    }
}

#[test]
fn list_and_get_read_the_sandboxed_store() {
    let fx = boot();
    let empty = fx.get(&format!("{}?session_id=sess-1", routes::SESSION_EXPLAINS));
    assert_eq!(empty.status, 200);
    assert_eq!(empty.json(), json!([]));

    fx.seed("sess-1", "rec-a", "first answer");
    fx.seed("sess-1", "rec-b", "second answer");
    fx.seed("sess-2", "rec-c", "someone else's");

    let listed = fx.get(&format!("{}?session_id=sess-1", routes::SESSION_EXPLAINS));
    assert_eq!(listed.status, 200);
    let ids: Vec<String> = listed
        .json()
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids, ["rec-a", "rec-b"]);

    let one = fx.get(&format!(
        "{}?session_id=sess-1&id=rec-b",
        routes::SESSION_EXPLAIN
    ));
    assert_eq!(one.status, 200);
    let body = one.json();
    // camelCase on the wire — the shape `liveProxy.ts` and the phone consume.
    assert_eq!(body["sessionId"], "sess-1");
    assert_eq!(body["status"], "done");
    assert_eq!(body["text"], "second answer");
    assert_eq!(body["cacheReadTokens"], 30);
    assert_eq!(body["costUsd"], 0.01);
    assert_eq!(body["preset"], "explain");
}

#[test]
fn unknown_record_is_a_404_and_bad_asks_are_400() {
    let fx = boot();
    let missing = fx.get(&format!(
        "{}?session_id=sess-1&id=nope",
        routes::SESSION_EXPLAIN
    ));
    assert_eq!(missing.status, 404);
    assert_eq!(missing.json()["error"], "no such explanation");

    // A path-traversing id never reaches the filesystem.
    let evil = fx.get(&format!(
        "{}?session_id=..%2F..&id=x",
        routes::SESSION_EXPLAIN
    ));
    assert_eq!(evil.status, 404);

    let garbage = fx.post(routes::SESSION_EXPLAIN, &json!({"nope": true}));
    assert_eq!(garbage.status, 400);
    assert!(
        garbage.json()["error"]
            .as_str()
            .unwrap()
            .starts_with("bad request body"),
        "{}",
        garbage.json()
    );

    // Well-formed but empty selection: rejected before any source is asked.
    let blank = fx.post(
        routes::SESSION_EXPLAIN,
        &json!({
            "sessionId": "sess-1",
            "sessionPath": "/nowhere/sess-1.jsonl",
            "quote": "   ",
            "preset": "explain"
        }),
    );
    assert_eq!(blank.status, 400);
    assert_eq!(blank.json()["error"], "nothing selected");

    // Nothing was written by any of the rejected calls.
    assert!(!fx.home.path().join(".fleet").join("explain").exists());
}

/// The dismissal a reader presses ✕ for has to be readable back by every
/// client, which is what makes the card stay gone across a session switch.
#[test]
fn dismissal_rides_back_on_the_records_the_clients_read() {
    let fx = boot();
    fx.seed("sess-1", "rec-a", "first answer");
    fx.seed("sess-1", "rec-b", "second answer");

    let listed = fx.get(&format!("{}?session_id=sess-1", routes::SESSION_EXPLAINS));
    assert_eq!(listed.json()[0]["dismissed"], json!(false));

    let ok = fx.post(
        routes::SESSION_EXPLAIN_DISMISS,
        &json!({"sessionId": "sess-1", "id": "rec-a", "dismissed": true}),
    );
    assert_eq!(ok.status, 200);

    let listed = fx.get(&format!("{}?session_id=sess-1", routes::SESSION_EXPLAINS));
    let rows = listed.json();
    assert_eq!(rows.as_array().unwrap().len(), 2, "the record itself stays");
    assert_eq!(rows[0]["id"], json!("rec-a"));
    assert_eq!(rows[0]["dismissed"], json!(true));
    assert_eq!(rows[1]["dismissed"], json!(false));

    let one = fx.get(&format!(
        "{}?session_id=sess-1&id=rec-a",
        routes::SESSION_EXPLAIN
    ));
    assert_eq!(one.json()["dismissed"], json!(true));

    // Handing it back is the same call with the flag flipped.
    let back = fx.post(
        routes::SESSION_EXPLAIN_DISMISS,
        &json!({"sessionId": "sess-1", "id": "rec-a", "dismissed": false}),
    );
    assert_eq!(back.status, 200);
    let one = fx.get(&format!(
        "{}?session_id=sess-1&id=rec-a",
        routes::SESSION_EXPLAIN
    ));
    assert_eq!(one.json()["dismissed"], json!(false));

    let garbage = fx.post(routes::SESSION_EXPLAIN_DISMISS, &json!({"nope": true}));
    assert_eq!(garbage.status, 400);
}
