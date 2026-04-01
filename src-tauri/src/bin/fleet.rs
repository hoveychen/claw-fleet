use clap::{Parser, Subcommand};
use std::time::{SystemTime, UNIX_EPOCH};

use claw_fleet_lib::account::{fetch_account_info_blocking as fetch_account_info, AccountInfo, UsageStats};
use claw_fleet_lib::agent_source::{self, build_sources, find_source_for_path};
use claw_fleet_lib::hooks;
use claw_fleet_lib::memory;
use claw_fleet_lib::skills;
use claw_fleet_lib::session::{get_claude_dir, scan_all_sources, SessionInfo, SessionStatus};
use claw_fleet_lib::{FLEET_SKILL_MD, SKILL_TARGETS};

// ── Color helpers ─────────────────────────────────────────────────────────────

fn use_color() -> bool {
    std::env::var("NO_COLOR").is_err()
        && std::env::var("TERM").map_or(true, |t| t != "dumb")
}

fn status_color(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Thinking | SessionStatus::Streaming => "\x1b[32m",
        SessionStatus::Executing | SessionStatus::Processing => "\x1b[33m",
        SessionStatus::Delegating => "\x1b[35m",
        SessionStatus::WaitingInput => "\x1b[34m",
        SessionStatus::Active => "\x1b[36m",
        SessionStatus::Idle => "\x1b[2m",
    }
}

fn c_reset() -> &'static str {
    if use_color() { "\x1b[0m" } else { "" }
}

fn c_bold() -> &'static str {
    if use_color() { "\x1b[1m" } else { "" }
}

fn c_dim() -> &'static str {
    if use_color() { "\x1b[2m" } else { "" }
}

fn c_status(status: &SessionStatus) -> &'static str {
    if use_color() { status_color(status) } else { "" }
}

// ── Format helpers ─────────────────────────────────────────────────────────────

fn format_speed(tps: f64) -> String {
    if tps < 0.1 {
        return "-".to_string();
    }
    if tps >= 1000.0 {
        return format!("{:.1}k t/s", tps / 1000.0);
    }
    format!("{:.0} t/s", tps)
}

fn format_tokens(n: u64) -> String {
    if n == 0 {
        return "-".to_string();
    }
    if n >= 1_000_000 {
        return format!("{:.1}M", n as f64 / 1_000_000.0);
    }
    if n >= 1_000 {
        return format!("{:.0}K", n as f64 / 1_000.0);
    }
    format!("{}", n)
}

fn format_status(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Thinking => "Thinking",
        SessionStatus::Executing => "Executing",
        SessionStatus::Streaming => "Streaming",
        SessionStatus::Delegating => "Delegating",
        SessionStatus::Processing => "Processing",
        SessionStatus::WaitingInput => "WaitInput",
        SessionStatus::Active => "Active",
        SessionStatus::Idle => "Idle",
    }
}

fn format_age_ms(ms: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let diff_secs = now.saturating_sub(ms) / 1000;
    if diff_secs < 60 {
        return format!("{}s ago", diff_secs);
    }
    if diff_secs < 3600 {
        return format!("{}m ago", diff_secs / 60);
    }
    if diff_secs < 86400 {
        return format!("{}h ago", diff_secs / 3600);
    }
    format!("{}d ago", diff_secs / 86400)
}

fn format_resets_at(rfc3339: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|_| rfc3339.to_string())
}

fn short_id(id: &str) -> &str {
    &id[..id.len().min(8)]
}

fn short_model(model: &str) -> String {
    let s = model.trim_start_matches("claude-");
    // Strip trailing date suffix like -20251022
    if let Some(pos) = s.rfind('-') {
        let suffix = &s[pos + 1..];
        if suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_digit()) {
            return s[..pos].to_string();
        }
    }
    s.to_string()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}

// ── CLI definition ─────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "fleet",
    version,
    about = "Claw Fleet CLI — monitor Claude Code agents",
    long_about = None
)]
struct Cli {
    /// Run command on a remote host via SSH. Installs fleet on the remote if needed.
    /// Accepts any SSH destination: user@host, hostname, or an SSH config profile name.
    #[arg(long, global = true, value_name = "HOST")]
    remote: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List agents (active only by default; use --all to include idle)
    #[command(alias = "ls")]
    Agents {
        /// Include idle sessions
        #[arg(short, long)]
        all: bool,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Show details for a specific agent (prefix-match on ID or workspace name)
    Agent {
        /// Session ID prefix or workspace name
        id: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Stop an agent by sending SIGTERM (use --force for SIGKILL)
    Stop {
        /// Session ID prefix or workspace name
        id: String,
        /// Send SIGKILL instead of SIGTERM
        #[arg(short, long)]
        force: bool,
    },
    /// Show account info and rate-limit usage
    Account {
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Show per-agent and aggregate token speed
    Speed {
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// List and view agent memories across all workspaces
    #[command(alias = "mem")]
    Memory {
        /// Show content of a specific memory file (workspace/filename or full path)
        file: Option<String>,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Start the HTTP probe server (used by Fleet app for remote monitoring)
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value = "7007")]
        port: u16,
        /// Authentication token (required)
        #[arg(long)]
        token: String,
    },
    /// Search session content (full-text search across all sessions)
    Search {
        /// Search query (supports multiple terms for AND matching)
        query: Vec<String>,
        /// Maximum number of results
        #[arg(short, long, default_value = "20")]
        limit: usize,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Audit agent sessions for risky commands (network, file mutations, etc.)
    Audit {
        /// Filter by minimum risk level: medium, high, critical
        #[arg(short = 'l', long, default_value = "medium")]
        level: String,
        /// Only show events for sessions matching this ID prefix or workspace name
        #[arg(short, long)]
        filter: Option<String>,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// View or generate daily reports
    Report {
        /// Date to view (YYYY-MM-DD, default: yesterday)
        #[arg(short, long)]
        date: Option<String>,
        /// Backfill all missing reports for the last 90 days
        #[arg(long)]
        backfill: bool,
        /// Regenerate a specific date's report (metrics only)
        #[arg(long)]
        regenerate: bool,
        /// Generate lessons from sessions (requires claude CLI)
        #[arg(long)]
        lessons: bool,
        /// Force regenerate AI summary (requires claude CLI)
        #[arg(long)]
        summary: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Language for AI-generated content (en, zh, etc. Default: en)
        #[arg(long, default_value = "en")]
        lang: String,
    },
    /// Manage Fleet skill for AI coding tools
    Skill {
        #[command(subcommand)]
        action: SkillCommands,
    },
}

#[derive(Subcommand)]
enum SkillCommands {
    /// Install Fleet skill to all detected AI tools (Claude Code, Copilot, Gemini CLI)
    Install,
}

fn main() {
    let cli = Cli::parse();

    if let Some(ref host) = cli.remote {
        match &cli.command {
            Commands::Serve { .. } | Commands::Skill { .. } => {
                eprintln!("Error: --remote is not supported with the '{}' subcommand.",
                    match &cli.command {
                        Commands::Serve { .. } => "serve",
                        Commands::Skill { .. } => "skill",
                        _ => unreachable!(),
                    }
                );
                std::process::exit(1);
            }
            _ => {}
        }
        let remote_bin = ensure_remote_fleet(host);
        delegate_to_remote(host, &remote_bin);
    }

    match cli.command {
        Commands::Agents { all, json } => cmd_agents(all, json),
        Commands::Agent { id, json } => cmd_agent(&id, json),
        Commands::Stop { id, force } => cmd_stop(&id, force),
        Commands::Account { json } => cmd_account(json),
        Commands::Speed { json } => cmd_speed(json),
        Commands::Memory { file, json } => cmd_memory(file, json),
        Commands::Search { query, limit, json } => cmd_search(&query.join(" "), limit, json),
        Commands::Audit { level, filter, json } => cmd_audit(&level, filter.as_deref(), json),
        Commands::Report { date, backfill, regenerate, lessons, summary, json, lang } => cmd_report(date, backfill, regenerate, lessons, summary, json, &lang),
        Commands::Serve { port, token } => cmd_serve(port, token),
        Commands::Skill { action } => match action {
            SkillCommands::Install => cmd_skill_install(),
        },
    }
}

// ── Commands ───────────────────────────────────────────────────────────────────

// ── Remote SSH helpers ─────────────────────────────────────────────────────────

fn remote_fleet_install_path() -> &'static str {
    "~/.fleet-probe/fleet"
}

/// Find the local fleet binary: sidecar next to the current exe, then PATH.
fn find_local_fleet_binary() -> Option<std::path::PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let c = dir.join("fleet");
            if c.exists() { return Some(c); }
            let c2 = dir.join("fleet-cli");
            if c2.exists() { return Some(c2); }
        }
    }
    for dir in std::env::var("PATH").unwrap_or_default().split(':') {
        let p = std::path::PathBuf::from(dir).join("fleet");
        if p.exists() { return Some(p); }
    }
    None
}

