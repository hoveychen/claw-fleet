//! End-to-end coverage of the artifact store's HTTP surface on a real
//! `fleet serve`, with `/artifact_blob`'s ranged reads as the point.
//!
//! Range is the one thing the artifact routes do that no other Fleet route
//! does, and it is the whole reason the store exists as its own page: a
//! deliverable can be a 400 MB render, and the three older byte-serving
//! surfaces (`fleet-wiki://`, `fleet-decision://`, `fleet-attach://`) all read
//! the entire file into memory and answer `200`. A `<video>` seeking against
//! that has to buffer everything first.
//!
//! So these tests assert the wire contract a media element depends on —
//! `206`, an exact `Content-Range`, exactly the requested bytes, and `416`
//! past EOF — through the real binary rather than by calling the store
//! functions directly, because the parsing on both ends is where it would
//! silently drift.
//!
//! `FLEET_HOME` points at a per-test tempdir so nothing lands in the
//! developer's `~/.fleet`. Drives `fleet-cli`, the real binary
//! (`target/debug/fleet` is an unrelated 39-byte sidecar placeholder).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Size of the artifact under test. Bigger than any range asked for, small
/// enough to keep the test instant.
const BLOB_LEN: usize = 100_000;

/// Deterministic, position-dependent bytes: `body[i]` depends on `i`, so a
/// wrong offset can never accidentally match. Covers the full 0..=255 range,
/// which also keeps this honest about binary payloads rather than ASCII.
fn expected_body() -> Vec<u8> {
    (0..BLOB_LEN).map(|i| (i % 256) as u8).collect()
}

// ── Harness ──────────────────────────────────────────────────────────────────

fn unique_tempdir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "fleet-artifact-range-{}-{}-{}",
        label,
        std::process::id(),
        nanos
    ));
    std::fs::create_dir_all(&dir).expect("create tempdir");
    dir
}

struct ServeGuard {
    child: Child,
    stdout_log: PathBuf,
    stderr_log: PathBuf,
}

impl ServeGuard {
    fn logs(&self) -> String {
        format!(
            "  --- stdout ---\n{}\n  --- stderr ---\n{}",
            std::fs::read_to_string(&self.stdout_log).unwrap_or_default(),
            std::fs::read_to_string(&self.stderr_log).unwrap_or_default()
        )
    }
}

impl Drop for ServeGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = Command::new("kill")
                .arg("-TERM")
                .arg(self.child.id().to_string())
                .status();
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if matches!(self.child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_serve(fleet_home: &Path, port_file: &Path, token: &str) -> ServeGuard {
    let binary = env!("CARGO_BIN_EXE_fleet-cli");
    let stdout_log = fleet_home.join("serve.stdout.log");
    let stderr_log = fleet_home.join("serve.stderr.log");
    let stdout_file = std::fs::File::create(&stdout_log).expect("create stdout log");
    let stderr_file = std::fs::File::create(&stderr_log).expect("create stderr log");
    let child = Command::new(binary)
        .args([
            "serve",
            "--port",
            "0",
            "--token",
            token,
            "--port-file",
            port_file.to_str().unwrap(),
        ])
        .env("FLEET_HOME", fleet_home)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .expect("spawn fleet-cli serve");
    ServeGuard { child, stdout_log, stderr_log }
}

fn wait_for_port_file(path: &Path, timeout: Duration, serve: &mut ServeGuard) -> u16 {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(s) = std::fs::read_to_string(path) {
            if let Ok(n) = s.trim().parse::<u16>() {
                if n > 0 {
                    return n;
                }
            }
        }
        if Instant::now() >= deadline {
            let exit = match serve.child.try_wait() {
                Ok(Some(status)) => format!("exited {status:?}"),
                Ok(None) => "still running".to_string(),
                Err(e) => format!("try_wait err: {e}"),
            };
            panic!(
                "timed out waiting for port-file {}\n  child: {exit}\n{}",
                path.display(),
                serve.logs()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A raw HTTP response, kept as bytes because the body under test is binary.
struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Decode a `Transfer-Encoding: chunked` body.
///
/// tiny_http chunks responses above its buffer size, so the whole-blob case
/// arrives framed while the small ranged ones do not. Without this the test
/// would compare chunk headers against file bytes and read as a server bug —
/// it isn't one: reqwest (the RemoteBackend's client) and the webview both
/// de-chunk transparently, so nothing in production ever sees this framing.
fn dechunk(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let Some(eol) = raw[i..].windows(2).position(|w| w == b"\r\n") else {
            break;
        };
        let header = String::from_utf8_lossy(&raw[i..i + eol]).to_string();
        // A chunk header may carry `;ext=…` after the size.
        let size_hex = header.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_hex, 16) else {
            break;
        };
        i += eol + 2;
        if size == 0 {
            break;
        }
        if i + size > raw.len() {
            break;
        }
        out.extend_from_slice(&raw[i..i + size]);
        i += size + 2; // skip the chunk's trailing CRLF
    }
    out
}

/// One HTTP request over a fresh connection.
///
/// `Connection: close` so the body ends at EOF, plus [`dechunk`] for the
/// responses tiny_http frames — the point here is the headers and the bytes,
/// not reimplementing HTTP.
fn request(port: u16, method: &str, path: &str, token: &str, extra: &[(&str, &str)], body: Option<&str>) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("set read timeout");

    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n"
    );
    for (k, v) in extra {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
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
        .expect("response has a header/body separator");
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut body = raw[split + 4..].to_vec();

    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();

    if headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("transfer-encoding") && v.to_ascii_lowercase().contains("chunked")
    }) {
        body = dechunk(&body);
    }

    Resp { status, headers, body }
}

