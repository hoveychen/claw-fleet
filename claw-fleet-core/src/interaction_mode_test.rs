//! Active diagnostic tests for the QA interaction mode pipeline.
//!
//! The diagnostics view exposes three buttons of increasing depth:
//!
//!   1. **Frontend-only** — pure Tauri-app concern (emit a synthetic event
//!      straight to the local React listener); lives in `gui.rs`, not here.
//!   2. **End-to-end** — write a fake `ElicitationRequest` to
//!      `~/.fleet/elicitation/<uuid>.json` so the existing watcher loop
//!      picks it up exactly like a real one. `run_end_to_end_test` below.
//!   3. **Claude CLI** — actually spawn `claude -p "<prompt>"` so the model
//!      itself exercises the `AskUserQuestion` injection from CLAUDE.md.
//!      `run_claude_cli_test` below.
//!
//! Test (2) and (3) live in core so RemoteBackend can drive them on the
//! probe server (where the real `~/.fleet/elicitation/` dir and the
//! configured Claude binary live).

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use uuid::Uuid;

use crate::elicitation::{ElicitationOption, ElicitationQuestion, ElicitationRequest};

/// Marker used in workspace_name and ai_title so the frontend can tag the
/// test card visually and so a forgotten test request is obvious in logs.
pub const TEST_WORKSPACE_MARKER: &str = "[QA Diagnostic Test]";

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TestRunResult {
    /// One of "frontend_only" / "end_to_end" / "claude_cli".
    pub kind: String,
    /// Id of the elicitation request placed on disk, if any. Frontend
    /// uses this to correlate the incoming Decision Card with the click.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// One-line status for the frontend to display inline.
    pub message: String,
    /// Captured Claude CLI stdout/stderr — only set for `claude_cli`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_output: Option<String>,
}

/// Build a synthetic ElicitationRequest with a stable shape: two options,
/// one question, clear test markers in workspace_name and ai_title.
pub fn build_test_request() -> ElicitationRequest {
    let id = Uuid::new_v4().to_string();
    ElicitationRequest {
        id: id.clone(),
        session_id: format!("qa-diagnostic-{id}"),
        workspace_name: TEST_WORKSPACE_MARKER.to_string(),
        ai_title: Some("QA Diagnostic Test".into()),
        questions: vec![ElicitationQuestion {
            question: "QA 诊断 — 看到这条卡片说明 Decision Panel 通路正常。\n\n---\n\n这是 Fleet 自动发出的测试卡片，回答任一选项或忽略都行（10 秒后自动清理）。".into(),
            header: "诊断测试".into(),
            options: vec![
                ElicitationOption {
                    label: "收到了".into(),
                    description: "Decision Panel 工作正常".into(),
                    preview: None,
                },
                ElicitationOption {
                    label: "忽略".into(),
                    description: "知道了，关掉就行".into(),
                    preview: None,
                },
            ],
            multi_select: false,
        }],
        timestamp: chrono::Utc::now().to_rfc3339(),
    }
}

/// End-to-end test: write a fake request file so the existing watcher
/// loop picks it up and broadcasts to the frontend exactly like a real
/// AskUserQuestion-originated one. Spawns a background cleanup thread
/// so a forgotten test card doesn't linger past `timeout`.
pub fn run_end_to_end_test(timeout: Duration) -> Result<TestRunResult, String> {
    let req = build_test_request();
    crate::elicitation::write_request(&req)?;

    let id_for_thread = req.id.clone();
    std::thread::spawn(move || {
        std::thread::sleep(timeout);
        // If the user answered, both request + response exist; if they
        // ignored, only the request exists. `cleanup` nukes both — safe
        // either way.
        crate::elicitation::cleanup(&id_for_thread);
    });

    Ok(TestRunResult {
        kind: "end_to_end".into(),
        request_id: Some(req.id.clone()),
        message: format!(
            "Fake request at ~/.fleet/elicitation/{}.json — watcher should emit within ~2s",
            req.id
        ),
        claude_output: None,
    })
}

