use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool};

use crate::auth::ProjectPrincipal;
use crate::error::ApiError;
use crate::idempotency;
use crate::services::tasks::new_id;

#[derive(Debug, Clone, FromRow)]
struct DecisionRow {
    id: String,
    project_id: String,
    task_id: String,
    run_id: String,
    kind: String,
    status: String,
    payload: Value,
    response_schema: Value,
    response: Option<Value>,
    deadline: Option<DateTime<Utc>>,
    version: i64,
    created_at: DateTime<Utc>,
    resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecisionView {
    pub id: String,
    pub project_id: String,
    pub task_id: String,
    pub run_id: String,
    pub kind: String,
    pub status: String,
    pub payload: Value,
    pub response_schema: Value,
    pub response: Option<Value>,
    pub deadline: Option<DateTime<Utc>>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

impl From<DecisionRow> for DecisionView {
    fn from(row: DecisionRow) -> Self {
        Self { id: row.id, project_id: row.project_id, task_id: row.task_id, run_id: row.run_id,
            kind: row.kind, status: row.status, payload: row.payload, response_schema: row.response_schema,
            response: row.response, deadline: row.deadline, version: row.version,
            created_at: row.created_at, resolved_at: row.resolved_at }
    }
}

#[derive(Debug, Serialize)]
pub struct DecisionPage { pub data: Vec<DecisionView> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionResponseRequest {
    pub action: String,
    #[serde(default)]
    pub answers: Value,
}

pub struct MutationResult { pub status: u16, pub body: Value }

const SELECT_VIEW: &str = "SELECT id,project_id,task_id,run_id,kind,status,payload,response_schema,response,deadline,version,created_at,resolved_at FROM decisions";

pub async fn list(pool: &PgPool, principal: &ProjectPrincipal, status: Option<&str>) -> Result<DecisionPage, ApiError> {
    expire_pending(pool, &principal.project_id).await?;
    let sql = format!("{SELECT_VIEW} WHERE organization_id=$1 AND project_id=$2 AND ($3::text IS NULL OR status=$3) ORDER BY created_at DESC,id DESC");
    let rows = sqlx::query_as::<_, DecisionRow>(&sql).bind(&principal.organization_id).bind(&principal.project_id)
        .bind(status).fetch_all(pool).await?;
    Ok(DecisionPage { data: rows.into_iter().map(Into::into).collect() })
}

pub async fn get(pool: &PgPool, principal: &ProjectPrincipal, id: &str) -> Result<DecisionView, ApiError> {
    expire_pending(pool, &principal.project_id).await?;
    let sql = format!("{SELECT_VIEW} WHERE id=$1 AND organization_id=$2 AND project_id=$3");
    sqlx::query_as::<_, DecisionRow>(&sql).bind(id).bind(&principal.organization_id).bind(&principal.project_id)
        .fetch_optional(pool).await?.map(Into::into).ok_or(ApiError::NotFound)
}

pub async fn respond(pool: &PgPool, principal: &ProjectPrincipal, id: &str, key: &str,
    expected_version: i64, request: DecisionResponseRequest) -> Result<MutationResult, ApiError> {
    let mut tx = pool.begin().await?;
    let endpoint = format!("POST /decisions/{id}/responses");
    let hash = idempotency::request_hash(&(expected_version, &request))?;
    if let Some(stored) = idempotency::lock_and_find(&mut tx, principal, &endpoint, key, &hash).await? {
        tx.commit().await?;
        return Ok(MutationResult { status: stored.status, body: stored.body });
    }
    let state = sqlx::query_as::<_, (String,i64,Option<DateTime<Utc>>,String,String,String)>(
        "SELECT status,version,deadline,runner_id,task_id,run_id FROM decisions WHERE id=$1 AND organization_id=$2 AND project_id=$3 FOR UPDATE")
        .bind(id).bind(&principal.organization_id).bind(&principal.project_id).fetch_optional(&mut *tx).await?
        .ok_or(ApiError::NotFound)?;
    if state.0 != "pending" { return Err(if state.0 == "expired" { ApiError::DecisionExpired } else { ApiError::DecisionAlreadyResolved }); }
    if state.2.is_some_and(|deadline| deadline <= Utc::now()) {
        sqlx::query("UPDATE decisions SET status='expired',version=version+1,resolved_at=now() WHERE id=$1 AND status='pending'")
            .bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        return Err(ApiError::DecisionExpired);
    }
    if state.1 != expected_version { return Err(ApiError::VersionConflict); }
    let terminal = match request.action.as_str() {
        "answer" | "allow" | "approve" => "answered",
        "decline" | "deny" | "reject" => "declined",
        "cancel" => "cancelled",
        _ => return Err(ApiError::Validation("unsupported Decision action".into())),
    };
    let response = serde_json::to_value(&request).map_err(|_| ApiError::Internal)?;
    let changed = sqlx::query_scalar::<_, i64>(
        "UPDATE decisions SET status=$1,response=$2,response_principal_id=$3,resolved_at=now(),version=version+1
         WHERE id=$4 AND status='pending' AND version=$5 RETURNING version")
        .bind(terminal).bind(&response).bind(&principal.api_key_id).bind(id).bind(expected_version)
        .fetch_optional(&mut *tx).await?;
    let Some(version) = changed else { return Err(ApiError::DecisionAlreadyResolved); };
    let sequence = sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(task_sequence),0)+1 FROM events WHERE task_id=$1")
        .bind(&state.4).fetch_one(&mut *tx).await?;
    sqlx::query("INSERT INTO events(id,organization_id,project_id,task_id,run_id,event_type,task_sequence,occurred_at,data,schema_version)
        VALUES($1,$2,$3,$4,$5,'decision.resolved',$6,now(),$7,1)")
        .bind(new_id("evt")).bind(&principal.organization_id).bind(&principal.project_id).bind(&state.4).bind(&state.5)
        .bind(sequence).bind(json!({"decision_id":id,"status":terminal,"version":version})).execute(&mut *tx).await?;
    let assignment = sqlx::query_scalar::<_, i64>("SELECT COALESCE(MAX(assignment_sequence),0)+1 FROM commands WHERE runner_id=$1")
        .bind(&state.3).fetch_one(&mut *tx).await?;
    sqlx::query("INSERT INTO commands(id,organization_id,project_id,task_id,run_id,runner_id,assignment_sequence,command_type,payload,expected_version,deadline)
        VALUES($1,$2,$3,$4,$5,$6,$7,'resolve_decision',$8,$9,now()+interval '7 days')")
        .bind(new_id("cmd")).bind(&principal.organization_id).bind(&principal.project_id).bind(&state.4).bind(&state.5)
        .bind(&state.3).bind(assignment).bind(json!({"decision_id":id,"response":response})).bind(expected_version).execute(&mut *tx).await?;
    let sql = format!("{SELECT_VIEW} WHERE id=$1");
    let view: DecisionView = sqlx::query_as::<_, DecisionRow>(&sql).bind(id).fetch_one(&mut *tx).await?.into();
    let body = serde_json::to_value(view).map_err(|_| ApiError::Internal)?;
    idempotency::store(&mut tx, principal, &endpoint, key, &hash, 202, &body).await?;
    tx.commit().await?;
    Ok(MutationResult { status: 202, body })
}

async fn expire_pending(pool: &PgPool, project_id: &str) -> Result<(), ApiError> {
    sqlx::query("UPDATE decisions SET status='expired',version=version+1,resolved_at=now() WHERE project_id=$1 AND status='pending' AND deadline <= now()")
        .bind(project_id).execute(pool).await?;
    Ok(())
}
