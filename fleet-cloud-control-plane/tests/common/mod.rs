#![allow(dead_code)]

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use fleet_cloud_control_plane::app::{self, AppState};
use fleet_cloud_control_plane::auth;
use fleet_cloud_control_plane::services::artifacts::ArtifactObjectStore;
use sqlx::PgPool;
use tower::ServiceExt;

pub const PEPPER: &[u8] = b"fleet-cloud-test-pepper";
pub const VALID_KEY: &str = "flk_test_valid_0123456789abcdef";
pub const REVOKED_KEY: &str = "flk_test_revoked_0123456789abcdef";

pub struct Seed {
    pub organization_id: &'static str,
    pub project_id: &'static str,
    pub valid_key_id: &'static str,
}

pub async fn seed(pool: &PgPool) -> Seed {
    sqlx::query("INSERT INTO organizations(id, name) VALUES ($1, $2)")
        .bind("org_test")
        .bind("Test organization")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO projects(id, organization_id, slug, name) VALUES ($1, $2, $3, $4)")
        .bind("proj_test")
        .bind("org_test")
        .bind("test")
        .bind("Test project")
        .execute(pool)
        .await
        .unwrap();

    for (id, key, revoked) in [
        ("key_valid", VALID_KEY, false),
        ("key_revoked", REVOKED_KEY, true),
    ] {
        sqlx::query(
            "INSERT INTO api_keys(
                id, organization_id, project_id, name, key_prefix, key_hash, revoked_at
             ) VALUES ($1, $2, $3, $4, $5, $6, CASE WHEN $7 THEN now() ELSE NULL END)",
        )
        .bind(id)
        .bind("org_test")
        .bind("proj_test")
        .bind(id)
        .bind(auth::key_prefix(key))
        .bind(auth::hash_api_key(PEPPER, key))
        .bind(revoked)
        .execute(pool)
        .await
        .unwrap();
    }

    Seed {
        organization_id: "org_test",
        project_id: "proj_test",
        valid_key_id: "key_valid",
    }
}

pub fn router(pool: PgPool) -> Router {
    app::router(AppState::new(pool, PEPPER.to_vec()))
}

pub fn router_with_artifact_store(pool: PgPool, store: ArtifactObjectStore) -> Router {
    app::router(AppState::new(pool, PEPPER.to_vec()).with_artifact_store(store))
}

pub async fn get_tasks(app: Router, bearer: Option<&str>) -> StatusCode {
    let mut request = Request::get("/tasks").body(Body::empty()).unwrap();
    if let Some(token) = bearer {
        request
            .headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
    }
    app.oneshot(request).await.unwrap().status()
}

pub struct JsonResponse {
    pub status: StatusCode,
    pub body: serde_json::Value,
    pub headers: axum::http::HeaderMap,
}

pub async fn post_json(
    app: Router,
    uri: &str,
    bearer: &str,
    idempotency_key: Option<&str>,
    body: serde_json::Value,
) -> JsonResponse {
    let mut builder = Request::post(uri)
        .header("authorization", format!("Bearer {bearer}"))
        .header("content-type", "application/json");
    if let Some(key) = idempotency_key {
        builder = builder.header("idempotency-key", key);
    }
    let response = app
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    JsonResponse {
        status,
        body,
        headers,
    }
}

pub async fn request_json(
    app: Router,
    method: axum::http::Method,
    uri: &str,
    bearer: &str,
    headers: &[(&str, &str)],
    body: Option<serde_json::Value>,
) -> JsonResponse {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {bearer}"));
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let response = app
        .oneshot(builder.body(Body::from(payload)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let response_headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    JsonResponse {
        status,
        body,
        headers: response_headers,
    }
}

pub fn create_task_body(project_id: &str, goal: &str) -> serde_json::Value {
    serde_json::json!({
        "project_id": project_id,
        "external_id": "github:hoveychen/claw-fleet#123",
        "title": "Fix the duplicate event",
        "goal": goal,
        "workspace": {
            "repository": "github:hoveychen/claw-fleet",
            "ref": "main"
        },
        "agent": {
            "provider": "claude_code",
            "model": "claude-opus-4-8",
            "effort": "high",
            "permission_policy_id": "policy_01JZSAFE"
        },
        "metadata": {"issue_number": 123}
    })
}
