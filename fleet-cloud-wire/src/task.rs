use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    WaitingInput,
    Paused,
    Succeeded,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn can_transition_to(self, next: Self) -> bool {
        use TaskStatus::*;
        matches!(
            (self, next),
            (Queued, Running | Paused | Failed | Cancelled)
                | (
                    Running,
                    WaitingInput | Paused | Succeeded | Failed | Cancelled
                )
                | (WaitingInput, Running | Paused | Failed | Cancelled)
                | (Paused, Queued | Cancelled)
                | (Failed, Queued)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Assigned,
    Starting,
    Running,
    WaitingInput,
    Stopping,
    Succeeded,
    Failed,
    Cancelled,
    Lost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProvider {
    ClaudeCode,
    Codex,
}

#[cfg(test)]
mod tests {
    use super::TaskStatus::*;

    #[test]
    fn task_status_allows_only_declared_edges() {
        let allowed = [
            (Queued, Running),
            (Queued, Paused),
            (Queued, Failed),
            (Queued, Cancelled),
            (Running, WaitingInput),
            (Running, Paused),
            (Running, Succeeded),
            (Running, Failed),
            (Running, Cancelled),
            (WaitingInput, Running),
            (WaitingInput, Paused),
            (WaitingInput, Failed),
            (WaitingInput, Cancelled),
            (Paused, Queued),
            (Paused, Cancelled),
            (Failed, Queued),
        ];
        for (from, to) in allowed {
            assert!(from.can_transition_to(to), "expected {from:?} -> {to:?}");
        }

        let forbidden = [
            (Succeeded, Running),
            (Succeeded, Queued),
            (Cancelled, Queued),
            (Cancelled, Running),
            (Paused, Succeeded),
            (WaitingInput, Succeeded),
            (Failed, Succeeded),
            (Running, Queued),
        ];
        for (from, to) in forbidden {
            assert!(!from.can_transition_to(to), "forbid {from:?} -> {to:?}");
        }
    }
}