/// Run a command on the remote host and return stdout, or an error string.
fn ssh_exec_remote(host: &str, cmd: &str) -> Result<String, String> {
    let output = std::process::Command::new("ssh")
        .args([
            "-o", "StrictHostKeyChecking=accept-new",
            "-o", "ConnectTimeout=15",
            "-o", "BatchMode=yes",
            host,
            cmd,
        ])
        .output()
        .map_err(|e| format!("ssh failed: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Detect the usable fleet binary path on the remote host, and install/upgrade if needed.
/// Returns the remote path to use for delegation.
fn ensure_remote_fleet(host: &str) -> String {
    let current_version = env!("CARGO_PKG_VERSION");
    let install_bin = remote_fleet_install_path();

    // One SSH call: detect remote platform, find fleet (PATH first, then install path),
    // and read its version. Outputs two lines: "BIN:<path>" and "VER:<version>".
    let detect_cmd = format!(
        r#"uname -sm; FLEET=$(which fleet 2>/dev/null || echo {install_bin}); echo "BIN:$FLEET"; $FLEET --version 2>/dev/null | head -1 | sed 's/^/VER:/' || echo "VER:""#
    );
    let detect_out = ssh_exec_remote(host, &detect_cmd).unwrap_or_else(|e| {
        eprintln!("Error: cannot connect to {host}: {e}");
        std::process::exit(1);
    });

    let mut uname = String::new();
    let mut found_bin = install_bin.to_string();
    let mut found_ver = String::new();
    for line in detect_out.lines() {
        if let Some(v) = line.strip_prefix("BIN:") { found_bin = v.trim().to_string(); }
        else if let Some(v) = line.strip_prefix("VER:") { found_ver = v.trim().to_string(); }
        else if !line.is_empty() { uname = line.trim().to_string(); }
    }

    if found_ver.contains(current_version) {
        return found_bin; // Already up to date — use the discovered path
    }

    eprintln!("Installing fleet {current_version} on {host}…");

    if let Err(e) = ssh_exec_remote(host, "mkdir -p ~/.fleet-probe") {
        eprintln!("Error: cannot create remote directory: {e}");
        std::process::exit(1);
    }

    // Try SCP if local binary matches remote platform
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let local_matches = match (os, arch) {
        ("linux",  "x86_64")  => uname.contains("Linux") && uname.contains("x86_64"),
        ("linux",  "aarch64") => uname.contains("Linux") && (uname.contains("aarch64") || uname.contains("arm64")),
        ("macos",  "aarch64") => uname.contains("Darwin") && uname.contains("arm64"),
        ("macos",  "x86_64")  => uname.contains("Darwin") && uname.contains("x86_64"),
        _ => false,
    };

    if local_matches {
        if let Some(bin_path) = find_local_fleet_binary() {
            let scp_ok = std::process::Command::new("scp")
                .args([
                    "-o", "StrictHostKeyChecking=accept-new",
                    "-o", "ConnectTimeout=30",
                    &bin_path.to_string_lossy(),
                    &format!("{host}:{install_bin}"),
                ])
                .status()
                .map(|s| s.success())
                .unwrap_or(false);

            if scp_ok {
                let _ = ssh_exec_remote(host, &format!("chmod +x {install_bin}"));
                eprintln!("Fleet installed via SCP.");
                return install_bin.to_string();
            }
            eprintln!("SCP failed, falling back to remote download…");
        }
    }

    // Fall back: download directly on the remote
    let release_suffix = if uname.contains("Linux") && uname.contains("x86_64") {
        "linux-x64"
    } else if uname.contains("Linux") && (uname.contains("aarch64") || uname.contains("arm64")) {
        "linux-arm64"
    } else {
        eprintln!("Error: unsupported remote platform ({uname}). Cannot auto-install fleet.");
        std::process::exit(1);
    };

    let dl_url = format!(
        "https://github.com/hoveychen/claw-fleet/releases/latest/download/fleet-{release_suffix}"
    );
    let dl_cmd = format!(
        "curl -fsSL '{dl_url}' -o {install_bin}.tmp && mv {install_bin}.tmp {install_bin} && chmod +x {install_bin} && echo OK"
    );

    match ssh_exec_remote(host, &dl_cmd) {
        Ok(out) if out.contains("OK") => eprintln!("Fleet installed via remote download."),
        Ok(out) => {
            eprintln!("Remote install may have failed: {out}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: remote install failed: {e}");
            std::process::exit(1);
        }
    }

    install_bin.to_string()
}

/// Replace the current process with `ssh <host> <remote_bin> <original-args-minus-remote>`.
fn delegate_to_remote(host: &str, remote_bin: &str) -> ! {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut filtered: Vec<String> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == "--remote" {
            i += 2; // skip flag and its value
        } else if raw[i].starts_with("--remote=") {
            i += 1; // skip --remote=value
        } else {
            filtered.push(raw[i].clone());
            i += 1;
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new("ssh")
            .args(["-o", "StrictHostKeyChecking=accept-new", host, remote_bin])
            .args(&filtered)
            .exec(); // replaces current process
        eprintln!("exec ssh failed: {err}");
        std::process::exit(1);
    }

    #[cfg(not(unix))]
    {
        let status = std::process::Command::new("ssh")
            .args(["-o", "StrictHostKeyChecking=accept-new", host, remote_bin])
            .args(&filtered)
            .status()
            .unwrap_or_else(|e| {
                eprintln!("ssh failed: {e}");
                std::process::exit(1);
            });
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn load_sessions() -> Vec<SessionInfo> {
    let sources = build_sources();
    scan_all_sources(&sources)
}

fn cmd_agents(show_all: bool, as_json: bool) {
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
        "{b}{:<10}{r}  {b}{:<20}{r}  {b}{:<10}{r}  {b}{:>8}{r}  {b}{:>7}{r}  {b}{:>5}{r}  {b}{}{r}",
        "ID", "WORKSPACE", "STATUS", "SPEED", "TOKENS", "CTX%", "MODEL"
    );
    println!("{}", "─".repeat(80));

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
            "{:<10}  {:<20}  {sc}{:<10}{r}  {:>8}  {:>7}  {:>5}  {}",
            id_display,
            ws,
            status_str,
            format_speed(s.token_speed),
            format_tokens(s.total_output_tokens),
            ctx_str,
            model_str,
            r = c_reset(),
        );
    }
}

fn cmd_agent(id_prefix: &str, as_json: bool) {
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

    kv("Token Speed:", &format!("{:.1} tok/s", s.token_speed));
    kv("Total Tokens:", &format_tokens(s.total_output_tokens));
    if let Some(pct) = s.context_percent {
        kv("Context:", &format!("{}%", (pct * 100.0).round() as u32));
    }

    if let Some(ref model) = s.model {
        kv("Model:", model);
    }
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

fn cmd_stop(id_prefix: &str, force: bool) {
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
            "Error: '{}' is a subagent — stop the parent session instead.",
            short_id(&s.id)
        );
        std::process::exit(1);
    }

    let Some(pid) = s.pid else {
        eprintln!(
            "Agent {} ({}) has no associated PID — cannot stop.",
            short_id(&s.id),
            s.workspace_name
        );
        std::process::exit(1);
    };

    if !s.pid_precise {
        eprintln!(
            "Warning: multiple claude processes share workspace '{}'. \
             Stopping may affect other sessions in the same workspace.",
            s.workspace_name
        );
    }

    #[cfg(unix)]
    {
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        let signal_name = if force { "SIGKILL" } else { "SIGTERM" };
        let ret = unsafe { libc::kill(pid as libc::pid_t, signal) };
        if ret == 0 {
            println!(
                "Sent {} to agent {} ({}) [PID {}]",
                signal_name,
                short_id(&s.id),
                s.workspace_name,
                pid
            );
        } else {
            let err = std::io::Error::last_os_error();
            eprintln!("Failed to send {} to PID {}: {}", signal_name, pid, err);
            std::process::exit(1);
        }
    }

    #[cfg(not(unix))]
    {
        eprintln!("Stop is not supported on this platform.");
        std::process::exit(1);
    }
}

fn cmd_account(as_json: bool) {
    match fetch_account_info() {
        Ok(info) => {
            if as_json {
                println!("{}", serde_json::to_string_pretty(&info).unwrap_or_default());
                return;
            }
            print_account(&info);
        }
        Err(e) => {
            eprintln!("Error fetching account info: {e}");
            std::process::exit(1);
        }
    }
}


fn print_usage_bar(stat: &UsageStats) -> String {
    let bar_len = 20usize;
    let filled = (stat.utilization * bar_len as f64).round() as usize;
    let filled = filled.min(bar_len);
    let empty = bar_len - filled;
    let color = if use_color() {
        if stat.utilization > 0.8 {
            "\x1b[31m"
        } else if stat.utilization > 0.5 {
            "\x1b[33m"
        } else {
            "\x1b[32m"
        }
    } else {
        ""
    };
    format!(
        "{color}[{}{}]{r}  {color}{:5.1}%{r}",
        "█".repeat(filled),
        "░".repeat(empty),
        stat.utilization * 100.0,
        r = c_reset(),
    )
}

fn print_account(info: &AccountInfo) {
    let b = c_bold();
    let r = c_reset();

    println!("{b}Account{r}");
    println!("  {b}{:<16}{r}  {} <{}>", "Name:", info.full_name, info.email);
    if !info.organization_name.is_empty() {
        println!("  {b}{:<16}{r}  {}", "Organization:", info.organization_name);
    }
    println!("  {b}{:<16}{r}  {}", "Plan:", info.plan);

    let has_usage = info.five_hour.is_some()
        || info.seven_day.is_some()
        || info.seven_day_sonnet.is_some();

    if has_usage {
        println!();
        println!("{b}Rate Limits{r}");

        let print_stat = |label: &str, stat: &UsageStats| {
            let bar = print_usage_bar(stat);
            let resets = format_resets_at(&stat.resets_at);
            let prev = stat.prev_utilization.map(|p| {
                let arrow = if p < stat.utilization { "↑" } else { "↓" };
                format!("  {d}(prev {:.1}% {arrow}){r}", p * 100.0, d = c_dim(), r = c_reset())
            }).unwrap_or_default();
            println!(
                "  {b}{:<16}{r}  {}  {d}resets {}{r}{}",
                label, bar, resets, prev,
                d = c_dim(), r = c_reset()
            );
        };

        if let Some(ref s) = info.five_hour {
            print_stat("5h window:", s);
        }
        if let Some(ref s) = info.seven_day {
            print_stat("7d window:", s);
        }
        if let Some(ref s) = info.seven_day_sonnet {
            print_stat("7d Sonnet:", s);
        }
    } else {
        println!();
        println!("  {d}No usage data available.{r}", d = c_dim(), r = c_reset());
    }
}

// ── fleet memory ──────────────────────────────────────────────────────────────

fn cmd_memory(file: Option<String>, as_json: bool) {
    let memories = memory::scan_all_memories();

    // If a specific file is requested, show its content
    if let Some(ref query) = file {
        // Try to find matching file: either by "workspace/filename" or path substring
        let mut found: Option<&memory::MemoryFile> = None;
        let mut found_ws: Option<&str> = None;

        for ws in &memories {
            for f in &ws.files {
                // Match by "workspace/filename"
                let ws_file = format!("{}/{}", ws.workspace_name, f.name);
                if ws_file == *query || f.name == *query || f.path.contains(query.as_str()) {
                    if found.is_some() && f.name != *query {
                        eprintln!(
                            "{}Error:{} ambiguous match '{}' — use workspace/filename to disambiguate",
                            "\x1b[31m", c_reset(), query
                        );
                        // List matches
                        for ws2 in &memories {
                            for f2 in &ws2.files {
                                let ws_file2 = format!("{}/{}", ws2.workspace_name, f2.name);
                                if ws_file2 == *query
                                    || f2.name == *query
                                    || f2.path.contains(query.as_str())
                                {
                                    eprintln!("  {}/{}", ws2.workspace_name, f2.name);
                                }
                            }
                        }
                        std::process::exit(1);
                    }
                    found = Some(f);
                    found_ws = Some(&ws.workspace_name);
                }
            }
        }

        match found {
            Some(f) => {
                match memory::read_memory_file(&f.path) {
                    Ok(content) => {
                        if as_json {
                            let obj = serde_json::json!({
                                "workspace": found_ws.unwrap_or(""),
                                "name": f.name,
                                "path": f.path,
                                "content": content,
                            });
                            println!("{}", serde_json::to_string_pretty(&obj).unwrap());
                        } else {
                            println!(
                                "{}{}  {}/{}{}",
                                c_bold(),
                                "\x1b[36m",
                                found_ws.unwrap_or(""),
                                f.name,
                                c_reset()
                            );
                            println!("{}{}{}", c_dim(), "─".repeat(60), c_reset());
                            println!("{}", content);
                        }
                    }
                    Err(e) => {
                        eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                        std::process::exit(1);
                    }
                }
            }
            None => {
                eprintln!(
                    "{}Error:{} no memory file matching '{}'",
                    "\x1b[31m",
                    c_reset(),
                    query
                );
                std::process::exit(1);
            }
        }
        return;
    }

    // List all memories
    if as_json {
        println!("{}", serde_json::to_string_pretty(&memories).unwrap());
        return;
    }

    if memories.is_empty() {
        println!("{}No memories found.{}", c_dim(), c_reset());
        return;
    }

    let total_files: usize = memories.iter().map(|w| w.files.len()).sum();
    println!(
        "{}Memories{} — {} workspace(s), {} file(s)\n",
        c_bold(),
        c_reset(),
        memories.len(),
        total_files
    );

    for ws in &memories {
        print!(
            "{}{}{}",
            c_bold(),
            ws.workspace_name,
            c_reset()
        );
        if ws.has_claude_md {
            print!("  {}\x1b[33mCLAUDE.md\x1b[0m{}", "", c_reset());
        }
        println!();

        for f in &ws.files {
            let size = if f.size_bytes < 1024 {
                format!("{}B", f.size_bytes)
            } else {
                format!("{:.1}K", f.size_bytes as f64 / 1024.0)
            };
            let age = format_age_ms(f.modified_ms);
            let name_style = if f.name == "MEMORY.md" {
                c_bold()
            } else {
                ""
            };
            let name_reset = if f.name == "MEMORY.md" {
                c_reset()
            } else {
                ""
            };
            println!(
                "  {}{}{}{} {:>6}  {}{}{}",
                name_style, f.name, name_reset,
                "",
                size,
                c_dim(), age, c_reset()
            );
        }
        println!();
    }
}

// ── fleet search ──────────────────────────────────────────────────────────────

fn cmd_search(query: &str, limit: usize, as_json: bool) {
    use claw_fleet_lib::search_index::SearchIndex;

    if query.trim().is_empty() {
        eprintln!("Error: search query cannot be empty");
        std::process::exit(1);
    }

    // Ensure the search index is up-to-date with all current sessions.
    let index = SearchIndex::open().unwrap_or_else(|e| {
        eprintln!("Error: cannot open search index: {e}");
        std::process::exit(1);
    });

    let sessions = load_sessions();
    let pairs: Vec<(String, String)> = sessions
        .iter()
        .map(|s| (s.jsonl_path.clone(), s.id.clone()))
        .collect();
    index.index_batch(&pairs);

    let hits = index.search(query, limit).unwrap_or_default();

    if as_json {
        println!("{}", serde_json::to_string_pretty(&hits).unwrap_or_default());
        return;
    }

    if hits.is_empty() {
        println!("No results for '{}'.", query);
        return;
    }

    // Enrich hits with workspace name from sessions
    let session_map: std::collections::HashMap<&str, &str> = sessions
        .iter()
        .map(|s| (s.id.as_str(), s.workspace_name.as_str()))
        .collect();

    let b = c_bold();
    let r = c_reset();
    let d = c_dim();

    println!("{b}Search results for '{query}'{r} — {} hit(s)\n", hits.len());

    for (i, hit) in hits.iter().enumerate() {
        let ws = session_map
            .get(hit.session_id.as_str())
            .copied()
            .unwrap_or("?");
        let snippet = hit
            .snippet
            .replace("<mark>", &format!("{b}"))
            .replace("</mark>", r);
        println!(
            "  {d}{}){r}  {b}{}{r}  {d}({}){r}",
            i + 1,
            ws,
            short_id(&hit.session_id),
        );
        // Show first 2 lines of snippet, trimmed
        for line in snippet.lines().take(2) {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                println!("     {}", truncate(trimmed, 100));
            }
        }
        println!();
    }
}

