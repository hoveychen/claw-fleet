use fleet_cloud_wire::event::RunnerEvent;
use fleet_cloud_wire::runner::{
    ClientHello, CloudCommand, CommandAck, CommandAckStatus, RunnerCapability, ServerHello,
};
use sqlx::{PgPool, Postgres, Transaction};

use crate::error::ApiError;
use crate::routes::runners::authorize_runner_identity;
use crate::services::tasks::new_id;

pub async fn connect(
    pool: &PgPool,
    hello: &ClientHello,
    certificate_fingerprint: &[u8],
) -> Result<ServerHello, ApiError> {
    if hello.protocol_version != fleet_cloud_wire::RUNNER_PROTOCOL_VERSION {
        return Err(ApiError::Validation(
            "unsupported Runner protocol version".into(),
        ));
    }
    authorize_runner_identity(pool, &hello.runner_id, certificate_fingerprint).await?;
    let capabilities = serde_json::to_value(&hello.capabilities).map_err(|_| ApiError::Internal)?;
    let result = sqlx::query(
        "UPDATE runners SET status=CASE WHEN status='draining' THEN 'draining' ELSE 'online' END,
         build_version=$2,platform=$3,architecture=$4,max_concurrency=$5,capabilities=$6,
         last_heartbeat_at=now(),updated_at=now(),version=version+1 WHERE id=$1 AND revoked_at IS NULL",
    )
    .bind(&hello.runner_id)
    .bind(&hello.build_version)
    .bind(&hello.platform)
    .bind(&hello.architecture)
    .bind(i32::from(hello.max_concurrency))
    .bind(capabilities)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::AuthenticationRequired);
    }
    let replay_commands_after = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(assignment_sequence) FROM commands WHERE runner_id=$1 AND status IN ('completed','rejected','failed')",
    ).bind(&hello.runner_id).fetch_one(pool).await?.map(|value| value as u64);
    let accepted_outbox = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(outbox_sequence) FROM runner_source_events WHERE runner_id=$1",
    )
    .bind(&hello.runner_id)
    .fetch_one(pool)
    .await?
    .map(|value| value as u64 + 1)
    .or(Some(1));
    Ok(ServerHello {
        protocol_version: fleet_cloud_wire::RUNNER_PROTOCOL_VERSION,
        heartbeat_interval_seconds: 15,
        config_version: 1,
        replay_commands_after,
        request_outbox_from_sequence: accepted_outbox,
    })
}

pub async fn heartbeat(pool: &PgPool, runner_id: &str, active_runs: u16) -> Result<(), ApiError> {
    let changed = sqlx::query(
        "UPDATE runners SET active_runs=$2,last_heartbeat_at=now(),updated_at=now() WHERE id=$1 AND revoked_at IS NULL",
    ).bind(runner_id).bind(i32::from(active_runs)).execute(pool).await?;
    if changed.rows_affected() == 0 {
        return Err(ApiError::AuthenticationRequired);
    }
    Ok(())
}

pub async fn mark_stale_offline(pool: &PgPool) -> Result<u64, ApiError> {
    Ok(sqlx::query(
        "UPDATE runners SET status='offline',updated_at=now(),version=version+1
         WHERE status='online' AND last_heartbeat_at < now() - interval '45 seconds'",
    )
    .execute(pool)
    .await?
    .rows_affected())
}

pub async fn assign_command(
    pool: &PgPool,
    runner_id: &str,
    command_id: &str,
    required: Option<&RunnerCapability>,
) -> Result<u64, ApiError> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query_as::<_, (String, bool, serde_json::Value)>(
        "SELECT status,scheduling_enabled,capabilities FROM runners WHERE id=$1 AND revoked_at IS NULL FOR UPDATE",
    ).bind(runner_id).fetch_optional(&mut *tx).await?.ok_or(ApiError::RunnerUnavailable)?;
    if row.0 != "online" || !row.1 {
        return Err(ApiError::RunnerUnavailable);
    }
    if let Some(required) = required {
        let advertised: Vec<RunnerCapability> =
            serde_json::from_value(row.2).map_err(|_| ApiError::Internal)?;
        if !advertised.iter().any(|capability| {
            capability.name == required.name && capability.version >= required.version
        }) {
            return Err(ApiError::RunnerCapabilityMissing);
        }
    }
    let sequence = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(assignment_sequence),0)+1 FROM commands WHERE runner_id=$1",
    )
    .bind(runner_id)
    .fetch_one(&mut *tx)
    .await?;
    let changed = sqlx::query(
        "UPDATE commands SET runner_id=$1,assignment_sequence=$2,required_capability=$3 WHERE id=$4 AND runner_id IS NULL",
    ).bind(runner_id).bind(sequence).bind(serde_json::to_value(required).map_err(|_|ApiError::Internal)?).bind(command_id)
        .execute(&mut *tx).await?;
    if changed.rows_affected() == 0 {
        return Err(ApiError::StateConflict);
    }
    tx.commit().await?;
    Ok(sequence as u64)
}

