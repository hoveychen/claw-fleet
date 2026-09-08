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
    let token;

    {
        let mut server = DshServer::start(&binary(), &workspace).expect("start dsh web");
        port = server.port();
        println!("dsh web listening on {port}");
        assert!(port > 0, "the OS must have assigned a real port");
        assert!(server.is_alive(), "server must be alive right after start");

        // The health gate already probed `settings/describe`; prove the
        // caller-facing client works too — including the launch-token → cookie
        // exchange it performs at construction.
        let value = server
            .client()
            .expect("client")
            .call("session/list", json!({ "_request": {} }))
            .expect("session/list");
        assert!(value.get("items").and_then(|v| v.as_array()).is_some());
        token = server.launch_token().to_string();
    }

    // Drop killed it: nothing may still answer on that port. Construction is
    // enough to prove it — the token exchange is itself an HTTP round trip, so
    // a dead port fails there rather than at the first call.
    let refused = claw_fleet_core::dsh_client::DshClient::new(port, &token)
        .map(|c| c.call("session/list", json!({ "_request": {} })));
    assert!(
        refused.is_err(),
        "dropping DshServer must leave no unauthenticated port behind (port {port} still answers)"
    );
}

#[test]
#[ignore = "starts two real `dsh web` instances; run manually with --ignored"]
fn live_two_instances_get_distinct_ports() {
    // The first start takes the remembered port (or 0 on a fresh machine); the
    // second finds it busy and falls back to `--port 0`, so two concurrent
    // servers must never collide.
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

    // A restarted listener comes back on the remembered port (dsh writes its
    // GUI URL into every session's system prompt, so a new port would miss the
    // provider's prefix cache on the whole history). Any cached client is
    // still stale — the token is minted per process — which is why `client()`
    // is called per use, not memoized.
    assert_eq!(first_port, server.port());
    server
        .client()
        .expect("client")
        .call("session/list", json!({ "_request": {} }))
        .expect("restarted server must serve RPC");
}
