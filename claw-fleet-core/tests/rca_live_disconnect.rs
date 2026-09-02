//! Live verification of the ssh-disconnect stop-loss, over a REAL rca transport.
//!
//! The unit tests fake the marker with a shell script. This one does not: it
//! registers a real remote workspace, launches an agent through the real `rca`
//! binary over a real ssh tunnel, kills the tunnel, and asserts that Fleet
//! noticed, killed the agent, and filed a reason. It is the only test that can
//! prove the marker string still matches what the shipped rca actually prints.
//!
//! `#[ignore]` because it needs a reachable host. Run it with:
//!
//! ```sh
//! FLEET_RCA_LIVE_SSH=own-api-ko FLEET_RCA_LIVE_REMOTE_RCA=/root/.fleet/bin/rca \
//!   cargo test -p claw-fleet-core --test rca_live_disconnect -- --ignored --nocapture
//! ```
//!
//! The "agent" here is a loop that reads a file which exists only on the remote
//! — the same probe the manual 2026-09-02 experiment used. That is the point:
//! while the tunnel is up it reads the remote file; the moment the tunnel dies
//! it starts seeing the empty local mirror instead, which is exactly the state a
//! real agent misreads as "the repo was deleted".
//!
//! The probe is a throwaway Rust binary the test compiles with `rustc`, for two
//! reasons discovered the hard way here:
//!   * rca refuses to intercept Apple platform binaries ("macOS kills copies of
//!     these outside the system trust cache"), which rules out /bin/sh;
//!   * it re-signs a *copy* of the target into a temp dir, so a Homebrew
//!     interpreter fails at `dyld` — its `@rpath/libnode.147.dylib` isn't next
//!     to the copy. A Rust binary links only libSystem, which is in the dyld
//!     cache, so the copy runs.

use std::time::{Duration, Instant};

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// Kill the local ssh process rca spawned for `--via`. Matches on the
/// ServerAliveInterval option Fleet always passes, so it can't hit an unrelated
/// ssh the developer has open.
fn kill_the_tunnel(ssh_target: &str) {
    let pattern = format!("ssh -o ServerAliveInterval=15 .*{ssh_target}");
    let out = std::process::Command::new("pkill")
        .args(["-f", &pattern])
        .output()
        .expect("pkill must be runnable");
    eprintln!("pkill -f {pattern:?} -> {:?}", out.status.code());
}

