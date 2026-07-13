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
        parked: false,
    }
}

/// Build a synthetic `FleetAskRequest` exercising all three render hooks
/// (html / formFields / options) so the diagnostic Decision Card visually
/// proves end-to-end coverage of the fleet__ask renderer in one shot.
pub fn build_test_fleet_ask_request() -> crate::mcp_ipc::FleetAskRequest {
    use crate::mcp_ipc::{FleetAskFormField, FleetAskOption, FleetAskQuestion, FleetAskRequest, FormFieldKind};
    let id = Uuid::new_v4().to_string();
    FleetAskRequest {
        id: id.clone(),
        session_id: format!("qa-diagnostic-{id}"),
        workspace_name: TEST_WORKSPACE_MARKER.to_string(),
        ai_title: Some("QA Diagnostic — fleet__ask".into()),
        timestamp: chrono::Utc::now().to_rfc3339(),
        parked: false,
        questions: vec![FleetAskQuestion {
            question: "QA 诊断 (fleet__ask)：看到这条卡片说明 MCP 通路正常。\n\n---\n\n这是 Fleet 通过新 fleet__ask 通道发出的测试卡片，回答任一选项或忽略都行（10 秒后自动清理）。".into(),
            header: "诊断测试".into(),
            multi_select: false,
            html: Some(
                "<p style='font-family:sans-serif'>HTML preview render works.</p>\
                 <ul><li>iframe sandbox=\"\" 已生效</li><li>表单 + 选项可共存</li></ul>"
                    .into(),
            ),
            form_fields: vec![FleetAskFormField {
                name: "feedback".into(),
                kind: FormFieldKind::Text,
                label: "随手反馈（可选）".into(),
                placeholder: Some("有任何卡片渲染问题写这".into()),
                options: vec![],
                required: false,
                default: None,
                min: None,
                max: None,
                step: None,
            }],
            options: vec![
                FleetAskOption {
                    label: "三段都正常".into(),
                    description: "HTML / 表单 / 选项都渲染了".into(),
                    preview: None,
                },
                FleetAskOption {
                    label: "缺了什么".into(),
                    description: "在反馈框写下缺失的部分".into(),
                    preview: None,
                },
            ],
            images: vec![],
        }],
    }
}

