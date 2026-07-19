use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};

use crate::auth::ProjectPrincipal;
use crate::error::ApiError;

#[derive(Debug, FromRow)]
struct TranscriptRow {
    id: String,
    run_id: String,
    source_sequence: i64,
    role: String,
    content: Value,
    redactions: Value,
    occurred_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct TranscriptView {
    id: String,
    run_id: String,
    sequence: i64,
    role: String,
    content: Vec<Value>,
    redactions: Vec<Value>,
    record: Value,
    occurred_at: DateTime<Utc>,
}

pub async fn list(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    run_id: &str,
    after: i64,
    limit: i64,
) -> Result<Vec<TranscriptView>, ApiError> {
    let owned = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE id=$1 AND organization_id=$2 AND project_id=$3)",
    )
    .bind(run_id)
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .fetch_one(pool)
    .await?;
    if !owned {
        return Err(ApiError::NotFound);
    }
    let rows = sqlx::query_as::<_, TranscriptRow>(
        "SELECT id,run_id,source_sequence,role,content,redactions,occurred_at
         FROM transcript_records WHERE run_id=$1 AND source_sequence>$2
         ORDER BY source_sequence LIMIT $3",
    )
    .bind(run_id)
    .bind(after)
    .bind(limit.clamp(1, 200))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let content = normalize_content(&row.content);
            let redactions = row
                .redactions
                .as_object()
                .map(|map| {
                    map.iter()
                        .filter_map(|(kind, count)| {
                            count
                                .as_u64()
                                .map(|count| json!({"kind":kind,"count":count}))
                        })
                        .collect()
                })
                .unwrap_or_default();
            TranscriptView {
                id: row.id,
                run_id: row.run_id,
                sequence: row.source_sequence,
                role: row.role,
                content,
                redactions,
                record: row.content,
                occurred_at: row.occurred_at,
            }
        })
        .collect())
}

fn normalize_content(record: &Value) -> Vec<Value> {
    let content = record
        .pointer("/message/content")
        .or_else(|| record.get("content"))
        .unwrap_or(record);
    match content {
        Value::Array(values) => values.clone(),
        Value::String(text) => vec![json!({"type":"text","text":text})],
        value => vec![json!({"type":"record","value":value})],
    }
}
