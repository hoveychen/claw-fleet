mod in_memory;
mod postgres;

pub use in_memory::InMemoryTaskStore;
pub use postgres::PgTaskStore;

use crate::domain::{CreateTaskRequest, RequestScope, Task, TaskEvent};
use async_trait::async_trait;
use std::fmt;
use uuid::Uuid;

#[derive(Debug)]
pub enum StoreError {
    NotFound,
    Conflict { code: &'static str, detail: String },
    Internal(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "resource not found"),
            Self::Conflict { detail, .. } | Self::Internal(detail) => f.write_str(detail),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<sqlx::Error> for StoreError {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(error.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct CreateTaskCommand {
    pub scope: RequestScope,
    pub idempotency_key: String,
    pub request_fingerprint: [u8; 32],
    pub request: CreateTaskRequest,
}

#[derive(Debug, Clone)]
pub struct CreateTaskOutcome {
    pub task: Task,
    pub replayed: bool,
}

#[async_trait]
pub trait TaskStore: Send + Sync {
    async fn create_task(
        &self,
        command: CreateTaskCommand,
    ) -> Result<CreateTaskOutcome, StoreError>;

    async fn get_task(&self, scope: RequestScope, task_id: Uuid) -> Result<Task, StoreError>;

    async fn list_events(
        &self,
        scope: RequestScope,
        task_id: Uuid,
        after: i64,
    ) -> Result<Vec<TaskEvent>, StoreError>;
}
