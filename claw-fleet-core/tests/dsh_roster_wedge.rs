//! A `dsh web` that stalls on `session/list` gets replaced.
//!
//! On 2026-10-03 and 2026-10-04 a long-lived `dsh web` answered every other RPC
//! in milliseconds but never answered `session/list`. The process was alive, so
//! `ensure_alive` saw nothing wrong, and every roster scan ran into the client's
//! 30s timeout for hours. The roster now treats a stalled `session/list` as a
//! wedge and restarts the server.
//!
//! The fixture (`tests/fixtures/fake-dsh.js`) answers `host.describe` at once
//! — so adoption and health checks pass, as they did for the real wedge — and
//! holds `session/list` for longer than the client waits. The restarted fixture
//! reads its delay afresh, so lowering it mid-test models "a fresh server
//! answers fine".

use std::path::PathBuf;
use std::time::{Duration, Instant};

use claw_fleet_core::agent_source::AgentSource;
use claw_fleet_core::dsh_source::DshSource;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-dsh.js")
}

#[test]
fn a_stalled_session_list_restarts_the_server() {
    if claw_fleet_core::process_util::which("node").is_none() {
        eprintln!("skipped: node not on PATH, the dsh fixture cannot run");
        return;
    }
    let fleet_home = tempfile::tempdir().expect("temp fleet home");
    std::env::set_var("FLEET_HOME", fleet_home.path());
    claw_fleet_core::launch_spec::record("session-fake-slow", None, None);
    std::env::set_var("FLEET_DSH_BIN", fixture());
    std::env::set_var("FAKE_DSH_HISTORY_DELAY_MS", "50");
    // Longer than the client's 30s timeout: the call can only end by stalling.
    std::env::set_var("FAKE_DSH_LIST_DELAY_MS", "120000");

    let source = DshSource::new();
    // Boot the fixture without touching the roster.
    let _ = source.get_messages_tail("dsh://session-probe", 1);
    let wedged_token = source
        .server_launch_token()
        .expect("the fixture server is up");

    // Whatever the restart spawns must answer promptly.
    std::env::set_var("FAKE_DSH_LIST_DELAY_MS", "50");

    let started = Instant::now();
    let stalled = source.scan_sessions();
    let stall_took = started.elapsed();
    assert!(stalled.is_empty(), "a stalled roster yields no rows");
    assert!(
        stall_took < Duration::from_secs(90),
        "the stalled scan should end at the client timeout plus a restart, took {stall_took:?}"
    );

    let fresh_token = source
        .server_launch_token()
        .expect("a server is up after the restart");
    assert_ne!(
        fresh_token, wedged_token,
        "the wedged server must have been replaced"
    );

    let started = Instant::now();
    let rows = source.scan_sessions();
    let fresh_took = started.elapsed();
    claw_fleet_core::dsh_source::shutdown();

    assert!(
        fresh_took < Duration::from_secs(5),
        "the replacement answers session/list promptly, took {fresh_took:?}"
    );
    assert!(
        rows.iter().any(|s| s.id.contains("session-fake-slow")),
        "the replacement's roster reaches the scan: {:?}",
        rows.iter().map(|s| &s.id).collect::<Vec<_>>()
    );
}
