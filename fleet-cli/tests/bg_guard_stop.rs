//! End-to-end coverage of the Stop-hook background-task guard.
//!
//! `fleet session idle` is the Stop hook. When a *headless* session ends a turn
//! with background tasks still running, it must exit 2 — Claude Code reads that
//! as "don't stop" and feeds stderr back to the agent — because a `claude -p`
//! run kills those shells ~5s after its final result and nothing will ever wake
//! the model to collect them.
//!
//! These tests cover the safety envelope around that block: the hook must stay
//! out of the way in every case where it cannot *prove* the session is a live
//! headless process. The block decision itself is unit-tested in
//! `claw_fleet_core::bg_guard`; reproducing it here would mean faking a live
//! `claude -p` process whose argv names the test's session id.
//!
//! Each test points `FLEET_HOME` at its own tempdir, so idle markers land there
//! instead of the developer's `~/.fleet`.

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn bin_path(name: &str) -> PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // drop test exe name
    if p.ends_with("deps") {
        p.pop();
    }
    p.push(name);
    p
}

const SID: &str = "sess-bg-guard-test";

/// A Stop payload with two background shells still running — the exact shape
/// that killed the scene-items relay session.
const RUNNING_TASKS_PAYLOAD: &str = r#"{
    "session_id": "sess-bg-guard-test",
    "hook_event_name": "Stop",
    "stop_hook_active": false,
    "background_tasks": [
        {"id":"bqkeuowy9","type":"shell","status":"running",
         "description":"Wait for prod to serve new GIT_SHA","command":"until ...; do sleep 30; done"}
    ],
    "session_crons": []
}"#;

/// Run `fleet session idle` with `stdin` piped (as a real hook invocation does).
fn run_idle(home: &std::path::Path, sid: Option<&str>, stdin_json: &str) -> std::process::Output {
    let mut cmd = Command::new(bin_path("fleet-cli"));
    cmd.args(["session", "idle"])
        .env("FLEET_HOME", home)
        .env_remove("FLEET_SESSION_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(s) = sid {
        cmd.env("FLEET_SESSION_ID", s);
    }

    let mut child = cmd.spawn().expect("spawn fleet session idle");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(stdin_json.as_bytes())
            .unwrap();
    }
    child.wait_with_output().expect("wait for fleet session idle")
}

/// The load-bearing safety property. No live `claude -p` process names this
/// session id — the test harness is not one — so the guard cannot prove the
/// session is headless and must let the turn end.
///
/// Getting this backwards would wedge every interactive session on the machine:
/// a VS Code session keeps its background shells alive across turns and *is*
/// re-invoked when they finish, so blocking it would be a false alarm the user
/// has no way to clear.
#[test]
fn does_not_block_when_the_session_is_not_a_live_headless_process() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();

    let out = run_idle(&home, Some(SID), RUNNING_TASKS_PAYLOAD);

    assert!(
        out.status.success(),
        "must not block a session it can't prove is headless (exit {:?}): {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    // And the turn really ended, so the card must flip to idle.
    assert!(
        home.join(".fleet").join("idle").join(format!("{SID}.json")).exists(),
        "a non-blocked stop must still mark the session idle"
    );
}

/// A clean stop (no background work) is the common case — never touched.
#[test]
fn clean_stop_marks_idle_and_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();

    let out = run_idle(
        &home,
        Some(SID),
        r#"{"session_id":"sess-bg-guard-test","hook_event_name":"Stop","background_tasks":[]}"#,
    );

    assert!(out.status.success(), "clean stop must exit 0");
    assert!(home.join(".fleet").join("idle").join(format!("{SID}.json")).exists());
}

/// Hooks must never fail a session over a payload they can't read — an older CLI
/// sends no `background_tasks`, and a malformed line must not take the turn down.
#[test]
fn unreadable_stdin_does_not_fail_the_hook() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();

    for payload in ["", "not json at all", r#"{"hook_event_name":"Stop"}"#] {
        let out = run_idle(&home, Some(SID), payload);
        assert!(
            out.status.success(),
            "payload {payload:?} must not fail the hook (exit {:?})",
            out.status.code()
        );
    }
}

/// No session id (user has Fleet's global hooks but ran claude outside the
/// supervisor) — exit 0 without touching anything.
#[test]
fn missing_session_id_exits_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();

    let out = run_idle(&home, None, RUNNING_TASKS_PAYLOAD);

    assert!(out.status.success(), "no session id must exit 0");
    assert!(
        !home.join(".fleet").join("idle").exists(),
        "no session id must not create idle markers"
    );
}
