//! `fleet webui` vs `fleet serve` — two subcommands, opposite defaults.
//!
//! `webui` is the browser build: the bundle plus the data routes it needs, no
//! token. `serve` is the token-gated API probe and must stay exactly that — in
//! particular it must NOT become a web server just because the environment
//! happens to carry the web-UI env vars, which is the regression these tests
//! exist to catch.
//!
//! Driven as real subprocesses against an isolated `FLEET_HOME`, same shape as
//! `serve_request_concurrency.rs` — this is wiring between CLI, router and
//! asset source, which a unit test on any one piece would miss.

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

/// A two-file stand-in for the real bundle, plus the mobile UI in `m/`.
///
/// `m/` is how the mobile bundle ships (deploy/lean/Dockerfile copies
/// `mobile-web/dist-webui` there): the phone gets its own UI at `/m/` off the
/// same port, and the desktop `index.html` redirects it there. There is no
/// route for it — the asset resolver maps paths to files, so a subdirectory is
/// reachable for free. That "for free" is exactly what could silently stop
/// being true, hence the assertion below.
fn write_bundle(root: &Path) {
    std::fs::create_dir_all(root.join("assets")).unwrap();
    std::fs::write(root.join("index.html"), b"<html>fleet board</html>").unwrap();
    std::fs::write(root.join("assets/app.js"), b"console.log('boot')").unwrap();
    std::fs::create_dir_all(root.join("m/assets")).unwrap();
    std::fs::write(root.join("m/index.html"), b"<html>fleet mobile</html>").unwrap();
    std::fs::write(root.join("m/assets/app.js"), b"console.log('mobile')").unwrap();
}

fn spawn(fleet_home: &Path, args: &[&str], envs: &[(&str, &str)]) -> ServeGuard {
    let binary = env!("CARGO_BIN_EXE_fleet-cli");
    let stderr_log = fleet_home.join("serve.stderr.log");
    let stdout_file = std::fs::File::create(fleet_home.join("serve.stdout.log")).unwrap();
    let stderr_file = std::fs::File::create(&stderr_log).unwrap();
    let mut cmd = Command::new(binary);
    cmd.args(args)
        .env("FLEET_HOME", fleet_home)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    for (k, v) in envs {
        cmd.env(k, v);
    }
    ServeGuard {
        child: cmd.spawn().expect("spawn fleet-cli"),
        stderr_log,
    }
}

fn spawn_webui(fleet_home: &Path, port_file: &Path, bundle: &Path) -> ServeGuard {
    spawn(
        fleet_home,
        &[
            "webui",
            "--port",
            "0",
            "--web-root",
            bundle.to_str().unwrap(),
            "--port-file",
            port_file.to_str().unwrap(),
        ],
        &[],
    )
}

