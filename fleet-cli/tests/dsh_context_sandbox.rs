//! `fleet dsh-context` decides the sandbox mode; the plugin only carries it out.
//!
//! Every `fleet` command writes under `~/.fleet`, outside any workspace, so a
//! dsh session on dsh's default `workspace-write` cannot register a watch or
//! tick a plan without an escalation round-trip each time. dsh's sandbox has no
//! allow-list — three modes, bounded by the session's own cwd — so the only way
//! to let `~/.fleet` through is to turn the file sandbox off for that session.
//!
//! 老板 took that trade **only for the sessions Fleet drives**. A session he
//! opens in dsh himself must keep dsh's boundary and the confirmation it puts in
//! front of him, so the escalation is gated on Fleet ownership. This test drives
//! the real binary, because the gate is only worth anything end to end: the
//! knowledge of who spawned a session lives on Fleet's side, and this command is
//! where it reaches the plugin.

use std::process::Command;

fn dsh_context(fleet_home: &std::path::Path, session: &str) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_fleet-cli"))
        .args(["dsh-context", "--session", session, "--title", "Boss", "--locale", "en"])
        .arg("--cwd")
        .arg(fleet_home)
        .env("FLEET_HOME", fleet_home)
        .output()
        .expect("run fleet-cli dsh-context");
    assert!(
        out.status.success(),
        "dsh-context failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("dsh-context must emit JSON")
}

#[test]
fn only_a_fleet_spawned_session_is_told_to_leave_the_sandbox() {
    let home = tempfile::tempdir().unwrap();

    // A session Fleet spawned leaves a launch spec behind; that record is the
    // whole evidence of ownership.
    let specs = home.path().join(".fleet").join("launch-spec");
    std::fs::create_dir_all(&specs).unwrap();
    std::fs::write(
        specs.join("session-fleet-1.json"),
        r#"{"model":"openrouter/anthropic/claude-opus-5","entrypoint":"schedule"}"#,
    )
    .unwrap();

    let owned = dsh_context(home.path(), "session-fleet-1");
    let hand_opened = dsh_context(home.path(), "session-hand-opened");

    assert_eq!(
        owned.get("sandboxMode").and_then(|v| v.as_str()),
        Some("danger-full-access"),
        "a Fleet-driven session must be handed the escalation, or every fleet \
         command in it costs an approval round-trip"
    );
    assert!(
        hand_opened.get("sandboxMode").is_none(),
        "a session 老板 opened himself must keep dsh's sandbox — the field must \
         be absent, not null, so the plugin's `if (mode)` check cannot escalate \
         it by accident"
    );
    // The sections must survive either way: the sandbox decision rides alongside
    // them, it does not replace them.
    for payload in [&owned, &hand_opened] {
        let names: Vec<&str> = payload["sections"]
            .as_array()
            .expect("sections array")
            .iter()
            .filter_map(|s| s["name"].as_str())
            .collect();
        assert!(
            names.contains(&"fleet-session-id"),
            "the id block must still be injected, got {names:?}"
        );
    }
}
