//! HTTP-level coverage of `fleet serve`'s wiki routes (plus the auth gate and
//! the `/tail` image-trimming contract), against a real in-process
//! `hooks_server::serve` on an ephemeral port.
//!
//! These used to live in the desktop crate as tests of its remote HTTP client;
//! that client is gone, but the routes are still what `fleet webui`, the cloud
//! container and the mobile relay depend on, and nothing else exercises their
//! request/response shapes over the wire. The handlers each define their own
//! request struct inline, so the JSON field names asserted here are the
//! contract — a renamed field is only caught at this layer.
//!
//! Requests are hand-rolled over `TcpStream` (no HTTP client dependency in
//! core); `Connection: close` keeps the parsing trivial.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::MutexGuard;
use std::time::Duration;

use claw_fleet_core::paths::fleet_home_lock;
use claw_fleet_core::routes;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};

const TOKEN: &str = "integration-test-token";

fn encode(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

struct Resp {
    status: u16,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!("non-JSON body (status {}): {e}\n{}", self.status, String::from_utf8_lossy(&self.body))
        })
    }

    /// The `error` field every 4xx wiki route answers with.
    fn error(&self) -> String {
        assert!(self.status >= 400, "expected an error status, got {}", self.status);
        self.json()["error"].as_str().unwrap_or("").to_string()
    }
}

fn request(port: u16, method: &str, path: &str, token: &str, body: Option<&str>) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream.set_read_timeout(Some(Duration::from_secs(30))).expect("set read timeout");

    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n"
    );
    if let Some(b) = body {
        req.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", b.len()));
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
        .expect("response has a header/body split");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let mut body = raw[split + 4..].to_vec();
    if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        body = dechunk(&body);
    }
    Resp { status, body }
}

fn dechunk(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let line_end = raw[i..].windows(2).position(|w| w == b"\r\n").map(|p| i + p);
        let Some(end) = line_end else { break };
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

/// A booted server plus everything that must outlive it.
struct Fixture {
    port: u16,
    home: tempfile::TempDir,
    /// Serialises every test that mutates the global `FLEET_HOME`.
    _guard: MutexGuard<'static, ()>,
}

impl Fixture {
    fn get(&self, path: &str) -> Resp {
        request(self.port, "GET", path, TOKEN, None)
    }

    fn post(&self, path: &str, body: Option<&Value>) -> Resp {
        let text = body.map(|b| b.to_string());
        request(self.port, "POST", path, TOKEN, text.as_deref())
    }

    /// POST that must succeed and answer JSON.
    fn post_json(&self, path: &str, body: &Value) -> Value {
        let r = self.post(path, Some(body));
        assert_eq!(r.status, 200, "POST {path} failed: {}", String::from_utf8_lossy(&r.body));
        r.json()
    }

    /// Absolute path of the sandboxed wiki root.
    fn wiki_root(&self) -> std::path::PathBuf {
        self.home.path().join(".fleet").join("wiki")
    }

    /// Publish a doc straight onto disk, bypassing HTTP.
    fn seed(&self, slug: &str) {
        self.seed_with(slug, &format!("# {slug}\n"));
    }

    /// Publishing the same slug twice with different bodies stacks versions.
    fn seed_with(&self, slug: &str, body: &str) {
        let src = self.home.path().join("seed.md");
        std::fs::write(&src, body).unwrap();
        claw_fleet_core::wiki::publish_in(&self.wiki_root(), &src, Some(slug), None, self.home.path())
            .unwrap();
    }

    fn slugs(&self) -> Vec<String> {
        let mut s: Vec<String> = claw_fleet_core::wiki::list_docs_in(&self.wiki_root())
            .into_iter()
            .map(|d| d.slug)
            .collect();
        s.sort();
        s
    }
}

/// Boots `hooks_server::serve` on an ephemeral port against an isolated
/// `FLEET_HOME`.
fn boot() -> Fixture {
    let guard = fleet_home_lock();
    let home = tempfile::TempDir::new().unwrap();
    // SAFETY: every test touching FLEET_HOME holds `guard`, so no other
    // thread reads the env while we swap it.
    unsafe { std::env::set_var("FLEET_HOME", home.path()) };

    let port_file = home.path().join("port");
    {
        let pf = port_file.clone();
        // `serve` never returns; the thread dies with the test process.
        std::thread::spawn(move || {
            claw_fleet_core::hooks_server::serve(claw_fleet_core::hooks_server::ServeOptions {
                port: 0,
                token: TOKEN.to_string(),
                port_file: Some(pf),
                ..Default::default()
            })
        });
    }

    // `serve` writes the OS-assigned port once it is listening.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let port = loop {
        assert!(std::time::Instant::now() < deadline, "serve never wrote its port file");
        if let Ok(text) = std::fs::read_to_string(&port_file) {
            if let Ok(p) = text.trim().parse::<u16>() {
                if p != 0 {
                    break p;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    Fixture { port, home, _guard: guard }
}

#[test]
fn health_answers_and_bad_token_is_rejected() {
    let fx = boot();

    let health = fx.get(routes::HEALTH);
    assert_eq!(health.status, 200);
    assert_eq!(health.json()["status"], "ok", "server is up");

    // The auth gate is the reason every other test's requests are trusted.
    let wrong = request(fx.port, "GET", routes::HEALTH, "not-the-token", None);
    assert_ne!(wrong.status, 200, "a bad bearer token must be rejected");
}

#[test]
fn live_tail_image_result_uses_transport_trimming_contract() {
    let fx = boot();
    let path = fx.home.path().join("live-image.jsonl");
    let tool_use_id = "toolu_live_image";
    let line = json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": [{
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": "A".repeat(8_192)
                    }
                }]
            }]
        }
    });
    std::fs::write(&path, format!("{line}\n")).unwrap();

    let endpoint = format!("{}?path={}&offset=0", routes::TAIL, encode(path.to_str().unwrap()));
    let resp = fx.get(&endpoint);
    assert_eq!(resp.status, 200);
    let body = resp.json();
    let message = &body["lines"][0];

    assert_eq!(message["_fleetTruncated"], true);
    assert_eq!(message["message"]["content"][0]["tool_use_id"], tool_use_id);
    let data = message["message"]["content"][0]["content"][0]["source"]["data"]
        .as_str()
        .expect("trimmed base64 preview");
    assert!(data.len() < 8_192);
    assert!(data.contains("Fleet truncated"));
}