/// Claude CLI test: spawn `claude -p "<prompt>"` in a dedicated workspace
/// under `~/.fleet/diagnostics/` so the Agent actually exercises the
/// CLAUDE.md interaction-mode injection. Returns either when the process
/// exits or when `timeout` lapses (in which case we kill it and report —
/// a timeout is the *expected* outcome when the Decision Panel did receive
/// the question and is waiting for an answer).
pub fn run_claude_cli_test(timeout: Duration) -> Result<TestRunResult, String> {
    use std::process::{Command, Stdio};

    let bin = crate::claude_binary::resolve(None)
        .ok_or("No Claude CLI binary discovered (set Claude Binary in Settings)")?;

    let home = crate::session::real_home_dir().ok_or("cannot determine home directory")?;
    let workdir = home.join(".fleet").join("diagnostics");
    std::fs::create_dir_all(&workdir)
        .map_err(|e| format!("create diagnostics workspace: {e}"))?;

    // Keep the prompt tight: ask Claude to do exactly one AskUserQuestion
    // call and nothing else. The interaction-mode guidance in CLAUDE.md
    // does most of the steering; this prompt just supplies a topic.
    const PROMPT: &str =
        "诊断测试：请只调用一次 AskUserQuestion，问我「现在心情如何」，提供两个选项（开心 / 一般），不要做其他任何事。";

    let mut child = Command::new(&bin.path)
        .args(["-p", PROMPT])
        .current_dir(&workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn claude -p: {e}"))?;

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(TestRunResult {
                        kind: "claude_cli".into(),
                        request_id: None,
                        message: format!(
                            "claude -p ran {}s without exiting — if you saw a test question pop up, that's success (the session was waiting on your answer when we killed it)",
                            timeout.as_secs()
                        ),
                        claude_output: None,
                    });
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("waitpid: {e}")),
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("collect output: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = if stderr.trim().is_empty() {
        stdout
    } else if stdout.trim().is_empty() {
        format!("[stderr]\n{stderr}")
    } else {
        format!("{stdout}\n---stderr---\n{stderr}")
    };

    Ok(TestRunResult {
        kind: "claude_cli".into(),
        request_id: None,
        message: if output.status.success() {
            "claude -p exited 0 — if no test question popped up, the AskUserQuestion injection is not steering the model".into()
        } else {
            format!("claude -p exited with status {:?}", output.status.code())
        },
        claude_output: Some(combined),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_test_request_marks_workspace_and_title() {
        let req = build_test_request();
        assert_eq!(req.workspace_name, TEST_WORKSPACE_MARKER);
        assert_eq!(req.ai_title.as_deref(), Some("QA Diagnostic Test"));
        assert_eq!(req.questions.len(), 1);
        assert_eq!(req.questions[0].options.len(), 2);
    }

    #[test]
    fn build_test_request_question_has_divider_for_tts() {
        // Per the interaction-mode contract the first question's body is
        // split on a `---` line so TTS reads the pre-divider summary aloud.
        // Make sure our synthetic test card keeps that convention so the
        // diagnostic announcement reads naturally.
        let req = build_test_request();
        let body = &req.questions[0].question;
        let divider_count = body.matches("\n---\n").count();
        assert_eq!(
            divider_count, 1,
            "test question must contain exactly one TTS divider line, got body: {body}"
        );
    }

    #[test]
    fn build_test_request_uses_unique_ids() {
        let a = build_test_request();
        let b = build_test_request();
        assert_ne!(a.id, b.id);
        assert_ne!(a.session_id, b.session_id);
    }

    #[test]
    fn test_run_result_serializes_camel_case() {
        let r = TestRunResult {
            kind: "end_to_end".into(),
            request_id: Some("abc".into()),
            message: "ok".into(),
            claude_output: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"requestId\":\"abc\""));
        assert!(
            !json.contains("claudeOutput"),
            "None fields should be skipped: {json}"
        );
    }
}
