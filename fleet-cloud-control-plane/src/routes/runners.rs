use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{response::IntoResponse, Json};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::auth::{self, ProjectPrincipal};
use crate::error::ApiError;
use crate::idempotency;
use crate::services::tasks::new_id;

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateRegistrationRequest {
    pool_id: String,
    name: Option<String>,
    #[serde(default = "default_expiry")]
    expires_in_seconds: i64,
}

fn default_expiry() -> i64 {
    600
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ClaimRegistrationRequest {
    token: String,
    certificate_fingerprint: Option<String>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default = "default_concurrency")]
    max_concurrency: i32,
    platform: Option<String>,
    architecture: Option<String>,
    build_version: Option<String>,
}

fn default_concurrency() -> i32 {
    1
}

#[derive(Debug, Serialize)]
struct RegistrationResponse {
    id: String,
    token: String,
    expires_at: DateTime<Utc>,
}

pub async fn create_registration(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    headers: HeaderMap,
    Json(request): Json<CreateRegistrationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if !(60..=3600).contains(&request.expires_in_seconds) {
        return Err(ApiError::Validation(
            "expires_in_seconds must be between 60 and 3600".into(),
        ));
    }
    let key = idempotency_key(&headers)?;
    let hash = idempotency::request_hash(&request)?;
    let mut tx = state.pool.begin().await?;
    if let Some(stored) = idempotency::lock_and_find(
        &mut tx,
        &principal,
        "POST /runner-registrations",
        key,
        &hash,
    )
    .await?
    {
        tx.commit().await?;
        return Ok((StatusCode::CREATED, Json(stored.body)));
    }
    let pool_exists = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM runner_pools WHERE id=$1 AND organization_id=$2 AND project_id=$3)")
        .bind(&request.pool_id).bind(&principal.organization_id).bind(&principal.project_id).fetch_one(&mut *tx).await?;
    if !pool_exists {
        return Err(ApiError::NotFound);
    }
    let id = new_id("reg");
    let token = format!(
        "flr_{}{}",
        uuid::Uuid::now_v7().simple(),
        uuid::Uuid::now_v7().simple()
    );
    let expires_at = Utc::now() + Duration::seconds(request.expires_in_seconds);
    sqlx::query("INSERT INTO runner_registration_tokens(id,organization_id,project_id,pool_id,name,token_hash,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(&id).bind(&principal.organization_id).bind(&principal.project_id).bind(&request.pool_id).bind(&request.name)
        .bind(auth::hash_api_key(&state.api_key_pepper, &token)).bind(expires_at).execute(&mut *tx).await?;
    let body = serde_json::to_value(RegistrationResponse {
        id,
        token,
        expires_at,
    })
    .map_err(|_| ApiError::Internal)?;
    idempotency::store(
        &mut tx,
        &principal,
        "POST /runner-registrations",
        key,
        &hash,
        201,
        &body,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(body)))
}

