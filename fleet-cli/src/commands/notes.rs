//! `fleet notes` / `fleet history` — the CLI face of the session-notes and
//! session-history tools, for sessions Fleet did not launch (which never see
//! the `fleet__notes` / `fleet__history` MCP tools).
//!
//! Both delegate to [`claw_fleet_core::mcp_control::handle`] with the same
//! argument shape the MCP tool receives, so the two front ends print
//! byte-identical output and cannot drift.

use crate::commands::session::read_fleet_session_id;
use crate::{HistoryCommands, NotesCommands};
use serde_json::{json, Value};

fn run(tool: &str, args: Value) {
    let sid = read_fleet_session_id();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    match claw_fleet_core::mcp_control::handle(tool, &args, sid.as_deref(), &cwd) {
        Ok(text) => println!("{text}"),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

pub(crate) fn cmd_notes(action: NotesCommands) {
    let args = match action {
        NotesCommands::Write { path, text } => json!({"action":"write","path":path,"text":text}),
        NotesCommands::Append { path, text } => json!({"action":"append","path":path,"text":text}),
        NotesCommands::Read { path, start_line, stop_line } => {
            json!({"action":"read","path":path,"start_line":start_line,"stop_line":stop_line})
        }
        NotesCommands::List { prefix } => json!({"action":"list","prefix":prefix}),
        NotesCommands::Search { query, prefix, max_files, max_matches_per_file } => json!({
            "action":"search","query":query,"prefix":prefix,
            "max_files":max_files,"max_matches_per_file":max_matches_per_file
        }),
    };
    run("fleet__notes", args);
}

/// `fleet notes-hint` — the `SessionStart` hook entrypoint. Claude Code sends
/// `{ session_id, cwd, source, … }` on stdin; when the session (or a handoff
/// predecessor) has notes, emit a bounded summary as additional context so the
/// first turn of the new context window opens with the checkpoint in view.
/// Silent exit 0 otherwise — most sessions never take notes and must pay
/// nothing for the hook.
pub(crate) fn cmd_notes_hint() {
    use std::io::Read;
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let parsed = serde_json::from_str::<Value>(&input).ok();
    // The payload's session_id is authoritative for this firing; the env is the
    // fallback for a hand-run invocation.
    let session_id = parsed
        .as_ref()
        .and_then(|v| v.get("session_id").and_then(Value::as_str))
        .map(str::to_string)
        .or_else(read_fleet_session_id);
    let Some(sid) = session_id else { return };
    let Some(hint) = claw_fleet_core::session_notes::render_hint(&sid) else { return };
    let out = json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": hint,
        }
    });
    println!("{out}");
}

pub(crate) fn cmd_history(action: HistoryCommands) {
    let args = match action {
        HistoryCommands::Search { query, limit } => {
            json!({"action":"search","query":query.join(" "),"limit":limit})
        }
        HistoryCommands::Read { line_no, session, offset_chars, limit_chars } => json!({
            "action":"read","line_no":line_no,"session":session,
            "offset_chars":offset_chars,"limit_chars":limit_chars
        }),
    };
    run("fleet__history", args);
}
