//! `fleet-hooks-server` — Phase 4 P2 entry point for the hook endpoints
//! (`/guard/*`, `/elicitation/*`, `/plan-approval/*`, `/accounts`, `/llm/*`,
//! `/feishu/*`, `/audit`, `/daily_report`, `/search`).
//!
//! Phase 4 P1 extracted `fleet serve`'s body to
//! `claw_fleet_core::hooks_server::serve`, so this binary now calls the
//! shared lib directly instead of trampolining through `execvp` into
//! `fleet serve`. The two entry points run the exact same code; the rename
//! is purely about responsibility — fleet serve also exists as a backwards-
//! compat alias for the `fleet` CLI.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "fleet-hooks-server", version, about = "Hook endpoints for Claude Code (guard / elicitation / plan-approval / etc.)")]
struct Cli {
    /// Port to listen on. 0 = ephemeral (default).
    #[arg(long, default_value_t = 0)]
    port: u16,
    /// Bearer token required on incoming requests; defaults to the value
    /// the fleet CLI writes under `~/.claude/fleet/`.
    #[arg(long)]
    token: Option<String>,
    /// Path to write the chosen port number (for clients that don't read
    /// the canonical `~/.claude/fleet/port` file).
    #[arg(long)]
    port_file: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let token = cli.token.unwrap_or_default();
    claw_fleet_core::hooks_server::serve(cli.port, token, cli.port_file);
    Ok(())
}
