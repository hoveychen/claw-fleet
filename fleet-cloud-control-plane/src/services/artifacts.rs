use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};

use crate::auth::ProjectPrincipal;
use crate::error::ApiError;
use crate::services::tasks::new_id;

pub const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_TASK_ARTIFACT_BYTES: i64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteFault {
    None,
    BeforeObjectDelete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionResult {
    pub acquired: bool,
    pub transcripts_deleted: u64,
    pub artifacts_deleted: u64,
}

#[derive(Debug, Clone)]
pub struct ArtifactCrypto {
    master_key: [u8; 32],
    signing_key: [u8; 32],
    pub kms_key_id: String,
}

impl ArtifactCrypto {
    pub fn from_pepper(pepper: &[u8]) -> Self {
        fn derive(label: &[u8], pepper: &[u8]) -> [u8; 32] {
            let mut hash = Sha256::new();
            hash.update(label);
            hash.update([0]);
            hash.update(pepper);
            hash.finalize().into()
        }
        Self {
            master_key: derive(b"fleet-artifact-master", pepper),
            signing_key: derive(b"fleet-artifact-signing", pepper),
            kms_key_id: "fleet-local-kms-v1".into(),
        }
    }
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ArtifactView {
    pub id: String,
    pub project_id: String,
    pub task_id: String,
    pub run_id: Option<String>,
    pub kind: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub retention_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

const VIEW_COLUMNS: &str = "id,project_id,task_id,run_id,kind,filename,mime_type,size_bytes,sha256,retention_until,created_at";
const SELECT_VIEW: &str = "SELECT id,project_id,task_id,run_id,kind,filename,mime_type,size_bytes,sha256,retention_until,created_at FROM artifacts";

pub struct ArtifactUpload<'a> {
    pub task_id: &'a str,
    pub run_id: Option<&'a str>,
    pub kind: &'a str,
    pub filename: &'a str,
    pub mime_type: &'a str,
    pub bytes: &'a [u8],
}

pub async fn upload(
    pool: &PgPool,
    crypto: &ArtifactCrypto,
    principal: &ProjectPrincipal,
    upload: ArtifactUpload<'_>,
) -> Result<ArtifactView, ApiError> {
    let ArtifactUpload {
        task_id,
        run_id,
        kind,
        filename,
        mime_type,
        bytes,
    } = upload;
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(ApiError::Validation(
            "Artifact exceeds the 16 MiB file limit".into(),
        ));
    }
    if !allowed_mime(mime_type) {
        return Err(ApiError::Validation(
            "Artifact MIME type is not allowed".into(),
        ));
    }
    if !matches!(
        kind,
        "attachment" | "image" | "patch" | "report" | "log" | "wiki" | "other"
    ) {
        return Err(ApiError::Validation("invalid Artifact kind".into()));
    }
    if filename.is_empty()
        || filename.len() > 255
        || filename.contains('/')
        || filename.contains('\\')
    {
        return Err(ApiError::Validation("invalid Artifact filename".into()));
    }

    let mut tx = pool.begin().await?;
    let owned = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM tasks WHERE id=$1 AND organization_id=$2 AND project_id=$3 FOR UPDATE)",
    )
    .bind(task_id)
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .fetch_one(&mut *tx)
    .await?;
    if !owned {
        return Err(ApiError::NotFound);
    }
    if let Some(run_id) = run_id {
        let owned_run = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE id=$1 AND task_id=$2)",
        )
        .bind(run_id)
        .bind(task_id)
        .fetch_one(&mut *tx)
        .await?;
        if !owned_run {
            return Err(ApiError::NotFound);
        }
    }
    let used = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(SUM(size_bytes),0)::bigint FROM artifacts WHERE task_id=$1 AND status='active'",
    )
    .bind(task_id)
    .fetch_one(&mut *tx)
    .await?;
    if used.saturating_add(bytes.len() as i64) > MAX_TASK_ARTIFACT_BYTES {
        return Err(ApiError::Validation("Task Artifact quota exceeded".into()));
    }

    let artifact_id = new_id("art");
    let mut data_key = [0u8; 32];
    let mut object_nonce = [0u8; 12];
    let mut data_key_nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut data_key);
    rand::rngs::OsRng.fill_bytes(&mut object_nonce);
    rand::rngs::OsRng.fill_bytes(&mut data_key_nonce);
    let ciphertext = Aes256Gcm::new_from_slice(&data_key)
        .map_err(|_| ApiError::Internal)?
        .encrypt(
            Nonce::from_slice(&object_nonce),
            Payload {
                msg: bytes,
                aad: artifact_id.as_bytes(),
            },
        )
        .map_err(|_| ApiError::Internal)?;
    let encrypted_data_key = Aes256Gcm::new_from_slice(&crypto.master_key)
        .map_err(|_| ApiError::Internal)?
        .encrypt(
            Nonce::from_slice(&data_key_nonce),
            Payload {
                msg: &data_key,
                aad: artifact_id.as_bytes(),
            },
        )
        .map_err(|_| ApiError::Internal)?;
    data_key.fill(0);

    let digest = hex::encode(Sha256::digest(bytes));
    let sql = format!(
        "INSERT INTO artifacts(id,organization_id,project_id,task_id,run_id,kind,filename,mime_type,size_bytes,sha256)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING {}",
        VIEW_COLUMNS
    );
    let artifact = sqlx::query_as::<_, ArtifactView>(&sql)
        .bind(&artifact_id)
        .bind(&principal.organization_id)
        .bind(&principal.project_id)
        .bind(task_id)
        .bind(run_id)
        .bind(kind)
        .bind(filename)
        .bind(mime_type)
        .bind(bytes.len() as i64)
        .bind(digest)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO artifact_objects(artifact_id,object_key,ciphertext,object_nonce,encrypted_data_key,data_key_nonce,kms_key_id)
         VALUES($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(&artifact_id)
    .bind(format!("{}/{}/{}", principal.organization_id, task_id, artifact_id))
    .bind(ciphertext)
    .bind(object_nonce.to_vec())
    .bind(encrypted_data_key)
    .bind(data_key_nonce.to_vec())
    .bind(&crypto.kms_key_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(artifact)
}

pub async fn get(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    artifact_id: &str,
) -> Result<ArtifactView, ApiError> {
    let sql = format!(
        "{SELECT_VIEW} WHERE id=$1 AND organization_id=$2 AND project_id=$3 AND status='active'"
    );
    sqlx::query_as(&sql)
        .bind(artifact_id)
        .bind(&principal.organization_id)
        .bind(&principal.project_id)
        .fetch_optional(pool)
        .await?
        .ok_or(ApiError::NotFound)
}

pub async fn list_task(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    task_id: &str,
) -> Result<Vec<ArtifactView>, ApiError> {
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
    let sql = format!("{SELECT_VIEW} WHERE task_id=$1 AND status='active' ORDER BY created_at,id");
    Ok(sqlx::query_as(&sql).bind(task_id).fetch_all(pool).await?)
}

pub fn signed_download_url(
    crypto: &ArtifactCrypto,
    artifact_id: &str,
    principal_id: &str,
    expires_in_seconds: i64,
) -> Result<(String, DateTime<Utc>), ApiError> {
    if !(30..=900).contains(&expires_in_seconds) {
        return Err(ApiError::Validation(
            "expires_in_seconds must be between 30 and 900".into(),
        ));
    }
    let expires_at = Utc::now() + Duration::seconds(expires_in_seconds.min(300));
    let expires = expires_at.timestamp();
    let payload = format!("{artifact_id}\n{principal_id}\n{expires}");
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&crypto.signing_key)
        .map_err(|_| ApiError::Internal)?;
    mac.update(payload.as_bytes());
    let signature =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok((
        format!("/artifact-downloads/{artifact_id}?expires={expires}&principal={principal_id}&signature={signature}"),
        expires_at,
    ))
}