fn get(port: u16, path: &str, token: &str, extra: &[(&str, &str)]) -> Resp {
    request(port, "GET", path, token, extra, None)
}

fn post(port: u16, path: &str, token: &str, body: &str) -> Resp {
    request(port, "POST", path, token, &[], Some(body))
}

/// Bring up a serve with one artifact already in the store; returns its id.
fn serve_with_one_artifact(label: &str) -> (ServeGuard, u16, String, String) {
    let fleet_home = unique_tempdir(label);
    let workspace = fleet_home.join("workspace");
    std::fs::create_dir_all(&workspace).expect("create workspace");
    let src = workspace.join("render.mp4");
    std::fs::write(&src, expected_body()).expect("write source file");

    let token = "artifact-range-test-token";
    let port_file = fleet_home.join("port");
    let mut serve = spawn_serve(&fleet_home, &port_file, token);
    let port = wait_for_port_file(&port_file, Duration::from_secs(30), &mut serve);

    let body = serde_json::json!({
        "source_path": src.to_str().unwrap(),
        "title": "成片",
        "note": "给客户的最终版",
        "workspace_path": workspace.to_str().unwrap(),
    })
    .to_string();
    let added = post(port, "/artifact_add", token, &body);
    assert_eq!(
        added.status,
        200,
        "POST /artifact_add failed: {}\n{}",
        String::from_utf8_lossy(&added.body),
        serve.logs()
    );
    let json: serde_json::Value =
        serde_json::from_slice(&added.body).expect("artifact_add returns json");
    let id = json["id"].as_str().expect("added artifact has an id").to_string();

    (serve, port, token.to_string(), id)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn add_then_list_round_trips_through_the_probe() {
    let (serve, port, token, id) = serve_with_one_artifact("list");

    let listed = get(port, "/artifacts", &token, &[]);
    assert_eq!(listed.status, 200, "{}", serve.logs());
    let docs: serde_json::Value = serde_json::from_slice(&listed.body).unwrap();
    let docs = docs.as_array().expect("/artifacts returns an array");
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["id"].as_str(), Some(id.as_str()));
    assert_eq!(docs[0]["name"].as_str(), Some("render.mp4"));
    assert_eq!(docs[0]["title"].as_str(), Some("成片"));
    assert_eq!(docs[0]["note"].as_str(), Some("给客户的最终版"));
    // The kind drives the frontend's icon and preview choice, so it has to
    // survive the JSON round trip, not just exist in core.
    assert_eq!(docs[0]["kind"].as_str(), Some("video"));
    assert_eq!(docs[0]["mime"].as_str(), Some("video/mp4"));
    assert_eq!(docs[0]["sizeBytes"].as_u64(), Some(BLOB_LEN as u64));
}

