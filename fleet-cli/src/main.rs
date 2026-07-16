mod commands;
mod fmt;

use clap::{Parser, Subcommand};

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
    /// Stop an agent and its whole process tree (SIGTERM, or SIGKILL with --force)
    Stop {
        /// Session ID prefix or workspace name
        id: String,
        /// Send SIGKILL instead of SIGTERM
        #[arg(short, long)]
        force: bool,
    },
    /// Interrupt an agent's in-flight tool call, leaving the session resumable
    Interrupt {
        /// Session ID prefix or workspace name
        id: String,
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
        /// Pin the successor's model, e.g. `claude-opus-4-8[1m]`. Overrides the
        /// auto-inherited model — otherwise recovered from Fleet's launch-spec or,
        /// for sessions Fleet did not launch, the transcript's most recent turn,
        /// which is unreliable when that turn ran on a rate-limit fallback. The
        /// bracketed suffix (`[1m]`) is passed through verbatim.
        #[arg(long)]
        model: Option<String>,
        /// Pin the successor's reasoning effort (e.g. low|medium|high|max).
        /// Overrides the value otherwise inherited from CLAUDE_EFFORT.
        #[arg(long)]
        effort: Option<String>,
        #[command(subcommand)]
        action: Option<HandoffCommands>,
    },
    /// Manage PRD Discipline mode guidance (the generated
    /// ~/.claude/fleet-prd-discipline.md + its @import in ~/.claude/CLAUDE.md).
    PrdDiscipline {
        #[command(subcommand)]
        action: PrdDisciplineCommands,
    },
    /// Manage the model-selection cheat-sheet in ~/.claude/CLAUDE.md that tells
    /// agents how to pick a Claude/Codex model for subagents, workflow agents,
    /// and new sessions.
    ModelGuidance {
        #[command(subcommand)]
        action: ModelGuidanceCommands,
    },
    /// Manage agent loops: a prompt Fleet re-runs on an interval by spawning a
    /// fresh session each time. Unlike Claude Code's own CronCreate /
    /// ScheduleWakeup / `/loop` — in-process timers that silently die with a
    /// headless `claude -p` session — a Fleet loop survives the turn boundary,
    /// because each iteration is a brand-new detached session. Create reads
    /// FLEET_SESSION_ID to inherit the current session's workspace/model/effort.
    Loop {
        #[command(subcommand)]
        action: LoopCommands,
    },
    /// Monitor / background Bash / ScheduleWakeup — background tasks that silently
    /// die with a headless `claude -p` session — a Fleet watch survives the turn
    /// boundary: it polls a shell condition in a detached timer and, when the
    /// condition fires, resumes THIS session (`claude --resume`) so its next turn
    /// sees the result. Create reads FLEET_SESSION_ID to know which session to
    /// reanimate and to inherit its workspace/model/effort.
    Watch {
        #[command(subcommand)]
        action: WatchCommands,
    },
}

#[derive(Subcommand)]
pub(crate) enum SkillCommands {
    /// Install Fleet skill to all detected AI tools (Claude Code, Copilot, Gemini CLI)
    Install,
}

