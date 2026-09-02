//! `POST /mobile_rpc` — the mobile RPC surface over plain HTTP.
//!
//! The phone's 48 data methods live in one dispatcher
//! (`mobile_relay::serve_request`) that until now was only reachable over the
//! relay WebSocket. The browser build served by `fleet webui` needs the same
//! methods same-origin, with no relay in the picture at all — so the route is a
//! thin bridge onto that dispatcher rather than 48 new paths.
//!
//! Driven as a real subprocess against an isolated `FLEET_HOME`, same shape as
//! `webui_vs_serve.rs`: this is wiring between CLI, router and dispatcher, and
//! a unit test on any one piece would miss it. The isolation is also what makes
//! `wiki_list` a deterministic probe — a fresh `FLEET_HOME` has no wiki dir, so
//! the answer is an empty list rather than whatever this machine happens to
//! have published.

use std::io::Read;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TOKEN: &str = "mobile-rpc-token";

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
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_bundle(root: &Path) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("index.html"), b"<html>fleet board</html>").unwrap();
}

fn spawn(fleet_home: &Path, args: &[&str]) -> ServeGuard {
    let binary = env!("CARGO_BIN_EXE_fleet-cli");
    let stderr_log = fleet_home.join("serve.stderr.log");
    let stdout_file = std::fs::File::create(fleet_home.join("serve.stdout.log")).unwrap();
    let stderr_file = std::fs::File::create(&stderr_log).unwrap();
    let mut cmd = Command::new(binary);
    cmd.args(args)
        .env("FLEET_HOME", fleet_home)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
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
    )
}

