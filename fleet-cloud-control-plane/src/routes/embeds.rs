use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

use crate::app::AppState;
use crate::auth::ProjectPrincipal;
use crate::error::ApiError;
use crate::services::embeds::{self, CreateEmbedTokenRequest, CreateEmbedTokenResponse};

pub async fn create(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Json(request): Json<CreateEmbedTokenRequest>,
) -> Result<(StatusCode, Json<CreateEmbedTokenResponse>), ApiError> {
    Ok((
        StatusCode::CREATED,
        Json(embeds::create(&state.pool, &state.api_key_pepper, &principal, request).await?),
    ))
}
