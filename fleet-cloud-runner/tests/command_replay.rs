use chrono::{TimeZone, Utc};
use fleet_cloud_runner::journal::{CommandJournal, PersistResult};
use fleet_cloud_wire::runner::{CloudCommand, CommandAckStatus};
use serde_json::json;

fn command() -> CloudCommand {
    CloudCommand {
        command_id: "cmd_test_1".into(),
        assignment_sequence: 1,
        command_type: "start_run".into(),
        task_id: Some("task_test_1".into()),
        run_id: Some("run_test_1".into()),
        deadline: Utc
            .with_ymd_and_hms(2026, 7, 18, 13, 0, 0)
            .single()
            .unwrap(),
        expected_version: Some(1),
        required_capability: None,
        payload: json!({"goal":"fix"}),
    }
}

#[test]
fn persisted_before_ack_survives_crash_and_redelivery_is_not_reexecuted() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runner.sqlite");
    {
        let mut journal = CommandJournal::open(&path).unwrap();
        assert_eq!(
            journal.persist(&command()).unwrap(),
            PersistResult::Inserted
        );
        // Simulated crash before accepted ack: no terminal update.
    }
    let mut journal = CommandJournal::open(&path).unwrap();
    assert_eq!(journal.pending().unwrap(), vec![command()]);
    let mut executions = 0;
    if journal.persist(&command()).unwrap() == PersistResult::Inserted {
        executions += 1;
    }
    assert_eq!(executions, 0, "redelivery must not execute twice");
    journal
        .mark_terminal(
            "cmd_test_1",
            CommandAckStatus::Completed,
            Some(&json!({"ok":true})),
        )
        .unwrap();
    journal
        .mark_terminal(
            "cmd_test_1",
            CommandAckStatus::Completed,
            Some(&json!({"ok":true})),
        )
        .unwrap();
    assert!(journal.pending().unwrap().is_empty());
}
