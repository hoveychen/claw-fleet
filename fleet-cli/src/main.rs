use clap::{Parser, Subcommand};
use std::time::{SystemTime, UNIX_EPOCH};

mod feishu;
mod fleet_cli_host;
mod task_runtime;

use claw_fleet_core::account::{fetch_account_info_blocking as fetch_account_info, AccountInfo, UsageStats};
use claw_fleet_core::agent_source::{build_sources, find_source_for_path};
use claw_fleet_core::memory;
use claw_fleet_core::session::{scan_all_sources, SessionInfo, SessionStatus};
use claw_fleet_core::wiki;
use claw_fleet_core::{FLEET_SKILL_MD, SKILL_TARGETS};

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
        SessionStatus::RateLimited => "\x1b[31m",
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
        SessionStatus::RateLimited => "RateLimit",
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
    /// Publish and browse docs in the Fleet wiki knowledge base (~/.fleet/wiki)
    Wiki {
        #[command(subcommand)]
        action: WikiCommands,
    },
    /// Start the HTTP probe server (used by Fleet app for remote monitoring)
    Serve {
        /// Port to listen on.  Pass 0 to let the OS pick a free ephemeral port;
        /// use `--port-file` to capture the actual port the server bound to.
        #[arg(short, long, default_value = "7007")]
        port: u16,
        /// Authentication token (required)
        #[arg(long)]
        token: String,
        /// If set, the actual bound port is written to this file after bind.
        #[arg(long)]
        port_file: Option<std::path::PathBuf>,
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
    /// [internal] Guard hook — intercepts critical commands for user approval.
    /// With no subcommand: runs the hook stub (reads stdin from Claude Code).
    /// With a subcommand: manage persisted guard allow-list rules.
    #[command(hide = true)]
    Guard {
        #[command(subcommand)]
        action: Option<GuardCommands>,
    },
    /// [internal] Elicitation hook — intercepts AskUserQuestion for Fleet UI
    #[command(hide = true)]
    Elicitation,
    /// [internal] MCP server — exposes `fleet__ask` over stdio JSON-RPC
    #[command(hide = true)]
    Mcp,
    /// [internal] Plan-approval hook — intercepts ExitPlanMode for Fleet UI
    #[command(hide = true)]
    PlanApproval,
    /// [internal] PRD-context hook — re-injects the workspace's TASKS.md on every UserPromptSubmit
    #[command(hide = true)]
    PrdContext,
    /// Manage the current fleet-managed session (called from inside a Claude session)
    Session {
        #[command(subcommand)]
        action: SessionCommands,
    },
    /// Manage tasks. Master agent tools: `get-plan`, `get-dispatchable`,
    /// `dispatch`, `mark-done`, `mark-failed`, `update-plan`. Shell utility:
    /// `list`, `clear`. Process lifecycle (pause/resume/kill) lives with the
    /// owning fleet-task process — signal its pid or use the TUI.
    Task {
        #[command(subcommand)]
        action: TaskCommands,
    },
    /// Manage this workspace's TASKS.md PRD plans (PRD Discipline mode). Use
    /// these instead of hand-editing TASKS.md so Fleet records which session is
    /// working which plan/P, with a timestamp. Reads FLEET_SESSION_ID.
    Plan {
        #[command(subcommand)]
        action: PlanCommands,
    },
    /// Register a session handoff (接力): when this session next ends its
    /// turn, Fleet spawns a fresh successor session in the same workspace to
    /// continue the work, opening with your --note. Use when your context is
    /// running long mid-plan. Reads FLEET_SESSION_ID / CLAUDE_CODE_SESSION_ID.
    Handoff {
        /// Relay note for the successor (required): what's done, what's next,
        /// key files, gotchas.
        #[arg(long)]
        note: Option<String>,
        /// TASKS.md plan id the successor should continue (optional).
        #[arg(long)]
        plan: Option<String>,
        /// Next P label for --plan, e.g. `P4` (optional).
        #[arg(long)]
        next: Option<String>,
        #[command(subcommand)]
        action: Option<HandoffCommands>,
    },
    /// Manage PRD Discipline mode guidance (the generated
    /// ~/.claude/fleet-prd-discipline.md + its @import in ~/.claude/CLAUDE.md).
    PrdDiscipline {
        #[command(subcommand)]
        action: PrdDisciplineCommands,
    },
    /// [internal] Own one task's lifecycle in this process (master + workers +
    /// plan). The desktop spawns `fleet task-runtime resume <id>`; bare
    /// invocation opens the launchpad. Was the standalone `fleet-task` binary.
    #[command(hide = true)]
    TaskRuntime {
        #[command(subcommand)]
        cmd: Option<TaskRuntimeCommand>,
    },
}

#[derive(Subcommand, Debug)]
enum TaskRuntimeCommand {
    /// Start a new task in the given workspace and spawn its master agent.
    New {
        /// Project workspace directory (must be a git repo).
        #[arg(long)]
        workspace: std::path::PathBuf,
        /// Initial task description / prompt for the master agent.
        #[arg(long)]
        prompt: String,
        /// Task title; defaults to the prompt's first 40 chars.
        #[arg(long)]
        title: Option<String>,
        /// Skip the TUI; run as a headless HTTP-only daemon.
        #[arg(long, default_value_t = false)]
        no_tui: bool,
    },
    /// Resume an existing task by id (`~/.fleet/tasks/<id>.json`).
    Resume {
        /// Task id to resume.
        task_id: String,
        /// Workspace directory.
        #[arg(long)]
        workspace: std::path::PathBuf,
        #[arg(long, default_value_t = false)]
        no_tui: bool,
    },
    /// Open a TUI picker over all tasks on disk; Enter resumes the highlighted
    /// task.
    List {
        /// Override the workspace path for the resumed task.
        #[arg(long)]
        workspace: Option<std::path::PathBuf>,
        #[arg(long, default_value_t = false)]
        no_tui: bool,
    },
}

