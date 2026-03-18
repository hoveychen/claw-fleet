use clap::{Parser, Subcommand};
use std::time::{SystemTime, UNIX_EPOCH};

use claude_fleet_lib::account::{fetch_account_info, AccountInfo, UsageStats};
use claude_fleet_lib::session::{get_claude_dir, scan_sessions, SessionInfo, SessionStatus};
use claude_fleet_lib::{FLEET_SKILL_MD, SKILL_TARGETS};

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
    about = "Claude Fleet CLI — monitor Claude Code agents",
    long_about = None
)]
struct Cli {
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
    /// Start the HTTP probe server (used by Fleet app for remote monitoring)
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value = "7007")]
        port: u16,
        /// Authentication token (required)
        #[arg(long)]
        token: String,
    },
    /// Manage Fleet skill for AI coding tools
    Skill {
        #[command(subcommand)]
        action: SkillCommands,
    },
}

#[derive(Subcommand)]
enum SkillCommands {
    /// Install Fleet skill to all detected AI tools (Claude Code, Cursor, Copilot, Gemini CLI)
    Install,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Agents { all, json } => cmd_agents(all, json),
        Commands::Agent { id, json } => cmd_agent(&id, json),
        Commands::Stop { id, force } => cmd_stop(&id, force),
        Commands::Account { json } => cmd_account(json),
        Commands::Speed { json } => cmd_speed(json),
        Commands::Serve { port, token } => cmd_serve(port, token),
        Commands::Skill { action } => match action {
            SkillCommands::Install => cmd_skill_install(),
        },
    }
}

// ── Commands ───────────────────────────────────────────────────────────────────

fn load_sessions() -> Vec<SessionInfo> {
    let claude_dir = match get_claude_dir() {
        Some(d) => d,
        None => {
            eprintln!("Error: cannot locate ~/.claude directory");
            std::process::exit(1);
        }
    };
    scan_sessions(&claude_dir)
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
        "{b}{:<10}{r}  {b}{:<20}{r}  {b}{:<10}{r}  {b}{:>8}{r}  {b}{:>7}{r}  {b}{}{r}",
        "ID", "WORKSPACE", "STATUS", "SPEED", "TOKENS", "MODEL"
    );
    println!("{}", "─".repeat(72));

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

        println!(
            "{:<10}  {:<20}  {sc}{:<10}{r}  {:>8}  {:>7}  {}",
            id_display,
            ws,
            status_str,
            format_speed(s.token_speed),
            format_tokens(s.total_output_tokens),
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

    let Some(pid) = s.pid else {
        eprintln!(
            "Agent {} ({}) has no associated PID — cannot stop.",
            short_id(&s.id),
            s.workspace_name
        );
        std::process::exit(1);
    };

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

// ── fleet serve ────────────────────────────────────────────────────────────────

fn cmd_serve(port: u16, token: String) {
    use std::io::{Read, Seek, SeekFrom};
    use percent_encoding::percent_decode_str;

    let claude_dir = match get_claude_dir() {
        Some(d) => d,
        None => {
            eprintln!("Error: cannot locate ~/.claude directory");
            std::process::exit(1);
        }
    };

    let addr = format!("127.0.0.1:{}", port);
    let server = tiny_http::Server::http(&addr).unwrap_or_else(|e| {
        eprintln!("Error: cannot bind to {}: {}", addr, e);
        std::process::exit(1);
    });
    eprintln!("[fleet serve] listening on {} (version {})", addr, env!("CARGO_PKG_VERSION"));

    let expected_auth = format!("Bearer {}", token);

    for request in server.incoming_requests() {
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
                let sessions = scan_sessions(&claude_dir);
                let body = serde_json::to_string(&sessions).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/messages" => {
                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                match std::fs::read_to_string(&file_path) {
                    Ok(content) => {
                        let messages: Vec<serde_json::Value> = content
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .filter_map(|l| serde_json::from_str(l).ok())
                            .collect();
                        let body = serde_json::to_string(&messages).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    Err(_) => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                    }
                }
            }

            "/file_size" => {
                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                let size = std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);
                let body = format!(r#"{{"size":{}}}"#, size);
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/tail" => {
                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                let offset: u64 = query
                    .get("offset")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);

                match std::fs::File::open(&file_path) {
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
        eprintln!("No supported AI tools detected. Install Claude Code, Cursor, GitHub Copilot, or Gemini CLI first.");
        std::process::exit(1);
    }
}
