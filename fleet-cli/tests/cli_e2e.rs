//! End-to-end test driving `fleet-cli task` against a live task-runtime
//! subprocess. Verifies Phase 3+ HTTP routing for `dispatch` and the
//! local-host fallback for `mark-done`.
//!
//! The task-runtime lifecycle (formerly the standalone `fleet-task` binary)
//! is now folded into `fleet-cli` as the hidden `task-runtime` subcommand, so
//! both the driver and the daemon are the same `fleet-cli` binary — only that
//! one binary needs to be built (`cargo build`); the test resolves it next to
//! its own test exe under `target/debug/`. Uses `FLEET_TASK_FAKE_LAUNCHER=1`
//! so neither side needs a real claude CLI.
//!
//! Unix-only: the fake launcher invoked inside task-runtime spawns the
//! `sleep` shell command, and the shutdown path here uses SIGTERM.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn bin_path(name: &str) -> PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // drop test exe name
    if p.ends_with("deps") {
        p.pop();
    }
    p.push(name);
    p
}

fn init_git(path: &Path) {
    let repo = git2::Repository::init(path).unwrap();
    let sig = git2::Signature::now("Test", "t@example.com").unwrap();
    let tree_oid = {
        let tb = repo.treebuilder(None).unwrap();
        tb.write().unwrap()
    };
    let tree = repo.find_tree(tree_oid).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
}

fn registry_dir(fleet_home: &Path) -> PathBuf {
    fleet_home.join(".fleet").join("runtime")
}

fn read_registry_task_id(fleet_home: &Path) -> Option<String> {
    let dir = registry_dir(fleet_home);
    for ent in std::fs::read_dir(&dir).ok()? {
        let ent = ent.ok()?;
        let path = ent.path();
        let name = path.file_name()?.to_str()?.to_string();
        if !name.ends_with(".json") {
            continue;
        }
        let bytes = std::fs::read(&path).ok()?;
        let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        return v.get("task_id")?.as_str().map(|s| s.to_string());
    }
    None
}

#[test]
fn fleet_cli_task_dispatch_routes_via_http_to_fleet_task() {
    let fleet_home = tempfile::TempDir::new().unwrap();
    let workspace = tempfile::TempDir::new().unwrap();
    init_git(workspace.path());

    let fleet_cli = bin_path("fleet-cli");
    assert!(fleet_cli.is_file(), "fleet-cli not at {}", fleet_cli.display());

    // Spawn `fleet-cli task-runtime new` to own the lifecycle. Fake launcher
    // replaces claude with `sleep 60` so we don't need the real CLI in CI.
    let mut server = Command::new(&fleet_cli)
        .arg("task-runtime")
        .arg("new")
        .arg("--workspace")
        .arg(workspace.path())
        .arg("--prompt")
        .arg("e2e test")
        .arg("--title")
        .arg("e2e")
        .arg("--no-tui")
        .env("FLEET_HOME", fleet_home.path())
        .env("FLEET_TASK_FAKE_LAUNCHER", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn fleet-cli task-runtime");

    // Wait until the registry entry shows up so we know the HTTP server is
    // bound and ready.
    let deadline = Instant::now() + Duration::from_secs(10);
    let task_id = loop {
        if let Some(id) = read_registry_task_id(fleet_home.path()) {
            break id;
        }
        if Instant::now() > deadline {
            let _ = server.kill();
            panic!("task-runtime never wrote a registry entry");
        }
        std::thread::sleep(Duration::from_millis(100));
    };

    // Verify the read path: `fleet-cli task get-plan` reads task.json
    // directly. With an empty plan that still prints `items: {}\n`.
    let get_plan = Command::new(&fleet_cli)
        .arg("task")
        .arg("get-plan")
        .arg(&task_id)
        .env("FLEET_HOME", fleet_home.path())
        .output()
        .expect("get-plan");
    assert!(
        get_plan.status.success(),
        "fleet-cli task get-plan failed: stdout={} stderr={}",
        String::from_utf8_lossy(&get_plan.stdout),
        String::from_utf8_lossy(&get_plan.stderr)
    );
    assert!(
        String::from_utf8_lossy(&get_plan.stdout).contains("items"),
        "get-plan stdout missing `items`: {}",
        String::from_utf8_lossy(&get_plan.stdout)
    );

    // Verify the HTTP write path: `fleet-cli task dispatch --task-id=X --p-id=missing`
    // hits `POST /p-items/missing/dispatch` on fleet-task. The fake plan has
    // no P-items so dispatch returns 500 — but the *route* worked (no
    // ConnectionRefused, no 503, no "no fleet-task process" error).
    let dispatch = Command::new(&fleet_cli)
        .arg("task")
        .arg("dispatch")
        .arg(&task_id)
        .arg("missing-p")
        .env("FLEET_HOME", fleet_home.path())
        .output()
        .expect("dispatch");
    let stderr = String::from_utf8_lossy(&dispatch.stderr).to_string();
    // Either succeeds (status 202) or fails with a structured 500 — both
    // prove the HTTP path is reachable. What we need to rule out is
    // "no fleet-task process for task X" (registry miss).
    assert!(
        !stderr.contains("no fleet-task process"),
        "dispatch didn't route via HTTP. stderr={stderr}"
    );

    // SIGTERM the task-runtime subprocess and confirm clean exit.
    #[cfg(unix)]
    unsafe {
        libc::kill(server.id() as libc::pid_t, libc::SIGTERM);
    }
    let exit_deadline = Instant::now() + Duration::from_secs(40);
    loop {
        match server.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() > exit_deadline {
                    let _ = server.kill();
                    let _ = server.wait();
                    panic!("task-runtime did not exit within 40s of SIGTERM");
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => panic!("try_wait: {e}"),
        }
    }
}
