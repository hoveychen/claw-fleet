use crate::protocol::RunnerEvent;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;
use std::{path::Path, sync::Mutex};
use thiserror::Error;
use uuid::Uuid;

const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Error)]
pub enum SpoolError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("spool lock poisoned")]
    LockPoisoned,
    #[error("command {0} is not pending")]
    CommandNotPending(Uuid),
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandClaim {
    New,
    InProgress,
    AlreadyApplied { result: Value },
    Rejected { error: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpoolStats {
    pub pending_events: i64,
    pub accepted_through_local_sequence: i64,
}

pub struct Spool {
    connection: Mutex<Connection>,
}

impl Spool {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SpoolError> {
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS runner_meta (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS applied_commands (
                 command_id TEXT PRIMARY KEY,
                 payload TEXT NOT NULL,
                 status TEXT NOT NULL CHECK (status IN ('in_progress', 'applied', 'rejected')),
                 result TEXT,
                 error TEXT,
                 created_at TEXT NOT NULL,
                 completed_at TEXT
             );
             CREATE TABLE IF NOT EXISTS event_spool (
                 local_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                 dedupe_key TEXT NOT NULL UNIQUE,
                 task_id TEXT NOT NULL,
                 attempt_id TEXT,
                 event_type TEXT NOT NULL,
                 occurred_at TEXT NOT NULL,
                 schema_version TEXT NOT NULL,
                 data TEXT NOT NULL
             );",
        )?;
        connection.execute(
            "INSERT INTO runner_meta (key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [SCHEMA_VERSION.to_string()],
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO runner_meta (key, value)
             VALUES ('accepted_through_local_sequence', '0')",
            [],
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn claim_command(
        &self,
        command_id: Uuid,
        payload: &Value,
    ) -> Result<CommandClaim, SpoolError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| SpoolError::LockPoisoned)?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO applied_commands
             (command_id, payload, status, created_at)
             VALUES (?1, ?2, 'in_progress', ?3)",
            params![
                command_id.to_string(),
                serde_json::to_string(payload)?,
                Utc::now().to_rfc3339()
            ],
        )? == 1;
        if inserted {
            tx.commit()?;
            return Ok(CommandClaim::New);
        }
        let row: (String, Option<String>, Option<String>) = tx.query_row(
            "SELECT status, result, error FROM applied_commands WHERE command_id = ?1",
            [command_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        tx.commit()?;
        match row.0.as_str() {
            "in_progress" => Ok(CommandClaim::InProgress),
            "applied" => Ok(CommandClaim::AlreadyApplied {
                result: serde_json::from_str(row.1.as_deref().unwrap_or("null"))?,
            }),
            "rejected" => Ok(CommandClaim::Rejected {
                error: row.2.unwrap_or_else(|| "command rejected".into()),
            }),
            other => Err(SpoolError::Json(serde_json::Error::io(
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unknown command status {other}"),
                ),
            ))),
        }
    }

    pub fn complete_command(&self, command_id: Uuid, result: &Value) -> Result<(), SpoolError> {
        let changed = self
            .connection
            .lock()
            .map_err(|_| SpoolError::LockPoisoned)?
            .execute(
                "UPDATE applied_commands
                 SET status = 'applied', result = ?2, completed_at = ?3
                 WHERE command_id = ?1 AND status = 'in_progress'",
                params![
                    command_id.to_string(),
                    serde_json::to_string(result)?,
                    Utc::now().to_rfc3339()
                ],
            )?;
        if changed == 1 {
            Ok(())
        } else {
            Err(SpoolError::CommandNotPending(command_id))
        }
    }

    pub fn reject_command(&self, command_id: Uuid, error: &str) -> Result<(), SpoolError> {
        let changed = self
            .connection
            .lock()
            .map_err(|_| SpoolError::LockPoisoned)?
            .execute(
                "UPDATE applied_commands
                 SET status = 'rejected', error = ?2, completed_at = ?3
                 WHERE command_id = ?1 AND status = 'in_progress'",
                params![command_id.to_string(), error, Utc::now().to_rfc3339()],
            )?;
        if changed == 1 {
            Ok(())
        } else {
            Err(SpoolError::CommandNotPending(command_id))
        }
    }

    pub fn last_acked_command(&self) -> Result<Option<Uuid>, SpoolError> {
        let value: Option<String> = self
            .connection
            .lock()
            .map_err(|_| SpoolError::LockPoisoned)?
            .query_row(
                "SELECT command_id FROM applied_commands
                 WHERE status IN ('applied', 'rejected')
                 ORDER BY completed_at DESC, rowid DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        value
            .map(|value| {
                Uuid::parse_str(&value).map_err(|error| {
                    SpoolError::Json(serde_json::Error::io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        error,
                    )))
                })
            })
            .transpose()
    }

    pub fn append_event(
        &self,
        dedupe_key: &str,
        task_id: Uuid,
        attempt_id: Option<Uuid>,
        event_type: &str,
        schema_version: &str,
        data: &Value,
    ) -> Result<i64, SpoolError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SpoolError::LockPoisoned)?;
        connection.execute(
            "INSERT OR IGNORE INTO event_spool
             (dedupe_key, task_id, attempt_id, event_type, occurred_at, schema_version, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                dedupe_key,
                task_id.to_string(),
                attempt_id.map(|id| id.to_string()),
                event_type,
                Utc::now().to_rfc3339(),
                schema_version,
                serde_json::to_string(data)?
            ],
        )?;
        connection
            .query_row(
                "SELECT local_sequence FROM event_spool WHERE dedupe_key = ?1",
                [dedupe_key],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn pending_events(&self, limit: usize) -> Result<Vec<RunnerEvent>, SpoolError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SpoolError::LockPoisoned)?;
        let mut statement = connection.prepare(
            "SELECT local_sequence, dedupe_key, task_id, attempt_id, event_type,
                    occurred_at, schema_version, data
             FROM event_spool ORDER BY local_sequence ASC LIMIT ?1",
        )?;
        let rows = statement.query_map([limit.max(1) as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
            ))
        })?;
        rows.map(|row| {
            let row = row?;
            Ok(RunnerEvent {
                local_sequence: row.0,
                dedupe_key: row.1,
                task_id: parse_uuid(&row.2)?,
                attempt_id: row.3.as_deref().map(parse_uuid).transpose()?,
                event_type: row.4,
                occurred_at: row.5,
                schema_version: row.6,
                data: serde_json::from_str(&row.7)?,
            })
        })
        .collect()
    }

    pub fn acknowledge_events(&self, accepted_through: i64) -> Result<usize, SpoolError> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| SpoolError::LockPoisoned)?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: i64 = tx
            .query_row(
                "SELECT value FROM runner_meta WHERE key = 'accepted_through_local_sequence'",
                [],
                |row| row.get::<_, String>(0),
            )?
            .parse()
            .unwrap_or(0);
        let watermark = current.max(accepted_through.max(0));
        tx.execute(
            "UPDATE runner_meta SET value = ?1
             WHERE key = 'accepted_through_local_sequence'",
            [watermark.to_string()],
        )?;
        let removed = tx.execute(
            "DELETE FROM event_spool WHERE local_sequence <= ?1",
            [watermark],
        )?;
        tx.commit()?;
        Ok(removed)
    }

    pub fn stats(&self) -> Result<SpoolStats, SpoolError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| SpoolError::LockPoisoned)?;
        let pending_events =
            connection.query_row("SELECT count(*) FROM event_spool", [], |row| row.get(0))?;
        let accepted = connection.query_row(
            "SELECT value FROM runner_meta WHERE key = 'accepted_through_local_sequence'",
            [],
            |row| row.get::<_, String>(0),
        )?;
        Ok(SpoolStats {
            pending_events,
            accepted_through_local_sequence: accepted.parse().unwrap_or(0),
        })
    }
}

