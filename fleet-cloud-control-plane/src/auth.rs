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
        let parent_origin = parts
            .headers
            .get("fleet-embed-parent-origin")
            .or_else(|| parts.headers.get(header::ORIGIN))
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let method = parts.method.clone();
        let path = parts.uri.path().to_owned();
        let query = parts.uri.query().unwrap_or_default().to_owned();
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
            .bind(&hash)
            .fetch_optional(&pool)
            .await?;

            if let Some(principal) = principal {
                sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE id = $1")
                    .bind(&principal.api_key_id)
                    .execute(&pool)
                    .await?;
                return Ok(principal);
            }

            let embed = sqlx::query_as::<
                _,
                (
                    String,
                    String,
                    String,
                    Option<String>,
                    serde_json::Value,
                    serde_json::Value,
                ),
            >(
                "SELECT organization_id,project_id,id,task_id,allowed_origins,views
                 FROM embed_tokens WHERE token_hash=$1 AND revoked_at IS NULL AND expires_at>now()",
            )
            .bind(&hash)
            .fetch_optional(&pool)
            .await?
            .ok_or(ApiError::AuthenticationRequired)?;
            authorize_embed_request(
                &pool,
                &embed.1,
                embed.3.as_deref(),
                &embed.4,
                &embed.5,
                parent_origin.as_deref(),
                &method,
                &path,
                &query,
            )
            .await?;

            Ok(ProjectPrincipal {
                organization_id: embed.0,
                project_id: embed.1,
                api_key_id: embed.2,
            })
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn authorize_embed_request(
    pool: &sqlx::PgPool,
    project_id: &str,
    task_scope: Option<&str>,
    allowed_origins: &serde_json::Value,
    views: &serde_json::Value,
    parent_origin: Option<&str>,
    method: &axum::http::Method,
    path: &str,
    query: &str,
) -> Result<(), ApiError> {
    let origins = allowed_origins
        .as_array()
        .ok_or(ApiError::AuthenticationRequired)?;
    if !origins
        .iter()
        .any(|origin| origin.as_str() == parent_origin)
    {
        return Err(ApiError::PermissionDenied);
    }
    let views = views.as_array().ok_or(ApiError::AuthenticationRequired)?;
    let has_view = |view: &str| views.iter().any(|value| value.as_str() == Some(view));
    let is_decision = path == "/decisions" || path.starts_with("/decisions/");
    let is_usage = path.ends_with("/usage");
    if (is_decision && !(has_view("decision_inbox") || has_view("decision_card")))
        || (is_usage && !has_view("usage"))
        || (!is_decision && !is_usage && !has_view("task_detail"))
    {
        return Err(ApiError::PermissionDenied);
    }
    if method != axum::http::Method::GET
        && !(method == axum::http::Method::POST
            && path.ends_with("/responses")
            && has_view("decision_card"))
    {
        return Err(ApiError::PermissionDenied);
    }
    let Some(task_scope) = task_scope else {
        return Ok(());
    };
    if path == format!("/tasks/{task_scope}")
        || path.starts_with(&format!("/tasks/{task_scope}/"))
        || (path == "/events/stream" && query_has(query, "task_id", task_scope))
        || (path == "/decisions" && query_has(query, "task_id", task_scope))
    {
        return Ok(());
    }
    if let Some(run_id) = path
        .strip_prefix("/runs/")
        .and_then(|tail| tail.split('/').next())
    {
        let owned = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE id=$1 AND project_id=$2 AND task_id=$3)",
        )
        .bind(run_id)
        .bind(project_id)
        .bind(task_scope)
        .fetch_one(pool)
        .await?;
        if owned {
            return Ok(());
        }
    }
    if let Some(decision_id) = path
        .strip_prefix("/decisions/")
        .and_then(|tail| tail.split('/').next())
    {
        let owned = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM decisions WHERE id=$1 AND project_id=$2 AND task_id=$3)",
        )
        .bind(decision_id)
        .bind(project_id)
        .bind(task_scope)
        .fetch_one(pool)
        .await?;
        if owned {
            return Ok(());
        }
    }
    Err(ApiError::PermissionDenied)
}

fn query_has(query: &str, key: &str, expected: &str) -> bool {
    query.split('&').any(|pair| {
        pair.split_once('=')
            .is_some_and(|(name, value)| name == key && value == expected)
    })
}
