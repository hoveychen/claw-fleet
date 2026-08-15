//! Live validation that [`DshEventWatcher`] really follows a running `dsh web`
//! and derives session phases from what it pushes.
//!
//! Ignored by default: these run a real turn, which costs real model credits.
//!   DSH_BIN=$(ls ~/.npm/_npx/*/node_modules/.bin/dsh | head -1) \
//!   FLEET_DSH_BIN=$DSH_BIN \
//!   cargo test -p claw-fleet-core --test dsh_events_live -- --ignored --nocapture --test-threads=1
//!
//! The unit tests beside `dsh_events.rs` prove the frame decoder against frames
//! captured off the wire. This file proves the other half: that the sockets
//! exist at those paths, accept a parameterless connection, and push the frames
//! the decoder expects while an actual turn runs.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use claw_fleet_core::agent_source::AgentSource;
use claw_fleet_core::dsh_events::DshEventWatcher;
use claw_fleet_core::dsh_server::DshServer;
use claw_fleet_core::dsh_source::DshSource;
use claw_fleet_core::session::SessionStatus;
use serde_json::json;

fn binary() -> PathBuf {
    PathBuf::from(std::env::var("DSH_BIN").expect("set DSH_BIN to a dsh executable"))
}

/// A prompt cheap enough to be worth running in a test, but one that still
/// drives a full turn through the tool path.
const PROBE_PROMPT: &str = "Run the shell command: echo fleet-events-live";

/// Poll `f` until it yields a value or the budget runs out.
fn wait_for<T>(budget: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some(v) = f() {
            return Some(v);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

#[test]
#[ignore = "runs a real dsh turn (costs model credits); run manually with --ignored"]
fn live_watcher_reports_the_phases_of_a_real_turn() {
    let server = DshServer::start(&binary(), &std::env::temp_dir()).expect("start dsh web");
    let watcher = DshEventWatcher::start(server.port());
    let client = server.client().expect("client");

    // Give both downlinks a moment to finish their handshakes before the turn
    // starts — a frame published before we are connected is simply not resent.
    std::thread::sleep(Duration::from_secs(2));

    let session_id = format!("session-{}", uuid::Uuid::new_v4());
    let created = client
        .call(
            "session.create",
            json!({ "cwd": "/tmp", "sessionId": session_id }),
        )
        .expect("session.create");
    assert_eq!(
        created.get("sessionId").and_then(|v| v.as_str()),
        Some(session_id.as_str()),
        "dsh must honour the pre-assigned id: {created}"
    );

    client
        .call(
            "session.prompt",
            json!({
                "sessionId": session_id,
                "mode": "queue",
                "content": [{ "type": "text", "text": PROBE_PROMPT }],
            }),
        )
        .expect("session.prompt");

    // The turn is in flight: the watcher must report *some* phase, and it must
    // be a working one — not the Idle default it would show if no frame landed.
    let mut seen: Vec<SessionStatus> = Vec::new();
    let working = wait_for(Duration::from_secs(60), || {
        let phase = watcher.phase_of(&session_id)?;
        if seen.last() != Some(&phase) {
            println!("phase -> {phase:?}");
            seen.push(phase.clone());
        }
        matches!(
            phase,
            SessionStatus::Processing
                | SessionStatus::Executing
                | SessionStatus::Streaming
                | SessionStatus::Thinking
        )
        .then_some(phase)
    });
    assert!(
        working.is_some(),
        "watcher saw no working phase in 60s (seen: {seen:?})"
    );

    // …and when the turn ends, the last frame must move it to WaitingInput.
    let settled = wait_for(Duration::from_secs(120), || {
        let phase = watcher.phase_of(&session_id)?;
        if seen.last() != Some(&phase) {
            println!("phase -> {phase:?}");
            seen.push(phase.clone());
        }
        (phase == SessionStatus::WaitingInput).then_some(phase)
    });
    assert!(
        settled.is_some(),
        "turn never settled to WaitingInput (seen: {seen:?})"
    );

    println!("phase sequence: {seen:?}");
    assert!(
        seen.contains(&SessionStatus::Executing),
        "the probe prompt runs a shell tool, so Executing must appear: {seen:?}"
    );
    assert!(
        watcher.tracked() >= 1,
        "the watcher must have tracked at least the probe session"
    );
}

/// A session the sockets have said nothing about must not be given a phase —
/// otherwise `scan_sessions` would overwrite the polled status with a default.
#[test]
#[ignore = "starts a real `dsh web`; run manually with --ignored"]
fn live_unknown_session_has_no_phase() {
    let server = DshServer::start(&binary(), &std::env::temp_dir()).expect("start dsh web");
    let watcher = DshEventWatcher::start(server.port());
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(watcher.phase_of("session-does-not-exist"), None);
}

/// The whole point of the overlay: a source scan must carry the pushed phase,
/// not just the `running` bit `session.list` reports.
#[test]
#[ignore = "runs a real dsh turn (costs model credits); run manually with --ignored"]
fn live_scan_sessions_carries_the_pushed_phase() {
    // Nothing else reclaims the process-global server when a test binary exits.
    struct ServerGuard;
    impl Drop for ServerGuard {
        fn drop(&mut self) {
            claw_fleet_core::dsh_source::shutdown();
        }
    }
    let _guard = ServerGuard;

    // `DshSource` starts (and shares) Fleet's server, so this exercises the
    // whole desktop wiring: with_client → ensure_watcher → overlay.
    let source = DshSource::new();
    // The first scan is what starts the server and the watcher.
    let before = source.scan_sessions();
    let port = source
        .server_port()
        .expect("the first scan must have started a server");
    println!("{} session(s) before the probe; server on {port}", before.len());

    // Drive the turn through *that* server: both downlinks are scoped to the
    // process that runs the turn, so a helper server's turn would be invisible
    // here (measured: an observer instance sees neither frames nor `running`).
    let client = claw_fleet_core::dsh_client::DshClient::new(port).expect("client");
    let session_id = format!("session-{}", uuid::Uuid::new_v4());
    client
        .call(
            "session.create",
            json!({ "cwd": "/tmp", "sessionId": session_id }),
        )
        .expect("session.create");
    client
        .call(
            "session.prompt",
            json!({
                "sessionId": session_id,
                "mode": "queue",
                "content": [{ "type": "text", "text": PROBE_PROMPT }],
            }),
        )
        .expect("session.prompt");

    let overlaid = wait_for(Duration::from_secs(90), || {
        let found = source
            .scan_sessions()
            .into_iter()
            .find(|s| s.id == session_id)?;
        // Idle and Active are the two statuses the *poll alone* can produce;
        // anything else can only have come off the socket.
        (found.status != SessionStatus::Idle && found.status != SessionStatus::Active)
            .then_some(found.status)
    });

    assert!(
        overlaid.is_some(),
        "scan_sessions never reported a pushed phase for {session_id}"
    );
    println!("overlaid phase: {overlaid:?}");
}
