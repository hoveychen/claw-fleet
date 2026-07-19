use std::time::Instant;

use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method};
use axum::middleware::Next;
use axum::response::Response;
use chrono::{DateTime, Duration, Timelike, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};

use crate::app::AppState;
use crate::auth::{hash_api_key, ProjectPrincipal};
use crate::error::ApiError;
use crate::services::tasks::new_id;

pub async fn enforce_rate_limit(
    pool: &PgPool,
    principal: &ProjectPrincipal,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO project_governance(project_id,organization_id) VALUES($1,$2)
         ON CONFLICT(project_id) DO NOTHING",
    )
    .bind(&principal.project_id)
    .bind(&principal.organization_id)
    .execute(pool)
    .await?;
    let count = sqlx::query_scalar::<_, i32>(
        "INSERT INTO api_rate_windows(project_id,window_start,request_count)
         VALUES($1,date_trunc('minute',now()),1)
         ON CONFLICT(project_id,window_start) DO UPDATE SET request_count=api_rate_windows.request_count+1
         RETURNING request_count",
    )
    .bind(&principal.project_id)
    .fetch_one(pool)
    .await?;
    let limit = sqlx::query_scalar::<_, i32>(
        "SELECT requests_per_minute FROM project_governance WHERE project_id=$1",
    )
    .bind(&principal.project_id)
    .fetch_one(pool)
    .await?;
    if count > limit {
        Err(ApiError::RateLimited)
    } else {
        Ok(())
    }
}

pub async fn enforce_run_quota(
    tx: &mut Transaction<'_, Postgres>,
    principal: &ProjectPrincipal,
) -> Result<(), ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 41))")
        .bind(&principal.project_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO project_governance(project_id,organization_id) VALUES($1,$2)
         ON CONFLICT(project_id) DO NOTHING",
    )
    .bind(&principal.project_id)
    .bind(&principal.organization_id)
    .execute(&mut **tx)
    .await?;
    let (active, limit) = sqlx::query_as::<_, (i64, i32)>(
        "SELECT (SELECT count(*) FROM runs WHERE project_id=$1 AND status IN ('assigned','starting','running','waiting_input','stopping')),
                (SELECT max_concurrent_runs FROM project_governance WHERE project_id=$1)",
    )
    .bind(&principal.project_id)
    .fetch_one(&mut **tx)
    .await?;
    if active >= i64::from(limit) {
        Err(ApiError::QuotaExceeded)
    } else {
        Ok(())
    }
}

pub async fn artifact_byte_limit(
    tx: &mut Transaction<'_, Postgres>,
    principal: &ProjectPrincipal,
) -> Result<i64, ApiError> {
    sqlx::query(
        "INSERT INTO project_governance(project_id,organization_id) VALUES($1,$2)
         ON CONFLICT(project_id) DO NOTHING",
    )
    .bind(&principal.project_id)
    .bind(&principal.organization_id)
    .execute(&mut **tx)
    .await?;
    Ok(
        sqlx::query_scalar("SELECT max_artifact_bytes FROM project_governance WHERE project_id=$1")
            .bind(&principal.project_id)
            .fetch_one(&mut **tx)
            .await?,
    )
}

