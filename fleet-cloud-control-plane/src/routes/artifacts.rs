use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::app::AppState;
use crate::auth::ProjectPrincipal;
use crate::error::ApiError;
use crate::services::artifacts;

pub async fn upload(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(task_id): Path<String>,
    headers: axum::http::HeaderMap,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    let kind = headers
        .get("x-artifact-kind")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("other");
    let run_id = headers
        .get("x-run-id")
        .and_then(|value| value.to_str().ok());
    let mut upload = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::Validation("invalid multipart body".into()))?
    {
        if field.name() != Some("file") || upload.is_some() {
            continue;
        }
        let filename = field
            .file_name()
            .ok_or_else(|| ApiError::Validation("file name is required".into()))?
            .to_owned();
        let mime_type = field
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_owned();
        let bytes = field
            .bytes()
            .await
            .map_err(|_| ApiError::Validation("invalid Artifact body".into()))?;
        upload = Some((filename, mime_type, bytes));
    }
    let (filename, mime_type, bytes) =
        upload.ok_or_else(|| ApiError::Validation("multipart file field is required".into()))?;
    let artifact = artifacts::upload(
        &state.pool,
        &state.artifact_crypto,
        &principal,
        artifacts::ArtifactUpload {
            task_id: &task_id,
            run_id,
            kind,
            filename: &filename,
            mime_type: &mime_type,
            bytes: &bytes,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(artifact)))
}

pub async fn get(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(artifact_id): Path<String>,
) -> Result<Json<artifacts::ArtifactView>, ApiError> {
    Ok(Json(
        artifacts::get(&state.pool, &principal, &artifact_id).await?,
    ))
}

pub async fn delete(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(artifact_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    artifacts::delete_artifact(&state.pool, &principal, &artifact_id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "command_id":crate::services::tasks::new_id("cmd"),
            "status":"accepted",
            "accepted_at":chrono::Utc::now()
        })),
    ))
}

pub async fn list_task(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(task_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let data = artifacts::list_task(&state.pool, &principal, &task_id).await?;
    Ok(Json(json!({"data":data,"next_cursor":null})))
}

#[derive(Debug, Deserialize, Default)]
pub struct DownloadUrlRequest {
    expires_in_seconds: Option<i64>,
}

pub async fn create_download_url(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(artifact_id): Path<String>,
    request: Option<Json<DownloadUrlRequest>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    artifacts::get(&state.pool, &principal, &artifact_id).await?;
    let seconds = request
        .and_then(|Json(request)| request.expires_in_seconds)
        .unwrap_or(300);
    let (url, expires_at) = artifacts::signed_download_url(
        &state.artifact_crypto,
        &artifact_id,
        &principal.api_key_id,
        seconds,
    )?;
    Ok(Json(json!({"url":url,"expires_at":expires_at})))
}

#[derive(Debug, Deserialize)]
pub struct DownloadQuery {
    expires: i64,
    principal: String,
    signature: String,
}

pub async fn download(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(artifact_id): Path<String>,
    Query(query): Query<DownloadQuery>,
) -> Result<Response, ApiError> {
    if query.principal != principal.api_key_id {
        return Err(ApiError::NotFound);
    }
    artifacts::verify_download_signature(
        &state.artifact_crypto,
        &artifact_id,
        &query.principal,
        query.expires,
        &query.signature,
    )?;
    let (artifact, bytes) = artifacts::download(
        &state.pool,
        &state.artifact_crypto,
        &principal,
        &artifact_id,
    )
    .await?;
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&artifact.mime_type).map_err(|_| ApiError::Internal)?,
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{}\"", artifact.filename))
            .map_err(|_| ApiError::Internal)?,
    );
    Ok(response)
}
