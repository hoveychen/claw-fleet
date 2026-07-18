use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Durable fact emitted by a Runner and recorded by the control plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunnerEvent {
    pub source_event_id: String,
    pub sequence: u64,
    pub event_type: String,
    pub occurred_at: DateTime<Utc>,
    pub data: Value,
    pub schema_version: u16,
}
