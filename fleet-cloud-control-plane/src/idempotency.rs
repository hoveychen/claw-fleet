use sha2::{Digest, Sha256};
use sqlx::{Postgres, Transaction};

use crate::auth::ProjectPrincipal;
use crate::error::ApiError;

pub struct StoredResponse {
    pub status: u16,
    pub body: serde_json::Value,
}

pub fn request_hash<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, ApiError> {
    let canonical = serde_json::to_vec(value).map_err(|_| ApiError::Internal)?;
    Ok(Sha256::digest(canonical).to_vec())
}

pub async fn lock_and_find(
    tx: &mut Transaction<'_, Postgres>,
    principal: &ProjectPrincipal,
    endpoint: &str,
    key: &str,
    hash: &[u8],
) -> Result<Option<StoredResponse>, ApiError> {
    let lock_key = format!(
        "{}:{}:{}:{}:{}",
        principal.organization_id, principal.project_id, principal.api_key_id, endpoint, key
    );
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_key)
        .execute(&mut **tx)
        .await?;

    let row = sqlx::query_as::<_, (Vec<u8>, i16, serde_json::Value)>(
        "SELECT request_hash, response_status, response_body
         FROM idempotency_keys
         WHERE organization_id = $1 AND project_id = $2 AND principal_id = $3
           AND endpoint = $4 AND idempotency_key = $5 AND expires_at > now()",
    )
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .bind(&principal.api_key_id)
    .bind(endpoint)
    .bind(key)
    .fetch_optional(&mut **tx)
    .await?;

    match row {
        Some((stored_hash, _, _)) if stored_hash != hash => Err(ApiError::IdempotencyMismatch),
        Some((_, status, body)) => Ok(Some(StoredResponse {
            status: status as u16,
            body,
        })),
        None => Ok(None),
    }
}

pub async fn store(
    tx: &mut Transaction<'_, Postgres>,
    principal: &ProjectPrincipal,
    endpoint: &str,
    key: &str,
    hash: &[u8],
    status: u16,
    body: &serde_json::Value,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO idempotency_keys(
            organization_id, project_id, principal_id, endpoint, idempotency_key,
            request_hash, response_status, response_body, expires_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now() + interval '7 days')",
    )
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .bind(&principal.api_key_id)
    .bind(endpoint)
    .bind(key)
    .bind(hash)
    .bind(status as i16)
    .bind(body)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
