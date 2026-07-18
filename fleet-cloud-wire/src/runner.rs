use serde::{Deserialize, Serialize};

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
}