pub fn verify_download_signature(
    crypto: &ArtifactCrypto,
    artifact_id: &str,
    principal_id: &str,
    expires: i64,
    signature: &str,
) -> Result<(), ApiError> {
    if expires <= Utc::now().timestamp() {
        return Err(ApiError::NotFound);
    }
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| ApiError::NotFound)?;
    let payload = format!("{artifact_id}\n{principal_id}\n{expires}");
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&crypto.signing_key)
        .map_err(|_| ApiError::Internal)?;
    mac.update(payload.as_bytes());
    mac.verify_slice(&decoded).map_err(|_| ApiError::NotFound)
}

pub async fn download(
    pool: &PgPool,
    crypto: &ArtifactCrypto,
    principal: &ProjectPrincipal,
    artifact_id: &str,
) -> Result<(ArtifactView, Vec<u8>), ApiError> {
    let artifact = get(pool, principal, artifact_id).await?;
    let row = sqlx::query_as::<_, (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, String)>(
        "SELECT ciphertext,object_nonce,encrypted_data_key,data_key_nonce,kms_key_id FROM artifact_objects WHERE artifact_id=$1",
    )
    .bind(artifact_id).fetch_optional(pool).await?.ok_or(ApiError::NotFound)?;
    if row.4 != crypto.kms_key_id || row.1.len() != 12 || row.3.len() != 12 {
        return Err(ApiError::Internal);
    }
    let mut data_key = Aes256Gcm::new_from_slice(&crypto.master_key)
        .map_err(|_| ApiError::Internal)?
        .decrypt(
            Nonce::from_slice(&row.3),
            Payload {
                msg: &row.2,
                aad: artifact_id.as_bytes(),
            },
        )
        .map_err(|_| ApiError::Internal)?;
    let plaintext = Aes256Gcm::new_from_slice(&data_key)
        .map_err(|_| ApiError::Internal)?
        .decrypt(
            Nonce::from_slice(&row.1),
            Payload {
                msg: &row.0,
                aad: artifact_id.as_bytes(),
            },
        )
        .map_err(|_| ApiError::Internal)?;
    data_key.fill(0);
    if hex::encode(Sha256::digest(&plaintext)) != artifact.sha256 {
        return Err(ApiError::Internal);
    }
    Ok((artifact, plaintext))
}

