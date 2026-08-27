//! `fleet acp` — speak the Agent Client Protocol on stdin/stdout.
//!
//! This is the subcommand an ACP-compatible editor spawns. Register it in the
//! editor's agent list (Zed's `agent_servers`, the JetBrains ACP settings, the
//! VS Code ACP extension, …) and Fleet shows up alongside Claude Code, Gemini
//! CLI and the rest of the ACP registry.
//!
//! Two modes, chosen by `--url`:
//!
//! - **local** (no `--url`) — run the agent against this machine's workspace.
//! - **proxy** (`--url wss://host/acp`) — forward the same stdio conversation
//!   to a remote Fleet. This is the shape that makes Fleet Cloud reachable from
//!   an editor: the editor can only spawn a local subprocess, so the local
//!   subprocess is a pipe to the container.

/// Run the ACP agent on stdio. Never returns until the peer hangs up.
pub(crate) fn cmd_acp(url: Option<&str>, token: Option<&str>) {
    match url {
        Some(url) => {
            // The token may also come from the environment so it does not have
            // to sit in an editor's settings file in plaintext.
            let token = token
                .map(String::from)
                .or_else(|| std::env::var("FLEET_PUBLIC_TOKEN").ok())
                .filter(|t| !t.is_empty());
            if let Err(e) = claw_fleet_core::acp::stdio::serve_proxy(url, token.as_deref()) {
                // stderr, never stdout: stdout carries the protocol, and one
                // stray non-JSON line there breaks the client's parser.
                eprintln!("fleet acp: {e}");
                std::process::exit(1);
            }
        }
        None => {
            if let Err(e) = claw_fleet_core::acp::stdio::serve_local() {
                eprintln!("fleet acp: {e}");
                std::process::exit(1);
            }
        }
    }
}
