//! The reviver's card round-trip against an isolated `FLEET_HOME`: a plan
//! whose last session the boss closed raises a card instead of a spawn, and
//! the answer lands as a snooze. Spawns nothing.
//!
//! One test per binary: it sets process-wide env.

use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn read_json(p: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

#[test]
fn boss_closed_plan_asks_first_and_the_answer_becomes_a_snooze() {
    let fleet_home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    // SAFETY: the only test in this binary; set before any Fleet call.
    unsafe {
        std::env::set_var("FLEET_HOME", fleet_home.path());
    }
    let ws_str = ws.path().to_string_lossy().to_string();
    std::fs::write(
        ws.path().join("TASKS.md"),
        "<!-- fleet:prd:begin id=\"toy\" v=\"2\" -->\n\n**Plan:** toy plan\n\n\
         - [ ] **P1** — eyeball it on a real phone\n\n\
         <!-- fleet:prd:end id=\"toy\" -->\n",
    )
    .unwrap();
    let fleet = fleet_home.path().join(".fleet");
    let owner = "00000000-c105-4000-8000-000000000000";
    for (dir, body) in [
        (
            "task-progress",
            serde_json::json!({"workspacePath": ws_str, "planId": "toy", "updated": now_ms() - 3600 * 1000}),
        ),
        (
            "task-outcome",
            serde_json::json!({"outcome": "completed", "workspacePath": ws_str, "cardId": "x", "updated": now_ms()}),
        ),
    ] {
        std::fs::create_dir_all(fleet.join(dir)).unwrap();
        std::fs::write(fleet.join(dir).join(format!("{owner}.json")), body.to_string()).unwrap();
    }

    // Start the clock, then backdate it past the grace window.
    claw_fleet_core::plan_revive::tick();
    let state_path = fleet.join("plan-revive-state.json");
    let mut state = read_json(&state_path);
    let key = state["plans"].as_object().unwrap().keys().next().unwrap().clone();
    state["plans"][&key]["orphanSinceMs"] = serde_json::json!(now_ms() - 31 * 60 * 1000);
    std::fs::write(&state_path, state.to_string()).unwrap();
    claw_fleet_core::plan_revive::tick();

    let state = read_json(&state_path);
    let st = &state["plans"][&key];
    assert!(st["revivedSessionId"].is_null(), "boss-closed plan must not spawn: {st}");
    assert_eq!(st["askKind"], "bossClosed");
    let card_id = st["askCardId"].as_str().unwrap().to_string();
    let card = claw_fleet_core::elicitation::read_request(&card_id).expect("card on disk");
    assert_eq!(card.session_id, owner);
    assert!(card.questions[0].question.contains("`toy`"));
    assert!(card.questions[0].question.contains("\n---\n"));

    // While the card is out, further passes neither re-ask nor spawn.
    claw_fleet_core::plan_revive::tick();
    assert_eq!(read_json(&state_path)["plans"][&key]["askCardId"], card_id.as_str());

    // The boss picks "静默 7 天".
    let question = card.questions[0].question.clone();
    std::fs::write(
        fleet.join("elicitation").join(format!("{card_id}.response.json")),
        serde_json::json!({"id": card_id, "declined": false, "answers": {question: "静默 7 天"}})
            .to_string(),
    )
    .unwrap();
    claw_fleet_core::plan_revive::tick();

    let state = read_json(&state_path);
    assert!(state["plans"][&key]["askCardId"].is_null());
    assert!(claw_fleet_core::elicitation::read_request(&card_id).is_none(), "card cleaned up");
    let snooze = claw_fleet_core::plan_snooze::active(&ws_str, "toy").expect("snoozed");
    assert_eq!(snooze.set_by, "fleet");
    let left = snooze.until_ms.unwrap() - now_ms();
    assert!(left > 6 * 24 * 3600 * 1000, "about a week: {left}");

    // And the report agrees.
    let report = claw_fleet_core::plan_revive::dry_run();
    assert!(report[0].verdict.starts_with("snoozed:"), "{:?}", report);
}
