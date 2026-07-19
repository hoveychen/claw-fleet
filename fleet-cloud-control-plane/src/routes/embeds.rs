use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;

use crate::app::AppState;
use crate::auth::ProjectPrincipal;
use crate::error::ApiError;
use crate::services::embeds::{self, CreateEmbedTokenRequest, CreateEmbedTokenResponse};

pub async fn create(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    headers: HeaderMap,
    Json(request): Json<CreateEmbedTokenRequest>,
) -> Result<(StatusCode, Json<CreateEmbedTokenResponse>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(
            embeds::create_idempotent(
                &state.pool,
                &state.api_key_pepper,
                &principal,
                headers
                    .get("idempotency-key")
                    .and_then(|v| v.to_str().ok())
                    .map(str::trim)
                    .filter(|v| (8..=255).contains(&v.len()))
                    .ok_or_else(|| {
                        ApiError::Validation("valid Idempotency-Key is required".into())
                    })?,
                request,
            )
            .await?,
        ),
    ))
}
