use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use claw_fleet_core::agent_source::SpawnSpec;
use claw_fleet_core::backend::HarnessEvent;
use fleet_cloud_runner::outbox::EventOutbox;
use fleet_cloud_runner::supervisor::{Harness, HarnessSnapshot, PendingDecision, Supervisor};
use fleet_cloud_wire::runner::{CloudCommand, CommandAckStatus};
use serde_json::{json, Value};

static HARNESS_SINK_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Default)]
struct FakeHarness {
    launches: Mutex<Vec<(String, SpawnSpec)>>,
    terminated: Mutex<Vec<u32>>,
    queued: Mutex<Vec<(String, String)>>,
    snapshots: Mutex<HashMap<String, HarnessSnapshot>>,
    pending_decisions: Mutex<Vec<PendingDecision>>,
    decision_responses: Mutex<Vec<(String, String, Value)>>,
}

impl Harness for FakeHarness {
    fn spawn(
        &self,
        provider: &str,
        spec: &SpawnSpec,
    ) -> Result<claw_fleet_core::session_launch::SpawnSessionResponse, String> {
        let mut launches = self.launches.lock().unwrap();
        let number = launches.len() + 1;
        let session_id = format!("fixture-{provider}-{number}");
        launches.push((provider.into(), spec.clone()));
        self.snapshots.lock().unwrap().insert(
            session_id.clone(),
            HarnessSnapshot {
                messages: vec![
                    json!({
                        "type":"user",
                        "cwd":"/private/customer/repo",
                        "message":{
                            "content":"token ghp_abcdefghijklmnopqrstuvwxyz1234567890",
                            "headers":{"Authorization":"Bearer transcript-auth-secret"}
                        }
                    }),
                    json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"tool-1","name":"Read","input":{"file":"safe.txt"}}]}}),
                    json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"ok"}]}}),
                ],
                input_tokens: 120,
                output_tokens: 40,
                cost_usd: 0.25,
            },
        );
        Ok(claw_fleet_core::session_launch::SpawnSessionResponse {
            pid: 10_000 + number as u32,
            session_id: Some(session_id),
        })
    }

    fn terminate(&self, pid: u32) -> Result<(), String> {
        self.terminated.lock().unwrap().push(pid);
        Ok(())
    }

    fn enqueue(&self, session_id: &str, _workspace: &str, text: &str) -> Result<(), String> {
        self.queued
            .lock()
            .unwrap()
            .push((session_id.into(), text.into()));
        Ok(())
    }

    fn snapshot(&self, session_id: &str) -> Result<Option<HarnessSnapshot>, String> {
        Ok(self.snapshots.lock().unwrap().get(session_id).cloned())
    }

    fn pending_decisions(&self) -> Result<Vec<PendingDecision>, String> {
        Ok(self.pending_decisions.lock().unwrap().clone())
    }

    fn respond_decision(&self, kind: &str, id: &str, response: &Value) -> Result<(), String> {
        self.decision_responses
            .lock()
            .unwrap()
            .push((kind.into(), id.into(), response.clone()));
        Ok(())
    }
}

fn command(
    sequence: u64,
    command_type: &str,
    task_id: &str,
    run_id: Option<&str>,
    payload: Value,
) -> CloudCommand {
    CloudCommand {
        command_id: format!("cmd-{sequence}"),
        assignment_sequence: sequence,
        command_type: command_type.into(),
        task_id: Some(task_id.into()),
        run_id: run_id.map(str::to_owned),
        deadline: Utc::now() + Duration::minutes(5),
        expected_version: None,
        required_capability: None,
        payload,
    }
}

fn start_payload(repository: &str, provider: &str) -> Value {
    json!({
        "workspace":{"repository":repository,"ref":"main"},
        "agent":{"provider":provider,"model":"test-model","effort":"high","permission_policy_id":"policy-safe"},
        "goal":"implement the requested change"
    })
}

fn exit_event(task_id: &str, run_id: &str, session_id: &str, provider: &str) -> HarnessEvent {
    HarnessEvent {
        event_type: "run.process_exited".into(),
        task_id: Some(task_id.into()),
        run_id: Some(run_id.into()),
        provider_session_ref: Some(session_id.into()),
        data: json!({"provider":provider,"success":true}),
    }
}

