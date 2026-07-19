use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use std::collections::HashMap;
use uuid::Uuid;

use crate::auth::ProjectPrincipal;
use crate::error::ApiError;
use crate::idempotency;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSpec {
    pub repository: String,
    #[serde(default = "default_ref")]
    pub r#ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<String>,
}

fn default_ref() -> String {
    "main".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_policy_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub goal: String,
    pub workspace: WorkspaceSpec,
    pub agent: AgentSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_pool_id: Option<String>,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, FromRow)]
pub(crate) struct TaskRow {
    id: String,
    project_id: String,
    external_id: Option<String>,
    title: Option<String>,
    goal: String,
    status: String,
    active_run_id: Option<String>,
    metadata: Value,
    version: i64,
    created_by_type: String,
    created_by_id: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    pub id: String,
    pub project_id: String,
    pub external_id: Option<String>,
    pub title: Option<String>,
    pub goal: String,
    pub status: String,
    pub active_run_id: Option<String>,
    pub waiting_decision_ids: Vec<String>,
    pub metadata: Value,
    pub version: i64,
    pub created_by: PrincipalView,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrincipalView {
    pub r#type: String,
    pub id: String,
}

impl From<TaskRow> for TaskView {
    fn from(row: TaskRow) -> Self {
        Self {
            id: row.id,
            project_id: row.project_id,
            external_id: row.external_id,
            title: row.title,
            goal: row.goal,
            status: row.status,
            active_run_id: row.active_run_id,
            waiting_decision_ids: Vec::new(),
            metadata: row.metadata,
            version: row.version,
            created_by: PrincipalView {
                r#type: row.created_by_type,
                id: row.created_by_id,
            },
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RunView {
    pub id: String,
    pub task_id: String,
    pub attempt: i32,
    pub status: String,
    pub agent: AgentSpec,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateTaskResponse {
    pub task: TaskView,
    pub run: RunView,
}

pub struct CreateResult {
    pub status: u16,
    pub body: Value,
}

pub type MutationResult = CreateResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendMessageRequest {
    pub text: String,
    #[serde(default)]
    pub attachment_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_override: Option<AgentSpec>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskControlRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResumeTaskRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentSpec>,
}

#[derive(Debug, Serialize)]
pub struct TaskPage {
    pub data: Vec<TaskView>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, FromRow, Serialize)]
pub struct EventView {
    pub id: String,
    pub cursor: String,
    pub organization_id: String,
    pub project_id: String,
    pub task_id: Option<String>,
    pub run_id: Option<String>,
    #[sqlx(rename = "event_type")]
    #[serde(rename = "type")]
    pub event_type: String,
    #[sqlx(rename = "task_sequence")]
    #[serde(rename = "sequence")]
    pub task_sequence: Option<i64>,
    pub occurred_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
    pub data: Value,
    pub schema_version: i32,
}

#[derive(Debug, Serialize)]
pub struct EventPage {
    pub data: Vec<EventView>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

pub async fn list_tasks(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    status: Option<&str>,
    external_id: Option<&str>,
    cursor: Option<&str>,
    limit: i64,
) -> Result<TaskPage, ApiError> {
    if status.is_some_and(|s| {
        !matches!(
            s,
            "queued"
                | "running"
                | "waiting_input"
                | "paused"
                | "succeeded"
                | "failed"
                | "cancelled"
        )
    }) {
        return Err(ApiError::Validation("invalid task status".into()));
    }
    let boundary = if let Some(cursor) = cursor {
        Some(sqlx::query_as::<_, (DateTime<Utc>, String)>("SELECT updated_at, id FROM tasks WHERE organization_id=$1 AND project_id=$2 AND id=$3")
            .bind(&principal.organization_id).bind(&principal.project_id).bind(cursor).fetch_optional(pool).await?.ok_or(ApiError::Validation("invalid cursor".into()))?)
    } else {
        None
    };
    let (boundary_time, boundary_id) = boundary
        .map(|(time, id)| (Some(time), Some(id)))
        .unwrap_or((None, None));
    let mut rows = sqlx::query_as::<_, TaskRow>(
        "SELECT id, project_id, external_id, title, goal, status, active_run_id, metadata, version, created_by_type, created_by_id, created_at, updated_at
         FROM tasks WHERE organization_id=$1 AND project_id=$2
           AND ($3::text IS NULL OR status=$3) AND ($4::text IS NULL OR external_id=$4)
           AND ($5::timestamptz IS NULL OR (updated_at, id) < ($5, $6))
         ORDER BY updated_at DESC, id DESC LIMIT $7")
        .bind(&principal.organization_id).bind(&principal.project_id).bind(status).bind(external_id)
        .bind(boundary_time).bind(boundary_id).bind(limit + 1).fetch_all(pool).await?;
    let has_more = rows.len() as i64 > limit;
    if has_more {
        rows.pop();
    }
    let next_cursor = has_more
        .then(|| rows.last().map(|row| row.id.clone()))
        .flatten();
    let mut data: Vec<TaskView> = rows.into_iter().map(Into::into).collect();
    attach_waiting_decisions(
        pool,
        &principal.organization_id,
        &principal.project_id,
        &mut data,
    )
    .await?;
    Ok(TaskPage {
        data,
        next_cursor,
        has_more,
    })
}

pub async fn get_task(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    task_id: &str,
) -> Result<TaskView, ApiError> {
    let row = sqlx::query_as::<_, TaskRow>(
        "SELECT id, project_id, external_id, title, goal, status, active_run_id, metadata, version, created_by_type, created_by_id, created_at, updated_at
         FROM tasks WHERE organization_id=$1 AND project_id=$2 AND id=$3")
        .bind(&principal.organization_id).bind(&principal.project_id).bind(task_id).fetch_optional(pool).await?.ok_or(ApiError::NotFound)?;
    let mut views = vec![row.into()];
    attach_waiting_decisions(
        pool,
        &principal.organization_id,
        &principal.project_id,
        &mut views,
    )
    .await?;
    Ok(views.remove(0))
}

async fn attach_waiting_decisions(
    pool: &PgPool,
    organization_id: &str,
    project_id: &str,
    tasks: &mut [TaskView],
) -> Result<(), ApiError> {
    if tasks.is_empty() {
        return Ok(());
    }
    let ids: Vec<&str> = tasks.iter().map(|task| task.id.as_str()).collect();
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT task_id,id FROM decisions WHERE organization_id=$1 AND project_id=$2
         AND task_id=ANY($3) AND status IN ('pending','answer_queued') ORDER BY created_at,id",
    )
    .bind(organization_id)
    .bind(project_id)
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    let mut grouped: HashMap<String, Vec<String>> = HashMap::new();
    for (task_id, decision_id) in rows {
        grouped.entry(task_id).or_default().push(decision_id);
    }
    for task in tasks {
        task.waiting_decision_ids = grouped.remove(&task.id).unwrap_or_default();
    }
    Ok(())
}

pub async fn create_task(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    idempotency_key: &str,
    request: CreateTaskRequest,
) -> Result<CreateResult, ApiError> {
    if request.project_id != principal.project_id {
        return Err(ApiError::PermissionDenied);
    }
    if request.goal.trim().is_empty() {
        return Err(ApiError::Validation("goal must not be empty".into()));
    }
    if !matches!(request.agent.provider.as_str(), "claude_code" | "codex") {
        return Err(ApiError::Validation("unsupported agent provider".into()));
    }

    let request_hash = idempotency::request_hash(&request)?;
    let mut tx = pool.begin().await?;
    if let Some(stored) = idempotency::lock_and_find(
        &mut tx,
        principal,
        "POST /tasks",
        idempotency_key,
        &request_hash,
    )
    .await?
    {
        tx.commit().await?;
        return Ok(CreateResult {
            status: stored.status,
            body: stored.body,
        });
    }
    crate::services::governance::enforce_run_quota(&mut tx, principal).await?;
    if let Some(runner_pool_id) = request.runner_pool_id.as_deref() {
        let belongs_to_project = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM runner_pools WHERE id=$1 AND organization_id=$2 AND project_id=$3)",
        )
        .bind(runner_pool_id)
        .bind(&principal.organization_id)
        .bind(&principal.project_id)
        .fetch_one(&mut *tx)
        .await?;
        if !belongs_to_project {
            return Err(ApiError::Validation(
                "runner_pool_id is not available to this project".into(),
            ));
        }
    }

    let task_id = new_id("task");
    let run_id = new_id("run");
    let event_id = new_id("evt");
    let command_id = new_id("cmd");
    let now = Utc::now();

    let task_row = sqlx::query_as::<_, TaskRow>(
        "INSERT INTO tasks(
            id, organization_id, project_id, external_id, title, goal, workspace,
            status, metadata, created_by_type, created_by_id, created_at, updated_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'queued', $8, 'api_key', $9, $10, $10)
         RETURNING id, project_id, external_id, title, goal, status, active_run_id,
                   metadata, version, created_by_type, created_by_id, created_at, updated_at",
    )
    .bind(&task_id)
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .bind(&request.external_id)
    .bind(&request.title)
    .bind(&request.goal)
    .bind(serde_json::to_value(&request.workspace).map_err(|_| ApiError::Internal)?)
    .bind(&request.metadata)
    .bind(&principal.api_key_id)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO runs(
            id, organization_id, project_id, task_id, attempt, status, provider,
            model, effort, permission_policy_id, created_at, updated_at
         ) VALUES ($1, $2, $3, $4, 1, 'assigned', $5, $6, $7, $8, $9, $9)",
    )
    .bind(&run_id)
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .bind(&task_id)
    .bind(&request.agent.provider)
    .bind(&request.agent.model)
    .bind(&request.agent.effort)
    .bind(&request.agent.permission_policy_id)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    sqlx::query("UPDATE tasks SET active_run_id = $1 WHERE id = $2")
        .bind(&run_id)
        .bind(&task_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        "INSERT INTO events(
            id, organization_id, project_id, task_id, run_id, event_type,
            task_sequence, occurred_at, data, schema_version
         ) VALUES ($1, $2, $3, $4, $5, 'task.created', 1, $6, $7, 1)",
    )
    .bind(&event_id)
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .bind(&task_id)
    .bind(&run_id)
    .bind(now)
    .bind(json!({"status": "queued"}))
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO commands(
            id, organization_id, project_id, task_id, run_id, command_type,
            payload, expected_version, runner_pool_id
         ) VALUES ($1, $2, $3, $4, $5, 'start_run', $6, 1, $7)",
    )
    .bind(&command_id)
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .bind(&task_id)
    .bind(&run_id)
    .bind(json!({
        "workspace": request.workspace,
        "agent": request.agent,
        "goal": request.goal
    }))
    .bind(&request.runner_pool_id)
    .execute(&mut *tx)
    .await?;

