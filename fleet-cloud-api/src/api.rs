use crate::{
    domain::{CreateTaskRequest, RequestScope},
    store::{CreateTaskCommand, StoreError, TaskStore},
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{sse::Event, IntoResponse, Response, Sse},
    Json,
};
use futures_util::stream;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{convert::Infallible, sync::Arc};
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<dyn TaskStore>,
}

#[derive(Debug, Serialize)]
pub struct Problem {
    #[serde(rename = "type")]
    problem_type: String,
    title: String,
    status: u16,
    code: String,
    detail: String,
    request_id: String,
    retryable: bool,
}

pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    title: &'static str,
    detail: String,
    retryable: bool,
}

impl ApiError {
    fn bad_request(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            title: "Invalid request",
            detail: detail.into(),
            retryable: false,
        }
    }

    fn from_store(error: StoreError) -> Self {
        match error {
            StoreError::NotFound => Self {
                status: StatusCode::NOT_FOUND,
                code: "not_found",
                title: "Resource not found",
                detail: "resource is absent or outside the caller's scope".into(),
                retryable: false,
            },
            StoreError::Conflict { code, detail } => Self {
                status: StatusCode::CONFLICT,
                code,
                title: "Resource conflict",
                detail,
                retryable: false,
            },
            StoreError::Internal(detail) => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "internal_error",
                title: "Internal server error",
                detail,
                retryable: true,
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let problem = Problem {
            problem_type: format!("https://fleet.example.com/problems/{}", self.code),
            title: self.title.into(),
            status: self.status.as_u16(),
            code: self.code.into(),
            detail: self.detail,
            request_id: Uuid::new_v4().to_string(),
            retryable: self.retryable,
        };
        let mut response = (self.status, Json(problem)).into_response();
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}

pub async fn create_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateTaskRequest>,
) -> Result<Response, ApiError> {
    request
        .validate()
        .map_err(|detail| ApiError::bad_request("invalid_task", detail))?;
    let scope = scope_from_headers(&headers)?;
    let idempotency_key = required_header(&headers, "idempotency-key")?;
    if !(8..=255).contains(&idempotency_key.len()) {
        return Err(ApiError::bad_request(
            "invalid_idempotency_key",
            "Idempotency-Key must be 8 to 255 bytes",
        ));
    }
    let canonical = serde_json::to_vec(&request)
        .map_err(|error| ApiError::bad_request("invalid_json", error.to_string()))?;
    let fingerprint: [u8; 32] = Sha256::digest(canonical).into();
    let outcome = state
        .store
        .create_task(CreateTaskCommand {
            scope,
            idempotency_key,
            request_fingerprint: fingerprint,
            request,
        })
        .await
        .map_err(ApiError::from_store)?;

    let status = if outcome.replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    let mut response = (status, Json(outcome.task)).into_response();
    response.headers_mut().insert(
        HeaderName::from_static("idempotency-replayed"),
        HeaderValue::from_static(if outcome.replayed { "true" } else { "false" }),
    );
    Ok(response)
}

pub async fn get_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let task = state
        .store
        .get_task(scope_from_headers(&headers)?, task_id)
        .await
        .map_err(ApiError::from_store)?;
    Ok(Json(task))
}

#[derive(Debug, Deserialize)]
pub struct EventQuery {
    #[serde(default)]
    after: Option<i64>,
}

pub async fn stream_task_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(task_id): Path<Uuid>,
    Query(query): Query<EventQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    let after = query.after.unwrap_or(0).max(last_event_id).max(0);
    let events = state
        .store
        .list_events(scope_from_headers(&headers)?, task_id, after)
        .await
        .map_err(ApiError::from_store)?;
    let frames = events.into_iter().map(|task_event| {
        let frame = Event::default()
            .id(task_event.sequence.to_string())
            .event(task_event.event_type.clone())
            .json_data(task_event)
            .expect("TaskEvent serialization cannot fail");
        Ok::<_, Infallible>(frame)
    });
    Ok(Sse::new(stream::iter(frames)).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("heartbeat"),
    ))
}

fn scope_from_headers(headers: &HeaderMap) -> Result<RequestScope, ApiError> {
    let organization_id = parse_uuid_header(headers, "x-fleet-organization-id")?;
    let project_id = parse_uuid_header(headers, "x-fleet-project-id")?;
    Ok(RequestScope {
        organization_id,
        project_id,
    })
}

fn parse_uuid_header(headers: &HeaderMap, name: &'static str) -> Result<Uuid, ApiError> {
    let value = required_header(headers, name)?;
    Uuid::parse_str(&value)
        .map_err(|_| ApiError::bad_request("invalid_scope", format!("{name} must be a UUID")))
}

fn required_header(headers: &HeaderMap, name: &'static str) -> Result<String, ApiError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| ApiError::bad_request("missing_header", format!("missing {name}")))
}

pub fn example_create_task() -> serde_json::Value {
    json!({
        "prompt": "run the Fleet Cloud spike",
        "workspace_selector": { "labels": { "repo": "claude-fleet" } },
        "agent_profile": { "tool": "codex", "required_capabilities": [] }
    })
}