// ── fleet audit ───────────────────────────────────────────────────────────────

fn cmd_audit(min_level: &str, filter: Option<&str>, as_json: bool) {
    use claw_fleet_lib::audit::{extract_audit_events, AuditRiskLevel};

    let min = match min_level.to_lowercase().as_str() {
        "medium" => AuditRiskLevel::Medium,
        "high" => AuditRiskLevel::High,
        "critical" => AuditRiskLevel::Critical,
        other => {
            eprintln!("Error: unknown risk level '{}'. Use: medium, high, critical", other);
            std::process::exit(1);
        }
    };

    let sessions = load_sessions();
    let sources = build_sources();

    // Optionally filter sessions
    let filtered: Vec<&SessionInfo> = if let Some(needle) = filter {
        let n = needle.to_lowercase();
        sessions
            .iter()
            .filter(|s| {
                s.id.starts_with(needle)
                    || s.workspace_name.to_lowercase().contains(&n)
            })
            .collect()
    } else {
        // Default: non-idle sessions
        sessions
            .iter()
            .filter(|s| !matches!(s.status, SessionStatus::Idle))
            .collect()
    };

    let total = filtered.len();
    let mut all_events = Vec::new();

    for session in &filtered {
        let path = &session.jsonl_path;
        if let Some(source) = find_source_for_path(&sources, path) {
            if let Ok(messages) = source.get_messages(path) {
                let events = extract_audit_events(&messages, session);
                all_events.extend(events);
            }
        }
    }

    // Filter by minimum risk level
    all_events.retain(|e| e.risk_level >= min);
    all_events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    if as_json {
        let summary = serde_json::json!({
            "events": all_events,
            "totalSessionsScanned": total,
        });
        println!("{}", serde_json::to_string_pretty(&summary).unwrap_or_default());
        return;
    }

    if all_events.is_empty() {
        println!(
            "No risky commands found across {} session(s) (min level: {}).",
            total, min_level
        );
        return;
    }

    let b = c_bold();
    let r = c_reset();
    let d = c_dim();

    println!(
        "{b}Audit{r} — {} event(s) across {} session(s)  {d}(min: {}){r}\n",
        all_events.len(),
        total,
        min_level,
    );

    let risk_color = |level: &AuditRiskLevel| -> &'static str {
        if !use_color() { return ""; }
        match level {
            AuditRiskLevel::Critical => "\x1b[31m", // red
            AuditRiskLevel::High => "\x1b[33m",     // yellow
            AuditRiskLevel::Medium => "\x1b[36m",   // cyan
        }
    };

    let risk_label = |level: &AuditRiskLevel| -> &'static str {
        match level {
            AuditRiskLevel::Critical => "CRITICAL",
            AuditRiskLevel::High => "HIGH",
            AuditRiskLevel::Medium => "MEDIUM",
        }
    };

    for event in &all_events {
        let rc = risk_color(&event.risk_level);
        let rl = risk_label(&event.risk_level);
        let tags = event.risk_tags.join(", ");

        println!(
            "  {rc}{:<8}{r}  {b}{}{r}  {d}({}){r}  {d}[{}]{r}",
            rl,
            event.workspace_name,
            short_id(&event.session_id),
            tags,
        );
        println!("           {}", truncate(&event.command_summary, 90));
        println!();
    }
}

