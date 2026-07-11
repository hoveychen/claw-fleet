//! End-to-end coverage of which workspace `fleet handoff` hands the successor.
//!
//! Regression: the successor is spawned in the pending record's `workspacePath`
//! (`handoff::consume_and_spawn_in` → `spawn(&pending.workspace_path, …)`), and
//! that path used to come from `std::env::current_dir()` — the agent's *Bash*
//! cwd, which under the Rule-3 worktree workflow has been `cd`-ed into
//! `<repo>/.worktrees/<task>`. So a session living in the main checkout handed
//! its successor a worktree cwd; the successor then finished the plan, ran
//! `git worktree remove`, and deleted its own cwd — after which the session
//! scan hides it (workspace dir gone) and its decision cards lose their
//! workspace label and session detail.
//!
//! The successor must inherit the *session's* cwd (from its transcript), not
//! the agent's Bash cwd.
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

const SID: &str = "sess-handoff-cwd-test";

/// Claude Code's projects-dir encoding: `/`, `.` and `_` all collapse to `-`.
fn claude_encode(p: &Path) -> String {
    p.to_string_lossy()
        .chars()
        .map(|c| if c == '/' || c == '.' || c == '_' { '-' } else { c })
        .collect()
}

/// Write a transcript for `SID` whose records carry `cwd` — i.e. the directory
/// Claude Code itself was launched in.
fn write_transcript(fleet_home: &Path, session_cwd: &Path) {
    let projects = fleet_home
        .join(".claude")
        .join("projects")
        .join(claude_encode(session_cwd));
    std::fs::create_dir_all(&projects).unwrap();
    let line = serde_json::json!({
        "type": "user",
        "cwd": session_cwd.to_string_lossy(),
        "message": {"role": "user", "content": "hi"},
        "timestamp": "2026-07-11T00:00:00.000Z",
    });
    std::fs::write(
        projects.join(format!("{SID}.jsonl")),
        format!("{line}\n"),
    )
    .unwrap();
}

/// The pending handoff record `fleet handoff` just wrote.
fn pending(fleet_home: &Path) -> serde_json::Value {
    let p = fleet_home
        .join(".fleet")
        .join("handoffs")
        .join("pending")
        .join(format!("{SID}.json"));
    let s = std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("read pending handoff {}: {e}", p.display()));
    serde_json::from_str(&s).unwrap()
}

/// An agent working the Rule-3 worktree workflow leaves its Bash cwd inside
/// `<repo>/.worktrees/<task>` and registers the handoff from there. The
/// successor must still be handed the repo the *session* runs in.
#[test]
fn handoff_from_worktree_bash_cwd_hands_successor_the_session_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();

    let repo = home.join("repo");
    let worktree = repo.join(".worktrees").join("feat");
    std::fs::create_dir_all(&worktree).unwrap();

    // The session itself runs in the main checkout.
    write_transcript(&home, &repo);

    // …but the agent's Bash cwd is the worktree when it calls `fleet handoff`.
    let out = Command::new(bin_path("fleet-cli"))
        .args(["handoff", "--note", "P1-P3 done, continue with P4"])
        .current_dir(&worktree)
        .env("FLEET_HOME", &home)
        .env("FLEET_SESSION_ID", SID)
        .output()
        .expect("run fleet handoff");
    assert!(
        out.status.success(),
        "fleet handoff failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let rec = pending(&home);
    let ws = rec["workspacePath"].as_str().unwrap();
    assert_eq!(
        Path::new(ws),
        repo.as_path(),
        "successor must be handed the session's cwd (the main checkout), not the \
         agent's Bash cwd inside the worktree — a worktree the successor is \
         expected to delete when it merges the plan. got: {ws}"
    );

    // The worktree is still where the work lives, so it must not be lost — the
    // successor is told about it rather than dropped into it.
    assert_eq!(
        rec["predecessorCwd"].as_str().map(Path::new),
        Some(worktree.as_path()),
        "the worktree the predecessor was working in must be recorded so the \
         successor can `cd` into it"
    );
}

/// No worktree in play: Bash cwd == session cwd, nothing to rewrite.
#[test]
fn handoff_from_session_cwd_is_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();

    let repo = home.join("plain-repo");
    std::fs::create_dir_all(&repo).unwrap();
    write_transcript(&home, &repo);

    let out = Command::new(bin_path("fleet-cli"))
        .args(["handoff", "--note", "keep going"])
        .current_dir(&repo)
        .env("FLEET_HOME", &home)
        .env("FLEET_SESSION_ID", SID)
        .output()
        .expect("run fleet handoff");
    assert!(
        out.status.success(),
        "fleet handoff failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let rec = pending(&home);
    assert_eq!(Path::new(rec["workspacePath"].as_str().unwrap()), repo.as_path());
    assert!(
        rec["predecessorCwd"].is_null(),
        "no shell-cwd hint when the agent never left the session cwd: {rec:?}"
    );
}
