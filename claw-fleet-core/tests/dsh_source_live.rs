//! Live validation that [`DshSource`] really lists and reads dsh sessions
//! through a server it starts itself.
//!
//! Ignored by default because it starts a real `dsh web`. Run:
//!   FLEET_DSH_BIN=$(ls ~/.npm/_npx/*/node_modules/.bin/dsh | head -1) \
//!   cargo test -p claw-fleet-core --test dsh_source_live -- --ignored --nocapture
//!
//! The unit tests beside `dsh_source.rs` map a recorded wire payload; this one
//! proves the payload still looks like that, that the source starts its own
//! server without help, and that it hands back the durable events of a session
//! written by a *different* dsh profile.

use claw_fleet_core::agent_source::AgentSource;
use claw_fleet_core::dsh_source::DshSource;

/// Stops Fleet's process-global `dsh web` when the test that started it ends.
///
/// The server outlives every `DshSource` on purpose (see `dsh_source::SERVER`),
/// so a test binary that starts one and exits leaves it running — measured: one
/// orphaned `dsh web` after a run of this file. `dsh web` has no authentication
/// layer, so an orphan is an open door onto every session on the machine.
struct ServerGuard;

impl Drop for ServerGuard {
    fn drop(&mut self) {
        claw_fleet_core::dsh_source::shutdown();
    }
}

#[test]
#[ignore = "starts a real `dsh web`; run manually with --ignored"]
fn live_scan_lists_sessions_with_a_self_started_server() {
    let _guard = ServerGuard;
    let source = DshSource::new();
    assert!(
        source.is_available(),
        "set FLEET_DSH_BIN to a dsh executable"
    );

    let sessions = source.scan_sessions();
    println!("scanned {} dsh session(s)", sessions.len());
    for s in sessions.iter().take(5) {
        println!(
            "  {} | ws={} | title={:?} | in={} out={}",
            s.id, s.workspace_path, s.ai_title, s.total_input_tokens, s.total_output_tokens
        );
    }

    assert!(
        !sessions.is_empty(),
        "expected at least one session in ~/.dsh/sessions"
    );
    for s in &sessions {
        assert_eq!(s.agent_source, "dsh");
        assert!(
            s.jsonl_path.starts_with("dsh://"),
            "uri must carry the scheme: {}",
            s.jsonl_path
        );
        assert!(source.owns_path(&s.jsonl_path));
    }
}

#[test]
#[ignore = "starts a real `dsh web`; run manually with --ignored"]
fn live_history_returns_durable_events() {
    let _guard = ServerGuard;
    let source = DshSource::new();
    assert!(
        source.is_available(),
        "set FLEET_DSH_BIN to a dsh executable"
    );

    let sessions = source.scan_sessions();
    let target = sessions.first().expect("at least one session").clone();
    println!("reading history of {}", target.jsonl_path);

    let events = source
        .get_messages_tail(&target.jsonl_path, 3)
        .expect("history");
    println!("got {} event(s)", events.len());
    assert!(!events.is_empty(), "a non-blank session must have events");

    // Every entry must be a durable SessionEvent: typed and sequenced. The
    // transient host-computed tool `view` must not have leaked in.
    for e in &events {
        assert!(e.get("type").and_then(|v| v.as_str()).is_some(), "{e}");
        assert!(e.get("seq").and_then(|v| v.as_u64()).is_some(), "{e}");
        assert!(e.get("view").is_none(), "view is not durable: {e}");
    }
}

#[test]
#[ignore = "starts a real `dsh web`; run manually with --ignored"]
fn live_bad_uri_is_rejected_without_touching_the_server() {
    let source = DshSource::new();
    let err = source.get_messages("codex://not-ours").unwrap_err();
    assert!(err.contains("invalid dsh URI"), "{err}");
}
