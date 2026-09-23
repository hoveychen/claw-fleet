//! Live end-to-end check of the plan reviver: one real pass against an
//! isolated `FLEET_HOME`, with a stranded toy plan, spawning a real `claude`.
//!
//! Ignored by default — it launches a real agent session (costs tokens, needs
//! a logged-in `claude`). Run by hand:
//!
//! ```sh
//! cargo test -p claw-fleet-core --test plan_revive_live -- --ignored --nocapture
//! ```

use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[test]
#[ignore]
fn one_pass_wakes_and_attributes_a_session_for_a_stranded_plan() {
    let real_home = std::env::var("HOME").unwrap();
    let fleet_home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    // Keep both dirs after the test so the spawned session (which outlives us)
    // still has its workspace, and the state can be inspected.
    let fleet_home = fleet_home.keep();
    let ws = ws.keep();
    // SAFETY: single-threaded test binary, set before any Fleet call.
    unsafe {
        std::env::set_var("FLEET_HOME", &fleet_home);
        std::env::set_var("FLEET_AGENT_HOME", &real_home);
    }

    let ws_str = ws.to_string_lossy().to_string();
    std::fs::write(
        ws.join("TASKS.md"),
        "<!-- fleet:prd:begin id=\"toy\" v=\"2\" -->\n\n**Plan:** toy plan\n\n\
         - [x] **P1** — create the folder\n\
         - [ ] **P2** — write the word hi into hello.txt\n\n\
         <!-- fleet:prd:end id=\"toy\" -->\n",
    )
    .unwrap();

    let fleet = fleet_home.join(".fleet");
    std::fs::create_dir_all(fleet.join("task-progress")).unwrap();
    let dead_owner = "00000000-dead-4000-8000-000000000000";
    std::fs::write(
        fleet.join("task-progress").join(format!("{dead_owner}.json")),
        serde_json::json!({
            "workspacePath": ws_str,
            "planId": "toy",
            "currentTask": "**P2** — write the word hi into hello.txt",
            "updated": now_ms() - 3600 * 1000,
        })
        .to_string(),
    )
    .unwrap();

    // First pass only starts the orphan clock.
    claw_fleet_core::plan_revive::tick();
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fleet.join("plan-revive-state.json")).unwrap())
            .unwrap();
    let (key, st) = state["plans"].as_object().unwrap().iter().next().unwrap();
    assert!(st["orphanSinceMs"].is_u64(), "orphan clock started: {st}");
    assert!(st["revivedSessionId"].is_null(), "no spawn inside the grace window");

    // Backdate the clock past the grace window; the next pass must spawn.
    let mut state = state.clone();
    state["plans"][key]["orphanSinceMs"] = serde_json::json!(now_ms() - 31 * 60 * 1000);
    std::fs::write(fleet.join("plan-revive-state.json"), state.to_string()).unwrap();
    claw_fleet_core::plan_revive::tick();

    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fleet.join("plan-revive-state.json")).unwrap())
            .unwrap();
    let st = &state["plans"][key];
    let revived = st["revivedSessionId"].as_str().expect("a session was spawned").to_string();
    assert_eq!(st["attempts"], 1);
    let attributed: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fleet.join("task-progress").join(format!("{revived}.json")))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(attributed["planId"], "toy");
    println!("revived session {revived} in {ws_str} (FLEET_HOME={})", fleet_home.display());

    // A third pass right away must not spawn a second session: the new one is
    // alive (or at worst restarts the orphan clock).
    std::thread::sleep(std::time::Duration::from_secs(5));
    claw_fleet_core::plan_revive::tick();
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fleet.join("plan-revive-state.json")).unwrap())
            .unwrap();
    assert_eq!(state["plans"][key]["revivedSessionId"], revived.as_str());
    assert_eq!(state["plans"][key]["attempts"], 1);
}
