mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use chrono::Timelike;
use fleet_cloud_control_plane::services::governance;
use tower::ServiceExt;

#[sqlx::test(migrations = "./migrations")]
async fn hourly_usage_rebuild_is_idempotent(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let task = common::post_json(
        common::router(pool.clone()),
        "/tasks",
        common::VALID_KEY,
        Some("usage-task-001"),
        common::create_task_body("proj_test", "meter once"),
    )
    .await;
    assert_eq!(task.status, StatusCode::ACCEPTED);
    let hour = chrono::Utc::now()
        .with_minute(0)
        .and_then(|value| value.with_second(0))
        .and_then(|value| value.with_nanosecond(0))
        .unwrap();
    governance::aggregate_usage_hour(&pool, "org_test", "proj_test", hour)
        .await
        .unwrap();
    let first: (i64, i64) = sqlx::query_as(
        "SELECT task_count,event_bytes FROM usage_hourly WHERE project_id='proj_test' AND hour=$1",
    )
    .bind(hour)
    .fetch_one(&pool)
    .await
    .unwrap();
    governance::aggregate_usage_hour(&pool, "org_test", "proj_test", hour)
        .await
        .unwrap();
    let second: (i64, i64) = sqlx::query_as(
        "SELECT task_count,event_bytes FROM usage_hourly WHERE project_id='proj_test' AND hour=$1",
    )
    .bind(hour)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.0, 1);
    assert!(first.1 > 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn mutation_is_audited_and_audit_rows_are_immutable(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let response = common::post_json(
        common::router(pool.clone()),
        "/tasks",
        common::VALID_KEY,
        Some("audit-task-001"),
        common::create_task_body("proj_test", "audit me"),
    )
    .await;
    assert_eq!(response.status, StatusCode::ACCEPTED);
    let record: (String, String, serde_json::Value) = sqlx::query_as(
        "SELECT principal_id,request_id,details FROM audit_records WHERE action='POST /tasks'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(record.0, "key_valid");
    assert!(record.1.starts_with("req_"));
    assert!(record.2["before_hash"].as_str().is_some());
    assert!(record.2["after_hash"].as_str().is_some());
    assert!(sqlx::query("DELETE FROM audit_records")
        .execute(&pool)
        .await
        .is_err());
}

#[sqlx::test(migrations = "./migrations")]
async fn quota_and_rate_limit_return_stable_errors(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    sqlx::query("INSERT INTO project_governance(project_id,organization_id,requests_per_minute,max_concurrent_runs) VALUES('proj_test','org_test',1,0)")
        .execute(&pool).await.unwrap();
    let quota = common::post_json(
        common::router(pool.clone()),
        "/tasks",
        common::VALID_KEY,
        Some("quota-task-001"),
        common::create_task_body("proj_test", "over quota"),
    )
    .await;
    assert_eq!(quota.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(quota.body["error"]["code"], "quota_exceeded");

    sqlx::query("UPDATE project_governance SET max_concurrent_runs=32,requests_per_minute=1")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM api_rate_windows")
        .execute(&pool)
        .await
        .unwrap();
    let first = common::get_tasks(common::router(pool.clone()), Some(common::VALID_KEY)).await;
    assert_eq!(first, StatusCode::OK);
    let response = common::router(pool)
        .oneshot(
            Request::get("/tasks")
                .header("authorization", format!("Bearer {}", common::VALID_KEY))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers().get("retry-after").unwrap(), "60");
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"]["code"],
        "rate_limited"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn ready_metrics_and_operations_surface_governance_failures(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let app = common::router(pool.clone());
    let ready = app
        .clone()
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(ready.status(), StatusCode::OK);

    sqlx::query("INSERT INTO runner_pools(id,organization_id,project_id,name) VALUES('pool_ops','org_test','proj_test','ops')")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO runners(id,organization_id,project_id,pool_id,name,certificate_fingerprint,status,last_heartbeat_at) VALUES('runner_ops','org_test','proj_test','pool_ops','ops',decode(repeat('aa',32),'hex'),'offline',now()-interval '10 minutes')")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO retention_jobs(job_name,last_error) VALUES('artifact-retention','object store unavailable')")
        .execute(&pool).await.unwrap();
    let ops = common::request_json(
        app.clone(),
        axum::http::Method::GET,
        "/operations",
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert_eq!(ops.status, StatusCode::OK, "{}", ops.body);
    assert_eq!(ops.body["offline_runners"].as_array().unwrap().len(), 1);
    assert_eq!(ops.body["retention_failures"].as_array().unwrap().len(), 1);
    assert!(ops.body.to_string().find("secret").is_none());
    let metrics = common::request_json(
        app,
        axum::http::Method::GET,
        "/metrics",
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert_eq!(metrics.status, StatusCode::OK);
    for key in [
        "api",
        "runner_heartbeat_age_seconds",
        "command_age_seconds",
        "outbox_lag",
        "event_lag_seconds",
        "webhook_backlog",
    ] {
        assert!(metrics.body.get(key).is_some(), "missing {key}");
    }
}
