use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const PROTOCOL_VERSION: &str = "2026-07-18";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub protocol_version: String,
    pub message_id: Uuid,
    pub payload: T,
}

impl<T> Envelope<T> {
    pub fn new(payload: T) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION.to_string(),
            message_id: Uuid::new_v4(),
            payload,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub runner_id: Uuid,
    pub runner_version: String,
    pub protocol_versions: Vec<String>,
    pub capabilities: Vec<String>,
    pub last_acked_command: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Welcome {
    pub connection_id: Uuid,
    pub selected_protocol: String,
    pub heartbeat_interval_seconds: u32,
    pub max_batch: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAdvertisement {
    pub workspace_id: Uuid,
    pub display_name: String,
    pub labels: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveAttempt {
    pub task_id: Uuid,
    pub attempt_id: Uuid,
    pub agent_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Heartbeat {
    pub workspaces: Vec<WorkspaceAdvertisement>,
    pub active_attempts: Vec<ActiveAttempt>,
    pub running_count: u32,
    pub capacity: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Command {
    pub command_id: Uuid,
    pub lease_id: Uuid,
    pub lease_expires_at: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandAckState {
    Accepted,
    AlreadyApplied,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandAck {
    pub command_id: Uuid,
    pub state: CommandAckState,
    pub result: Option<Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerEvent {
    pub local_sequence: i64,
    pub dedupe_key: String,
    pub task_id: Uuid,
    pub attempt_id: Option<Uuid>,
    pub event_type: String,
    pub occurred_at: String,
    pub schema_version: String,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventBatch {
    pub batch_id: Uuid,
    pub events: Vec<RunnerEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventAck {
    pub batch_id: Uuid,
    pub accepted_through_local_sequence: i64,
}

pub fn negotiate_protocol(runner_versions: &[String], cloud_versions: &[String]) -> Option<String> {
    cloud_versions
        .iter()
        .find(|version| runner_versions.contains(version))
        .cloned()
}

pub fn capabilities_satisfy(advertised: &[String], required: &[String]) -> bool {
    required.iter().all(|item| advertised.contains(item))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trips_as_json() {
        let hello = Hello {
            runner_id: Uuid::new_v4(),
            runner_version: "0.1.0".into(),
            protocol_versions: vec![PROTOCOL_VERSION.into()],
            capabilities: vec!["agent.codex".into()],
            last_acked_command: None,
        };
        let envelope = Envelope::new(hello);
        let json = serde_json::to_string(&envelope).unwrap();
        let decoded: Envelope<Hello> = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn negotiation_uses_cloud_preference_and_rejects_no_overlap() {
        let runner = vec!["v1".into(), "v2".into()];
        let cloud = vec!["v3".into(), "v2".into(), "v1".into()];
        assert_eq!(negotiate_protocol(&runner, &cloud), Some("v2".into()));
        assert_eq!(negotiate_protocol(&runner, &["v3".into()]), None);
    }

    #[test]
    fn capability_match_requires_every_requested_capability() {
        let advertised = vec!["agent.codex".into(), "task.handoff.v1".into()];
        assert!(capabilities_satisfy(&advertised, &["agent.codex".into()]));
        assert!(!capabilities_satisfy(&advertised, &["agent.claude".into()]));
    }
}
