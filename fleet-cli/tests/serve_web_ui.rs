//! `fleet serve` serving the web UI alongside the data API.
//!
//! This is what makes the cloud container answer a browser: paths that match no
//! data route fall through to the `vite build` bundle at `FLEET_WEB_ROOT`, and
//! `FLEET_SERVE_NO_AUTH` lets a deployment that fronts the port with its own
//! auth gateway skip the token tiering (the UI needs routes outside the
//! `/v1/*` scoped whitelist).
//!
//! Driven as a real subprocess against an isolated `FLEET_HOME`, same shape as
//! `serve_request_concurrency.rs` — the two behaviours are wiring between env,
//! router and asset source, which a unit test on any one piece would miss.

use std::io::Read;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TOKEN: &str = "serve-web-ui-token";

struct ServeGuard {
    child: Child,
    stderr_log: PathBuf,
}

impl ServeGuard {
    fn logs(&self) -> String {
        std::fs::read_to_string(&self.stderr_log).unwrap_or_default()
    }
}

impl Drop for ServeGuard {
    fn drop(&mut self) {
        // SIGKILL is fine here: the isolated FLEET_HOME is a temp dir and
        // nothing outside it needs the termination handler to run.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A two-file stand-in for the real bundle.
fn write_bundle(root: &Path) {
    std::fs::create_dir_all(root.join("assets")).unwrap();
    std::fs::write(root.join("index.html"), b"<html>fleet board</html>").unwrap();
    std::fs::write(root.join("assets/app.js"), b"console.log('boot')").unwrap();
}

fn spawn_serve(fleet_home: &Path, port_file: &Path, envs: &[(&str, &str)]) -> ServeGuard {
    let binary = env!("CARGO_BIN_EXE_fleet-cli");
    let stderr_log = fleet_home.join("serve.stderr.log");
    let stdout_file = std::fs::File::create(fleet_home.join("serve.stdout.log")).unwrap();
    let stderr_file = std::fs::File::create(&stderr_log).unwrap();
    let mut cmd = Command::new(binary);
    cmd.args([
        "serve",
        "--port",
        "0",
        "--token",
        TOKEN,
        "--port-file",
        port_file.to_str().unwrap(),
    ])
    .env("FLEET_HOME", fleet_home)
    .stdout(Stdio::from(stdout_file))
    .stderr(Stdio::from(stderr_file));
    for (k, v) in envs {
        cmd.env(k, v);
    }
    ServeGuard {
        child: cmd.spawn().expect("spawn fleet-cli serve"),
        stderr_log,
    }
}

fn wait_for_port(path: &Path, serve: &mut ServeGuard) -> u16 {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Ok(s) = std::fs::read_to_string(path) {
            if let Ok(n) = s.trim().parse::<u16>() {
                if n > 0 {
                    return n;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the port file\n{}",
            serve.logs()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Minimal HTTP/1.0 GET — avoids pulling a client dependency into this crate's
/// test deps just to read a status line.
fn get(port: u16, path: &str, bearer: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let auth = match bearer {
        Some(t) => format!("Authorization: Bearer {t}\r\n"),
        None => String::new(),
    };
    let req = format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\n{auth}\r\n");
    std::io::Write::write_all(&mut stream, req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    (status, text)
}

#[test]
fn web_root_is_served_and_data_routes_still_win() {
    let home = tempfile::TempDir::new().unwrap();
    let bundle = home.path().join("dist");
    write_bundle(&bundle);
    let port_file = home.path().join("port");

    let mut serve = spawn_serve(
        home.path(),
        &port_file,
        &[
            ("FLEET_WEB_ROOT", bundle.to_str().unwrap()),
            ("FLEET_SERVE_NO_AUTH", "1"),
        ],
    );
    let port = wait_for_port(&port_file, &mut serve);

    // `/` resolves to index.html out of the bundle.
    let (status, body) = get(port, "/", None);
    assert_eq!(status, 200, "index should be served\n{}", serve.logs());
    assert!(body.contains("fleet board"), "got: {body}");

    // Nested asset, with the mime the shared resolver assigns.
    let (status, body) = get(port, "/assets/app.js", None);
    assert_eq!(status, 200);
    assert!(
        body.to_ascii_lowercase().contains("text/javascript"),
        "asset mime missing from headers: {body}"
    );

    // A data route must not be shadowed by the bundle — the UI would receive
    // HTML where it expects JSON.
    let (status, body) = get(port, "/sessions", None);
    assert_eq!(status, 200);
    assert!(
        body.contains('[') && !body.contains("fleet board"),
        "sessions should answer JSON, got: {body}"
    );

    // A path in neither is still a 404, not an index fallback.
    let (status, _) = get(port, "/assets/missing.js", None);
    assert_eq!(status, 404);
}

/// Without the opt-in env the tiering is untouched: no token, no data. This is
/// what keeps existing deployments unchanged by the feature.
#[test]
fn without_no_auth_the_token_is_still_required() {
    let home = tempfile::TempDir::new().unwrap();
    let bundle = home.path().join("dist");
    write_bundle(&bundle);
    let port_file = home.path().join("port");

    let mut serve = spawn_serve(
        home.path(),
        &port_file,
        &[("FLEET_WEB_ROOT", bundle.to_str().unwrap())],
    );
    let port = wait_for_port(&port_file, &mut serve);

    let (status, _) = get(port, "/sessions", None);
    assert_eq!(status, 401, "a bare request must still be refused");

    let (status, _) = get(port, "/sessions", Some(TOKEN));
    assert_eq!(status, 200, "the admin token must still work");
}

/// No bundle configured → the container is API-only and unknown paths 404
/// rather than erroring.
#[test]
fn without_web_root_unknown_paths_are_404() {
    let home = tempfile::TempDir::new().unwrap();
    let port_file = home.path().join("port");

    let mut serve = spawn_serve(home.path(), &port_file, &[("FLEET_SERVE_NO_AUTH", "1")]);
    let port = wait_for_port(&port_file, &mut serve);

    let (status, _) = get(port, "/", None);
    assert_eq!(status, 404);

    // The data API is unaffected.
    let (status, _) = get(port, "/sessions", None);
    assert_eq!(status, 200);
}
