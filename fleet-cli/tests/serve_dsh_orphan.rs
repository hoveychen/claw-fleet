//! A terminated `fleet serve` must **leave** its `dsh web` running — and the
//! next Fleet process must adopt that same one rather than start a second.
//!
//! This file used to assert the opposite. Until `d9632e22` (2026-09-07,
//! "fix(dsh): preserve server across Fleet restarts") a `serve` exit killed its
//! `dsh web`, and the test that landed on 2026-08-18 guarded that. The contract
//! was then deliberately inverted: one `dsh web` serves every dsh session on the
//! machine, so tearing it down when Fleet restarts kills sessions mid-turn.
//! `serve`'s ctrlc handler now releases the two injectors and exits, with the
//! reason stated at the call site — "the authenticated dsh service is
//! machine-level and must survive a `fleet serve` restart while a turn is still
//! running".
//!
//! So the surviving process is the *feature*. What keeps "survives" from meaning
//! "leaks" is the registry at `$FLEET_HOME/dsh-server.json`: a record carrying a
//! launch token is retained for adoption, `DshServer::adopt_existing` reconnects
//! to it (authenticating with that token, health-probing `settings/describe`
//! first), and only token-less legacy records are killed by `reap_orphans`.
//! Both halves are asserted below, because "still alive" on its own is exactly
//! what an orphan looks like.
//!
//! Signal handling is still load-bearing and still tested by proxy: the exit
//! path only runs if `ctrlc::try_set_handler` installed, which it refuses to do
//! when SIGINT/SIGTERM/SIGHUP arrives as `SIG_IGN`. A background job in a
//! non-interactive shell inherits exactly that, so the child here sets `SIG_IGN`
//! between fork and exec, and `clear_inherited_signal_ignores` has to undo it —
//! otherwise the injector releases never run either.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long we watch after signalling serve, to be sure the `dsh web` is not
/// merely slow to die. Generous on purpose: the assertion is that nothing kills
/// it, so a short window would pass even on a regression that kills it late.
const SURVIVAL_WINDOW: Duration = Duration::from_secs(5);

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

/// The registry records Fleet keeps for live `dsh web` servers, read straight
/// from the file `dsh_server`'s registry helpers own. Only the two fields this
/// test reasons about are pulled out.
///
/// Note the `.fleet` component: `get_fleet_dir()` treats `FLEET_HOME` as a *home*
/// and appends `.fleet` to it, so the registry of a serve started with
/// `FLEET_HOME=<dir>` lands at `<dir>/.fleet/dsh-server.json`.
fn registry_servers(fleet_home: &Path) -> Vec<(u32, bool)> {
    let raw = std::fs::read_to_string(fleet_home.join(".fleet").join("dsh-server.json"))
        .unwrap_or_default();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();
    parsed
        .get("servers")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|r| {
                    let pid = r
                        .get("server")
                        .and_then(|s| s.get("pid"))
                        .and_then(|p| p.as_u64())
                        .unwrap_or(0) as u32;
                    let has_token = r
                        .get("launchToken")
                        .map(|t| !t.is_null())
                        .unwrap_or(false);
                    (pid, has_token)
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_terminated_serve_leaves_its_dsh_web_for_the_next_fleet_to_adopt() {
    if claw_fleet_core::process_util::which("node").is_none() {
        eprintln!("skipped: node not on PATH, the dsh fixture cannot run");
        return;
    }

    let fleet_home = unique_tempdir("adopt");
    let token = "orphan-test-token";

    let mut serve = spawn_serve_with_sigint_ignored(&fleet_home, &fleet_home.join("port"), token);
    let port = wait_for_port_file(&fleet_home.join("port"), Duration::from_secs(20), &mut serve);
    let serve_pid = serve.child.id();

    // A scan is what starts `dsh web`; nothing spawns it before the first call.
    let body = get(port, "/sessions", token);
    assert!(
        body.contains("session-fake-slow"),
        "the fixture's session is missing, so no dsh web was started and there \
         is nothing to hand over: {}",
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
    let _ = serve.child.wait();

    // Watch the whole window: a regression that kills the service late must fail.
    let deadline = Instant::now() + SURVIVAL_WINDOW;
    while Instant::now() < deadline {
        assert!(
            alive(dsh_pid),
            "dsh web {dsh_pid} died with the serve process that started it. That \
             was the contract until d9632e22 and is no longer: one dsh web serves \
             every dsh session on this machine, so a Fleet restart that takes it \
             down kills sessions mid-turn. serve log:\n{}",
            std::fs::read_to_string(&serve.log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(200));
    }

    // Surviving is only correct because the service is claimable. A record with
    // no launch token is precisely what `reap_orphans` kills, so a token-less
    // record here would mean the survivor is unreachable — an orphan holding a
    // port, which is the failure this file originally existed to catch.
    let records = registry_servers(&fleet_home);
    assert_eq!(
        records,
        vec![(dsh_pid, true)],
        "registry must retain exactly the surviving server, with its launch \
         token, so the next Fleet can authenticate to it"
    );

    // The other half of "not a leak": a second serve adopts it instead of
    // starting its own.
    let port_file_2 = fleet_home.join("port2");
    let mut serve2 = spawn_serve_with_sigint_ignored(&fleet_home, &port_file_2, token);
    let port2 = wait_for_port_file(&port_file_2, Duration::from_secs(20), &mut serve2);
    let body2 = get(port2, "/sessions", token);
    let serve2_pid = serve2.child.id();
    let own_children = dsh_children(serve2_pid);
    let records_after = registry_servers(&fleet_home);

    // Leave the machine no dirtier than we found it, whatever the asserts say.
    drop(serve2);
    unsafe { libc::kill(dsh_pid as libc::pid_t, libc::SIGKILL) };

    assert!(
        body2.contains("session-fake-slow"),
        "the second serve could not talk to the surviving dsh web, so the \
         retained launch token bought nothing: {}",
        body2.chars().take(300).collect::<String>()
    );
    assert!(
        own_children.is_empty(),
        "the second serve started its own dsh web ({own_children:?}) instead of \
         adopting pid {dsh_pid} — two servers on one machine is the duplicate \
         this registry exists to prevent"
    );
    assert_eq!(
        records_after,
        vec![(dsh_pid, true)],
        "after adoption the registry must still describe one server, the same one"
    );
}
