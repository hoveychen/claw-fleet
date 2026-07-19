use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::{response::IntoResponse, Json};
use serde::Deserialize;

use crate::app::AppState;
use crate::auth::ProjectPrincipal;
use crate::error::ApiError;
use crate::services::artifacts::{self, DeleteFault};
use crate::services::tasks::{
    self, AppendMessageRequest, CreateTaskRequest, ResumeTaskRequest, TaskControlRequest,
};

#[derive(Debug, Deserialize)]
pub struct TaskListQuery {
    project_id: Option<String>,
    status: Option<String>,
    external_id: Option<String>,
    cursor: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct EventListQuery {
    after: Option<String>,
    limit: Option<i64>,
}

pub async fn list_tasks(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Query(query): Query<TaskListQuery>,
) -> Result<Json<tasks::TaskPage>, ApiError> {
    if query
        .project_id
        .as_deref()
        .is_some_and(|id| id != principal.project_id)
    {
        return Err(ApiError::PermissionDenied);
    }
    Ok(Json(
        tasks::list_tasks(
            &state.pool,
            &principal,
            query.status.as_deref(),
            query.external_id.as_deref(),
            query.cursor.as_deref(),
            checked_limit(query.limit)?,
        )
        .await?,
    ))
}

pub async fn get_task(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(task_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let task = tasks::get_task(&state.pool, &principal, &task_id).await?;
    let mut headers = HeaderMap::new();
    headers.insert(
        "etag",
        HeaderValue::from_str(&format!("\"{}\"", task.version)).map_err(|_| ApiError::Internal)?,
    );
    Ok((headers, Json(task)))
}

pub async fn delete_task(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(task_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    artifacts::delete_task_content(
        &state.pool,
        &state.artifact_store,
        &principal,
        &task_id,
        DeleteFault::None,
    )
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "command_id":tasks::new_id("cmd"),
            "status":"accepted",
            "accepted_at":chrono::Utc::now()
        })),
    ))
}

pub async fn create_task(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    headers: HeaderMap,
    Json(request): Json<CreateTaskRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let key = idempotency_key(&headers)?;
    let result = tasks::create_task(&state.pool, &principal, key, request).await?;
    let status = StatusCode::from_u16(result.status).map_err(|_| ApiError::Internal)?;
    Ok((status, Json(result.body)))
}

pub async fn append_message(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<AppendMessageRequest>,
) -> Result<impl IntoResponse, ApiError> {
    mutation_response(
        tasks::append_message(
            &state.pool,
            &principal,
            &task_id,
            idempotency_key(&headers)?,
            request,
        )
        .await?,
    )
}

pub async fn cancel_task(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    request: Option<Json<TaskControlRequest>>,
) -> Result<impl IntoResponse, ApiError> {
    control_response(
        &state,
        &principal,
        &task_id,
        &headers,
        "cancel",
        request.map(|Json(v)| v).unwrap_or_default(),
    )
    .await
}

pub async fn pause_task(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    request: Option<Json<TaskControlRequest>>,
) -> Result<impl IntoResponse, ApiError> {
    control_response(
        &state,
        &principal,
        &task_id,
        &headers,
        "pause",
        request.map(|Json(v)| v).unwrap_or_default(),
    )
    .await
}

pub async fn resume_task(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(task_id): Path<String>,
    headers: HeaderMap,
    request: Option<Json<ResumeTaskRequest>>,
) -> Result<impl IntoResponse, ApiError> {
    let result = tasks::resume_task(
        &state.pool,
        &principal,
        &task_id,
        idempotency_key(&headers)?,
        parse_if_match(&headers)?,
        request.map(|Json(v)| v).unwrap_or_default(),
    )
    .await?;
    mutation_response(result)
}

pub async fn list_task_events(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(task_id): Path<String>,
    Query(query): Query<EventListQuery>,
) -> Result<Json<tasks::EventPage>, ApiError> {
    Ok(Json(
        tasks::list_task_events(
            &state.pool,
            &principal,
            &task_id,
            query.after.as_deref(),
            checked_limit(query.limit)?,
        )
        .await?,
    ))
}

async fn control_response(
    state: &AppState,
    principal: &ProjectPrincipal,
    task_id: &str,
    headers: &HeaderMap,
    action: &str,
    request: TaskControlRequest,
) -> Result<impl IntoResponse, ApiError> {
    let result = tasks::control_task(
        &state.pool,
        principal,
        task_id,
        idempotency_key(headers)?,
        parse_if_match(headers)?,
        action,
        request,
    )
    .await?;
    mutation_response(result)
}

fn mutation_response(result: tasks::MutationResult) -> Result<impl IntoResponse, ApiError> {
    let status = StatusCode::from_u16(result.status).map_err(|_| ApiError::Internal)?;
    Ok((status, Json(result.body)))
}

fn idempotency_key(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| value.len() >= 8 && value.len() <= 255)
        .ok_or_else(|| ApiError::Validation("valid Idempotency-Key is required".into()))
}

fn parse_if_match(headers: &HeaderMap) -> Result<Option<i64>, ApiError> {
    headers
        .get("if-match")
        .map(|value| {
            value
                .to_str()
                .ok()
                .and_then(|v| v.trim().trim_matches('"').parse().ok())
                .ok_or_else(|| {
                    ApiError::Validation("If-Match must be a quoted task version".into())
                })
        })
        .transpose()
}

fn checked_limit(limit: Option<i64>) -> Result<i64, ApiError> {
    let limit = limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(ApiError::Validation(
            "limit must be between 1 and 200".into(),
        ));
    }
    Ok(limit)
}
