mod common;

use chrono::Utc;
use fleet_cloud_control_plane::error::ApiError;
use fleet_cloud_control_plane::runner_gateway::registry;
use fleet_cloud_wire::event::RunnerEvent;
use fleet_cloud_wire::runner::{ClientHello, CommandAck, CommandAckStatus, RunnerCapability};

async fn seed_runner(pool: &sqlx::PgPool, fingerprint: &[u8]) {
    common::seed(pool).await;
    sqlx::query("INSERT INTO runner_pools(id,organization_id,project_id,name) VALUES('pool_test','org_test','proj_test','Test')")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO runners(id,organization_id,project_id,pool_id,name,certificate_fingerprint) VALUES('runner_test','org_test','proj_test','pool_test','Runner', $1)")
        .bind(fingerprint).execute(pool).await.unwrap();
}

fn hello() -> ClientHello {
    ClientHello {
        protocol_version: fleet_cloud_wire::RUNNER_PROTOCOL_VERSION,
        runner_id: "runner_test".into(),
        build_version: "test".into(),
        platform: "linux".into(),
        architecture: "x86_64".into(),
        max_concurrency: 2,
        capabilities: vec![RunnerCapability {
            name: "claude_code".into(),
            version: 1,
        }],
        last_cloud_cursor: None,
        outbox_first_sequence: None,
        outbox_last_sequence: None,
    }
}

async fn create_task(pool: &sqlx::PgPool, key: &str, external: &str) -> serde_json::Value {
    let mut body = common::create_task_body("proj_test", external);
    body["external_id"] = serde_json::json!(external);
    common::post_json(
        common::router(pool.clone()),
        "/tasks",
        common::VALID_KEY,
        Some(key),
        body,
    )
    .await
    .body
}