/// fleet__ask end-to-end test: mirrors `run_end_to_end_test` but writes a
/// `FleetAskRequest` so the new MCP-side watcher (local_backend.rs +
/// hooks_server.rs) emits `fleet-ask-request` and the frontend's
/// `FleetAskCard` paints all three render hooks in one composite card.
pub fn run_fleet_ask_end_to_end_test(timeout: Duration) -> Result<TestRunResult, String> {
    let req = build_test_fleet_ask_request();
    crate::mcp_ipc::write_request(&req)?;
    let id_for_thread = req.id.clone();
    std::thread::spawn(move || {
        std::thread::sleep(timeout);
        crate::mcp_ipc::cleanup(&id_for_thread);
    });
    Ok(TestRunResult {
        kind: "fleet_ask_end_to_end".into(),
        request_id: Some(req.id.clone()),
        message: format!(
            "Fake fleet__ask request at ~/.fleet/fleet-ask/{}.json — watcher should emit within ~2s",
            req.id
        ),
        claude_output: None,
    })
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
/// CLAUDE.md interaction-mode injection.
///
/// **Evidence-based pass criteria.** Earlier versions declared success
/// whenever `claude -p` exited 0, which silently passed even when the
/// agent never called AskUserQuestion (the model can end the turn with
/// plain text in `-p` mode under various conditions). The new version
/// polls `elicitation::list_pending_requests()` throughout the spawn
/// and records whether a `TEST_WORKSPACE_MARKER` request ever appeared.
/// Combined with exit status, the final message says exactly which leg
/// failed:
///
/// - exit 0 + saw test card → wired end-to-end
/// - exit 0 + no test card → agent ran but didn't call AskUserQuestion
///   (look at the captured stream-json)
/// - timeout + saw test card → success (card landed, agent was waiting
///   on Boss's answer when we killed it)
/// - timeout + no test card → infrastructure hung
/// - non-zero exit → claude itself errored
pub fn run_claude_cli_test(timeout: Duration) -> Result<TestRunResult, String> {
    run_cli_test_inner(
        ElicitationKind::AskUserQuestion,
        "AskUserQuestion",
        "claude_cli",
        "诊断测试：请只调用一次 AskUserQuestion，问我「现在心情如何」，提供两个选项（开心 / 一般），不要做其他任何事。",
        timeout,
    )
}

/// Outcome of polling for the test card during a claude CLI run.
struct CardPollResult {
    /// Set when a TEST_WORKSPACE_MARKER request appeared at any point
    /// during the spawn (even briefly — Boss may have already answered
    /// by the time the child exited, in which case the request file is
    /// gone but the test still passed).
    matched_request_id: Option<String>,
    /// True if the timeout fired before the child exited; in that case
    /// the child has been killed in place so the caller's stdio read
    /// can complete.
    timed_out: bool,
}

#[derive(Clone, Copy)]
enum ElicitationKind {
    AskUserQuestion,
    FleetAsk,
}

/// Shared spawn + poll + report logic for both the v1 (AskUserQuestion)
/// and v2 (fleet__ask) Claude CLI diagnostics. Keeps the prompt and
/// `--allowed-tools` value per-variant; everything else is identical.
fn run_cli_test_inner(
    kind: ElicitationKind,
    allowed_tool: &str,
    test_kind_label: &str,
    prompt: &str,
    timeout: Duration,
) -> Result<TestRunResult, String> {
    use std::io::Read;
    use std::process::Stdio;

    let bin = crate::claude_binary::resolve(None)
        .ok_or("No Claude CLI binary discovered (set Claude Binary in Settings)")?;

    let home = crate::session::real_home_dir().ok_or("cannot determine home directory")?;
    let workdir = home.join(".fleet").join("diagnostics");
    std::fs::create_dir_all(&workdir)
        .map_err(|e| format!("create diagnostics workspace: {e}"))?;

    // Snapshot existing pending request ids so we can identify new
    // ones the test produced (vs. unrelated cards already in flight).
    let baseline: std::collections::HashSet<String> = match kind {
        ElicitationKind::AskUserQuestion => {
            crate::elicitation::list_pending_requests().into_iter().collect()
        }
        ElicitationKind::FleetAsk => {
            crate::mcp_ipc::list_pending_requests().into_iter().collect()
        }
    };

    let mut child = build_claude_command(std::path::Path::new(&bin.path), allowed_tool, prompt, &workdir)?
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn claude -p: {e}"))?;

    let poll = poll_for_test_card(&mut child, timeout, &baseline, kind);

    // Collect whatever stdout/stderr accumulated, even on timeout (we
    // had piped handles open). Without this, the timeout branch would
    // leave Boss with no evidence of what the agent did.
    let mut stdout_buf = String::new();
    let mut stderr_buf = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut stdout_buf);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut stderr_buf);
    }
    let exit_status = child.wait().ok();
    let combined = combine_streams(&stdout_buf, &stderr_buf);

    let message = build_cli_message(
        allowed_tool,
        exit_status.and_then(|s| s.code()),
        poll.timed_out,
        poll.matched_request_id.is_some(),
        timeout,
    );

    Ok(TestRunResult {
        kind: test_kind_label.into(),
        request_id: poll.matched_request_id,
        message,
        claude_output: Some(combined),
    })
}

