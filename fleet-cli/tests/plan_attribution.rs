//! End-to-end coverage of the `fleet plan` → `task_progress` attribution
//! contract, driving the real binary so the whole path (arg parsing → TASKS.md
//! mutation → side-channel write) is exercised.
//!
//! Attribution is what lets a session card name the plan the session is working
//! on. `summarize_workspace_tasks` renders nothing without it, so a subcommand
//! that *should* claim focus but doesn't leaves the card permanently blank.
//!
//! Each test points `FLEET_HOME` at its own tempdir, so records land there
//! instead of the developer's `~/.fleet`.

use std::path::{Path, PathBuf};
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

const SID: &str = "sess-attribution-test";

/// Run `fleet plan <args...>` in `cwd` with records rooted at `fleet_home`.
///
/// Drives `fleet-cli`, the real binary. (`target/debug/fleet` is an unrelated
/// 39-byte sidecar placeholder written by claw-fleet-desktop's build.rs; running
/// it silently exits 0 and does nothing.)
fn plan(fleet_home: &Path, cwd: &Path, args: &[&str]) -> std::process::Output {
    let out = Command::new(bin_path("fleet-cli"))
        .arg("plan")
        .args(args)
        .current_dir(cwd)
        .env("FLEET_HOME", fleet_home)
        // A Claude-session ancestor's CLAUDE_CODE_SESSION_ID outranks
        // FLEET_SESSION_ID in resolve_fleet_session_id_from_env; strip it so
        // attribution lands under SID when the suite runs inside a session.
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env("FLEET_SESSION_ID", SID)
        .output()
        .expect("run fleet plan");
    assert!(
        out.status.success(),
        "fleet plan {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// The session's focus record, if the subcommand claimed attribution.
fn focus(fleet_home: &Path) -> Option<serde_json::Value> {
    let p = fleet_home
        .join(".fleet")
        .join("task-progress")
        .join(format!("{SID}.json"));
    let s = std::fs::read_to_string(p).ok()?;
    serde_json::from_str(&s).ok()
}

/// Creating a plan IS starting it — the agent that writes the plan is the agent
/// about to execute it, so `create` claims focus without a separate ceremony.
#[test]
fn create_claims_focus() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();

    plan(home.path(), ws.path(), &["create", "alpha", "--title", "Alpha", "--root"]);

    let rec = focus(home.path()).expect("create must claim focus");
    assert_eq!(rec["planId"], "alpha");
    // A freshly created plan has no tasks yet, so there is no current task.
    assert!(rec.get("currentTask").is_none(), "unexpected: {rec}");
}

/// `add` edits a plan's structure; it is not a claim to be executing that plan.
/// A master appending a P-item to someone else's plan must not repoint its own
/// card at it.
#[test]
fn add_does_not_claim_focus() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();

    // `create` claims focus on alpha; the second create re-points it to beta so
    // `add`'s effect is isolated. Beta takes no `--root`: authored while alpha is
    // in flight, it becomes alpha's child by default — which is irrelevant here,
    // but passing `--root` would now require a justification.
    plan(home.path(), ws.path(), &["create", "alpha", "--title", "Alpha", "--root"]);
    plan(home.path(), ws.path(), &["create", "beta", "--title", "Beta"]);
    let rec = focus(home.path()).expect("create claimed focus");
    assert_eq!(rec["planId"], "beta", "second create re-points focus");

    // Appending to alpha must leave focus on beta.
    plan(home.path(), ws.path(), &["add", "alpha", "P1", "--text", "a1"]);
    let rec = focus(home.path()).expect("focus record still present");
    assert_eq!(rec["planId"], "beta", "add must not steal focus");
}

/// The tree-position default, end-to-end through the real binary: a plan
/// authored while the session is executing another one becomes its child, with
/// no flag passed. Leaving that tree still works but has to be justified.
///
/// This is the mechanism that keeps a decomposed goal running to completion — a
/// relay chain in `agent-workspace` produced 8 sibling roots with 0 `parent=`
/// from a single boss request, so every hop finished its own plan, found no
/// ancestor to walk back to, and stopped with the goal unfinished. Only an
/// end-to-end test catches a regression here: the default reads the focus record
/// the *previous* invocation wrote, which a unit test stubs out.
#[test]
fn create_inherits_the_focused_plan_as_parent() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let tasks = ws.path().join("TASKS.md");

    // Nothing in flight ⇒ a fresh topic, no flag needed and no parent inherited.
    plan(home.path(), ws.path(), &["create", "goal", "--title", "Goal"]);
    let body = std::fs::read_to_string(&tasks).unwrap();
    assert!(
        body.contains(r#"id="goal" v="2" -->"#),
        "first plan must be a root: {body}"
    );

    // Authored while 'goal' is in flight ⇒ child of goal, no flag passed.
    plan(home.path(), ws.path(), &["create", "item1", "--title", "Item 1"]);
    let body = std::fs::read_to_string(&tasks).unwrap();
    assert!(
        body.contains(r#"id="item1" v="2" parent="goal""#),
        "spawned plan must inherit the focused plan as parent: {body}"
    );

    // Opting out is refused without a reason…
    let out = Command::new(bin_path("fleet-cli"))
        .args(["plan", "create", "escape", "--title", "Escape", "--root"])
        .current_dir(ws.path())
        .env("FLEET_HOME", home.path())
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env("FLEET_SESSION_ID", SID)
        .output()
        .expect("run fleet plan");
    assert!(!out.status.success(), "a bare --root mid-plan must be refused");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--root-reason"), "{err}");
    assert!(
        !std::fs::read_to_string(&tasks).unwrap().contains("escape"),
        "a refused create must not write"
    );

    // …and allowed with one.
    plan(
        home.path(),
        ws.path(),
        &["create", "escape", "--title", "Escape", "--root", "--root-reason", "另一条产品线,与 goal 无关"],
    );
    let body = std::fs::read_to_string(&tasks).unwrap();
    assert!(
        body.contains(r#"id="escape" v="2" -->"#),
        "a justified root must be written without a parent: {body}"
    );
}

/// `check` both ticks the box and refreshes focus onto that plan.
#[test]
fn check_claims_focus_and_advances_current_task() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();

    plan(home.path(), ws.path(), &["create", "alpha", "--title", "Alpha", "--root"]);
    plan(home.path(), ws.path(), &["add", "alpha", "P1", "--text", "first"]);
    plan(home.path(), ws.path(), &["add", "alpha", "P2", "--text", "second"]);
    plan(home.path(), ws.path(), &["check", "alpha", "P1"]);

    let rec = focus(home.path()).expect("check must claim focus");
    assert_eq!(rec["planId"], "alpha");
    // P1 is done, so the current task advances to P2.
    let cur = rec["currentTask"].as_str().unwrap_or_default();
    assert!(cur.contains("**P2**"), "current task should be P2, got {cur:?}");
}
