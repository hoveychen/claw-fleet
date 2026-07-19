use std::collections::HashMap;
use std::future::Future;
use std::net::IpAddr;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};

use crate::auth::ProjectPrincipal;
use crate::error::ApiError;
use crate::idempotency;
use crate::services::tasks::new_id;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct WebhookEndpoint {
    pub id: String,
    pub project_id: String,
    pub url: String,
    pub event_types: Vec<String>,
    pub description: Option<String>,
    pub enabled: bool,
    pub last_delivery_at: Option<DateTime<Utc>>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWebhookEndpointRequest {
    pub project_id: String,
    pub url: String,
    pub event_types: Vec<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateWebhookEndpointRequest {
    pub url: Option<String>,
    pub event_types: Option<Vec<String>>,
    pub enabled: Option<bool>,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DeliveryRequest {
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct WebhookDelivery {
    pub id: String,
    pub endpoint_id: String,
    pub event_id: String,
    pub status: String,
    pub attempt_count: i32,
    pub next_attempt_at: DateTime<Utc>,
    pub deadline_at: DateTime<Utc>,
    pub last_status_code: Option<i32>,
    pub last_error: Option<String>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub replay_of_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[allow(async_fn_in_trait)]
pub trait WebhookTransport {
    async fn post(&mut self, request: DeliveryRequest) -> Result<u16, String>;
}

pub struct ReqwestWebhookTransport {
    client: reqwest::Client,
}

impl ReqwestWebhookTransport {
    pub fn new() -> Result<Self, ApiError> {
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|_| ApiError::Internal)?;
        Ok(Self { client })
    }
}

impl WebhookTransport for ReqwestWebhookTransport {
    async fn post(&mut self, request: DeliveryRequest) -> Result<u16, String> {
        validate_webhook_url(&request.url)
            .await
            .map_err(|error| error.to_string())?;
        let mut builder = self.client.post(&request.url).body(request.body);
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        builder
            .header("content-type", "application/json")
            .send()
            .await
            .map(|response| response.status().as_u16())
            .map_err(|error| error.to_string())
    }
}

const VIEW_COLUMNS: &str = "id,project_id,url,event_types,description,enabled,last_delivery_at,version,created_at,updated_at";

pub fn sign_payload(secret: &[u8], timestamp: i64, raw_body: &[u8]) -> Result<String, ApiError> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret).map_err(|_| ApiError::Internal)?;
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(raw_body);
    Ok(format!("v1={}", hex::encode(mac.finalize().into_bytes())))
}

pub async fn validate_webhook_url(url: &str) -> Result<(), ApiError> {
    validate_webhook_url_with(url, |host, port| async move {
        tokio::net::lookup_host((host, port))
            .await
            .map(|addresses| addresses.map(|address| address.ip()).collect())
            .map_err(|error| error.to_string())
    })
    .await
}

pub async fn validate_webhook_url_with<F, Fut>(url: &str, resolver: F) -> Result<(), ApiError>
where
    F: FnOnce(String, u16) -> Fut,
    Fut: Future<Output = Result<Vec<IpAddr>, String>>,
{
    let parsed =
        reqwest::Url::parse(url).map_err(|_| ApiError::Validation("invalid webhook URL".into()))?;
    if parsed.scheme() != "https" || parsed.username() != "" || parsed.password().is_some() {
        return Err(ApiError::Validation(
            "webhook URL must be HTTPS without user info".into(),
        ));
    }
    let raw_host = parsed
        .host_str()
        .ok_or_else(|| ApiError::Validation("webhook URL host is required".into()))?;
    let host = raw_host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return Err(ApiError::Validation(
            "webhook URL target is not public".into(),
        ));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if !is_public_ip(ip) {
            return Err(ApiError::Validation(
                "webhook URL target is not public".into(),
            ));
        }
    }
    let addresses = resolver(
        host.to_owned(),
        parsed.port_or_known_default().unwrap_or(443),
    )
    .await
    .map_err(|_| ApiError::Validation("webhook URL DNS lookup failed".into()))?;
    if addresses.is_empty() || addresses.into_iter().any(|ip| !is_public_ip(ip)) {
        return Err(ApiError::Validation(
            "webhook URL target is not public".into(),
        ));
    }
    Ok(())
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified()
                || ip.octets()[0] == 0
                || ip.octets()[0] >= 224)
        }
        IpAddr::V6(ip) => {
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast())
        }
    }
}