#[derive(Subcommand)]
enum TaskCommands {
    /// Print the task's current plan as YAML. Master tool.
    GetPlan {
        task_id: String,
    },
    /// Print the IDs of P-items the scheduler can dispatch right now.
    /// Master tool.
    GetDispatchable {
        task_id: String,
        /// Output raw JSON instead of one-id-per-line.
        #[arg(long)]
        json: bool,
    },
    /// Request the supervisor to dispatch a worker for `p_id`. Master tool.
    Dispatch {
        task_id: String,
        p_id: String,
    },
    /// Replace the task's plan with a new YAML document. Reads from stdin
    /// when `--from-stdin`, otherwise from `--file <path>`. Used by the
    /// planning session to write the DAG.
    UpdatePlan {
        task_id: String,
        #[arg(long)]
        from_stdin: bool,
        #[arg(long)]
        file: Option<std::path::PathBuf>,
    },
    /// Delete a task's json + materials dir. **Does not touch any running
    /// fleet-task process** — kill that separately (e.g. SIGTERM the pid in
    /// `~/.fleet/runtime/<task_id>.json`) before clearing if you want a
    /// clean teardown.
    Clear { task_id: String },
    /// List all tasks on disk with their status, plan progress, and whether
    /// they're backed by a live fleet-task process. Shell-friendly: pipe
    /// `--json` into `jq` to grab task IDs by status.
    List {
        /// Only list tasks for this project_id.
        #[arg(long)]
        project_id: Option<String>,
        /// Output raw JSON (one task per record).
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SkillCommands {
    /// Install Fleet skill to all detected AI tools (Claude Code, Copilot, Gemini CLI)
    Install,
}

#[derive(Subcommand, Debug)]
enum GuardCommands {
    /// List persisted "always allow" rules for the Bash guard.
    ListRules {
        /// Output raw JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Remove an "always allow" rule by its id (use `list-rules` to find it).
    RemoveRule {
        /// The rule id (UUID).
        id: String,
    },
}

#[derive(Subcommand)]
enum SessionCommands {
    /// Report the current session's status (e.g. complete, in-review, custom-column-name).
    /// Reads FLEET_SESSION_ID from env. If the status matches one of the project's
    /// kanban_columns, the card moves; otherwise the value is recorded as a free-text note.
    Status {
        /// New status value (kanban column id or any free-text label).
        state: String,
        /// Optional note to attach (overrides status if status is unknown).
        #[arg(long)]
        note: Option<String>,
    },
    /// [internal] Mark the current fleet session idle (turn ended, awaiting next user prompt).
    /// Wired to Claude Code's `Stop` hook by `apply_idle_hooks`. Silent no-op if
    /// FLEET_SESSION_ID is unset (non-fleet-managed sessions running with global hooks).
    #[command(hide = true)]
    Idle,
    /// [internal] Clear the idle sentinel for the current session (next prompt arrived).
    /// Wired to Claude Code's `UserPromptSubmit` hook.
    #[command(hide = true)]
    Resume,
}

#[derive(Subcommand)]
enum HandoffCommands {
    /// Cancel this session's pending handoff.
    Cancel,
    /// List recorded handoff chains.
    List,
    /// Show the relay chain containing a session id.
    Show { session_id: String },
}

#[derive(Subcommand)]
enum PlanCommands {
    /// Tick a task done (`[ ]`→`[x]`) and record this session's focus.
    Check {
        /// Plan id (the sentinel `id="..."`).
        plan_id: String,
        /// Task label, e.g. `P3` (matched as the bold token `**P3**`).
        task: String,
    },
    /// Untick a task (`[x]`→`[ ]`).
    Uncheck { plan_id: String, task: String },
    /// Declare this session is now focused on a plan (records focus only; no
    /// file change). Optional task label sets the current P explicitly.
    Start {
        plan_id: String,
        task: Option<String>,
    },
    /// Create a new empty v2 plan block in the workspace TASKS.md.
    Create {
        plan_id: String,
        #[arg(long)]
        title: String,
    },
    /// Append a pending task to a plan.
    Add {
        plan_id: String,
        /// Task label, e.g. `P4`.
        task: String,
        #[arg(long)]
        text: String,
    },
    /// Migrate v1 TASKS.md blocks to v2 in place (idempotent). Defaults to the
    /// workspace's main TASKS.md; pass a path to target a specific file.
    Migrate {
        #[arg(long)]
        path: Option<std::path::PathBuf>,
    },
    /// List all plans in the workspace with progress.
    List,
    /// Print one plan's tasks.
    Get { plan_id: String },
}

#[derive(Subcommand)]
enum WikiCommands {
    /// Publish a .html file, a .md file, or a directory containing an HTML
    /// entry. Re-publishing an existing slug creates a new version (old
    /// versions are kept and stay browsable in the Fleet app).
    Publish {
        /// Path to the .html file, .md file, or directory
        path: std::path::PathBuf,
        /// Stable doc id (lowercase letters/digits/hyphens). Default:
        /// normalized file/dir name.
        #[arg(long)]
        slug: Option<String>,
        /// Display title. Default: <title> tag / first `# ` heading / slug.
        #[arg(long)]
        title: Option<String>,
        /// Workspace to tag the doc with. Default: current directory.
        #[arg(long)]
        workspace: Option<std::path::PathBuf>,
        /// Output the published doc metadata as JSON
        #[arg(long)]
        json: bool,
    },
    /// List all wiki docs
    #[command(alias = "ls")]
    List {
        /// Filter by workspace path substring
        #[arg(long)]
        workspace: Option<String>,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Show one doc's metadata and version history
    Show {
        slug: String,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove a doc (all versions), or a single old version with --version
    Rm {
        slug: String,
        /// Remove only this version (refused for the current version)
        #[arg(long)]
        version: Option<String>,
    },
    /// Retag docs whose workspace was recorded as a session scratchpad
    /// directory (published before scratchpad decoding existed)
    FixWorkspaces {
        /// Output the fixed docs as JSON
        #[arg(long)]
        json: bool,
    },
    /// Manage the agent-guidance block in ~/.claude/CLAUDE.md that tells
    /// agents to publish durable reports/demos via `fleet wiki publish`
    Guidance {
        #[command(subcommand)]
        action: WikiGuidanceCommands,
    },
}

#[derive(Subcommand)]
enum WikiGuidanceCommands {
    /// Write ~/.claude/fleet-wiki-guidance.md and inject its @import into
    /// ~/.claude/CLAUDE.md (idempotent)
    Apply {
        /// Guidance locale: `en` or `zh`
        #[arg(long, default_value = "en")]
        locale: String,
    },
    /// Strip the sentinel block and delete the guidance file (idempotent)
    Remove,
    /// Print whether the guidance block is currently installed
    Status,
}

#[derive(Subcommand)]
enum PrdDisciplineCommands {
    /// Regenerate the guidance file with the current schema (e.g. after a Fleet
    /// upgrade) and ensure the @import in CLAUDE.md + the prd-context hook.
    /// Idempotent. The desktop app does this automatically; this is the CLI
    /// equivalent for headless / scripted refreshes.
    Apply {
        /// How the assistant addresses the user (interpolated into the guidance).
        #[arg(long, default_value = "Boss")]
        title: String,
        /// Guidance locale: `en` or `zh`.
        #[arg(long, default_value = "en")]
        locale: String,
    },
}

fn main() {
    claw_fleet_core::console::init_utf8();
    let cli = Cli::parse();

    if let Some(ref host) = cli.remote {
        match &cli.command {
            Commands::Serve { .. }
            | Commands::Skill { .. }
            | Commands::Guard { .. }
            | Commands::Elicitation
            | Commands::Mcp
            | Commands::PlanApproval
            | Commands::PrdContext
            | Commands::TaskRuntime { .. } => {
                eprintln!("Error: --remote is not supported with the '{}' subcommand.",
                    match &cli.command {
                        Commands::Serve { .. } => "serve",
                        Commands::Skill { .. } => "skill",
                        Commands::Guard { .. } => "guard",
                        Commands::Elicitation => "elicitation",
                        Commands::Mcp => "mcp",
                        Commands::PlanApproval => "plan-approval",
                        Commands::PrdContext => "prd-context",
                        Commands::TaskRuntime { .. } => "task-runtime",
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
        Commands::Wiki { action } => cmd_wiki(action),
        Commands::Search { query, limit, json } => cmd_search(&query.join(" "), limit, json),
        Commands::Audit { level, filter, json } => cmd_audit(&level, filter.as_deref(), json),
        Commands::Report { date, backfill, regenerate, lessons, summary, json, lang } => cmd_report(date, backfill, regenerate, lessons, summary, json, &lang),
        Commands::Serve { port, token, port_file } => cmd_serve(port, token, port_file),
        Commands::Skill { action } => match action {
            SkillCommands::Install => cmd_skill_install(),
        },
        Commands::Guard { action } => match action {
            None => cmd_guard(),
            Some(GuardCommands::ListRules { json }) => cmd_guard_list_rules(json),
            Some(GuardCommands::RemoveRule { id }) => cmd_guard_remove_rule(&id),
        },
        Commands::Elicitation => cmd_elicitation(),
        Commands::Mcp => cmd_mcp(),
        Commands::PlanApproval => cmd_plan_approval(),
        Commands::PrdContext => cmd_prd_context(),
        Commands::Session { action } => match action {
            SessionCommands::Status { state, note } => cmd_session_status(&state, note.as_deref()),
            SessionCommands::Idle => cmd_session_idle(),
            SessionCommands::Resume => cmd_session_resume(),
        },
        Commands::Task { action } => cmd_task(action),
        Commands::TaskRuntime { cmd } => cmd_task_runtime(cmd),
        Commands::Plan { action } => cmd_plan(action),
        Commands::Handoff {
            note,
            plan,
            next,
            action,
        } => cmd_handoff(note.as_deref(), plan.as_deref(), next.as_deref(), action),
        Commands::PrdDiscipline { action } => match action {
            PrdDisciplineCommands::Apply { title, locale } => {
                cmd_prd_discipline_apply(&title, &locale)
            }
        },
    }
}

// ── task-runtime dispatcher (folded-in fleet-task lifecycle) ──────────────────

fn cmd_task_runtime(cmd: Option<TaskRuntimeCommand>) {
    if let Err(e) = task_runtime::run(cmd) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

// ── Task subcommand dispatcher ────────────────────────────────────────────────

fn cmd_task(action: TaskCommands) {
    fn die(msg: String) -> ! {
        eprintln!("error: {msg}");
        std::process::exit(1);
    }

    match action {
        TaskCommands::GetPlan { task_id } => {
            let task = claw_fleet_core::task::get_task(&task_id).unwrap_or_else(|e| die(e));
            // Convert plan JSON → YAML so the master can read it cleanly.
            let json = serde_json::to_value(&task.plan).unwrap_or(serde_json::Value::Null);
            match serde_yaml::to_string(&json) {
                Ok(s) => print!("{s}"),
                Err(e) => die(format!("yaml encode: {e}")),
            }
        }
        TaskCommands::GetDispatchable { task_id, json } => {
            let task = claw_fleet_core::task::get_task(&task_id).unwrap_or_else(|e| die(e));
            // Dispatch is purely by dependency topology now (resource
            // scheduling was removed in the task-subsystem rebuild).
            let dispatch = claw_fleet_core::scheduler::dispatchable(&task.plan);
            if let Some(e) = &dispatch.error {
                die(e.clone());
            }
            if json {
                println!("{}", serde_json::to_string(&dispatch.ready).unwrap_or_default());
            } else {
                for id in dispatch.ready {
                    println!("{id}");
                }
            }
        }
        TaskCommands::Dispatch { task_id, p_id } => {
            // Phase 3: dispatch belongs to the fleet-task runtime's LocalHost
            // so the spawned worker is tracked by fleet-task's pid HashMap
            // (otherwise SIGSTOP/CONT/TERM wouldn't reach it). Route via the
            // HTTP endpoint on the live fleet-task process; error if no
            // process is serving this task.
            match crate::fleet_cli_host::lookup_task_port(&task_id) {
                Some(port) => {
                    let url = format!("http://127.0.0.1:{port}/p-items/{p_id}/dispatch");
                    let resp = ureq::post(&url).call().unwrap_or_else(|e| {
                        die(format!("dispatch http: {e}"));
                    });
                    let status = resp.status();
                    if !(200..300).contains(&status) {
                        die(format!("dispatch returned http {status}"));
                    }
                    println!("dispatched");
                }
                None => die(format!(
                    "no fleet-task process for task {task_id} (no runtime registry entry)"
                )),
            }
        }
        TaskCommands::UpdatePlan {
            task_id,
            from_stdin,
            file,
        } => {
            let yaml_text = if from_stdin {
                let mut buf = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
                    .unwrap_or_else(|e| die(format!("stdin: {e}")));
                buf
            } else if let Some(p) = file {
                std::fs::read_to_string(&p).unwrap_or_else(|e| die(format!("read {}: {e}", p.display())))
            } else {
                die("provide --from-stdin or --file <path>".into());
            };
            // YAML → JSON Value → DagPlan (sidesteps serde_yaml's external-
            // tagged-enum tuple-variant gotcha, same trick as the skill
            // examples integration test).
            let yv: serde_yaml::Value =
                serde_yaml::from_str(&yaml_text).unwrap_or_else(|e| die(format!("yaml parse: {e}")));
            let jv: serde_json::Value =
                serde_json::to_value(&yv).unwrap_or_else(|e| die(format!("yaml → json: {e}")));
            let plan: claw_fleet_core::plan::DagPlan = serde_json::from_value(jv)
                .unwrap_or_else(|e| die(format!("plan schema: {e}")));
            if let Err(e) = plan.validate() {
                die(format!("plan validation: {e:?}"));
            }
            claw_fleet_core::task::update_plan(&task_id, plan).unwrap_or_else(|e| die(e));
            println!("plan updated");
        }
        TaskCommands::Clear { task_id } => {
            // File-only delete: bypass the legacy SupervisorHost terminate
            // path (it can't reach fleet-task subprocesses anyway). If a
            // fleet-task process is still serving this task, the user
            // should SIGTERM it via the pid in `~/.fleet/runtime/<id>.json`
            // *before* calling clear.
            let json = claw_fleet_core::task::task_json_path(&task_id)
                .unwrap_or_else(|e| die(e));
            if json.exists() {
                std::fs::remove_file(&json)
                    .unwrap_or_else(|e| die(format!("remove task json: {e}")));
            }
            let materials = claw_fleet_core::task::tasks_dir()
                .unwrap_or_else(|e| die(e))
                .join(&task_id);
            if materials.exists() {
                std::fs::remove_dir_all(&materials)
                    .unwrap_or_else(|e| die(format!("remove materials dir: {e}")));
            }
            println!("cleared");
        }
        TaskCommands::List { project_id, json } => {
            cmd_task_list(project_id.as_deref(), json);
        }
    }
}

fn cmd_task_list(project_id: Option<&str>, as_json: bool) {
    use std::collections::HashMap;
    use claw_fleet_core::registry;

    let tasks = claw_fleet_core::task::list_tasks(project_id);
    let runtime: HashMap<String, registry::RegistryEntry> = registry::list_alive()
        .unwrap_or_default()
        .into_iter()
        .map(|e| (e.task_id.clone(), e))
        .collect();

    if as_json {
        let records: Vec<serde_json::Value> = tasks
            .iter()
            .map(|t| {
                let live = runtime.get(&t.id);
                serde_json::json!({
                    "id": t.id,
                    "project_id": t.project_id,
                    "title": t.title,
                    "status": format!("{:?}", t.status).to_lowercase(),
                    "branch": t.task_branch,
                    "workspace": t.workspace,
                    "plan_total": t.plan.len(),
                    "plan_done": t.plan.items.values()
                        .filter(|p| matches!(p.status, claw_fleet_core::pitem::PItemStatus::Done))
                        .count(),
                    "live": live.is_some(),
                    "port": live.map(|e| e.port),
                    "pid": live.map(|e| e.pid),
                })
            })
            .collect();
        println!("{}", serde_json::to_string(&records).unwrap_or_default());
        return;
    }

    if tasks.is_empty() {
        println!("(no tasks under ~/.fleet/tasks/)");
        return;
    }

    // Table: ID(8) | live | status | done/total | title | branch
    println!(
        "{:<10} {:<6} {:<10} {:<7} {:<40} {}",
        "ID", "LIVE", "STATUS", "DONE", "TITLE", "BRANCH"
    );
    for t in &tasks {
        let short = if t.id.len() >= 8 { &t.id[..8] } else { &t.id };
        let live = if runtime.contains_key(&t.id) { "●" } else { "" };
        let status = format!("{:?}", t.status).to_lowercase();
        let total = t.plan.len();
        let done = t.plan.items.values()
            .filter(|p| matches!(p.status, claw_fleet_core::pitem::PItemStatus::Done))
            .count();
        let title = truncate(&t.title, 38);
        let branch = t.task_branch.as_deref().unwrap_or("-");
        println!(
            "{:<10} {:<6} {:<10} {:<7} {:<40} {}",
            short,
            live,
            status,
            format!("{done}/{total}"),
            title,
            branch
        );
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
    let output = claw_fleet_core::process_util::command("ssh")
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
            let scp_ok = claw_fleet_core::process_util::command("scp")
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
        let status = claw_fleet_core::process_util::command("ssh")
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
        let _ = (force, pid);
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
    let source = match info.usage_source.as_str() {
        "foxy-switcher" => "Foxy Switcher",
        "anthropic" => "Anthropic API",
        other if !other.is_empty() => other,
        _ => "Anthropic API",
    };
    println!("  {b}{:<16}{r}  {}", "Usage source:", source);

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
    use claw_fleet_core::search_index::SearchIndex;

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
    use claw_fleet_core::audit::{extract_audit_events, AuditRiskLevel};

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

// ── fleet wiki ────────────────────────────────────────────────────────────────

fn cmd_wiki(action: WikiCommands) {
    match action {
        WikiCommands::Publish { path, slug, title, workspace, json } => {
            let workspace = workspace.unwrap_or_else(|| {
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
            });
            match wiki::publish(&path, slug.as_deref(), title.as_deref(), &workspace) {
                Ok(doc) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&doc).unwrap());
                        return;
                    }
                    let nth = doc.versions.len();
                    println!(
                        "{}Published {}{} (version {}, {} total)",
                        c_bold(),
                        doc.slug,
                        c_reset(),
                        doc.current_version,
                        nth,
                    );
                    println!("  title:     {}", doc.title);
                    println!("  kind:      {:<9} entry: {}", doc.kind, doc.entry);
                    println!("  workspace: {}", doc.workspace_path);
                    if let Some(v) = doc.versions.first() {
                        println!(
                            "  files:     {} ({})",
                            v.file_count,
                            format_wiki_size(v.size_bytes)
                        );
                    }
                    println!("{}View it in the Fleet app → 知识库 board.{}", c_dim(), c_reset());
                }
                Err(e) => {
                    eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                    std::process::exit(1);
                }
            }
        }

        WikiCommands::List { workspace, json } => {
            let mut docs = wiki::list_docs();
            if let Some(ref ws) = workspace {
                docs.retain(|d| d.workspace_path.contains(ws.as_str()) || d.workspace_name.contains(ws.as_str()));
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&docs).unwrap());
                return;
            }
            if docs.is_empty() {
                println!(
                    "{}No wiki docs yet. Publish one with `fleet wiki publish <path>`.{}",
                    c_dim(),
                    c_reset()
                );
                return;
            }
            println!(
                "{}{:<26} {:<8} {:<18} {:>4}  {:<9} TITLE{}",
                c_bold(),
                "SLUG",
                "KIND",
                "WORKSPACE",
                "VERS",
                "UPDATED",
                c_reset()
            );
            for d in &docs {
                println!(
                    "{:<26} {:<8} {:<18} {:>4}  {:<9} {}",
                    truncate(&d.slug, 25),
                    d.kind,
                    truncate(&d.workspace_name, 17),
                    d.versions.len(),
                    format_age_ms(d.updated_ms),
                    truncate(&d.title, 40),
                );
            }
        }

        WikiCommands::Show { slug, json } => match wiki::get_doc(&slug) {
            Ok(doc) => {
                if json {
                    println!("{}", serde_json::to_string_pretty(&doc).unwrap());
                    return;
                }
                println!("{}{}{} — {}", c_bold(), doc.slug, c_reset(), doc.title);
                println!("  kind:      {:<9} entry: {}", doc.kind, doc.entry);
                println!("  workspace: {}", doc.workspace_path);
                println!("  created:   {}", format_age_ms(doc.created_ms));
                println!("  updated:   {}", format_age_ms(doc.updated_ms));
                println!("  versions:");
                for v in &doc.versions {
                    let marker = if v.id == doc.current_version { "*" } else { " " };
                    println!(
                        "   {marker} {}  {} file(s), {}  {}",
                        v.id,
                        v.file_count,
                        format_wiki_size(v.size_bytes),
                        format_age_ms(v.published_ms),
                    );
                }
            }
            Err(e) => {
                eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                std::process::exit(1);
            }
        },

        WikiCommands::Rm { slug, version } => {
            let result = match version {
                Some(ref v) => wiki::delete_version(&slug, v),
                None => wiki::delete_doc(&slug),
            };
            match result {
                Ok(()) => match version {
                    Some(v) => println!("Removed version {v} of {slug}"),
                    None => println!("Removed {slug} (all versions)"),
                },
                Err(e) => {
                    eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                    std::process::exit(1);
                }
            }
        }

        WikiCommands::FixWorkspaces { json } => match wiki::fix_scratchpad_workspaces() {
            Ok(fixed) => {
                if json {
                    let rows: Vec<serde_json::Value> = fixed
                        .iter()
                        .map(|(slug, old, new)| {
                            serde_json::json!({ "slug": slug, "old": old, "new": new })
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&rows).unwrap());
                    return;
                }
                if fixed.is_empty() {
                    println!("No docs needed retagging.");
                    return;
                }
                for (slug, old, new) in &fixed {
                    println!("{}{slug}{}", c_bold(), c_reset());
                    println!("  {old}");
                    println!("  → {new}");
                }
                println!("Retagged {} doc(s).", fixed.len());
            }
            Err(e) => {
                eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                std::process::exit(1);
            }
        },

        WikiCommands::Guidance { action } => match action {
            WikiGuidanceCommands::Apply { locale } => {
                match claw_fleet_core::wiki_guidance::apply_wiki_guidance(&locale) {
                    Ok(()) => println!(
                        "Wiki guidance installed (~/.claude/fleet-wiki-guidance.md + CLAUDE.md import)."
                    ),
                    Err(e) => {
                        eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                        std::process::exit(1);
                    }
                }
            }
            WikiGuidanceCommands::Remove => {
                match claw_fleet_core::wiki_guidance::remove_wiki_guidance() {
                    Ok(()) => println!("Wiki guidance removed."),
                    Err(e) => {
                        eprintln!("{}Error:{} {}", "\x1b[31m", c_reset(), e);
                        std::process::exit(1);
                    }
                }
            }
            WikiGuidanceCommands::Status => {
                if claw_fleet_core::wiki_guidance::is_wiki_guidance_installed() {
                    println!("installed");
                } else {
                    println!("not installed");
                    std::process::exit(1);
                }
            }
        },
    }
}

fn format_wiki_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

// ── fleet serve ───────────────────────────────────────────────────────────────
// Phase 4 P1: the serve loop body moved to `claw_fleet_core::hooks_server::serve`.
// `cmd_serve` is now a thin wrapper so fleet-cli and fleet-hooks-server call the
// same code path.

fn cmd_serve(port: u16, token: String, port_file: Option<std::path::PathBuf>) {
    claw_fleet_core::hooks_server::serve(port, token, port_file);
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

// ── Guard CLI (hook entrypoint) ─────────────────────────────────────────────

fn cmd_guard_list_rules(json: bool) {
    let rules = claw_fleet_core::audit::list_guard_allow_rules();
    if json {
        let body = serde_json::to_string_pretty(&rules)
            .unwrap_or_else(|_| "[]".to_string());
        println!("{}", body);
        return;
    }
    if rules.is_empty() {
        println!("No guard allow rules configured.");
        println!("(They get added when you click \"始终允许\" / \"Always allow\" on a guard card.)");
        return;
    }
    println!("{:<38}  {:<24}  {}", "ID", "SOURCE TAG", "PREFIX");
    for r in &rules {
        let tag = r.source_tag.as_deref().unwrap_or("-");
        println!("{:<38}  {:<24}  {}", r.id, tag, r.prefix);
    }
}

fn cmd_guard_remove_rule(id: &str) {
    match claw_fleet_core::audit::remove_guard_allow_rule(id) {
        Ok(()) => println!("Removed guard allow rule {}", id),
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

fn cmd_guard() {
    use claw_fleet_core::consumer_heartbeat;
    use claw_fleet_core::decision_panel_config::DecisionPanelConfig;
    use claw_fleet_core::guard::{
        self, GuardClassification, GuardDecision, GuardRequest, HookInput,
    };
    use std::io::Read;
    use std::time::Instant;

    // Read hook JSON from stdin.
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        // Can't read stdin — allow silently.
        return;
    }

    let hook_input: HookInput = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return, // Malformed input — allow silently.
    };

    // Classify the command.
    match guard::classify_hook_input(&hook_input) {
        GuardClassification::Allow => {
            // Not critical — allow.
            return;
        }
        GuardClassification::NeedsConfirmation {
            command,
            risk_tags,
        } => {
            // No live consumer (Fleet app not running / no SSE client on
            // `fleet serve`) — fall through silently so Claude isn't blocked
            // by a request nobody will answer.
            let cfg = DecisionPanelConfig::load();
            let liveness_window = cfg.heartbeat_window();
            let status = consumer_heartbeat::consumer_status(liveness_window);
            if !status.is_alive() {
                claw_fleet_core::log_debug(&format!(
                    "[guard hook] no live consumer at startup (status={status}); falling back to Claude Code's native UI",
                ));
                return;
            }

            let session_id = hook_input
                .session_id
                .clone()
                .unwrap_or_else(|| "unknown".to_string());

            let request_id = guard::new_request_id();

            let mut structured = claw_fleet_core::cmd_ast::extract_structured_view(&command);
            let user_rules = claw_fleet_core::audit::load_user_rules();
            claw_fleet_core::audit::annotate_view_with_flags(&mut structured, &user_rules);

            let req = GuardRequest {
                id: request_id.clone(),
                session_id,
                workspace_name: String::new(), // Desktop app resolves from session_id
                ai_title: None, // Desktop app resolves from session_id
                tool_name: "Bash".to_string(),
                command: command.clone(),
                command_summary: guard::truncate_command(&command, 120),
                risk_tags,
                timestamp: chrono::Utc::now().to_rfc3339(),
                structured_command: Some(structured),
            };

            // Write request for the desktop app to pick up.
            if let Err(e) = guard::write_request(&req) {
                eprintln!("guard: failed to write request: {e}");
                // On error, block by default for Critical commands.
                let out = serde_json::json!({
                    "decision": "block",
                    "reason": "Fleet Guard: failed to communicate with Fleet app"
                });
                println!("{}", out);
                return;
            }

            // Poll for response (configured wait_seconds, default 600s),
            // bailing out early if the consumer dies mid-flight.
            let timeout = cfg.wait_duration();
            let poll_interval = cfg.poll_duration();
            let start = Instant::now();
            loop {
                if let Some(resp) = guard::try_read_response(&request_id) {
                    guard::cleanup(&request_id);
                    match resp.decision {
                        GuardDecision::Allow => {}
                        GuardDecision::Block => {
                            let user_reason = resp
                                .reason
                                .as_deref()
                                .map(str::trim)
                                .filter(|s| !s.is_empty());
                            let reason_text = match user_reason {
                                Some(r) => {
                                    format!("Fleet Guard: blocked by user — {r}")
                                }
                                None => "Fleet Guard: blocked by user".to_string(),
                            };
                            let out = serde_json::json!({
                                "decision": "block",
                                "reason": reason_text,
                            });
                            println!("{}", out);
                        }
                    }
                    return;
                }
                if start.elapsed() > timeout {
                    guard::cleanup(&request_id);
                    let out = serde_json::json!({
                        "decision": "block",
                        "reason": "Fleet Guard: no response from Fleet app (timeout)"
                    });
                    println!("{}", out);
                    return;
                }
                let status = consumer_heartbeat::consumer_status(liveness_window);
                if !status.is_alive() {
                    // Head went away while we waited — fall through silently.
                    claw_fleet_core::log_debug(&format!(
                        "[guard hook] consumer heartbeat lost after {:.1}s (request {}, status={status}); deleting request file and falling back to native UI",
                        start.elapsed().as_secs_f32(),
                        request_id,
                    ));
                    guard::cleanup(&request_id);
                    return;
                }
                std::thread::sleep(poll_interval);
            }
        }
    }
}

// ── Elicitation CLI (hook entrypoint for AskUserQuestion) ───────────────

fn cmd_elicitation() {
    use claw_fleet_core::consumer_heartbeat;
    use claw_fleet_core::decision_history::{
        self, DecisionHistoryRecord, ElicitationOutcome, build_elicitation_record,
    };
    use claw_fleet_core::decision_panel_config::DecisionPanelConfig;
    use claw_fleet_core::elicitation::{self, ElicitationRequest};
    use claw_fleet_core::guard::{self, HookInput};
    use std::io::Read;
    use std::time::Instant;

    // Read hook JSON from stdin.
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return;
    }

    let hook_input: HookInput = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Only handle AskUserQuestion.
    let tool_name = hook_input.tool_name.as_deref().unwrap_or("");
    if tool_name != "AskUserQuestion" {
        return;
    }

    let tool_input = match &hook_input.tool_input {
        Some(v) => v.clone(),
        None => return,
    };

    // Parse questions from tool_input.
    let questions_val = match tool_input.get("questions") {
        Some(q) => q.clone(),
        None => return,
    };

    let questions: Vec<elicitation::ElicitationQuestion> =
        match serde_json::from_value(questions_val.clone()) {
            Ok(q) => q,
            Err(_) => return,
        };

    if questions.is_empty() {
        return;
    }

    // No live consumer — fall through silently; Claude Code will use its
    // native AskUserQuestion UI.
    let cfg = DecisionPanelConfig::load();
    let liveness_window = cfg.heartbeat_window();
    let startup_status = consumer_heartbeat::consumer_status(liveness_window);
    if !startup_status.is_alive() {
        claw_fleet_core::log_debug(&format!(
            "[elicitation hook] no live consumer at startup (status={startup_status}); falling back to Claude Code's native UI",
        ));
        return;
    }

    let session_id = hook_input
        .session_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    let request_id = guard::new_request_id();

    let req = ElicitationRequest {
        id: request_id.clone(),
        session_id,
        workspace_name: String::new(),
        ai_title: None,
        questions,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    if let Err(e) = elicitation::write_request(&req) {
        eprintln!("elicitation: failed to write request: {e}");
        // On error, deny so Claude Code falls back to its own UI.
        let out = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "Fleet: failed to communicate with Fleet app"
            }
        });
        println!("{}", out);
        return;
    }

    // Poll for response (configured wait_seconds, default 600s), bailing out
    // early if the consumer dies mid-flight so Claude Code can take over
    // with its native UI.
    let timeout = cfg.wait_duration();
    let poll_interval = cfg.poll_duration();
    let start = Instant::now();
    let resp = loop {
        if let Some(r) = elicitation::try_read_response(&request_id) {
            break Some(r);
        }
        if start.elapsed() > timeout {
            break None;
        }
        let loop_status = consumer_heartbeat::consumer_status(liveness_window);
        if !loop_status.is_alive() {
            // Head went away — silently fall through to Claude's native UI.
            claw_fleet_core::log_debug(&format!(
                "[elicitation hook] consumer heartbeat lost after {:.1}s (request {}, status={loop_status}); deleting request file and falling back to native UI",
                start.elapsed().as_secs_f32(),
                request_id,
            ));
            let rec = build_elicitation_record(
                &req,
                ElicitationOutcome::HeartbeatLost,
                &std::collections::HashMap::new(),
                chrono::Utc::now().to_rfc3339(),
            );
            if let Err(e) = decision_history::append_record(&DecisionHistoryRecord::Elicitation(rec)) {
                eprintln!("decision_history append (heartbeat-lost): {e}");
            }
            elicitation::cleanup(&request_id);
            return;
        }
        std::thread::sleep(poll_interval);
    };
    match resp {
        Some(resp) => {
            let outcome = if resp.declined {
                ElicitationOutcome::Declined
            } else {
                ElicitationOutcome::Answered
            };
            let rec = build_elicitation_record(
                &req,
                outcome,
                &resp.answers,
                chrono::Utc::now().to_rfc3339(),
            );
            if let Err(e) = decision_history::append_record(&DecisionHistoryRecord::Elicitation(rec)) {
                eprintln!("decision_history append: {e}");
            }
            elicitation::cleanup(&request_id);
            if resp.declined {
                // User declined — deny so Claude Code knows.
                let out = serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": "Fleet: user declined to answer"
                    }
                });
                println!("{}", out);
            } else {
                // Build updatedInput with original questions + user answers.
                let mut updated_input = tool_input.clone();
                updated_input["answers"] =
                    serde_json::to_value(&resp.answers).unwrap_or_default();
                let out = serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "allow",
                        "updatedInput": updated_input
                    }
                });
                println!("{}", out);
            }
        }
        None => {
            claw_fleet_core::log_debug(&format!(
                "[elicitation hook] timed out after {:.1}s waiting for user response (request {}); returning deny",
                start.elapsed().as_secs_f32(),
                request_id,
            ));
            let rec = build_elicitation_record(
                &req,
                ElicitationOutcome::Timeout,
                &std::collections::HashMap::new(),
                chrono::Utc::now().to_rfc3339(),
            );
            if let Err(e) = decision_history::append_record(&DecisionHistoryRecord::Elicitation(rec)) {
                eprintln!("decision_history append (timeout): {e}");
            }
            elicitation::cleanup(&request_id);
            // Timeout — deny so Claude Code falls back.
            let out = serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": "Fleet: no response from Fleet app (timeout)"
                }
            });
            println!("{}", out);
        }
    }
}

