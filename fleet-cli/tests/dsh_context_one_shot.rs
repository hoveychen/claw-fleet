//! `fleet dsh-context` tells the plugin which sessions are one-step forks.
//!
//! A side question (`session_explain`) is answered on a child session dsh
//! forked from the parent, and the answer must be one model call: the prompt
//! forbids tools, and the plugin's `agent/pre-step` rejects step ≥ 2 as the
//! hard stop behind that. The plugin only knows to do so because this command
//! says `oneShot: true` for the child's id — the marker that identifies a fork
//! lives on Fleet's side (`~/.fleet/explain/forks/<id>`), written before the
//! child's prompt goes in. This drives the real binary because the field is
//! only worth anything end to end, and because the contract with an older
//! plugin is "absent, not false": the flag is a sibling of `sandboxMode`, not a
//! new required argument.

use std::process::Command;

fn dsh_context(fleet_home: &std::path::Path, session: &str) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_fleet-cli"))
        .args([
            "dsh-context",
            "--session",
            session,
            "--title",
            "Boss",
            "--locale",
            "en",
        ])
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
fn only_a_marked_fork_session_is_told_it_is_one_shot() {
    let home = tempfile::tempdir().unwrap();

    // `dsh_fork_ask` leaves exactly this behind the moment `session/fork`
    // answers: an empty marker file named after the child.
    let forks = home.path().join(".fleet").join("explain").join("forks");
    std::fs::create_dir_all(&forks).unwrap();
    std::fs::write(forks.join("child-fork-1"), b"").unwrap();

    let fork = dsh_context(home.path(), "child-fork-1");
    let ordinary = dsh_context(home.path(), "session-ordinary");

    assert_eq!(
        fork.get("oneShot").and_then(|v| v.as_bool()),
        Some(true),
        "a fork's second step must be rejected, so the plugin has to be told"
    );
    assert!(
        ordinary.get("oneShot").is_none(),
        "an ordinary session must not carry the field at all — absence is what \
         an older plugin reads as 'keep running', and a `false` here would be \
         a second spelling of the same thing"
    );
    // The flag rides alongside the sections; a fork still receives its context.
    for payload in [&fork, &ordinary] {
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