// ── fleet serve ────────────────────────────────────────────────────────────────

fn cmd_serve(port: u16, token: String) {
    use std::io::{Read, Seek, SeekFrom};
    use percent_encoding::percent_decode_str;
    use claw_fleet_lib::search_index::SearchIndex;

    use std::sync::{Arc, Mutex};
    use claw_fleet_lib::daily_report::{
        ReportStore, generate_report_from_sessions, scan_sessions_for_date, generate_ai_summary,
        generate_lessons, append_lesson_to_claude_md, Lesson,
    };

    let sources = build_sources();

    // Open the daily report store.
    let report_store = Arc::new(Mutex::new(
        ReportStore::open().expect("report store open"),
    ));

    // Open the full-text search index (stored on the remote host).
    let search_index = {
        let db_path = dirs::home_dir()
            .expect("cannot determine home dir")
            .join(".claude")
            .join("fleet-search.db");
        SearchIndex::open_at(&db_path).unwrap_or_else(|e| {
            eprintln!("[fleet serve] search index open failed, retrying fresh: {e}");
            let _ = std::fs::remove_file(&db_path);
            SearchIndex::open_at(&db_path).expect("search index open failed twice")
        })
    };

    let addr = format!("127.0.0.1:{}", port);
    let server = tiny_http::Server::http(&addr).unwrap_or_else(|e| {
        eprintln!("Error: cannot bind to {}: {}", addr, e);
        std::process::exit(1);
    });
    eprintln!("[fleet serve] listening on {} (version {})", addr, env!("CARGO_PKG_VERSION"));

    let expected_auth = format!("Bearer {}", token);

    for mut request in server.incoming_requests() {
        // Auth check
        let auth_ok = request
            .headers()
            .iter()
            .any(|h| h.field.equiv("authorization")
                && h.value.as_str() == expected_auth.as_str());

        if !auth_ok {
            let _ = request.respond(tiny_http::Response::empty(401));
            continue;
        }

        let url = request.url().to_string();
        let (path, query_str) = match url.split_once('?') {
            Some((p, q)) => (p, q),
            None => (url.as_str(), ""),
        };

        let query = parse_query(query_str);

        let json_header: tiny_http::Header = "Content-Type: application/json".parse().unwrap();

        match path {
            "/health" => {
                let body = format!(
                    r#"{{"version":"{}","status":"ok"}}"#,
                    env!("CARGO_PKG_VERSION")
                );
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/sessions" => {
                let sessions = scan_all_sources(&sources);
                // Incrementally update the search index with the latest session list.
                let pairs: Vec<(String, String)> = sessions
                    .iter()
                    .map(|s| (s.jsonl_path.clone(), s.id.clone()))
                    .collect();
                search_index.index_batch(&pairs);
                let body = serde_json::to_string(&sessions).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/stop" => {
                let pid: u32 = query.get("pid").and_then(|s| s.parse().ok()).unwrap_or(0);
                if pid == 0 {
                    let _ = request.respond(tiny_http::Response::empty(400));
                    continue;
                }
                let force: bool = query.get("force").map(|s| s == "true").unwrap_or(false);
                #[cfg(unix)]
                {
                    let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
                    let ret = unsafe { libc::kill(pid as libc::pid_t, signal) };
                    if ret == 0 {
                        let _ = request.respond(
                            tiny_http::Response::from_string(r#"{"ok":true}"#)
                                .with_header(json_header),
                        );
                    } else {
                        let err = std::io::Error::last_os_error().to_string();
                        let body = format!(r#"{{"error":"{}"}}"#, err);
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(500)
                                .with_header(json_header),
                        );
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = request.respond(tiny_http::Response::empty(400));
                }
            }

            "/stop_workspace" => {
                let workspace = query.get("path")
                    .map(|s| s.replace("%2F", "/"))
                    .unwrap_or_default();
                if workspace.is_empty() {
                    let _ = request.respond(tiny_http::Response::empty(400));
                    continue;
                }
                #[cfg(unix)]
                {
                    use claw_fleet_lib::session::scan_cli_processes;
                    let procs = scan_cli_processes();
                    let pids: Vec<u32> = procs.iter()
                        .filter(|p| p.cwd == workspace)
                        .map(|p| p.pid)
                        .collect();
                    for &pid in &pids {
                        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
                    }
                    let _ = request.respond(
                        tiny_http::Response::from_string(r#"{"ok":true}"#)
                            .with_header(json_header),
                    );
                }
                #[cfg(not(unix))]
                {
                    let _ = request.respond(tiny_http::Response::empty(400));
                }
            }

            // ── Unified /sources/{name}/account and /sources/{name}/usage ──
            _ if path.starts_with("/sources/") => {
                let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
                // Expected: ["sources", "<name>", "account"|"usage"]
                if parts.len() == 3 {
                    let source_name = parts[1];
                    let kind = parts[2];

                    // Check sources config before serving
                    let config = claw_fleet_lib::agent_source::SourcesConfig::load();
                    if !config.is_source_enabled(source_name) {
                        let body = format!(r#"{{"error":"Source '{}' is disabled"}}"#, source_name);
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(403)
                                .with_header(json_header),
                        );
                        continue;
                    }

                    if let Some(source) = agent_source::find_source_by_api_name(&sources, source_name) {
                        let result = match kind {
                            "account" => source.fetch_account(),
                            "usage" => source.fetch_usage(),
                            _ => Err(format!("Unknown endpoint: {kind}")),
                        };
                        match result {
                            Ok(val) => {
                                let body = serde_json::to_string(&val).unwrap_or_default();
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body).with_header(json_header),
                                );
                            }
                            Err(e) => {
                                let body = format!(r#"{{"error":"{}"}}"#, e.replace('"', "\\\""));
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body)
                                        .with_status_code(404)
                                        .with_header(json_header),
                                );
                            }
                        }
                    } else {
                        let body = format!(r#"{{"error":"Unknown source: {}"}}"#, source_name);
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(404)
                                .with_header(json_header),
                        );
                    }
                } else {
                    let _ = request.respond(tiny_http::Response::empty(404));
                }
            }

            "/setup-status" => {
                let sessions = scan_all_sources(&sources);
                let detected_tools = claw_fleet_lib::detect_installed_tools(&sessions);
                let (cli_installed, cli_path) = claw_fleet_lib::check_cli_installed();
                let claude_dir_exists = get_claude_dir().map_or(false, |d| d.is_dir());
                let logged_in = claw_fleet_lib::account::read_keychain_credentials().is_ok();
                let has_sessions = !sessions.is_empty();

                let status = claw_fleet_lib::backend::SetupStatus {
                    cli_installed,
                    cli_path,
                    claude_dir_exists,
                    detected_tools,
                    logged_in,
                    has_sessions,
                    credentials_valid: None,
                };
                let body = serde_json::to_string(&status).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/usage_summaries" => {
                let summaries = claw_fleet_lib::local_backend::fetch_usage_summaries_from_sources(&sources);
                let body = serde_json::to_string(&summaries).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/messages" => {
                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                if let Some(source) = find_source_for_path(&sources, &file_path) {
                    match source.get_messages(&file_path) {
                        Ok(messages) => {
                            let body = serde_json::to_string(&messages).unwrap_or_default();
                            let _ = request.respond(
                                tiny_http::Response::from_string(body).with_header(json_header),
                            );
                        }
                        Err(_) => {
                            let _ = request.respond(tiny_http::Response::empty(404));
                        }
                    }
                } else {
                    let _ = request.respond(tiny_http::Response::empty(404));
                }
            }

            "/file_size" => {
                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let uri = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                let resolved = find_source_for_path(&sources, &uri)
                    .and_then(|s| s.resolve_file_path(&uri));
                let size = resolved
                    .and_then(|p| std::fs::metadata(&p).ok())
                    .map(|m| m.len())
                    .unwrap_or(0);
                let body = format!(r#"{{"size":{}}}"#, size);
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/tail" => {
                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let uri = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                let offset: u64 = query
                    .get("offset")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);

                let resolved = find_source_for_path(&sources, &uri)
                    .and_then(|s| s.resolve_file_path(&uri));
                let resolved = match resolved {
                    Some(p) => p,
                    None => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                        continue;
                    }
                };

                match std::fs::File::open(&resolved) {
                    Ok(mut file) => {
                        let file_size = file.metadata().map(|m| m.len()).unwrap_or(0);
                        if file_size <= offset {
                            let body = format!(r#"{{"lines":[],"newOffset":{}}}"#, offset);
                            let _ = request.respond(
                                tiny_http::Response::from_string(body).with_header(json_header),
                            );
                        } else {
                            let _ = file.seek(SeekFrom::Start(offset));
                            let mut buf = String::new();
                            let _ = file.read_to_string(&mut buf);
                            let lines: Vec<serde_json::Value> = buf
                                .lines()
                                .filter(|l| !l.trim().is_empty())
                                .filter_map(|l| serde_json::from_str(l).ok())
                                .collect();
                            let body = serde_json::json!({
                                "lines": lines,
                                "newOffset": file_size
                            })
                            .to_string();
                            let _ = request.respond(
                                tiny_http::Response::from_string(body).with_header(json_header),
                            );
                        }
                    }
                    Err(_) => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                    }
                }
            }

            "/memories" => {
                let mut memories = Vec::new();
                for source in &sources {
                    memories.extend(source.list_memories());
                }
                let body = serde_json::to_string(&memories).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/memory_content" => {
                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                // Try each source for memory content; fall back to direct read for Claude Code
                let result = sources.iter()
                    .find_map(|s| s.get_memory_content(&file_path).ok())
                    .or_else(|| memory::read_memory_file(&file_path).ok());
                match result {
                    Some(content) => {
                        let body = serde_json::to_string(&content).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    None => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                    }
                }
            }

            "/memory_history" => {
                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                // Aggregate history from all sources; fall back to direct trace
                let mut history = Vec::new();
                for source in &sources {
                    let h = source.get_memory_history(&file_path);
                    if !h.is_empty() {
                        history = h;
                        break;
                    }
                }
                if history.is_empty() {
                    history = memory::trace_memory_history(&file_path);
                }
                let body = serde_json::to_string(&history).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/skills" => {
                let items = skills::scan_all_skills();
                let body = serde_json::to_string(&items).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/skill_content" => {
                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                match skills::read_skill_file(&file_path) {
                    Ok(content) => {
                        let body = serde_json::to_string(&content).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    Err(_) => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                    }
                }
            }

            "/hooks_plan" => {
                let plan = hooks::plan_hook_setup();
                let body = serde_json::to_string(&plan).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/apply_hooks" => {
                match hooks::apply_hook_setup() {
                    Ok(()) => {
                        let _ = request.respond(
                            tiny_http::Response::from_string(r#"{"ok":true}"#)
                                .with_header(json_header),
                        );
                    }
                    Err(e) => {
                        let body = serde_json::json!({"error": e}).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(500)
                                .with_header(json_header),
                        );
                    }
                }
            }

            "/sources_config" => {
                let info = agent_source::get_sources_config_local();
                let body = serde_json::to_string(&info).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/set_source_enabled" => {
                let name = query.get("name").cloned().unwrap_or_default();
                let enabled: bool = query.get("enabled").map(|s| s == "true").unwrap_or(false);
                if name.is_empty() {
                    let _ = request.respond(
                        tiny_http::Response::from_string(r#"{"error":"missing name param"}"#)
                            .with_status_code(400)
                            .with_header(json_header),
                    );
                } else {
                    match agent_source::set_source_enabled_local(&name, enabled) {
                        Ok(()) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string(r#"{"ok":true}"#)
                                    .with_header(json_header),
                            );
                        }
                        Err(e) => {
                            let body = serde_json::json!({"error": e}).to_string();
                            let _ = request.respond(
                                tiny_http::Response::from_string(body)
                                    .with_status_code(500)
                                    .with_header(json_header),
                            );
                        }
                    }
                }
            }

            "/remove_hooks" => {
                match hooks::remove_fleet_hooks() {
                    Ok(()) => {
                        let _ = request.respond(
                            tiny_http::Response::from_string(r#"{"ok":true}"#)
                                .with_header(json_header),
                        );
                    }
                    Err(e) => {
                        let body = serde_json::json!({"error": e}).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(500)
                                .with_header(json_header),
                        );
                    }
                }
            }

            "/search" => {
                let q = query.get("q").cloned().unwrap_or_default();
                let limit: usize = query
                    .get("limit")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(50);
                let hits = search_index.search(&q, limit).unwrap_or_default();
                let body = serde_json::to_string(&hits).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/audit" => {
                use claw_fleet_lib::audit::extract_audit_events;
                let sessions = scan_all_sources(&sources);
                let active: Vec<&SessionInfo> = sessions
                    .iter()
                    .filter(|s| !matches!(s.status, SessionStatus::Idle))
                    .collect();
                let total = active.len();
                let mut all_events = Vec::new();
                for session in &active {
                    let path = &session.jsonl_path;
                    if let Some(src) = find_source_for_path(&sources, path) {
                        if let Ok(messages) = src.get_messages(path) {
                            let events = extract_audit_events(&messages, session);
                            all_events.extend(events);
                        }
                    }
                }
                all_events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
                let summary = claw_fleet_lib::audit::AuditSummary {
                    events: all_events,
                    total_sessions_scanned: total,
                };
                let body = serde_json::to_string(&summary).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/audit/pattern-info" => {
                let (version, path) = claw_fleet_lib::pattern_update::get_patterns_info();
                let body = serde_json::json!({
                    "version": version,
                    "path": path,
                }).to_string();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/audit/check-update" => {
                let msg = claw_fleet_lib::pattern_update::check_update_now();
                let body = serde_json::json!({ "message": msg }).to_string();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/daily_report" => {
                let date = query.get("date").cloned().unwrap_or_default();
                let store = report_store.lock().unwrap();
                match store.get_report(&date) {
                    Ok(report) => {
                        let body = serde_json::to_string(&report).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    Err(e) => {
                        let body = format!("{{\"error\":\"{}\"}}", e);
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(500)
                                .with_header(json_header),
                        );
                    }
                }
            }

            "/daily_report_stats" => {
                let from = query.get("from").cloned().unwrap_or_default();
                let to = query.get("to").cloned().unwrap_or_default();
                let store = report_store.lock().unwrap();
                let stats = store.list_stats(&from, &to).unwrap_or_default();
                let body = serde_json::to_string(&stats).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/daily_report/generate" => {
                let date = query.get("date").cloned().unwrap_or_default();
                let sessions = scan_sessions_for_date(&date);
                if sessions.is_empty() {
                    let body = r#"{"error":"no sessions found for date"}"#;
                    let _ = request.respond(
                        tiny_http::Response::from_string(body)
                            .with_status_code(404)
                            .with_header(json_header),
                    );
                } else {
                    let session_refs: Vec<&SessionInfo> = sessions.iter().collect();
                    let tz = chrono::Local::now().format("%Z").to_string();
                    let report = generate_report_from_sessions(&date, &tz, &session_refs);
                    report_store.lock().unwrap().save_report(&report).ok();
                    let body = serde_json::to_string(&report).unwrap_or_default();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body).with_header(json_header),
                    );
                }
            }

            "/daily_report/ai_summary" => {
                let date = query.get("date").cloned().unwrap_or_default();
                let lang = query.get("lang").map(|s| s.as_str()).unwrap_or("en");
                let store = report_store.lock().unwrap();
                match store.get_report(&date) {
                    Ok(Some(report)) => {
                        drop(store);
                        match generate_ai_summary(&report, lang) {
                            Some(summary) => {
                                report_store
                                    .lock()
                                    .unwrap()
                                    .update_ai_summary(&date, &summary)
                                    .ok();
                                let body = serde_json::to_string(&summary).unwrap_or_default();
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body)
                                        .with_header(json_header),
                                );
                            }
                            None => {
                                let body = r#"{"error":"AI summary generation failed"}"#;
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body)
                                        .with_status_code(500)
                                        .with_header(json_header),
                                );
                            }
                        }
                    }
                    _ => {
                        let body = r#"{"error":"report not found"}"#;
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(404)
                                .with_header(json_header),
                        );
                    }
                }
            }

            "/daily_report/lessons" => {
                let date = query.get("date").cloned().unwrap_or_default();
                let lang = query.get("lang").map(|s| s.as_str()).unwrap_or("en");
                let store = report_store.lock().unwrap();
                match store.get_report(&date) {
                    Ok(Some(report)) => {
                        drop(store);
                        match generate_lessons(&report, lang) {
                            Some(lessons) => {
                                report_store
                                    .lock()
                                    .unwrap()
                                    .update_lessons(&date, &lessons)
                                    .ok();
                                let body = serde_json::to_string(&lessons).unwrap_or_default();
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body)
                                        .with_header(json_header),
                                );
                            }
                            None => {
                                let body = r#"{"error":"Lessons generation failed"}"#;
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body)
                                        .with_status_code(500)
                                        .with_header(json_header),
                                );
                            }
                        }
                    }
                    _ => {
                        let body = r#"{"error":"report not found"}"#;
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(404)
                                .with_header(json_header),
                        );
                    }
                }
            }

            "/daily_report/append_lesson" => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<Lesson>(&body_bytes) {
                    Ok(lesson) => match append_lesson_to_claude_md(&lesson) {
                        Ok(()) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string("{}")
                                    .with_header(json_header),
                            );
                        }
                        Err(e) => {
                            let body = format!(r#"{{"error":"{}"}}"#, e.replace('"', "'"));
                            let _ = request.respond(
                                tiny_http::Response::from_string(body)
                                    .with_status_code(500)
                                    .with_header(json_header),
                            );
                        }
                    },
                    Err(e) => {
                        let body = format!(r#"{{"error":"invalid lesson: {}"}}"#, e.to_string().replace('"', "'"));
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(400)
                                .with_header(json_header),
                        );
                    }
                }
            }

            _ => {
                let _ = request.respond(tiny_http::Response::empty(404));
            }
        }
    }
}

