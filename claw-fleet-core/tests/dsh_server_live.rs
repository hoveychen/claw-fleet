//! Live validation that [`DshServer`] can actually run a `dsh web` instance:
//! spawn it, learn the OS-assigned port, health-check it, talk RPC to it, and
//! kill it on drop.
//!
//! Ignored by default because it starts a real server. Point it at a `dsh`
//! executable and run:
//!   DSH_BIN=$(ls ~/.npm/_npx/*/node_modules/.bin/dsh | head -1) \
//!   cargo test -p claw-fleet-core --test dsh_server_live -- --ignored --nocapture
//!
//! The unit tests next to `dsh_server.rs` only cover the stdout parser; this is
//! the one that proves the lifecycle works against the real launcher.

use std::path::PathBuf;

use claw_fleet_core::dsh_server::DshServer;
use serde_json::json;

fn binary() -> PathBuf {
    PathBuf::from(std::env::var("DSH_BIN").expect("set DSH_BIN to a dsh executable"))
}

#[test]
#[ignore = "starts a real `dsh web`; run manually with --ignored"]
fn live_start_serves_rpc_then_stops_on_drop() {
    let workspace = std::env::temp_dir();
    let port;

    {
        let mut server = DshServer::start(&binary(), &workspace).expect("start dsh web");
        port = server.port();
        println!("dsh web listening on {port}");
        assert!(port > 0, "the OS must have assigned a real port");
        assert!(server.is_alive(), "server must be alive right after start");

        // The health gate already called host.describe; prove the caller-facing
        // client works too, and that the server rooted itself where we asked.
        let value = server
            .client()
            .expect("client")
            .call("host.describe", json!({}))
            .expect("host.describe");
        println!("host.describe -> {value}");
        assert!(value.get("cwd").and_then(|v| v.as_str()).is_some());
    }

    // Drop killed it: nothing may still answer on that port.
    let refused = claw_fleet_core::dsh_client::DshClient::new(port)
        .expect("client")
        .call("host.describe", json!({}));
    assert!(
        refused.is_err(),
        "dropping DshServer must leave no unauthenticated port behind (port {port} still answers)"
    );
}

#[test]
#[ignore = "starts two real `dsh web` instances; run manually with --ignored"]
fn live_two_instances_get_distinct_ports() {
    // `--port 0` exists precisely so Fleet never has to pick a port; two
    // concurrent workspaces must not collide.
    let a = DshServer::start(&binary(), &std::env::temp_dir()).expect("start a");
    let b = DshServer::start(&binary(), &std::env::temp_dir()).expect("start b");
    println!("ports: {} and {}", a.port(), b.port());
    assert_ne!(a.port(), b.port());
}

#[test]
#[ignore = "starts and kills a real `dsh web`; run manually with --ignored"]
fn live_ensure_alive_restarts_a_killed_server() {
    let mut server = DshServer::start(&binary(), &std::env::temp_dir()).expect("start");
    let first_port = server.port();

    server.stop();
    assert!(!server.is_alive(), "stop() must reap the child");

    server.ensure_alive().expect("restart after crash");
    assert!(server.is_alive(), "ensure_alive must bring it back");
    println!("restarted: {first_port} -> {}", server.port());

    // A restarted listener gets a fresh OS-assigned port, so any cached client
    // is stale — this is why `client()` is called per use, not memoized.
    assert_ne!(first_port, server.port());
    server
        .client()
        .expect("client")
        .call("host.describe", json!({}))
        .expect("restarted server must serve RPC");
}
