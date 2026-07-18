use super::{CreateTaskCommand, CreateTaskOutcome, StoreError, TaskStore};
use crate::domain::{RequestScope, Task, TaskEvent, TaskStatus};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::str::FromStr;
use uuid::Uuid;

pub struct PgTaskStore {
    pool: PgPool,
}

impl PgTaskStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn connect(database_url: &str) -> Result<Self, StoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await?;
        Ok(Self::new(pool))
    }

    pub async fn migrate(&self) -> Result<(), StoreError> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|error| StoreError::Internal(error.to_string()))
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[derive(sqlx::FromRow)]
struct TaskRow {
    id: Uuid,
    organization_id: Uuid,
    project_id: Uuid,
    external_id: Option<String>,
    title: Option<String>,
    prompt: String,
    status: String,
    current_attempt_id: Option<Uuid>,
    waiting_decision_count: i32,
    event_cursor: i64,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<TaskRow> for Task {
    type Error = StoreError;

    fn try_from(row: TaskRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            organization_id: row.organization_id,
            project_id: row.project_id,
            external_id: row.external_id,
            title: row.title,
            prompt: row.prompt,
            status: TaskStatus::from_str(&row.status).map_err(StoreError::Internal)?,
            current_attempt_id: row.current_attempt_id,
            waiting_decision_count: row.waiting_decision_count,
            event_cursor: row.event_cursor,
            version: row.version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct EventRow {
    id: Uuid,
    organization_id: Uuid,
    project_id: Uuid,
    task_id: Uuid,
    attempt_id: Option<Uuid>,
    sequence: i64,
    event_type: String,
    occurred_at: DateTime<Utc>,
    recorded_at: DateTime<Utc>,
    producer: Value,
    schema_version: String,
    data: Value,
}

impl TryFrom<EventRow> for TaskEvent {
    type Error = StoreError;

    fn try_from(row: EventRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            organization_id: row.organization_id,
            project_id: row.project_id,
            task_id: row.task_id,
            attempt_id: row.attempt_id,
            sequence: row.sequence,
            event_type: row.event_type,
            occurred_at: row.occurred_at,
            recorded_at: row.recorded_at,
            producer: serde_json::from_value(row.producer)
                .map_err(|error| StoreError::Internal(error.to_string()))?,
            schema_version: row.schema_version,
            data: row.data,
        })
    }
}

const TASK_COLUMNS: &str = "id, organization_id, project_id, external_id, title, prompt, status, current_attempt_id, waiting_decision_count, event_cursor, version, created_at, updated_at";

#[async_trait]
impl TaskStore for PgTaskStore {
    async fn create_task(
        &self,
        command: CreateTaskCommand,
    ) -> Result<CreateTaskOutcome, StoreError> {
        let mut tx = self.pool.begin().await?;
        let inserted = sqlx::query(
            "INSERT INTO idempotency_keys
             (project_id, operation, idempotency_key, request_fingerprint, expires_at)
             VALUES ($1, 'create_task', $2, $3, now() + interval '24 hours')
             ON CONFLICT (project_id, operation, idempotency_key) DO NOTHING",
        )
        .bind(command.scope.project_id)
        .bind(&command.idempotency_key)
        .bind(command.request_fingerprint.as_slice())
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;

        if !inserted {
            let existing: (Vec<u8>, Option<Uuid>) = sqlx::query_as(
                "SELECT request_fingerprint, resource_id
                 FROM idempotency_keys
                 WHERE project_id = $1 AND operation = 'create_task' AND idempotency_key = $2",
            )
            .bind(command.scope.project_id)
            .bind(&command.idempotency_key)
            .fetch_one(&mut *tx)
            .await?;
            if existing.0.as_slice() != command.request_fingerprint {
                return Err(StoreError::Conflict {
                    code: "idempotency_key_reused",
                    detail: "idempotency key was reused with a different request".into(),
                });
            }
            let task_id = existing
                .1
                .ok_or_else(|| StoreError::Internal("idempotency resource missing".into()))?;
            let query = format!(
                "SELECT {TASK_COLUMNS} FROM tasks
                 WHERE organization_id = $1 AND project_id = $2 AND id = $3"
            );
            let row = sqlx::query_as::<_, TaskRow>(&query)
                .bind(command.scope.organization_id)
                .bind(command.scope.project_id)
                .bind(task_id)
                .fetch_one(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(CreateTaskOutcome {
                task: row.try_into()?,
                replayed: true,
            });
        }

        let now = Utc::now();
        let task_id = Uuid::new_v4();
        let event_id = Uuid::new_v4();
        let outbox_id = Uuid::new_v4();
        let selector = serde_json::to_value(&command.request.workspace_selector)
            .map_err(|error| StoreError::Internal(error.to_string()))?;
        let profile = serde_json::to_value(&command.request.agent_profile)
            .map_err(|error| StoreError::Internal(error.to_string()))?;
        let metadata = serde_json::to_value(&command.request.metadata)
            .map_err(|error| StoreError::Internal(error.to_string()))?;

        let query = format!(
            "INSERT INTO tasks
             (id, organization_id, project_id, external_id, title, prompt, status,
              workspace_selector, agent_profile, metadata, event_cursor, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, 'queued', $7, $8, $9, 1, $10, $10)
             RETURNING {TASK_COLUMNS}"
        );
        let row = sqlx::query_as::<_, TaskRow>(&query)
            .bind(task_id)
            .bind(command.scope.organization_id)
            .bind(command.scope.project_id)
            .bind(&command.request.external_id)
            .bind(&command.request.title)
            .bind(&command.request.prompt)
            .bind(selector)
            .bind(profile)
            .bind(metadata)
            .bind(now)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_write_error)?;
        let task: Task = row.try_into()?;

        let producer = json!({ "type": "api_client", "id": "oauth-client" });
        let event_data = json!({ "status": "queued" });
        sqlx::query(
            "INSERT INTO task_events
             (id, organization_id, project_id, task_id, sequence, event_type,
              occurred_at, recorded_at, producer, schema_version, data)
             VALUES ($1, $2, $3, $4, 1, 'task.created', $5, $5, $6, '1.0', $7)",
        )
        .bind(event_id)
        .bind(command.scope.organization_id)
        .bind(command.scope.project_id)
        .bind(task_id)
        .bind(now)
        .bind(&producer)
        .bind(&event_data)
        .execute(&mut *tx)
        .await?;

        let outbox_payload = json!({
            "event_id": event_id,
            "task_id": task_id,
            "sequence": 1,
            "type": "task.created"
        });
        sqlx::query(
            "INSERT INTO outbox
             (id, organization_id, project_id, topic, aggregate_id, payload)
             VALUES ($1, $2, $3, 'task.event', $4, $5)",
        )
        .bind(outbox_id)
        .bind(command.scope.organization_id)
        .bind(command.scope.project_id)
        .bind(task_id)
        .bind(outbox_payload)
        .execute(&mut *tx)
        .await?;

        let response =
            serde_json::to_value(&task).map_err(|error| StoreError::Internal(error.to_string()))?;
        sqlx::query(
            "UPDATE idempotency_keys SET resource_id = $1, response = $2
             WHERE project_id = $3 AND operation = 'create_task' AND idempotency_key = $4",
        )
        .bind(task_id)
        .bind(response)
        .bind(command.scope.project_id)
        .bind(&command.idempotency_key)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(CreateTaskOutcome {
            task,
            replayed: false,
        })
    }