    let task = TaskView {
        active_run_id: Some(run_id.clone()),
        ..task_row.into()
    };
    let run = RunView {
        id: run_id,
        task_id,
        attempt: 1,
        status: "assigned".into(),
        agent: request.agent,
        version: 1,
        created_at: now,
        updated_at: now,
    };
    let body =
        serde_json::to_value(CreateTaskResponse { task, run }).map_err(|_| ApiError::Internal)?;
    idempotency::store(
        &mut tx,
        principal,
        "POST /tasks",
        idempotency_key,
        &request_hash,
        202,
        &body,
    )
    .await?;
    tx.commit().await?;

    Ok(CreateResult { status: 202, body })
}

pub async fn append_message(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    task_id: &str,
    key: &str,
    request: AppendMessageRequest,
) -> Result<MutationResult, ApiError> {
    if request.text.trim().is_empty() {
        return Err(ApiError::Validation("text must not be empty".into()));
    }
    if request.text.len() > 100_000 || request.attachment_ids.len() > 20 {
        return Err(ApiError::Validation("message exceeds API limits".into()));
    }
    mutate_task(
        pool,
        principal,
        MutationSpec {
            task_id,
            key,
            expected_version: None,
            action: "message",
            request: &request,
            payload: json!({
                "text": request.text, "attachment_ids": request.attachment_ids, "agent_override": request.agent_override
            }),
        },
    )
    .await
}

