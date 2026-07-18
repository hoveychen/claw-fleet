use super::{
    CreateTaskCommand, CreateTaskOutcome, RespondDecisionCommand, RespondDecisionOutcome,
    StoreError, TaskStore,
};
use crate::domain::{
    Attempt, Decision, EventProducer, RequestScope, Task, TaskDetail, TaskEvent, TaskStatus,
};
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
    attempts: HashMap<Uuid, Vec<Attempt>>,
    decisions: HashMap<Uuid, Decision>,
    idempotency: HashMap<(Uuid, String, String), ([u8; 32], Uuid)>,
}

impl InMemoryTaskStore {
    fn lock(&self) -> Result<MutexGuard<'_, State>, StoreError> {
        self.state
            .lock()
            .map_err(|_| StoreError::Internal("in-memory store lock poisoned".into()))
    }

    pub fn seed_open_decision(
        &self,
        scope: RequestScope,
        task_id: Uuid,
        kind: &str,
        presentation: serde_json::Value,
    ) -> Result<Decision, StoreError> {
        let mut state = self.lock()?;
        let now = Utc::now();
        let attempt = Attempt {
            id: Uuid::new_v4(),
            task_id,
            runner_id: None,
            workspace_id: None,
            agent_source: "codex".into(),
            agent_session_id: Some("seed-session".into()),
            ordinal: 1,
            reason: "initial".into(),
            status: "waiting".into(),
            started_at: now,
            ended_at: None,
        };
        let decision = Decision {
            id: Uuid::new_v4(),
            task_id,
            attempt_id: attempt.id,
            kind: kind.into(),
            blocking: true,
            schema_version: "1.0".into(),
            presentation,
            status: "open".into(),
            response: None,
            created_at: now,
            resolved_at: None,
        };
        let task = state
            .tasks
            .get_mut(&(scope.project_id, task_id))
            .filter(|task| task.organization_id == scope.organization_id)
            .ok_or(StoreError::NotFound)?;
        task.current_attempt_id = Some(attempt.id);
        task.status = TaskStatus::WaitingForInput;
        task.waiting_decision_count += 1;
        task.event_cursor += 2;
        task.updated_at = now;
        let attempt_sequence = task.event_cursor - 1;
        let decision_sequence = task.event_cursor;
        let attempt_event = TaskEvent {
            id: Uuid::new_v4(),
            organization_id: scope.organization_id,
            project_id: scope.project_id,
            task_id,
            attempt_id: Some(attempt.id),
            sequence: attempt_sequence,
            event_type: "attempt.started".into(),
            occurred_at: now,
            recorded_at: now,
            producer: EventProducer {
                producer_type: "runner".into(),
                id: "seed-runner".into(),
            },
            schema_version: "1.0".into(),
            data: json!({ "attempt": &attempt }),
        };
        let decision_event = TaskEvent {
            id: Uuid::new_v4(),
            organization_id: scope.organization_id,
            project_id: scope.project_id,
            task_id,
            attempt_id: Some(attempt.id),
            sequence: decision_sequence,
            event_type: "decision.opened".into(),
            occurred_at: now,
            recorded_at: now,
            producer: EventProducer {
                producer_type: "runner".into(),
                id: "seed-runner".into(),
            },
            schema_version: "1.0".into(),
            data: json!({ "decision": &decision }),
        };
        state.attempts.entry(task_id).or_default().push(attempt);
        state.decisions.insert(decision.id, decision.clone());
        state
            .events
            .entry(task_id)
            .or_default()
            .extend([attempt_event, decision_event]);
        Ok(decision)
    }
}

