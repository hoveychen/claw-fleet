use super::{CreateTaskCommand, CreateTaskOutcome, StoreError, TaskStore};
use crate::domain::{EventProducer, RequestScope, Task, TaskEvent, TaskStatus};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
};
use uuid::Uuid;

#[derive(Default)]
pub struct InMemoryTaskStore {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    tasks: HashMap<(Uuid, Uuid), Task>,
    events: HashMap<Uuid, Vec<TaskEvent>>,
    idempotency: HashMap<(Uuid, String), ([u8; 32], Uuid)>,
}

impl InMemoryTaskStore {
    fn lock(&self) -> Result<MutexGuard<'_, State>, StoreError> {
        self.state
            .lock()
            .map_err(|_| StoreError::Internal("in-memory store lock poisoned".into()))
    }
}

#[async_trait]
impl TaskStore for InMemoryTaskStore {
    async fn create_task(
        &self,
        command: CreateTaskCommand,
    ) -> Result<CreateTaskOutcome, StoreError> {
        let mut state = self.lock()?;
        let idem_key = (command.scope.project_id, command.idempotency_key.clone());
        if let Some((fingerprint, task_id)) = state.idempotency.get(&idem_key) {
            if fingerprint != &command.request_fingerprint {
                return Err(StoreError::Conflict {
                    code: "idempotency_key_reused",
                    detail: "idempotency key was reused with a different request".into(),
                });
            }
            let task = state
                .tasks
                .get(&(command.scope.project_id, *task_id))
                .cloned()
                .ok_or_else(|| StoreError::Internal("idempotency task missing".into()))?;
            return Ok(CreateTaskOutcome {
                task,
                replayed: true,
            });
        }

        let now = Utc::now();
        let task_id = Uuid::new_v4();
        let task = Task {
            id: task_id,
            organization_id: command.scope.organization_id,
            project_id: command.scope.project_id,
            external_id: command.request.external_id,
            title: command.request.title,
            prompt: command.request.prompt,
            status: TaskStatus::Queued,
            current_attempt_id: None,
            waiting_decision_count: 0,
            event_cursor: 1,
            version: 1,
            created_at: now,
            updated_at: now,
        };
        let event = TaskEvent {
            id: Uuid::new_v4(),
            organization_id: command.scope.organization_id,
            project_id: command.scope.project_id,
            task_id,
            attempt_id: None,
            sequence: 1,
            event_type: "task.created".into(),
            occurred_at: now,
            recorded_at: now,
            producer: EventProducer {
                producer_type: "api_client".into(),
                id: "test-client".into(),
            },
            schema_version: "1.0".into(),
            data: json!({ "status": "queued" }),
        };
        state
            .tasks
            .insert((command.scope.project_id, task_id), task.clone());
        state.events.insert(task_id, vec![event]);
        state
            .idempotency
            .insert(idem_key, (command.request_fingerprint, task_id));
        Ok(CreateTaskOutcome {
            task,
            replayed: false,
        })
    }

    async fn get_task(&self, scope: RequestScope, task_id: Uuid) -> Result<Task, StoreError> {
        self.lock()?
            .tasks
            .get(&(scope.project_id, task_id))
            .filter(|task| task.organization_id == scope.organization_id)
            .cloned()
            .ok_or(StoreError::NotFound)
    }

    async fn list_events(
        &self,
        scope: RequestScope,
        task_id: Uuid,
        after: i64,
    ) -> Result<Vec<TaskEvent>, StoreError> {
        self.get_task(scope, task_id).await?;
        Ok(self
            .lock()?
            .events
            .get(&task_id)
            .into_iter()
            .flatten()
            .filter(|event| event.sequence > after)
            .cloned()
            .collect())
    }
}