/// Tight poll loop: every 200ms, peek `try_wait` and the appropriate
/// pending-request list. Returns as soon as the child exits or the
/// timeout lapses; on timeout the child is killed in place so the
/// caller can collect its captured stdio.
fn poll_for_test_card(
    child: &mut std::process::Child,
    timeout: Duration,
    baseline: &std::collections::HashSet<String>,
    kind: ElicitationKind,
) -> CardPollResult {
    let start = Instant::now();
    let mut matched: Option<String> = None;
    loop {
        // Always check pending IDs first so even a sub-200ms-lived card
        // gets caught.
        if matched.is_none() {
            let pending = match kind {
                ElicitationKind::AskUserQuestion => crate::elicitation::list_pending_requests(),
                ElicitationKind::FleetAsk => crate::mcp_ipc::list_pending_requests(),
            };
            for id in pending {
                if baseline.contains(&id) {
                    continue;
                }
                let is_test_card = match kind {
                    ElicitationKind::AskUserQuestion => crate::elicitation::read_request(&id)
                        .map(|r| r.workspace_name == TEST_WORKSPACE_MARKER)
                        .unwrap_or(false),
                    ElicitationKind::FleetAsk => crate::mcp_ipc::read_request(&id)
                        .map(|r| r.workspace_name == TEST_WORKSPACE_MARKER)
                        .unwrap_or(false),
                };
                if is_test_card {
                    matched = Some(id);
                    break;
                }
            }
        }
        match child.try_wait() {
            Ok(Some(_)) => {
                return CardPollResult { matched_request_id: matched, timed_out: false };
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return CardPollResult { matched_request_id: matched, timed_out: true };
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(_) => {
                // try_wait failed (rare); treat as exited so we don't
                // spin forever, and return whatever we matched.
                return CardPollResult { matched_request_id: matched, timed_out: false };
            }
        }
    }
}

fn combine_streams(stdout: &str, stderr: &str) -> String {
    if stderr.trim().is_empty() {
        stdout.to_string()
    } else if stdout.trim().is_empty() {
        format!("[stderr]\n{stderr}")
    } else {
        format!("{stdout}\n---stderr---\n{stderr}")
    }
}

/// Build the result message from the four orthogonal facts the test
/// observed. Kept pure so it's straightforward to unit-test each
/// branch without spawning a real claude binary.
fn build_cli_message(
    tool_name: &str,
    exit_code: Option<i32>,
    timed_out: bool,
    saw_test_card: bool,
    timeout: Duration,
) -> String {
    let timeout_secs = timeout.as_secs();
    match (timed_out, saw_test_card, exit_code) {
        // Happy paths: card landed, regardless of whether we killed
        // claude on timeout (which is expected — it was waiting for
        // Boss's answer).
        (false, true, Some(0)) => format!(
            "✅ claude -p exited 0 AND the test {tool_name} card landed in your Decision Panel — wired end-to-end."
        ),
        (true, true, _) => format!(
            "✅ claude -p ran {timeout_secs}s and we killed it, but the test {tool_name} card DID land — wired end-to-end (the agent was waiting on Boss's answer when we killed it)."
        ),
        // Smoking-gun failure: claude finished cleanly but never produced a card.
        (false, false, Some(0)) => format!(
            "❌ claude -p exited 0 but NO test {tool_name} card landed — the agent ran but never called the tool. Read the stream-json output below: look for `\"name\":\"{tool_name}\"` tool_use blocks (if missing, the CLAUDE.md interaction-mode injection isn't reaching the agent)."
        ),
        // Timeout with no card: hung without producing the tool call.
        (true, false, _) => format!(
            "❌ claude -p ran {timeout_secs}s, no test {tool_name} card ever landed — agent likely hung without calling the tool. Read the stream-json output below."
        ),
        // Non-zero exit.
        (_, _, Some(code)) => format!(
            "❌ claude -p exited with status {code}. Read the stream-json output below for the failure."
        ),
        (_, _, None) => "❌ claude -p exited without a status code (process error). Read the stream-json output below.".into(),
    }
}

/// Build the `claude --allowed-tools <tool> -p <prompt>` command Fleet
/// spawns from the QA diagnostic. Extracted so it can be unit-tested
/// without actually running claude — the failure mode this helper exists
/// to defend against (a redirected `$HOME`; historically the App-Sandbox
/// container on Tauri release builds, before the sandbox was dropped
/// 2026-07) is invisible to a behavioural test that just spawns the child.
///
/// `--allowed-tools <name>` is load-bearing: without it the tool isn't
/// in `claude -p`'s default tool list, so the CLAUDE.md interaction-mode
/// guidance has nothing to call and the model silently falls back to
/// plain text. `--output-format stream-json --verbose` makes the
/// captured output self-explanatory.
///
/// `HOME` is explicitly overridden to `real_home_dir()`. Origin: a child
/// spawned from the (then-sandboxed) Fleet desktop inherited
/// `$HOME=~/Library/Containers/com.hoveychen.claw-fleet/Data/`,
/// where Claude Code read an empty container settings.json with no
/// PreToolUse hook for AskUserQuestion → CC's default permission gate
/// denied the call in `-p` (non-interactive) mode → permission_denials
/// fired and the diagnostic reported a false negative. The sandbox is gone
/// (2026-07); the override stays as a defence against a polluted `$HOME`.
pub(crate) fn build_claude_command(
    claude_bin: &std::path::Path,
    allowed_tool: &str,
    prompt: &str,
    workdir: &std::path::Path,
) -> Result<std::process::Command, String> {
    let real_home = crate::session::real_home_dir()
        .ok_or("cannot determine home directory for HOME override")?;
    let mut cmd = std::process::Command::new(claude_bin);
    cmd.args([
        "--allowed-tools",
        allowed_tool,
        "--output-format",
        "stream-json",
        "--verbose",
        "-p",
        prompt,
    ])
    .current_dir(workdir)
    .env("HOME", real_home);
    Ok(cmd)
}

/// Claude CLI test for the `fleet__ask` MCP tool: same evidence-based
/// shape as `run_claude_cli_test`, but steers the model toward
/// `mcp__fleet__ask` (Claude Code's canonical name for the
/// `fleet__ask` MCP tool) and watches `mcp_ipc::list_pending_requests()`
/// for the test card instead of the AskUserQuestion elicitation dir.
pub fn run_fleet_ask_claude_cli_test(timeout: Duration) -> Result<TestRunResult, String> {
    run_cli_test_inner(
        ElicitationKind::FleetAsk,
        "mcp__fleet__ask",
        "fleet_ask_claude_cli",
        "诊断测试：请只调用一次 fleet__ask MCP 工具，问我「现在心情如何」，提供两个选项（开心 / 一般），不要做其他任何事。",
        timeout,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_command_overrides_home_to_escape_macos_sandbox() {
        // Historically (App Sandbox enabled, dropped 2026-07): when Fleet
        // desktop spawned `claude -p`, the child inherited
        // `$HOME=~/Library/Containers/com.hoveychen.claw-fleet/Data/`.
        // Claude Code then read an empty container settings.json with no
        // PreToolUse hook for AskUserQuestion → CC's default permission
        // gate denied the tool in `-p` mode and the diagnostic reported a
        // false negative. The HOME pin stays as a polluted-$HOME defence.
        //
        // The fix is to explicitly set HOME to `real_home_dir()` on the
        // Command builder. Pin that with this regression test: the
        // returned Command must declare a HOME env binding pointing at
        // the user's real home directory.
        use std::ffi::OsStr;
        let expected_home = crate::session::real_home_dir()
            .expect("self has a home dir");
        let cmd = build_claude_command(
            std::path::Path::new("/usr/bin/false"),
            "AskUserQuestion",
            "noop",
            std::path::Path::new("/tmp"),
        )
        .expect("build_claude_command should succeed when real_home_dir() resolves");
        let home_env = cmd
            .get_envs()
            .find(|(k, _)| *k == OsStr::new("HOME"))
            .and_then(|(_, v)| v.map(|os| os.to_owned()));
        assert_eq!(
            home_env.as_deref(),
            Some(expected_home.as_os_str()),
            "Command must explicitly bind HOME to real_home_dir() to escape \
             the macOS app sandbox container path"
        );
    }

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
    fn build_test_fleet_ask_request_exercises_all_three_render_hooks() {
        let req = build_test_fleet_ask_request();
        assert_eq!(req.workspace_name, TEST_WORKSPACE_MARKER);
        assert_eq!(req.questions.len(), 1);
        let q = &req.questions[0];
        assert!(q.html.is_some(), "html hook must be exercised");
        assert!(!q.form_fields.is_empty(), "formFields hook must be exercised");
        assert_eq!(q.options.len(), 2, "options hook must be exercised");
    }

    #[test]
    fn build_test_fleet_ask_request_question_has_tts_divider() {
        let req = build_test_fleet_ask_request();
        let body = &req.questions[0].question;
        let count = body.matches("\n---\n").count();
        assert_eq!(count, 1, "test question needs exactly one TTS divider: {body}");
    }

    #[test]
    fn build_cli_message_happy_path_exit_zero_with_card() {
        let msg = build_cli_message("AskUserQuestion", Some(0), false, true, Duration::from_secs(60));
        assert!(msg.starts_with("✅"), "expected success marker: {msg}");
        assert!(msg.contains("exited 0"));
        assert!(msg.contains("card landed"));
    }

    #[test]
    fn build_cli_message_smoking_gun_exit_zero_no_card() {
        // The most useful failure mode: agent ran cleanly but never
        // produced the card. Message must call this out clearly.
        let msg = build_cli_message("AskUserQuestion", Some(0), false, false, Duration::from_secs(60));
        assert!(msg.starts_with("❌"), "expected failure marker: {msg}");
        assert!(msg.contains("exited 0 but NO test"));
        assert!(msg.contains("AskUserQuestion"));
        assert!(msg.contains("interaction-mode injection"));
    }

    #[test]
    fn build_cli_message_timeout_with_card_is_success() {
        // Card landed but we killed claude on timeout (it was waiting on
        // Boss's answer). This is the normal happy path when Boss doesn't
        // click within the window — still wired end-to-end.
        let msg = build_cli_message("mcp__fleet__ask", None, true, true, Duration::from_secs(60));
        assert!(msg.starts_with("✅"), "timeout-with-card must be success: {msg}");
        assert!(msg.contains("60s"));
        assert!(msg.contains("DID land"));
    }

    #[test]
    fn build_cli_message_timeout_no_card() {
        let msg = build_cli_message("AskUserQuestion", None, true, false, Duration::from_secs(45));
        assert!(msg.starts_with("❌"));
        assert!(msg.contains("45s"));
        assert!(msg.contains("no test"));
    }

    #[test]
    fn build_cli_message_nonzero_exit() {
        let msg = build_cli_message("AskUserQuestion", Some(127), false, false, Duration::from_secs(60));
        assert!(msg.starts_with("❌"));
        assert!(msg.contains("status 127"));
    }

    #[test]
    fn combine_streams_handles_three_combinations() {
        assert_eq!(combine_streams("hello\n", ""), "hello\n");
        assert_eq!(combine_streams("", "boom\n"), "[stderr]\nboom\n");
        assert!(combine_streams("ok\n", "warn\n").contains("---stderr---"));
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