    async fn get_task(&self, scope: RequestScope, task_id: Uuid) -> Result<Task, StoreError> {
        let query = format!(
            "SELECT {TASK_COLUMNS} FROM tasks
             WHERE organization_id = $1 AND project_id = $2 AND id = $3"
        );
        let row = sqlx::query_as::<_, TaskRow>(&query)
            .bind(scope.organization_id)
            .bind(scope.project_id)
            .bind(task_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StoreError::NotFound)?;
        row.try_into()
    }

    async fn list_events(
        &self,
        scope: RequestScope,
        task_id: Uuid,
        after: i64,
    ) -> Result<Vec<TaskEvent>, StoreError> {
        let rows = sqlx::query_as::<_, EventRow>(
            "SELECT e.id, e.organization_id, e.project_id, e.task_id, e.attempt_id,
                    e.sequence, e.event_type, e.occurred_at, e.recorded_at,
                    e.producer, e.schema_version, e.data
             FROM task_events e
             JOIN tasks t ON t.id = e.task_id
             WHERE e.organization_id = $1 AND e.project_id = $2 AND e.task_id = $3
               AND t.organization_id = $1 AND t.project_id = $2
               AND e.sequence > $4
             ORDER BY e.sequence ASC",
        )
        .bind(scope.organization_id)
        .bind(scope.project_id)
        .bind(task_id)
        .bind(after.max(0))
        .fetch_all(&self.pool)
        .await?;
        if rows.is_empty() {
            self.get_task(scope, task_id).await?;
        }
        rows.into_iter().map(TryInto::try_into).collect()
    }
}

fn map_write_error(error: sqlx::Error) -> StoreError {
    if let sqlx::Error::Database(database) = &error {
        if database.constraint() == Some("tasks_project_external_id_unique") {
            return StoreError::Conflict {
                code: "external_id_exists",
                detail: "external_id already exists in this project".into(),
            };
        }
    }
    StoreError::Internal(error.to_string())
}