pub async fn claim_registration(
    State(state): State<AppState>,
    Json(request): Json<ClaimRegistrationRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if request.token.len() < 32 || !(1..=256).contains(&request.max_concurrency) {
        return Err(ApiError::Validation(
            "invalid Runner registration claim".into(),
        ));
    }
    let request_hash = hex::encode(Sha256::digest(
        serde_json::to_vec(&request).map_err(|_| ApiError::Internal)?,
    ));
    let token_hash = auth::hash_api_key(&state.api_key_pepper, &request.token);
    let mut tx = state.pool.begin().await?;
    let registration = sqlx::query_as::<_, (String,String,String,Option<String>)>(
        "SELECT id,organization_id,project_id,name FROM runner_registration_tokens WHERE token_hash=$1 AND used_at IS NULL AND expires_at>now() FOR UPDATE")
        .bind(token_hash).fetch_optional(&mut *tx).await?.ok_or(ApiError::AuthenticationRequired)?;
    let pool_id = sqlx::query_scalar::<_, String>(
        "SELECT pool_id FROM runner_registration_tokens WHERE id=$1",
    )
    .bind(&registration.0)
    .fetch_one(&mut *tx)
    .await?;
    let runner_id = new_id("runner");
    let issued = state
        .runner_identity_issuer
        .as_ref()
        .map(|issuer| issuer.issue(&runner_id))
        .transpose()
        .map_err(|_| ApiError::Internal)?;
    let fingerprint = match (&issued, request.certificate_fingerprint.as_deref()) {
        (Some(identity), _) => identity.fingerprint.clone(),
        (None, Some(fingerprint)) if fingerprint.len() >= 32 => hex::decode(fingerprint)
            .map_err(|_| ApiError::Validation("certificate_fingerprint must be hex".into()))?,
        (None, _) => {
            return Err(ApiError::Validation(
                "certificate_fingerprint is required when certificate issuance is disabled".into(),
            ))
        }
    };
    sqlx::query("INSERT INTO runners(id,organization_id,project_id,pool_id,name,certificate_fingerprint,build_version,platform,architecture,capabilities,max_concurrency) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
        .bind(&runner_id).bind(&registration.1).bind(&registration.2).bind(pool_id)
        .bind(registration.3.unwrap_or_else(|| runner_id.clone())).bind(fingerprint).bind(request.build_version)
        .bind(request.platform).bind(request.architecture).bind(json!(request.capabilities)).bind(request.max_concurrency).execute(&mut *tx).await?;
    sqlx::query("UPDATE runner_registration_tokens SET used_at=now() WHERE id=$1")
        .bind(&registration.0)
        .execute(&mut *tx)
        .await?;
    let response_hash = hex::encode(Sha256::digest(
        serde_json::to_vec(&json!({"runner_id":&runner_id,"status":"offline"}))
            .map_err(|_| ApiError::Internal)?,
    ));
    sqlx::query("INSERT INTO audit_records(organization_id,project_id,principal_type,principal_id,request_id,action,resource_type,resource_id,details) VALUES($1,$2,'runner_registration',$3,$4,'POST /runner-registrations/claim','runner',$5,$6)")
        .bind(&registration.1).bind(&registration.2).bind(&registration.0)
        .bind(new_id("req")).bind(&runner_id)
        .bind(json!({"before_hash":request_hash,"after_hash":response_hash,"status":201}))
        .execute(&mut *tx).await?;
    tx.commit().await?;
    let mut body = json!({"runner_id": runner_id, "status": "offline"});
    if let Some(identity) = issued {
        body["certificate_pem"] = json!(identity.certificate_pem);
        body["private_key_pem"] = json!(identity.private_key_pem);
        body["ca_certificate_pem"] = json!(identity.ca_certificate_pem);
    }
    Ok((StatusCode::CREATED, Json(body)))
}

pub async fn revoke_runner(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(runner_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let result = sqlx::query("UPDATE runners SET status='revoked',revoked_at=now(),scheduling_enabled=false,version=version+1,updated_at=now() WHERE id=$1 AND organization_id=$2 AND project_id=$3 AND revoked_at IS NULL")
        .bind(runner_id).bind(principal.organization_id).bind(principal.project_id).execute(&state.pool).await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn drain_runner(
    State(state): State<AppState>,
    principal: ProjectPrincipal,
    Path(runner_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let result = sqlx::query("UPDATE runners SET status='draining',scheduling_enabled=false,version=version+1,updated_at=now() WHERE id=$1 AND organization_id=$2 AND project_id=$3 AND revoked_at IS NULL")
        .bind(&runner_id).bind(&principal.organization_id).bind(&principal.project_id).execute(&state.pool).await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"command_id":new_id("cmd"),"status":"accepted","accepted_at":Utc::now()})),
    ))
}

pub async fn authorize_runner_identity(
    pool: &sqlx::PgPool,
    runner_id: &str,
    fingerprint: &[u8],
) -> Result<(), ApiError> {
    let valid = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM runners WHERE id=$1 AND certificate_fingerprint=$2 AND revoked_at IS NULL)")
        .bind(runner_id).bind(fingerprint).fetch_one(pool).await?;
    if valid {
        Ok(())
    } else {
        Err(ApiError::AuthenticationRequired)
    }
}

fn idempotency_key(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| v.len() >= 8 && v.len() <= 255)
        .ok_or_else(|| ApiError::Validation("valid Idempotency-Key is required".into()))
}
