use fleet_cloud_api::{
    domain::{
        AgentProfile, AgentTool, CreateTaskRequest, Decision, RequestScope, RunnerEventInput,
        WorkspaceSelector,
    },
    store::{CreateTaskCommand, PgTaskStore, RespondDecisionCommand, StoreError, TaskStore},
    webhook::{PgWebhookDispatcher, WebhookTransport, WebhookTransportResponse},
};
use serde_json::json;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};
use uuid::Uuid;

struct RecordingWebhookTransport {
    requests: Mutex<Vec<(String, String, Vec<u8>)>>,
    statuses: Mutex<VecDeque<u16>>,
}

impl Default for RecordingWebhookTransport {
    fn default() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            statuses: Mutex::new(VecDeque::new()),
        }
    }
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
        Ok(WebhookTransportResponse {
            status: self.statuses.lock().unwrap().pop_front().unwrap_or(204),
        })
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

/// The disposable Postgres this test runs against, or `None` when the machine
/// has not been pointed at one.
///
/// Absent is a *skip*, not a failure. This test needs a live database that it
/// `TRUNCATE`s on the way in, so it can only ever run where someone has stood
/// one up on purpose; CI does not (`ci.yml` builds `fleet-cli` /
/// `claw-fleet-core` / `fleet-hooks-server` only). Failing on absence made a
/// bare `cargo test --workspace` red on every machine that is not doing Fleet
/// Cloud work, which trains everyone to run the workspace suite with
/// `--exclude fleet-cloud-api` — and an excluded crate is an untested one.
fn integration_database() -> Option<String> {
    match std::env::var("DATABASE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => {
            eprintln!(
                "SKIP postgres_create_task_is_atomic_and_idempotent: no DATABASE_URL. \
                 Point it at a disposable database to run this — the test TRUNCATEs \
                 every table, so never aim it at anything you care about."
            );
            None
        }
    }
}