#[sqlx::test(migrations = "./migrations")]
async fn reconnect_replays_commands_and_deduplicates_one_hundred_events(pool: sqlx::PgPool) {
    let fingerprint = vec![0x55; 32];
    seed_runner(&pool, &fingerprint).await;
    let server = registry::connect(&pool, &hello(), &fingerprint)
        .await
        .unwrap();
    assert_eq!(server.heartbeat_interval_seconds, 15);
    assert_eq!(server.request_outbox_from_sequence, Some(1));

    let first = create_task(&pool, "runner-task-001", "runner-ticket-1").await;
    let task_id = first["task"]["id"].as_str().unwrap();
    let run_id = first["run"]["id"].as_str().unwrap();
    let command_id: String = sqlx::query_scalar("SELECT id FROM commands WHERE task_id=$1")
        .bind(task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let sequence = registry::assign_command(
        &pool,
        "runner_test",
        &command_id,
        Some(&RunnerCapability {
            name: "claude_code".into(),
            version: 1,
        }),
    )
    .await
    .unwrap();
    assert_eq!(sequence, 1);
    assert_eq!(
        registry::pending_commands(&pool, "runner_test")
            .await
            .unwrap()
            .len(),
        1
    );

    let second = create_task(&pool, "runner-task-002", "runner-ticket-2").await;
    let second_command: String = sqlx::query_scalar("SELECT id FROM commands WHERE task_id=$1")
        .bind(second["task"]["id"].as_str().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    let missing = registry::assign_command(
        &pool,
        "runner_test",
        &second_command,
        Some(&RunnerCapability {
            name: "codex".into(),
            version: 1,
        }),
    )
    .await
    .unwrap_err();
    assert!(matches!(missing, ApiError::RunnerCapabilityMissing));

    let accepted = CommandAck {
        command_id: command_id.clone(),
        assignment_sequence: 1,
        status: CommandAckStatus::Accepted,
        occurred_at: Utc::now(),
        result: None,
        error_code: None,
    };
    registry::acknowledge_command(&pool, "runner_test", &accepted)
        .await
        .unwrap();
    assert_eq!(
        registry::pending_commands(&pool, "runner_test")
            .await
            .unwrap()
            .len(),
        1
    );
    let completed = CommandAck {
        status: CommandAckStatus::Completed,
        ..accepted
    };
    registry::acknowledge_command(&pool, "runner_test", &completed)
        .await
        .unwrap();
    registry::acknowledge_command(&pool, "runner_test", &completed)
        .await
        .unwrap();
    assert!(registry::pending_commands(&pool, "runner_test")
        .await
        .unwrap()
        .is_empty());

    let events = (1..=100)
        .map(|sequence| RunnerEvent {
            source_event_id: format!("source_{sequence}"),
            sequence,
            event_type: "run.output".into(),
            occurred_at: Utc::now(),
            data: serde_json::json!({"task_id":task_id,"run_id":run_id,"part":sequence}),
            schema_version: 1,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        registry::ingest_events(&pool, "runner_test", events.clone())
            .await
            .unwrap(),
        100
    );
    assert_eq!(
        registry::ingest_events(&pool, "runner_test", events)
            .await
            .unwrap(),
        100
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM runner_source_events")
            .fetch_one(&pool)
            .await
            .unwrap(),
        100
    );
    let reconnected = registry::connect(&pool, &hello(), &fingerprint)
        .await
        .unwrap();
    assert_eq!(reconnected.replay_commands_after, Some(1));
    assert_eq!(reconnected.request_outbox_from_sequence, Some(101));

    registry::heartbeat(&pool, "runner_test", 1).await.unwrap();
    sqlx::query(
        "UPDATE runners SET last_heartbeat_at=now()-interval '46 seconds' WHERE id='runner_test'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(registry::mark_stale_offline(&pool).await.unwrap(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM runners WHERE id='runner_test'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "offline"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn drain_prevents_new_assignment(pool: sqlx::PgPool) {
    let fingerprint = vec![0x66; 32];
    seed_runner(&pool, &fingerprint).await;
    registry::connect(&pool, &hello(), &fingerprint)
        .await
        .unwrap();
    let task = create_task(&pool, "runner-drain-001", "runner-drain").await;
    let command_id: String = sqlx::query_scalar("SELECT id FROM commands WHERE task_id=$1")
        .bind(task["task"]["id"].as_str().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE runners SET status='draining',scheduling_enabled=false WHERE id='runner_test'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let error = registry::assign_command(&pool, "runner_test", &command_id, None)
        .await
        .unwrap_err();
    assert!(matches!(error, ApiError::RunnerUnavailable));
}

#[sqlx::test(migrations = "./migrations")]
async fn harness_events_project_runs_and_handoff_without_finishing_task_early(pool: sqlx::PgPool) {
    let fingerprint = vec![0x77; 32];
    seed_runner(&pool, &fingerprint).await;
    let created = create_task(&pool, "projection-task-001", "projection-ticket").await;
    let task_id = created["task"]["id"].as_str().unwrap();
    let first_run = created["run"]["id"].as_str().unwrap();
    let second_run = "run_handoff_projection";
    let facts = vec![
        (
            "run.started",
            first_run,
            serde_json::json!({"status":"running"}),
        ),
        (
            "run.assigned",
            second_run,
            serde_json::json!({"predecessor_run_id":first_run,"provider":"claude_code"}),
        ),
        (
            "run.finished",
            first_run,
            serde_json::json!({"status":"succeeded"}),
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (event_type, run_id, mut data))| {
        data["task_id"] = serde_json::json!(task_id);
        data["run_id"] = serde_json::json!(run_id);
        RunnerEvent {
            source_event_id: format!("projection-{}", index + 1),
            sequence: (index + 1) as u64,
            event_type: event_type.into(),
            occurred_at: Utc::now(),
            data,
            schema_version: 1,
        }
    })
    .collect();
    assert_eq!(
        registry::ingest_events(&pool, "runner_test", facts)
            .await
            .unwrap(),
        3
    );
    let task: (String, Option<String>) =
        sqlx::query_as("SELECT status,active_run_id FROM tasks WHERE id=$1")
            .bind(task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(task.0, "running");
    assert_eq!(task.1.as_deref(), Some(second_run));
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM runs WHERE id=$1")
            .bind(first_run)
            .fetch_one(&pool)
            .await
            .unwrap(),
        "succeeded"
    );

    let terminal = [
        ("run.started", "running"),
        ("run.finished", "succeeded"),
        ("task.completed", "succeeded"),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (event_type, status))| RunnerEvent {
        source_event_id: format!("projection-{}", index + 4),
        sequence: (index + 4) as u64,
        event_type: event_type.into(),
        occurred_at: Utc::now(),
        data: serde_json::json!({"task_id":task_id,"run_id":second_run,"status":status}),
        schema_version: 1,
    })
    .collect();
    assert_eq!(
        registry::ingest_events(&pool, "runner_test", terminal)
            .await
            .unwrap(),
        6
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM tasks WHERE id=$1")
            .bind(task_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        "succeeded"
    );
}