#[async_trait]
impl TaskStore for InMemoryTaskStore {
    async fn create_task(
        &self,
        command: CreateTaskCommand,
    ) -> Result<CreateTaskOutcome, StoreError> {
        let mut state = self.lock()?;
        let idem_key = (
            command.scope.project_id,
            "create_task".into(),
            command.idempotency_key.clone(),
        );
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

    async fn list_tasks(
        &self,
        scope: RequestScope,
        status: Option<TaskStatus>,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Task>, StoreError> {
        let mut tasks: Vec<_> = self
            .lock()?
            .tasks
            .values()
            .filter(|task| {
                task.organization_id == scope.organization_id
                    && task.project_id == scope.project_id
                    && status.is_none_or(|expected| task.status == expected)
            })
            .cloned()
            .collect();
        tasks.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(tasks.into_iter().skip(offset).take(limit).collect())
    }

    async fn get_task_detail(
        &self,
        scope: RequestScope,
        task_id: Uuid,
    ) -> Result<TaskDetail, StoreError> {
        let task = self.get_task(scope, task_id).await?;
        let state = self.lock()?;
        Ok(TaskDetail {
            task,
            attempts: state.attempts.get(&task_id).cloned().unwrap_or_default(),
            decisions: state
                .decisions
                .values()
                .filter(|decision| decision.task_id == task_id)
                .cloned()
                .collect(),
        })
    }

    async fn get_decision(
        &self,
        scope: RequestScope,
        decision_id: Uuid,
    ) -> Result<Decision, StoreError> {
        let state = self.lock()?;
        let decision = state
            .decisions
            .get(&decision_id)
            .cloned()
            .ok_or(StoreError::NotFound)?;
        let is_scoped = state
            .tasks
            .get(&(scope.project_id, decision.task_id))
            .is_some_and(|task| task.organization_id == scope.organization_id);
        if !is_scoped {
            return Err(StoreError::NotFound);
        }
        Ok(decision)
    }

    async fn respond_decision(
        &self,
        command: RespondDecisionCommand,
    ) -> Result<RespondDecisionOutcome, StoreError> {
        let mut state = self.lock()?;
        let operation = format!("respond_decision:{}", command.decision_id);
        let idem_key = (
            command.scope.project_id,
            operation,
            command.idempotency_key.clone(),
        );
        if let Some((fingerprint, decision_id)) = state.idempotency.get(&idem_key).cloned() {
            if fingerprint != command.request_fingerprint {
                return Err(StoreError::Conflict {
                    code: "idempotency_key_reused",
                    detail: "idempotency key was reused with a different request".into(),
                });
            }
            let decision = state
                .decisions
                .get(&decision_id)
                .cloned()
                .ok_or(StoreError::NotFound)?;
            return Ok(RespondDecisionOutcome {
                decision,
                replayed: true,
            });
        }

        let original = state
            .decisions
            .get(&command.decision_id)
            .cloned()
            .ok_or(StoreError::NotFound)?;
        let scoped = state
            .tasks
            .get(&(command.scope.project_id, original.task_id))
            .is_some_and(|task| task.organization_id == command.scope.organization_id);
        if !scoped {
            return Err(StoreError::NotFound);
        }
        if original.status != "open" {
            return Err(StoreError::Conflict {
                code: "decision_already_resolved",
                detail: "decision is no longer open".into(),
            });
        }

        let now = Utc::now();
        let mut decision = original;
        decision.status = if command.action == "decline" {
            "declined"
        } else {
            "answered"
        }
        .into();
        decision.response = command.answers.map(|answers| json!({ "answers": answers }));
        decision.resolved_at = Some(now);
        state.decisions.insert(decision.id, decision.clone());

        let task = state
            .tasks
            .get_mut(&(command.scope.project_id, decision.task_id))
            .expect("scoped task disappeared");
        task.waiting_decision_count = (task.waiting_decision_count - 1).max(0);
        if task.waiting_decision_count == 0 && task.status == TaskStatus::WaitingForInput {
            task.status = TaskStatus::Running;
        }
        task.event_cursor += 1;
        task.updated_at = now;
        let sequence = task.event_cursor;

        state
            .events
            .entry(decision.task_id)
            .or_default()
            .push(TaskEvent {
                id: Uuid::new_v4(),
                organization_id: command.scope.organization_id,
                project_id: command.scope.project_id,
                task_id: decision.task_id,
                attempt_id: Some(decision.attempt_id),
                sequence,
                event_type: "decision.resolved".into(),
                occurred_at: now,
                recorded_at: now,
                producer: EventProducer {
                    producer_type: "user".into(),
                    id: "api-user".into(),
                },
                schema_version: "1.0".into(),
                data: json!({ "decision": &decision }),
            });
        state
            .idempotency
            .insert(idem_key, (command.request_fingerprint, decision.id));
        Ok(RespondDecisionOutcome {
            decision,
            replayed: false,
        })
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