#[tokio::test]
async fn postgres_create_task_is_atomic_and_idempotent() {
    let Some(database_url) = integration_database() else {
        return;
    };
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

    let runner_id = Uuid::new_v4();
    let workspace_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO runners
         (id, organization_id, project_id, display_name, runner_version, connection_state)
         VALUES ($1, $2, $3, 'integration-runner', '0.1.0', 'online')",
    )
    .bind(runner_id)
    .bind(scope.organization_id)
    .bind(scope.project_id)
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workspaces
         (id, organization_id, project_id, runner_id, display_name, locator)
         VALUES ($1, $2, $3, $4, 'Fleet', '/workspace/fleet')",
    )
    .bind(workspace_id)
    .bind(scope.organization_id)
    .bind(scope.project_id)
    .bind(runner_id)
    .execute(store.pool())
    .await
    .unwrap();
    let attempt_id = Uuid::new_v4();
    let decision_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    let decision = Decision {
        id: decision_id,
        task_id: first.task.id,
        attempt_id,
        kind: "fleet_ask".into(),
        blocking: true,
        schema_version: "1.0".into(),
        presentation: json!({ "question": "Ship?", "sourceDecisionId": "native-ask-1" }),
        status: "open".into(),
        response: None,
        created_at: now,
        resolved_at: None,
    };
    let initial_runner_events = vec![
        RunnerEventInput {
            local_sequence: 1,
            dedupe_key: "runner-attempt-1-start".into(),
            task_id: first.task.id,
            attempt_id: Some(attempt_id),
            event_type: "attempt.started".into(),
            occurred_at: now,
            schema_version: "1.0".into(),
            data: json!({
                "workspaceId": workspace_id,
                "agentSource": "codex",
                "agentSessionId": "session-1",
                "ordinal": 1,
                "reason": "initial"
            }),
        },
        RunnerEventInput {
            local_sequence: 2,
            dedupe_key: "runner-decision-1-open".into(),
            task_id: first.task.id,
            attempt_id: Some(attempt_id),
            event_type: "decision.opened".into(),
            occurred_at: now,
            schema_version: "1.0".into(),
            data: json!({ "decision": decision }),
        },
    ];
    let ingested = store
        .ingest_runner_events(scope, runner_id, initial_runner_events.clone())
        .await
        .unwrap();
    assert_eq!(ingested.accepted, 2);
    assert_eq!(ingested.duplicates, 0);
    let replayed_events = store
        .ingest_runner_events(scope, runner_id, initial_runner_events)
        .await
        .unwrap();
    assert_eq!(replayed_events.accepted, 0);
    assert_eq!(replayed_events.duplicates, 2);

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
    let task_after_response = store.get_task(scope, first.task.id).await.unwrap();
    assert_eq!(task_after_response.waiting_decision_count, 0);
    assert_eq!(
        task_after_response.status,
        fleet_cloud_api::domain::TaskStatus::Running
    );
    assert_eq!(task_after_response.event_cursor, 4);

    let successor_attempt_id = Uuid::new_v4();
    let handoff_events = vec![
        RunnerEventInput {
            local_sequence: 3,
            dedupe_key: "runner-attempt-1-ended".into(),
            task_id: first.task.id,
            attempt_id: Some(attempt_id),
            event_type: "attempt.ended".into(),
            occurred_at: now + chrono::Duration::seconds(1),
            schema_version: "1.0".into(),
            data: json!({ "reason": "handoff", "successorAttemptId": successor_attempt_id }),
        },
        RunnerEventInput {
            local_sequence: 4,
            dedupe_key: "runner-attempt-2-started".into(),
            task_id: first.task.id,
            attempt_id: Some(successor_attempt_id),
            event_type: "attempt.started".into(),
            occurred_at: now + chrono::Duration::seconds(2),
            schema_version: "1.0".into(),
            data: json!({
                "workspaceId": workspace_id,
                "agentSource": "codex",
                "agentSessionId": "session-2",
                "ordinal": 2,
                "reason": "handoff"
            }),
        },
        RunnerEventInput {
            local_sequence: 5,
            dedupe_key: "runner-task-succeeded".into(),
            task_id: first.task.id,
            attempt_id: Some(successor_attempt_id),
            event_type: "task.status_changed".into(),
            occurred_at: now + chrono::Duration::seconds(3),
            schema_version: "1.0".into(),
            data: json!({ "status": "succeeded" }),
        },
    ];
    let handoff = store
        .ingest_runner_events(scope, runner_id, handoff_events)
        .await
        .unwrap();
    assert_eq!(handoff.accepted, 3);
    let task_after = store.get_task(scope, first.task.id).await.unwrap();
    assert_eq!(
        task_after.status,
        fleet_cloud_api::domain::TaskStatus::Succeeded
    );
    assert_eq!(task_after.event_cursor, 7);
    let final_detail = store.get_task_detail(scope, first.task.id).await.unwrap();
    assert_eq!(final_detail.attempts.len(), 2);
    assert_eq!(final_detail.attempts[1].ordinal, 2);
    let all_events = store.list_events(scope, first.task.id, 0).await.unwrap();
    assert_eq!(all_events.len(), 7);
    assert!(all_events
        .iter()
        .enumerate()
        .all(|(index, event)| event.sequence == index as i64 + 1));
    let outbox_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE aggregate_id = $1 AND topic = 'task.event'",
    )
    .bind(first.task.id)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(outbox_count, 7);

    let runner_payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM outbox WHERE topic = 'runner.command' AND aggregate_id = $1",
    )
    .bind(attempt_id)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(runner_payload["type"], "decision.response");
    assert_eq!(runner_payload["nativePayload"]["kind"], "fleet-ask");

    let transport = Arc::new(RecordingWebhookTransport::default());
    transport.statuses.lock().unwrap().extend([500, 204]);
    let dispatcher = PgWebhookDispatcher::new(store.pool().clone(), transport.clone());
    let summary = dispatcher.dispatch_once(10).await.unwrap();
    assert_eq!(summary.claimed, 7);
    assert_eq!(summary.delivered, 0);
    assert_eq!(summary.failed, 1);
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "https://hooks.example.com/fleet");
    assert!(requests[0].1.starts_with("sha256="));
    let body: serde_json::Value = serde_json::from_slice(&requests[0].2).unwrap();
    assert_eq!(body["type"], "task.created");
    let first_event_id = body["event_id"].clone();
    drop(requests);
    let pending_after_failure: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE topic = 'task.event' AND completed_at IS NULL",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    let (delivery_id, delivery_status, retry_scheduled): (Uuid, String, bool) = sqlx::query_as(
        "SELECT id, status, next_attempt_at > created_at FROM webhook_deliveries
         WHERE endpoint_id = $1",
    )
    .bind(webhook_endpoint_id)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(pending_after_failure, 1);
    assert_eq!(delivery_status, "retrying");
    assert!(retry_scheduled);

    dispatcher.replay_delivery(delivery_id).await.unwrap();
    let replay_summary = dispatcher.dispatch_once(10).await.unwrap();
    assert_eq!(replay_summary.claimed, 1);
    assert_eq!(replay_summary.delivered, 1);
    assert_eq!(replay_summary.failed, 0);
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let replay_body: serde_json::Value = serde_json::from_slice(&requests[1].2).unwrap();
    assert_eq!(replay_body["event_id"], first_event_id);
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