#[derive(Subcommand)]
pub(crate) enum LoopCommands {
    /// Create a loop that re-runs <prompt> every <interval> in this workspace.
    Create {
        /// Interval between iterations, e.g. `5m`, `30m`, `2h`, `1d` (min 60s).
        #[arg(long)]
        interval: String,
        /// The prompt each iteration runs.
        #[arg(long)]
        prompt: String,
        /// Stop after this many iterations (default: run until stopped, capped
        /// at 500).
        #[arg(long)]
        max: Option<u32>,
    },
    /// List all registered loops.
    #[command(alias = "ls")]
    List {
        /// Output raw JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Stop a loop by its id (from `fleet loop list`).
    Stop {
        /// The loop id.
        id: String,
    },
    /// Internal: the detached timer body. Sleeps until the loop is due, fires
    /// one iteration, repeats. Spawned by `create` and the Stop-hook reconcile;
    /// not meant to be run by hand.
    #[command(hide = true)]
    Fire {
        /// The loop id.
        id: String,
        /// Generation this timer was armed for (staleness guard).
        generation: u64,
    },
}

#[derive(Subcommand)]
pub(crate) enum WatchCommands {
    /// Wait until <until> succeeds, then resume this session with the result.
    Create {
        /// Shell command polled each interval; exit status 0 ⇒ the condition is
        /// met and the watch fires. E.g. `gh run view 123 --json status -q .status
        /// | grep -qx completed`.
        #[arg(long)]
        until: String,
        /// Shell command whose stdout is handed to the resumed session as the
        /// event text. Omit to just say the watch fired. E.g. `gh run view 123
        /// --json conclusion -q .conclusion`.
        #[arg(long)]
        capture: Option<String>,
        /// Human note describing what is being waited for, shown to the woken
        /// session so it knows why it came back.
        #[arg(long)]
        note: Option<String>,
        /// Seconds between polls, e.g. `30s`, `2m` (min 5s, default 30s).
        #[arg(long)]
        poll: Option<String>,
        /// Give up after this long and resume with a timeout notice, e.g. `30m`,
        /// `2h` (default 2h, max 7d).
        #[arg(long)]
        timeout: Option<String>,
    },
    /// List all registered watches.
    #[command(alias = "ls")]
    List {
        /// Output raw JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Stop a watch by its id (from `fleet watch list`).
    Stop {
        /// The watch id.
        id: String,
    },
    /// Internal: the detached timer body. Polls the condition, fires once, exits.
    /// Spawned by `create` and the Stop-hook reconcile; not meant to be run by hand.
    #[command(hide = true)]
    Fire {
        /// The watch id.
        id: String,
        /// Generation this timer was armed for (staleness guard).
        generation: u64,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum GuardCommands {
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
pub(crate) enum SessionCommands {
    /// [internal] Mark the current fleet session idle (turn ended, awaiting next user prompt).
    /// Wired to Claude Code's `Stop` hook by `apply_idle_hooks`. Silent no-op if
    /// FLEET_SESSION_ID is unset (non-fleet-managed sessions running with global hooks).
    #[command(hide = true)]
    Idle,
    /// [internal] Clear the idle sentinel for the current session (next prompt arrived).
    /// Wired to Claude Code's `UserPromptSubmit` hook.
    #[command(hide = true)]
    Resume,
    /// [internal] Codex `notify` callback (the codex analogue of Claude's `Stop`
    /// hook). Injected as `-c notify=[fleet,session,codex-notify]` on Fleet-spawned
    /// codex sessions; codex appends the `agent-turn-complete` JSON payload as the
    /// final arg. Parses the thread id and fires the turn-exit relay.
    #[command(hide = true)]
    CodexNotify {
        /// Codex appends the JSON payload (and possibly fixed args) here.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        payload: Vec<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum HandoffCommands {
    /// Cancel this session's pending handoff.
    Cancel,
    /// List recorded handoff chains.
    List,
    /// Show the relay chain containing a session id.
    Show { session_id: String },
}

#[derive(Subcommand)]
pub(crate) enum PlanCommands {
    /// Tick a task done (`[ ]`→`[x]`) and record this session's focus.
    Check {
        /// Plan id (the sentinel `id="..."`).
        plan_id: String,
        /// Task label, e.g. `P3` (matched as the bold token `**P3**`).
        task: String,
    },
    /// Untick a task (`[x]`→`[ ]`).
    Uncheck { plan_id: String, task: String },
    /// Take over an existing plan: record that this session is now executing it
    /// (records focus only; no file change). Optional task label sets the
    /// current P explicitly, else the first pending one. Not needed after
    /// `create` (which claims focus) nor after a handoff (Fleet attributes the
    /// successor); use it when picking an existing plan back up.
    Resume {
        plan_id: String,
        task: Option<String>,
    },
    /// Create a new empty v2 plan block in the workspace TASKS.md, and record
    /// this session as its executor.
    Create {
        plan_id: String,
        #[arg(long)]
        title: String,
        /// Parent plan id. Marks this as a child (side-branch) plan: when it
        /// completes, Fleet points you back at the parent to keep going.
        #[arg(long)]
        parent: Option<String>,
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
pub(crate) enum WikiCommands {
    /// Publish a .html file, a .md file, or a directory containing an HTML
    /// entry. Re-publishing an existing slug creates a new version (old
    /// versions are kept and stay browsable in the Fleet app).
    Publish {
        /// Path to the .html file, .md file, or directory
        path: std::path::PathBuf,
        /// Stable doc id (lowercase letters/digits/hyphens). `/` separates
        /// virtual directories, e.g. `arch/overview`. Default: normalized
        /// file/dir name.
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
    /// List wiki docs published from the current workspace, newest first
    /// (--all for every workspace)
    #[command(alias = "ls")]
    List {
        /// Filter by workspace path substring instead of the current workspace
        #[arg(long)]
        workspace: Option<String>,
        /// List docs from every workspace
        #[arg(long, conflicts_with = "workspace")]
        all: bool,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
    /// Full-text search over doc titles, slugs and the text of each doc's
    /// current version. Scoped to the current workspace unless --all.
    #[command(alias = "grep")]
    Search {
        /// Text to look for (case-insensitive)
        query: String,
        /// Filter by workspace path substring instead of the current workspace
        #[arg(long)]
        workspace: Option<String>,
        /// Search docs from every workspace
        #[arg(long, conflicts_with = "workspace")]
        all: bool,
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
    /// Print a doc's content to stdout — the way to actually read a wiki doc.
    /// Defaults to the entry file of the current version.
    #[command(alias = "get")]
    Cat {
        slug: String,
        /// Version id to read (see `fleet wiki show <slug>`). Default: current.
        #[arg(long)]
        version: Option<String>,
        /// File inside the version, relative to its root (e.g. `assets/app.js`).
        /// Default: the doc's entry file.
        #[arg(long)]
        file: Option<String>,
    },
    /// Remove a doc (all versions), or a single old version with --version
    Rm {
        slug: String,
        /// Remove only this version (refused for the current version)
        #[arg(long)]
        version: Option<String>,
    },
    /// Re-key a doc, which moves it between virtual directories. Version
    /// history rides along. e.g. `fleet wiki mv overview arch/overview`
    #[command(alias = "move")]
    Mv {
        /// Current slug
        from: String,
        /// New slug — `/` separates virtual directories
        to: String,
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
pub(crate) enum WikiGuidanceCommands {
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
pub(crate) enum PrdDisciplineCommands {
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

#[derive(Subcommand)]
pub(crate) enum ModelGuidanceCommands {
    /// Write ~/.claude/fleet-model-guidance.md and inject its @import into
    /// ~/.claude/CLAUDE.md (idempotent)
    Apply {
        /// Guidance locale: `en` or `zh`
        #[arg(long, default_value = "en")]
        locale: String,
    },
    /// Strip the sentinel block and delete the guidance file (idempotent)
    Remove,
    /// Print whether the guidance block is installed (exit 1 if not)
    Status,
}

fn main() {
    claw_fleet_core::console::init_utf8();

    // `fleet fleet-proc-host <id>` — re-exec target for detached workspace
    // command hosts (see `claw_fleet_core::proc_runner`). Intercepted before
    // clap so the marker never collides with real subcommands.
    {
        let args: Vec<String> = std::env::args().collect();
        if args.len() >= 3 && args[1] == claw_fleet_core::proc_runner::HOST_ARGV_MARKER {
            claw_fleet_core::proc_runner::host_main(&args[2]);
        }
    }

    let cli = Cli::parse();

    if let Some(ref host) = cli.remote {
        match &cli.command {
            Commands::Serve { .. }
            | Commands::Skill { .. }
            | Commands::Guard { .. }
            | Commands::Elicitation
            | Commands::Mcp
            | Commands::PlanApproval
            | Commands::PrdContext => {
                eprintln!("Error: --remote is not supported with the '{}' subcommand.",
                    match &cli.command {
                        Commands::Serve { .. } => "serve",
                        Commands::Skill { .. } => "skill",
                        Commands::Guard { .. } => "guard",
                        Commands::Elicitation => "elicitation",
                        Commands::Mcp => "mcp",
                        Commands::PlanApproval => "plan-approval",
                        Commands::PrdContext => "prd-context",
                        _ => unreachable!(),
                    }
                );
                std::process::exit(1);
            }
            _ => {}
        }
        let remote_bin = commands::remote::ensure_remote_fleet(host);
        commands::remote::delegate_to_remote(host, &remote_bin);
    }

    match cli.command {
        Commands::Agents { all, json } => commands::agents::cmd_agents(all, json),
        Commands::Agent { id, json } => commands::agents::cmd_agent(&id, json),
        Commands::Stop { id, force } => commands::agents::cmd_stop(&id, force),
        Commands::Interrupt { id } => commands::agents::cmd_interrupt(&id),
        Commands::Account { json } => commands::account::cmd_account(json),
        Commands::Speed { json } => commands::agents::cmd_speed(json),
        Commands::Memory { file, json } => commands::memory::cmd_memory(file, json),
        Commands::Wiki { action } => commands::wiki::cmd_wiki(action),
        Commands::Search { query, limit, json } => commands::search::cmd_search(&query.join(" "), limit, json),
        Commands::Audit { level, filter, json } => commands::audit::cmd_audit(&level, filter.as_deref(), json),
        Commands::Report { date, backfill, regenerate, lessons, summary, json, lang } => commands::report::cmd_report(date, backfill, regenerate, lessons, summary, json, &lang),
        Commands::Serve { port, token, port_file } => commands::serve::cmd_serve(port, token, port_file),
        Commands::Skill { action } => match action {
            SkillCommands::Install => commands::skill::cmd_skill_install(),
        },
        Commands::Guard { action } => match action {
            None => commands::guard::cmd_guard(),
            Some(GuardCommands::ListRules { json }) => commands::guard::cmd_guard_list_rules(json),
            Some(GuardCommands::RemoveRule { id }) => commands::guard::cmd_guard_remove_rule(&id),
        },
        Commands::Elicitation => commands::guard::cmd_elicitation(),
        Commands::Mcp => commands::guard::cmd_mcp(),
        Commands::PlanApproval => commands::guard::cmd_plan_approval(),
        Commands::PrdContext => commands::prd::cmd_prd_context(),
        Commands::Session { action } => match action {
            SessionCommands::Idle => commands::session::cmd_session_idle(),
            SessionCommands::Resume => commands::session::cmd_session_resume(),
            SessionCommands::CodexNotify { payload } => {
                commands::session::cmd_codex_notify(&payload)
            }
        },
        Commands::Plan { action } => commands::plan::cmd_plan(action),
        Commands::Handoff {
            note,
            plan,
            next,
            model,
            effort,
            action,
        } => commands::handoff::cmd_handoff(
            note.as_deref(),
            plan.as_deref(),
            next.as_deref(),
            model.as_deref(),
            effort.as_deref(),
            action,
        ),
        Commands::Loop { action } => commands::loop_cmd::cmd_loop(action),
        Commands::Watch { action } => commands::watch::cmd_watch(action),
        Commands::PrdDiscipline { action } => match action {
            PrdDisciplineCommands::Apply { title, locale } => {
                commands::prd::cmd_prd_discipline_apply(&title, &locale)
            }
        },
        Commands::ModelGuidance { action } => match action {
            ModelGuidanceCommands::Apply { locale } => {
                match claw_fleet_core::model_guidance::apply_model_guidance(&locale) {
                    Ok(()) => println!(
                        "Model guidance installed (~/.claude/fleet-model-guidance.md + CLAUDE.md import)."
                    ),
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
            }
            ModelGuidanceCommands::Remove => {
                match claw_fleet_core::model_guidance::remove_model_guidance() {
                    Ok(()) => println!("Model guidance removed."),
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(1);
                    }
                }
            }
            ModelGuidanceCommands::Status => {
                if claw_fleet_core::model_guidance::is_model_guidance_installed() {
                    println!("installed");
                } else {
                    println!("not installed");
                    std::process::exit(1);
                }
            }
        },
    }
}
