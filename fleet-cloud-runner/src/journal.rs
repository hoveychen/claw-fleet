use std::path::Path;

use fleet_cloud_wire::runner::{CloudCommand, CommandAckStatus};
use rusqlite::{params, Connection, OptionalExtension};

pub struct CommandJournal {
    connection: Connection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistResult {
    Inserted,
    Duplicate,
}

impl CommandJournal {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS commands(
                command_id TEXT PRIMARY KEY,
                assignment_sequence INTEGER NOT NULL UNIQUE,
                command_json TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'persisted',
                result_json TEXT,
                updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );",
        )?;
        Ok(Self { connection })
    }

    pub fn persist(&mut self, command: &CloudCommand) -> anyhow::Result<PersistResult> {
        let encoded = serde_json::to_string(command)?;
        let tx = self.connection.transaction()?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO commands(command_id,assignment_sequence,command_json) VALUES(?1,?2,?3)",
            params![command.command_id, command.assignment_sequence, encoded],
        )?;
        let stored: String = tx.query_row(
            "SELECT command_json FROM commands WHERE command_id=?1",
            [&command.command_id],
            |row| row.get(0),
        )?;
        anyhow::ensure!(
            stored == encoded,
            "duplicate command_id has different payload"
        );
        tx.commit()?;
        Ok(if inserted == 1 {
            PersistResult::Inserted
        } else {
            PersistResult::Duplicate
        })
    }

    pub fn pending(&self) -> anyhow::Result<Vec<CloudCommand>> {
        let mut statement = self.connection.prepare(
            "SELECT command_json FROM commands WHERE status='persisted' ORDER BY assignment_sequence",
        )?;
        let commands = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|value| Ok(serde_json::from_str(&value?)?))
            .collect();
        commands
    }

    pub fn mark_terminal(
        &self,
        command_id: &str,
        status: CommandAckStatus,
        result: Option<&serde_json::Value>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            matches!(
                status,
                CommandAckStatus::Completed | CommandAckStatus::Rejected | CommandAckStatus::Failed
            ),
            "terminal status required"
        );
        let status = serde_json::to_value(status)?.as_str().unwrap().to_owned();
        let result = result.map(serde_json::to_string).transpose()?;
        let current: Option<String> = self
            .connection
            .query_row(
                "SELECT status FROM commands WHERE command_id=?1",
                [command_id],
                |row| row.get(0),
            )
            .optional()?;
        let current = current.ok_or_else(|| anyhow::anyhow!("unknown command"))?;
        if current != "persisted" {
            anyhow::ensure!(current == status, "command already terminal differently");
            return Ok(());
        }
        self.connection.execute(
            "UPDATE commands SET status=?2,result_json=?3,updated_at=CURRENT_TIMESTAMP WHERE command_id=?1",
            params![command_id, status, result],
        )?;
        Ok(())
    }

    pub fn ack_status(&self, command_id: &str) -> anyhow::Result<CommandAckStatus> {
        let status: String = self.connection.query_row(
            "SELECT status FROM commands WHERE command_id=?1",
            [command_id],
            |row| row.get(0),
        )?;
        Ok(match status.as_str() {
            "persisted" => CommandAckStatus::Accepted,
            "completed" => CommandAckStatus::Completed,
            "rejected" => CommandAckStatus::Rejected,
            "failed" => CommandAckStatus::Failed,
            _ => anyhow::bail!("unknown command journal status {status}"),
        })
    }
}
