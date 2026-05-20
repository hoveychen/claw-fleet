//! `fleet-hooks-server` — Phase 3 P7 entry point for the hook endpoints
//! (`/guard/*`, `/elicitation/*`, `/plan-approval/*`, `/accounts`, `/llm/*`,
//! `/feishu/*`, `/audit`, `/daily_report`, `/search`) that used to live
//! inside `fleet serve`'s ~3000-line `cmd_serve` together with the now-
//! retired supervisor::tick loop.
//!
//! Phase 3 P7 ships this binary as the **canonical entry name** for those
//! hooks (the LaunchAgent autostart was removed in the same phase). To keep
//! the diff tractable, the implementation is delegated to the existing
//! `fleet serve` subcommand via `execvp`: this binary resolves the `fleet`
//! / `fleet-cli` binary next to itself and replaces its own image, so all
//! existing routes, port-file conventions, and SSE behaviour carry over
//! 1:1. The follow-up cleanup (extract `cmd_serve` into a shared
//! `claw_fleet_core::hooks_server` module so both this binary and `fleet
//! serve` can call it directly) is tracked in TASKS.md.

use std::ffi::CString;
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
    let fleet = resolve_fleet_binary()
        .ok_or_else(|| anyhow::anyhow!("fleet (or fleet-cli) binary not found next to fleet-hooks-server"))?;

    let mut args: Vec<String> = vec!["fleet".into(), "serve".into()];
    if cli.port != 0 {
        args.push("--port".into());
        args.push(cli.port.to_string());
    }
    if let Some(t) = cli.token {
        args.push("--token".into());
        args.push(t);
    }
    if let Some(pf) = cli.port_file {
        args.push("--port-file".into());
        args.push(pf.to_string_lossy().into_owned());
    }

    eprintln!(
        "[fleet-hooks-server] Phase 3 P7 shim: execvp -> {} serve …",
        fleet.display()
    );

    #[cfg(unix)]
    {
        let path = CString::new(fleet.as_os_str().to_string_lossy().as_bytes())
            .map_err(|e| anyhow::anyhow!("path C-string: {e}"))?;
        let argv: Vec<CString> = args
            .iter()
            .map(|s| CString::new(s.as_str()).unwrap())
            .collect();
        let mut argv_ptrs: Vec<*const libc::c_char> =
            argv.iter().map(|c| c.as_ptr()).collect();
        argv_ptrs.push(std::ptr::null());
        unsafe {
            libc::execvp(path.as_ptr(), argv_ptrs.as_ptr());
        }
        // execvp only returns on error.
        Err(anyhow::anyhow!(
            "execvp failed: {}",
            std::io::Error::last_os_error()
        ))
    }
    #[cfg(not(unix))]
    {
        // Fallback: spawn as subprocess (no exec-replace on non-unix).
        let status = std::process::Command::new(&fleet)
            .args(&args[1..])
            .status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// Same resolution logic as the desktop crate: look next to ourselves for
/// the bundled `fleet` (production) or `fleet-cli` (cargo dev).
fn resolve_fleet_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let parent = exe.parent()?;
    let prod = parent.join("fleet");
    if prod.exists() {
        return Some(prod);
    }
    let dev = parent.join("fleet-cli");
    if dev.exists() {
        return Some(dev);
    }
    None
}
