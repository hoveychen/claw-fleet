//! Fleet Cloud control-plane HTTP surface.

pub mod api;
pub mod domain;
pub mod store;

use api::AppState;
use axum::{http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use serde::Serialize;
use std::sync::Arc;

pub const API_VERSION: &str = "0.1.0-spike";

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

async fn health() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(HealthResponse {
            status: "ok",
            version: API_VERSION,
        }),
    )
}

pub fn app() -> Router {
    app_with_store(Arc::new(store::InMemoryTaskStore::default()))
}

pub fn app_with_store(store: Arc<dyn store::TaskStore>) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/v1/tasks", get(api::list_tasks).post(api::create_task))
        .route("/v1/tasks/{task_id}", get(api::get_task))
        .route("/v1/tasks/{task_id}/events", get(api::stream_task_events))
        .with_state(AppState { store })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request},
    };
    use serde_json::{json, Value};
    use tower::ServiceExt;
    use uuid::Uuid;

    const ORG: &str = "11111111-1111-4111-8111-111111111111";
    const PROJECT: &str = "22222222-2222-4222-8222-222222222222";

    fn scoped_request(method: &str, uri: &str, body: Body) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("x-fleet-organization-id", ORG)
            .header("x-fleet-project-id", PROJECT)
            .body(body)
            .unwrap()
    }

    fn create_request(key: &str, prompt: &str) -> Request<Body> {
        let mut request = scoped_request(
            "POST",
            "/v1/tasks",
            Body::from(
                json!({
                    "prompt": prompt,
                    "workspace_selector": { "labels": { "repo": "fleet" } },
                    "agent_profile": { "tool": "codex" }
                })
                .to_string(),
            ),
        );
        request
            .headers_mut()
            .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        request
            .headers_mut()
            .insert("idempotency-key", key.parse().unwrap());
        request
    }

    #[tokio::test]
    async fn health_route_reports_version() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["version"], API_VERSION);
    }

    #[tokio::test]
    async fn create_task_is_idempotent_and_rejects_key_reuse() {
        let app = app();
        let first = app
            .clone()
            .oneshot(create_request("create-key-001", "ship it"))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::CREATED);
        assert_eq!(first.headers()["idempotency-replayed"], "false");
        let body = to_bytes(first.into_body(), 64 * 1024).await.unwrap();
        let task: Value = serde_json::from_slice(&body).unwrap();
        let task_id = task["id"].as_str().unwrap();
        assert_eq!(task["status"], "queued");
        assert_eq!(task["event_cursor"], 1);

        for _ in 0..19 {
            let replay = app
                .clone()
                .oneshot(create_request("create-key-001", "ship it"))
                .await
                .unwrap();
            assert_eq!(replay.status(), StatusCode::OK);
            assert_eq!(replay.headers()["idempotency-replayed"], "true");
            let body = to_bytes(replay.into_body(), 64 * 1024).await.unwrap();
            let replayed: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(replayed["id"], task_id);
        }

        let conflict = app
            .oneshot(create_request("create-key-001", "different body"))
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert_eq!(
            conflict.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        let body = to_bytes(conflict.into_body(), 64 * 1024).await.unwrap();
        let problem: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(problem["code"], "idempotency_key_reused");
    }

    #[tokio::test]
    async fn foreign_scope_get_returns_not_found() {
        let app = app();
        let created = app
            .clone()
            .oneshot(create_request("create-key-002", "scope test"))
            .await
            .unwrap();
        let body = to_bytes(created.into_body(), 64 * 1024).await.unwrap();
        let task: Value = serde_json::from_slice(&body).unwrap();
        let mut request = scoped_request(
            "GET",
            &format!("/v1/tasks/{}", task["id"].as_str().unwrap()),
            Body::empty(),
        );
        request.headers_mut().insert(
            "x-fleet-organization-id",
            Uuid::new_v4().to_string().parse().unwrap(),
        );
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn event_stream_replays_after_exclusive_cursor() {
        let app = app();
        let created = app
            .clone()
            .oneshot(create_request("create-key-003", "events"))
            .await
            .unwrap();
        let body = to_bytes(created.into_body(), 64 * 1024).await.unwrap();
        let task: Value = serde_json::from_slice(&body).unwrap();
        let task_id = task["id"].as_str().unwrap();

        let response = app
            .clone()
            .oneshot(scoped_request(
                "GET",
                &format!("/v1/tasks/{task_id}/events?after=0"),
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream"));
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("id: 1"), "{text}");
        assert!(text.contains("event: task.created"), "{text}");

        let response = app
            .oneshot(scoped_request(
                "GET",
                &format!("/v1/tasks/{task_id}/events?after=1"),
                Body::empty(),
            ))
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn list_tasks_filters_status_and_detail_is_browser_ready() {
        let app = app();
        let first = app
            .clone()
            .oneshot(create_request("create-key-list-1", "first task"))
            .await
            .unwrap();
        let body = to_bytes(first.into_body(), 64 * 1024).await.unwrap();
        let created: Value = serde_json::from_slice(&body).unwrap();
        let task_id = created["id"].as_str().unwrap();
        app.clone()
            .oneshot(create_request("create-key-list-2", "second task"))
            .await
            .unwrap();

        let response = app
            .clone()
            .oneshot(scoped_request(
                "GET",
                "/v1/tasks?status=queued&limit=1",
                Body::empty(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let page: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(page["data"].as_array().unwrap().len(), 1);
        assert_eq!(page["has_more"], true);
        assert!(page["next_cursor"].is_string());

        let response = app
            .oneshot(scoped_request(
                "GET",
                &format!("/v1/tasks/{task_id}"),
                Body::empty(),
            ))
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let detail: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(detail["id"], task_id);
        assert_eq!(detail["attempts"], json!([]));
        assert_eq!(detail["decisions"], json!([]));
    }
}
