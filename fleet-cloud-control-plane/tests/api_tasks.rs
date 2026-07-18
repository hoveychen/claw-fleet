mod common;

use axum::body::Body;
use axum::http::StatusCode;
use axum::http::{Method, Request};
use futures_util::StreamExt;
use tower::ServiceExt;

#[sqlx::test(migrations = "./migrations")]
async fn create_task_writes_one_task_run_event_and_command(pool: sqlx::PgPool) {
    let seed = common::seed(&pool).await;
    let response = common::post_json(
        common::router(pool.clone()),
        "/tasks",
        common::VALID_KEY,
        Some("create-github-123"),
        common::create_task_body(seed.project_id, "Fix issue 123"),
    )
    .await;
    assert_eq!(response.status, StatusCode::ACCEPTED, "{}", response.body);
    assert!(response.body["task"]["id"]
        .as_str()
        .unwrap()
        .starts_with("task_"));
    assert_eq!(response.body["task"]["status"], "queued");
    assert_eq!(response.body["run"]["status"], "assigned");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tasks")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM runs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM commands")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn same_idempotency_key_and_body_returns_original_response(pool: sqlx::PgPool) {
    let seed = common::seed(&pool).await;
    let body = common::create_task_body(seed.project_id, "Fix issue 123");
    let first = common::post_json(
        common::router(pool.clone()),
        "/tasks",
        common::VALID_KEY,
        Some("create-github-123"),
        body.clone(),
    )
    .await;
    let second = common::post_json(
        common::router(pool.clone()),
        "/tasks",
        common::VALID_KEY,
        Some("create-github-123"),
        body,
    )
    .await;
    assert_eq!(first.status, StatusCode::ACCEPTED);
    assert_eq!(second.status, StatusCode::ACCEPTED);
    assert_eq!(first.body, second.body);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tasks")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn same_idempotency_key_with_different_body_is_409(pool: sqlx::PgPool) {
    let seed = common::seed(&pool).await;
    let first = common::post_json(
        common::router(pool.clone()),
        "/tasks",
        common::VALID_KEY,
        Some("create-github-123"),
        common::create_task_body(seed.project_id, "First goal"),
    )
    .await;
    assert_eq!(first.status, StatusCode::ACCEPTED);
    let second = common::post_json(
        common::router(pool),
        "/tasks",
        common::VALID_KEY,
        Some("create-github-123"),
        common::create_task_body(seed.project_id, "Different goal"),
    )
    .await;
    assert_eq!(second.status, StatusCode::CONFLICT);
    assert_eq!(second.body["error"]["code"], "idempotency_mismatch");
}

#[sqlx::test(migrations = "./migrations")]
async fn missing_idempotency_key_is_422(pool: sqlx::PgPool) {
    let seed = common::seed(&pool).await;
    let response = common::post_json(
        common::router(pool),
        "/tasks",
        common::VALID_KEY,
        None,
        common::create_task_body(seed.project_id, "Fix issue 123"),
    )
    .await;
    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(response.body["error"]["code"], "validation_failed");
}

#[sqlx::test(migrations = "./migrations")]
async fn project_id_outside_key_scope_is_403(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let response = common::post_json(
        common::router(pool),
        "/tasks",
        common::VALID_KEY,
        Some("create-other-project"),
        common::create_task_body("proj_other", "Cross project"),
    )
    .await;
    assert_eq!(response.status, StatusCode::FORBIDDEN);
    assert_eq!(response.body["error"]["code"], "permission_denied");
}