fn spawn_serve(fleet_home: &Path, port_file: &Path, envs: &[(&str, &str)]) -> ServeGuard {
    spawn(
        fleet_home,
        &[
            "serve",
            "--port",
            "0",
            "--token",
            TOKEN,
            "--port-file",
            port_file.to_str().unwrap(),
        ],
        envs,
    )
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
fn webui_serves_the_bundle_and_data_routes_still_win() {
    let home = tempfile::TempDir::new().unwrap();
    let bundle = home.path().join("dist");
    write_bundle(&bundle);
    let port_file = home.path().join("port");

    let mut serve = spawn_webui(home.path(), &port_file, &bundle);
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

    // The mobile UI rides the same bundle dir under `m/`. `/m/` has to resolve
    // to *its* index, not the desktop one — a bundle-relative index fallback
    // would hand a phone the desktop app instead.
    let (status, body) = get(port, "/m/index.html", None);
    assert_eq!(status, 200, "mobile index should be served\n{}", serve.logs());
    assert!(body.contains("fleet mobile"), "got: {body}");

    let (status, body) = get(port, "/m/assets/app.js", None);
    assert_eq!(status, 200);
    assert!(body.contains("mobile"), "got: {body}");

    // The bare directory URL is the one that actually gets typed and redirected
    // to (`location.replace("/m/")`), so it has to resolve to that directory's
    // index the same way `/` does. Nothing else in the bundle needs this, which
    // is why it was missing.
    let (status, body) = get(port, "/m/", None);
    assert_eq!(status, 200, "/m/ should serve m/index.html\n{}", serve.logs());
    assert!(body.contains("fleet mobile"), "got: {body}");

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

/// `fleet serve` stays the token-gated API probe — and, critically, stays that
/// way even with the web-UI env vars set in its environment. Those two used to
/// be read inside the server; a stray env would then silently turn the API port
/// into an unauthenticated web server.
#[test]
fn serve_ignores_web_ui_env_and_still_demands_a_token() {
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

    let (status, _) = get(port, "/sessions", None);
    assert_eq!(status, 401, "a bare request must still be refused");

    let (status, _) = get(port, "/sessions", Some(TOKEN));
    assert_eq!(status, 200, "the admin token must still work");

    // And it serves no bundle: `/` is not a data route, so it 404s.
    let (status, _) = get(port, "/", Some(TOKEN));
    assert_eq!(status, 404, "serve must not become a web server");
}

/// `fleet webui` with nothing to serve is a misconfiguration, not a port that
/// answers 404 for every page.
///
/// Only meaningful without `embed-webui`: a build that carries the UI always has
/// something to serve, so it starts and blocks on accept — which made this test
/// hang rather than fail when run against such a build. The embedded shape is
/// covered by `webui_falls_back_to_the_builtin_ui_when_the_env_path_holds_no_bundle`.
#[cfg(not(feature = "embed-webui"))]
#[test]
fn webui_without_a_bundle_refuses_to_start() {
    let home = tempfile::TempDir::new().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_fleet-cli"))
        .args(["webui", "--port", "0"])
        .env("FLEET_HOME", home.path())
        .env_remove("FLEET_WEB_ROOT")
        .output()
        .expect("run fleet-cli webui");
    assert!(!out.status.success(), "must exit non-zero");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--web-root"), "should say how to fix it: {err}");
}

/// An explicit `--web-root` is a demand: a path that holds no bundle is a typo
/// worth failing on, not something to quietly paper over with the built-in UI.
#[test]
fn explicit_web_root_without_a_bundle_still_refuses_to_start() {
    let home = tempfile::TempDir::new().unwrap();
    let missing = home.path().join("nope");
    let out = Command::new(env!("CARGO_BIN_EXE_fleet-cli"))
        .args(["webui", "--port", "0", "--web-root", missing.to_str().unwrap()])
        .env("FLEET_HOME", home.path())
        .env_remove("FLEET_WEB_ROOT")
        .output()
        .expect("run fleet-cli webui");
    assert!(
        !out.status.success(),
        "an explicit --web-root at a bundle-less path must exit non-zero"
    );
}

/// `FLEET_WEB_ROOT` is ambient deployment config, not a per-invocation demand —
/// so a value that holds no bundle must fall back to the built-in UI rather than
/// refuse to start.
///
/// This is the exact shape the cloud container comes up in: the image presets
/// `FLEET_WEB_ROOT=/usr/share/fleet-web` as the UI/API mode switch, and since
/// the UI moved into the binary nothing is written to that path any more. The
/// first cut of that change only stopped the *entrypoint* from passing
/// `--web-root`; `cmd_webui` read the very same env var itself and exited 2, so
/// the container crash-looped in production with:
///
/// ```text
/// fleet-entrypoint: no bundle at /usr/share/fleet-web/index.html — serving the web UI built into the fleet binary
/// fleet webui: /usr/share/fleet-web is not a directory
/// ```
///
/// Gated on the feature because "fall back to the built-in UI" needs a built-in
/// UI to fall back to; with the feature off there is nothing to serve and the
/// hard error is still correct.
#[cfg(feature = "embed-webui")]
#[test]
fn webui_falls_back_to_the_builtin_ui_when_the_env_path_holds_no_bundle() {
    let home = tempfile::TempDir::new().unwrap();
    let port_file = home.path().join("port");
    let missing = home.path().join("usr-share-fleet-web");

    let mut serve = spawn(
        home.path(),
        &["webui", "--port", "0", "--port-file", port_file.to_str().unwrap()],
        &[("FLEET_WEB_ROOT", missing.to_str().unwrap())],
    );
    let port = wait_for_port(&port_file, &mut serve);

    let (status, body) = get(port, "/", None);
    assert_eq!(status, 200, "/ must serve the built-in UI, logs:\n{}", serve.logs());
    assert!(
        body.contains("<title>Claw Fleet</title>"),
        "expected the real embedded index.html, got: {}",
        &body[..body.len().min(200)]
    );

    // The mobile UI ships inside the same embedded bundle under `m/`.
    let (status, _) = get(port, "/m/", None);
    assert_eq!(status, 200, "/m/ must serve too, logs:\n{}", serve.logs());

    assert!(
        serve.logs().contains("built into this binary"),
        "startup line should say which bundle won, logs:\n{}",
        serve.logs()
    );
}

/// The env is `fleet webui`'s own fallback — that is how the container passes
/// the bundle path in.
#[test]
fn webui_accepts_the_bundle_from_the_env() {
    let home = tempfile::TempDir::new().unwrap();
    let bundle = home.path().join("dist");
    write_bundle(&bundle);
    let port_file = home.path().join("port");

    let mut serve = spawn(
        home.path(),
        &["webui", "--port", "0", "--port-file", port_file.to_str().unwrap()],
        &[("FLEET_WEB_ROOT", bundle.to_str().unwrap())],
    );
    let port = wait_for_port(&port_file, &mut serve);

    let (status, body) = get(port, "/", None);
    assert_eq!(status, 200);
    assert!(body.contains("fleet board"));
}
