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

#[derive(Debug, Clone)]
pub struct S3ObjectStore {
    endpoint: String,
    bucket: String,
    region: String,
    access_key: String,
    secret_key: String,
    client: reqwest::Client,
}

#[derive(Debug, Clone)]
pub enum ArtifactObjectStore {
    Postgres,
    S3(S3ObjectStore),
}

impl S3ObjectStore {
    pub fn new(
        endpoint: String,
        bucket: String,
        region: String,
        access_key: String,
        secret_key: String,
    ) -> Result<Self, ApiError> {
        let parsed = reqwest::Url::parse(&endpoint)
            .map_err(|_| ApiError::Validation("invalid Artifact S3 endpoint".into()))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(ApiError::Validation(
                "Artifact S3 endpoint must be an HTTP(S) origin".into(),
            ));
        }
        if bucket.is_empty()
            || bucket.contains('/')
            || access_key.is_empty()
            || secret_key.is_empty()
        {
            return Err(ApiError::Validation(
                "invalid Artifact S3 bucket or credentials".into(),
            ));
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ApiError::Internal)?;
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').into(),
            bucket,
            region,
            access_key,
            secret_key,
            client,
        })
    }

    async fn put(&self, object_key: &str, bytes: Vec<u8>) -> Result<(), ApiError> {
        self.request(reqwest::Method::PUT, object_key, bytes)
            .await
            .map(|_| ())
    }

    async fn get(&self, object_key: &str) -> Result<Vec<u8>, ApiError> {
        self.request(reqwest::Method::GET, object_key, Vec::new())
            .await
    }

    async fn delete(&self, object_key: &str) -> Result<(), ApiError> {
        self.request(reqwest::Method::DELETE, object_key, Vec::new())
            .await
            .map(|_| ())
    }

    async fn request(
        &self,
        method: reqwest::Method,
        object_key: &str,
        body: Vec<u8>,
    ) -> Result<Vec<u8>, ApiError> {
        let url = reqwest::Url::parse(&format!(
            "{}/{}/{}",
            self.endpoint,
            self.bucket,
            object_key.trim_start_matches('/')
        ))
        .map_err(|_| ApiError::Internal)?;
        let host = match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_owned(),
            _ => return Err(ApiError::Internal),
        };
        let now = Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = now.format("%Y%m%d").to_string();
        let payload_hash = hex::encode(Sha256::digest(&body));
        let canonical_headers =
            format!("host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n");
        let signed_headers = "host;x-amz-content-sha256;x-amz-date";
        let canonical_request = format!(
            "{}\n{}\n\n{}\n{}\n{}",
            method.as_str(),
            url.path(),
            canonical_headers,
            signed_headers,
            payload_hash
        );
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
            hex::encode(Sha256::digest(canonical_request.as_bytes()))
        );
        fn hmac(key: &[u8], value: &[u8]) -> Result<Vec<u8>, ApiError> {
            let mut mac =
                <Hmac<Sha256> as Mac>::new_from_slice(key).map_err(|_| ApiError::Internal)?;
            mac.update(value);
            Ok(mac.finalize().into_bytes().to_vec())
        }
        let date_key = hmac(
            format!("AWS4{}", self.secret_key).as_bytes(),
            date.as_bytes(),
        )?;
        let region_key = hmac(&date_key, self.region.as_bytes())?;
        let service_key = hmac(&region_key, b"s3")?;
        let signing_key = hmac(&service_key, b"aws4_request")?;
        let signature = hex::encode(hmac(&signing_key, string_to_sign.as_bytes())?);
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            self.access_key
        );
        let response = self
            .client
            .request(method, url)
            .header("x-amz-date", amz_date)
            .header("x-amz-content-sha256", payload_hash)
            .header(reqwest::header::AUTHORIZATION, authorization)
            .body(body)
            .send()
            .await
            .map_err(|_| ApiError::StorageUnavailable)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ApiError::NotFound);
        }
        if !response.status().is_success() {
            return Err(ApiError::StorageUnavailable);
        }
        Ok(response
            .bytes()
            .await
            .map_err(|_| ApiError::StorageUnavailable)?
            .to_vec())
    }
}

impl ArtifactObjectStore {
    async fn put(&self, object_key: &str, ciphertext: Vec<u8>) -> Result<(), ApiError> {
        match self {
            Self::Postgres => Ok(()),
            Self::S3(store) => store.put(object_key, ciphertext).await,
        }
    }

