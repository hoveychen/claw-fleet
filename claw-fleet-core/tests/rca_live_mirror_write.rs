//! Live proof that a misrouted write is caught, over a REAL rca transport.
//!
//! The unit tests in `mirror_guard` prove the rule ("anything in the mirror is a
//! report"). This proves the premise: that a relative-path write really does
//! land in the local mirror while the tunnel is perfectly healthy, and that
//! Fleet's spawn path notices after the session exits.
//!
//! Both halves matter. If rca ever starts routing relative writes, the rule
//! stays correct but the whole guard becomes dead weight — and this is the only
//! test that would tell us.
//!
//! ```sh
//! FLEET_RCA_LIVE_SSH=own-api-ko \
//!   cargo test -p claw-fleet-core --test rca_live_mirror_write -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

#[test]
#[ignore = "needs a reachable ssh host; see the module docs"]
fn a_relative_write_lands_locally_and_is_reported() {
    let Some(ssh_target) = env("FLEET_RCA_LIVE_SSH") else {
        panic!("set FLEET_RCA_LIVE_SSH");
    };
    let remote_rca =
        env("FLEET_RCA_LIVE_REMOTE_RCA").unwrap_or_else(|| "/root/.fleet/bin/rca".to_string());

    let home = std::env::temp_dir().join(format!("fleet-mirrorlive-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: this test binary runs one test.
    unsafe { std::env::set_var("FLEET_HOME", &home) };

    // Symlink-free identity-mapped path (see `rca_live_disconnect` for why not
    // /tmp on macOS).
    let ws = format!(
        "{}/fleet-mirror-live-{}",
        std::env::var("HOME").expect("HOME"),
        std::process::id()
    );
    claw_fleet_core::remote_host::ssh_exec(&ssh_target, &format!("mkdir -p {ws}"))
        .expect("remote mkdir");

    // A probe that writes BOTH ways: one relative, one absolute. The absolute
    // one is the control — it proves the tunnel was up, so a failure here can't
    // be explained away as "the remote was unreachable anyway".
    let src = home.join("probe.rs");
    let bin = home.join("probe");
    std::fs::write(
        &src,
        r#"
fn main() {
    let abs = std::env::args().nth(1).expect("abs");
    std::fs::write("relative-write.txt", "landed locally").expect("relative write");
    std::fs::write(format!("{abs}/absolute-write.txt"), "landed remotely").expect("absolute write");
}
"#,
    )
    .unwrap();
    let rustc = std::process::Command::new("rustc")
        .args(["-O", "-o"])
        .arg(&bin)
        .arg(&src)
        .output()
        .expect("rustc");
    assert!(rustc.status.success(), "{}", String::from_utf8_lossy(&rustc.stderr));

    claw_fleet_core::remote_workspace::upsert(
        claw_fleet_core::remote_workspace::RemoteWorkspace {
            path: ws.clone(),
            ssh_target: Some(ssh_target.clone()),
            remote_rca_path: Some(remote_rca),
            label: Some(ssh_target.clone()),
            ..Default::default()
        },
    )
    .expect("register the remote workspace");

    let session_id = "mirror-live-1";
    let (tx, rx) = std::sync::mpsc::channel();
    let stderr_log = home.join("stderr.log");
    claw_fleet_core::session_launch::spawn_claude_detached_with_envs(
        bin.to_str().unwrap(),
        &[ws.clone(), "--session-id".to_string(), session_id.to_string()],
        &ws,
        &stderr_log,
        "mirror-live-test",
        &format!("session={session_id}"),
        &[],
        false,
        move |_| {
            let _ = tx.send(());
        },
    )
    .expect("spawn through rca");

    rx.recv_timeout(Duration::from_secs(60)).unwrap_or_else(|_| {
        panic!(
            "the probe never exited.\nstderr: {:?}",
            std::fs::read_to_string(&stderr_log).unwrap_or_default()
        )
    });

    // Control first: if the absolute write did not reach the remote, the tunnel
    // was broken and the rest of this test proves nothing.
    let remote_ls = claw_fleet_core::remote_host::ssh_exec(&ssh_target, &format!("ls {ws}"))
        .expect("remote ls");
    assert!(
        remote_ls.contains("absolute-write.txt"),
        "the absolute write never reached the remote — the tunnel was down, so this \
         test can't tell us anything about routing. remote ls: {remote_ls:?}"
    );
    assert!(
        !remote_ls.contains("relative-write.txt"),
        "the relative write DID reach the remote — rca now routes relative paths, \
         which makes this guard obsolete rather than broken. remote ls: {remote_ls:?}"
    );

    // The report is filed by the reaper thread, which can be a beat behind the
    // exit notification above.
    let deadline = Instant::now() + Duration::from_secs(10);
    let rec = loop {
        if let Some(r) = claw_fleet_core::mirror_guard::read(session_id) {
            break r;
        }
        assert!(
            Instant::now() < deadline,
            "the misrouted write was not reported. mirror holds: {:?}",
            std::fs::read_dir(&ws)
                .map(|d| d.flatten().map(|e| e.file_name()).collect::<Vec<_>>())
                .unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(100));
    };

    eprintln!("report: {rec:?}");
    assert_eq!(rec.files, vec!["relative-write.txt"]);
    assert_eq!(rec.total, 1);
    assert_eq!(rec.workspace_path, ws);

    let _ = claw_fleet_core::remote_host::ssh_exec(&ssh_target, &format!("rm -rf {ws}"));
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&home);
}
