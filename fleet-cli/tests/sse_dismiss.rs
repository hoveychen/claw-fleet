//! Integration tests: verify that `fleet serve` broadcasts `*-dismissed`
//! SSE events when a pending request file disappears (answered by another
//! client, timed out, or cleaned up by the respective CLI).
//!
//! This is the end-to-end regression test for the cross-client decision-panel
//! sync feature: without the dismiss broadcasts, mobile and desktop can each
//! answer the same decision independently because neither is notified that
//! the other's answer already landed.
//!
//! All three decision kinds share the same `HashSet::difference` diff pattern
//! in the fleet-cli SSE broadcaster loop, so we cover each with its own test
//! rather than trust one to imply the others.
//!
//! Tests isolate `~/.fleet/` by setting `FLEET_HOME` to a tempdir so they
//! never touch the real user's pending files.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ── Harness ────────────────────────────────────────────────────────────────

fn unique_tempdir(label: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "fleet-cli-test-{}-{}-{}",
        label,
        std::process::id(),
        nanos
    ));
    std::fs::create_dir_all(&dir).expect("create tempdir");
    dir
}

fn wait_for_port_file(path: &Path, timeout: Duration) -> u16 {
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
            panic!("timed out waiting for port-file {}", path.display());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

struct ServeGuard {
    child: Child,
}

impl Drop for ServeGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_fleet_serve(fleet_home: &Path, port_file: &Path, token: &str) -> ServeGuard {
    let binary = env!("CARGO_BIN_EXE_fleet-cli");
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
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn fleet-cli serve");
    ServeGuard { child }
}

fn connect_sse(port: u16, token: &str) -> TcpStream {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .expect("set read timeout");
    let req = format!(
        "GET /events?token={token} HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Accept: text/event-stream\r\n\
         Connection: keep-alive\r\n\
         \r\n"
    );
    stream.write_all(req.as_bytes()).expect("send http request");
    stream.flush().expect("flush");
    stream
}

/// Drain SSE frames looking for one whose `event:` line matches `event` and
/// whose parsed `data:` JSON satisfies `matcher`. Returns `true` if found
/// before the deadline.
fn wait_for_frame<F>(
    stream: &mut TcpStream,
    event: &str,
    timeout: Duration,
    mut matcher: F,
) -> bool
where
    F: FnMut(&str) -> bool,
{
    let deadline = Instant::now() + timeout;
    let mut buffer: Vec<u8> = Vec::new();
    let mut scratch = [0u8; 4096];
    while Instant::now() < deadline {
        match stream.read(&mut scratch) {
            Ok(0) => return false,
            Ok(n) => buffer.extend_from_slice(&scratch[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => panic!("read sse: {e}"),
        }
        let text = String::from_utf8_lossy(&buffer);
        let mut idx = 0;
        while let Some(end) = text[idx..].find("\n\n") {
            let frame = &text[idx..idx + end];
            idx += end + 2;
            let mut evt = "";
            let mut data = "";
            for line in frame.lines() {
                if let Some(rest) = line.strip_prefix("event: ") {
                    evt = rest;
                } else if let Some(rest) = line.strip_prefix("data: ") {
                    data = rest;
                }
            }
            if evt == event && matcher(data) {
                return true;
            }
        }
        if idx > 0 {
            buffer.drain(..idx);
        }
    }
    false
}

/// Asserts that after an SSE client connects and a fresh request file
/// appears-then-disappears in `dir_name` under `$FLEET_HOME/.fleet/`, the
/// server broadcasts `*-request` and then `*-dismissed` events. Parametrizes
/// the three decision kinds over a single shared flow.
fn run_dismiss_roundtrip(
    label: &str,
    dir_name: &str,
    request_event: &str,
    dismiss_event: &str,
    build_payload: impl FnOnce(&str) -> serde_json::Value,
) {
    let fleet_home = unique_tempdir(label);
    let target_dir = fleet_home.join(".fleet").join(dir_name);
    std::fs::create_dir_all(&target_dir).unwrap();

    let port_file = fleet_home.join("port");
    let token = "integration-test-token";

    let _serve = spawn_fleet_serve(&fleet_home, &port_file, token);
    let port = wait_for_port_file(&port_file, Duration::from_secs(10));

    let mut stream = connect_sse(port, token);
    let mut preamble = [0u8; 256];
    let _ = stream.read(&mut preamble); // HTTP headers + ": connected"

    // Let the broadcaster loop (2s tick) notice the new SSE client before
    // we seed the request file — otherwise it might see the file on the
    // same tick the client connects, which racily still works but is
    // harder to reason about.
    std::thread::sleep(Duration::from_millis(2_200));

    let test_id = format!("__fleet_cli_test_{}_{}__", label, std::process::id());
    let request_path = target_dir.join(format!("{test_id}.json"));
    let payload = build_payload(&test_id);
    std::fs::write(&request_path, payload.to_string()).unwrap();

    // Sanity: the `*-request` event fires, proving the SSE pipe is up.
    // Request payloads are full JSON objects with an `id` field.
    assert!(
        wait_for_frame(&mut stream, request_event, Duration::from_secs(5), |data| {
            serde_json::from_str::<serde_json::Value>(data)
                .ok()
                .and_then(|v| v.get("id").and_then(|x| x.as_str()).map(String::from))
                .as_deref()
                == Some(test_id.as_str())
        }),
        "did not receive {request_event} for {test_id}"
    );

    // Core assertion: removing the file emits `*-dismissed` carrying the id
    // as a bare JSON string.
    std::fs::remove_file(&request_path).unwrap();
    assert!(
        wait_for_frame(&mut stream, dismiss_event, Duration::from_secs(5), |data| {
            serde_json::from_str::<String>(data).ok().as_deref() == Some(test_id.as_str())
        }),
        "did not receive {dismiss_event} for {test_id}"
    );
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[test]
fn guard_dismissed_broadcasts_when_request_file_removed() {
    run_dismiss_roundtrip(
        "guard",
        "guard",
        "guard-request",
        "guard-dismissed",
        |id| {
            serde_json::json!({
                "id": id,
                "sessionId": "test-session",
                "workspaceName": "test-workspace",
                "aiTitle": null,
                "toolName": "Bash",
                "command": "echo integration-test",
                "commandSummary": "echo integration-test",
                "riskTags": [],
                "timestamp": "2024-01-01T00:00:00Z",
            })
        },
    );
}

#[test]
fn elicitation_dismissed_broadcasts_when_request_file_removed() {
    run_dismiss_roundtrip(
        "elicitation",
        "elicitation",
        "elicitation-request",
        "elicitation-dismissed",
        |id| {
            serde_json::json!({
                "id": id,
                "sessionId": "test-session",
                "workspaceName": "test-workspace",
                "aiTitle": null,
                "questions": [{
                    "question": "Pick one",
                    "header": "Pick",
                    "options": [
                        {"label": "A", "description": "Option A"},
                        {"label": "B", "description": "Option B"},
                    ],
                    "multiSelect": false,
                }],
                "timestamp": "2024-01-01T00:00:00Z",
            })
        },
    );
}

#[test]
fn plan_approval_dismissed_broadcasts_when_request_file_removed() {
    run_dismiss_roundtrip(
        "plan-approval",
        "plan-approval",
        "plan-approval-request",
        "plan-approval-dismissed",
        |id| {
            serde_json::json!({
                "id": id,
                "sessionId": "test-session",
                "workspaceName": "test-workspace",
                "aiTitle": null,
                "planContent": "# Plan\n\n- Step 1\n- Step 2\n",
                "planFilePath": null,
                "timestamp": "2024-01-01T00:00:00Z",
            })
        },
    );
}