#[test]
fn an_unranged_blob_request_is_a_plain_200_that_advertises_seeking() {
    let (serve, port, token, id) = serve_with_one_artifact("whole");

    let r = get(port, &format!("/artifact_blob?id={id}"), &token, &[]);
    assert_eq!(r.status, 200, "{}", serve.logs());
    assert_eq!(r.body, expected_body(), "the whole blob must come back intact");
    assert_eq!(r.header("Content-Type"), Some("video/mp4"));
    // Without this header a media element will not even try to seek.
    assert_eq!(r.header("Accept-Ranges"), Some("bytes"));
    assert!(r.header("Content-Range").is_none(), "a 200 must not claim a range");
}

#[test]
fn a_ranged_request_answers_206_with_exactly_those_bytes() {
    let (serve, port, token, id) = serve_with_one_artifact("ranged");
    let full = expected_body();

    let r = get(
        port,
        &format!("/artifact_blob?id={id}"),
        &token,
        &[("Range", "bytes=1000-1099")],
    );
    assert_eq!(r.status, 206, "a satisfiable range must be 206\n{}", serve.logs());
    assert_eq!(
        r.header("Content-Range"),
        Some(format!("bytes 1000-1099/{BLOB_LEN}").as_str()),
        "Content-Range must name the slice AND the full size"
    );
    assert_eq!(r.body.len(), 100);
    assert_eq!(r.body, full[1000..1100], "wrong offset would still be 100 bytes");
}

#[test]
fn an_open_ended_range_clamps_to_the_last_byte() {
    let (serve, port, token, id) = serve_with_one_artifact("openended");
    let full = expected_body();

    // `bytes=<start>-` is what a <video> sends to stream on from a seek point.
    let r = get(
        port,
        &format!("/artifact_blob?id={id}"),
        &token,
        &[("Range", "bytes=99990-")],
    );
    assert_eq!(r.status, 206, "{}", serve.logs());
    assert_eq!(
        r.header("Content-Range"),
        Some(format!("bytes 99990-{}/{BLOB_LEN}", BLOB_LEN - 1).as_str())
    );
    assert_eq!(r.body, full[99990..], "the tail must be the real tail");
}

#[test]
fn a_range_starting_past_the_end_is_416_not_404() {
    let (serve, port, token, id) = serve_with_one_artifact("past-eof");

    let r = get(
        port,
        &format!("/artifact_blob?id={id}"),
        &token,
        &[("Range", "bytes=200000-")],
    );
    // 416 tells the client to re-ask with a sane range; 404 would tell it the
    // artifact is gone and stop playback for good.
    assert_eq!(r.status, 416, "{}", serve.logs());
}

#[test]
fn an_unknown_id_is_404_even_with_a_range_header() {
    let (serve, port, token, _id) = serve_with_one_artifact("missing");

    let r = get(
        port,
        "/artifact_blob?id=20200101-000000",
        &token,
        &[("Range", "bytes=0-99")],
    );
    assert_eq!(r.status, 404, "a missing artifact is not a range problem\n{}", serve.logs());
}

#[test]
fn update_and_delete_reach_the_store_through_the_probe() {
    let (serve, port, token, id) = serve_with_one_artifact("mutate");

    let body = serde_json::json!({ "id": id, "starred": true }).to_string();
    let r = post(port, "/artifact_update", &token, &body);
    assert_eq!(r.status, 200, "{}", serve.logs());
    let updated: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(updated["starred"].as_bool(), Some(true));
    // An absent field means "leave it alone" — a starred-only patch must not
    // blank the title the add supplied.
    assert_eq!(updated["title"].as_str(), Some("成片"));

    let usage = get(port, "/artifact_usage", &token, &[]);
    let usage: serde_json::Value = serde_json::from_slice(&usage.body).unwrap();
    assert_eq!(usage["count"].as_u64(), Some(1));
    assert_eq!(usage["totalBytes"].as_u64(), Some(BLOB_LEN as u64));

    let body = serde_json::json!({ "id": id }).to_string();
    let r = post(port, "/artifact_delete", &token, &body);
    assert_eq!(r.status, 200, "{}", serve.logs());

    let listed = get(port, "/artifacts", &token, &[]);
    let docs: serde_json::Value = serde_json::from_slice(&listed.body).unwrap();
    assert!(docs.as_array().unwrap().is_empty(), "delete must actually remove it");
}
