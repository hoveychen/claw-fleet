use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;
use serde_json::Value;

use crate::app::AppState;
use crate::auth::ProjectPrincipal;
use crate::error::ApiError;
use crate::services::governance;

pub async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    match governance::ready(&state.pool).await {
        Ok(status) => (StatusCode::OK, Json(status)),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"status":"not_ready"})),
        ),
    }
}

pub async fn operations(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(governance::operations(&state.pool, &principal).await?))
}

pub async fn metrics(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(governance::metrics(&state.pool, &principal).await?))
}
