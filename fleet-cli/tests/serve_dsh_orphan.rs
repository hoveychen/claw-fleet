//! `fleet serve` must take its `dsh web` with it when it is terminated.
//!
//! `serve()` installs its exit path through `ctrlc::try_set_handler`, which is
//! where `dsh_source::shutdown()` (and the injector releases) live. ctrlc refuses
//! to install when SIGINT, SIGTERM or SIGHUP already has a non-`SIG_DFL`
//! disposition — `platform::unix::init_os_handler` returns `EEXIST`, surfacing as
//! `MultipleHandlers`. A non-interactive shell sets SIGINT to `SIG_IGN` for
//! background jobs and `nohup` ignores SIGHUP, so a `fleet serve` started the way
//! Fleet's own harnesses and launchers start it inherits exactly that, logs
//! "ctrlc handler install failed", and dies on SIGTERM with no cleanup — leaving
//! its `dsh web` reparented to init, still holding its port.
//!
//! Measured before the fix, same binary and same SIGTERM:
//!   - spawned with SIGINT ignored (this test's shape) → dsh web survived, ppid=1
//!   - spawned with default dispositions              → dsh web reaped
//! Twelve such orphans had accumulated on one developer machine.
//!
//! So the test reproduces the inherited disposition rather than the shell: it
//! sets `SIG_IGN` in the child between fork and exec, which is precisely what a
//! background job inherits.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long the `dsh web` gets to disappear after serve is signalled. Generous:
/// the handler only has to kill and reap one child.
const CLEANUP_BUDGET: Duration = Duration::from_secs(8);

fn unique_tempdir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "fleet-dsh-orphan-{}-{}-{}",
        label,
        std::process::id(),
        nanos
    ));
    std::fs::create_dir_all(&dir).expect("create tempdir");
    dir
}

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
    log: PathBuf,
}

impl Drop for ServeGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start `fleet serve` with SIGINT ignored, the disposition a background job in a
/// non-interactive shell inherits.
fn spawn_serve_with_sigint_ignored(fleet_home: &Path, port_file: &Path, token: &str) -> ServeGuard {
    let log = fleet_home.join("serve.log");
    let log_file = std::fs::File::create(&log).expect("create serve log");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_fleet-cli"));
    cmd.args([
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
    .env("FAKE_DSH_LIST_DELAY_MS", "0")
    .env("FAKE_DSH_HISTORY_DELAY_MS", "0")
    .env("FAKE_DSH_SESSION_CWD", fleet_home)
    .stdout(Stdio::from(log_file.try_clone().expect("clone log")))
    .stderr(Stdio::from(log_file));
    // Between fork and exec, so the exec'd serve starts with it already ignored —
    // the same state it would inherit from `serve &` in a script.
    unsafe {
        cmd.pre_exec(|| {
            if libc::signal(libc::SIGINT, libc::SIG_IGN) == libc::SIG_ERR {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd.spawn().expect("spawn fleet-cli serve");
    ServeGuard { child, log }
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
            panic!(
                "timed out waiting for port-file {}\n  child: {:?}\n  --- log ---\n{}",
                path.display(),
                serve.child.try_wait(),
                std::fs::read_to_string(&serve.log).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn get(port: u16, path: &str, token: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("tcp connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\n\
         Connection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).expect("send");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read");
    String::from_utf8_lossy(&raw).to_string()
}

/// Direct children of `pid` that are the dsh fixture server.
fn dsh_children(pid: u32) -> Vec<u32> {
    let out = Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .expect("pgrep");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .filter(|child| {
            Command::new("ps")
                .args(["-o", "command=", "-p", &child.to_string()])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).contains("fake-dsh.js"))
                .unwrap_or(false)
        })
        .collect()
}

fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[test]
fn terminating_serve_takes_its_dsh_web_with_it() {
    if claw_fleet_core::process_util::which("node").is_none() {
        eprintln!("skipped: node not on PATH, the dsh fixture cannot run");
        return;
    }

    let fleet_home = unique_tempdir("sigterm");
    let port_file = fleet_home.join("port");
    let token = "orphan-test-token";

    let mut serve = spawn_serve_with_sigint_ignored(&fleet_home, &port_file, token);
    let port = wait_for_port_file(&port_file, Duration::from_secs(20), &mut serve);
    let serve_pid = serve.child.id();

    // A scan is what starts `dsh web`; nothing spawns it before the first call.
    let body = get(port, "/sessions", token);
    assert!(
        body.contains("session-fake-slow"),
        "the fixture's session is missing, so no dsh web was started and there \
         would be nothing to orphan: {}",
        body.chars().take(300).collect::<String>()
    );

    let children = dsh_children(serve_pid);
    assert_eq!(
        children.len(),
        1,
        "expected exactly one dsh web child of serve {serve_pid}, found {children:?}"
    );
    let dsh_pid = children[0];

    // The signal a supervisor, a script's cleanup trap, or `pkill` sends.
    unsafe { libc::kill(serve_pid as libc::pid_t, libc::SIGTERM) };

    let deadline = Instant::now() + CLEANUP_BUDGET;
    while alive(dsh_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }

    let still_there = alive(dsh_pid);
    if still_there {
        // Do not leave the machine dirtier than the test found it, whatever the
        // assertion decides.
        unsafe { libc::kill(dsh_pid as libc::pid_t, libc::SIGKILL) };
    }
    assert!(
        !still_there,
        "dsh web {dsh_pid} outlived the serve process that started it (its exit \
         path never ran, so nothing stopped it and it kept its port). serve log:\n{}",
        std::fs::read_to_string(&serve.log).unwrap_or_default()
    );
}