#[test]
fn supervisor_bridges_providers_handoff_controls_and_safe_cloud_events() {
    let _sink_lock = HARNESS_SINK_TEST_LOCK.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let repository = directory.path().join("repo");
    std::fs::create_dir_all(&repository).unwrap();
    std::fs::write(repository.join("README.md"), "fixture").unwrap();
    let state = directory.path().join("state");
    let outbox = Arc::new(EventOutbox::open(&directory.path().join("outbox.sqlite")).unwrap());
    let harness = Arc::new(FakeHarness::default());
    let mut supervisor =
        Supervisor::open_with_harness(&state, outbox.clone(), harness.clone()).unwrap();
    let repository = repository.to_string_lossy();

    for (sequence, provider) in [(1, "claude_code"), (2, "codex")] {
        let task_id = format!("task-{provider}");
        let run_id = format!("run-{provider}");
        let result = supervisor.execute(&command(
            sequence,
            "start_run",
            &task_id,
            Some(&run_id),
            start_payload(&repository, provider),
        ));
        assert_eq!(result.status, CommandAckStatus::Completed, "{result:?}");
    }
    let launches = harness.launches.lock().unwrap().clone();
    assert_eq!(launches.len(), 2);
    assert_eq!(launches[0].0, "claude_code");
    assert_eq!(launches[1].0, "codex");
    for (_, spec) in &launches {
        assert!(spec.workspace_path.starts_with(state.to_str().unwrap()));
        assert!(spec
            .environment
            .iter()
            .any(|(key, _)| key == "FLEET_CLOUD_TASK_ID"));
        assert!(spec
            .environment
            .iter()
            .any(|(key, _)| key == "FLEET_CLOUD_RUN_ID"));
    }

    claw_fleet_core::backend::emit_harness_event(exit_event(
        "task-claude_code",
        "run-claude_code",
        "fixture-claude_code-1",
        "claude_code",
    ));
    claw_fleet_core::backend::emit_harness_event(exit_event(
        "task-codex",
        "run-codex",
        "fixture-codex-2",
        "codex",
    ));
    supervisor.reconcile().unwrap();

    let handoff_start = supervisor.execute(&command(
        3,
        "start_run",
        "task-handoff",
        Some("run-handoff-1"),
        start_payload(&repository, "claude_code"),
    ));
    assert_eq!(handoff_start.status, CommandAckStatus::Completed);
    claw_fleet_core::backend::emit_harness_event(HarnessEvent {
        event_type: "run.handoff_created".into(),
        task_id: Some("task-handoff".into()),
        run_id: Some("run-handoff-2".into()),
        provider_session_ref: None,
        data: json!({"predecessor_run_id":"run-handoff-1","provider":"claude_code"}),
    });
    claw_fleet_core::backend::emit_harness_event(exit_event(
        "task-handoff",
        "run-handoff-1",
        "fixture-claude_code-3",
        "claude_code",
    ));
    supervisor.reconcile().unwrap();
    let before_successor = outbox.batch_after(0, 500).unwrap();
    assert!(!before_successor.iter().any(|event| {
        event.event_type == "task.completed" && event.data["task_id"] == "task-handoff"
    }));
    claw_fleet_core::backend::emit_harness_event(exit_event(
        "task-handoff",
        "run-handoff-2",
        "fixture-successor",
        "claude_code",
    ));
    supervisor.reconcile().unwrap();

    let control_start = supervisor.execute(&command(
        4,
        "start_run",
        "task-control",
        Some("run-control-1"),
        start_payload(&repository, "claude_code"),
    ));
    assert_eq!(control_start.status, CommandAckStatus::Completed);
    assert_eq!(
        supervisor
            .execute(&command(
                5,
                "pause_task",
                "task-control",
                Some("run-control-1"),
                json!({})
            ))
            .status,
        CommandAckStatus::Completed
    );
    let paused_append = supervisor.execute(&command(
        6,
        "append_message",
        "task-control",
        Some("run-control-1"),
        json!({"text":"queued while paused"}),
    ));
    assert_eq!(paused_append.result.unwrap()["reason"], "paused");
    assert!(harness.queued.lock().unwrap().is_empty());
    let resumed = supervisor.execute(&command(
        7,
        "resume_task",
        "task-control",
        Some("run-control-1"),
        json!({"message":"resume now"}),
    ));
    assert_eq!(resumed.status, CommandAckStatus::Completed);
    let resumed_run = resumed.result.unwrap()["run_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(resumed_run, "run-control-1");
    let resumed_spec = harness.launches.lock().unwrap().last().unwrap().1.clone();
    assert_eq!(resumed_spec.model.as_deref(), Some("test-model"));
    assert_eq!(resumed_spec.effort.as_deref(), Some("high"));
    let running_append = supervisor.execute(&command(
        8,
        "append_message",
        "task-control",
        Some(&resumed_run),
        json!({"text":"next message"}),
    ));
    assert_eq!(running_append.result.unwrap()["reason"], "running");
    assert_eq!(harness.queued.lock().unwrap().len(), 1);
    let cancelled = supervisor.execute(&command(
        9,
        "cancel_task",
        "task-control",
        Some(&resumed_run),
        json!({}),
    ));
    assert_eq!(cancelled.status, CommandAckStatus::Completed);
    assert_eq!(harness.terminated.lock().unwrap().len(), 1);

    let events = outbox.batch_after(0, 500).unwrap();
    for required in [
        "run.started",
        "message.created",
        "tool.started",
        "tool.finished",
        "usage.updated",
        "run.finished",
        "task.completed",
        "run.assigned",
    ] {
        assert!(
            events.iter().any(|event| event.event_type == required),
            "missing {required}"
        );
    }
    let encoded = serde_json::to_string(&events).unwrap();
    assert!(!encoded.contains("\"pid\""));
    assert!(!encoded.contains("jsonl_path"));
    assert!(!encoded.contains("workspace_path"));
    assert!(!encoded.contains("/private/customer/repo"));
    assert!(!encoded.contains("transcript-auth-secret"));
    assert!(!encoded.contains("ghp_abcdefghijklmnopqrstuvwxyz1234567890"));
    assert!(encoded.contains("[REDACTED:authorization]"));
    assert!(encoded.contains("[REDACTED:github_token]"));
    assert!(events.iter().any(|event| {
        event.event_type == "message.created"
            && event.data["redactions"]["authorization"] == 1
            && event.data["redactions"]["github_token"] == 1
    }));
    assert!(!encoded.contains(state.to_str().unwrap()));
}

#[test]
fn supervisor_scans_six_decisions_and_replays_delivery_idempotently_after_restart() {
    let _sink_lock = HARNESS_SINK_TEST_LOCK.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let repository = directory.path().join("repo");
    std::fs::create_dir_all(&repository).unwrap();
    std::fs::write(repository.join("README.md"), "fixture").unwrap();
    let state = directory.path().join("state");
    let outbox = Arc::new(EventOutbox::open(&directory.path().join("outbox.sqlite")).unwrap());
    let harness = Arc::new(FakeHarness::default());
    let session_id = "fixture-claude_code-1";
    let fixtures = [
        ("guard", json!({"allow":true})),
        ("elicitation", json!({"answers":{"Question":"Yes"}})),
        ("fleet_ask", json!({"answers":{"Question":"Yes"}})),
        ("plan_approval", json!({"decision":"approve"})),
        (
            "a2ui",
            json!({"actionName":"submit","actionContext":{"x":"1"}}),
        ),
        ("permission_prompt", json!({"allow":true})),
    ];

    let mut supervisor =
        Supervisor::open_with_harness(&state, outbox.clone(), harness.clone()).unwrap();
    assert_eq!(
        supervisor
            .execute(&command(
                1,
                "start_run",
                "task-decisions",
                Some("run-decisions"),
                start_payload(repository.to_str().unwrap(), "claude_code"),
            ))
            .status,
        CommandAckStatus::Completed
    );
    *harness.pending_decisions.lock().unwrap() = fixtures
        .iter()
        .enumerate()
        .map(|(index, (kind, _))| PendingDecision {
            id: format!("source-{index}"),
            session_id: session_id.into(),
            kind: (*kind).into(),
            request: json!({"id":format!("source-{index}"),"sessionId":session_id,"kind":kind}),
        })
        .collect();
    supervisor.reconcile().unwrap();

    let created = outbox
        .batch_after(0, 100)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "decision.created")
        .collect::<Vec<_>>();
    assert_eq!(created.len(), 6);
    let encoded_kinds = created
        .iter()
        .map(|event| event.data["decision"]["kind"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        encoded_kinds,
        vec![
            "guard",
            "elicitation",
            "fleet_ask",
            "plan_approval",
            "a2ui",
            "permission_prompt"
        ]
    );

    for (index, (_, response)) in fixtures.iter().enumerate() {
        let result = supervisor.execute(&command(
            index as u64 + 2,
            "resolve_decision",
            "task-decisions",
            Some("run-decisions"),
            json!({
                "decision_id":format!("cloud-{index}"),
                "source_decision_id":format!("source-{index}"),
                "response":response,
                "terminal_status":"answered"
            }),
        ));
        assert_eq!(result.status, CommandAckStatus::Completed, "{result:?}");
    }
    assert_eq!(harness.decision_responses.lock().unwrap().len(), 6);
    drop(supervisor);

    let mut restarted =
        Supervisor::open_with_harness(&state, outbox.clone(), harness.clone()).unwrap();
    let replay = restarted.execute(&command(
        20,
        "resolve_decision",
        "task-decisions",
        Some("run-decisions"),
        json!({
            "decision_id":"cloud-0",
            "source_decision_id":"source-0",
            "response":{"allow":true},
            "terminal_status":"answered"
        }),
    ));
    assert_eq!(replay.status, CommandAckStatus::Completed);
    assert_eq!(harness.decision_responses.lock().unwrap().len(), 6);
    let delivered = outbox
        .batch_after(0, 100)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "decision.delivered")
        .collect::<Vec<_>>();
    assert_eq!(delivered.len(), 6);
}
