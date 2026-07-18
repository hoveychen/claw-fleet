use fleet_cloud_api::{
    domain::{AgentProfile, AgentTool, CreateTaskRequest, RequestScope, WorkspaceSelector},
    store::{CreateTaskCommand, PgTaskStore, RespondDecisionCommand, StoreError, TaskStore},
    webhook::{PgWebhookDispatcher, WebhookTransport, WebhookTransportResponse},
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use uuid::Uuid;

#[derive(Default)]
struct RecordingWebhookTransport {
    requests: Mutex<Vec<(String, String, Vec<u8>)>>,
}

#[async_trait::async_trait]
impl WebhookTransport for RecordingWebhookTransport {
    async fn post(
        &self,
        url: &str,
        signature: &str,
        body: &[u8],
    ) -> Result<WebhookTransportResponse, String> {
        self.requests
            .lock()
            .unwrap()
            .push((url.to_string(), signature.to_string(), body.to_vec()));
        Ok(WebhookTransportResponse { status: 204 })
    }
}

fn request(prompt: &str, external_id: &str) -> CreateTaskRequest {
    CreateTaskRequest {
        external_id: Some(external_id.into()),
        title: Some("Postgres integration".into()),
        prompt: prompt.into(),
        workspace_selector: WorkspaceSelector {
            workspace_id: None,
            labels: BTreeMap::from([("repo".into(), "fleet".into())]),
        },
        agent_profile: AgentProfile {
            tool: AgentTool::Codex,
            model: None,
            effort: None,
            permission_policy: None,
            required_capabilities: Vec::new(),
        },
        metadata: BTreeMap::new(),
    }
}

fn command(
    scope: RequestScope,
    key: &str,
    fingerprint_byte: u8,
    prompt: &str,
    external_id: &str,
) -> CreateTaskCommand {
    CreateTaskCommand {
        scope,
        idempotency_key: key.into(),
        request_fingerprint: [fingerprint_byte; 32],
        request: request(prompt, external_id),
    }
}

#[tokio::test]
async fn postgres_create_task_is_atomic_and_idempotent() {
    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must point at the disposable integration database");
    let store = PgTaskStore::connect(&database_url).await.unwrap();
    store.migrate().await.unwrap();
    sqlx::query("TRUNCATE organizations CASCADE")
        .execute(store.pool())
        .await
        .unwrap();

    let scope = RequestScope {
        organization_id: Uuid::new_v4(),
        project_id: Uuid::new_v4(),
    };
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(scope.organization_id)
        .bind(format!("org-{}", scope.organization_id))
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO projects (id, organization_id, name) VALUES ($1, $2, $3)")
        .bind(scope.project_id)
        .bind(scope.organization_id)
        .bind(format!("project-{}", scope.project_id))
        .execute(store.pool())
        .await
        .unwrap();
    let webhook_endpoint_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO webhook_endpoints
         (id, organization_id, project_id, url, event_types, signing_secret)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(webhook_endpoint_id)
    .bind(scope.organization_id)
    .bind(scope.project_id)
    .bind("https://hooks.example.com/fleet")
    .bind(json!(["task.created"]))
    .bind("webhook-test-secret")
    .execute(store.pool())
    .await
    .unwrap();

    let first = store
        .create_task(command(scope, "idem-postgres-001", 7, "ship", "ext-1"))
        .await
        .unwrap();
    assert!(!first.replayed);
    assert_eq!(first.task.event_cursor, 1);
    for _ in 0..19 {
        let replay = store
            .create_task(command(scope, "idem-postgres-001", 7, "ship", "ext-1"))
            .await
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.task.id, first.task.id);
    }

    let conflict = store
        .create_task(command(scope, "idem-postgres-001", 8, "changed", "ext-2"))
        .await
        .unwrap_err();
    assert!(matches!(
        conflict,
        StoreError::Conflict {
            code: "idempotency_key_reused",
            ..
        }
    ));

    let duplicate_external = store
        .create_task(command(scope, "idem-postgres-002", 9, "second", "ext-1"))
        .await
        .unwrap_err();
    assert!(matches!(
        duplicate_external,
        StoreError::Conflict {
            code: "external_id_exists",
            ..
        }
    ));

    let task_count: i64 = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE project_id = $1")
        .bind(scope.project_id)
        .fetch_one(store.pool())
        .await
        .unwrap();
    let event_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM task_events WHERE task_id = $1")
            .bind(first.task.id)
            .fetch_one(store.pool())
            .await
            .unwrap();
    let outbox_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM outbox WHERE aggregate_id = $1")
            .bind(first.task.id)
            .fetch_one(store.pool())
            .await
            .unwrap();
    let rolled_back_key_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM idempotency_keys
         WHERE project_id = $1 AND idempotency_key = 'idem-postgres-002'",
    )
    .bind(scope.project_id)
    .fetch_one(store.pool())
    .await
    .unwrap();

    assert_eq!(task_count, 1);
    assert_eq!(event_count, 1);
    assert_eq!(outbox_count, 1);
    assert_eq!(rolled_back_key_count, 0);

    let events = store.list_events(scope, first.task.id, 0).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].sequence, 1);
    assert_eq!(events[0].event_type, "task.created");
    assert!(store
        .list_events(scope, first.task.id, 1)
        .await
        .unwrap()
        .is_empty());

    let listed = store
        .list_tasks(
            scope,
            Some(fleet_cloud_api::domain::TaskStatus::Queued),
            0,
            10,
        )
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, first.task.id);

    let attempt_id = Uuid::new_v4();
    let decision_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO attempts
         (id, organization_id, project_id, task_id, agent_source, ordinal, reason, status)
         VALUES ($1, $2, $3, $4, 'codex', 1, 'initial', 'waiting')",
    )
    .bind(attempt_id)
    .bind(scope.organization_id)
    .bind(scope.project_id)
    .bind(first.task.id)
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO decisions
         (id, organization_id, project_id, task_id, attempt_id, kind, blocking,
          schema_version, presentation, status)
         VALUES ($1, $2, $3, $4, $5, 'fleet_ask', true, '1.0', $6, 'open')",
    )
    .bind(decision_id)
    .bind(scope.organization_id)
    .bind(scope.project_id)
    .bind(first.task.id)
    .bind(attempt_id)
    .bind(json!({ "question": "Ship?" }))
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE tasks SET status = 'waiting_for_input', current_attempt_id = $1,
                waiting_decision_count = 1
         WHERE id = $2",
    )
    .bind(attempt_id)
    .bind(first.task.id)
    .execute(store.pool())
    .await
    .unwrap();

    let detail = store.get_task_detail(scope, first.task.id).await.unwrap();
    assert_eq!(detail.task.id, first.task.id);
    assert_eq!(detail.attempts.len(), 1);
    assert_eq!(detail.decisions.len(), 1);
    assert_eq!(detail.decisions[0].id, decision_id);
    assert_eq!(
        store.get_decision(scope, decision_id).await.unwrap().status,
        "open"
    );
    let foreign_scope = RequestScope {
        organization_id: Uuid::new_v4(),
        project_id: scope.project_id,
    };
    assert!(matches!(
        store.get_decision(foreign_scope, decision_id).await,
        Err(StoreError::NotFound)
    ));

    let response = RespondDecisionCommand {
        scope,
        decision_id,
        idempotency_key: "decision-pg-001".into(),
        request_fingerprint: [21; 32],
        action: "answer".into(),
        answers: Some(json!({ "choice": "ship" })),
    };
    let answered = store.respond_decision(response.clone()).await.unwrap();
    assert!(!answered.replayed);
    assert_eq!(answered.decision.status, "answered");
    let replay = store.respond_decision(response).await.unwrap();
    assert!(replay.replayed);
    let reused = store
        .respond_decision(RespondDecisionCommand {
            scope,
            decision_id,
            idempotency_key: "decision-pg-001".into(),
            request_fingerprint: [22; 32],
            action: "decline".into(),
            answers: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(
        reused,
        StoreError::Conflict {
            code: "idempotency_key_reused",
            ..
        }
    ));
    let resolved = store
        .respond_decision(RespondDecisionCommand {
            scope,
            decision_id,
            idempotency_key: "decision-pg-002".into(),
            request_fingerprint: [23; 32],
            action: "decline".into(),
            answers: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(
        resolved,
        StoreError::Conflict {
            code: "decision_already_resolved",
            ..
        }
    ));
    let task_after = store.get_task(scope, first.task.id).await.unwrap();
    assert_eq!(task_after.waiting_decision_count, 0);
    assert_eq!(
        task_after.status,
        fleet_cloud_api::domain::TaskStatus::Running
    );
    assert_eq!(task_after.event_cursor, 2);
    let events_after = store.list_events(scope, first.task.id, 1).await.unwrap();
    assert_eq!(events_after.len(), 1);
    assert_eq!(events_after[0].event_type, "decision.resolved");
    let outbox_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE aggregate_id = $1 AND topic = 'task.event'",
    )
    .bind(first.task.id)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(outbox_count, 2);

    let transport = Arc::new(RecordingWebhookTransport::default());
    let dispatcher = PgWebhookDispatcher::new(store.pool().clone(), transport.clone());
    let summary = dispatcher.dispatch_once(10).await.unwrap();
    assert_eq!(summary.claimed, 2);
    assert_eq!(summary.delivered, 1);
    assert_eq!(summary.failed, 0);
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "https://hooks.example.com/fleet");
    assert!(requests[0].1.starts_with("sha256="));
    let body: serde_json::Value = serde_json::from_slice(&requests[0].2).unwrap();
    assert_eq!(body["type"], "task.created");
    drop(requests);
    let pending_outbox: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE topic = 'task.event' AND completed_at IS NULL",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    let pending_runner_commands: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE topic = 'runner.command' AND completed_at IS NULL",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    let delivered_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM webhook_deliveries WHERE endpoint_id = $1 AND status = 'delivered'",
    )
    .bind(webhook_endpoint_id)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(pending_outbox, 0);
    assert_eq!(pending_runner_commands, 1);
    assert_eq!(delivered_rows, 1);
}
