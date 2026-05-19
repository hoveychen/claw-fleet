//! End-to-end smoke test for the `fleet-task` binary. Spawns the compiled
//! binary as a subprocess with `FLEET_TASK_FAKE_LAUNCHER=1` so the master
//! "claude" call is replaced with a long-running `sleep` — no real Claude CLI
//! required in CI.
//!
//! What we verify:
//! 1. `fleet-task new --no-tui` boots successfully (process stays alive).
//! 2. The runtime registry has an entry pointing at a port.
//! 3. `GET /health` on that port returns 200 with our task_id.
//! 4. `GET /state` returns the persisted task.
//! 5. SIGTERM the subprocess → exits within a few seconds.
//! 6. After the subprocess exits, the runtime registry entry is gone.

use std::io::Read;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn bin_path() -> PathBuf {
    let mut p = std::env::current_exe().unwrap();
    // `target/debug/deps/binary_smoke-<hash>` → strip `deps/<name>` to land
    // in `target/debug`, then `target/debug/fleet-task`.
    p.pop();
    if p.ends_with("deps") {
        p.pop();
    }
    p.push("fleet-task");
    p
}

fn init_git(path: &std::path::Path) {
    let repo = git2::Repository::init(path).unwrap();
    let sig = git2::Signature::now("Test", "t@example.com").unwrap();
    let tree_oid = {
        let tb = repo.treebuilder(None).unwrap();
        tb.write().unwrap()
    };
    let tree = repo.find_tree(tree_oid).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .unwrap();
}

fn read_registry_port(fleet_home: &std::path::Path) -> Option<(String, u16)> {
    let dir = fleet_home.join(".fleet").join("runtime");
    for ent in std::fs::read_dir(&dir).ok()? {
        let ent = ent.ok()?;
        let path = ent.path();
        let name = path.file_name()?.to_str()?.to_string();
        if !name.ends_with(".json") {
            continue;
        }
        let bytes = std::fs::read(&path).ok()?;
        let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let task_id = v.get("task_id")?.as_str()?.to_string();
        let port = v.get("port")?.as_u64()? as u16;
        return Some((task_id, port));
    }
    None
}

fn http_get(port: u16, path: &str) -> Option<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .ok()?;
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    );
    std::io::Write::write_all(&mut stream, req.as_bytes()).ok()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).to_string();
    let mut lines = text.lines();
    let first = lines.next()?;
    let status: u16 = first.split_whitespace().nth(1)?.parse().ok()?;
    let body_start = text.find("\r\n\r\n")? + 4;
    Some((status, text[body_start..].to_string()))
}

#[test]
fn fleet_task_new_serves_http_and_cleans_up_on_sigterm() {
    let fleet_home = tempfile::TempDir::new().unwrap();
    let workspace = tempfile::TempDir::new().unwrap();
    init_git(workspace.path());

    let bin = bin_path();
    assert!(bin.is_file(), "fleet-task binary not at {}", bin.display());

    let mut child = Command::new(&bin)
        .arg("new")
        .arg("--workspace")
        .arg(workspace.path())
        .arg("--prompt")
        .arg("integration smoke")
        .arg("--title")
        .arg("smoke")
        .arg("--no-tui")
        .env("FLEET_HOME", fleet_home.path())
        .env("FLEET_TASK_FAKE_LAUNCHER", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fleet-task");

    // Wait until the registry entry appears (HTTP server bound + entry written).
    let deadline = Instant::now() + Duration::from_secs(10);
    let (task_id, port) = loop {
        if let Some(pair) = read_registry_port(fleet_home.path()) {
            break pair;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("fleet-task never wrote a registry entry");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(port > 0);

    let (status, body) =
        http_get(port, "/health").expect("/health should respond");
    assert_eq!(status, 200, "body={body}");
    assert!(body.contains(&task_id), "/health body missing task_id: {body}");

    let (status, body) =
        http_get(port, "/state").expect("/state should respond");
    assert_eq!(status, 200, "body={body}");
    assert!(body.contains(&task_id), "/state body missing task_id: {body}");

    // Send SIGTERM and confirm the subprocess exits and clears the registry.
    #[cfg(unix)]
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }

    let exit_deadline = Instant::now() + Duration::from_secs(40);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() > exit_deadline {
                    let _ = child.kill();
                    let mut buf = String::new();
                    if let Some(mut e) = child.stderr.take() {
                        let _ = e.read_to_string(&mut buf);
                    }
                    let _ = child.wait();
                    panic!(
                        "fleet-task did not exit within 40s of SIGTERM. stderr:\n{}",
                        buf
                    );
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => panic!("try_wait: {e}"),
        }
    }

    assert!(
        read_registry_port(fleet_home.path()).is_none(),
        "registry entry should be cleaned up after exit"
    );
}