// ── /wiki_move ──────────────────────────────────────────────────────────────

#[test]
fn wiki_move_rekeys_and_normalizes_the_target_slug() {
    let fx = boot();
    fx.seed("overview");

    let doc = fx.post_json(routes::WIKI_MOVE, &json!({ "from": "overview", "to": "Arch/Overview" }));

    // The server normalizes; the client gets the canonical slug back.
    assert_eq!(doc["slug"], "arch/overview");
    assert_eq!(fx.slugs(), vec!["arch/overview"]);
}

#[test]
fn wiki_move_onto_an_occupied_slug_is_a_400_with_a_message() {
    let fx = boot();
    fx.seed("a");
    fx.seed("b");

    let resp = fx.post(routes::WIKI_MOVE, Some(&json!({ "from": "a", "to": "b" })));
    assert_eq!(resp.status, 400);
    let err = resp.error();
    assert!(err.contains("already exists"), "unexpected error: {err}");
    assert_eq!(fx.slugs(), vec!["a", "b"], "the rejected move changed nothing");
}

// ── /wiki_delete ────────────────────────────────────────────────────────────

/// The slug travels in the query string, so a slug with directories has to
/// survive a percent-encode/decode round trip (`a/b` → `a%2Fb` → `a/b`).
#[test]
fn wiki_delete_removes_a_doc_whose_slug_has_directories() {
    let fx = boot();
    fx.seed("arch/deep/overview");
    fx.seed("keeper");

    let resp = fx.post(&format!("{}?slug={}", routes::WIKI_DELETE, encode("arch/deep/overview")), None);
    assert_eq!(resp.status, 200, "{}", String::from_utf8_lossy(&resp.body));

    assert_eq!(fx.slugs(), vec!["keeper"]);
}

#[test]
fn wiki_delete_drops_one_version_and_keeps_the_current() {
    let fx = boot();
    fx.seed_with("notes", "# v1\n");
    fx.seed_with("notes", "# v2\n");

    let doc = claw_fleet_core::wiki::get_doc_in(&fx.wiki_root(), "notes").unwrap();
    assert_eq!(doc.versions.len(), 2);
    let old = doc.versions.iter().find(|v| v.id != doc.current_version).unwrap().id.clone();

    let resp = fx.post(
        &format!("{}?slug=notes&version={}", routes::WIKI_DELETE, encode(&old)),
        None,
    );
    assert_eq!(resp.status, 200, "{}", String::from_utf8_lossy(&resp.body));

    let after = claw_fleet_core::wiki::get_doc_in(&fx.wiki_root(), "notes").unwrap();
    assert_eq!(after.versions.len(), 1);
    assert_eq!(after.current_version, doc.current_version, "current version survives");
}

// ── /wiki_move_folder ───────────────────────────────────────────────────────

#[test]
fn wiki_move_folder_rekeys_every_doc_beneath_and_returns_them() {
    let fx = boot();
    fx.seed("arch/one");
    fx.seed("arch/deep/two");
    fx.seed("arch"); // same name as the folder — renders beside it, must stay
    fx.seed("unrelated");

    let moved = fx.post_json(routes::WIKI_MOVE_FOLDER, &json!({ "from": "arch", "to": "design" }));

    assert_eq!(moved.as_array().map(Vec::len), Some(2), "server returns exactly the docs it moved");
    assert_eq!(fx.slugs(), vec!["arch", "design/deep/two", "design/one", "unrelated"]);
}

