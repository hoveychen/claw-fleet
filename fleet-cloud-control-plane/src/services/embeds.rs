use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;

use crate::auth::{hash_api_key, ProjectPrincipal};
use crate::error::ApiError;
use crate::services::tasks::new_id;

const PRODUCTION_ORIGIN: &str = "https://fleet-pilot.muveeai.com";
const DEVELOPMENT_ORIGIN: &str = "http://localhost:5173";

#[derive(Debug, Deserialize)]
pub struct CreateEmbedTokenRequest {
    pub project_id: String,
    pub task_id: Option<String>,
    pub allowed_origins: Vec<String>,
    pub views: Vec<String>,
    pub expires_in_seconds: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct CreateEmbedTokenResponse {
    pub token: String,
    pub embed_url: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct Claims<'a> {
    project_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<&'a str>,
    allowed_origins: &'a [String],
    views: &'a [String],
    iat: i64,
    exp: i64,
    jti: &'a str,
}

pub async fn create(
    pool: &PgPool,
    pepper: &[u8],
    principal: &ProjectPrincipal,
    request: CreateEmbedTokenRequest,
) -> Result<CreateEmbedTokenResponse, ApiError> {
    if request.project_id != principal.project_id || principal.api_key_id.starts_with("emb_") {
        return Err(ApiError::PermissionDenied);
    }
    let expires_in = request.expires_in_seconds.unwrap_or(600);
    if !(60..=3_600).contains(&expires_in) {
        return Err(ApiError::Validation(
            "expires_in_seconds must be between 60 and 3600".into(),
        ));
    }
    if request.allowed_origins.is_empty()
        || request.allowed_origins.len() > 10
        || request
            .allowed_origins
            .iter()
            .any(|origin| !matches!(origin.as_str(), PRODUCTION_ORIGIN | DEVELOPMENT_ORIGIN))
    {
        return Err(ApiError::Validation(
            "embed origin is not allowlisted".into(),
        ));
    }
    let mut views = request.views;
    views.sort();
    views.dedup();
    if views.is_empty()
        || views.iter().any(|view| {
            !matches!(
                view.as_str(),
                "task_detail" | "decision_inbox" | "decision_card" | "usage"
            )
        })
    {
        return Err(ApiError::Validation("invalid embed view scope".into()));
    }
    if let Some(task_id) = request.task_id.as_deref() {
        let owned = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE id=$1 AND organization_id=$2 AND project_id=$3)",
        )
        .bind(task_id)
        .bind(&principal.organization_id)
        .bind(&principal.project_id)
        .fetch_one(pool)
        .await?;
        if !owned {
            return Err(ApiError::NotFound);
        }
    }

    let id = new_id("emb");
    let now = Utc::now();
    let expires_at = now + Duration::seconds(expires_in);
    let claims = Claims {
        project_id: &principal.project_id,
        task_id: request.task_id.as_deref(),
        allowed_origins: &request.allowed_origins,
        views: &views,
        iat: now.timestamp(),
        exp: expires_at.timestamp(),
        jti: &id,
    };
    let header = encode_json(&json!({"alg":"opaque","typ":"FLEET-EMBED"}))?;
    let payload = encode_json(&claims)?;
    let mut signature = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut signature);
    let token = format!(
        "{header}.{payload}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
    );
    sqlx::query(
        "INSERT INTO embed_tokens(id,organization_id,project_id,task_id,token_hash,allowed_origins,views,created_by_api_key_id,expires_at)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(&id)
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .bind(&request.task_id)
    .bind(hash_api_key(pepper, &token))
    .bind(json!(request.allowed_origins))
    .bind(json!(views))
    .bind(&principal.api_key_id)
    .bind(expires_at)
    .execute(pool)
    .await?;
    let target = request
        .task_id
        .as_deref()
        .map(|task_id| format!("task/{task_id}"))
        .unwrap_or_else(|| "decisions".into());
    Ok(CreateEmbedTokenResponse {
        token,
        embed_url: format!("https://fleet-cloud.muveeai.com/embed/{target}"),
        expires_at,
    })
}

fn encode_json(value: &impl Serialize) -> Result<String, ApiError> {
    let bytes = serde_json::to_vec(value).map_err(|_| ApiError::Internal)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}
