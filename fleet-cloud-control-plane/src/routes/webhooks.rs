use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::auth::ProjectPrincipal;
use crate::error::ApiError;
use crate::services::webhooks::{self, CreateWebhookEndpointRequest, UpdateWebhookEndpointRequest};

pub async fn create(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    headers: HeaderMap,
    Json(request): Json<CreateWebhookEndpointRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(
            webhooks::create(
                &state.pool,
                &state.api_key_pepper,
                &principal,
                idempotency_key(&headers)?,
                request,
            )
            .await?,
        ),
    ))
}

pub async fn list(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        json!({"data":webhooks::list(&state.pool, &principal).await?}),
    ))
}

#[derive(Debug, serde::Deserialize)]
pub struct DeliveryQuery {
    endpoint_id: Option<String>,
}

pub async fn list_deliveries(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Query(query): Query<DeliveryQuery>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!({
        "data":webhooks::list_deliveries(&state.pool, &principal, query.endpoint_id.as_deref()).await?
    })))
}

pub async fn update(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<UpdateWebhookEndpointRequest>,
) -> Result<Json<Value>, ApiError> {
    let version = expected_version(&headers)?;
    Ok(Json(
        webhooks::update(
            &state.pool,
            &principal,
            &id,
            version,
            idempotency_key(&headers)?,
            request,
        )
        .await?,
    ))
}

pub async fn delete(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    webhooks::delete(&state.pool, &principal, &id, idempotency_key(&headers)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn rotate_secret(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        webhooks::rotate_secret(
            &state.pool,
            &state.api_key_pepper,
            &principal,
            &id,
            idempotency_key(&headers)?,
        )
        .await?,
    ))
}

pub async fn replay(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(webhooks::replay(&state.pool, &principal, &id, idempotency_key(&headers)?).await?),
    ))
}

fn idempotency_key(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| (8..=255).contains(&value.len()))
        .ok_or_else(|| ApiError::Validation("valid Idempotency-Key is required".into()))
}

fn expected_version(headers: &HeaderMap) -> Result<i64, ApiError> {
    headers
        .get("if-match")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().trim_matches('"'))
        .and_then(|value| value.parse().ok())
        .ok_or(ApiError::PreconditionRequired)
}