/// `to: ""` is how the UI dissolves a folder. It has to survive JSON — an
/// empty string must not be dropped or coerced on the way in.
#[test]
fn wiki_move_folder_with_empty_target_dissolves_into_the_root() {
    let fx = boot();
    fx.seed("arch/one");
    fx.seed("arch/two");

    let moved = fx.post_json(routes::WIKI_MOVE_FOLDER, &json!({ "from": "arch", "to": "" }));

    assert_eq!(moved.as_array().map(Vec::len), Some(2));
    assert_eq!(fx.slugs(), vec!["one", "two"]);
}

/// A collision anywhere under the prefix must abort the whole move, and the
/// 400 body must carry the reason.
#[test]
fn wiki_move_folder_collision_is_a_400_and_moves_nothing() {
    let fx = boot();
    fx.seed("a/x");
    fx.seed("a/y");
    fx.seed("b/y"); // b/y is taken, so a/y cannot land

    let resp = fx.post(routes::WIKI_MOVE_FOLDER, Some(&json!({ "from": "a", "to": "b" })));
    assert_eq!(resp.status, 400);
    let err = resp.error();
    assert!(err.contains("already exists"), "unexpected error: {err}");
    assert_eq!(fx.slugs(), vec!["a/x", "a/y", "b/y"], "nothing moved");
}

// ── /wiki_delete_folder ─────────────────────────────────────────────────────

/// The `{"deleted": n}` response shape is what every client deserializes — if
/// the server ever renamed that field, only this test catches it.
#[test]
fn wiki_delete_folder_removes_everything_beneath_and_reports_the_count() {
    let fx = boot();
    fx.seed("a/x");
    fx.seed("a/deep/y");
    fx.seed("a"); // beside the folder, must survive
    fx.seed("b/z");

    let resp = fx.post_json(routes::WIKI_DELETE_FOLDER, &json!({ "prefix": "a" }));

    assert_eq!(resp["deleted"], 2);
    assert_eq!(fx.slugs(), vec!["a", "b/z"]);
}

#[test]
fn wiki_delete_folder_on_an_empty_prefix_is_a_400() {
    let fx = boot();
    fx.seed("keeper");

    let resp = fx.post(routes::WIKI_DELETE_FOLDER, Some(&json!({ "prefix": "nosuch" })));
    assert_eq!(resp.status, 400);
    let err = resp.error();
    assert!(err.contains("no wiki docs under"), "unexpected error: {err}");
    assert_eq!(fx.slugs(), vec!["keeper"]);
}

// ── /wiki_publish_text ──────────────────────────────────────────────────────

/// The reader's "publish this message" over the wire: text goes up, the
/// server writes it, and a second call with `append` grows the same doc
/// instead of replacing it.
#[test]
fn wiki_publish_text_creates_then_appends_over_http() {
    let fx = boot();
    let ws = fx.home.path().display().to_string();
    let req = |text: &str, mode: &str| {
        json!({
            "slug": "notes/from-reader",
            "title": "",
            "text": text,
            "workspacePath": ws,
            "mode": mode,
        })
    };

    let created = fx.post_json(routes::WIKI_PUBLISH_TEXT, &req("# First\n\nentry one\n", "replace"));
    assert_eq!(created["slug"], "notes/from-reader");
    assert_eq!(created["kind"], "markdown");
    // Title was left empty on the wire, so the server derived it.
    assert_eq!(created["title"], "First");

    let appended = fx.post_json(routes::WIKI_PUBLISH_TEXT, &req("entry two\n", "append"));
    assert_eq!(appended["versions"].as_array().map(Vec::len), Some(2), "a note keeps its history");

    let body = claw_fleet_core::wiki::get_file_in(
        &fx.wiki_root(),
        "notes/from-reader",
        appended["currentVersion"].as_str().expect("currentVersion"),
        appended["entry"].as_str().expect("entry"),
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(body.bytes).unwrap(),
        "# First\n\nentry one\n\n---\n\nentry two\n"
    );
}

#[test]
fn wiki_publish_text_rejects_an_unusable_slug_with_a_400() {
    let fx = boot();
    let resp = fx.post(
        routes::WIKI_PUBLISH_TEXT,
        Some(&json!({ "slug": "汉字", "title": "", "text": "x", "workspacePath": "", "mode": "replace" })),
    );
    assert_eq!(resp.status, 400);
    let err = resp.error();
    assert!(err.contains("cannot derive a slug"), "unexpected error: {err}");
    assert!(fx.slugs().is_empty(), "nothing must be written");
}

/// A malformed body must be rejected by the route, not panic the server.
#[test]
fn wiki_move_folder_rejects_a_malformed_body() {
    let fx = boot();
    fx.seed("a/x");

    let resp = fx.post(routes::WIKI_MOVE_FOLDER, Some(&json!({ "nonsense": "x" })));
    assert_eq!(resp.status, 400);
    let err = resp.error();
    assert!(err.contains("bad /wiki_move_folder body"), "unexpected error: {err}");

    // The server is still alive and the wiki is untouched.
    let health = fx.get(routes::HEALTH);
    assert_eq!(health.json()["status"], "ok");
    assert_eq!(fx.slugs(), vec!["a/x"]);
}