pub async fn create(
    pool: &PgPool,
    pepper: &[u8],
    principal: &ProjectPrincipal,
    key: &str,
    request: CreateWebhookEndpointRequest,
) -> Result<Value, ApiError> {
    if request.project_id != principal.project_id {
        return Err(ApiError::PermissionDenied);
    }
    validate_event_types(&request.event_types)?;
    validate_webhook_url(&request.url).await?;
    let hash = idempotency::request_hash(&request)?;
    let mut tx = pool.begin().await?;
    if let Some(stored) =
        idempotency::lock_and_find(&mut tx, principal, "POST /webhook-endpoints", key, &hash)
            .await?
    {
        tx.commit().await?;
        return Ok(stored.body);
    }
    let id = new_id("wh");
    let secret = new_secret();
    let (ciphertext, nonce) = encrypt_secret(pepper, &id, secret.as_bytes())?;
    let sql = format!(
        "INSERT INTO webhook_endpoints(id,organization_id,project_id,url,event_types,description,secret_ciphertext,secret_nonce)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING {VIEW_COLUMNS}"
    );
    let endpoint = sqlx::query_as::<_, WebhookEndpoint>(&sql)
        .bind(&id)
        .bind(&principal.organization_id)
        .bind(&principal.project_id)
        .bind(&request.url)
        .bind(&request.event_types)
        .bind(&request.description)
        .bind(ciphertext)
        .bind(nonce)
        .fetch_one(&mut *tx)
        .await?;
    let body = json!({"endpoint": endpoint, "signing_secret": secret});
    idempotency::store(
        &mut tx,
        principal,
        "POST /webhook-endpoints",
        key,
        &hash,
        201,
        &body,
    )
    .await?;
    tx.commit().await?;
    Ok(body)
}

pub async fn list(
    pool: &PgPool,
    principal: &ProjectPrincipal,
) -> Result<Vec<WebhookEndpoint>, ApiError> {
    let sql = format!(
        "SELECT {VIEW_COLUMNS} FROM webhook_endpoints WHERE organization_id=$1 AND project_id=$2 ORDER BY created_at,id"
    );
    Ok(sqlx::query_as::<_, WebhookEndpoint>(&sql)
        .bind(&principal.organization_id)
        .bind(&principal.project_id)
        .fetch_all(pool)
        .await?)
}

pub async fn list_deliveries(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    endpoint_id: Option<&str>,
) -> Result<Vec<WebhookDelivery>, ApiError> {
    Ok(sqlx::query_as::<_, WebhookDelivery>(
        "SELECT id,endpoint_id,event_id,status,attempt_count,next_attempt_at,deadline_at,last_status_code,last_error,delivered_at,replay_of_id,created_at,updated_at
         FROM webhook_deliveries WHERE organization_id=$1 AND project_id=$2
         AND ($3::text IS NULL OR endpoint_id=$3) ORDER BY created_at DESC,id DESC LIMIT 200",
    )
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .bind(endpoint_id)
    .fetch_all(pool)
    .await?)
}

