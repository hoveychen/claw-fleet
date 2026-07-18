use std::future::Future;

use axum::extract::FromRequestParts;
use axum::http::{header, request::Parts};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::error::ApiError;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProjectPrincipal {
    pub organization_id: String,
    pub project_id: String,
    pub api_key_id: String,
}

pub fn hash_api_key(pepper: &[u8], token: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(pepper);
    hasher.update([0]);
    hasher.update(token.as_bytes());
    hasher.finalize().to_vec()
}

pub fn key_prefix(token: &str) -> String {
    token.chars().take(20).collect()
}

impl FromRequestParts<AppState> for ProjectPrincipal {
    type Rejection = ApiError;

    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let authorization = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let pool = state.pool.clone();
        let pepper = state.api_key_pepper.clone();

        async move {
            let token = authorization.ok_or(ApiError::AuthenticationRequired)?;
            let hash = hash_api_key(&pepper, &token);
            let principal = sqlx::query_as::<_, ProjectPrincipal>(
                "SELECT organization_id, project_id, id AS api_key_id
                 FROM api_keys
                 WHERE key_hash = $1 AND revoked_at IS NULL",
            )
            .bind(hash)
            .fetch_optional(&pool)
            .await?
            .ok_or(ApiError::AuthenticationRequired)?;

            sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE id = $1")
                .bind(&principal.api_key_id)
                .execute(&pool)
                .await?;
            Ok(principal)
        }
    }
}
