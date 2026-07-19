mod common;

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr};

use axum::http::{Method, StatusCode};
use fleet_cloud_control_plane::services::webhooks::{self, DeliveryRequest, WebhookTransport};

struct SequenceTransport {
    statuses: VecDeque<u16>,
    requests: Vec<DeliveryRequest>,
}

impl WebhookTransport for SequenceTransport {
    async fn post(&mut self, request: DeliveryRequest) -> Result<u16, String> {
        self.requests.push(request);
        Ok(self.statuses.pop_front().unwrap())
    }
}

#[test]
fn signature_fixture_is_stable() {
    let body = br#"{"id":"evt_fixture","type":"task.completed"}"#;
    assert_eq!(
        webhooks::sign_payload(b"whsec_fixture_secret", 1_784_361_791, body).unwrap(),
        "v1=90cbf48ab92661e643cead090b237867014061c26090f4245ebe1e45f7ac9f49"
    );
}

#[tokio::test]
async fn webhook_url_policy_rejects_ssrf_targets() {
    let public = |_host: String, _port: u16| async {
        Ok::<_, String>(vec![IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))])
    };
    assert!(
        webhooks::validate_webhook_url_with("https://hooks.example.test/fleet", public)
            .await
            .is_ok()
    );

    for url in [
        "http://hooks.example.test/fleet",
        "https://127.0.0.1/fleet",
        "https://169.254.169.254/latest/meta-data",
        "https://[::1]/fleet",
    ] {
        assert!(
            webhooks::validate_webhook_url_with(url, public)
                .await
                .is_err(),
            "{url}"
        );
    }

    let metadata = |_host: String, _port: u16| async {
        Ok::<_, String>(vec![IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))])
    };
    assert!(
        webhooks::validate_webhook_url_with("https://metadata.internal/fleet", metadata)
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn event_delivery_retries_three_times_then_succeeds(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let created = common::post_json(
        common::router(pool.clone()),
        "/webhook-endpoints",
        common::VALID_KEY,
        Some("webhook-create-001"),
        serde_json::json!({
            "project_id": "proj_test",
            "url": "https://hooks.example.com/fleet",
            "event_types": ["task.created"],
            "description": "ticket callback"
        }),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert!(created.body["signing_secret"]
        .as_str()
        .unwrap()
        .starts_with("whsec_"));
    let endpoint_id = created.body["endpoint"]["id"].as_str().unwrap();

    let task = common::post_json(
        common::router(pool.clone()),
        "/tasks",
        common::VALID_KEY,
        Some("webhook-task-001"),
        common::create_task_body("proj_test", "emit a webhook"),
    )
    .await;
    assert_eq!(task.status, StatusCode::ACCEPTED);
    let (delivery_id, event_id): (String, String) =
        sqlx::query_as("SELECT id,event_id FROM webhook_deliveries WHERE endpoint_id=$1")
            .bind(endpoint_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    let mut transport = SequenceTransport {
        statuses: VecDeque::from([500, 500, 500, 204]),
        requests: Vec::new(),
    };
    for _ in 0..4 {
        webhooks::process_delivery(&pool, common::PEPPER, &delivery_id, &mut transport)
            .await
            .unwrap();
        sqlx::query("UPDATE webhook_deliveries SET next_attempt_at=now() WHERE id=$1")
            .bind(&delivery_id)
            .execute(&pool)
            .await
            .unwrap();
    }
    let row: (String, i32) =
        sqlx::query_as("SELECT status,attempt_count FROM webhook_deliveries WHERE id=$1")
            .bind(&delivery_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row, ("delivered".into(), 4));
    assert_eq!(transport.requests.len(), 4);
    assert!(transport.requests.iter().all(|request| {
        request.headers.get("Fleet-Event-Id") == Some(&event_id)
            && request.headers.get("Fleet-Delivery-Id") == Some(&delivery_id)
    }));

    let log = common::request_json(
        common::router(pool.clone()),
        Method::GET,
        &format!("/webhook-deliveries?endpoint_id={endpoint_id}"),
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert_eq!(log.status, StatusCode::OK);
    assert_eq!(log.body["data"][0]["id"], delivery_id);
    assert!(log.body.to_string().find("whsec_").is_none());

    let replay = common::request_json(
        common::router(pool.clone()),
        Method::POST,
        &format!("/webhook-deliveries/{delivery_id}/replay"),
        common::VALID_KEY,
        &[("idempotency-key", "webhook-replay-001")],
        None,
    )
    .await;
    assert_eq!(replay.status, StatusCode::CREATED, "{}", replay.body);
    assert_ne!(replay.body["id"], delivery_id);
    assert_eq!(replay.body["event_id"], event_id);
}

#[sqlx::test(migrations = "./migrations")]
async fn webhook_crud_is_project_scoped_versioned_and_secret_safe(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let created = common::post_json(
        common::router(pool.clone()),
        "/webhook-endpoints",
        common::VALID_KEY,
        Some("webhook-create-002"),
        serde_json::json!({
            "project_id": "proj_test",
            "url": "https://hooks.example.com/original",
            "event_types": ["*"],
            "description": "all events"
        }),
    )
    .await;
    let id = created.body["endpoint"]["id"].as_str().unwrap();
    let listed = common::request_json(
        common::router(pool.clone()),
        Method::GET,
        "/webhook-endpoints",
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK);
    assert!(listed.body.to_string().find("whsec_").is_none());

    let updated = common::request_json(
        common::router(pool.clone()),
        Method::PATCH,
        &format!("/webhook-endpoints/{id}"),
        common::VALID_KEY,
        &[
            ("idempotency-key", "webhook-update-001"),
            ("if-match", "\"1\""),
        ],
        Some(serde_json::json!({"enabled": false})),
    )
    .await;
    assert_eq!(updated.status, StatusCode::OK, "{}", updated.body);
    assert_eq!(updated.body["version"], 2);

    let rotated = common::request_json(
        common::router(pool.clone()),
        Method::POST,
        &format!("/webhook-endpoints/{id}/rotate-secret"),
        common::VALID_KEY,
        &[("idempotency-key", "webhook-rotate-001")],
        None,
    )
    .await;
    assert_eq!(rotated.status, StatusCode::OK, "{}", rotated.body);
    assert!(rotated.body["signing_secret"]
        .as_str()
        .unwrap()
        .starts_with("whsec_"));

    let deleted = common::request_json(
        common::router(pool),
        Method::DELETE,
        &format!("/webhook-endpoints/{id}"),
        common::VALID_KEY,
        &[("idempotency-key", "webhook-delete-001")],
        None,
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
}
