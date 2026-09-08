//! End-to-end coverage of the `prd-context` hook's dedupe: an unchanged
//! TASKS.md must cost exactly one injected copy, not one per prompt.
//!
//! The hook's contract is its stdout — JSON with `additionalContext` when the
//! reminder should enter, nothing at all when it should not. These tests drive
//! the real binary with the payload Claude Code sends, including the
//! `transcript_path` the dedupe reads.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn bin_path(name: &str) -> PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // drop test exe name
    if p.ends_with("deps") {
        p.pop();
    }
    p.push(name);
    p
}

/// Run the hook and return its raw stdout (empty string = stayed silent).
fn prd_context(home: &Path, ws: &Path, transcript: Option<&Path>) -> String {
    let mut child = Command::new(bin_path("fleet-cli"))
        .arg("prd-context")
        .env("FLEET_HOME", home)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn fleet prd-context");
    let mut payload = serde_json::json!({
        "session_id": "sess-prd-context-dedup",
        "cwd": ws.to_string_lossy(),
        "prompt": "继续",
    });
    if let Some(t) = transcript {
        payload["transcript_path"] = serde_json::json!(t.to_string_lossy());
    }
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait prd-context");
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn injected_text(stdout: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(stdout).expect("hook must emit JSON");
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext")
        .to_string()
}

fn write_tasks(ws: &Path, body: &str) {
    std::fs::write(
        ws.join("TASKS.md"),
        format!(
            "# TASKS\n\n\
             <!-- fleet:prd:begin id=\"dedupe-demo\" v=\"2\" -->\n\
             **Plan:** 去重演示\n\n{body}\n\
             <!-- fleet:prd:end id=\"dedupe-demo\" -->\n"
        ),
    )
    .unwrap();
}

/// Append the record Claude Code writes when it persists a hook's
/// `additionalContext`, so the next run sees the previous copy.
fn append_injected(transcript: &Path, text: &str) {
    use std::io::Write as _;
    let line = serde_json::json!({
        "type": "attachment",
        "isSidechain": false,
        "attachment": {"type": "hook_additional_context", "content": [text]},
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(transcript)
        .unwrap();
    writeln!(f, "{line}").unwrap();
}

#[test]
fn second_prompt_with_unchanged_tasks_injects_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let ws = home.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    write_tasks(&ws, "- [ ] **P1** — 第一个任务\n");
    let transcript = home.join("transcript.jsonl");
    std::fs::write(&transcript, "").unwrap();

    // Prompt 1: nothing in the transcript yet, so the reminder goes in.
    let first = prd_context(&home, &ws, Some(&transcript));
    let text = injected_text(&first);
    assert!(text.contains("## Plan: dedupe-demo"));
    append_injected(&transcript, &text);

    // Prompt 2: identical text is still in front of the model.
    assert_eq!(
        prd_context(&home, &ws, Some(&transcript)),
        "",
        "an unchanged TASKS.md must not be re-injected"
    );

    // Prompt 3: a ticked box changes the text, so it enters again.
    write_tasks(&ws, "- [x] **P1** — 第一个任务\n- [ ] **P2** — 第二个任务\n");
    let third = injected_text(&prd_context(&home, &ws, Some(&transcript)));
    assert!(
        third.contains("**P2**"),
        "a changed plan must re-enter, got: {third}"
    );
}

/// Older Claude Code builds (and hand-fed payloads) carry no `transcript_path`.
/// With no evidence to compare against, the hook keeps its old behaviour.
#[test]
fn missing_transcript_path_still_injects() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let ws = home.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    write_tasks(&ws, "- [ ] **P1** — 第一个任务\n");

    let text = injected_text(&prd_context(&home, &ws, None));
    assert!(text.contains("## Plan: dedupe-demo"));
}
