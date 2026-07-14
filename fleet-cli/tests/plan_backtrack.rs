//! End-to-end coverage of child-plan → parent-plan backtracking.
//!
//! When an agent ticks the last box of a child plan (`parent="..."` on its
//! sentinel), `fleet plan check` must (1) print a directive pointing the agent
//! back at the nearest ancestor that still has pending work, and (2) re-attribute
//! this session's focus to that ancestor so the desktop card follows without the
//! agent running `fleet plan resume` itself. Top-level plans (no parent) and
//! non-final checks must NOT trigger any of this.
//!
//! Each test points `FLEET_HOME` at its own tempdir, so the task-progress
//! side-channel lands there instead of the developer's `~/.fleet`.

use std::path::PathBuf;
use std::process::Command;

fn bin_path(name: &str) -> PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // drop test exe name
    if p.ends_with("deps") {
        p.pop();
    }
    p.push(name);
    p
}

const SID: &str = "sess-plan-backtrack-test";

fn check(home: &std::path::Path, ws: &std::path::Path, plan: &str, task: &str) -> String {
    let out = Command::new(bin_path("fleet-cli"))
        .args(["plan", "check", plan, task])
        .current_dir(ws)
        .env("FLEET_HOME", home)
        .env("FLEET_SESSION_ID", SID)
        .output()
        .expect("run fleet plan check");
    assert!(
        out.status.success(),
        "fleet plan check failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The recorded focus record for `SID` under this test's home.
fn focus(home: &std::path::Path) -> serde_json::Value {
    let p = home
        .join(".fleet")
        .join("task-progress")
        .join(format!("{SID}.json"));
    let s = std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("read task-progress {}: {e}", p.display()));
    serde_json::from_str(&s).unwrap()
}

#[test]
fn completing_child_plan_backtracks_to_pending_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let ws = home.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        ws.join("TASKS.md"),
        "# TASKS\n\n\
         <!-- fleet:prd:begin id=\"parent-x\" v=\"2\" -->\n\
         **Plan:** 父计划\n\
         - [x] **P1** — 已完成\n\
         - [ ] **P3** — 父还没做完\n\
         <!-- fleet:prd:end id=\"parent-x\" -->\n\n\
         <!-- fleet:prd:begin id=\"child-y\" v=\"2\" parent=\"parent-x\" -->\n\
         **Plan:** 子支线\n\
         - [ ] **P1** — 子任务一\n\
         - [ ] **P2** — 子任务二\n\
         <!-- fleet:prd:end id=\"child-y\" -->\n",
    )
    .unwrap();

    // Non-final check: still pending P2, so no backtrack yet.
    let mid = check(&home, &ws, "child-y", "P1");
    assert!(
        !mid.contains("parent-x") && mid.trim() == "ok",
        "ticking a non-final box must stay a plain ok, got: {mid:?}"
    );

    // Final check: last box of the child → directive naming the parent + its
    // first pending task.
    let last = check(&home, &ws, "child-y", "P2");
    assert!(
        last.contains("parent-x") && last.contains("**P3**"),
        "completing the child must point the agent back at the parent's first \
         pending task, got: {last:?}"
    );

    // Focus must now be re-attributed to the parent, not left on the child.
    let rec = focus(&home);
    assert_eq!(
        rec["planId"].as_str(),
        Some("parent-x"),
        "session focus must follow the backtrack to the parent plan"
    );
    assert!(
        rec["currentTask"]
            .as_str()
            .map_or(false, |t| t.contains("**P3**")),
        "focus's current task must be the parent's first pending P, got: {:?}",
        rec["currentTask"]
    );
}

#[test]
fn completing_top_level_plan_does_not_backtrack() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let ws = home.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        ws.join("TASKS.md"),
        "# TASKS\n\n\
         <!-- fleet:prd:begin id=\"solo\" v=\"2\" -->\n\
         **Plan:** 无父顶层\n\
         - [ ] **P1** — 唯一任务\n\
         <!-- fleet:prd:end id=\"solo\" -->\n",
    )
    .unwrap();

    let out = check(&home, &ws, "solo", "P1");
    assert_eq!(
        out.trim(),
        "ok",
        "a top-level plan completing must not emit any backtrack directive, got: {out:?}"
    );
}
