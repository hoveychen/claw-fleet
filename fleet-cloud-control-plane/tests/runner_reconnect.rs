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
async fn online_runner_claims_pending_commands_for_its_pool_and_capability(pool: sqlx::PgPool) {
    let fingerprint = vec![0x54; 32];
    seed_runner(&pool, &fingerprint).await;
    registry::connect(&pool, &hello(), &fingerprint)
        .await
        .unwrap();

    let mut matching = common::create_task_body("proj_test", "matching");
    matching["runner_pool_id"] = serde_json::json!("pool_test");
    common::post_json(
        common::router(pool.clone()),
        "/tasks",
        common::VALID_KEY,
        Some("runner-auto-claim-001"),
        matching,
    )
    .await;
    let mut wrong_capability = common::create_task_body("proj_test", "wrong capability");
    wrong_capability["external_id"] = serde_json::json!("runner-auto-claim-codex");
    wrong_capability["runner_pool_id"] = serde_json::json!("pool_test");
    wrong_capability["agent"]["provider"] = serde_json::json!("codex");
    common::post_json(
        common::router(pool.clone()),
        "/tasks",
        common::VALID_KEY,
        Some("runner-auto-claim-002"),
        wrong_capability,
    )
    .await;

    let claimed = registry::claim_available_commands(&pool, "runner_test")
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(
        claimed[0].required_capability.as_ref().unwrap().name,
        "claude_code"
    );
    assert_eq!(claimed[0].assignment_sequence, 1);
    assert!(registry::claim_available_commands(&pool, "runner_test")
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM commands WHERE runner_id IS NULL AND status='pending'",
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
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

#[sqlx::test(migrations = "./migrations")]
async fn transcript_records_are_projected_once_in_runner_sequence_order(pool: sqlx::PgPool) {
    let fingerprint = vec![0x88; 32];
    seed_runner(&pool, &fingerprint).await;
    let created = create_task(&pool, "transcript-task-001", "transcript-ticket").await;
    let task_id = created["task"]["id"].as_str().unwrap();
    let run_id = created["run"]["id"].as_str().unwrap();
    let records = [
        (
            "message.created",
            serde_json::json!({"type":"assistant","content":"first"}),
        ),
        (
            "tool.finished",
            serde_json::json!({"type":"tool_result","content":"second"}),
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (event_type, content))| RunnerEvent {
        source_event_id: format!("transcript-{}", index + 1),
        sequence: (index + 1) as u64,
        event_type: event_type.into(),
        occurred_at: Utc::now(),
        data: serde_json::json!({
            "task_id":task_id,"run_id":run_id,
            "record":content,"tool":content,
            "redactions":{"authorization":index + 1}
        }),
        schema_version: 1,
    })
    .collect::<Vec<_>>();

    registry::ingest_events(&pool, "runner_test", records.clone())
        .await
        .unwrap();
    registry::ingest_events(&pool, "runner_test", records)
        .await
        .unwrap();

    let stored = sqlx::query_as::<_, (i64, String, serde_json::Value, serde_json::Value)>(
        "SELECT source_sequence,record_type,content,redactions FROM transcript_records WHERE run_id=$1 ORDER BY source_sequence",
    )
    .bind(run_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].0, 1);
    assert_eq!(stored[0].1, "message.created");
    assert_eq!(stored[0].2["content"], "first");
    assert_eq!(stored[1].0, 2);
    assert_eq!(stored[1].1, "tool.finished");
    assert_eq!(stored[1].2["content"], "second");
    assert_eq!(stored[1].3["authorization"], 2);

    let replay = common::request_json(
        common::router(pool.clone()),
        axum::http::Method::GET,
        &format!("/runs/{run_id}/messages?after=0&limit=10"),
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert_eq!(replay.status, axum::http::StatusCode::OK);
    assert_eq!(replay.body["data"].as_array().unwrap().len(), 2);
    assert_eq!(replay.body["data"][0]["sequence"], 1);
    assert_eq!(replay.body["data"][1]["role"], "tool");
    assert_eq!(
        replay.body["data"][1]["redactions"][0]["kind"],
        "authorization"
    );
}