pub async fn control_task(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    task_id: &str,
    key: &str,
    expected_version: Option<i64>,
    action: &str,
    request: TaskControlRequest,
) -> Result<MutationResult, ApiError> {
    if !matches!(action, "cancel" | "pause") {
        return Err(ApiError::Validation("unsupported task action".into()));
    }
    if request
        .reason
        .as_ref()
        .is_some_and(|reason| reason.len() > 2_000)
    {
        return Err(ApiError::Validation("reason exceeds API limit".into()));
    }
    mutate_task(
        pool,
        principal,
        MutationSpec {
            task_id,
            key,
            expected_version,
            action,
            request: &request,
            payload: json!({"reason": request.reason}),
        },
    )
    .await
}

pub async fn resume_task(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    task_id: &str,
    key: &str,
    expected_version: Option<i64>,
    request: ResumeTaskRequest,
) -> Result<MutationResult, ApiError> {
    if request
        .message
        .as_ref()
        .is_some_and(|message| message.len() > 100_000)
    {
        return Err(ApiError::Validation("message exceeds API limit".into()));
    }
    mutate_task(
        pool,
        principal,
        MutationSpec {
            task_id,
            key,
            expected_version,
            action: "resume",
            request: &request,
            payload: json!({
                "message": request.message, "agent": request.agent
            }),
        },
    )
    .await
}

