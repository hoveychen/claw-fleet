//! `fleet agents` / `agent` / `stop` / `interrupt` / `speed` — session listing,
//! detail, signalling, and token-speed reporting.

use crate::fmt::*;
use claw_fleet_core::agent_source::build_sources;
use claw_fleet_core::session::{scan_all_sources, SessionInfo, SessionStatus};

pub(crate) fn load_sessions() -> Vec<SessionInfo> {
    let sources = build_sources();
    scan_all_sources(&sources)
}

pub(crate) fn cmd_agents(show_all: bool, as_json: bool) {
    let sessions = load_sessions();
    let filtered: Vec<&SessionInfo> = sessions
        .iter()
        .filter(|s| show_all || !matches!(s.status, SessionStatus::Idle))
        .collect();

    if as_json {
        println!("{}", serde_json::to_string_pretty(&filtered).unwrap_or_default());
        return;
    }

    if filtered.is_empty() {
        if show_all {
            println!("No sessions found.");
        } else {
            println!("No active agents. Use --all to show idle sessions.");
        }
        return;
    }

    let b = c_bold();
    let r = c_reset();

    println!(
        "{b}{:<10}{r}  {b}{:<20}{r}  {b}{:<10}{r}  {b}{:>8}{r}  {b}{:>7}{r}  {b}{:>5}{r}  {b}{:<8}{r}  {b}{}{r}",
        "ID", "WORKSPACE", "STATUS", "SPEED", "TOKENS", "CTX%", "HARNESS", "MODEL"
    );
    println!("{}", "─".repeat(89));

    for s in &filtered {
        let id_display = if s.is_subagent {
            format!("  └ {}", short_id(&s.id))
        } else {
            short_id(&s.id).to_string()
        };
        let ws = truncate(&s.workspace_name, 20);
        let sc = c_status(&s.status);
        let status_str = format_status(&s.status);
        let model_str = s
            .model
            .as_deref()
            .map(short_model)
            .unwrap_or_else(|| "-".to_string());
        let ctx_str = s
            .context_percent
            .map(|p| format!("{}%", (p * 100.0).round() as u32))
            .unwrap_or_else(|| "-".to_string());

        println!(
            "{:<10}  {:<20}  {sc}{:<10}{r}  {:>8}  {:>7}  {:>5}  {:<8}  {}",
            id_display,
            ws,
            status_str,
            format_speed(s.token_speed),
            format_tokens(s.total_output_tokens),
            ctx_str,
            short_harness(&s.agent_source),
            model_str,
            r = c_reset(),
        );
    }
}

pub(crate) fn cmd_agent(id_prefix: &str, as_json: bool) {
    let sessions = load_sessions();
    let needle = id_prefix.to_lowercase();

    let matched: Vec<&SessionInfo> = sessions
        .iter()
        .filter(|s| {
            s.id.starts_with(id_prefix)
                || s.workspace_name.to_lowercase().contains(&needle)
        })
        .collect();

    if matched.is_empty() {
        eprintln!("No agent matching '{id_prefix}'");
        std::process::exit(1);
    }

    if matched.len() > 1 {
        if as_json {
            println!("{}", serde_json::to_string_pretty(&matched).unwrap_or_default());
            return;
        }
        eprintln!("Multiple agents match '{id_prefix}':");
        for s in &matched {
            eprintln!("  {} ({})", short_id(&s.id), s.workspace_name);
        }
        eprintln!("Please be more specific.");
        std::process::exit(1);
    }

    let s = matched[0];

    if as_json {
        println!("{}", serde_json::to_string_pretty(s).unwrap_or_default());
        return;
    }

    let b = c_bold();
    let r = c_reset();
    let kv = |k: &str, v: &str| {
        println!("  {b}{k:<18}{r}  {v}");
    };

    kv("Session ID:", &s.id);
    kv("Workspace:", &s.workspace_path);

    let sc = c_status(&s.status);
    kv("Status:", &format!("{sc}{}{r}", format_status(&s.status), r = c_reset()));

    kv("Harness:", short_harness(&s.agent_source));
    kv("Token Speed:", &format!("{:.1} tok/s", s.token_speed));
    kv("Total Tokens:", &format_tokens(s.total_output_tokens));
    if let Some(pct) = s.context_percent {
        kv("Context:", &format!("{}%", (pct * 100.0).round() as u32));
    }

    if let Some(ref model) = s.model {
        kv("Model:", model);
    }
    if let Some(ref effort) = s.effort {
        kv("Effort:", effort);
    }
    // The extended-thinking marker, which is a different fact from the effort
    // dial above — before they were split, dsh's effort arrived in this slot
    // and printing only `Thinking:` would have dropped it here.
    if let Some(ref thinking) = s.thinking_level {
        kv("Thinking:", thinking);
    }
    if let Some(ref ide) = s.ide_name {
        kv("IDE:", ide);
    }
    kv("Subagent:", if s.is_subagent { "Yes" } else { "No" });
    if let Some(ref parent) = s.parent_session_id {
        kv("Parent Session:", short_id(parent));
    }
    if let Some(ref desc) = s.agent_description {
        kv("Description:", desc);
    }
    if let Some(ref atype) = s.agent_type {
        kv("Agent Type:", atype);
    }
    if let Some(ref pid) = s.pid {
        kv("PID:", &pid.to_string());
    }
    kv("Last Active:", &format_age_ms(s.last_activity_ms));
    kv("Created:", &format_age_ms(s.created_at_ms));

    if let Some(ref preview) = s.last_message_preview {
        let first_line = preview.lines().next().unwrap_or("").trim();
        let truncated = truncate(first_line, 100);
        kv("Last Message:", &truncated);
    }
}