#[test]
#[ignore = "needs a reachable ssh host; see the module docs for the env vars"]
fn a_dead_ssh_tunnel_stops_the_agent_and_files_a_reason() {
    let Some(ssh_target) = env("FLEET_RCA_LIVE_SSH") else {
        panic!("set FLEET_RCA_LIVE_SSH (and optionally FLEET_RCA_LIVE_REMOTE_RCA)");
    };
    let remote_rca =
        env("FLEET_RCA_LIVE_REMOTE_RCA").unwrap_or_else(|| "/root/.fleet/bin/rca".to_string());

    // Isolated FLEET_HOME so this never touches the developer's real ~/.fleet
    // (the workspace registry and the disconnect records both live there).
    let home = std::env::temp_dir().join(format!("fleet-rcalive-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: this test binary runs alone (one #[ignore]d test in the file).
    unsafe { std::env::set_var("FLEET_HOME", &home) };

    // A directory that exists ONLY on the remote, holding one marker file. The
    // identity-mapped local mirror Fleet creates at launch stays empty, so
    // "reads fine" vs "reads empty" is an unambiguous signal.
    //
    // NOT under /tmp: on macOS that resolves to /private/tmp, and rca routes by
    // the RESOLVED cwd — it would ask the Linux remote for a /private/tmp path
    // that cannot exist there. Anywhere symlink-free works as long as both ends
    // can hold the same absolute path (the remote runs as root, so it can).
    let ws = format!(
        "{}/fleet-rca-live-{}",
        std::env::var("HOME").expect("HOME"),
        std::process::id()
    );
    let remote_setup = format!("mkdir -p {ws} && printf 'alive' > {ws}/marker.txt && ls {ws}");
    let setup = claw_fleet_core::remote_host::ssh_exec(&ssh_target, &remote_setup)
        .expect("remote setup must succeed");
    assert!(setup.contains("marker.txt"), "remote setup output: {setup}");

    claw_fleet_core::remote_workspace::upsert(claw_fleet_core::remote_workspace::RemoteWorkspace {
        path: ws.clone(),
        ssh_target: Some(ssh_target.clone()),
        remote_rca_path: Some(remote_rca),
        label: Some(ssh_target.clone()),
        ..Default::default()
    })
    .expect("register the remote workspace");

    let probe_log = home.join("probe.log");
    let session_id = "live-disconnect-1";
    let (tx, rx) = std::sync::mpsc::channel();
    let stderr_log = home.join("stderr.log");
    let probe_src = home.join("probe.rs");
    let probe_bin = home.join("probe");
    std::fs::write(
        &probe_src,
        r#"
fn main() {
    let log = std::env::args().nth(1).expect("log path");
    let marker = std::env::args().nth(2).expect("marker path");
    for i in 1.. {
        let line = match std::fs::read_to_string(&marker) {
            Ok(s) => format!("{i} {s}"),
            Err(_) => format!("{i} MISSING"),
        };
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
            let _ = writeln!(f, "{line}");
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
"#,
    )
    .unwrap();
    let rustc = std::process::Command::new("rustc")
        .args(["-O", "-o"])
        .arg(&probe_bin)
        .arg(&probe_src)
        .output()
        .expect("rustc must be runnable");
    assert!(
        rustc.status.success(),
        "compiling the probe failed: {}",
        String::from_utf8_lossy(&rustc.stderr)
    );

    let pid = claw_fleet_core::session_launch::spawn_claude_detached_with_envs(
        probe_bin.to_str().unwrap(),
        &[
            probe_log.to_string_lossy().into_owned(),
            // ABSOLUTE, deliberately: measured here on 2026-09-02, rca routed
            // this Rust binary's absolute reads to the remote but left a
            // relative "marker.txt" resolving against the empty local mirror —
            // which would make the probe report MISSING while the tunnel was
            // still perfectly healthy, i.e. a permanently green-looking test.
            format!("{ws}/marker.txt"),
            "--session-id".to_string(),
            session_id.to_string(),
        ],
        &ws,
        &stderr_log,
        "rca-live-test",
        &format!("session={session_id}"),
        &[],
        false,
        move |_| {
            let _ = tx.send(());
        },
    )
    .expect("spawn through the real rca");
    eprintln!("agent pid={pid}, workspace={ws}");

    // Let the tunnel come up and the probe read the remote file at least once.
    std::thread::sleep(Duration::from_secs(8));
    let before = std::fs::read_to_string(&probe_log).unwrap_or_default();
    assert!(
        before.contains("alive"),
        "the probe never read the remote file, so the tunnel was never up.\n\
         probe log: {before:?}\nstderr: {:?}",
        std::fs::read_to_string(&stderr_log).unwrap_or_default()
    );

    kill_the_tunnel(&ssh_target);

    // Fleet must reap the agent. Generous window: this covers rca noticing the
    // reset, printing it, our monitor reading the line, and the kill landing.
    rx.recv_timeout(Duration::from_secs(30)).unwrap_or_else(|_| {
        panic!(
            "the agent survived a dead tunnel.\nstderr: {:?}",
            std::fs::read_to_string(&stderr_log).unwrap_or_default()
        )
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    let rec = loop {
        if let Some(r) = claw_fleet_core::remote_disconnect::read(session_id) {
            break r;
        }
        assert!(
            Instant::now() < deadline,
            "no disconnect record was filed.\nstderr: {:?}",
            std::fs::read_to_string(&stderr_log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(100));
    };

    eprintln!("record: {rec:?}");
    assert_eq!(
        rec.code,
        claw_fleet_core::remote_workspace::codes::TRANSPORT_LOST
    );
    assert!(
        rec.detail.contains("remote recv failed:")
            || rec.detail.contains("remote send failed:")
            || rec.detail.contains("remote dial failed:"),
        "the recorded line is not one of rca's transport-failure markers — the \
         shipped rca's wording may have changed: {}",
        rec.detail
    );
    assert!(rec.agent_stopped, "Fleet reported it could not stop the agent");
    assert_eq!(rec.host_label.as_deref(), Some(ssh_target.as_str()));

    // The probe must NOT have logged a long run of MISSING lines: that would
    // mean the agent kept working against the empty mirror, which is the exact
    // damage this feature exists to prevent. A couple of ticks is the detection
    // latency; a dozen would mean nobody stopped it.
    let after = std::fs::read_to_string(&probe_log).unwrap_or_default();
    let missing = after.lines().filter(|l| l.contains("MISSING")).count();
    eprintln!("probe ticks against the empty mirror: {missing}");
    assert!(
        missing <= 5,
        "the agent kept reading the empty local mirror for {missing} ticks:\n{after}"
    );

    let _ = claw_fleet_core::remote_host::ssh_exec(&ssh_target, &format!("rm -rf {ws}"));
    let _ = std::fs::remove_dir_all(&home);
}
