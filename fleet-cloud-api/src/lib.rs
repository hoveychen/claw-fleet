//! Fleet Cloud control-plane HTTP surface.

pub mod api;
pub mod domain;
pub mod embed;
pub mod store;
pub mod webhook;

use api::AppState;
use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
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
    app_with_store_and_optional_embed(store, None)
}

pub fn app_with_store_and_embed(
    store: Arc<dyn store::TaskStore>,
    embed: Arc<embed::EmbedTokenVerifier>,
) -> Router {
    app_with_store_and_optional_embed(store, Some(embed))
}

fn app_with_store_and_optional_embed(
    store: Arc<dyn store::TaskStore>,
    embed: Option<Arc<embed::EmbedTokenVerifier>>,
) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/v1/tasks", get(api::list_tasks).post(api::create_task))
        .route("/v1/tasks/{task_id}", get(api::get_task))
        .route("/v1/tasks/{task_id}/events", get(api::stream_task_events))
        .route("/v1/decisions/{decision_id}", get(api::get_decision))
        .route(
            "/v1/decisions/{decision_id}/responses",
            post(api::respond_to_decision),
        )
        .with_state(AppState { store, embed })
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

    #[tokio::test]
    async fn decision_response_is_scoped_cas_and_idempotent() {
        let store = Arc::new(store::InMemoryTaskStore::default());
        let app = app_with_store(store.clone());
        let created = app
            .clone()
            .oneshot(create_request("create-decision-task", "needs approval"))
            .await
            .unwrap();
        let body = to_bytes(created.into_body(), 64 * 1024).await.unwrap();
        let task: Value = serde_json::from_slice(&body).unwrap();
        let task_id = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
        let decision = store
            .seed_open_decision(
                domain::RequestScope {
                    organization_id: Uuid::parse_str(ORG).unwrap(),
                    project_id: Uuid::parse_str(PROJECT).unwrap(),
                },
                task_id,
                "fleet_ask",
                json!({ "question": "Ship now?" }),
            )
            .unwrap();

        let detail = app
            .clone()
            .oneshot(scoped_request(
                "GET",
                &format!("/v1/tasks/{task_id}"),
                Body::empty(),
            ))
            .await
            .unwrap();
        let body = to_bytes(detail.into_body(), 64 * 1024).await.unwrap();
        let detail: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(detail["attempts"].as_array().unwrap().len(), 1);
        assert_eq!(detail["decisions"].as_array().unwrap().len(), 1);

        let mut foreign = scoped_request(
            "GET",
            &format!("/v1/decisions/{}", decision.id),
            Body::empty(),
        );
        foreign.headers_mut().insert(
            "x-fleet-organization-id",
            Uuid::new_v4().to_string().parse().unwrap(),
        );
        assert_eq!(
            app.clone().oneshot(foreign).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );

        let response_body = json!({ "action": "answer", "answers": { "choice": "ship" } });
        let response_request = |key: &str, body: Value| {
            let mut request = scoped_request(
                "POST",
                &format!("/v1/decisions/{}/responses", decision.id),
                Body::from(body.to_string()),
            );
            request
                .headers_mut()
                .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
            request
                .headers_mut()
                .insert("idempotency-key", key.parse().unwrap());
            request
        };

        let first = app
            .clone()
            .oneshot(response_request(
                "decision-answer-001",
                response_body.clone(),
            ))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(first.headers()["idempotency-replayed"], "false");
        let body = to_bytes(first.into_body(), 64 * 1024).await.unwrap();
        let answered: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(answered["status"], "answered");

        let replay = app
            .clone()
            .oneshot(response_request("decision-answer-001", response_body))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(replay.headers()["idempotency-replayed"], "true");

        let conflict = app
            .clone()
            .oneshot(response_request(
                "decision-answer-001",
                json!({ "action": "decline" }),
            ))
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);

        let already_resolved = app
            .oneshot(response_request(
                "decision-answer-002",
                json!({ "action": "decline" }),
            ))
            .await
            .unwrap();
        assert_eq!(already_resolved.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn embed_token_is_origin_and_task_scoped_at_routes() {
        let store = Arc::new(store::InMemoryTaskStore::default());
        let verifier = Arc::new(embed::EmbedTokenVerifier::new(b"route-test-secret"));
        let app = app_with_store_and_embed(store, verifier.clone());
        let first = app
            .clone()
            .oneshot(create_request("embed-task-001", "embedded task"))
            .await
            .unwrap();
        let body = to_bytes(first.into_body(), 64 * 1024).await.unwrap();
        let first: Value = serde_json::from_slice(&body).unwrap();
        let first_id = Uuid::parse_str(first["id"].as_str().unwrap()).unwrap();
        let second = app
            .clone()
            .oneshot(create_request("embed-task-002", "other task"))
            .await
            .unwrap();
        let body = to_bytes(second.into_body(), 64 * 1024).await.unwrap();
        let second: Value = serde_json::from_slice(&body).unwrap();
        let second_id = second["id"].as_str().unwrap();
        let token = verifier
            .issue(&embed::EmbedClaims {
                organization_id: Uuid::parse_str(ORG).unwrap(),
                project_id: Uuid::parse_str(PROJECT).unwrap(),
                task_id: Some(first_id),
                origins: vec!["https://tickets.example.com".into()],
                expires_at: chrono::Utc::now().timestamp() + 60,
            })
            .unwrap();
        let embedded_request = |uri: &str, origin: &str| {
            Request::builder()
                .uri(uri)
                .header("authorization", format!("Embed {token}"))
                .header("origin", origin)
                .body(Body::empty())
                .unwrap()
        };

        let allowed = app
            .clone()
            .oneshot(embedded_request(
                &format!("/v1/tasks/{first_id}"),
                "https://tickets.example.com",
            ))
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
        let foreign_task = app
            .clone()
            .oneshot(embedded_request(
                &format!("/v1/tasks/{second_id}"),
                "https://tickets.example.com",
            ))
            .await
            .unwrap();
        assert_eq!(foreign_task.status(), StatusCode::FORBIDDEN);
        let list = app
            .clone()
            .oneshot(embedded_request("/v1/tasks", "https://tickets.example.com"))
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::FORBIDDEN);
        let wrong_origin = app
            .oneshot(embedded_request(
                &format!("/v1/tasks/{first_id}"),
                "https://evil.example.com",
            ))
            .await
            .unwrap();
        assert_eq!(wrong_origin.status(), StatusCode::FORBIDDEN);
    }
}
