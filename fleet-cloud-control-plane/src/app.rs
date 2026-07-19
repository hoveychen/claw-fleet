use axum::{extract::DefaultBodyLimit, middleware, routing::get, Json, Router};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;
use tower_http::cors::CorsLayer;

use crate::routes;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub api_key_pepper: Vec<u8>,
    pub runner_identity_issuer: Option<Arc<crate::runner_gateway::identity::RunnerIdentityIssuer>>,
    pub artifact_crypto: Arc<crate::services::artifacts::ArtifactCrypto>,
}

impl AppState {
    pub fn new(pool: PgPool, api_key_pepper: Vec<u8>) -> Self {
        let artifact_crypto = Arc::new(crate::services::artifacts::ArtifactCrypto::from_pepper(
            &api_key_pepper,
        ));
        Self {
            pool,
            api_key_pepper,
            runner_identity_issuer: None,
            artifact_crypto,
        }
    }

    pub fn with_runner_identity_issuer(
        mut self,
        issuer: crate::runner_gateway::identity::RunnerIdentityIssuer,
    ) -> Self {
        self.runner_identity_issuer = Some(Arc::new(issuer));
        self
    }
}

pub fn router(state: AppState) -> Router {
    let app = Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(routes::governance::ready))
        .route(
            "/tasks",
            get(routes::tasks::list_tasks).post(routes::tasks::create_task),
        )
        .route(
            "/tasks/{task_id}",
            get(routes::tasks::get_task).delete(routes::tasks::delete_task),
        )
        .route(
            "/tasks/{task_id}/messages",
            axum::routing::post(routes::tasks::append_message),
        )
        .route(
            "/tasks/{task_id}/cancel",
            axum::routing::post(routes::tasks::cancel_task),
        )
        .route(
            "/tasks/{task_id}/pause",
            axum::routing::post(routes::tasks::pause_task),
        )
        .route(
            "/tasks/{task_id}/resume",
            axum::routing::post(routes::tasks::resume_task),
        )
        .route(
            "/tasks/{task_id}/events",
            get(routes::tasks::list_task_events),
        )
        .route(
            "/tasks/{task_id}/artifacts",
            get(routes::artifacts::list_task).post(routes::artifacts::upload),
        )
        .route("/events/stream", get(routes::events::stream_events))
        .route(
            "/webhook-endpoints",
            get(routes::webhooks::list).post(routes::webhooks::create),
        )
        .route(
            "/webhook-endpoints/{webhook_endpoint_id}",
            axum::routing::patch(routes::webhooks::update).delete(routes::webhooks::delete),
        )
        .route(
            "/webhook-endpoints/{webhook_endpoint_id}/rotate-secret",
            axum::routing::post(routes::webhooks::rotate_secret),
        )
        .route(
            "/webhook-deliveries/{delivery_id}/replay",
            axum::routing::post(routes::webhooks::replay),
        )
        .route(
            "/webhook-deliveries",
            get(routes::webhooks::list_deliveries),
        )
        .route("/operations", get(routes::governance::operations))
        .route("/metrics", get(routes::governance::metrics))
        .route("/embed-tokens", axum::routing::post(routes::embeds::create))
        .route("/runs/{run_id}", get(routes::runs::get))
        .route("/runs/{run_id}/messages", get(routes::runs::list_messages))
        .route("/runs/{run_id}/usage", get(routes::runs::usage))
        .route("/decisions", get(routes::decisions::list))
        .route("/decisions/{decision_id}", get(routes::decisions::get))
        .route(
            "/decisions/{decision_id}/responses",
            axum::routing::post(routes::decisions::respond),
        )
        .route(
            "/artifacts/{artifact_id}",
            get(routes::artifacts::get).delete(routes::artifacts::delete),
        )
        .route(
            "/artifacts/{artifact_id}/download-url",
            axum::routing::post(routes::artifacts::create_download_url),
        )
        .route(
            "/artifact-downloads/{artifact_id}",
            get(routes::artifacts::download),
        )
        .route(
            "/runner-registrations",
            axum::routing::post(routes::runners::create_registration),
        )
        .route(
            "/runner-registrations/claim",
            axum::routing::post(routes::runners::claim_registration),
        )
        .route(
            "/runners/{runner_id}",
            axum::routing::delete(routes::runners::revoke_runner),
        )
        .route(
            "/runners/{runner_id}/drain",
            axum::routing::post(routes::runners::drain_runner),
        )
        .layer(DefaultBodyLimit::max(18 * 1024 * 1024))
        .layer(cloud_cors())
        .with_state(state.clone());
    app.layer(middleware::from_fn_with_state(
        state,
        crate::services::governance::observe_and_audit,
    ))
}

fn cloud_cors() -> CorsLayer {
    use axum::http::{header, HeaderName, HeaderValue, Method};
    CorsLayer::new()
        .allow_origin([
            HeaderValue::from_static("https://fleet-cloud.muveeai.com"),
            HeaderValue::from_static("http://localhost:5173"),
            HeaderValue::from_static("http://127.0.0.1:4173"),
        ])
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
            header::IF_MATCH,
            HeaderName::from_static("idempotency-key"),
            HeaderName::from_static("fleet-embed-parent-origin"),
            HeaderName::from_static("last-event-id"),
        ])
        .expose_headers([header::ETAG, HeaderName::from_static("fleet-request-id")])
}

async fn live() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

#[cfg(test)]
mod tests {
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    #[tokio::test]
    async fn live_route_returns_ok_without_touching_database() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://fleet:unused@127.0.0.1:1/fleet")
            .unwrap();
        let response = super::router(super::AppState::new(pool, vec![b'x'; 32]))
            .oneshot(Request::get("/health/live").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], br#"{"status":"ok"}"#);
    }
}
