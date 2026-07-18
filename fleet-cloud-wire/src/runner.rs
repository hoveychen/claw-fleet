use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::RunnerEvent;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerCapability {
    pub name: String,
    pub version: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHello {
    pub protocol_version: u16,
    pub runner_id: String,
    pub build_version: String,
    pub platform: String,
    pub architecture: String,
    pub max_concurrency: u16,
    pub capabilities: Vec<RunnerCapability>,
    pub last_cloud_cursor: Option<String>,
    pub outbox_first_sequence: Option<u64>,
    pub outbox_last_sequence: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerHello {
    pub protocol_version: u16,
    pub heartbeat_interval_seconds: u16,
    pub config_version: u64,
    pub replay_commands_after: Option<u64>,
    pub request_outbox_from_sequence: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CloudCommand {
    pub command_id: String,
    pub assignment_sequence: u64,
    pub command_type: String,
    pub task_id: Option<String>,
    pub run_id: Option<String>,
    pub deadline: DateTime<Utc>,
    pub expected_version: Option<i64>,
    pub required_capability: Option<RunnerCapability>,
    pub payload: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandAckStatus {
    Accepted,
    Completed,
    Rejected,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandAck {
    pub command_id: String,
    pub assignment_sequence: u64,
    pub status: CommandAckStatus,
    pub occurred_at: DateTime<Utc>,
    pub result: Option<Value>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RunnerFrame {
    ClientHello(ClientHello),
    EventBatch {
        first_sequence: u64,
        events: Vec<RunnerEvent>,
    },
    Heartbeat {
        active_runs: u16,
    },
    CommandAck(CommandAck),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ServerFrame {
    ServerHello(ServerHello),
    Command(CloudCommand),
    BatchAck { through_sequence: u64 },
}