pub async fn update(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    id: &str,
    expected_version: i64,
    key: &str,
    request: UpdateWebhookEndpointRequest,
) -> Result<Value, ApiError> {
    if let Some(url) = request.url.as_deref() {
        validate_webhook_url(url).await?;
    }
    if let Some(event_types) = request.event_types.as_deref() {
        validate_event_types(event_types)?;
    }
    let hash = idempotency::request_hash(&(expected_version, &request))?;
    let endpoint_name = format!("PATCH /webhook-endpoints/{id}");
    let mut tx = pool.begin().await?;
    if let Some(stored) =
        idempotency::lock_and_find(&mut tx, principal, &endpoint_name, key, &hash).await?
    {
        tx.commit().await?;
        return Ok(stored.body);
    }
    let sql = format!(
        "UPDATE webhook_endpoints SET url=COALESCE($1,url),event_types=COALESCE($2,event_types),enabled=COALESCE($3,enabled),description=COALESCE($4,description),version=version+1,updated_at=now()
         WHERE id=$5 AND organization_id=$6 AND project_id=$7 AND version=$8 RETURNING {VIEW_COLUMNS}"
    );
    let endpoint = sqlx::query_as::<_, WebhookEndpoint>(&sql)
        .bind(request.url)
        .bind(request.event_types)
        .bind(request.enabled)
        .bind(request.description)
        .bind(id)
        .bind(&principal.organization_id)
        .bind(&principal.project_id)
        .bind(expected_version)
        .fetch_optional(&mut *tx)
        .await?;
    let endpoint = match endpoint {
        Some(endpoint) => endpoint,
        None => {
            let exists = sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM webhook_endpoints WHERE id=$1 AND organization_id=$2 AND project_id=$3)")
                .bind(id).bind(&principal.organization_id).bind(&principal.project_id).fetch_one(&mut *tx).await?;
            return Err(if exists {
                ApiError::VersionConflict
            } else {
                ApiError::NotFound
            });
        }
    };
    let body = serde_json::to_value(endpoint).map_err(|_| ApiError::Internal)?;
    idempotency::store(&mut tx, principal, &endpoint_name, key, &hash, 200, &body).await?;
    tx.commit().await?;
    Ok(body)
}

