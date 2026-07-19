use fleet_cloud_control_plane::github_adapter::{
    parse_labeled_issue, status_label, verify_github_signature,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use axum::body::{to_bytes, Body};
use axum::http::Request;
use axum::{extract::State, routing::post, Json, Router};
use fleet_cloud_control_plane::github_adapter::{webhook_router, FleetApiClient, GithubWebhookState};
use std::sync::Arc;
use std::future::IntoFuture;
use tokio::sync::mpsc;
use tower::ServiceExt;

fn signature(secret: &[u8], body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[test]
fn github_signature_must_match_raw_request_body() {
    let body = br#"{"action":"labeled"}"#;
    let header = signature(b"github-webhook-secret", body);
    assert!(verify_github_signature(b"github-webhook-secret", body, &header).is_ok());
    assert!(verify_github_signature(b"github-webhook-secret", b"{}", &header).is_err());
    assert!(verify_github_signature(b"github-webhook-secret", body, "sha1=legacy").is_err());
}

#[test]
fn fleet_task_label_maps_real_issue_to_public_task_contract() {
    let payload = serde_json::json!({
        "action": "labeled",
        "label": {"name": "fleet-task"},
        "repository": {
            "full_name": "hoveychen/claw-fleet",
            "clone_url": "https://github.com/hoveychen/claw-fleet.git",
            "default_branch": "main"
        },
        "issue": {
            "number": 123,
            "title": "Fix duplicate Runner events",
            "body": "Reproduce the duplicate and add a regression test.",
            "html_url": "https://github.com/hoveychen/claw-fleet/issues/123"
        }
    });
    let action = parse_labeled_issue(&payload, "hoveychen/claw-fleet", "proj_fleet_cloud_pilot")
        .unwrap()
        .expect("fleet-task label should create a task");
    assert_eq!(action.issue_number, 123);
    assert_eq!(action.external_id, "github:hoveychen/claw-fleet#123");
    assert_eq!(action.task.project_id, "proj_fleet_cloud_pilot");
    assert_eq!(action.task.title.as_deref(), Some("Fix duplicate Runner events"));
    assert_eq!(action.task.workspace.repository, "https://github.com/hoveychen/claw-fleet.git");
    assert_eq!(action.task.workspace.r#ref, "main");
    assert!(action.task.goal.contains("Reproduce the duplicate"));
    assert_eq!(action.task.metadata["issue_number"], 123);
}

#[test]
fn unrelated_labels_and_repositories_are_ignored() {
    let mut payload = serde_json::json!({
        "action": "labeled",
        "label": {"name": "bug"},
        "repository": {
            "full_name": "hoveychen/claw-fleet",
            "clone_url": "https://github.com/hoveychen/claw-fleet.git",
            "default_branch": "main"
        },
        "issue": {"number": 1, "title": "x", "body": null, "html_url": "https://example.test/1"}
    });
    assert!(parse_labeled_issue(&payload, "hoveychen/claw-fleet", "proj").unwrap().is_none());
    payload["label"]["name"] = "fleet-task".into();
    assert!(parse_labeled_issue(&payload, "other/repository", "proj").unwrap().is_none());
}

#[test]
fn task_statuses_map_to_frozen_github_labels() {
    for (status, label) in [
        ("queued", "fleet:queued"),
        ("running", "fleet:running"),
        ("waiting_input", "fleet:waiting"),
        ("paused", "fleet:waiting"),
        ("succeeded", "fleet:succeeded"),
        ("failed", "fleet:failed"),
        ("cancelled", "fleet:cancelled"),
    ] {
        assert_eq!(status_label(status).unwrap(), label);
    }
    assert!(status_label("unknown").is_none());
}

#[derive(Clone)]
struct FleetCapture(mpsc::Sender<(String, String, serde_json::Value)>);

async fn capture_fleet_task(
    State(capture): State<FleetCapture>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    capture
        .0
        .send((
            headers["authorization"].to_str().unwrap().to_owned(),
            headers["idempotency-key"].to_str().unwrap().to_owned(),
            body,
        ))
        .await
        .unwrap();
    Json(serde_json::json!({
        "task": {"id":"task_github_123","external_id":"github:hoveychen/claw-fleet#123","status":"queued","title":"Fix duplicate Runner events"},
        "run": {"id":"run_123"}
    }))
}

#[tokio::test]
async fn signed_webhook_calls_only_public_task_api_with_delivery_idempotency() {
    let (tx, mut rx) = mpsc::channel(2);
    let fleet_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fleet_addr = fleet_listener.local_addr().unwrap();
    tokio::spawn(
        axum::serve(
            fleet_listener,
            Router::new()
                .route("/tasks", post(capture_fleet_task))
                .with_state(FleetCapture(tx)),
        )
        .into_future(),
    );
    let fleet = FleetApiClient::new(
        format!("http://{fleet_addr}"),
        "proj_fleet_cloud_pilot".into(),
        "flk_pilot_fixture".into(),
    )
    .unwrap();
    let app = webhook_router(GithubWebhookState {
        webhook_secret: Arc::new(b"github-webhook-secret".to_vec()),
        repository: Arc::new("hoveychen/claw-fleet".into()),
        project_id: Arc::new("proj_fleet_cloud_pilot".into()),
        fleet,
    });
    let payload = serde_json::json!({
        "action": "labeled",
        "label": {"name": "fleet-task"},
        "repository": {"full_name": "hoveychen/claw-fleet", "clone_url": "https://github.com/hoveychen/claw-fleet.git", "default_branch": "main"},
        "issue": {"number": 123, "title": "Fix duplicate Runner events", "body": "Add a regression test.", "html_url": "https://github.com/hoveychen/claw-fleet/issues/123"}
    });
    let raw = serde_json::to_vec(&payload).unwrap();
    let response = app
        .oneshot(
            Request::post("/github/webhook")
                .header("x-hub-signature-256", signature(b"github-webhook-secret", &raw))
                .header("x-github-event", "issues")
                .header("x-github-delivery", "delivery-fixture-123")
                .header("content-type", "application/json")
                .body(Body::from(raw))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let response_body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&response_body).unwrap()["task_id"], "task_github_123");
    let (auth, idempotency, task) = rx.recv().await.unwrap();
    assert_eq!(auth, "Bearer flk_pilot_fixture");
    assert_eq!(idempotency, "github-delivery-delivery-fixture-123");
    assert_eq!(task["external_id"], "github:hoveychen/claw-fleet#123");
    assert_eq!(task["project_id"], "proj_fleet_cloud_pilot");
}
