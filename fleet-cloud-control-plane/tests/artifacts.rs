mod common;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use fleet_cloud_control_plane::services::artifacts::{self, DeleteFault};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

async fn create_task(pool: &sqlx::PgPool) -> serde_json::Value {
    common::post_json(
        common::router(pool.clone()),
        "/tasks",
        common::VALID_KEY,
        Some("artifact-task-001"),
        common::create_task_body("proj_test", "produce an encrypted report"),
    )
    .await
    .body
}

fn multipart(filename: &str, mime: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "fleet-artifact-boundary";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {mime}\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn upload(
    pool: &sqlx::PgPool,
    task_id: &str,
    key: &str,
    filename: &str,
    mime: &str,
    bytes: &[u8],
) -> (StatusCode, serde_json::Value) {
    let (content_type, body) = multipart(filename, mime, bytes);
    let response = common::router(pool.clone())
        .oneshot(
            Request::post(format!("/tasks/{task_id}/artifacts"))
                .header("authorization", format!("Bearer {key}"))
                .header("content-type", content_type)
                .header("x-artifact-kind", "report")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn seed_other_tenant(pool: &sqlx::PgPool) -> &'static str {
    const OTHER_KEY: &str = "flk_test_other_0123456789abcdef";
    sqlx::query("INSERT INTO organizations(id,name) VALUES('org_other','Other')")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO projects(id,organization_id,slug,name) VALUES('proj_other','org_other','other','Other')")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO api_keys(id,organization_id,project_id,name,key_prefix,key_hash) VALUES('key_other','org_other','proj_other','Other',$1,$2)")
        .bind(fleet_cloud_control_plane::auth::key_prefix(OTHER_KEY))
        .bind(fleet_cloud_control_plane::auth::hash_api_key(common::PEPPER, OTHER_KEY))
        .execute(pool).await.unwrap();
    OTHER_KEY
}

#[sqlx::test(migrations = "./migrations")]
async fn artifact_is_encrypted_hashed_and_download_url_is_principal_scoped(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let created = create_task(&pool).await;
    let task_id = created["task"]["id"].as_str().unwrap();
    let plaintext = b"# Fleet report\nsecret-free result\n";

    let (status, artifact) = upload(
        &pool,
        task_id,
        common::VALID_KEY,
        "report.md",
        "text/markdown",
        plaintext,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let artifact_id = artifact["id"].as_str().unwrap();
    assert_eq!(artifact["size_bytes"], plaintext.len());
    assert_eq!(artifact["sha256"], hex::encode(Sha256::digest(plaintext)));

    let stored = sqlx::query_as::<_, (Vec<u8>, Vec<u8>, String)>(
        "SELECT ciphertext,encrypted_data_key,kms_key_id FROM artifact_objects WHERE artifact_id=$1",
    )
    .bind(artifact_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!stored
        .0
        .windows(plaintext.len())
        .any(|window| window == plaintext));
    assert!(!stored
        .1
        .windows(plaintext.len())
        .any(|window| window == plaintext));
    assert_eq!(stored.2, "fleet-local-kms-v1");

    let signed = common::request_json(
        common::router(pool.clone()),
        axum::http::Method::POST,
        &format!("/artifacts/{artifact_id}/download-url"),
        common::VALID_KEY,
        &[],
        Some(serde_json::json!({"expires_in_seconds":300})),
    )
    .await;
    assert_eq!(signed.status, StatusCode::OK);
    let url = signed.body["url"].as_str().unwrap();
    assert!(url.contains(artifact_id));

    let response = common::router(pool.clone())
        .oneshot(
            Request::get(url)
                .header("authorization", format!("Bearer {}", common::VALID_KEY))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        &to_bytes(response.into_body(), 1024 * 1024).await.unwrap()[..],
        plaintext
    );

    let other_key = seed_other_tenant(&pool).await;
    let wrong_principal_download = common::router(pool.clone())
        .oneshot(
            Request::get(url)
                .header("authorization", format!("Bearer {other_key}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(wrong_principal_download.status(), StatusCode::NOT_FOUND);
    for (method, uri) in [
        (axum::http::Method::GET, format!("/artifacts/{artifact_id}")),
        (
            axum::http::Method::POST,
            format!("/artifacts/{artifact_id}/download-url"),
        ),
    ] {
        let response = common::request_json(
            common::router(pool.clone()),
            method,
            &uri,
            other_key,
            &[],
            Some(serde_json::json!({})),
        )
        .await;
        assert_eq!(response.status, StatusCode::NOT_FOUND);
    }

    let deleted = common::request_json(
        common::router(pool.clone()),
        axum::http::Method::DELETE,
        &format!("/artifacts/{artifact_id}"),
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert_eq!(deleted.status, StatusCode::ACCEPTED);
    let missing = common::request_json(
        common::router(pool.clone()),
        axum::http::Method::GET,
        &format!("/artifacts/{artifact_id}"),
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "./migrations")]
async fn artifact_upload_rejects_disallowed_mime(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let created = create_task(&pool).await;
    let task_id = created["task"]["id"].as_str().unwrap();
    let (status, body) = upload(
        &pool,
        task_id,
        common::VALID_KEY,
        "payload.sh",
        "application/x-sh",
        b"echo unsafe",
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "validation_failed");
}

#[sqlx::test(migrations = "./migrations")]
async fn artifact_upload_enforces_file_and_task_quotas(pool: sqlx::PgPool) {
    let seed = common::seed(&pool).await;
    let created = create_task(&pool).await;
    let task_id = created["task"]["id"].as_str().unwrap();
    let principal = fleet_cloud_control_plane::auth::ProjectPrincipal {
        organization_id: seed.organization_id.into(),
        project_id: seed.project_id.into(),
        api_key_id: seed.valid_key_id.into(),
    };
    let oversized = vec![0u8; artifacts::MAX_ARTIFACT_BYTES + 1];
    let error = artifacts::upload(
        &pool,
        &artifacts::ArtifactCrypto::from_pepper(common::PEPPER),
        &principal,
        artifacts::ArtifactUpload {
            task_id,
            run_id: None,
            kind: "other",
            filename: "large.bin",
            mime_type: "application/octet-stream",
            bytes: &oversized,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        fleet_cloud_control_plane::error::ApiError::Validation(_)
    ));

    sqlx::query("INSERT INTO artifacts(id,organization_id,project_id,task_id,kind,filename,mime_type,size_bytes,sha256) VALUES('art_quota','org_test','proj_test',$1,'other','existing.bin','application/octet-stream',$2,$3)")
        .bind(task_id)
        .bind(artifacts::MAX_TASK_ARTIFACT_BYTES)
        .bind("0".repeat(64))
        .execute(&pool).await.unwrap();
    let (status, body) = upload(
        &pool,
        task_id,
        common::VALID_KEY,
        "one-more.txt",
        "text/plain",
        b"x",
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"]["code"], "quota_exceeded");
}

#[sqlx::test(migrations = "./migrations")]
async fn task_delete_retries_after_object_failure_and_keeps_billing_tombstone(pool: sqlx::PgPool) {
    let seed = common::seed(&pool).await;
    let created = create_task(&pool).await;
    let task_id = created["task"]["id"].as_str().unwrap();
    let run_id = created["run"]["id"].as_str().unwrap();
    let (_, artifact) = upload(
        &pool,
        task_id,
        common::VALID_KEY,
        "report.md",
        "text/markdown",
        b"durable report",
    )
    .await;
    let artifact_id = artifact["id"].as_str().unwrap();
    sqlx::query("INSERT INTO transcript_records(id,organization_id,project_id,task_id,run_id,source_sequence,record_type,role,content,occurred_at) VALUES('trn_delete','org_test','proj_test',$1,$2,1,'message.created','assistant','{\"content\":\"delete me\"}',now())")
        .bind(task_id).bind(run_id).execute(&pool).await.unwrap();
    let principal = fleet_cloud_control_plane::auth::ProjectPrincipal {
        organization_id: seed.organization_id.into(),
        project_id: seed.project_id.into(),
        api_key_id: seed.valid_key_id.into(),
    };

    let failed =
        artifacts::delete_task_content(&pool, &principal, task_id, DeleteFault::BeforeObjectDelete)
            .await;
    assert!(failed.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM artifact_objects WHERE artifact_id=$1")
            .bind(artifact_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM audit_records WHERE resource_id=$1 AND action='task.content_delete_requested'")
            .bind(task_id).fetch_one(&pool).await.unwrap(),
        1
    );

    let retried = common::request_json(
        common::router(pool.clone()),
        axum::http::Method::DELETE,
        &format!("/tasks/{task_id}"),
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert_eq!(retried.status, StatusCode::ACCEPTED);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM artifact_objects WHERE artifact_id=$1")
            .bind(artifact_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM transcript_records WHERE task_id=$1")
            .bind(task_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tasks WHERE id=$1")
            .bind(task_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    let summary = sqlx::query_as::<_, (i64, Option<chrono::DateTime<chrono::Utc>>)>(
        "SELECT artifact_bytes,content_deleted_at FROM task_usage_summaries WHERE task_id=$1",
    )
    .bind(task_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(summary.0, b"durable report".len() as i64);
    assert!(summary.1.is_some());
}

#[sqlx::test(migrations = "./migrations")]
async fn retention_lease_prevents_double_run_and_cleans_expired_content(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let created = create_task(&pool).await;
    let task_id = created["task"]["id"].as_str().unwrap();
    let run_id = created["run"]["id"].as_str().unwrap();
    let (_, artifact) = upload(
        &pool,
        task_id,
        common::VALID_KEY,
        "old.log",
        "text/plain",
        b"old object",
    )
    .await;
    let artifact_id = artifact["id"].as_str().unwrap();
    sqlx::query("UPDATE artifacts SET retention_until=now()-interval '1 day' WHERE id=$1")
        .bind(artifact_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO transcript_records(id,organization_id,project_id,task_id,run_id,source_sequence,record_type,role,content,occurred_at,retention_until) VALUES('trn_expired','org_test','proj_test',$1,$2,1,'message.created','assistant','{}',now(),now()-interval '1 day')")
        .bind(task_id).bind(run_id).execute(&pool).await.unwrap();

    assert!(artifacts::try_acquire_retention_lease(&pool, "owner-a")
        .await
        .unwrap());
    let skipped = artifacts::run_retention(&pool, "owner-b").await.unwrap();
    assert!(!skipped.acquired);
    artifacts::release_retention_lease(&pool, "owner-a", None)
        .await
        .unwrap();
    let result = artifacts::run_retention(&pool, "owner-b").await.unwrap();
    assert!(result.acquired);
    assert_eq!(result.transcripts_deleted, 1);
    assert_eq!(result.artifacts_deleted, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM artifact_objects WHERE artifact_id=$1")
            .bind(artifact_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}