pub async fn delete(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    id: &str,
    key: &str,
) -> Result<(), ApiError> {
    let endpoint_name = format!("DELETE /webhook-endpoints/{id}");
    let hash = idempotency::request_hash(&id)?;
    let mut tx = pool.begin().await?;
    if idempotency::lock_and_find(&mut tx, principal, &endpoint_name, key, &hash)
        .await?
        .is_some()
    {
        tx.commit().await?;
        return Ok(());
    }
    let changed = sqlx::query(
        "DELETE FROM webhook_endpoints WHERE id=$1 AND organization_id=$2 AND project_id=$3",
    )
    .bind(id)
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    idempotency::store(
        &mut tx,
        principal,
        &endpoint_name,
        key,
        &hash,
        204,
        &json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn rotate_secret(
    pool: &PgPool,
    pepper: &[u8],
    principal: &ProjectPrincipal,
    id: &str,
    key: &str,
) -> Result<Value, ApiError> {
    let endpoint_name = format!("POST /webhook-endpoints/{id}/rotate-secret");
    let hash = idempotency::request_hash(&id)?;
    let mut tx = pool.begin().await?;
    if let Some(stored) =
        idempotency::lock_and_find(&mut tx, principal, &endpoint_name, key, &hash).await?
    {
        tx.commit().await?;
        return Ok(stored.body);
    }
    let secret = new_secret();
    let (ciphertext, nonce) = encrypt_secret(pepper, id, secret.as_bytes())?;
    let previous_valid_until = Utc::now() + Duration::hours(1);
    let changed = sqlx::query("UPDATE webhook_endpoints SET previous_secret_ciphertext=secret_ciphertext,previous_secret_nonce=secret_nonce,previous_valid_until=$1,secret_ciphertext=$2,secret_nonce=$3,version=version+1,updated_at=now() WHERE id=$4 AND organization_id=$5 AND project_id=$6")
        .bind(previous_valid_until).bind(ciphertext).bind(nonce).bind(id)
        .bind(&principal.organization_id).bind(&principal.project_id).execute(&mut *tx).await?;
    if changed.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    let body = json!({"signing_secret":secret,"previous_valid_until":previous_valid_until});
    idempotency::store(&mut tx, principal, &endpoint_name, key, &hash, 200, &body).await?;
    tx.commit().await?;
    Ok(body)
}

pub async fn replay(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    delivery_id: &str,
    key: &str,
) -> Result<Value, ApiError> {
    let endpoint_name = format!("POST /webhook-deliveries/{delivery_id}/replay");
    let hash = idempotency::request_hash(&delivery_id)?;
    let mut tx = pool.begin().await?;
    if let Some(stored) =
        idempotency::lock_and_find(&mut tx, principal, &endpoint_name, key, &hash).await?
    {
        tx.commit().await?;
        return Ok(stored.body);
    }
    let source = sqlx::query_as::<_, (String,String)>("SELECT endpoint_id,event_id FROM webhook_deliveries WHERE id=$1 AND organization_id=$2 AND project_id=$3")
        .bind(delivery_id).bind(&principal.organization_id).bind(&principal.project_id).fetch_optional(&mut *tx).await?.ok_or(ApiError::NotFound)?;
    let id = new_id("del");
    sqlx::query("INSERT INTO webhook_deliveries(id,organization_id,project_id,endpoint_id,event_id,replay_of_id) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(&id).bind(&principal.organization_id).bind(&principal.project_id).bind(&source.0).bind(&source.1).bind(delivery_id).execute(&mut *tx).await?;
    let body = json!({"id":id,"event_id":source.1,"status":"pending","replay_of_id":delivery_id});
    idempotency::store(&mut tx, principal, &endpoint_name, key, &hash, 201, &body).await?;
    tx.commit().await?;
    Ok(body)
}

pub async fn process_delivery<T: WebhookTransport>(
    pool: &PgPool,
    pepper: &[u8],
    delivery_id: &str,
    transport: &mut T,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query_as::<_, (String,String,String,String,Vec<u8>,Vec<u8>,DateTime<Utc>,i32,Value)>(
        "SELECT d.endpoint_id,d.event_id,e.url,e.project_id,e.secret_ciphertext,e.secret_nonce,d.deadline_at,d.attempt_count,
         jsonb_build_object('id',v.id,'cursor',v.cursor::text,'organization_id',v.organization_id,'project_id',v.project_id,'task_id',v.task_id,'run_id',v.run_id,'type',v.event_type,'sequence',v.task_sequence,'occurred_at',v.occurred_at,'recorded_at',v.recorded_at,'data',v.data,'schema_version',v.schema_version)
         FROM webhook_deliveries d JOIN webhook_endpoints e ON e.id=d.endpoint_id JOIN events v ON v.id=d.event_id
         WHERE d.id=$1 AND d.status='pending' AND d.next_attempt_at<=now() FOR UPDATE OF d")
        .bind(delivery_id).fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(());
    };
    let secret = decrypt_secret(pepper, &row.0, &row.4, &row.5)?;
    let body = serde_json::to_vec(&row.8).map_err(|_| ApiError::Internal)?;
    let timestamp = Utc::now().timestamp();
    let headers = HashMap::from([
        ("Fleet-Event-Id".into(), row.1.clone()),
        ("Fleet-Delivery-Id".into(), delivery_id.to_owned()),
        ("Fleet-Timestamp".into(), timestamp.to_string()),
        (
            "Fleet-Signature".into(),
            sign_payload(&secret, timestamp, &body)?,
        ),
    ]);
    let result = transport
        .post(DeliveryRequest {
            url: row.2,
            headers,
            body,
        })
        .await;
    let attempt = row.7 + 1;
    match result {
        Ok(status) if (200..300).contains(&status) => {
            sqlx::query("UPDATE webhook_deliveries SET status='delivered',attempt_count=$2,last_status_code=$3,last_error=NULL,delivered_at=now(),updated_at=now() WHERE id=$1")
                .bind(delivery_id).bind(attempt).bind(i32::from(status)).execute(&mut *tx).await?;
            sqlx::query(
                "UPDATE webhook_endpoints SET last_delivery_at=now(),updated_at=now() WHERE id=$1",
            )
            .bind(&row.0)
            .execute(&mut *tx)
            .await?;
        }
        outcome => {
            let error = match &outcome {
                Ok(status) => format!("HTTP {status}"),
                Err(error) => error.clone(),
            };
            let status = match outcome {
                Ok(status) => Some(i32::from(status)),
                Err(_) => None,
            };
            let terminal = Utc::now() >= row.6;
            let backoff_seconds = 2_i64
                .pow((attempt.saturating_sub(1).min(12)) as u32)
                .min(3600);
            sqlx::query("UPDATE webhook_deliveries SET status=CASE WHEN $2 THEN 'failed' ELSE 'pending' END,attempt_count=$3,last_status_code=$4,last_error=$5,next_attempt_at=now()+make_interval(secs=>$6::int),updated_at=now() WHERE id=$1")
                .bind(delivery_id).bind(terminal).bind(attempt).bind(status).bind(error).bind(backoff_seconds as i32).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

pub async fn process_due_batch(
    pool: &PgPool,
    pepper: &[u8],
    limit: i64,
) -> Result<usize, ApiError> {
    let ids = sqlx::query_scalar::<_, String>("SELECT id FROM webhook_deliveries WHERE status='pending' AND next_attempt_at<=now() ORDER BY next_attempt_at,id LIMIT $1")
        .bind(limit).fetch_all(pool).await?;
    let mut transport = ReqwestWebhookTransport::new()?;
    for id in &ids {
        process_delivery(pool, pepper, id, &mut transport).await?;
    }
    Ok(ids.len())
}

fn validate_event_types(event_types: &[String]) -> Result<(), ApiError> {
    if event_types.is_empty()
        || event_types.len() > 100
        || event_types
            .iter()
            .any(|value| value.trim().is_empty() || value.len() > 100)
    {
        return Err(ApiError::Validation(
            "event_types must contain 1-100 valid values".into(),
        ));
    }
    Ok(())
}

fn webhook_key(pepper: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(pepper);
    hasher.update(b"\0fleet-cloud-webhook-secret-v1");
    hasher.finalize().into()
}

fn encrypt_secret(
    pepper: &[u8],
    endpoint_id: &str,
    secret: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), ApiError> {
    let mut nonce = [0_u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let ciphertext = Aes256Gcm::new_from_slice(&webhook_key(pepper))
        .map_err(|_| ApiError::Internal)?
        .encrypt(
            Nonce::from_slice(&nonce),
            aes_gcm::aead::Payload {
                msg: secret,
                aad: endpoint_id.as_bytes(),
            },
        )
        .map_err(|_| ApiError::Internal)?;
    Ok((ciphertext, nonce.to_vec()))
}

fn decrypt_secret(
    pepper: &[u8],
    endpoint_id: &str,
    ciphertext: &[u8],
    nonce: &[u8],
) -> Result<Vec<u8>, ApiError> {
    if nonce.len() != 12 {
        return Err(ApiError::Internal);
    }
    Aes256Gcm::new_from_slice(&webhook_key(pepper))
        .map_err(|_| ApiError::Internal)?
        .decrypt(
            Nonce::from_slice(nonce),
            aes_gcm::aead::Payload {
                msg: ciphertext,
                aad: endpoint_id.as_bytes(),
            },
        )
        .map_err(|_| ApiError::Internal)
}

fn new_secret() -> String {
    format!(
        "whsec_{}{}",
        uuid::Uuid::now_v7().simple(),
        uuid::Uuid::now_v7().simple()
    )
}