pub async fn pending_commands(
    pool: &PgPool,
    runner_id: &str,
) -> Result<Vec<CloudCommand>, ApiError> {
    let rows = sqlx::query_as::<_, (String,i64,String,Option<String>,Option<String>,chrono::DateTime<chrono::Utc>,Option<i64>,Option<serde_json::Value>,serde_json::Value)>(
        "SELECT id,assignment_sequence,command_type,task_id,run_id,deadline,expected_version,required_capability,payload
         FROM commands WHERE runner_id=$1 AND status IN ('pending','accepted') ORDER BY assignment_sequence",
    ).bind(runner_id).fetch_all(pool).await?;
    rows.into_iter()
        .map(|row| {
            Ok(CloudCommand {
                command_id: row.0,
                assignment_sequence: row.1 as u64,
                command_type: row.2,
                task_id: row.3,
                run_id: row.4,
                deadline: row.5,
                expected_version: row.6,
                required_capability: row
                    .7
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(|_| ApiError::Internal)?,
                payload: row.8,
            })
        })
        .collect()
}

pub async fn acknowledge_command(
    pool: &PgPool,
    runner_id: &str,
    ack: &CommandAck,
) -> Result<(), ApiError> {
    let (status, terminal) = match ack.status {
        CommandAckStatus::Accepted => ("accepted", false),
        CommandAckStatus::Completed => ("completed", true),
        CommandAckStatus::Rejected => ("rejected", true),
        CommandAckStatus::Failed => ("failed", true),
    };
    let changed=sqlx::query(
        "UPDATE commands SET status=$1,accepted_at=CASE WHEN $2 THEN accepted_at ELSE COALESCE(accepted_at,$3) END,
         completed_at=CASE WHEN $2 THEN $3 ELSE completed_at END,error_code=$4
         WHERE id=$5 AND runner_id=$6 AND assignment_sequence=$7
           AND (status IN ('pending','accepted') OR status=$1)",
    ).bind(status).bind(terminal).bind(ack.occurred_at).bind(&ack.error_code).bind(&ack.command_id).bind(runner_id)
        .bind(ack.assignment_sequence as i64).execute(pool).await?;
    if changed.rows_affected() == 0 {
        return Err(ApiError::StateConflict);
    }
    Ok(())
}

pub async fn ingest_events(
    pool: &PgPool,
    runner_id: &str,
    mut events: Vec<RunnerEvent>,
) -> Result<u64, ApiError> {
    events.sort_by_key(|event| event.sequence);
    let mut tx = pool.begin().await?;
    let runner = sqlx::query_as::<_, (String,String)>(
        "SELECT organization_id,project_id FROM runners WHERE id=$1 AND revoked_at IS NULL FOR UPDATE",
    ).bind(runner_id).fetch_optional(&mut *tx).await?.ok_or(ApiError::AuthenticationRequired)?;
    let mut through = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(outbox_sequence) FROM runner_source_events WHERE runner_id=$1",
    )
    .bind(runner_id)
    .fetch_one(&mut *tx)
    .await?
    .unwrap_or(0) as u64;
    for event in events {
        if event.sequence <= through {
            continue;
        }
        if event.sequence != through + 1 {
            break;
        }
        let existing = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM runner_source_events WHERE runner_id=$1 AND source_event_id=$2)",
        ).bind(runner_id).bind(&event.source_event_id).fetch_one(&mut *tx).await?;
        if existing {
            through = event.sequence;
            continue;
        }
        let task_id = event
            .data
            .get("task_id")
            .and_then(|value| value.as_str())
            .map(str::to_owned);
        let run_id = event
            .data
            .get("run_id")
            .and_then(|value| value.as_str())
            .map(str::to_owned);
        let task_sequence = if let Some(task_id) = task_id.as_deref() {
            Some(next_task_sequence(&mut tx, task_id).await?)
        } else {
            None
        };
        let cursor = sqlx::query_scalar::<_,i64>(
            "INSERT INTO events(id,organization_id,project_id,task_id,run_id,event_type,task_sequence,occurred_at,data,schema_version)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING cursor",
        ).bind(new_id("evt")).bind(&runner.0).bind(&runner.1).bind(&task_id).bind(&run_id).bind(&event.event_type)
            .bind(task_sequence).bind(event.occurred_at).bind(&event.data).bind(i32::from(event.schema_version)).fetch_one(&mut *tx).await?;
        sqlx::query("INSERT INTO runner_source_events(runner_id,source_event_id,outbox_sequence,event_cursor) VALUES($1,$2,$3,$4)")
            .bind(runner_id).bind(&event.source_event_id).bind(event.sequence as i64).bind(cursor).execute(&mut *tx).await?;
        through = event.sequence;
    }
    tx.commit().await?;
    Ok(through)
}

async fn next_task_sequence(
    tx: &mut Transaction<'_, Postgres>,
    task_id: &str,
) -> Result<i64, ApiError> {
    Ok(
        sqlx::query_scalar("SELECT COALESCE(MAX(task_sequence),0)+1 FROM events WHERE task_id=$1")
            .bind(task_id)
            .fetch_one(&mut **tx)
            .await?,
    )
}
