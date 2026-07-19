mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fleet_cloud_control_plane::routes::runners::authorize_runner_identity;
use tower::ServiceExt;

async fn create_registration(pool: &sqlx::PgPool, key: &str, name: &str) -> String {
    common::request_json(
        common::router(pool.clone()),
        Method::POST,
        "/runner-registrations",
        common::VALID_KEY,
        &[("idempotency-key", key)],
        Some(serde_json::json!({"pool_id":"pool_test","name":name})),
    )
    .await
    .body["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn claim(pool: &sqlx::PgPool, token: &str, fingerprint: &str) -> common::JsonResponse {
    common::request_json(
        common::router(pool.clone()),
        Method::POST,
        "/runner-registrations/claim",
        common::VALID_KEY,
        &[],
        Some(serde_json::json!({
            "token":token,
            "certificate_fingerprint":fingerprint,
            "capabilities":["claude_code"],
            "max_concurrency":2,
            "platform":"linux",
            "architecture":"x86_64"
        })),
    )
    .await
}

#[sqlx::test(migrations = "./migrations")]
async fn registration_token_is_single_use(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    sqlx::query("INSERT INTO runner_pools(id,organization_id,project_id,name) VALUES('pool_test','org_test','proj_test','Test')")
        .execute(&pool).await.unwrap();
    let token = create_registration(&pool, "register-once-001", "runner one").await;
    let first = claim(&pool, &token, &"11".repeat(32)).await;
    assert_eq!(first.status, StatusCode::CREATED, "{}", first.body);
    let second = claim(&pool, &token, &"22".repeat(32)).await;
    assert_eq!(second.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM runners")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn revoked_identity_fails_without_affecting_other_runner(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    sqlx::query("INSERT INTO runner_pools(id,organization_id,project_id,name) VALUES('pool_test','org_test','proj_test','Test')")
        .execute(&pool).await.unwrap();
    let fingerprint_one = vec![0x33; 32];
    let fingerprint_two = vec![0x44; 32];
    let one = claim(
        &pool,
        &create_registration(&pool, "register-revoke-001", "one").await,
        &hex::encode(&fingerprint_one),
    )
    .await;
    let two = claim(
        &pool,
        &create_registration(&pool, "register-revoke-002", "two").await,
        &hex::encode(&fingerprint_two),
    )
    .await;
    let runner_one = one.body["runner_id"].as_str().unwrap();
    let runner_two = two.body["runner_id"].as_str().unwrap();
    authorize_runner_identity(&pool, runner_one, &fingerprint_one)
        .await
        .unwrap();
    authorize_runner_identity(&pool, runner_two, &fingerprint_two)
        .await
        .unwrap();

    let request = Request::delete(format!("/runners/{runner_one}"))
        .header("authorization", format!("Bearer {}", common::VALID_KEY))
        .header("idempotency-key", "revoke-runner-001")
        .body(Body::empty())
        .unwrap();
    let response = common::router(pool.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        authorize_runner_identity(&pool, runner_one, &fingerprint_one)
            .await
            .is_err()
    );
    authorize_runner_identity(&pool, runner_two, &fingerprint_two)
        .await
        .unwrap();
}
