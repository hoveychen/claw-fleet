use fleet_cloud_api::{
    domain::{AgentProfile, AgentTool, CreateTaskRequest, RequestScope, WorkspaceSelector},
    store::{CreateTaskCommand, PgTaskStore, StoreError, TaskStore},
};
use std::collections::BTreeMap;
use uuid::Uuid;

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
}
