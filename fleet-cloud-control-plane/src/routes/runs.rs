use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::auth::ProjectPrincipal;
use crate::error::ApiError;
use crate::services::transcripts;

pub async fn get(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let row = sqlx::query_as::<_, (String,String,i32,Option<String>,Option<String>,String,String,Option<String>,Option<String>,Option<String>,Option<chrono::DateTime<chrono::Utc>>,Option<chrono::DateTime<chrono::Utc>>,Option<String>,i64,chrono::DateTime<chrono::Utc>,chrono::DateTime<chrono::Utc>)>(
        "SELECT id,task_id,attempt,predecessor_run_id,runner_id,status,provider,model,effort,permission_policy_id,started_at,finished_at,exit_reason,version,created_at,updated_at FROM runs WHERE id=$1 AND organization_id=$2 AND project_id=$3"
    ).bind(&run_id).bind(&principal.organization_id).bind(&principal.project_id)
     .fetch_optional(&state.pool).await?.ok_or(ApiError::NotFound)?;
    Ok(Json(
        json!({"id":row.0,"task_id":row.1,"attempt":row.2,"predecessor_run_id":row.3,
        "runner_id":row.4,"status":row.5,"agent":{"provider":row.6,"model":row.7,
        "effort":row.8,"permission_policy_id":row.9},"started_at":row.10,"finished_at":row.11,
        "exit_reason":row.12,"version":row.13,"created_at":row.14,"updated_at":row.15}),
    ))
}

pub async fn usage(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(run_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let owned = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE id=$1 AND organization_id=$2 AND project_id=$3)",
    )
    .bind(&run_id)
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .fetch_one(&state.pool)
    .await?;
    if !owned {
        return Err(ApiError::NotFound);
    }
    let row = sqlx::query_as::<_, (i64, i64, i64, i64, i64, f64, i64, i64)>(
        "SELECT COALESCE(SUM(COALESCE((data->>'input_tokens')::bigint,0)),0)::bigint,
         COALESCE(SUM(COALESCE((data->>'output_tokens')::bigint,0)),0)::bigint,
         COALESCE(SUM(COALESCE((data->>'cache_read_tokens')::bigint,0)),0)::bigint,
         COALESCE(SUM(COALESCE((data->>'cache_write_tokens')::bigint,0)),0)::bigint,
         COALESCE(SUM(COALESCE((data->>'reasoning_tokens')::bigint,0)),0)::bigint,
         COALESCE(SUM(COALESCE((data->>'cost_usd')::double precision,0)),0)::double precision,
         COALESCE(SUM(COALESCE((data->>'run_seconds')::bigint,0)),0)::bigint,
         COALESCE(SUM(octet_length(data::text)),0)::bigint FROM events
         WHERE run_id=$1 AND organization_id=$2 AND project_id=$3 AND event_type='usage.updated'",
    )
    .bind(&run_id)
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .fetch_one(&state.pool)
    .await?;
    let artifact_bytes: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(size_bytes),0)::bigint FROM artifacts WHERE run_id=$1 AND organization_id=$2 AND project_id=$3 AND deleted_at IS NULL")
        .bind(&run_id).bind(&principal.organization_id).bind(&principal.project_id).fetch_one(&state.pool).await?;
    Ok(Json(
        json!({"input_tokens":row.0,"output_tokens":row.1,"cache_read_tokens":row.2,
        "cache_write_tokens":row.3,"reasoning_tokens":row.4,"provider_cost_usd":row.5,
        "run_seconds":row.6,"event_bytes":row.7,"artifact_bytes":artifact_bytes}),
    ))
}

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
