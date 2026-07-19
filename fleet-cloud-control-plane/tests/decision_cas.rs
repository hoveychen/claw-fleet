mod common;

use axum::http::{Method, StatusCode};
use chrono::{Duration, Utc};
use fleet_cloud_control_plane::runner_gateway::registry;
use fleet_cloud_wire::event::RunnerEvent;

async fn seed_decision(pool: &sqlx::PgPool, id: &str, deadline: chrono::DateTime<Utc>) {
    common::seed(pool).await;
    sqlx::query("INSERT INTO runner_pools(id,organization_id,project_id,name) VALUES('pool_decision','org_test','proj_test','Decision pool')")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO runners(id,organization_id,project_id,pool_id,name,certificate_fingerprint,status) VALUES('runner_decision','org_test','proj_test','pool_decision','Runner',decode('55','hex'),'online')")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO tasks(id,organization_id,project_id,goal,workspace,status,active_run_id,created_by_type,created_by_id) VALUES('task_decision','org_test','proj_test','answer','{}','waiting_input',NULL,'api_key','key_valid')")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO runs(id,organization_id,project_id,task_id,attempt,runner_id,status,provider) VALUES('run_decision','org_test','proj_test','task_decision',1,'runner_decision','waiting_input','claude_code')")
        .execute(pool).await.unwrap();
    sqlx::query("UPDATE tasks SET active_run_id='run_decision' WHERE id='task_decision'")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO decisions(id,organization_id,project_id,task_id,run_id,runner_id,source_decision_id,kind,payload,response_schema,deadline,created_at,updated_at) VALUES($1,'org_test','proj_test','task_decision','run_decision','runner_decision','source_decision','fleet_ask','{\"questions\":[]}','{}',$2,now(),now())")
        .bind(id).bind(deadline).execute(pool).await.unwrap();
}

async fn answer(
    pool: &sqlx::PgPool,
    id: &str,
    key: &str,
    if_match: Option<&str>,
) -> common::JsonResponse {
    let mut headers = vec![("idempotency-key", key)];
    if let Some(value) = if_match {
        headers.push(("if-match", value));
    }
    common::request_json(
        common::router(pool.clone()),
        Method::POST,
        &format!("/decisions/{id}/responses"),
        common::VALID_KEY,
        &headers,
        Some(serde_json::json!({"action":"answer","answers":{"Question":"Yes"}})),
    )
    .await
}

#[sqlx::test(migrations = "./migrations")]
async fn response_requires_if_match_and_rejects_stale_or_second_answers(pool: sqlx::PgPool) {
    seed_decision(&pool, "dec_cas", Utc::now() + Duration::hours(1)).await;
    let missing = answer(&pool, "dec_cas", "decision-missing", None).await;
    assert_eq!(missing.status, StatusCode::PRECONDITION_REQUIRED);
    let stale = answer(
        &pool,
        "dec_cas",
        "decision-stale",
        Some("\"decision-version-9\""),
    )
    .await;
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED);

    let accepted = answer(
        &pool,
        "dec_cas",
        "decision-answer",
        Some("\"decision-version-1\""),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::ACCEPTED, "{}", accepted.body);
    assert_eq!(accepted.body["decision"]["status"], "answer_queued");
    assert_eq!(accepted.body["decision"]["version"], 2);
    assert_eq!(accepted.body["command"]["status"], "accepted");
    let replay = answer(
        &pool,
        "dec_cas",
        "decision-answer",
        Some("\"decision-version-1\""),
    )
    .await;
    assert_eq!(replay.status, StatusCode::ACCEPTED);
    assert_eq!(replay.body, accepted.body);
    let second = answer(
        &pool,
        "dec_cas",
        "decision-second",
        Some("\"decision-version-2\""),
    )
    .await;
    assert_eq!(second.status, StatusCode::CONFLICT);
    assert_eq!(second.body["error"]["code"], "decision_already_resolved");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM commands WHERE command_type='resolve_decision'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_answers_have_exactly_one_winner(pool: sqlx::PgPool) {
    seed_decision(&pool, "dec_race", Utc::now() + Duration::hours(1)).await;
    let (left, right) = tokio::join!(
        answer(&pool, "dec_race", "decision-race-left", Some("\"1\"")),
        answer(&pool, "dec_race", "decision-race-right", Some("\"1\"")),
    );
    let statuses = [left.status, right.status];
    assert_eq!(
        statuses
            .iter()
            .filter(|&&s| s == StatusCode::ACCEPTED)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|&&s| s == StatusCode::CONFLICT)
            .count(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM commands WHERE command_type='resolve_decision'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn expired_decision_is_persisted_and_cannot_be_answered(pool: sqlx::PgPool) {
    seed_decision(&pool, "dec_expired", Utc::now() - Duration::seconds(1)).await;
    let response = answer(&pool, "dec_expired", "decision-expired", Some("\"1\"")).await;
    assert_eq!(response.status, StatusCode::CONFLICT);
    assert_eq!(response.body["error"]["code"], "decision_expired");
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM decisions WHERE id='dec_expired'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "expired"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn queued_answer_survives_router_restart_and_runner_delivery(pool: sqlx::PgPool) {
    seed_decision(&pool, "dec_restart", Utc::now() + Duration::hours(1)).await;
    let response = answer(&pool, "dec_restart", "decision-restart", Some("\"1\"")).await;
    assert_eq!(response.status, StatusCode::ACCEPTED);
    drop(common::router(pool.clone()));

    let restored = common::request_json(
        common::router(pool.clone()),
        Method::GET,
        "/decisions/dec_restart",
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert_eq!(restored.body["status"], "answer_queued");
    let commands = registry::pending_commands(&pool, "runner_decision")
        .await
        .unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].payload["source_decision_id"], "source_decision");

    let delivered = RunnerEvent {
        source_event_id: "dec_restart:delivered".into(),
        sequence: 1,
        event_type: "decision.delivered".into(),
        occurred_at: Utc::now(),
        data: serde_json::json!({
            "task_id":"task_decision","run_id":"run_decision",
            "decision_id":"dec_restart","source_decision_id":"source_decision",
            "status":"answered"
        }),
        schema_version: 1,
    };
    assert_eq!(
        registry::ingest_events(&pool, "runner_decision", vec![delivered])
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_as::<_, (String, String, String)>(
            "SELECT d.status,t.status,r.status FROM decisions d JOIN tasks t ON t.id=d.task_id JOIN runs r ON r.id=d.run_id WHERE d.id='dec_restart'"
        ).fetch_one(&pool).await.unwrap(),
        ("answered".into(), "running".into(), "running".into())
    );
}
