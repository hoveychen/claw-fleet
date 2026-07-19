mod common;

use axum::http::{Method, StatusCode};

#[sqlx::test(migrations = "./migrations")]
async fn hosted_web_reads_run_and_normalized_usage(pool: sqlx::PgPool) {
    common::seed(&pool).await;
    let created = common::post_json(
        common::router(pool.clone()),
        "/tasks",
        common::VALID_KEY,
        Some("hosted-run-001"),
        common::create_task_body("proj_test", "ship hosted web"),
    )
    .await;
    let run_id = created.body["run"]["id"].as_str().unwrap();
    let task_id = created.body["task"]["id"].as_str().unwrap();
    sqlx::query(
        "INSERT INTO events(id,organization_id,project_id,task_id,run_id,event_type,task_sequence,occurred_at,data)
         VALUES('evt_usage_1','org_test','proj_test',$1,$2,'usage.updated',2,now(),$3)",
    )
    .bind(task_id)
    .bind(run_id)
    .bind(serde_json::json!({
        "input_tokens":1200,"output_tokens":345,"cache_read_tokens":80,
        "cache_write_tokens":20,"reasoning_tokens":45,"cost_usd":1.25,
        "run_seconds":42,"event_bytes":900
    }))
    .execute(&pool)
    .await
    .unwrap();

    let run = common::request_json(
        common::router(pool.clone()),
        Method::GET,
        &format!("/runs/{run_id}"),
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert_eq!(run.status, StatusCode::OK);
    assert_eq!(run.body["id"], run_id);
    assert_eq!(run.body["agent"]["provider"], "claude_code");

    let usage = common::request_json(
        common::router(pool.clone()),
        Method::GET,
        &format!("/runs/{run_id}/usage"),
        common::VALID_KEY,
        &[],
        None,
    )
    .await;
    assert_eq!(usage.status, StatusCode::OK);
    assert_eq!(usage.body["input_tokens"], 1200);
    assert_eq!(usage.body["output_tokens"], 345);
    assert_eq!(usage.body["cache_read_tokens"], 80);
    assert_eq!(usage.body["provider_cost_usd"], 1.25);
    assert_eq!(usage.body["run_seconds"], 42);
}