// ── MCP server (stdio JSON-RPC; exposes `fleet__ask`) ──────────────────

fn cmd_mcp() {
    if let Err(e) = claw_fleet_core::mcp_server::run() {
        eprintln!("fleet mcp: stdio error: {e}");
        std::process::exit(1);
    }
}

// ── Plan-approval CLI (hook entrypoint for ExitPlanMode) ────────────────

fn cmd_plan_approval() {
    use claw_fleet_core::consumer_heartbeat;
    use claw_fleet_core::decision_history::{
        self, DecisionHistoryRecord, PlanApprovalOutcome, build_plan_approval_record,
    };
    use claw_fleet_core::decision_panel_config::DecisionPanelConfig;
    use claw_fleet_core::guard::{self, HookInput};
    use claw_fleet_core::plan_approval::{self, PlanApprovalRequest};
    use std::io::Read;
    use std::time::Instant;

    // Read hook JSON from stdin.
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return;
    }

    let hook_input: HookInput = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Only handle ExitPlanMode.
    let tool_name = hook_input.tool_name.as_deref().unwrap_or("");
    if tool_name != "ExitPlanMode" {
        return;
    }

    let tool_input = match &hook_input.tool_input {
        Some(v) => v.clone(),
        None => return,
    };

    // Pull plan content and file path. Claude Code's `normalizeToolInput`
    // injects these into the tool_input before hooks fire, so we can display
    // the plan without having to re-read from disk.
    let plan_content = tool_input
        .get("plan")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let plan_file_path = tool_input
        .get("planFilePath")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // No live consumer — fall through silently so Claude Code keeps its
    // native plan-approval UI as a fallback.
    let cfg = DecisionPanelConfig::load();
    let liveness_window = cfg.heartbeat_window();
    let startup_status = consumer_heartbeat::consumer_status(liveness_window);
    if !startup_status.is_alive() {
        claw_fleet_core::log_debug(&format!(
            "[plan-approval hook] no live consumer at startup (status={startup_status}); falling back to Claude Code's native UI",
        ));
        return;
    }

    let session_id = hook_input
        .session_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    let request_id = guard::new_request_id();

    let req = PlanApprovalRequest {
        id: request_id.clone(),
        session_id,
        workspace_name: String::new(),
        ai_title: None,
        plan_content,
        plan_file_path,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    if let Err(e) = plan_approval::write_request(&req) {
        eprintln!("plan-approval: failed to write request: {e}");
        // On error, deny so Claude Code falls back to its own UI.
        let out = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "Fleet: failed to communicate with Fleet app"
            }
        });
        println!("{}", out);
        return;
    }

    // Poll for response (configured wait_seconds, default 600s), bailing out
    // early if the consumer dies mid-flight so Claude Code can take over
    // with its native UI.
    let timeout = cfg.wait_duration();
    let poll_interval = cfg.poll_duration();
    let start = Instant::now();
    let resp = loop {
        if let Some(r) = plan_approval::try_read_response(&request_id) {
            break Some(r);
        }
        if start.elapsed() > timeout {
            break None;
        }
        let loop_status = consumer_heartbeat::consumer_status(liveness_window);
        if !loop_status.is_alive() {
            claw_fleet_core::log_debug(&format!(
                "[plan-approval hook] consumer heartbeat lost after {:.1}s (request {}, status={loop_status}); deleting request file and falling back to native UI",
                start.elapsed().as_secs_f32(),
                request_id,
            ));
            let rec = build_plan_approval_record(
                &req,
                PlanApprovalOutcome::HeartbeatLost,
                None,
                chrono::Utc::now().to_rfc3339(),
            );
            if let Err(e) = decision_history::append_record(&DecisionHistoryRecord::PlanApproval(rec)) {
                eprintln!("decision_history append (heartbeat-lost): {e}");
            }
            plan_approval::cleanup(&request_id);
            return;
        }
        std::thread::sleep(poll_interval);
    };

    match resp {
        Some(resp) => {
            let outcome = if resp.decision == "approve" {
                if resp.edited_plan.is_some() {
                    PlanApprovalOutcome::ApprovedWithEdits
                } else {
                    PlanApprovalOutcome::Approved
                }
            } else {
                PlanApprovalOutcome::Rejected
            };
            let rec = build_plan_approval_record(
                &req,
                outcome,
                Some(&resp),
                chrono::Utc::now().to_rfc3339(),
            );
            if let Err(e) = decision_history::append_record(&DecisionHistoryRecord::PlanApproval(rec)) {
                eprintln!("decision_history append: {e}");
            }
            plan_approval::cleanup(&request_id);
            if resp.decision == "approve" {
                // Approve — optionally echo back an edited plan via updatedInput.
                let mut updated_input = tool_input.clone();
                if let Some(edited) = resp.edited_plan.as_deref() {
                    updated_input["plan"] = serde_json::Value::String(edited.to_string());
                }
                let out = serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "allow",
                        "updatedInput": updated_input
                    }
                });
                println!("{}", out);
            } else {
                // Reject — pass user feedback (if any) back to the model.
                let reason = resp
                    .feedback
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| "User rejected plan".to_string());
                let out = serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": reason
                    }
                });
                println!("{}", out);
            }
        }
        None => {
            claw_fleet_core::log_debug(&format!(
                "[plan-approval hook] timed out after {:.1}s waiting for user response (request {}); returning deny",
                start.elapsed().as_secs_f32(),
                request_id,
            ));
            let rec = build_plan_approval_record(
                &req,
                PlanApprovalOutcome::Timeout,
                None,
                chrono::Utc::now().to_rfc3339(),
            );
            if let Err(e) = decision_history::append_record(&DecisionHistoryRecord::PlanApproval(rec)) {
                eprintln!("decision_history append (timeout): {e}");
            }
            plan_approval::cleanup(&request_id);
            let out = serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "deny",
                    "permissionDecisionReason": "Fleet: no response from Fleet app (timeout)"
                }
            });
            println!("{}", out);
        }
    }
}

