use super::{
    CreateTaskCommand, CreateTaskOutcome, RespondDecisionCommand, RespondDecisionOutcome,
    StoreError, TaskStore,
};
use crate::domain::{
    Attempt, Decision, IngestEventsOutcome, RequestScope, RunnerEventInput, Task, TaskDetail,
    TaskEvent, TaskStatus,
};
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

#[derive(sqlx::FromRow)]
struct AttemptRow {
    id: Uuid,
    task_id: Uuid,
    runner_id: Option<Uuid>,
    workspace_id: Option<Uuid>,
    agent_source: String,
    agent_session_id: Option<String>,
    ordinal: i32,
    reason: String,
    status: String,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
}

impl From<AttemptRow> for Attempt {
    fn from(row: AttemptRow) -> Self {
        Self {
            id: row.id,
            task_id: row.task_id,
            runner_id: row.runner_id,
            workspace_id: row.workspace_id,
            agent_source: row.agent_source,
            agent_session_id: row.agent_session_id,
            ordinal: row.ordinal,
            reason: row.reason,
            status: row.status,
            started_at: row.started_at,
            ended_at: row.ended_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct DecisionRow {
    id: Uuid,
    task_id: Uuid,
    attempt_id: Uuid,
    kind: String,
    blocking: bool,
    schema_version: String,
    presentation: Value,
    status: String,
    response: Option<Value>,
    created_at: DateTime<Utc>,
    resolved_at: Option<DateTime<Utc>>,
}

impl From<DecisionRow> for Decision {
    fn from(row: DecisionRow) -> Self {
        Self {
            id: row.id,
            task_id: row.task_id,
            attempt_id: row.attempt_id,
            kind: row.kind,
            blocking: row.blocking,
            schema_version: row.schema_version,
            presentation: row.presentation,
            status: row.status,
            response: row.response,
            created_at: row.created_at,
            resolved_at: row.resolved_at,
        }
    }
}

const TASK_COLUMNS: &str = "id, organization_id, project_id, external_id, title, prompt, status, current_attempt_id, waiting_decision_count, event_cursor, version, created_at, updated_at";
const DECISION_COLUMNS: &str = "id, task_id, attempt_id, kind, blocking, schema_version, presentation, status, response, created_at, resolved_at";

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

    async fn list_tasks(
        &self,
        scope: RequestScope,
        status: Option<TaskStatus>,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Task>, StoreError> {
        let query = format!(
            "SELECT {TASK_COLUMNS} FROM tasks
             WHERE organization_id = $1 AND project_id = $2
               AND ($3::text IS NULL OR status = $3)
             ORDER BY updated_at DESC, id DESC
             OFFSET $4 LIMIT $5"
        );
        let status = status.map(TaskStatus::as_str);
        let rows = sqlx::query_as::<_, TaskRow>(&query)
            .bind(scope.organization_id)
            .bind(scope.project_id)
            .bind(status)
            .bind(offset as i64)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    async fn get_task_detail(
        &self,
        scope: RequestScope,
        task_id: Uuid,
    ) -> Result<TaskDetail, StoreError> {
        let task = self.get_task(scope, task_id).await?;
        let attempt_rows = sqlx::query_as::<_, AttemptRow>(
            "SELECT id, task_id, runner_id, workspace_id, agent_source, agent_session_id,
                    ordinal, reason, status, started_at, ended_at
             FROM attempts
             WHERE organization_id = $1 AND project_id = $2 AND task_id = $3
             ORDER BY ordinal ASC",
        )
        .bind(scope.organization_id)
        .bind(scope.project_id)
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?;
        let decision_query = format!(
            "SELECT {DECISION_COLUMNS} FROM decisions
             WHERE organization_id = $1 AND project_id = $2 AND task_id = $3
             ORDER BY created_at ASC, id ASC"
        );
        let decision_rows = sqlx::query_as::<_, DecisionRow>(&decision_query)
            .bind(scope.organization_id)
            .bind(scope.project_id)
            .bind(task_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(TaskDetail {
            task,
            attempts: attempt_rows.into_iter().map(Into::into).collect(),
            decisions: decision_rows.into_iter().map(Into::into).collect(),
        })
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

    async fn get_decision(
        &self,
        scope: RequestScope,
        decision_id: Uuid,
    ) -> Result<Decision, StoreError> {
        let query = format!(
            "SELECT d.{columns} FROM decisions d
             JOIN tasks t ON t.id = d.task_id
             WHERE d.id = $1 AND d.organization_id = $2 AND d.project_id = $3
               AND t.organization_id = $2 AND t.project_id = $3",
            columns = DECISION_COLUMNS.replace(", ", ", d.")
        );
        let row = sqlx::query_as::<_, DecisionRow>(&query)
            .bind(decision_id)
            .bind(scope.organization_id)
            .bind(scope.project_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StoreError::NotFound)?;
        Ok(row.into())
    }

    async fn respond_decision(
        &self,
        command: RespondDecisionCommand,
    ) -> Result<RespondDecisionOutcome, StoreError> {
        let mut tx = self.pool.begin().await?;
        let operation = format!("respond_decision:{}", command.decision_id);
        let inserted = sqlx::query(
            "INSERT INTO idempotency_keys
             (project_id, operation, idempotency_key, request_fingerprint, expires_at)
             VALUES ($1, $2, $3, $4, now() + interval '24 hours')
             ON CONFLICT (project_id, operation, idempotency_key) DO NOTHING",
        )
        .bind(command.scope.project_id)
        .bind(&operation)
        .bind(&command.idempotency_key)
        .bind(command.request_fingerprint.as_slice())
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;

        if !inserted {
            let existing: (Vec<u8>, Option<Uuid>) = sqlx::query_as(
                "SELECT request_fingerprint, resource_id FROM idempotency_keys
                 WHERE project_id = $1 AND operation = $2 AND idempotency_key = $3",
            )
            .bind(command.scope.project_id)
            .bind(&operation)
            .bind(&command.idempotency_key)
            .fetch_one(&mut *tx)
            .await?;
            if existing.0.as_slice() != command.request_fingerprint {
                return Err(StoreError::Conflict {
                    code: "idempotency_key_reused",
                    detail: "idempotency key was reused with a different request".into(),
                });
            }
            let resource_id = existing
                .1
                .ok_or_else(|| StoreError::Internal("idempotency resource missing".into()))?;
            let query = format!(
                "SELECT {DECISION_COLUMNS} FROM decisions
                 WHERE id = $1 AND organization_id = $2 AND project_id = $3"
            );
            let row = sqlx::query_as::<_, DecisionRow>(&query)
                .bind(resource_id)
                .bind(command.scope.organization_id)
                .bind(command.scope.project_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(StoreError::NotFound)?;
            tx.commit().await?;
            return Ok(RespondDecisionOutcome {
                decision: row.into(),
                replayed: true,
            });
        }

        let now = Utc::now();
        let status = if command.action == "decline" {
            "declined"
        } else {
            "answered"
        };
        let response = command.answers.map(|answers| json!({ "answers": answers }));
        let update_query = format!(
            "UPDATE decisions SET status = $1, response = $2,
                    responded_by = $3, resolved_at = $4
             WHERE id = $5 AND organization_id = $6 AND project_id = $7 AND status = 'open'
             RETURNING {DECISION_COLUMNS}"
        );
        let row = sqlx::query_as::<_, DecisionRow>(&update_query)
            .bind(status)
            .bind(&response)
            .bind(json!({ "type": "api_user", "id": "authenticated-user" }))
            .bind(now)
            .bind(command.decision_id)
            .bind(command.scope.organization_id)
            .bind(command.scope.project_id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            let exists: Option<String> = sqlx::query_scalar(
                "SELECT d.status FROM decisions d JOIN tasks t ON t.id = d.task_id
                 WHERE d.id = $1 AND d.organization_id = $2 AND d.project_id = $3
                   AND t.organization_id = $2 AND t.project_id = $3",
            )
            .bind(command.decision_id)
            .bind(command.scope.organization_id)
            .bind(command.scope.project_id)
            .fetch_optional(&mut *tx)
            .await?;
            return match exists {
                Some(_) => Err(StoreError::Conflict {
                    code: "decision_already_resolved",
                    detail: "decision is no longer open".into(),
                }),
                None => Err(StoreError::NotFound),
            };
        };
        let decision: Decision = row.into();
        let sequence: i64 = sqlx::query_scalar(
            "UPDATE tasks SET
                waiting_decision_count = GREATEST(waiting_decision_count - 1, 0),
                status = CASE
                    WHEN waiting_decision_count <= 1 AND status = 'waiting_for_input' THEN 'running'
                    ELSE status
                END,
                event_cursor = event_cursor + 1,
                version = version + 1,
                updated_at = $1
             WHERE id = $2 AND organization_id = $3 AND project_id = $4
             RETURNING event_cursor",
        )
        .bind(now)
        .bind(decision.task_id)
        .bind(command.scope.organization_id)
        .bind(command.scope.project_id)
        .fetch_one(&mut *tx)
        .await?;
        let event_id = Uuid::new_v4();
        let event_data = json!({ "decision": &decision });
        sqlx::query(
            "INSERT INTO task_events
             (id, organization_id, project_id, task_id, attempt_id, sequence, event_type,
              occurred_at, recorded_at, producer, schema_version, data)
             VALUES ($1, $2, $3, $4, $5, $6, 'decision.resolved', $7, $7, $8, '1.0', $9)",
        )
        .bind(event_id)
        .bind(command.scope.organization_id)
        .bind(command.scope.project_id)
        .bind(decision.task_id)
        .bind(decision.attempt_id)
        .bind(sequence)
        .bind(now)
        .bind(json!({ "type": "user", "id": "authenticated-user" }))
        .bind(&event_data)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO outbox
             (id, organization_id, project_id, topic, aggregate_id, payload)
             VALUES ($1, $2, $3, 'task.event', $4, $5)",
        )
        .bind(Uuid::new_v4())
        .bind(command.scope.organization_id)
        .bind(command.scope.project_id)
        .bind(decision.task_id)
        .bind(json!({
            "event_id": event_id,
            "task_id": decision.task_id,
            "sequence": sequence,
            "type": "decision.resolved"
        }))
        .execute(&mut *tx)
        .await?;
        let runner_command_id = Uuid::new_v4();
        let native_payload = native_decision_payload(&decision, &command.action);
        sqlx::query(
            "INSERT INTO outbox
             (id, organization_id, project_id, topic, aggregate_id, payload)
             VALUES ($1, $2, $3, 'runner.command', $4, $5)",
        )
        .bind(runner_command_id)
        .bind(command.scope.organization_id)
        .bind(command.scope.project_id)
        .bind(decision.attempt_id)
        .bind(json!({
            "type": "decision.response",
            "commandId": runner_command_id,
            "taskId": decision.task_id,
            "attemptId": decision.attempt_id,
            "decisionId": decision.id,
            "nativePayload": native_payload
        }))
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE idempotency_keys SET resource_id = $1, response = $2
             WHERE project_id = $3 AND operation = $4 AND idempotency_key = $5",
        )
        .bind(decision.id)
        .bind(
            serde_json::to_value(&decision)
                .map_err(|error| StoreError::Internal(error.to_string()))?,
        )
        .bind(command.scope.project_id)
        .bind(&operation)
        .bind(&command.idempotency_key)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(RespondDecisionOutcome {
            decision,
            replayed: false,
        })
    }

    async fn record_audit_denial(
        &self,
        scope: RequestScope,
        resource_type: &str,
        resource_id: Uuid,
        reason: &str,
    ) -> Result<(), StoreError> {
        sqlx::query(
            "INSERT INTO audit_denials
             (id, requested_organization_id, requested_project_id, resource_type, resource_id, reason)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::new_v4())
        .bind(scope.organization_id)
        .bind(scope.project_id)
        .bind(resource_type)
        .bind(resource_id)
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn ingest_runner_events(
        &self,
        scope: RequestScope,
        runner_id: Uuid,
        mut events: Vec<RunnerEventInput>,
    ) -> Result<IngestEventsOutcome, StoreError> {
        events.sort_by_key(|event| event.local_sequence);
        let accepted_through_local_sequence = events
            .last()
            .map(|event| event.local_sequence)
            .unwrap_or_default();
        let mut tx = self.pool.begin().await?;
        let mut accepted = 0;
        let mut duplicates = 0;
        for event in events {
            let duplicate: Option<Uuid> = sqlx::query_scalar(
                "SELECT id FROM task_events WHERE runner_id = $1 AND dedupe_key = $2",
            )
            .bind(runner_id)
            .bind(&event.dedupe_key)
            .fetch_optional(&mut *tx)
            .await?;
            if duplicate.is_some() {
                duplicates += 1;
                continue;
            }
            let current_cursor: Option<i64> = sqlx::query_scalar(
                "SELECT event_cursor FROM tasks
                 WHERE id = $1 AND organization_id = $2 AND project_id = $3
                 FOR UPDATE",
            )
            .bind(event.task_id)
            .bind(scope.organization_id)
            .bind(scope.project_id)
            .fetch_optional(&mut *tx)
            .await?;
            let sequence = current_cursor.ok_or(StoreError::NotFound)? + 1;

            if event.event_type == "attempt.started" {
                let attempt_id = event.attempt_id.ok_or_else(|| {
                    StoreError::Internal("attempt.started missing attempt_id".into())
                })?;
                let workspace_id = event
                    .data
                    .get("workspaceId")
                    .and_then(Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok());
                sqlx::query(
                    "INSERT INTO attempts
                     (id, organization_id, project_id, task_id, runner_id, workspace_id,
                      agent_source, agent_session_id, ordinal, reason, status, started_at)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'running', $11)
                     ON CONFLICT (id) DO NOTHING",
                )
                .bind(attempt_id)
                .bind(scope.organization_id)
                .bind(scope.project_id)
                .bind(event.task_id)
                .bind(runner_id)
                .bind(workspace_id)
                .bind(
                    event
                        .data
                        .get("agentSource")
                        .and_then(Value::as_str)
                        .unwrap_or("codex"),
                )
                .bind(event.data.get("agentSessionId").and_then(Value::as_str))
                .bind(
                    event
                        .data
                        .get("ordinal")
                        .and_then(Value::as_i64)
                        .unwrap_or(1) as i32,
                )
                .bind(
                    event
                        .data
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("initial"),
                )
                .bind(event.occurred_at)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE tasks SET current_attempt_id = $1, status = 'running'
                     WHERE id = $2",
                )
                .bind(attempt_id)
                .bind(event.task_id)
                .execute(&mut *tx)
                .await?;
            } else if event.event_type == "attempt.ended" {
                if let Some(attempt_id) = event.attempt_id {
                    sqlx::query(
                        "UPDATE attempts SET status = 'ended', ended_at = $1
                         WHERE id = $2 AND organization_id = $3 AND project_id = $4",
                    )
                    .bind(event.occurred_at)
                    .bind(attempt_id)
                    .bind(scope.organization_id)
                    .bind(scope.project_id)
                    .execute(&mut *tx)
                    .await?;
                }
            } else if event.event_type == "decision.opened" {
                let decision: Decision =
                    serde_json::from_value(event.data.get("decision").cloned().ok_or_else(
                        || StoreError::Internal("decision.opened missing decision".into()),
                    )?)
                    .map_err(|error| StoreError::Internal(error.to_string()))?;
                sqlx::query(
                    "INSERT INTO decisions
                     (id, organization_id, project_id, task_id, attempt_id, kind, blocking,
                      schema_version, presentation, status, response, created_at, resolved_at)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                     ON CONFLICT (id) DO NOTHING",
                )
                .bind(decision.id)
                .bind(scope.organization_id)
                .bind(scope.project_id)
                .bind(decision.task_id)
                .bind(decision.attempt_id)
                .bind(&decision.kind)
                .bind(decision.blocking)
                .bind(&decision.schema_version)
                .bind(&decision.presentation)
                .bind(&decision.status)
                .bind(&decision.response)
                .bind(decision.created_at)
                .bind(decision.resolved_at)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE tasks SET waiting_decision_count = waiting_decision_count + 1,
                            status = CASE WHEN $1 THEN 'waiting_for_input' ELSE status END
                     WHERE id = $2",
                )
                .bind(decision.blocking)
                .bind(event.task_id)
                .execute(&mut *tx)
                .await?;
            } else if event.event_type == "task.status_changed" {
                let status = event
                    .data
                    .get("status")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        StoreError::Internal("task.status_changed missing status".into())
                    })?;
                TaskStatus::from_str(status).map_err(StoreError::Internal)?;
                sqlx::query("UPDATE tasks SET status = $1 WHERE id = $2")
                    .bind(status)
                    .bind(event.task_id)
                    .execute(&mut *tx)
                    .await?;
            }

            let cloud_event_id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO task_events
                 (id, organization_id, project_id, task_id, attempt_id, sequence, event_type,
                  occurred_at, producer, runner_id, dedupe_key, schema_version, data)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
            )
            .bind(cloud_event_id)
            .bind(scope.organization_id)
            .bind(scope.project_id)
            .bind(event.task_id)
            .bind(event.attempt_id)
            .bind(sequence)
            .bind(&event.event_type)
            .bind(event.occurred_at)
            .bind(json!({ "type": "runner", "id": runner_id }))
            .bind(runner_id)
            .bind(&event.dedupe_key)
            .bind(&event.schema_version)
            .bind(&event.data)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE tasks SET event_cursor = $1, version = version + 1, updated_at = $2
                 WHERE id = $3",
            )
            .bind(sequence)
            .bind(event.occurred_at)
            .bind(event.task_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO outbox
                 (id, organization_id, project_id, topic, aggregate_id, payload)
                 VALUES ($1, $2, $3, 'task.event', $4, $5)",
            )
            .bind(Uuid::new_v4())
            .bind(scope.organization_id)
            .bind(scope.project_id)
            .bind(event.task_id)
            .bind(json!({
                "event_id": cloud_event_id,
                "task_id": event.task_id,
                "sequence": sequence,
                "type": event.event_type
            }))
            .execute(&mut *tx)
            .await?;
            accepted += 1;
        }
        tx.commit().await?;
        Ok(IngestEventsOutcome {
            accepted,
            duplicates,
            accepted_through_local_sequence,
        })
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

fn native_decision_payload(decision: &Decision, action: &str) -> Value {
    let kind = match decision.kind.as_str() {
        "fleet_ask" => "fleet-ask",
        "plan_approval" => "plan-approval",
        "permission_prompt" => "permission-prompt",
        "a2ui" => "a2ui-render",
        other => other,
    };
    let id = decision
        .presentation
        .get("sourceDecisionId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| decision.id.to_string());
    let answers = decision
        .response
        .as_ref()
        .and_then(|response| response.get("answers"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    match kind {
        "guard" => json!({ "kind": kind, "id": id, "allow": action == "answer" }),
        "elicitation" => json!({
            "kind": kind, "id": id, "declined": action == "decline", "answers": answers
        }),
        "fleet-ask" => json!({
            "kind": kind, "id": id, "cancelled": action == "decline", "answers": answers
        }),
        "plan-approval" => json!({
            "kind": kind,
            "id": id,
            "decision": if action == "answer" { "approve" } else { "reject" }
        }),
        "permission-prompt" => json!({
            "kind": kind, "id": id, "allow": action == "answer"
        }),
        "a2ui-render" => json!({
            "kind": kind,
            "id": id,
            "cancelled": action == "decline",
            "actionContext": answers
        }),
        _ => json!({ "kind": kind, "id": id, "answers": answers }),
    }
}
