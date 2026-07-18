use async_trait::async_trait;
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebhookTransportResponse {
    pub status: u16,
}

#[async_trait]
pub trait WebhookTransport: Send + Sync {
    async fn post(
        &self,
        url: &str,
        signature: &str,
        body: &[u8],
    ) -> Result<WebhookTransportResponse, String>;
}

pub struct ReqwestWebhookTransport {
    client: reqwest::Client,
}

impl Default for ReqwestWebhookTransport {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl WebhookTransport for ReqwestWebhookTransport {
    async fn post(
        &self,
        url: &str,
        signature: &str,
        body: &[u8],
    ) -> Result<WebhookTransportResponse, String> {
        let response = self
            .client
            .post(url)
            .header("content-type", "application/json")
            .header("x-fleet-signature", signature)
            .body(body.to_vec())
            .send()
            .await
            .map_err(|error| error.to_string())?;
        Ok(WebhookTransportResponse {
            status: response.status().as_u16(),
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DispatchSummary {
    pub claimed: usize,
    pub delivered: usize,
    pub failed: usize,
}

#[derive(sqlx::FromRow)]
struct OutboxRow {
    id: Uuid,
    organization_id: Uuid,
    project_id: Uuid,
    payload: Value,
}

#[derive(sqlx::FromRow)]
struct EndpointRow {
    id: Uuid,
    url: String,
    event_types: Value,
    signing_secret: String,
}

pub struct PgWebhookDispatcher<T: WebhookTransport> {
    pool: PgPool,
    transport: Arc<T>,
}

impl<T: WebhookTransport> PgWebhookDispatcher<T> {
    pub fn new(pool: PgPool, transport: Arc<T>) -> Self {
        Self { pool, transport }
    }

    pub async fn dispatch_once(&self, limit: i64) -> Result<DispatchSummary, sqlx::Error> {
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;
        let rows = sqlx::query_as::<_, OutboxRow>(
            "SELECT id, organization_id, project_id, payload
             FROM outbox
             WHERE topic = 'task.event' AND completed_at IS NULL AND available_at <= $1
               AND (claimed_at IS NULL OR claimed_at < $1 - interval '5 minutes')
             ORDER BY available_at ASC, created_at ASC
             LIMIT $2
             FOR UPDATE SKIP LOCKED",
        )
        .bind(now)
        .bind(limit.clamp(1, 100))
        .fetch_all(&mut *tx)
        .await?;
        let ids: Vec<_> = rows.iter().map(|row| row.id).collect();
        if !ids.is_empty() {
            sqlx::query("UPDATE outbox SET claimed_at = $1 WHERE id = ANY($2)")
                .bind(now)
                .bind(&ids)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;

        let mut summary = DispatchSummary {
            claimed: rows.len(),
            ..DispatchSummary::default()
        };
        for row in rows {
            let delivered = self.dispatch_row(&row, &mut summary).await?;
            if delivered {
                sqlx::query("UPDATE outbox SET completed_at = $1 WHERE id = $2")
                    .bind(now)
                    .bind(row.id)
                    .execute(&self.pool)
                    .await?;
            } else {
                sqlx::query("UPDATE outbox SET claimed_at = NULL, available_at = $1 WHERE id = $2")
                    .bind(now + Duration::seconds(30))
                    .bind(row.id)
                    .execute(&self.pool)
                    .await?;
            }
        }
        Ok(summary)
    }

    async fn dispatch_row(
        &self,
        row: &OutboxRow,
        summary: &mut DispatchSummary,
    ) -> Result<bool, sqlx::Error> {
        let event_type = row
            .payload
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let event_id = row
            .payload
            .get("event_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok());
        let Some(event_id) = event_id else {
            summary.failed += 1;
            return Ok(false);
        };
        let endpoints = sqlx::query_as::<_, EndpointRow>(
            "SELECT id, url, event_types, signing_secret FROM webhook_endpoints
             WHERE organization_id = $1 AND project_id = $2 AND enabled = true",
        )
        .bind(row.organization_id)
        .bind(row.project_id)
        .fetch_all(&self.pool)
        .await?;
        let endpoints: Vec<_> = endpoints
            .into_iter()
            .filter(|endpoint| subscribes(&endpoint.event_types, event_type))
            .collect();
        if endpoints.is_empty() {
            return Ok(true);
        }

        let body = serde_json::to_vec(&row.payload)
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        let mut all_delivered = true;
        for endpoint in endpoints {
            let delivery_id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO webhook_deliveries
                 (id, organization_id, project_id, endpoint_id, event_id, status)
                 VALUES ($1, $2, $3, $4, $5, 'pending')
                 ON CONFLICT (endpoint_id, event_id) DO NOTHING",
            )
            .bind(delivery_id)
            .bind(row.organization_id)
            .bind(row.project_id)
            .bind(endpoint.id)
            .bind(event_id)
            .execute(&self.pool)
            .await?;
            let (delivery_id, current_status): (Uuid, String) = sqlx::query_as(
                "SELECT id, status FROM webhook_deliveries
                 WHERE endpoint_id = $1 AND event_id = $2",
            )
            .bind(endpoint.id)
            .bind(event_id)
            .fetch_one(&self.pool)
            .await?;
            if current_status == "delivered" {
                continue;
            }
            let signature = sign_webhook(&endpoint.signing_secret, &body);
            match self.transport.post(&endpoint.url, &signature, &body).await {
                Ok(response) if (200..300).contains(&response.status) => {
                    summary.delivered += 1;
                    sqlx::query(
                        "UPDATE webhook_deliveries SET status = 'delivered',
                            attempt_count = attempt_count + 1, last_status = $1,
                            last_error = NULL, delivered_at = now()
                         WHERE id = $2",
                    )
                    .bind(response.status as i32)
                    .bind(delivery_id)
                    .execute(&self.pool)
                    .await?;
                }
                outcome => {
                    summary.failed += 1;
                    all_delivered = false;
                    let (status, error) = match outcome {
                        Ok(response) => (
                            Some(response.status as i32),
                            format!("HTTP {}", response.status),
                        ),
                        Err(error) => (None, error),
                    };
                    sqlx::query(
                        "UPDATE webhook_deliveries SET status = 'retrying',
                            attempt_count = attempt_count + 1, last_status = $1,
                            last_error = $2, next_attempt_at = now() + interval '30 seconds'
                         WHERE id = $3",
                    )
                    .bind(status)
                    .bind(error)
                    .bind(delivery_id)
                    .execute(&self.pool)
                    .await?;
                }
            }
        }
        Ok(all_delivered)
    }
}

fn subscribes(event_types: &Value, event_type: &str) -> bool {
    event_types.as_array().is_some_and(|types| {
        types.is_empty() || types.iter().any(|value| value.as_str() == Some(event_type))
    })
}

pub fn sign_webhook(secret: &str, body: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts secrets of any size");
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    let mut encoded = String::with_capacity(bytes.len() * 2 + 7);
    encoded.push_str("sha256=");
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}