fn parse_query(query_str: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for pair in query_str.split('&') {
        if pair.is_empty() {
            continue;
        }
        match pair.split_once('=') {
            Some((k, v)) => {
                map.insert(k.to_string(), v.to_string());
            }
            None => {
                map.insert(pair.to_string(), String::new());
            }
        }
    }
    map
}

fn cmd_speed(as_json: bool) {
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

// ── fleet skill ────────────────────────────────────────────────────────────────

fn cmd_skill_install() {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            eprintln!("Error: cannot determine home directory");
            std::process::exit(1);
        });

    let b = c_bold();
    let mut any = false;

    for (name, dir) in SKILL_TARGETS {
        let tool_home = home.join(dir);
        if !tool_home.exists() {
            continue;
        }
        let skill_dir = tool_home.join("skills").join("fleet");
        let skill_path = skill_dir.join("SKILL.md");
        match std::fs::create_dir_all(&skill_dir)
            .and_then(|_| std::fs::write(&skill_path, FLEET_SKILL_MD))
        {
            Ok(_) => {
                println!("  {b}✓{r}  {name}  {d}{}{r}", skill_path.display(), d = c_dim(), r = c_reset());
                any = true;
            }
            Err(e) => {
                eprintln!("  ✗  {name}: {e}");
            }
        }
    }

    if !any {
        eprintln!("No supported AI tools detected. Install Claude Code, GitHub Copilot, or Gemini CLI first.");
        std::process::exit(1);
    }
}

