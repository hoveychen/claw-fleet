use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Guard,
    Elicitation,
    FleetAsk,
    PlanApproval,
    PermissionPrompt,
    A2ui,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionCreated {
    pub source_decision_id: String,
    pub kind: DecisionKind,
    pub payload: Value,
    pub response_schema: Value,
    pub deadline: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionResponse {
    pub action: String,
    #[serde(default)]
    pub answers: Value,
}

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