fn spawn_serve(fleet_home: &Path, port_file: &Path) -> ServeGuard {
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

/// Minimal HTTP/1.0 GET with a bearer token.
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

/// Minimal HTTP/1.0 request with an arbitrary method and no body — for the CORS
/// preflight, which is an `OPTIONS` carrying only headers.
fn options(port: u16, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    // 预检长这样:浏览器**不带** Authorization —— 它正是在问「带这个头行不行」。
    let req = format!(
        "OPTIONS {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nOrigin: https://fleet-relay.example.com\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: authorization, content-type\r\n\r\n"
    );
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

/// Minimal HTTP/1.0 POST — avoids pulling a client dependency into this
/// crate's test deps just to send a JSON body.
fn post(port: u16, path: &str, body: &str, bearer: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let auth = match bearer {
        Some(t) => format!("Authorization: Bearer {t}\r\n"),
        None => String::new(),
    };
    let req = format!(
        "POST {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{auth}\r\n{body}",
        body.len()
    );
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

/// The happy path the browser build depends on: a method name goes in, the
/// dispatcher's answer comes back under `ok`/`data`.
#[test]
fn webui_answers_mobile_rpc() {
    let home = tempfile::TempDir::new().unwrap();
    let bundle = home.path().join("dist");
    write_bundle(&bundle);
    let port_file = home.path().join("port");

    let mut serve = spawn_webui(home.path(), &port_file, &bundle);
    let port = wait_for_port(&port_file, &mut serve);

    let (status, body) = post(
        port,
        "/mobile_rpc",
        r#"{"method":"wiki_list","params":{}}"#,
        None,
    );
    assert_eq!(
        status,
        200,
        "mobile_rpc should answer on the browser build\n{}",
        serve.logs()
    );
    assert!(body.contains(r#""ok":true"#), "got: {body}");
    // A fresh FLEET_HOME has published nothing, so the dispatcher's own answer
    // is an empty list — proof the call reached `serve_wiki_list` rather than
    // some generic 200.
    assert!(body.contains(r#""data":[]"#), "got: {body}");
}

/// An unknown method is a dispatcher-level `ok:false`, not an HTTP error: the
/// client distinguishes "the desktop refused" from "the request never landed",
/// and collapsing the former into a 4xx would erase that.
#[test]
fn mobile_rpc_reports_an_unknown_method_in_band() {
    let home = tempfile::TempDir::new().unwrap();
    let bundle = home.path().join("dist");
    write_bundle(&bundle);
    let port_file = home.path().join("port");

    let mut serve = spawn_webui(home.path(), &port_file, &bundle);
    let port = wait_for_port(&port_file, &mut serve);

    let (status, body) = post(
        port,
        "/mobile_rpc",
        r#"{"method":"no_such_method","params":{}}"#,
        None,
    );
    assert_eq!(status, 200, "still a well-formed reply\n{}", serve.logs());
    assert!(body.contains(r#""ok":false"#), "got: {body}");
    assert!(body.contains("unknown method"), "got: {body}");
}

/// `fleet serve` is the token-gated probe and must stay that way: the bridge
/// rides the existing admin tier rather than opening a new unauthenticated
/// door onto every mobile method.
#[test]
fn serve_keeps_mobile_rpc_behind_the_admin_token() {
    let home = tempfile::TempDir::new().unwrap();
    let port_file = home.path().join("port");

    let mut serve = spawn_serve(home.path(), &port_file);
    let port = wait_for_port(&port_file, &mut serve);

    let (status, _) = post(
        port,
        "/mobile_rpc",
        r#"{"method":"wiki_list","params":{}}"#,
        None,
    );
    assert_eq!(status, 401, "no token ⇒ denied\n{}", serve.logs());

    let (status, body) = post(
        port,
        "/mobile_rpc",
        r#"{"method":"wiki_list","params":{}}"#,
        Some(TOKEN),
    );
    assert_eq!(status, 200, "admin token ⇒ allowed\n{}", serve.logs());
    assert!(body.contains(r#""ok":true"#), "got: {body}");
}

/// 手机在设备簿里直连一台主机时,页面的 origin 是中转域名,`/mobile_rpc` 因此是
/// 跨源请求。没有 CORS 头,浏览器会在页面读到响应之前把它拦掉 —— 服务端明明答了
/// 200,前端只看到一个网络错误。
///
/// 预检这一段单独钉住,因为它有一个特别容易写错的顺序:浏览器发 OPTIONS 时**不带**
/// Authorization,所以预检必须抢在认证之前答;放到之后会拿到 401,真正的请求永远
/// 发不出去。
#[test]
fn token_gated_serve_allows_cross_origin_mobile_rpc() {
    let home = tempfile::TempDir::new().unwrap();
    let port_file = home.path().join("port");
    let mut serve = spawn_serve(home.path(), &port_file);
    let port = wait_for_port(&port_file, &mut serve);

    let (status, headers) = options(port, "/mobile_rpc");
    assert_eq!(status, 204, "preflight must be answered before auth\n{headers}");
    let lower = headers.to_ascii_lowercase();
    assert!(lower.contains("access-control-allow-origin: *"), "got: {headers}");
    assert!(lower.contains("access-control-allow-headers"), "got: {headers}");
    assert!(lower.contains("authorization"), "token must be an allowed header\n{headers}");

    // 真正的请求(带 token)也要带上 ACAO,否则浏览器不让页面读这个响应。
    let (status, body) = post(
        port,
        "/mobile_rpc",
        r#"{"method":"wiki_list","params":{}}"#,
        Some(TOKEN),
    );
    assert_eq!(status, 200, "{}", serve.logs());
    assert!(
        body.to_ascii_lowercase().contains("access-control-allow-origin: *"),
        "got: {body}"
    );
}

/// 拒绝也必须带跨源头。少了它,手机那边**看不见** 401 —— 浏览器对缺 CORS 头的
/// 响应只交给 JS 一个笼统的网络错误,于是「token 填错了」在界面上显示成「连不上
/// 这台主机」。两句话指向完全不同的修法,所以这一条单独钉住。
#[test]
fn cross_origin_rejection_is_readable_by_the_page() {
    let home = tempfile::TempDir::new().unwrap();
    let port_file = home.path().join("port");
    let mut serve = spawn_serve(home.path(), &port_file);
    let port = wait_for_port(&port_file, &mut serve);

    let (status, body) = post(
        port,
        "/mobile_rpc",
        r#"{"method":"wiki_list","params":{}}"#,
        Some("wrong-token"),
    );
    assert_eq!(status, 403, "{}", serve.logs());
    assert!(
        body.to_ascii_lowercase().contains("access-control-allow-origin: *"),
        "a rejected cross-origin request must still be readable\ngot: {body}"
    );
}

/// 反面:`fleet webui` 那个端口本身没有认证(它自己的启动日志就写着必须在前面放
/// 网关)。在那种端口上发 CORS 头等于让用户浏览器里任何一个网页都能驱动这台
/// Fleet,所以它**不**发 —— 那种部署要跨源就在自己的网关上配。
#[test]
fn no_auth_webui_does_not_open_cross_origin() {
    let home = tempfile::TempDir::new().unwrap();
    let bundle = home.path().join("dist");
    write_bundle(&bundle);
    let port_file = home.path().join("port");
    let mut serve = spawn_webui(home.path(), &port_file, &bundle);
    let port = wait_for_port(&port_file, &mut serve);

    // 同源仍然照常工作 —— 同源本来就不需要 CORS 头。
    let (status, body) = post(
        port,
        "/mobile_rpc",
        r#"{"method":"wiki_list","params":{}}"#,
        None,
    );
    assert_eq!(status, 200, "{}", serve.logs());
    assert!(
        !body.to_ascii_lowercase().contains("access-control-allow-origin"),
        "an unauthenticated port must not advertise CORS\ngot: {body}"
    );
}

