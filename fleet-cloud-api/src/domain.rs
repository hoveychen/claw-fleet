use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Assigned,
    Running,
    WaitingForInput,
    Paused,
    RateLimited,
    Succeeded,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Assigned => "assigned",
            Self::Running => "running",
            Self::WaitingForInput => "waiting_for_input",
            Self::Paused => "paused",
            Self::RateLimited => "rate_limited",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl FromStr for TaskStatus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "queued" => Ok(Self::Queued),
            "assigned" => Ok(Self::Assigned),
            "running" => Ok(Self::Running),
            "waiting_for_input" => Ok(Self::WaitingForInput),
            "paused" => Ok(Self::Paused),
            "rate_limited" => Ok(Self::RateLimited),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("unknown task status: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub project_id: Uuid,
    pub external_id: Option<String>,
    pub title: Option<String>,
    pub prompt: String,
    pub status: TaskStatus,
    pub current_attempt_id: Option<Uuid>,
    pub waiting_decision_count: i32,
    pub event_cursor: i64,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskDetail {
    #[serde(flatten)]
    pub task: Task,
    pub attempts: Vec<Attempt>,
    pub decisions: Vec<Decision>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    pub id: Uuid,
    pub task_id: Uuid,
    pub runner_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub agent_source: String,
    pub agent_session_id: Option<String>,
    pub ordinal: i32,
    pub reason: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub id: Uuid,
    pub task_id: Uuid,
    pub attempt_id: Uuid,
    pub kind: String,
    pub blocking: bool,
    pub schema_version: String,
    pub presentation: Value,
    pub status: String,
    pub response: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSelector {
    #[serde(default)]
    pub workspace_id: Option<Uuid>,
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentProfile {
    pub tool: AgentTool,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub permission_policy: Option<String>,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTool {
    Claude,
    Codex,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    pub prompt: String,
    pub workspace_selector: WorkspaceSelector,
    pub agent_profile: AgentProfile,
    #[serde(default)]
    pub metadata: std::collections::BTreeMap<String, String>,
}

impl CreateTaskRequest {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.prompt.trim().is_empty() {
            return Err("prompt must not be empty");
        }
        if self.prompt.len() > 200_000 {
            return Err("prompt exceeds 200000 bytes");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventProducer {
    #[serde(rename = "type")]
    pub producer_type: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskEvent {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub project_id: Uuid,
    pub task_id: Uuid,
    pub attempt_id: Option<Uuid>,
    pub sequence: i64,
    #[serde(rename = "type")]
    pub event_type: String,
    pub occurred_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
    pub producer: EventProducer,
    pub schema_version: String,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunnerEventInput {
    pub local_sequence: i64,
    pub dedupe_key: String,
    pub task_id: Uuid,
    pub attempt_id: Option<Uuid>,
    #[serde(rename = "type")]
    pub event_type: String,
    pub occurred_at: DateTime<Utc>,
    pub schema_version: String,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestEventsOutcome {
    pub accepted: usize,
    pub duplicates: usize,
    pub accepted_through_local_sequence: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestScope {
    pub organization_id: Uuid,
    pub project_id: Uuid,
}
