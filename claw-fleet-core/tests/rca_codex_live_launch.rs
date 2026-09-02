//! Live proof that a codex launch on a remote workspace actually starts.
//!
//! The unit tests assert the argv Fleet *builds*. This one runs it: it takes
//! `wrap_launch`'s real output for a codex-shaped argv and executes it against a
//! real rca over a real ssh tunnel. That is the only thing that can catch the
//! class of bug it was written for — Fleet's argv looked perfectly reasonable,
//! and rca refused it.
//!
//! `#[ignore]` because it needs a reachable host:
//!
//! ```sh
//! FLEET_RCA_LIVE_SSH=own-api-ko \
//!   cargo test -p claw-fleet-core --test rca_codex_live_launch -- --ignored --nocapture
//! ```
//!
//! The "codex" is a stand-in binary that prints its argv — the question here is
//! whether rca accepts the launch and hands the child its argv intact, not what
//! codex then does with it. Using the real codex would spend a model turn to
//! test a flag-ordering bug, and would drown the evidence in agent output.

use std::time::Duration;

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

#[test]
#[ignore = "needs a reachable ssh host; see the module docs for the env vars"]
fn a_codex_shaped_argv_survives_the_rca_wrapper() {
    let Some(ssh_target) = env("FLEET_RCA_LIVE_SSH") else {
        panic!("set FLEET_RCA_LIVE_SSH");
    };
    let remote_rca =
        env("FLEET_RCA_LIVE_REMOTE_RCA").unwrap_or_else(|| "/root/.fleet/bin/rca".to_string());

    let home = std::env::temp_dir().join(format!("fleet-codexlive-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: this test binary runs one test.
    unsafe { std::env::set_var("FLEET_HOME", &home) };

    // Same symlink-free, identity-mapped path rule as `rca_live_disconnect`.
    let ws = format!(
        "{}/fleet-codex-live-{}",
        std::env::var("HOME").expect("HOME"),
        std::process::id()
    );
    claw_fleet_core::remote_host::ssh_exec(&ssh_target, &format!("mkdir -p {ws}"))
        .expect("remote mkdir");
    std::fs::create_dir_all(&ws).unwrap();

    // A stand-in codex: prints its argv, one per line, and exits.
    let src = home.join("fakecodex.rs");
    let bin = home.join("fakecodex");
    std::fs::write(
        &src,
        "fn main(){for a in std::env::args().skip(1){println!(\"ARGV {a}\")}}",
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

    // Exactly the shape `codex_launch::build_codex_spawn_args` produces: flags,
    // then `--`, then a prompt — and one that starts with `-`, which is the
    // whole reason codex puts the separator there.
    let prompt = "--not-a-flag 这是提示词";
    let codex_argv: Vec<String> = [
        "exec",
        "--json",
        "--skip-git-repo-check",
        "-c",
        "model_reasoning_summary=auto",
        "--",
        prompt,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    let wrapped = claw_fleet_core::remote_workspace::wrap_launch(
        &ws,
        bin.to_str().unwrap(),
        &codex_argv,
    )
    .expect("wrap")
    .expect("registered workspace must wrap");

    let out = std::process::Command::new(&wrapped.program)
        .args(&wrapped.args)
        .current_dir(&ws)
        .envs(wrapped.envs.iter().map(|(k, v)| (k.clone(), v.clone())))
        .output()
        .expect("run rca");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    eprintln!("--- rca stderr ---\n{stderr}\n--- child stdout ---\n{stdout}");

    assert!(
        !stderr.contains("is required (got 0)"),
        "rca never saw its transport flags — they are behind the `--` again:\n{stderr}"
    );
    let got: Vec<String> = stdout
        .lines()
        .filter_map(|l| l.strip_prefix("ARGV ").map(|s| s.to_string()))
        .collect();
    assert_eq!(
        got, codex_argv,
        "the child must receive codex's argv byte for byte, separator included"
    );

    let _ = claw_fleet_core::remote_host::ssh_exec(&ssh_target, &format!("rmdir {ws}"));
    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&home);
    let _ = Duration::from_secs(0);
}