/// Resolve an id prefix / workspace name to exactly one signalable agent and
/// its pid. Exits the process with a diagnostic when that can't be done.
/// `verb` names the action in the error text ("stop", "interrupt").
fn resolve_agent(id_prefix: &str, verb: &str) -> (SessionInfo, u32) {
    let sessions = load_sessions();
    let needle = id_prefix.to_lowercase();

    let matched: Vec<&SessionInfo> = sessions
        .iter()
        .filter(|s| {
            s.id.starts_with(id_prefix)
                || s.workspace_name.to_lowercase().contains(&needle)
        })
        .collect();

    if matched.is_empty() {
        eprintln!("No agent matching '{id_prefix}'");
        std::process::exit(1);
    }

    if matched.len() > 1 {
        eprintln!("Multiple agents match '{id_prefix}':");
        for s in &matched {
            eprintln!("  {} ({})", short_id(&s.id), s.workspace_name);
        }
        eprintln!("Please be more specific.");
        std::process::exit(1);
    }

    let s = matched[0];

    if s.is_subagent {
        eprintln!(
            "Error: '{}' is a subagent — {verb} the parent session instead.",
            short_id(&s.id)
        );
        std::process::exit(1);
    }

    let Some(pid) = s.pid else {
        eprintln!(
            "Agent {} ({}) has no associated PID — cannot {verb}.",
            short_id(&s.id),
            s.workspace_name
        );
        std::process::exit(1);
    };

    (s.clone(), pid)
}

/// Interrupt the agent's in-flight tool call. The session survives and stays
/// resumable, so unlike `stop` this refuses to act on an ambiguous pid: it
/// would abort a sibling session's turn with no confirmation step.
pub(crate) fn cmd_interrupt(id_prefix: &str) {
    let (s, pid) = resolve_agent(id_prefix, "interrupt");

    // An interactive claude treats a real SIGINT as "quit" and orphans its tool
    // child; only the headless sessions Fleet spawns abort the call and stay
    // resumable. Calling that an "interrupt" would be a hard kill in disguise.
    if !claw_fleet_core::session_launch::is_fleet_owned_entrypoint(s.entrypoint.as_deref()) {
        eprintln!(
            "Error: {} ({}) was not launched by Fleet (entrypoint: {}). Only headless \
             Fleet-spawned sessions can be interrupted — an interactive claude treats \
             SIGINT as 'quit'. Use `fleet stop` instead.",
            short_id(&s.id),
            s.workspace_name,
            s.entrypoint.as_deref().unwrap_or("unknown")
        );
        std::process::exit(1);
    }

    if !s.pid_precise {
        eprintln!(
            "Error: multiple claude processes share workspace '{}', so the PID \
             for {} is ambiguous — interrupting could abort another session's \
             turn. Use `fleet stop` if you really mean to signal them all.",
            s.workspace_name,
            short_id(&s.id)
        );
        std::process::exit(1);
    }

    #[cfg(unix)]
    match claw_fleet_core::session::interrupt_pid_impl(pid) {
        Ok(()) => println!(
            "Interrupted agent {} ({}) [PID {}] — session remains resumable",
            short_id(&s.id),
            s.workspace_name,
            pid
        ),
        Err(e) => {
            eprintln!("Failed to interrupt PID {pid}: {e}");
            std::process::exit(1);
        }
    }

    #[cfg(not(unix))]
    {
        let _ = pid;
        eprintln!("Interrupt is not supported on this platform.");
        std::process::exit(1);
    }
}