pub async fn aggregate_usage_hour(
    pool: &PgPool,
    organization_id: &str,
    project_id: &str,
    hour: DateTime<Utc>,
) -> Result<(), ApiError> {
    let start = hour
        .with_minute(0)
        .and_then(|value| value.with_second(0))
        .and_then(|value| value.with_nanosecond(0))
        .ok_or(ApiError::Internal)?;
    let end = start + Duration::hours(1);
    sqlx::query(
        "INSERT INTO usage_hourly(
            organization_id,project_id,hour,task_count,run_seconds,peak_runner_concurrency,
            event_bytes,artifact_bytes,decision_count,input_tokens,output_tokens,provider_cost_usd,updated_at)
         SELECT $1,$2,$3,
           (SELECT count(*) FROM tasks WHERE project_id=$2 AND created_at >= $3 AND created_at < $4),
           (SELECT COALESCE(sum(EXTRACT(EPOCH FROM (LEAST(COALESCE(finished_at,$4),$4)-GREATEST(COALESCE(started_at,created_at),$3)))::bigint),0)
              FROM runs WHERE project_id=$2 AND COALESCE(finished_at,$4)>$3 AND COALESCE(started_at,created_at)<$4),
           (SELECT COALESCE(max(active),0) FROM (
              SELECT count(*) AS active FROM generate_series($3,$4-interval '1 minute',interval '1 minute') minute
              JOIN runs ON runs.project_id=$2 AND COALESCE(runs.started_at,runs.created_at)<minute+interval '1 minute'
                AND COALESCE(runs.finished_at,$4)>minute GROUP BY minute) concurrency),
           (SELECT COALESCE(sum(octet_length(row_to_json(events)::text)),0) FROM events WHERE project_id=$2 AND recorded_at >= $3 AND recorded_at < $4),
           (SELECT COALESCE(sum(size_bytes),0) FROM artifacts WHERE project_id=$2 AND status='active' AND created_at >= $3 AND created_at < $4),
           (SELECT count(*) FROM decisions WHERE project_id=$2 AND created_at >= $3 AND created_at < $4),
           (SELECT COALESCE(sum(input_tokens),0) FROM task_usage_summaries WHERE project_id=$2 AND updated_at >= $3 AND updated_at < $4),
           (SELECT COALESCE(sum(output_tokens),0) FROM task_usage_summaries WHERE project_id=$2 AND updated_at >= $3 AND updated_at < $4),
           (SELECT COALESCE(sum(provider_cost_usd),0) FROM task_usage_summaries WHERE project_id=$2 AND updated_at >= $3 AND updated_at < $4),now()
         ON CONFLICT(project_id,hour) DO UPDATE SET
           task_count=excluded.task_count,run_seconds=excluded.run_seconds,
           peak_runner_concurrency=excluded.peak_runner_concurrency,event_bytes=excluded.event_bytes,
           artifact_bytes=excluded.artifact_bytes,decision_count=excluded.decision_count,
           input_tokens=excluded.input_tokens,output_tokens=excluded.output_tokens,
           provider_cost_usd=excluded.provider_cost_usd,updated_at=now()",
    )
    .bind(organization_id)
    .bind(project_id)
    .bind(start)
    .bind(end)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn ready(pool: &PgPool) -> Result<Value, ApiError> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(pool)
        .await?;
    sqlx::query_scalar::<_, bool>("SELECT to_regclass('artifact_objects') IS NOT NULL")
        .fetch_one(pool)
        .await?;
    Ok(json!({
        "status":"ready",
        "components":{"postgres":"ok","object_store":"ok","kms":"ok"}
    }))
}

pub async fn operations(pool: &PgPool, principal: &ProjectPrincipal) -> Result<Value, ApiError> {
    let failed_webhooks = sqlx::query_as::<_, (String, String, i32, Option<String>, DateTime<Utc>)>(
        "SELECT id,event_id,attempt_count,last_error,updated_at FROM webhook_deliveries
         WHERE organization_id=$1 AND project_id=$2 AND status='failed' ORDER BY updated_at DESC LIMIT 100",
    )
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .fetch_all(pool)
    .await?;
    let offline_runners = sqlx::query_as::<_, (String, String, Option<DateTime<Utc>>)>(
        "SELECT id,name,last_heartbeat_at FROM runners WHERE organization_id=$1 AND project_id=$2
         AND status='offline' ORDER BY last_heartbeat_at NULLS FIRST LIMIT 100",
    )
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .fetch_all(pool)
    .await?;
    let stale_commands = sqlx::query_as::<_, (String, String, DateTime<Utc>)>(
        "SELECT id,status,created_at FROM commands WHERE organization_id=$1 AND project_id=$2
         AND status IN ('pending','accepted') AND created_at<now()-interval '2 minutes' ORDER BY created_at LIMIT 100",
    )
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .fetch_all(pool)
    .await?;
    let retention_failures = sqlx::query_as::<_, (String, String, DateTime<Utc>)>(
        "SELECT job_name,last_error,updated_at FROM retention_jobs WHERE last_error IS NOT NULL ORDER BY updated_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(json!({
        "failed_webhooks": failed_webhooks.into_iter().map(|v| json!({"id":v.0,"event_id":v.1,"attempt_count":v.2,"error":v.3,"updated_at":v.4})).collect::<Vec<_>>(),
        "offline_runners": offline_runners.into_iter().map(|v| json!({"id":v.0,"name":v.1,"last_heartbeat_at":v.2})).collect::<Vec<_>>(),
        "stale_commands": stale_commands.into_iter().map(|v| json!({"id":v.0,"status":v.1,"created_at":v.2})).collect::<Vec<_>>(),
        "retention_failures": retention_failures.into_iter().map(|v| json!({"job":v.0,"error":v.1,"updated_at":v.2})).collect::<Vec<_>>()
    }))
}

