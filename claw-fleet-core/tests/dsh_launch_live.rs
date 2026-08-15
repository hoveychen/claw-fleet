//! Live validation of the dsh spawn and resume paths.
//!
//! Ignored by default: each of these runs real turns, which costs real model
//! credits.
//!   FLEET_DSH_BIN=$(ls ~/.npm/_npx/*/node_modules/.bin/dsh | head -1) \
//!   cargo test -p claw-fleet-core --test dsh_launch_live -- --ignored --nocapture --test-threads=1
//!
//! These drive [`DshSource`] through the [`AgentSource`] trait — the same entry
//! point `agent_source::spawn_session` / `resume_session` use — so they cover
//! the process-global server too: the source built here is a throwaway, exactly
//! like the one those dispatchers build per call.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use claw_fleet_core::agent_source::{AgentSource, ResumeSpec, SpawnSpec};
use claw_fleet_core::dsh_source::DshSource;
use claw_fleet_core::session::SessionStatus;

const PROBE_PROMPT: &str = "Reply with exactly: ok";

fn wait_for<T>(budget: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some(v) = f() {
            return Some(v);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

#[test]
#[ignore = "runs a real dsh turn (costs model credits); run manually with --ignored"]
fn live_spawn_creates_the_session_it_promised() {
    let source = DshSource::new();
    let spec = SpawnSpec {
        workspace_path: "/tmp".into(),
        prompt: PROBE_PROMPT.into(),
        ..Default::default()
    };

    let spawned = source.spawn(&spec).expect("spawn");
    let session_id = spawned.session_id.clone().expect("spawn must report an id");
    println!("spawned {session_id} (server pid {})", spawned.pid);

    assert!(
        session_id.starts_with("session-"),
        "dsh ids have their own shape: {session_id}"
    );
    assert!(spawned.pid > 0, "the server's pid must be reported");

    // The promise Fleet makes to its caller: the id it hands back is the session
    // that now exists, findable without guessing which one just appeared.
    let found = wait_for(Duration::from_secs(30), || {
        source.scan_sessions().into_iter().find(|s| s.id == session_id)
    })
    .expect("the spawned session must show up in a scan");

    assert_eq!(found.workspace_path, "/tmp");
    assert_eq!(found.agent_source, "dsh");
    assert_eq!(found.jsonl_path, format!("dsh://{session_id}"));

    // And it must actually be running the prompt we gave it, not sitting blank.
    let worked = wait_for(Duration::from_secs(90), || {
        let s = source.scan_sessions().into_iter().find(|s| s.id == session_id)?;
        (s.status != SessionStatus::Idle).then_some(s.status)
    });
    println!("status after spawn: {worked:?}");
    assert!(worked.is_some(), "the spawned session never started working");
}

/// A caller-supplied id must be honoured end to end — that is what lets Fleet
/// correlate the session it asked for with the one that appears.
#[test]
#[ignore = "runs a real dsh turn (costs model credits); run manually with --ignored"]
fn live_spawn_honours_a_preassigned_id() {
    let source = DshSource::new();
    let wanted = format!("session-{}", uuid::Uuid::new_v4());
    let spawned = source
        .spawn(&SpawnSpec {
            workspace_path: "/tmp".into(),
            prompt: PROBE_PROMPT.into(),
            session_id: Some(wanted.clone()),
            ..Default::default()
        })
        .expect("spawn");
    assert_eq!(spawned.session_id.as_deref(), Some(wanted.as_str()));
}

/// The resume contract: the turn continues the existing session, and `on_exit`
/// fires with the turn's real outcome rather than at admission time.
#[test]
#[ignore = "runs two real dsh turns (costs model credits); run manually with --ignored"]
fn live_resume_continues_a_session_and_reports_its_outcome() {
    let source = DshSource::new();
    let spawned = source
        .spawn(&SpawnSpec {
            workspace_path: "/tmp".into(),
            prompt: "Remember the number 41. Reply with exactly: stored".into(),
            ..Default::default()
        })
        .expect("spawn");
    let session_id = spawned.session_id.expect("id");

    // Let the first turn finish before layering a second one on top.
    let settled = wait_for(Duration::from_secs(120), || {
        let s = source.scan_sessions().into_iter().find(|x| x.id == session_id)?;
        matches!(s.status, SessionStatus::WaitingInput | SessionStatus::Idle).then_some(s.status)
    });
    println!("first turn settled: {settled:?}");

    let (tx, rx) = mpsc::channel();
    source
        .resume(
            &ResumeSpec {
                session_id: session_id.clone(),
                workspace_path: "/tmp".into(),
                prompt: "What number did I ask you to remember? Reply with just the number.".into(),
                ..Default::default()
            },
            Box::new(move |ok| {
                let _ = tx.send(ok);
            }),
        )
        .expect("resume");

    // `session.prompt` returns at admission; `on_exit` must wait for `turn/end`.
    let outcome = rx
        .recv_timeout(Duration::from_secs(180))
        .expect("on_exit must fire when the resumed turn ends");
    assert!(outcome, "a turn that completed must report success");

    // The resumed turn appended to the same session — the history the model
    // answered from is the one Fleet resumed, not a fresh transcript.
    let events = source
        .get_messages(&format!("dsh://{session_id}"))
        .expect("history");
    println!("{} event(s) after the resumed turn", events.len());
    let text = serde_json::to_string(&events).unwrap();
    assert!(
        text.contains("41"),
        "the resumed turn must see the first turn's context"
    );
}

/// Resuming something that does not exist must fail loudly *and* release the
/// caller's `on_exit`, or the auto-resume scheduler would hold its slot forever
/// waiting for a turn that never started.
#[test]
#[ignore = "starts a real `dsh web`; run manually with --ignored"]
fn live_resume_of_an_unknown_session_releases_its_callback() {
    let source = DshSource::new();
    let (tx, rx) = mpsc::channel();
    let err = source
        .resume(
            &ResumeSpec {
                session_id: "session-does-not-exist".into(),
                workspace_path: "/tmp".into(),
                prompt: PROBE_PROMPT.into(),
                ..Default::default()
            },
            Box::new(move |ok| {
                let _ = tx.send(ok);
            }),
        )
        .unwrap_err();
    println!("resume error: {err}");
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)),
        Ok(false),
        "a resume that never started must report failure immediately"
    );
}