pub(crate) fn cmd_stop(id_prefix: &str, force: bool) {
    let (s, pid) = resolve_agent(id_prefix, "stop");

    if !s.pid_precise {
        eprintln!(
            "Warning: multiple claude processes share workspace '{}'. \
             Stopping may affect other sessions in the same workspace.",
            s.workspace_name
        );
    }

    #[cfg(unix)]
    {
        let signal_name = if force { "SIGKILL" } else { "SIGTERM" };
        match signal_agent(pid, force) {
            Ok(()) => {
                println!(
                    "Sent {} to agent {} ({}) [PID {}]",
                    signal_name,
                    short_id(&s.id),
                    s.workspace_name,
                    pid
                );
            }
            Err(e) => {
                eprintln!("Failed to send {} to PID {}: {}", signal_name, pid, e);
                std::process::exit(1);
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = (force, pid);
        eprintln!("Stop is not supported on this platform.");
        std::process::exit(1);
    }
}

/// Signal an agent process **and its whole tree**; `force` picks SIGKILL over
/// SIGTERM. Signalling only the root orphans the agent's tool children.
#[cfg(unix)]
fn signal_agent(pid: u32, force: bool) -> Result<(), String> {
    // Probe first: a stale pid must still report "No such process" rather than
    // silently succeeding once the signalling fans out over the tree.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    claw_fleet_core::session::kill_pid_tree(pid, force)
}

pub(crate) fn cmd_speed(as_json: bool) {
    let sessions = load_sessions();
    let total: f64 = sessions.iter().map(|s| s.token_speed).sum();
    let active: Vec<&SessionInfo> = sessions
        .iter()
        .filter(|s| s.token_speed > 0.1)
        .collect();

    if as_json {
        let agents: Vec<serde_json::Value> = sessions
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id,
                    "workspace": s.workspace_name,
                    "status": format_status(&s.status),
                    "token_speed": s.token_speed,
                    "total_output_tokens": s.total_output_tokens,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "aggregate_speed": total,
                "active_agents": active.len(),
                "agents": agents,
            }))
            .unwrap_or_default()
        );
        return;
    }

    let b = c_bold();
    let r = c_reset();

    println!(
        "{b}Aggregate speed:{r}  {} tok/s",
        format!("{:.0}", total)
    );
    println!("{b}Active agents:{r}   {}", active.len());

    if !active.is_empty() {
        println!();
        println!(
            "  {b}{:<10}{r}  {b}{:<20}{r}  {b}{:>10}{r}  {b}{}{r}",
            "ID", "WORKSPACE", "SPEED", "STATUS"
        );
        println!("  {}", "─".repeat(56));

        for s in &active {
            let sc = c_status(&s.status);
            println!(
                "  {:<10}  {:<20}  {:>10}  {sc}{}{r}",
                short_id(&s.id),
                truncate(&s.workspace_name, 20),
                format_speed(s.token_speed),
                format_status(&s.status),
                r = c_reset(),
            );
        }
    } else if sessions.is_empty() {
        println!();
        println!("  {d}No sessions found.{r}", d = c_dim(), r = c_reset());
    } else {
        println!();
        println!(
            "  {d}No agents currently generating tokens.{r}",
            d = c_dim(),
            r = c_reset()
        );
    }
}

#[cfg(all(test, unix))]
mod stop_tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    fn pgrep(pattern: &str) -> bool {
        Command::new("pgrep")
            .args(["-f", pattern])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// `fleet stop` must reap the agent's whole process tree. A bare
    /// `kill(root)` leaves the tool child (a build, a test run, a server)
    /// running after the agent is gone.
    #[test]
    fn stop_kills_the_whole_process_tree() {
        // `sh` backgrounds the sleep and waits: signalling only `sh` reparents
        // the sleep to init, exactly like a claude process holding a Bash tool.
        let marker = "sleep 4747";
        let mut child = Command::new("sh")
            .args(["-c", "sleep 4747 & wait"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        std::thread::sleep(Duration::from_millis(300));
        assert!(pgrep(marker), "precondition: the tool child must be running");

        signal_agent(child.id(), false).expect("signal");
        std::thread::sleep(Duration::from_millis(600));

        let leaked = pgrep(marker);
        let _ = child.kill();
        let _ = child.wait();
        Command::new("pkill").args(["-9", "-f", marker]).output().ok();

        assert!(!leaked, "fleet stop orphaned the agent's tool child");
    }
}