    async fn get(
        &self,
        backend: &str,
        object_key: &str,
        postgres_ciphertext: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, ApiError> {
        match (backend, self) {
            ("postgres", _) => postgres_ciphertext.ok_or(ApiError::Internal),
            ("s3", Self::S3(store)) => store.get(object_key).await,
            _ => Err(ApiError::StorageUnavailable),
        }
    }

    async fn delete(&self, backend: &str, object_key: &str) -> Result<(), ApiError> {
        match (backend, self) {
            ("postgres", _) => Ok(()),
            ("s3", Self::S3(store)) => store.delete(object_key).await,
            _ => Err(ApiError::StorageUnavailable),
        }
    }

    fn backend(&self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::S3(_) => "s3",
        }
    }
}

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
    store: &ArtifactObjectStore,
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
    let project_limit =
        crate::services::governance::artifact_byte_limit(&mut tx, principal).await?;
    let project_used = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(SUM(size_bytes),0)::bigint FROM artifacts WHERE project_id=$1 AND status='active'",
    )
    .bind(&principal.project_id)
    .fetch_one(&mut *tx)
    .await?;
    if used.saturating_add(bytes.len() as i64) > MAX_TASK_ARTIFACT_BYTES
        || project_used.saturating_add(bytes.len() as i64) > project_limit
    {
        return Err(ApiError::QuotaExceeded);
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
    let object_key = format!("{}/{}/{}", principal.organization_id, task_id, artifact_id);
    let backend = store.backend();
    let postgres_ciphertext = if backend == "s3" {
        store.put(&object_key, ciphertext).await?;
        None
    } else {
        Some(ciphertext)
    };
    let inserted = sqlx::query(
        "INSERT INTO artifact_objects(artifact_id,object_key,ciphertext,object_nonce,encrypted_data_key,data_key_nonce,kms_key_id,storage_backend)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8)",
    )
    .bind(&artifact_id)
    .bind(&object_key)
    .bind(postgres_ciphertext)
    .bind(object_nonce.to_vec())
    .bind(encrypted_data_key)
    .bind(data_key_nonce.to_vec())
    .bind(&crypto.kms_key_id)
    .bind(backend)
    .execute(&mut *tx)
    .await;
    if let Err(error) = inserted {
        let _ = store.delete(backend, &object_key).await;
        return Err(error.into());
    }
    if let Err(error) = tx.commit().await {
        let _ = store.delete(backend, &object_key).await;
        return Err(error.into());
    }
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
    store: &ArtifactObjectStore,
    principal: &ProjectPrincipal,
    artifact_id: &str,
) -> Result<(ArtifactView, Vec<u8>), ApiError> {
    let artifact = get(pool, principal, artifact_id).await?;
    let row = sqlx::query_as::<_, (String, String, Option<Vec<u8>>, Vec<u8>, Vec<u8>, Vec<u8>, String)>(
        "SELECT storage_backend,object_key,ciphertext,object_nonce,encrypted_data_key,data_key_nonce,kms_key_id FROM artifact_objects WHERE artifact_id=$1",
    )
    .bind(artifact_id).fetch_optional(pool).await?.ok_or(ApiError::NotFound)?;
    if row.6 != crypto.kms_key_id || row.3.len() != 12 || row.5.len() != 12 {
        return Err(ApiError::Internal);
    }
    let ciphertext = store.get(&row.0, &row.1, row.2).await?;
    let mut data_key = Aes256Gcm::new_from_slice(&crypto.master_key)
        .map_err(|_| ApiError::Internal)?
        .decrypt(
            Nonce::from_slice(&row.5),
            Payload {
                msg: &row.4,
                aad: artifact_id.as_bytes(),
            },
        )
        .map_err(|_| ApiError::Internal)?;
    let plaintext = Aes256Gcm::new_from_slice(&data_key)
        .map_err(|_| ApiError::Internal)?
        .decrypt(
            Nonce::from_slice(&row.3),
            Payload {
                msg: &ciphertext,
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
    store: &ArtifactObjectStore,
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

    let objects = sqlx::query_as::<_, (String, String)>(
        "SELECT storage_backend,object_key FROM artifact_objects WHERE artifact_id IN (SELECT id FROM artifacts WHERE task_id=$1)",
    )
    .bind(task_id)
    .fetch_all(pool)
    .await?;
    for (backend, object_key) in &objects {
        store.delete(backend, object_key).await?;
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
    store: &ArtifactObjectStore,
    principal: &ProjectPrincipal,
    artifact_id: &str,
) -> Result<(), ApiError> {
    let mut prepare = pool.begin().await?;
    let object = sqlx::query_as::<_, (String, String)>(
        "SELECT o.storage_backend,o.object_key FROM artifact_objects o JOIN artifacts a ON a.id=o.artifact_id
         WHERE a.id=$1 AND a.organization_id=$2 AND a.project_id=$3 AND a.status IN ('active','deleting') FOR UPDATE OF a",
    )
    .bind(artifact_id)
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .fetch_optional(&mut *prepare)
    .await?;
    let (backend, object_key) = object.ok_or(ApiError::NotFound)?;
    sqlx::query("UPDATE artifacts SET status='deleting' WHERE id=$1")
        .bind(artifact_id)
        .execute(&mut *prepare)
        .await?;
    prepare.commit().await?;
    store.delete(&backend, &object_key).await?;

    let mut cleanup = pool.begin().await?;
    sqlx::query("DELETE FROM artifact_objects WHERE artifact_id=$1")
        .bind(artifact_id)
        .execute(&mut *cleanup)
        .await?;
    sqlx::query("UPDATE artifacts SET status='deleted',deleted_at=now() WHERE id=$1")
        .bind(artifact_id)
        .execute(&mut *cleanup)
        .await?;
    cleanup.commit().await?;
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

pub async fn run_retention(
    pool: &PgPool,
    store: &ArtifactObjectStore,
    owner: &str,
) -> Result<RetentionResult, ApiError> {
    if !try_acquire_retention_lease(pool, owner).await? {
        return Ok(RetentionResult {
            acquired: false,
            transcripts_deleted: 0,
            artifacts_deleted: 0,
        });
    }
    let result = async {
        let objects = sqlx::query_as::<_, (String, String)>(
            "SELECT o.storage_backend,o.object_key FROM artifact_objects o JOIN artifacts a ON a.id=o.artifact_id WHERE a.status='active' AND a.retention_until<=now() ORDER BY a.id",
        )
        .fetch_all(pool)
        .await?;
        for (backend, object_key) in &objects {
            store.delete(backend, object_key).await?;
        }
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
