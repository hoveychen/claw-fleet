mod common;

use axum::http::{Method, StatusCode};

async fn create_task(pool: &sqlx::PgPool, key: &str, external: &str) -> serde_json::Value {
    let mut body = common::create_task_body("proj_test", external);
    body["external_id"] = serde_json::json!(external);
    common::post_json(
        common::router(pool.clone()),
        "/tasks",
        common::VALID_KEY,
        Some(key),
        body,
    )
    .await
    .body
}

#[sqlx::test(migrations = "./migrations")]
async fn embed_token_is_origin_view_and_task_scoped_and_expires(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let first = create_task(&pool, "embed-task-001", "embed-1").await;
    let second = create_task(&pool, "embed-task-002", "embed-2").await;
    let first_id = first["task"]["id"].as_str().unwrap();
    let second_id = second["task"]["id"].as_str().unwrap();
    let issued = common::request_json(
        common::router(pool.clone()),
        Method::POST,
        "/embed-tokens",
        common::VALID_KEY,
        &[("idempotency-key", "embed-token-001")],
        Some(serde_json::json!({
            "project_id":"proj_test",
            "task_id":first_id,
            "allowed_origins":["https://fleet-pilot.muveeai.com"],
            "views":["task_detail"],
            "expires_in_seconds":600
        })),
    )
    .await;
    assert_eq!(issued.status, StatusCode::CREATED);
    let token = issued.body["token"].as_str().unwrap();
    assert!(!issued.body["embed_url"].as_str().unwrap().contains(token));
    let stored_hash: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM embed_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_ne!(stored_hash, token.as_bytes());

    let allowed = common::request_json(
        common::router(pool.clone()),
        Method::GET,
        &format!("/tasks/{first_id}"),
        token,
        &[(
            "fleet-embed-parent-origin",
            "https://fleet-pilot.muveeai.com",
        )],
        None,
    )
    .await;
    assert_eq!(allowed.status, StatusCode::OK);

    let wrong_task = common::request_json(
        common::router(pool.clone()),
        Method::GET,
        &format!("/tasks/{second_id}"),
        token,
        &[(
            "fleet-embed-parent-origin",
            "https://fleet-pilot.muveeai.com",
        )],
        None,
    )
    .await;
    assert_eq!(wrong_task.status, StatusCode::FORBIDDEN);
    let wrong_origin = common::request_json(
        common::router(pool.clone()),
        Method::GET,
        &format!("/tasks/{first_id}"),
        token,
        &[("fleet-embed-parent-origin", "https://evil.example")],
        None,
    )
    .await;
    assert_eq!(wrong_origin.status, StatusCode::FORBIDDEN);

    sqlx::query("UPDATE embed_tokens SET expires_at=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    let expired = common::request_json(
        common::router(pool.clone()),
        Method::GET,
        &format!("/tasks/{first_id}"),
        token,
        &[(
            "fleet-embed-parent-origin",
            "https://fleet-pilot.muveeai.com",
        )],
        None,
    )
    .await;
    assert_eq!(expired.status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "./migrations")]
async fn embed_token_rejects_unapproved_origin_at_issue_time(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let issued = common::request_json(
        common::router(pool.clone()),
        Method::POST,
        "/embed-tokens",
        common::VALID_KEY,
        &[("idempotency-key", "embed-token-evil")],
        Some(serde_json::json!({
            "project_id":"proj_test",
            "allowed_origins":["https://evil.example"],
            "views":["decision_inbox"]
        })),
    )
    .await;
    assert_eq!(issued.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "./migrations")]
async fn embed_token_creation_is_idempotent_and_rejects_key_reuse(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let body = serde_json::json!({
        "project_id":"proj_test",
        "allowed_origins":["https://fleet-pilot.muveeai.com"],
        "views":["decision_inbox"]
    });
    let first = common::request_json(
        common::router(pool.clone()),
        Method::POST,
        "/embed-tokens",
        common::VALID_KEY,
        &[("idempotency-key", "embed-idempotent-001")],
        Some(body.clone()),
    )
    .await;
    let replay = common::request_json(
        common::router(pool.clone()),
        Method::POST,
        "/embed-tokens",
        common::VALID_KEY,
        &[("idempotency-key", "embed-idempotent-001")],
        Some(body),
    )
    .await;
    assert_eq!(first.status, StatusCode::CREATED);
    assert_eq!(replay.status, StatusCode::CREATED);
    assert_eq!(first.body, replay.body);

    let mismatch = common::request_json(
        common::router(pool.clone()),
        Method::POST,
        "/embed-tokens",
        common::VALID_KEY,
        &[("idempotency-key", "embed-idempotent-001")],
        Some(serde_json::json!({
            "project_id":"proj_test",
            "allowed_origins":["http://localhost:5173"],
            "views":["decision_inbox"]
        })),
    )
    .await;
    assert_eq!(mismatch.status, StatusCode::CONFLICT);
}
