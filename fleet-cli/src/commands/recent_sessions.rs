//! `fleet recent-sessions` — the `SessionStart` hook that tells a freshly
//! opened context window what this repository has been worked on lately.
//!
//! Fleet's analogue of ChatGPT's *Recent Conversation Context* layer. The text
//! is rendered by [`claw_fleet_core::recent_sessions::render_for_workspace`],
//! which the codex and dsh arms call too, so the three harnesses inject the
//! same block.
//!
//! It is a separate `SessionStart` entry rather than an extension of
//! `notes-hint` because Claude Code keeps the `additionalContext` of every
//! matching hook and passes them to the model together — so this block gets
//! its own byte budget instead of competing with the notes summary for one.

use serde_json::{json, Value};
use std::path::PathBuf;

/// Emit the recent-sessions block for the workspace this session opened in.
///
/// Silent exit 0 when the workspace has no other sessions to report (a fresh
/// repository costs nothing) and when the scan or the render fails — a hook
/// that cannot say anything useful must not say anything at all.
pub(crate) fn cmd_recent_sessions() {
    use std::io::Read;
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let parsed = serde_json::from_str::<Value>(&input).ok();

    // The payload is authoritative for this firing; the process cwd is the
    // fallback for a hand-run invocation (which is how this is verified).
    let cwd = parsed
        .as_ref()
        .and_then(|v| v.get("cwd").and_then(Value::as_str).map(PathBuf::from))
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let session_id = parsed
        .as_ref()
        .and_then(|v| v.get("session_id").and_then(Value::as_str))
        .map(str::to_string)
        .or_else(crate::commands::session::read_fleet_session_id);

    let Some(block) = claw_fleet_core::recent_sessions::render_for_workspace(
        &cwd.to_string_lossy(),
        session_id.as_deref(),
    ) else {
        return;
    };
    let out = json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": block,
        }
    });
    println!("{out}");
}
