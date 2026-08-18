//! One slow request must not hold up every other request `fleet serve` has.
//!
//! `serve()` drives `for request in server.incoming_requests()` on the main
//! thread, so the whole HTTP surface — 155 routes — is one queue. A good number
//! of those routes scan every agent source synchronously (`/sessions`,
//! `/today_usage`, `/guard_pending`, …), and a scan is only as fast as the
//! slowest source's RPC. When dsh answers `session.list` in seconds, everything
//! queued behind that scan waits it out, including requests that touch no state
//! at all: measured through a real `fleet serve` with a 5s dsh, `/health` — a
//! handler that formats one const string — took 26–29s.
//!
//! `/health` is the probe precisely because it reads nothing: any latency it
//! shows is queueing in the request loop, not work. The slow side is the dsh
//! fixture (`claw-fleet-core/tests/fixtures/fake-dsh.js`), an ordinary
//! concurrent HTTP server, so it cannot be the thing serializing the two.
//!
//! The test is written to fail loudly rather than vacuously: it asserts the
//! scan request really was slow and really was in flight across the whole
//! `/health` probe, so "nothing was slow" can never read as "not blocked".

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The fixture's `session.list` latency — the slow scan every queued request
/// would have to wait out.
const LIST_DELAY_MS: u64 = 5000;
/// The fixture's `session.history` latency; irrelevant here, kept small.
const HISTORY_DELAY_MS: u64 = 100;

/// What `/health` is allowed to take while a scan is in flight.
///
/// The handler formats one const string, and an idle round trip measures single
/// -digit milliseconds. A whole second is generous room for process noise, and
/// still five times below the 5s scan a queued probe would inherit — so this
/// can only fail by queueing, not by the machine being slow.
const HEALTH_BUDGET: Duration = Duration::from_millis(1000);

// ── Harness ────────────────────────────────────────────────────────────────

fn unique_tempdir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "fleet-serve-concurrency-{}-{}-{}",
        label,
        std::process::id(),
        nanos
    ));
    std::fs::create_dir_all(&dir).expect("create tempdir");
    dir
}

/// The slow-dsh fixture, which lives in the core crate's tests.
fn dsh_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .join("claw-fleet-core")
        .join("tests")
        .join("fixtures")
        .join("fake-dsh.js")
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
        // SIGTERM, not SIGKILL: serve's termination handler is what stops the
        // `dsh web` it started (here the node fixture). SIGKILL would leave the
        // fixture running with nothing left to reap it — this test's FLEET_HOME
        // is a fresh tempdir, so no later Fleet start sweeps this registry.
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

/// Start a real `fleet serve` whose only dsh is the slow fixture.
fn spawn_slow_dsh_serve(fleet_home: &Path, port_file: &Path, token: &str) -> ServeGuard {
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
        .env("FLEET_DSH_BIN", dsh_fixture())
        .env("FAKE_DSH_LIST_DELAY_MS", LIST_DELAY_MS.to_string())
        .env("FAKE_DSH_HISTORY_DELAY_MS", HISTORY_DELAY_MS.to_string())
        .env("FAKE_DSH_SESSION_CWD", fleet_home)
        .env("FAKE_DSH_LOG", fleet_home.join("fake-dsh.log"))
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .expect("spawn fleet-cli serve");
    ServeGuard {
        child,
        stdout_log,
        stderr_log,
    }
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
                Ok(Some(status)) => format!("exited {:?}", status),
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

/// One plain HTTP GET, timed end to end, body returned.
///
/// `Connection: close` so the body ends at EOF and no framing parser is needed.
/// The read timeout is the ceiling on a single blocked read, not on the request.
fn timed_get(port: u16, path: &str, token: &str) -> (Duration, String) {
    let started = Instant::now();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(90)))
        .expect("set read timeout");
    let req = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Authorization: Bearer {token}\r\n\
         Connection: close\r\n\
         \r\n"
    );
    stream.write_all(req.as_bytes()).expect("send request");
    stream.flush().expect("flush");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read response");
    let elapsed = started.elapsed();
    (elapsed, String::from_utf8_lossy(&raw).to_string())
}

// ── The test ───────────────────────────────────────────────────────────────

#[test]
fn a_slow_scan_request_must_not_block_health() {
    if claw_fleet_core::process_util::which("node").is_none() {
        eprintln!("skipped: node not on PATH, the dsh fixture cannot run");
        return;
    }

    let fleet_home = unique_tempdir("health-behind-scan");
    let port_file = fleet_home.join("port");
    let token = "serve-concurrency-token";

    let mut serve = spawn_slow_dsh_serve(&fleet_home, &port_file, token);
    let port = wait_for_port_file(&port_file, Duration::from_secs(20), &mut serve);

    // Uncontended baseline: if `/health` is not fast when nothing else is in
    // flight, the harness is broken and the contended number would mean nothing.
    let (idle, body) = timed_get(port, "/health", token);
    assert!(
        body.contains("\"status\":\"ok\""),
        "baseline /health did not answer ok: {body}\n{}",
        serve.logs()
    );
    assert!(
        idle < HEALTH_BUDGET,
        "an idle /health already took {idle:?} (budget {HEALTH_BUDGET:?}); \
         the harness, not the request loop, is the problem\n{}",
        serve.logs()
    );

    // Put one scan-bearing request in flight. `/sessions` scans every source,
    // so it inherits the fixture's 5s `session.list`.
    let scan = {
        let token = token.to_string();
        std::thread::spawn(move || {
            let started = Instant::now();
            let (elapsed, body) = timed_get(port, "/sessions", &token);
            (started, elapsed, body)
        })
    };

    // Give the scan time to be accepted and reach the dsh RPC. Well under the
    // 5s the scan takes, so the probe below lands while it is still running.
    std::thread::sleep(Duration::from_millis(800));

    let probe_started = Instant::now();
    let (contended, health_body) = timed_get(port, "/health", token);

    let (scan_started, scan_elapsed, scan_body) = scan.join().expect("scan thread");

    // Non-vacuity: the scan must genuinely have been slow, and must have been
    // in flight for the whole probe. Otherwise "fast /health" proves nothing.
    assert!(
        scan_elapsed >= Duration::from_secs(3),
        "/sessions returned in {scan_elapsed:?}, so the slow dsh fixture was not \
         in the picture (is dsh disabled, or the fixture not being used?); \
         body: {}\n{}",
        scan_body.chars().take(400).collect::<String>(),
        serve.logs()
    );
    // The probe has to be *fired* while the scan is still running; where it
    // finishes is the very thing under test (blocked, it finishes when the scan
    // does, which is why only the start instant can be asserted here).
    assert!(
        probe_started > scan_started && probe_started < scan_started + scan_elapsed,
        "the /health probe was not fired while the scan was in flight: probe \
         started {:?} after the scan, which took {scan_elapsed:?}",
        probe_started.duration_since(scan_started),
    );

    assert!(
        health_body.contains("\"status\":\"ok\""),
        "contended /health did not answer ok: {health_body}\n{}",
        serve.logs()
    );
    assert!(
        contended < HEALTH_BUDGET,
        "/health took {contended:?} while a {scan_elapsed:?} scan was in flight \
         (idle it takes {idle:?}); budget {HEALTH_BUDGET:?}. The request loop is \
         serial: every request behind a slow scan inherits its latency.\n{}",
        serve.logs()
    );
}