// ── Session self-report (called by agents from inside a managed session) ──

/// `fleet session status <state> [--note <msg>]` — agent self-reports its
/// current status to the supervisor.
///
/// Reads `FLEET_SESSION_ID` from env, then writes the status directly to
/// `~/.claude/fleet/fleet-sessions.json` via `claw_fleet_core::supervisor`.
/// The running supervisor's tick loop (in fleet serve) picks up the change
/// on its next pass.
///
/// v1 uses direct file write rather than HTTP to keep fleet-cli light (no
/// extra HTTP client dep). The HTTP endpoint at `/fleet_sessions/status`
/// is still available for the Tauri GUI and remote backends that already
/// have a probe client.
fn cmd_session_status(state: &str, note: Option<&str>) {
    let session_id = match read_fleet_session_id() {
        Some(s) => s,
        None => {
            eprintln!("Error: no session id (neither FLEET_SESSION_ID nor CLAUDE_CODE_SESSION_ID set).");
            std::process::exit(2);
        }
    };

    match claw_fleet_core::supervisor::set_status(&session_id, state, note.map(String::from)) {
        Ok(()) => println!("ok"),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

/// `fleet session idle` — Stop-hook entrypoint. Marks the current session idle
/// so the supervisor's tick flips the kanban card from Running → Pending.
///
/// Silently exits 0 if FLEET_SESSION_ID is unset (e.g. user has global Fleet
/// hooks installed but ran claude outside the supervisor) — never blocks the hook.
fn cmd_session_idle() {
    let Some(sid) = read_fleet_session_id() else {
        return;
    };
    if let Err(e) = claw_fleet_core::idle::mark_idle(&sid) {
        eprintln!("fleet session idle: {e}");
    }
    // Handoff relay: the session just yielded its turn — exactly the moment a
    // registered handoff should fire. Errors are logged, never propagated:
    // the Stop hook must not block or fail the session over a relay problem.
    match claw_fleet_core::handoff::consume_and_spawn(&sid) {
        Ok(Some(to_sid)) => println!("handoff: successor session {to_sid} spawned"),
        Ok(None) => {}
        Err(e) => eprintln!("fleet session idle: handoff relay failed: {e}"),
    }
}

/// `fleet session resume` — UserPromptSubmit-hook entrypoint. Clears the idle
/// sentinel so the supervisor flips the card back to Running on next tick.
fn cmd_session_resume() {
    let Some(sid) = read_fleet_session_id() else {
        return;
    };
    claw_fleet_core::idle::clear_idle(&sid);
    // A fresh user prompt means the user took the session back over — a still
    // pending (i.e. never-consumed) handoff is stale intent; drop it so it
    // can't fire surprisingly on a later Stop.
    claw_fleet_core::handoff::cancel_pending(&sid);
}

/// The current session's id, for attributing `fleet plan` / `fleet session`
/// actions. Prefers `FLEET_SESSION_ID` (set by the Fleet supervisor) and falls
/// back to `CLAUDE_CODE_SESSION_ID` (set by Claude Code for *every* session,
/// Fleet-managed or not). Both carry the same value under Fleet — the supervisor
/// launches claude with `--session-id <id>` and exports `FLEET_SESSION_ID=<id>`,
/// and that id equals the session's jsonl stem (i.e. `SessionInfo.id`), so the
/// side-channel key always matches what the card looks up.
fn read_fleet_session_id() -> Option<String> {
    resolve_session_id(
        std::env::var("FLEET_SESSION_ID").ok(),
        std::env::var("CLAUDE_CODE_SESSION_ID").ok(),
    )
}

/// Pure resolution: first non-empty of (`FLEET_SESSION_ID`, `CLAUDE_CODE_SESSION_ID`).
fn resolve_session_id(fleet: Option<String>, claude: Option<String>) -> Option<String> {
    fleet
        .filter(|s| !s.is_empty())
        .or_else(|| claude.filter(|s| !s.is_empty()))
}

#[cfg(test)]
mod session_id_tests {
    use super::resolve_session_id;

    #[test]
    fn falls_back_to_claude_code_session_id() {
        // Non-Fleet-managed session: FLEET_SESSION_ID absent, but Claude Code
        // always sets CLAUDE_CODE_SESSION_ID — attribution must still resolve.
        assert_eq!(
            resolve_session_id(None, Some("claude-sid".into())),
            Some("claude-sid".into())
        );
        // Empty FLEET_SESSION_ID also falls back.
        assert_eq!(
            resolve_session_id(Some("".into()), Some("claude-sid".into())),
            Some("claude-sid".into())
        );
        // FLEET_SESSION_ID wins when present.
        assert_eq!(
            resolve_session_id(Some("fleet-sid".into()), Some("claude-sid".into())),
            Some("fleet-sid".into())
        );
        // Neither → None.
        assert_eq!(resolve_session_id(None, None), None);
    }
}

// ── `fleet prd-discipline apply` — regenerate guidance (CLI parity w/ GUI) ────

fn cmd_prd_discipline_apply(title: &str, locale: &str) {
    let result = claw_fleet_core::prd_discipline::apply_prd_discipline(title, locale)
        .and_then(|()| claw_fleet_core::hooks::apply_prd_context_hook());
    match result {
        Ok(()) => println!("ok: regenerated PRD guidance (title={title:?}, locale={locale:?})"),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

// ── `fleet handoff` — session relay registration ──────────────────────────────

/// Bare `fleet handoff --note ...` registers; subcommands manage/inspect.
/// Registration is a promise, not an action: the successor session is spawned
/// by the Stop hook (`fleet session idle`) the moment this session yields.
fn cmd_handoff(
    note: Option<&str>,
    plan: Option<&str>,
    next: Option<&str>,
    action: Option<HandoffCommands>,
) {
    match action {
        Some(HandoffCommands::Cancel) => {
            let Some(sid) = read_fleet_session_id() else {
                eprintln!("Error: no session id (neither FLEET_SESSION_ID nor CLAUDE_CODE_SESSION_ID set).");
                std::process::exit(2);
            };
            claw_fleet_core::handoff::cancel_pending(&sid);
            println!("ok: pending handoff cancelled (if any)");
            return;
        }
        Some(HandoffCommands::List) => {
            let chains = claw_fleet_core::handoff::list_chains();
            if chains.is_empty() {
                println!("no handoff chains recorded");
                return;
            }
            for c in chains {
                println!(
                    "{}  [{} hops]  plan={}  ws={}",
                    c.chain_id,
                    c.links.len(),
                    c.plan_id.as_deref().unwrap_or("-"),
                    c.workspace_path
                );
                for id in c.session_ids() {
                    println!("  -> {id}");
                }
            }
            return;
        }
        Some(HandoffCommands::Show { session_id }) => {
            match claw_fleet_core::handoff::chain_containing(&session_id) {
                Some(c) => println!("{}", serde_json::to_string_pretty(&c).unwrap_or_default()),
                None => {
                    eprintln!("no chain contains session {session_id}");
                    std::process::exit(1);
                }
            }
            return;
        }
        None => {}
    }

    // registration path
    let Some(note) = note.map(str::trim).filter(|n| !n.is_empty()) else {
        eprintln!(
            "Error: --note is required — the successor session only knows what you write here.\n\
             Include: what's done, what's next, key files, gotchas."
        );
        std::process::exit(2);
    };
    let Some(sid) = read_fleet_session_id() else {
        eprintln!("Error: no session id (neither FLEET_SESSION_ID nor CLAUDE_CODE_SESSION_ID set).");
        std::process::exit(2);
    };
    if next.is_some() && plan.is_none() {
        eprintln!("Error: --next requires --plan.");
        std::process::exit(2);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    if let Some(plan_id) = plan {
        if claw_fleet_core::prd_tasks::find_plan_source(&cwd, plan_id).is_none() {
            eprintln!(
                "Error: plan '{plan_id}' not found in any TASKS.md reachable from {}.",
                cwd.display()
            );
            std::process::exit(1);
        }
    }
    let ws = cwd.to_string_lossy().to_string();
    match claw_fleet_core::handoff::register(&sid, &ws, note, plan, next) {
        Ok(rec) => println!(
            "ok: handoff registered (chain {}, 第 {} 棒). \
             接力 session 将在本 session 结束 turn 后由 Stop hook 自动启动；请尽快结束当前 turn。",
            rec.chain_id, rec.hop
        ),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

// ── `fleet plan` — TASKS.md PRD-plan management (captures session focus) ──────

fn cmd_plan(action: PlanCommands) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let result: Result<(), String> = match action {
        PlanCommands::Check { plan_id, task } => plan_mutate_checkbox(&cwd, &plan_id, &task, true),
        PlanCommands::Uncheck { plan_id, task } => {
            plan_mutate_checkbox(&cwd, &plan_id, &task, false)
        }
        PlanCommands::Start { plan_id, task } => plan_start(&cwd, &plan_id, task.as_deref()),
        PlanCommands::Create { plan_id, title } => plan_create(&cwd, &plan_id, &title),
        PlanCommands::Add { plan_id, task, text } => plan_add(&cwd, &plan_id, &task, &text),
        PlanCommands::Migrate { path } => plan_migrate(&cwd, path),
        PlanCommands::List => plan_list(&cwd),
        PlanCommands::Get { plan_id } => plan_get(&cwd, &plan_id),
    };
    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

/// Workspace's primary TASKS.md (main checkout root, else cwd).
fn workspace_tasks_path(cwd: &std::path::Path) -> std::path::PathBuf {
    match claw_fleet_core::prd_tasks::discover_main_checkout_root(cwd) {
        Some(root) => root.join("TASKS.md"),
        None => cwd.join("TASKS.md"),
    }
}

/// Record this session's focus (plan + current pending P) in the side-channel.
/// No-op (with a warning) when FLEET_SESSION_ID is unset — file edits still
/// apply; only the per-session "current" attribution is skipped.
fn plan_record_focus(cwd: &std::path::Path, plan_id: &str, content: &str) {
    use claw_fleet_core::prd_tasks as pt;
    let Some(sid) = read_fleet_session_id() else {
        return;
    };
    let ws = pt::discover_main_checkout_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let current = pt::plan_body(content, plan_id).and_then(|b| pt::first_pending_task(&b));
    if let Err(e) =
        claw_fleet_core::task_progress::set_current(&sid, &ws.to_string_lossy(), plan_id, current)
    {
        eprintln!("warning: could not record task focus: {e}");
    }
}

fn plan_mutate_checkbox(
    cwd: &std::path::Path,
    plan_id: &str,
    task: &str,
    done: bool,
) -> Result<(), String> {
    use claw_fleet_core::prd_tasks as pt;
    let path = pt::find_plan_source(cwd, plan_id)
        .ok_or_else(|| format!("plan '{plan_id}' not found in any TASKS.md"))?;
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let updated = pt::set_checkbox(&content, plan_id, task, done)?;
    std::fs::write(&path, &updated).map_err(|e| format!("write {}: {e}", path.display()))?;
    plan_record_focus(cwd, plan_id, &updated);
    println!("ok");
    Ok(())
}

fn plan_start(cwd: &std::path::Path, plan_id: &str, task: Option<&str>) -> Result<(), String> {
    use claw_fleet_core::prd_tasks as pt;
    let current = pt::resolve_current_task(cwd, plan_id, task)?;
    let sid = read_fleet_session_id()
        .ok_or("no session id (neither FLEET_SESSION_ID nor CLAUDE_CODE_SESSION_ID set)")?;
    let ws = pt::discover_main_checkout_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    claw_fleet_core::task_progress::set_current(&sid, &ws.to_string_lossy(), plan_id, current)?;
    println!("ok");
    Ok(())
}

fn plan_create(cwd: &std::path::Path, plan_id: &str, title: &str) -> Result<(), String> {
    use claw_fleet_core::prd_tasks as pt;
    let path = workspace_tasks_path(cwd);
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = pt::create_plan(&content, plan_id, title)?;
    std::fs::write(&path, &updated).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("created plan '{plan_id}' in {}", path.display());
    Ok(())
}

fn plan_add(cwd: &std::path::Path, plan_id: &str, task: &str, text: &str) -> Result<(), String> {
    use claw_fleet_core::prd_tasks as pt;
    let path = pt::find_plan_source(cwd, plan_id)
        .ok_or_else(|| format!("plan '{plan_id}' not found in any TASKS.md"))?;
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let updated = pt::add_task(&content, plan_id, task, text)?;
    std::fs::write(&path, &updated).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!("ok");
    Ok(())
}

fn plan_migrate(cwd: &std::path::Path, path: Option<std::path::PathBuf>) -> Result<(), String> {
    use claw_fleet_core::prd_tasks as pt;
    let target = path.unwrap_or_else(|| workspace_tasks_path(cwd));
    let content =
        std::fs::read_to_string(&target).map_err(|e| format!("read {}: {e}", target.display()))?;
    let migrated = pt::migrate_v1_to_v2(&content);
    if migrated == content {
        println!("already v2 (no changes): {}", target.display());
        return Ok(());
    }
    std::fs::write(&target, &migrated).map_err(|e| format!("write {}: {e}", target.display()))?;
    println!("migrated {}", target.display());
    Ok(())
}

fn plan_list(cwd: &std::path::Path) -> Result<(), String> {
    let plans = claw_fleet_core::prd_tasks::list_workspace_task_plans(cwd, None);
    if plans.is_empty() {
        println!("(no plans)");
        return Ok(());
    }
    for p in plans {
        let done = p.items.iter().filter(|i| i.done).count();
        let src = p.source.map(|s| format!(" — {s}")).unwrap_or_default();
        println!(
            "{} [{}/{}]{}",
            p.id.as_deref().unwrap_or("(anonymous)"),
            done,
            p.items.len(),
            src
        );
    }
    Ok(())
}

fn plan_get(cwd: &std::path::Path, plan_id: &str) -> Result<(), String> {
    let plans = claw_fleet_core::prd_tasks::list_workspace_task_plans(cwd, None);
    let p = plans
        .iter()
        .find(|p| p.id.as_deref() == Some(plan_id))
        .ok_or_else(|| format!("plan '{plan_id}' not found"))?;
    for it in &p.items {
        println!("{} {}", if it.done { "[x]" } else { "[ ]" }, it.text);
    }
    Ok(())
}

// ── PRD-context CLI (hook entrypoint for UserPromptSubmit) ─────────────────

/// Re-inject the workspace's `TASKS.md` (active plan region) into every user
/// prompt as additional context. Companion to PRD Discipline mode — survives
/// context compression, since the file lives on disk.
///
/// Multi-source: discovers the repo's main checkout root and scans both
/// `<main>/TASKS.md` and every `<main>/.worktrees/*/TASKS.md`, so a worker
/// inside a worktree still sees plans living in the main checkout (and vice
/// versa). Plans are deduped by `id`; on conflict the source whose file was
/// modified most recently wins. Legacy anonymous (no `id`) blocks are kept
/// independently — they pre-date the multi-plan format.
fn cmd_prd_context() {
    use claw_fleet_core::prd_tasks::*;
    use std::io::Read;
    use std::path::PathBuf;

    // Read stdin payload — Claude Code sends `{ session_id, cwd, prompt, ... }`.
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);

    // Prefer the `cwd` field from stdin (authoritative for this hook firing);
    // fall back to process cwd if parsing fails.
    let cwd_from_stdin = serde_json::from_str::<serde_json::Value>(&input)
        .ok()
        .and_then(|v| {
            v.get("cwd")
                .and_then(|c| c.as_str())
                .map(PathBuf::from)
        });
    let cwd = cwd_from_stdin
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    let main_root = discover_main_checkout_root(&cwd);
    let sources = collect_task_sources(&cwd, main_root.as_deref());
    if sources.is_empty() {
        // No TASKS.md anywhere → silent no-op. PRD mode just doesn't trigger.
        return;
    }

    // `collect_from_sources` returns active blocks AND any structural problems
    // (unterminated/mismatched/malformed sentinels). We never rewrite the file;
    // problems become a warning so a broken block doesn't silently disappear.
    let (raw, problems) = collect_from_sources(&sources, true);
    let warning = render_problem_warning(&problems);

    let deduped = dedup_blocks_keep_latest_mtime(raw);
    let rendered = render_with_sources(&deduped, main_root.as_deref());

    // Nothing to inject: no active plan parsed AND no problem detected → the
    // original silent no-op.
    if deduped.is_empty() && warning.is_none() {
        return;
    }

    let sources_list = sources
        .iter()
        .map(|p| format!("  - {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");

    let reminder = if deduped.is_empty() {
        // A TASKS.md exists and looks broken, but no active plan parsed out of
        // it. Inject a warning-only reminder so the agent fixes the file
        // instead of losing the plan to silent degradation.
        let warn = warning.unwrap_or_default();
        format!(
            "<system-reminder>\n{warn}\n\nSources scanned:\n{sources_list}\n</system-reminder>",
        )
    } else {
        let n = deduped.len();
        let plan_word = if n == 1 { "plan" } else { "plans" };
        // Empty when no problems → output is byte-identical to the pre-warning
        // behaviour for clean files.
        let warn_block = match &warning {
            Some(w) => format!("\n\n{w}"),
            None => String::new(),
        };
        format!(
            "<system-reminder>\n\
The workspace `TASKS.md` (re-injected on every prompt by Fleet PRD \
Discipline mode) holds {n} active {plan_word} below — merged across the \
main checkout and any sibling worktrees. This file is the durable macro \
plan — defer to it over your in-context memory of which P-tasks are done. \
After each P-task, update the checkbox in the source file shown for that \
plan. Only modify the block whose `id` matches the plan you are working \
on; treat every other block as another agent's in-flight work. When the \
same `id` appears in multiple TASKS.md files, the most recently modified \
file wins — keep a given `id` in exactly one file.\n\
\n\
Sources scanned:\n{sources_list}\n\
\n\
{rendered}{warn_block}\n\
</system-reminder>",
        )
    };

    let out = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": reminder,
        }
    });
    println!("{out}");
}


// ── Daily report CLI ────────────────────────────────────────────────────────

fn cmd_report(date: Option<String>, backfill: bool, regenerate: bool, gen_lessons: bool, gen_summary: bool, as_json: bool, lang: &str) {
    use claw_fleet_core::daily_report::{
        ReportStore, generate_report_from_sessions, scan_sessions_for_date,
        generate_lessons, generate_ai_summary,
    };
    use claw_fleet_core::llm_provider::{self, LlmConfig};
    let llm_cfg = LlmConfig::default();
    let llm_prov = llm_provider::resolve_provider(&llm_cfg.provider)
        .expect("default LLM provider not available");

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
                match generate_ai_summary(llm_prov.as_ref(), &llm_cfg.standard_model, &report, lang) {
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
                match generate_lessons(llm_prov.as_ref(), &llm_cfg.standard_model, &report, lang) {
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

fn print_lessons(lessons: &[claw_fleet_core::daily_report::Lesson]) {
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

fn print_report(report: &claw_fleet_core::daily_report::DailyReport) {
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