fn parse_uuid(value: &str) -> Result<Uuid, SpoolError> {
    Uuid::parse_str(value).map_err(|error| {
        SpoolError::Json(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error,
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn duplicate_command_is_claimed_once_and_replays_result() {
        let dir = tempdir().unwrap();
        let spool = Spool::open(dir.path().join("runner.db")).unwrap();
        let command_id = Uuid::new_v4();
        let payload = json!({ "type": "launch", "attemptId": Uuid::new_v4() });
        assert_eq!(
            spool.claim_command(command_id, &payload).unwrap(),
            CommandClaim::New
        );
        for _ in 0..19 {
            assert_eq!(
                spool.claim_command(command_id, &payload).unwrap(),
                CommandClaim::InProgress
            );
        }
        let result = json!({ "pidRef": "opaque-1" });
        spool.complete_command(command_id, &result).unwrap();
        assert_eq!(
            spool.claim_command(command_id, &payload).unwrap(),
            CommandClaim::AlreadyApplied {
                result: result.clone()
            }
        );
        assert_eq!(spool.last_acked_command().unwrap(), Some(command_id));
    }

    #[test]
    fn event_spool_survives_restart_deduplicates_and_compacts_after_ack() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("runner.db");
        let task_id = Uuid::new_v4();
        let first_sequence;
        {
            let spool = Spool::open(&path).unwrap();
            first_sequence = spool
                .append_event(
                    "event-001",
                    task_id,
                    None,
                    "attempt.started",
                    "1.0",
                    &json!({ "ordinal": 1 }),
                )
                .unwrap();
            assert_eq!(
                spool
                    .append_event(
                        "event-001",
                        task_id,
                        None,
                        "attempt.started",
                        "1.0",
                        &json!({ "ordinal": 1 }),
                    )
                    .unwrap(),
                first_sequence
            );
            spool
                .append_event(
                    "event-002",
                    task_id,
                    Some(Uuid::new_v4()),
                    "attempt.ended",
                    "1.0",
                    &json!({}),
                )
                .unwrap();
        }

        let reopened = Spool::open(&path).unwrap();
        let pending = reopened.pending_events(100).unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].local_sequence, first_sequence);
        assert_eq!(pending[0].event_type, "attempt.started");
        assert_eq!(reopened.acknowledge_events(first_sequence).unwrap(), 1);
        assert_eq!(reopened.pending_events(100).unwrap().len(), 1);
        assert_eq!(
            reopened.stats().unwrap(),
            SpoolStats {
                pending_events: 1,
                accepted_through_local_sequence: first_sequence,
            }
        );
    }
}
