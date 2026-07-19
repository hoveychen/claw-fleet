use chrono::{TimeZone, Utc};
use fleet_cloud_wire::event::{DecisionCreated, DecisionKind, RunnerEvent};
use fleet_cloud_wire::runner::{
    ClientHello, CloudCommand, CommandAck, CommandAckStatus, RunnerCapability, RunnerFrame,
    ServerFrame, ServerHello,
};
use fleet_cloud_wire::RUNNER_PROTOCOL_VERSION;
use serde_json::json;

#[test]
fn client_hello_has_stable_tagged_json() {
    let frame = RunnerFrame::ClientHello(ClientHello {
        protocol_version: RUNNER_PROTOCOL_VERSION,
        runner_id: "runner_01JZ0A".into(),
        build_version: "abc1234".into(),
        platform: "linux".into(),
        architecture: "x86_64".into(),
        max_concurrency: 2,
        capabilities: vec![RunnerCapability {
            name: "claude_code".into(),
            version: 1,
        }],
        last_cloud_cursor: Some("019c123".into()),
        outbox_first_sequence: Some(41),
        outbox_last_sequence: Some(42),
    });

    let value = serde_json::to_value(&frame).expect("serialize hello");
    assert_eq!(value["type"], "client_hello");
    assert_eq!(value["payload"]["protocol_version"], 1);
    assert_eq!(
        serde_json::from_value::<RunnerFrame>(value).expect("deserialize hello"),
        frame
    );
}

#[test]
fn every_runner_frame_matches_fixed_json_and_ignores_unknown_fields() {
    let fixtures = [
        r#"{"type":"heartbeat","payload":{"active_runs":1,"future":true}}"#,
        r#"{"type":"command_ack","payload":{"command_id":"cmd_01JZ0A","assignment_sequence":9,"status":"completed","occurred_at":"2026-07-18T12:00:00Z","result":{"ok":true},"error_code":null}}"#,
    ];
    for fixture in fixtures {
        let frame: RunnerFrame = serde_json::from_str(fixture).expect("known Runner frame");
        let encoded = serde_json::to_value(frame).expect("re-encode Runner frame");
        assert!(encoded.get("type").is_some());
    }
    assert!(
        serde_json::from_str::<RunnerFrame>(r#"{"type":"future_frame","payload":{}}"#).is_err()
    );
}

#[test]
fn every_server_frame_matches_fixed_json() {
    let hello = ServerFrame::ServerHello(ServerHello {
        protocol_version: 1,
        heartbeat_interval_seconds: 15,
        config_version: 3,
        replay_commands_after: Some(8),
        request_outbox_from_sequence: Some(41),
    });
    assert_eq!(
        serde_json::to_string(&hello).unwrap(),
        r#"{"type":"server_hello","payload":{"protocol_version":1,"heartbeat_interval_seconds":15,"config_version":3,"replay_commands_after":8,"request_outbox_from_sequence":41}}"#
    );
    let command = ServerFrame::Command(CloudCommand {
        command_id: "cmd_01JZ0A".into(),
        assignment_sequence: 9,
        command_type: "start_run".into(),
        task_id: Some("task_01JZ0A".into()),
        run_id: Some("run_01JZ0A".into()),
        deadline: Utc
            .with_ymd_and_hms(2026, 7, 18, 12, 5, 0)
            .single()
            .unwrap(),
        expected_version: Some(1),
        required_capability: Some(RunnerCapability {
            name: "claude_code".into(),
            version: 1,
        }),
        payload: json!({"goal":"fix"}),
    });
    assert_eq!(
        serde_json::from_str::<ServerFrame>(&serde_json::to_string(&command).unwrap()).unwrap(),
        command
    );
    let ack = RunnerFrame::CommandAck(CommandAck {
        command_id: "cmd_01JZ0A".into(),
        assignment_sequence: 9,
        status: CommandAckStatus::Accepted,
        occurred_at: Utc
            .with_ymd_and_hms(2026, 7, 18, 12, 0, 1)
            .single()
            .unwrap(),
        result: None,
        error_code: None,
    });
    assert_eq!(
        serde_json::from_str::<RunnerFrame>(&serde_json::to_string(&ack).unwrap()).unwrap(),
        ack
    );
    assert!(
        serde_json::from_str::<ServerFrame>(r#"{"type":"future_frame","payload":{}}"#).is_err()
    );
}

#[test]
fn event_batch_round_trips_without_losing_data() {
    let frame = RunnerFrame::EventBatch {
        first_sequence: 7,
        events: vec![RunnerEvent {
            source_event_id: "source_7".into(),
            sequence: 7,
            event_type: "run.started".into(),
            occurred_at: Utc
                .with_ymd_and_hms(2026, 7, 18, 12, 0, 0)
                .single()
                .expect("valid timestamp"),
            data: json!({"task_id": "task_01JZ0A"}),
            schema_version: 1,
        }],
    };

    let encoded = serde_json::to_string(&frame).expect("serialize event batch");
    let decoded: RunnerFrame = serde_json::from_str(&encoded).expect("deserialize event batch");
    assert_eq!(decoded, frame);
}

#[test]
fn all_six_decision_kinds_round_trip_in_created_events() {
    let fixtures = [
        (DecisionKind::Guard, "guard"),
        (DecisionKind::Elicitation, "elicitation"),
        (DecisionKind::FleetAsk, "fleet_ask"),
        (DecisionKind::PlanApproval, "plan_approval"),
        (DecisionKind::PermissionPrompt, "permission_prompt"),
        (DecisionKind::A2ui, "a2ui"),
    ];

    for (kind, encoded_kind) in fixtures {
        let decision = DecisionCreated {
            source_decision_id: format!("local-{encoded_kind}"),
            kind,
            payload: json!({"sessionId":"session-1","fixture":encoded_kind}),
            response_schema: json!({"type":"object"}),
            deadline: None,
        };
        let value = serde_json::to_value(&decision).expect("encode Decision fixture");
        assert_eq!(value["kind"], encoded_kind);
        assert_eq!(
            serde_json::from_value::<DecisionCreated>(value).expect("decode Decision fixture"),
            decision
        );
    }
}