// ── Daily report CLI ────────────────────────────────────────────────────────

fn cmd_report(date: Option<String>, backfill: bool, regenerate: bool, gen_lessons: bool, gen_summary: bool, as_json: bool, lang: &str) {
    use claw_fleet_lib::daily_report::{
        ReportStore, generate_report_from_sessions, scan_sessions_for_date,
        generate_lessons, generate_ai_summary,
    };

    let store = ReportStore::open().expect("cannot open report store");

    if backfill {
        let today = chrono::Local::now();
        for days_ago in 1..=90 {
            let date = (today - chrono::Duration::days(days_ago))
                .format("%Y-%m-%d")
                .to_string();
            if store.get_report(&date).ok().flatten().is_some() {
                continue;
            }
            let sessions = scan_sessions_for_date(&date);
            if sessions.is_empty() {
                continue;
            }
            let session_refs: Vec<_> = sessions.iter().collect();
            let tz = chrono::Local::now().format("%Z").to_string();
            let report = generate_report_from_sessions(&date, &tz, &session_refs);
            store.save_report(&report).ok();
            println!(
                "Generated report for {}: {} sessions, {} tokens",
                date,
                report.metrics.total_sessions,
                report.metrics.total_input_tokens + report.metrics.total_output_tokens
            );
        }
        println!("Backfill complete.");
        return;
    }

    let target_date = date.unwrap_or_else(|| {
        (chrono::Local::now() - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string()
    });

    if regenerate {
        let sessions = scan_sessions_for_date(&target_date);
        if sessions.is_empty() {
            eprintln!("No sessions found for {}", target_date);
            std::process::exit(1);
        }
        let session_refs: Vec<_> = sessions.iter().collect();
        let tz = chrono::Local::now().format("%Z").to_string();
        let report = generate_report_from_sessions(&target_date, &tz, &session_refs);
        store.save_report(&report).ok();
        println!("Regenerated report for {}", target_date);
    }

    if gen_summary {
        match store.get_report(&target_date) {
            Ok(Some(report)) => {
                eprint!("Generating AI summary (may take up to 2 minutes)...");
                match generate_ai_summary(&report, lang) {
                    Some(summary) => {
                        eprintln!(" done");
                        store.update_ai_summary(&target_date, &summary).ok();
                        if as_json {
                            println!("{}", serde_json::to_string_pretty(&summary).unwrap());
                        } else {
                            println!("{summary}");
                        }
                    }
                    None => {
                        eprintln!(" failed (claude CLI unavailable or timed out)");
                        std::process::exit(1);
                    }
                }
            }
            Ok(None) => {
                eprintln!("No report for {}. Use --regenerate first.", target_date);
                std::process::exit(1);
            }
            Err(e) => { eprintln!("Error: {}", e); std::process::exit(1); }
        }
        if !gen_lessons { return; }
    }

    if gen_lessons {
        match store.get_report(&target_date) {
            Ok(Some(report)) => {
                eprint!("Generating lessons (may take up to 3 minutes)...");
                match generate_lessons(&report, lang) {
                    Some(lessons) => {
                        eprintln!(" done ({} lessons found)", lessons.len());
                        store.update_lessons(&target_date, &lessons).ok();
                        if as_json {
                            println!("{}", serde_json::to_string_pretty(&lessons).unwrap());
                            return;
                        }
                        print_lessons(&lessons);
                    }
                    None => {
                        eprintln!(" failed (claude CLI unavailable or timed out)");
                        std::process::exit(1);
                    }
                }
            }
            Ok(None) => {
                eprintln!("No report for {}. Use --regenerate first.", target_date);
                std::process::exit(1);
            }
            Err(e) => { eprintln!("Error: {}", e); std::process::exit(1); }
        }
        return;
    }

    match store.get_report(&target_date) {
        Ok(Some(report)) => {
            if as_json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
            } else {
                print_report(&report);
            }
        }
        Ok(None) => {
            eprintln!(
                "No report for {}. Use --regenerate to generate.",
                target_date
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

fn print_lessons(lessons: &[claw_fleet_lib::daily_report::Lesson]) {
    let b = c_bold();
    let d = c_dim();
    let r = c_reset();

    if lessons.is_empty() {
        println!("No AI mistakes found in this day's sessions.");
        return;
    }
    println!("{b}Lessons Learned{r}\n");
    for (i, lesson) in lessons.iter().enumerate() {
        println!("{}. {b}{}{r}", i + 1, lesson.content);
        println!("   {d}Why:{r} {}", lesson.reason);
        println!("   {d}From:{r} {} / {}", lesson.workspace_name, lesson.session_id);
        println!();
    }
}

fn print_report(report: &claw_fleet_lib::daily_report::DailyReport) {
    let b = c_bold();
    let d = c_dim();
    let r = c_reset();

    println!("{b}Daily Report \u{2014} {}{r}", report.date);
    println!();
    println!("  Sessions:    {}", report.metrics.total_sessions);
    println!("  Subagents:   {}", report.metrics.total_subagents);
    println!(
        "  Tokens:      {} in / {} out",
        format_tokens(report.metrics.total_input_tokens),
        format_tokens(report.metrics.total_output_tokens)
    );
    println!("  Tool calls:  {}", report.metrics.total_tool_calls);
    println!();

    if !report.metrics.tool_call_breakdown.is_empty() {
        println!("{b}Tool Calls{r}");
        let mut tools: Vec<_> = report.metrics.tool_call_breakdown.iter().collect();
        tools.sort_by(|a, b| b.1.cmp(a.1));
        for (tool, count) in tools {
            println!("  {tool:<20} {count}");
        }
        println!();
    }

    for proj in &report.metrics.projects {
        println!(
            "{b}{}{r} {d}({}){r}",
            proj.workspace_name, proj.workspace_path
        );
        println!(
            "  {} sessions, {} tool calls, {} tokens",
            proj.session_count,
            proj.tool_calls,
            format_tokens(proj.total_input_tokens + proj.total_output_tokens)
        );
        for s in &proj.sessions {
            let title = s.title.as_deref().unwrap_or("(untitled)");
            let sub = if s.is_subagent { " [sub]" } else { "" };
            println!(
                "  {d}\u{2022}{r} {title}{sub} {d}({}){r}",
                format_tokens(s.output_tokens)
            );
        }
        println!();
    }

    if let Some(ref summary) = report.ai_summary {
        println!("{b}AI Summary{r}");
        println!("{}", summary);
    }
}
