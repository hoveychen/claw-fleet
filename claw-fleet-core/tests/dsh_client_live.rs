//! Live validation that [`DshClient`] talks to a REAL `dsh web` server, not
//! just to a hand-written envelope fixture.
//!
//! Ignored by default because it needs a running server. Start one and run:
//!   dsh web --port 3080 --no-open &
//!   # the URL it prints carries the launch token: http://127.0.0.1:3080/?token=…
//!   DSH_PORT=3080 DSH_LAUNCH_TOKEN=<token> \
//!     cargo test -p claw-fleet-core --test dsh_client_live -- --ignored --nocapture
//!
//! The four assertions below are exactly the layers `dsh_client` splits its
//! error type over: a successful unary call, a business-level rejection
//! (`ok:false`), the endpoint-shape fence, and the waterfall answer path.
//! Passing them proves the client's envelope handling matches the server's
//! actual wire behavior.

use claw_fleet_core::dsh_client::{DshClient, DshRpcError};
use serde_json::json;

fn client() -> DshClient {
    let port: u16 = std::env::var("DSH_PORT")
        .unwrap_or_else(|_| "3080".into())
        .parse()
        .expect("DSH_PORT must be a port number");
    // Since 0.1.2 a port is not enough: `/api` admits only the cookie this
    // token buys, and the token exists solely in the line the server printed.
    let launch_token = std::env::var("DSH_LAUNCH_TOKEN")
        .expect("DSH_LAUNCH_TOKEN must be the token from the `dsh web` URL");
    DshClient::new(port, &launch_token).expect("build client")
}

#[test]
#[ignore = "needs a running `dsh web`; run manually with --ignored"]
fn live_settings_describe_reports_namespaces() {
    // `host.describe` is gone in 0.1.2 — the whole `host.*` service was split
    // up — and `settings/describe` is what Fleet polls for readiness instead.
    let value = client()
        .call("settings/describe", json!({}))
        .expect("settings/describe");
    assert!(
        value.get("namespaces").and_then(|v| v.as_array()).is_some(),
        "settings/describe must report namespaces: {value}"
    );
}

#[test]
#[ignore = "needs a running `dsh web`; run manually with --ignored"]
fn live_session_list_is_an_item_array() {
    let value = client()
        .call("session/list", json!({ "_request": {} }))
        .expect("session/list");
    assert!(
        value.get("items").and_then(|v| v.as_array()).is_some(),
        "session/list must return an items array: {value}"
    );
}

#[test]
#[ignore = "needs a running `dsh web`; run manually with --ignored"]
fn live_bad_payload_surfaces_rpc_error_not_transport() {
    // `credentials/describe` requires a `refs` array; omitting it is the
    // cheapest read-only way to make the server answer `{"ok":false}`.
    let err = client()
        .call("credentials/describe", json!({}))
        .expect_err("missing refs must be rejected");
    println!("credentials/describe(bad) -> {err}");
    match err {
        DshRpcError::Rpc { code, .. } => {
            assert!(
                code.starts_with("gateway/"),
                "argument validation is the gateway's, got {code}"
            )
        }
        other => panic!("expected a business-level Rpc error, got {other:?}"),
    }
}

/// The dotted 0.1.1 spelling is not a soft fallback: the gateway requires an
/// endpoint of exactly two slash-separated segments, so `session.list` never
/// reaches dispatch and comes back as a transport-level 404.
#[test]
#[ignore = "needs a running `dsh web`; run manually with --ignored"]
fn live_a_dotted_endpoint_is_a_404() {
    let err = client()
        .call("session.list", json!({ "_request": {} }))
        .expect_err("a dotted endpoint must not resolve");
    match err {
        DshRpcError::Transport(msg) => assert!(msg.contains("404"), "expected a 404, got {msg}"),
        other => panic!("expected a transport 404, got {other:?}"),
    }
}

/// Nothing is awaiting this event, so the answer must be refused — proving the
/// client reads the server's verdict instead of trusting HTTP 200.
#[test]
#[ignore = "needs a running `dsh web`; run manually with --ignored"]
fn live_answering_an_unknown_event_is_refused() {
    let err = client()
        .decide_event(
            "00000000-0000-4000-8000-000000000000",
            "00000000-0000-4000-8000-000000000001",
            json!("rejected"),
        )
        .expect_err("an unknown event id must be refused");
    println!("$events/result(unknown) -> {err}");
    assert!(
        matches!(err, DshRpcError::Rpc { .. }),
        "expected a business-level refusal, got {err:?}"
    );
}
