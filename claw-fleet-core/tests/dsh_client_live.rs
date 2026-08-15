//! Live validation that [`DshClient`] talks to a REAL `dsh web` server, not
//! just to a hand-written envelope fixture.
//!
//! Ignored by default because it needs a running server. Start one and run:
//!   npx -y @deepseek-ai/dsh@latest web --port 3080 &
//!   DSH_PORT=3080 cargo test -p claw-fleet-core --test dsh_client_live -- --ignored --nocapture
//!
//! The three assertions below are exactly the three layers `dsh_client` splits
//! its error type over: a successful unary call, a business-level rejection
//! (`ok:false`), and the loopback trust fence. Passing all three proves the
//! client's envelope handling matches the server's actual wire behavior.

use claw_fleet_core::dsh_client::{DshClient, DshRpcError};
use serde_json::json;

fn client() -> DshClient {
    let port: u16 = std::env::var("DSH_PORT")
        .unwrap_or_else(|_| "3080".into())
        .parse()
        .expect("DSH_PORT must be a port number");
    DshClient::new(port).expect("build client")
}

#[test]
#[ignore = "needs a running `dsh web`; run manually with --ignored"]
fn live_host_describe_returns_cwd_and_model() {
    let value = client()
        .call("host.describe", json!({}))
        .expect("host.describe");
    println!("host.describe -> {value}");
    assert!(
        value.get("cwd").and_then(|v| v.as_str()).is_some(),
        "host.describe must report a cwd: {value}"
    );
    assert!(
        value.get("model").and_then(|v| v.as_str()).is_some(),
        "host.describe must report a model: {value}"
    );
}

#[test]
#[ignore = "needs a running `dsh web`; run manually with --ignored"]
fn live_session_list_is_an_item_array() {
    let value = client()
        .call("session.list", json!({}))
        .expect("session.list");
    assert!(
        value.get("items").and_then(|v| v.as_array()).is_some(),
        "session.list must return an items array: {value}"
    );
}

#[test]
#[ignore = "needs a running `dsh web`; run manually with --ignored"]
fn live_bad_payload_surfaces_rpc_error_not_transport() {
    // `credentials.describe` requires a `refs` array; omitting it is the
    // cheapest read-only way to make the server answer `{"ok":false}`.
    let err = client()
        .call("credentials.describe", json!({}))
        .expect_err("missing refs must be rejected");
    println!("credentials.describe(bad) -> {err}");
    match err {
        DshRpcError::Rpc { code, .. } => assert_eq!(code, "bad-request"),
        other => panic!("expected a business-level Rpc error, got {other:?}"),
    }
}

#[test]
#[ignore = "needs a running `dsh web`; run manually with --ignored"]
fn live_respond_to_unknown_frame_is_not_pending() {
    // Nothing is awaiting this id, so the carrier receipt must refuse it —
    // proving `respond` reads the receipt instead of trusting HTTP 200.
    let err = client()
        .respond(
            "00000000-0000-4000-8000-000000000000",
            json!({ "sessionId": "session-nonexistent", "outcome": "rejected" }),
        )
        .expect_err("an unknown rpcId must be refused");
    println!("respond(unknown) -> {err}");
    assert!(
        matches!(err, DshRpcError::Rejected(_)),
        "expected a carrier rejection, got {err:?}"
    );
}
