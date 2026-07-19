use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::{response::IntoResponse, Json};
use serde::Deserialize;

use crate::app::AppState;
use crate::auth::ProjectPrincipal;
use crate::error::ApiError;
use crate::services::decisions::{self, DecisionResponseRequest};

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    status: Option<String>,
    project_id: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Query(query): Query<ListQuery>,
) -> Result<Json<decisions::DecisionPage>, ApiError> {
    if query
        .project_id
        .as_deref()
        .is_some_and(|id| id != principal.project_id)
    {
        return Err(ApiError::PermissionDenied);
    }
    Ok(Json(
        decisions::list(&state.pool, &principal, query.status.as_deref()).await?,
    ))
}

pub async fn get(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let view = decisions::get(&state.pool, &principal, &id).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        "etag",
        HeaderValue::from_str(&format!("\"decision-version-{}\"", view.version))
            .map_err(|_| ApiError::Internal)?,
    );
    Ok((headers, Json(view)))
}

pub async fn respond(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<DecisionResponseRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| (8..=255).contains(&v.len()))
        .ok_or_else(|| ApiError::Validation("valid Idempotency-Key is required".into()))?;
    let expected = headers
        .get("if-match")
        .ok_or(ApiError::PreconditionRequired)?
        .to_str()
        .ok()
        .and_then(|v| {
            v.trim()
                .trim_matches('"')
                .strip_prefix("decision-version-")
                .unwrap_or(v.trim().trim_matches('"'))
                .parse()
                .ok()
        })
        .ok_or_else(|| ApiError::Validation("If-Match must be a quoted Decision version".into()))?;
    let result = decisions::respond(&state.pool, &principal, &id, key, expected, request).await?;
    Ok((
        StatusCode::from_u16(result.status).map_err(|_| ApiError::Internal)?,
        Json(result.body),
    ))
}