struct MutationSpec<'a, T> {
    task_id: &'a str,
    key: &'a str,
    expected_version: Option<i64>,
    action: &'a str,
    request: &'a T,
    payload: Value,
}

async fn mutate_task<T: Serialize>(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    spec: MutationSpec<'_, T>,
) -> Result<MutationResult, ApiError> {
    let MutationSpec {
        task_id,
        key,
        expected_version,
        action,
        request,
        payload,
    } = spec;
    let endpoint = format!("POST /tasks/{task_id}/{action}");
    let request_hash = idempotency::request_hash(&(expected_version, request))?;
    let mut tx = pool.begin().await?;
    if let Some(stored) =
        idempotency::lock_and_find(&mut tx, principal, &endpoint, key, &request_hash).await?
    {
        tx.commit().await?;
        return Ok(MutationResult {
            status: stored.status,
            body: stored.body,
        });
    }
    let task = lock_task(&mut tx, principal, task_id).await?;
    if expected_version.is_some_and(|version| version != task.version) {
        return Err(ApiError::VersionConflict);
    }

    let target_status = match (action, task.status.as_str()) {
        ("message", "running" | "waiting_input" | "paused" | "failed") => task.status.clone(),
        ("pause", "queued" | "running" | "waiting_input") => "paused".into(),
        ("cancel", "queued" | "running" | "waiting_input" | "paused") => "cancelled".into(),
        ("resume", "paused" | "failed") => "queued".into(),
        ("resume", "waiting_input") => "running".into(),
        _ => return Err(ApiError::StateConflict),
    };
    let now = Utc::now();
    let version = task.version + 1;
    sqlx::query("UPDATE tasks SET status=$1, version=$2, updated_at=$3 WHERE id=$4")
        .bind(&target_status)
        .bind(version)
        .bind(now)
        .bind(task_id)
        .execute(&mut *tx)
        .await?;

    let command_id = new_id("cmd");
    let command_type = match action {
        "message" => "append_message",
        "pause" => "pause_task",
        "cancel" => "cancel_task",
        "resume" => "resume_task",
        _ => unreachable!(),
    };
    sqlx::query(
        "INSERT INTO commands(id, organization_id, project_id, task_id, run_id, command_type, status, payload, expected_version, accepted_at)
         VALUES ($1,$2,$3,$4,$5,$6,'accepted',$7,$8,$9)")
        .bind(&command_id).bind(&principal.organization_id).bind(&principal.project_id).bind(task_id)
        .bind(&task.active_run_id).bind(command_type).bind(payload).bind(version).bind(now).execute(&mut *tx).await?;

    let sequence = next_task_sequence(&mut tx, task_id).await?;
    let event_type = match action {
        "message" => "task.message_appended",
        "pause" => "task.pause_requested",
        "cancel" => "task.cancel_requested",
        "resume" => "task.resume_requested",
        _ => unreachable!(),
    };
    sqlx::query(
        "INSERT INTO events(id, organization_id, project_id, task_id, run_id, event_type, task_sequence, occurred_at, data, schema_version)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,1)")
        .bind(new_id("evt")).bind(&principal.organization_id).bind(&principal.project_id).bind(task_id)
        .bind(&task.active_run_id).bind(event_type).bind(sequence).bind(now)
        .bind(json!({"status": target_status, "task_version": version, "command_id": command_id})).execute(&mut *tx).await?;

    let body = json!({"command_id": command_id, "status": "accepted", "accepted_at": now});
    idempotency::store(
        &mut tx,
        principal,
        &endpoint,
        key,
        &request_hash,
        202,
        &body,
    )
    .await?;
    tx.commit().await?;
    Ok(MutationResult { status: 202, body })
}

async fn lock_task(
    tx: &mut Transaction<'_, Postgres>,
    principal: &ProjectPrincipal,
    task_id: &str,
) -> Result<TaskRow, ApiError> {
    sqlx::query_as::<_, TaskRow>(
        "SELECT id, project_id, external_id, title, goal, status, active_run_id, metadata, version, created_by_type, created_by_id, created_at, updated_at
         FROM tasks WHERE organization_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE")
        .bind(&principal.organization_id).bind(&principal.project_id).bind(task_id)
        .fetch_optional(&mut **tx).await?.ok_or(ApiError::NotFound)
}

async fn next_task_sequence(
    tx: &mut Transaction<'_, Postgres>,
    task_id: &str,
) -> Result<i64, ApiError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(task_sequence), 0) + 1 FROM events WHERE task_id=$1",
    )
    .bind(task_id)
    .fetch_one(&mut **tx)
    .await?)
}