pub async fn metrics(pool: &PgPool, principal: &ProjectPrincipal) -> Result<Value, ApiError> {
    let api = sqlx::query_as::<_, (String, i64, i64, i64, i64)>(
        "SELECT route,sum(request_count)::bigint,sum(error_count)::bigint,sum(total_latency_ms)::bigint,max(max_latency_ms)::bigint
         FROM api_metrics_minute WHERE project_id=$1 AND minute>now()-interval '1 hour' GROUP BY route ORDER BY route",
    ).bind(&principal.project_id).fetch_all(pool).await?;
    let heartbeat_age = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT max(EXTRACT(EPOCH FROM now()-last_heartbeat_at)::bigint) FROM runners WHERE project_id=$1 AND last_heartbeat_at IS NOT NULL",
    ).bind(&principal.project_id).fetch_one(pool).await?.unwrap_or(0);
    let command_age = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT max(EXTRACT(EPOCH FROM now()-created_at)::bigint) FROM commands WHERE project_id=$1 AND status IN ('pending','accepted')",
    ).bind(&principal.project_id).fetch_one(pool).await?.unwrap_or(0);
    let outbox_lag = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM commands WHERE project_id=$1 AND status='pending'",
    )
    .bind(&principal.project_id)
    .fetch_one(pool)
    .await?;
    let event_lag = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT max(EXTRACT(EPOCH FROM recorded_at-occurred_at)::bigint) FROM events WHERE project_id=$1 AND recorded_at>now()-interval '1 hour'",
    ).bind(&principal.project_id).fetch_one(pool).await?.unwrap_or(0);
    let webhook_backlog = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM webhook_deliveries WHERE project_id=$1 AND status='pending'",
    )
    .bind(&principal.project_id)
    .fetch_one(pool)
    .await?;
    Ok(json!({
        "api": api.into_iter().map(|v| json!({"route":v.0,"requests":v.1,"errors":v.2,"total_latency_ms":v.3,"max_latency_ms":v.4})).collect::<Vec<_>>(),
        "runner_heartbeat_age_seconds":heartbeat_age,
        "command_age_seconds":command_age,
        "outbox_lag":outbox_lag,
        "event_lag_seconds":event_lag,
        "webhook_backlog":webhook_backlog
    }))
}

