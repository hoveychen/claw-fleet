use serde::{Deserialize, Serialize};
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
}
