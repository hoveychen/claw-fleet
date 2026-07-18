use chrono::{TimeZone, Utc};
use fleet_cloud_wire::event::RunnerEvent;
use fleet_cloud_wire::runner::{ClientHello, RunnerCapability, RunnerFrame};
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
