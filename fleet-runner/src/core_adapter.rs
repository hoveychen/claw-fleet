use crate::{
    protocol::{CommandAck, CommandAckState},
    spool::{CommandClaim, Spool, SpoolError},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use thiserror::Error;
use uuid::Uuid;

pub const CLOUD_TASK_ENTRYPOINT: &str = "claw-fleet-cloud-task";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchTask {
    pub command_id: Uuid,
    pub task_id: Uuid,
    pub attempt_id: Uuid,
    pub workspace_id: Uuid,
    pub workspace_path: String,
    pub prompt: String,
    pub tool: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_policy: Option<String>,
    pub ordinal: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchResult {
    pub pid_ref: String,
    pub agent_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffSuccessor {
    pub session_id: String,
    pub note: String,
}

pub trait FleetCore: Send + Sync {
    fn spawn(&self, task: &LaunchTask) -> Result<LaunchResult, String>;
    fn handoff_successor(&self, session_id: &str) -> Option<HandoffSuccessor>;
}

pub struct RealFleetCore;

impl FleetCore for RealFleetCore {
    fn spawn(&self, task: &LaunchTask) -> Result<LaunchResult, String> {
        let response = claw_fleet_core::agent_source::spawn_session(
            &task.tool,
            &claw_fleet_core::agent_source::SpawnSpec {
                workspace_path: task.workspace_path.clone(),
                prompt: task.prompt.clone(),
                model: task.model.clone(),
                effort: task.effort.clone(),
                permission_mode: task.permission_policy.clone(),
                session_id: (task.tool == "claude").then(|| task.attempt_id.to_string()),
                entrypoint: CLOUD_TASK_ENTRYPOINT.into(),
            },
        )?;
        Ok(LaunchResult {
            pid_ref: format!("local:{}", response.pid),
            agent_session_id: response.session_id,
        })
    }

    fn handoff_successor(&self, session_id: &str) -> Option<HandoffSuccessor> {
        let chain = claw_fleet_core::handoff::chain_containing(session_id)?;
        chain
            .links
            .iter()
            .find(|link| link.from_session_id == session_id)
            .map(|link| HandoffSuccessor {
                session_id: link.to_session_id.clone(),
                note: link.note.clone(),
            })
    }
}

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error(transparent)]
    Spool(#[from] SpoolError),
    #[error("Fleet core launch failed: {0}")]
    Launch(String),
}

pub struct CoreAdapter<C: FleetCore> {
    core: Arc<C>,
    spool: Arc<Spool>,
}

impl<C: FleetCore> CoreAdapter<C> {
    pub fn new(core: Arc<C>, spool: Arc<Spool>) -> Self {
        Self { core, spool }
    }

    pub fn launch(&self, task: &LaunchTask) -> Result<CommandAck, AdapterError> {
        let payload = serde_json::to_value(task).expect("LaunchTask serialization cannot fail");
        match self.spool.claim_command(task.command_id, &payload)? {
            CommandClaim::AlreadyApplied { result } => Ok(CommandAck {
                command_id: task.command_id,
                state: CommandAckState::AlreadyApplied,
                result: Some(result),
                error: None,
            }),
            CommandClaim::Rejected { error } => Ok(CommandAck {
                command_id: task.command_id,
                state: CommandAckState::Rejected,
                result: None,
                error: Some(error),
            }),
            CommandClaim::InProgress => Ok(CommandAck {
                command_id: task.command_id,
                state: CommandAckState::Accepted,
                result: None,
                error: None,
            }),
            CommandClaim::New => self.launch_claimed(task),
        }
    }

    fn launch_claimed(&self, task: &LaunchTask) -> Result<CommandAck, AdapterError> {
        let result = match self.core.spawn(task) {
            Ok(result) => result,
            Err(error) => {
                self.spool.reject_command(task.command_id, &error)?;
                self.spool.append_event(
                    &format!("{}:attempt.failed", task.command_id),
                    task.task_id,
                    Some(task.attempt_id),
                    "attempt.ended",
                    "1.0",
                    &json!({ "reason": "launch_failed", "retryable": true, "error": error }),
                )?;
                return Err(AdapterError::Launch(error));
            }
        };
        let result_json = serde_json::to_value(&result).expect("LaunchResult cannot fail");
        self.spool.append_event(
            &format!("{}:attempt.started", task.command_id),
            task.task_id,
            Some(task.attempt_id),
            "attempt.started",
            "1.0",
            &json!({
                "attemptId": task.attempt_id,
                "workspaceId": task.workspace_id,
                "agentSource": task.tool,
                "agentSessionId": result.agent_session_id,
                "pidRef": result.pid_ref,
                "ordinal": task.ordinal,
                "reason": if task.ordinal == 1 { "initial" } else { "handoff" }
            }),
        )?;
        self.spool.complete_command(task.command_id, &result_json)?;
        Ok(CommandAck {
            command_id: task.command_id,
            state: CommandAckState::Accepted,
            result: Some(result_json),
            error: None,
        })
    }

    pub fn record_handoff(
        &self,
        task_id: Uuid,
        from_attempt_id: Uuid,
        from_session_id: &str,
        to_attempt_id: Uuid,
        next_ordinal: u32,
    ) -> Result<Option<HandoffSuccessor>, AdapterError> {
        let Some(successor) = self.core.handoff_successor(from_session_id) else {
            return Ok(None);
        };
        self.spool.append_event(
            &format!("handoff:{from_attempt_id}:ended"),
            task_id,
            Some(from_attempt_id),
            "attempt.ended",
            "1.0",
            &json!({
                "reason": "handoff",
                "successorAttemptId": to_attempt_id,
                "successorSessionId": successor.session_id
            }),
        )?;
        self.spool.append_event(
            &format!("handoff:{to_attempt_id}:started"),
            task_id,
            Some(to_attempt_id),
            "attempt.started",
            "1.0",
            &json!({
                "reason": "handoff",
                "ordinal": next_ordinal,
                "agentSessionId": successor.session_id,
                "predecessorAttemptId": from_attempt_id,
                "note": successor.note
            }),
        )?;
        Ok(Some(successor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    struct FakeCore {
        launches: AtomicUsize,
        successor: Option<HandoffSuccessor>,
    }

    impl FleetCore for FakeCore {
        fn spawn(&self, task: &LaunchTask) -> Result<LaunchResult, String> {
            self.launches.fetch_add(1, Ordering::SeqCst);
            Ok(LaunchResult {
                pid_ref: "fake:42".into(),
                agent_session_id: Some(format!("session-{}", task.attempt_id)),
            })
        }

        fn handoff_successor(&self, _session_id: &str) -> Option<HandoffSuccessor> {
            self.successor.clone()
        }
    }

    fn launch_task() -> LaunchTask {
        LaunchTask {
            command_id: Uuid::new_v4(),
            task_id: Uuid::new_v4(),
            attempt_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            workspace_path: "/workspace/fleet".into(),
            prompt: "continue the spike".into(),
            tool: "codex".into(),
            model: None,
            effort: Some("medium".into()),
            permission_policy: None,
            ordinal: 1,
        }
    }

    #[test]
    fn duplicate_launch_calls_real_core_once_and_replays_result() {
        let dir = tempdir().unwrap();
        let spool = Arc::new(Spool::open(dir.path().join("runner.db")).unwrap());
        let core = Arc::new(FakeCore {
            launches: AtomicUsize::new(0),
            successor: None,
        });
        let adapter = CoreAdapter::new(core.clone(), spool.clone());
        let task = launch_task();
        let first = adapter.launch(&task).unwrap();
        assert_eq!(first.state, CommandAckState::Accepted);
        for _ in 0..19 {
            let replay = adapter.launch(&task).unwrap();
            assert_eq!(replay.state, CommandAckState::AlreadyApplied);
            assert_eq!(replay.result, first.result);
        }
        assert_eq!(core.launches.load(Ordering::SeqCst), 1);
        let events = spool.pending_events(100).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "attempt.started");
        assert_eq!(events[0].task_id, task.task_id);
        assert_eq!(events[0].attempt_id, Some(task.attempt_id));
    }

    #[test]
    fn handoff_keeps_task_and_emits_two_ordered_attempts() {
        let dir = tempdir().unwrap();
        let spool = Arc::new(Spool::open(dir.path().join("runner.db")).unwrap());
        let core = Arc::new(FakeCore {
            launches: AtomicUsize::new(0),
            successor: Some(HandoffSuccessor {
                session_id: "session-next".into(),
                note: "P4 complete, continue P5".into(),
            }),
        });
        let adapter = CoreAdapter::new(core, spool.clone());
        let task_id = Uuid::new_v4();
        let from_attempt = Uuid::new_v4();
        let to_attempt = Uuid::new_v4();
        let successor = adapter
            .record_handoff(task_id, from_attempt, "session-old", to_attempt, 2)
            .unwrap()
            .unwrap();
        assert_eq!(successor.session_id, "session-next");
        let events = spool.pending_events(100).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].task_id, task_id);
        assert_eq!(events[0].attempt_id, Some(from_attempt));
        assert_eq!(events[0].event_type, "attempt.ended");
        assert_eq!(events[1].task_id, task_id);
        assert_eq!(events[1].attempt_id, Some(to_attempt));
        assert_eq!(events[1].event_type, "attempt.started");
        assert!(events[0].local_sequence < events[1].local_sequence);
    }
}
