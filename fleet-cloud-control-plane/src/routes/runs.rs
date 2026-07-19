use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::app::AppState;
use crate::auth::ProjectPrincipal;
use crate::error::ApiError;
use crate::services::transcripts;

#[derive(Debug, Deserialize)]
pub struct MessageQuery {
    after: Option<i64>,
    limit: Option<i64>,
}

pub async fn list_messages(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(run_id): Path<String>,
    Query(query): Query<MessageQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = transcripts::list(
        &state.pool,
        &principal,
        &run_id,
        query.after.unwrap_or(0),
        query.limit.unwrap_or(100),
    )
    .await?;
    let next_cursor = data.last().map(|_| "sequence");
    Ok(Json(json!({"data":data,"next_cursor":next_cursor})))
}
