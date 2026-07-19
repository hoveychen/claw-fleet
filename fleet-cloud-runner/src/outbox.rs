use std::path::Path;

use fleet_cloud_wire::event::RunnerEvent;
use rusqlite::{params, Connection};

pub struct EventOutbox {
    connection: Connection,
}

impl EventOutbox {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS outbox(
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                source_event_id TEXT NOT NULL UNIQUE,
                event_json TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );",
        )?;
        Ok(Self { connection })
    }

    pub fn append(&self, mut event: RunnerEvent) -> anyhow::Result<u64> {
        let tx = self.connection.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO outbox(source_event_id,event_json) VALUES(?1,'')",
            [&event.source_event_id],
        )?;
        let sequence: i64 = tx.query_row(
            "SELECT sequence FROM outbox WHERE source_event_id=?1",
            [&event.source_event_id],
            |row| row.get(0),
        )?;
        event.sequence = sequence as u64;
        tx.execute(
            "UPDATE outbox SET event_json=?2 WHERE source_event_id=?1 AND event_json=''",
            params![event.source_event_id, serde_json::to_string(&event)?],
        )?;
        tx.commit()?;
        Ok(sequence as u64)
    }

    pub fn batch_after(&self, sequence: u64, limit: usize) -> anyhow::Result<Vec<RunnerEvent>> {
        let mut statement = self.connection.prepare(
            "SELECT event_json FROM outbox WHERE sequence>?1 ORDER BY sequence LIMIT ?2",
        )?;
        let events = statement
            .query_map(params![sequence as i64, limit as i64], |row| {
                row.get::<_, String>(0)
            })?
            .map(|value| Ok(serde_json::from_str(&value?)?))
            .collect();
        events
    }

    pub fn acknowledge_through(&self, sequence: u64) -> anyhow::Result<usize> {
        Ok(self
            .connection
            .execute("DELETE FROM outbox WHERE sequence<=?1", [sequence as i64])?)
    }

    pub fn range(&self) -> anyhow::Result<(Option<u64>, Option<u64>)> {
        let (first, last): (Option<i64>, Option<i64>) = self.connection.query_row(
            "SELECT MIN(sequence),MAX(sequence) FROM outbox",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok((
            first.map(|value| value as u64),
            last.map(|value| value as u64),
        ))
    }
}