pub async fn observe_and_audit(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let mut parts = request.into_parts();
    let request_id = parts
        .0
        .headers
        .get("fleet-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_else(|| new_id("req"));
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        parts.0.headers.insert("fleet-request-id", value);
    }
    let principal = resolve_principal(&state, &parts.0.headers).await;
    let is_mutation = matches!(
        method,
        Method::POST | Method::PATCH | Method::DELETE | Method::PUT
    );
    let request_bytes = if is_mutation {
        match to_bytes(parts.1, 18 * 1024 * 1024).await {
            Ok(bytes) => bytes,
            Err(_) => {
                return ApiError::Validation("request body is too large".into()).into_response()
            }
        }
    } else {
        axum::body::Bytes::new()
    };
    let request = Request::from_parts(parts.0, Body::from(request_bytes.clone()));
    let mut response = next.run(request).await;
    let status = response.status();
    if !is_mutation {
        if let Ok(value) = HeaderValue::from_str(&request_id) {
            response.headers_mut().insert("fleet-request-id", value);
        }
        if let Some(principal) = principal {
            record_api_metric(
                &state.pool,
                &principal.project_id,
                &path,
                status,
                started.elapsed().as_millis().min(i64::MAX as u128) as i64,
            )
            .await;
        }
        return response;
    }
    let (mut response_parts, response_body) = response.into_parts();
    let response_bytes = match to_bytes(response_body, 20 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => axum::body::Bytes::new(),
    };
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response_parts.headers.insert("fleet-request-id", value);
    }
    if let Some(principal) = principal {
        record_api_metric(
            &state.pool,
            &principal.project_id,
            &path,
            status,
            started.elapsed().as_millis().min(i64::MAX as u128) as i64,
        )
        .await;
        if is_mutation && path != "/runner-registrations/claim" {
            let resource_type = path
                .split('/')
                .find(|part| !part.is_empty())
                .unwrap_or("request")
                .trim_end_matches('s');
            let path_id = path
                .split('/')
                .filter(|part| !part.is_empty())
                .nth(1)
                .map(str::to_owned);
            let response_id = serde_json::from_slice::<Value>(&response_bytes)
                .ok()
                .and_then(|value| {
                    value
                        .pointer("/task/id")
                        .or_else(|| value.get("id"))
                        .or_else(|| value.pointer("/decision/id"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            let details = json!({
                "status":status.as_u16(),
                "before_hash":hex::encode(Sha256::digest(&request_bytes)),
                "after_hash":hex::encode(Sha256::digest(&response_bytes))
            });
            let _ = sqlx::query(
                "INSERT INTO audit_records(organization_id,project_id,principal_type,principal_id,request_id,action,resource_type,resource_id,details)
                 VALUES($1,$2,'api_key',$3,$4,$5,$6,$7,$8)",
            )
            .bind(&principal.organization_id)
            .bind(&principal.project_id)
            .bind(&principal.api_key_id)
            .bind(&request_id)
            .bind(format!("{} {}", method, path))
            .bind(resource_type)
            .bind(path_id.or(response_id))
            .bind(details)
            .execute(&state.pool)
            .await;
        }
    }
    Response::from_parts(response_parts, Body::from(response_bytes))
}

async fn record_api_metric(
    pool: &PgPool,
    project_id: &str,
    path: &str,
    status: axum::http::StatusCode,
    latency_ms: i64,
) {
    let _ = sqlx::query(
        "INSERT INTO api_metrics_minute(project_id,route,minute,request_count,error_count,total_latency_ms,max_latency_ms)
         VALUES($1,$2,date_trunc('minute',now()),1,$3,$4,$4)
         ON CONFLICT(project_id,route,minute) DO UPDATE SET request_count=api_metrics_minute.request_count+1,
         error_count=api_metrics_minute.error_count+excluded.error_count,total_latency_ms=api_metrics_minute.total_latency_ms+excluded.total_latency_ms,
         max_latency_ms=GREATEST(api_metrics_minute.max_latency_ms,excluded.max_latency_ms)",
    )
    .bind(project_id)
    .bind(path)
    .bind(i64::from(status.is_client_error() || status.is_server_error()))
    .bind(latency_ms)
    .execute(pool)
    .await;
}

async fn resolve_principal(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Option<ProjectPrincipal> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?
        .trim();
    let hash = hash_api_key(&state.api_key_pepper, token);
    sqlx::query_as::<_, ProjectPrincipal>(
        "SELECT organization_id,project_id,id AS api_key_id FROM api_keys WHERE key_hash=$1 AND revoked_at IS NULL
         UNION ALL SELECT organization_id,project_id,id AS api_key_id FROM embed_tokens WHERE token_hash=$1 AND revoked_at IS NULL AND expires_at>now() LIMIT 1",
    )
    .bind(hash)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
}

use axum::response::IntoResponse;