fn allowed_mime(mime: &str) -> bool {
    matches!(
        mime,
        "text/plain"
            | "text/markdown"
            | "text/html"
            | "text/x-diff"
            | "application/json"
            | "application/pdf"
            | "application/octet-stream"
            | "image/png"
            | "image/jpeg"
            | "image/webp"
    )
}

pub async fn delete_task_content(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    task_id: &str,
    fault: DeleteFault,
) -> Result<(), ApiError> {
    let mut prepare = pool.begin().await?;
    let task_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM tasks WHERE id=$1 AND organization_id=$2 AND project_id=$3 FOR UPDATE)",
    )
    .bind(task_id)
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .fetch_one(&mut *prepare)
    .await?;
    if !task_exists {
        let completed = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM audit_records WHERE organization_id=$1 AND project_id=$2 AND resource_type='task' AND resource_id=$3 AND action='task.content_deleted')",
        )
        .bind(&principal.organization_id)
        .bind(&principal.project_id)
        .bind(task_id)
        .fetch_one(&mut *prepare)
        .await?;
        if completed {
            prepare.rollback().await?;
            return Ok(());
        }
        return Err(ApiError::NotFound);
    }
    let artifact_bytes = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(SUM(size_bytes),0)::bigint FROM artifacts WHERE task_id=$1 AND status IN ('active','deleting')",
    )
    .bind(task_id)
    .fetch_one(&mut *prepare)
    .await?;
    let usage = sqlx::query_as::<_, (i64, i64, f64, i64)>(
        "SELECT
            COALESCE(SUM(CASE WHEN event_type='usage.updated' THEN COALESCE((data->>'input_tokens')::bigint,0) ELSE 0 END),0)::bigint,
            COALESCE(SUM(CASE WHEN event_type='usage.updated' THEN COALESCE((data->>'output_tokens')::bigint,0) ELSE 0 END),0)::bigint,
            COALESCE(SUM(CASE WHEN event_type='usage.updated' THEN COALESCE((data->>'cost_usd')::double precision,0) ELSE 0 END),0)::double precision,
            COALESCE(SUM(octet_length(data::text)),0)::bigint
         FROM events WHERE task_id=$1",
    )
    .bind(task_id)
    .fetch_one(&mut *prepare)
    .await?;
    sqlx::query(
        "INSERT INTO task_usage_summaries(task_id,organization_id,project_id,input_tokens,output_tokens,provider_cost_usd,event_bytes,artifact_bytes)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8)
         ON CONFLICT(task_id) DO UPDATE SET
            input_tokens=GREATEST(task_usage_summaries.input_tokens,excluded.input_tokens),
            output_tokens=GREATEST(task_usage_summaries.output_tokens,excluded.output_tokens),
            provider_cost_usd=GREATEST(task_usage_summaries.provider_cost_usd,excluded.provider_cost_usd),
            event_bytes=GREATEST(task_usage_summaries.event_bytes,excluded.event_bytes),
            artifact_bytes=GREATEST(task_usage_summaries.artifact_bytes,excluded.artifact_bytes),updated_at=now()",
    )
    .bind(task_id)
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .bind(usage.0)
    .bind(usage.1)
    .bind(usage.2)
    .bind(usage.3)
    .bind(artifact_bytes)
    .execute(&mut *prepare)
    .await?;
    let requested = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM audit_records WHERE organization_id=$1 AND project_id=$2 AND resource_type='task' AND resource_id=$3 AND action='task.content_delete_requested')",
    )
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .bind(task_id)
    .fetch_one(&mut *prepare)
    .await?;
    if !requested {
        sqlx::query(
            "INSERT INTO audit_records(organization_id,project_id,principal_type,principal_id,request_id,action,resource_type,resource_id,details)
             VALUES($1,$2,'api_key',$3,$4,'task.content_delete_requested','task',$5,$6)",
        )
        .bind(&principal.organization_id)
        .bind(&principal.project_id)
        .bind(&principal.api_key_id)
        .bind(new_id("req"))
        .bind(task_id)
        .bind(serde_json::json!({"artifact_bytes":artifact_bytes}))
        .execute(&mut *prepare)
        .await?;
    }
    sqlx::query("UPDATE artifacts SET status='deleting' WHERE task_id=$1 AND status='active'")
        .bind(task_id)
        .execute(&mut *prepare)
        .await?;
    prepare.commit().await?;

    if fault == DeleteFault::BeforeObjectDelete {
        return Err(ApiError::StorageUnavailable);
    }

    let mut cleanup = pool.begin().await?;
    sqlx::query(
        "DELETE FROM artifact_objects WHERE artifact_id IN (SELECT id FROM artifacts WHERE task_id=$1)",
    )
    .bind(task_id)
    .execute(&mut *cleanup)
    .await?;
    sqlx::query("DELETE FROM transcript_records WHERE task_id=$1")
        .bind(task_id)
        .execute(&mut *cleanup)
        .await?;
    sqlx::query("UPDATE task_usage_summaries SET content_deleted_at=now(),updated_at=now() WHERE task_id=$1")
        .bind(task_id)
        .execute(&mut *cleanup)
        .await?;
    sqlx::query(
        "INSERT INTO audit_records(organization_id,project_id,principal_type,principal_id,request_id,action,resource_type,resource_id)
         VALUES($1,$2,'api_key',$3,$4,'task.content_deleted','task',$5)",
    )
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .bind(&principal.api_key_id)
    .bind(new_id("req"))
    .bind(task_id)
    .execute(&mut *cleanup)
    .await?;
    sqlx::query("DELETE FROM tasks WHERE id=$1 AND organization_id=$2 AND project_id=$3")
        .bind(task_id)
        .bind(&principal.organization_id)
        .bind(&principal.project_id)
        .execute(&mut *cleanup)
        .await?;
    cleanup.commit().await?;
    Ok(())
}

