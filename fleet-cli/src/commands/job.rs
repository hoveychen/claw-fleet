//! `fleet job` — the CLI face of the `fleet__job` MCP tool.
//!
//! Delegates to [`claw_fleet_core::mcp_control::handle`] with the argument shape
//! the MCP tool receives, so both front ends print byte-identical output.
//! `fleet job wait` fits in one foreground Bash call (it blocks at most 540s).

use crate::commands::session::resolve_session_id;
use crate::JobCommands;
use serde_json::json;

pub(crate) fn cmd_job(action: JobCommands) {
    let args = match action {
        JobCommands::Done { id } => {
            // The wake watch's condition: exit 0 once the job has exited.
            let done = claw_fleet_core::job::get(&id)
                .map(|r| claw_fleet_core::job::is_done(&r))
                .unwrap_or(false);
            std::process::exit(if done { 0 } else { 1 });
        }
        JobCommands::Run { command, cwd, wake } => {
            json!({"action":"run","command":command,"cwd":cwd,"wake":wake})
        }
        JobCommands::Wait { id, timeout, tail } => {
            json!({"action":"wait","id":id,"timeout":timeout,"tail":tail})
        }
        JobCommands::Status { id, tail } => json!({"action":"status","id":id,"tail":tail}),
        JobCommands::Stop { id } => json!({"action":"stop","id":id}),
        JobCommands::List => json!({"action":"list"}),
    };
    let sid = resolve_session_id(None);
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    match claw_fleet_core::mcp_control::handle("fleet__job", &args, sid.as_deref(), &cwd) {
        Ok(text) => println!("{text}"),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}