async fn create(pool: &sqlx::PgPool, key: &str, external_id: &str) -> serde_json::Value {
    let mut body = common::create_task_body("proj_test", external_id);
    body["external_id"] = serde_json::json!(external_id);
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
async fn task_get_and_cursor_list_are_project_scoped(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let first = create(&pool, "create-list-001", "ticket-1").await;
    let second = create(&pool, "create-list-002", "ticket-2").await;
    let first_id = first["task"]["id"].as_str().unwrap();
    let second_id = second["task"]["id"].as_str().unwrap();

    let page = common::request_json(
        common::router(pool.clone()),
        Method::GET,
        "/tasks?limit=1",
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(page.body["data"].as_array().unwrap().len(), 1);
    assert!(page.body["has_more"].as_bool().unwrap());
    let cursor = page.body["next_cursor"].as_str().unwrap();
    let next = common::request_json(
        common::router(pool.clone()),
        Method::GET,
        &format!("/tasks?limit=1&cursor={cursor}"),
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert_eq!(next.status, StatusCode::OK);
    assert_eq!(next.body["data"].as_array().unwrap().len(), 1);
    assert_ne!(page.body["data"][0]["id"], next.body["data"][0]["id"]);

    let get = common::request_json(
        common::router(pool.clone()),
        Method::GET,
        &format!("/tasks/{first_id}"),
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert_eq!(get.status, StatusCode::OK);
    assert_eq!(get.body["id"], first_id);
    assert_eq!(get.headers["etag"], "\"1\"");

    sqlx::query("INSERT INTO projects(id, organization_id, slug, name) VALUES ('proj_other', 'org_test', 'other', 'Other')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET project_id = 'proj_other' WHERE id = $1")
        .bind(second_id)
        .execute(&pool)
        .await
        .unwrap();
    let hidden = common::request_json(
        common::router(pool),
        Method::GET,
        &format!("/tasks/{second_id}"),
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "./migrations")]
async fn control_requires_matching_version_and_is_idempotent(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let created = create(&pool, "create-control-001", "ticket-control").await;
    let task_id = created["task"]["id"].as_str().unwrap();
    sqlx::query("UPDATE tasks SET status = 'running' WHERE id = $1")
        .bind(task_id)
        .execute(&pool)
        .await
        .unwrap();
    let appended = common::request_json(
        common::router(pool.clone()),
        Method::POST,
        &format!("/tasks/{task_id}/messages"),
        common::VALID_KEY,
        &[("idempotency-key", "append-message-001")],
        Some(serde_json::json!({"text": "please inspect logs"})),
    )
    .await;
    assert_eq!(appended.status, StatusCode::ACCEPTED, "{}", appended.body);

    let uri = format!("/tasks/{task_id}/pause");
    let paused = common::request_json(
        common::router(pool.clone()),
        Method::POST,
        &uri,
        common::VALID_KEY,
        &[
            ("idempotency-key", "pause-control-001"),
            ("if-match", "\"2\""),
        ],
        Some(serde_json::json!({"reason": "maintenance"})),
    )
    .await;
    assert_eq!(paused.status, StatusCode::ACCEPTED, "{}", paused.body);
    let replay = common::request_json(
        common::router(pool.clone()),
        Method::POST,
        &uri,
        common::VALID_KEY,
        &[
            ("idempotency-key", "pause-control-001"),
            ("if-match", "\"2\""),
        ],
        Some(serde_json::json!({"reason": "maintenance"})),
    )
    .await;
    assert_eq!(replay.body, paused.body);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM commands WHERE command_type = 'pause_task'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );

    let stale = common::request_json(
        common::router(pool.clone()),
        Method::POST,
        &format!("/tasks/{task_id}/cancel"),
        common::VALID_KEY,
        &[
            ("idempotency-key", "cancel-control-001"),
            ("if-match", "\"1\""),
        ],
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED);

    let resumed = common::request_json(
        common::router(pool.clone()),
        Method::POST,
        &format!("/tasks/{task_id}/resume"),
        common::VALID_KEY,
        &[
            ("idempotency-key", "resume-control-001"),
            ("if-match", "\"3\""),
        ],
        Some(serde_json::json!({"message": "continue"})),
    )
    .await;
    assert_eq!(resumed.status, StatusCode::ACCEPTED, "{}", resumed.body);
}

#[sqlx::test(migrations = "./migrations")]
async fn event_list_replays_after_sequence_or_cursor(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let created = create(&pool, "create-events-001", "ticket-events").await;
    let task_id = created["task"]["id"].as_str().unwrap();
    common::request_json(
        common::router(pool.clone()),
        Method::POST,
        &format!("/tasks/{task_id}/pause"),
        common::VALID_KEY,
        &[
            ("idempotency-key", "pause-events-001"),
            ("if-match", "\"1\""),
        ],
        Some(serde_json::json!({})),
    )
    .await;
    let events = common::request_json(
        common::router(pool.clone()),
        Method::GET,
        &format!("/tasks/{task_id}/events?after=1"),
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert_eq!(events.status, StatusCode::OK, "{}", events.body);
    assert_eq!(events.body["data"].as_array().unwrap().len(), 1);
    assert_eq!(events.body["data"][0]["type"], "task.pause_requested");
    let cursor = events.body["data"][0]["cursor"].as_str().unwrap();
    let empty = common::request_json(
        common::router(pool),
        Method::GET,
        &format!("/tasks/{task_id}/events?after={cursor}"),
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert!(empty.body["data"].as_array().unwrap().is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn sse_replays_persisted_event_and_honors_cursor(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let created = create(&pool, "create-sse-001", "ticket-sse").await;
    let task_id = created["task"]["id"].as_str().unwrap();
    let request = Request::get(format!("/events/stream?task_id={task_id}&after=0"))
        .header("authorization", format!("Bearer {}", common::VALID_KEY))
        .body(Body::empty())
        .unwrap();
    let response = common::router(pool).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut stream = response.into_body().into_data_stream();
    let chunk = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let frame = std::str::from_utf8(&chunk).unwrap();
    assert!(frame.contains("event: task.created"), "{frame}");
    assert!(frame.contains("\"type\":\"task.created\""), "{frame}");
}