pub async fn list_task_events(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    task_id: &str,
    after: Option<&str>,
    limit: i64,
) -> Result<EventPage, ApiError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM tasks WHERE organization_id=$1 AND project_id=$2 AND id=$3)",
    )
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .bind(task_id)
    .fetch_one(pool)
    .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    let (after_sequence, after_cursor) = match after {
        None => (None, None),
        Some(value) if value.starts_with("evt_") => {
            let cursor = sqlx::query_scalar::<_, i64>("SELECT cursor FROM events WHERE organization_id=$1 AND project_id=$2 AND task_id=$3 AND id=$4")
                .bind(&principal.organization_id).bind(&principal.project_id).bind(task_id).bind(value).fetch_optional(pool).await?
                .ok_or_else(|| ApiError::Validation("invalid event cursor".into()))?;
            (None, Some(cursor))
        }
        Some(value) => (
            Some(value.parse::<i64>().map_err(|_| {
                ApiError::Validation("after must be a sequence or event cursor".into())
            })?),
            None,
        ),
    };
    let mut rows = sqlx::query_as::<_, EventView>(
        "SELECT id, cursor::text AS cursor, organization_id, project_id, task_id, run_id, event_type, task_sequence, occurred_at, recorded_at, data, schema_version
         FROM events WHERE organization_id=$1 AND project_id=$2 AND task_id=$3
           AND ($4::bigint IS NULL OR task_sequence > $4) AND ($5::bigint IS NULL OR cursor > $5)
         ORDER BY task_sequence ASC LIMIT $6")
        .bind(&principal.organization_id).bind(&principal.project_id).bind(task_id).bind(after_sequence).bind(after_cursor).bind(limit + 1)
        .fetch_all(pool).await?;
    let has_more = rows.len() as i64 > limit;
    if has_more {
        rows.pop();
    }
    let next_cursor = has_more
        .then(|| rows.last().map(|event| event.id.clone()))
        .flatten();
    Ok(EventPage {
        data: rows,
        next_cursor,
        has_more,
    })
}

pub async fn resolve_event_cursor(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    after: Option<&str>,
) -> Result<i64, ApiError> {
    match after {
        None => Ok(0),
        Some(value) if value.starts_with("evt_") => sqlx::query_scalar::<_, i64>(
            "SELECT cursor FROM events WHERE organization_id=$1 AND project_id=$2 AND id=$3",
        )
        .bind(&principal.organization_id)
        .bind(&principal.project_id)
        .bind(value)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::Validation("invalid event cursor".into())),
        Some(value) => value
            .parse::<i64>()
            .map_err(|_| ApiError::Validation("invalid event cursor".into())),
    }
}

pub async fn list_project_events(
    pool: &PgPool,
    principal: &ProjectPrincipal,
    after_cursor: i64,
    task_id: Option<&str>,
    event_types: Option<&[String]>,
    limit: i64,
) -> Result<Vec<EventView>, ApiError> {
    Ok(sqlx::query_as::<_, EventView>(
        "SELECT id, cursor::text AS cursor, organization_id, project_id, task_id, run_id, event_type, task_sequence, occurred_at, recorded_at, data, schema_version
         FROM events WHERE organization_id=$1 AND project_id=$2 AND cursor>$3
           AND ($4::text IS NULL OR task_id=$4) AND ($5::text[] IS NULL OR event_type=ANY($5))
         ORDER BY cursor ASC LIMIT $6",
    )
    .bind(&principal.organization_id)
    .bind(&principal.project_id)
    .bind(after_cursor)
    .bind(task_id)
    .bind(event_types)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::now_v7().simple())
}
