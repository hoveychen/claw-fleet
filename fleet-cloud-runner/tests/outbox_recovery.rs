use chrono::{TimeZone, Utc};
use fleet_cloud_runner::outbox::EventOutbox;
use fleet_cloud_wire::event::RunnerEvent;
use serde_json::json;

fn event(number: u64) -> RunnerEvent {
    RunnerEvent {
        source_event_id: format!("source_{number}"),
        sequence: 0,
        event_type: "run.output".into(),
        occurred_at: Utc
            .with_ymd_and_hms(2026, 7, 18, 12, 0, 0)
            .single()
            .unwrap(),
        data: json!({"number":number}),
        schema_version: 1,
    }
}

#[test]
fn one_hundred_offline_events_replay_in_order_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runner.sqlite");
    {
        let outbox = EventOutbox::open(&path).unwrap();
        for number in 1..=100 {
            assert_eq!(outbox.append(event(number)).unwrap(), number);
        }
        assert_eq!(
            outbox.append(event(100)).unwrap(),
            100,
            "source ID deduplicates"
        );
    }
    let outbox = EventOutbox::open(&path).unwrap();
    assert_eq!(outbox.range().unwrap(), (Some(1), Some(100)));
    let replay = outbox.batch_after(0, 100).unwrap();
    assert_eq!(replay.len(), 100);
    assert!(replay
        .iter()
        .enumerate()
        .all(|(index, item)| item.sequence == index as u64 + 1));
    assert_eq!(outbox.acknowledge_through(60).unwrap(), 60);
    assert_eq!(outbox.range().unwrap(), (Some(61), Some(100)));
    assert_eq!(outbox.batch_after(60, 100).unwrap().len(), 40);
}