pub async fn delete_artifact(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    artifact_id: &str,
) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    let changed = sqlx::query(
        "UPDATE artifacts SET status='deleting' WHERE id=$1 AND organization_id=$2 AND project_id=$3 AND status='active'",
    )
    .bind(artifact_id)
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    sqlx::query("DELETE FROM artifact_objects WHERE artifact_id=$1")
        .bind(artifact_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE artifacts SET status='deleted',deleted_at=now() WHERE id=$1")
        .bind(artifact_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn try_acquire_retention_lease(pool: &PgPool, owner: &str) -> Result<bool, ApiError> {
    let acquired = sqlx::query_scalar::<_, String>(
        "INSERT INTO retention_jobs(job_name,lease_owner,lease_until,last_started_at)
         VALUES('daily-content-retention',$1,now()+interval '5 minutes',now())
         ON CONFLICT(job_name) DO UPDATE SET
            lease_owner=excluded.lease_owner,lease_until=excluded.lease_until,
            last_started_at=excluded.last_started_at,updated_at=now(),last_error=NULL
         WHERE retention_jobs.lease_until IS NULL OR retention_jobs.lease_until<now() OR retention_jobs.lease_owner=$1
         RETURNING job_name",
    )
    .bind(owner)
    .fetch_optional(pool)
    .await?;
    Ok(acquired.is_some())
}

pub async fn release_retention_lease(
    pool: &PgPool,
    owner: &str,
    error: Option<&str>,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE retention_jobs SET lease_owner=NULL,lease_until=NULL,
         last_completed_at=CASE WHEN $2::text IS NULL THEN now() ELSE last_completed_at END,
         last_error=$2,updated_at=now()
         WHERE job_name='daily-content-retention' AND lease_owner=$1",
    )
    .bind(owner)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn run_retention(pool: &PgPool, owner: &str) -> Result<RetentionResult, ApiError> {
    if !try_acquire_retention_lease(pool, owner).await? {
        return Ok(RetentionResult {
            acquired: false,
            transcripts_deleted: 0,
            artifacts_deleted: 0,
        });
    }
    let result = async {
        let mut tx = pool.begin().await?;
        let artifacts_deleted = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artifacts WHERE status='active' AND retention_until<=now()",
        )
        .fetch_one(&mut *tx)
        .await? as u64;
        sqlx::query(
            "DELETE FROM artifact_objects WHERE artifact_id IN (SELECT id FROM artifacts WHERE status='active' AND retention_until<=now())",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE artifacts SET status='deleted',deleted_at=now() WHERE status='active' AND retention_until<=now()",
        )
        .execute(&mut *tx)
        .await?;
        let transcripts_deleted = sqlx::query(
            "DELETE FROM transcript_records WHERE retention_until<=now()",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok::<_, ApiError>(RetentionResult {
            acquired: true,
            transcripts_deleted,
            artifacts_deleted,
        })
    }
    .await;
    match result {
        Ok(result) => {
            release_retention_lease(pool, owner, None).await?;
            Ok(result)
        }
        Err(error) => {
            let message = error.to_string();
            let _ = release_retention_lease(pool, owner, Some(&message)).await;
            Err(error)
        }
    }
}
