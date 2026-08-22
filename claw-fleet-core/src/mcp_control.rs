//! MCP tool defs + handlers for the Fleet *control* commands (plan / handoff /
//! watch / loop / schedule / wiki), the "remote-necessary set" an agent drives
//! during a PRD plan.
//!
//! # Why these are MCP tools
//!
//! In an rca **remote** workspace session, an agent's `Bash` `fleet …` is
//! wrapped as `/bin/zsh -c '…'` and routed by cwd to the remote executor, which
//! has no `fleet` → exit 127. An MCP `tools/call` never touches the shell or
//! `spawn_route`: it is JSON-RPC to the local `fleet mcp` server, which owns the
//! local `~/.fleet/*` state and the local TASKS.md (managed by the local
//! `prd-context` hook). So the control commands work remotely only as MCP tools.
//!
//! # Conditional registration
//!
//! [`control_tool_defs`] is appended to `tools/list` **only for Fleet-owned
//! sessions** (`launch_spec::was_fleet_spawned`). A user's hand-launched
//! `claude` never sees these tools and keeps using the `fleet` CLI — so "Fleet
//! sessions go through MCP, non-Fleet through the CLI" holds at the
//! tool-visibility layer with zero per-session guidance.
//!
//! Each group is one tool (`fleet__plan`, …) with an `action` discriminator plus
//! flat params, mirroring the CLI subcommand structure the guidance already
//! teaches. Handlers resolve inherited launch context and session focus exactly
//! as the CLI does (they share [`crate::plan_ops`] and
//! [`crate::session::inherit_launch_context`]), so behavior is identical across
//! the two front ends.

use serde_json::{json, Value};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Names of the six control tools, in `tools/list` order.
pub const CONTROL_TOOL_NAMES: [&str; 6] = [
    "fleet__plan",
    "fleet__handoff",
    "fleet__watch",
    "fleet__loop",
    "fleet__schedule",
    "fleet__wiki",
];

/// True when `name` is one of the control tools this module owns.
pub fn is_control_tool(name: &str) -> bool {
    CONTROL_TOOL_NAMES.contains(&name)
}

/// The six control-tool definitions, appended to `tools/list` for Fleet-owned
/// sessions only.
pub fn control_tool_defs() -> Vec<Value> {
    vec![
        plan_tool_def(),
        handoff_tool_def(),
        watch_tool_def(),
        loop_tool_def(),
        schedule_tool_def(),
        wiki_tool_def(),
    ]
}

// ── Tool definitions ─────────────────────────────────────────────────────────

fn plan_tool_def() -> Value {
    json!({
        "name": "fleet__plan",
        "description": "Manage this workspace's TASKS.md PRD plans (the durable macro plan) and record which session works which plan/P. Use this instead of the `fleet plan` CLI. Actions: check/uncheck (tick a P's checkbox), create (new plan block), add (append a task), resume (claim focus on an existing plan), migrate (v1→v2), list, get.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["check", "uncheck", "create", "add", "resume", "migrate", "list", "get"]},
                "plan_id": {"type": "string", "description": "Plan sentinel id (kebab-case). Required for check/uncheck/create/add/resume/get."},
                "task": {"type": "string", "description": "P-task id like \"P2\". Required for check/uncheck/add; optional for resume."},
                "title": {"type": "string", "description": "Plan title. Required for create."},
                "parent": {"type": "string", "description": "Parent plan id, when this plan is side work spawned mid-parent (create only). Fleet points you back at the parent once this plan completes. Exactly one of parent/root is required for create."},
                "root": {"type": "boolean", "description": "Declare this plan a new top-level tree (create only). Required when parent is absent, so a plan's position in the tree is always an explicit choice."},
                "kind": {"type": "string", "enum": ["exec", "explore"], "description": "What the P-tasks are for (create only, default exec). `exec` changes code; `explore` investigates and its deliverable is the exec child plans it spawns, not edits of its own — use it so an exploration's findings can't silently redefine the implementation."},
                "text": {"type": "string", "description": "Task text. Required for add."}
            },
            "required": ["action"],
            "additionalProperties": false
        }
    })
}

fn handoff_tool_def() -> Value {
    json!({
        "name": "fleet__handoff",
        "description": "Register a session relay (接力), or read the relay chain this session sits on. The successor is spawned by the Stop hook the moment this session yields its turn. Use this instead of the `fleet handoff` CLI. Actions: register (--note is required; --plan/--next/--model/--effort optional), show, cancel, list. Reach for `show` whenever the work predates your own session — a relayed session's plan is often NOT where the chain started, so \"what did the boss originally ask?\" is answered by hop 1 of `show`, not by the plan you happen to be executing.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["register", "show", "cancel", "list"], "default": "register"},
                "session": {"type": "string", "description": "Session whose chain to show (show only; defaults to this session)."},
                "note": {"type": "string", "description": "Handoff briefing — what's done, what's next, key files, gotchas. Required for register."},
                "plan": {"type": "string", "description": "Plan id to attribute the successor to."},
                "next": {"type": "string", "description": "P-task the successor resumes at (requires plan)."},
                "model": {"type": "string", "description": "Override the successor's model (else inherits this session's)."},
                "effort": {"type": "string", "description": "Override the successor's effort (low|medium|high|max)."}
            },
            "required": ["action"],
            "additionalProperties": false
        }
    })
}

fn watch_tool_def() -> Value {
    json!({
        "name": "fleet__watch",
        "description": "Register a one-shot condition wait that resumes THIS session when a shell condition succeeds — the survivable replacement for Monitor / background Bash / ScheduleWakeup. Use this instead of the `fleet watch` CLI. Actions: create (--until required; --capture/--note/--poll/--timeout optional), stop, list.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["create", "stop", "list"], "default": "create"},
                "until": {"type": "string", "description": "Shell command that exits 0 when the condition is met. Required for create."},
                "capture": {"type": "string", "description": "Shell command whose stdout is reported back on the resumed turn."},
                "note": {"type": "string", "description": "What you're waiting for."},
                "poll": {"type": "string", "description": "Poll interval, e.g. 30s / 5m (default 30s)."},
                "timeout": {"type": "string", "description": "Give-up deadline, e.g. 2h (default 2h)."},
                "id": {"type": "string", "description": "Watch id. Required for stop."}
            },
            "required": ["action"],
            "additionalProperties": false
        }
    })
}

fn loop_tool_def() -> Value {
    json!({
        "name": "fleet__loop",
        "description": "A recurring / cron-style scheduler (CLI alias: `fleet cron`): re-runs a prompt on an interval by spawning a fresh detached LOCAL session each time. Reach for this for any 'do X every N minutes / hourly / daily / periodically / on a schedule' need. Durable — survives the session, unlike Claude Code's own `/loop`, ScheduleWakeup, or CronCreate, which silently die in a headless `claude -p` turn. Because each tick is a local session, local creds (e.g. muveectl) are present. Tip: `until` is a cheap non-LLM gate — a shell probe run each tick that only spawns the (paid) LLM session when it exits 0, so you can poll often yet pay for an LLM only when there is real work. Use this instead of the `fleet loop` CLI. Actions: create (--prompt and --interval required; --max/--until optional), stop, list, update, get, run.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["create", "stop", "list", "update", "get", "run"], "default": "create"},
                "prompt": {"type": "string", "description": "The prompt to re-run each interval. Required for create."},
                "interval": {"type": "string", "description": "Interval between runs, e.g. 5m / 1h. Required for create."},
                "max": {"type": "integer", "description": "Optional cap on iterations."},
                "until": {"type": "string", "description": "Optional non-LLM gate: a shell command checked at each interval tick. The iteration spawns only when it exits 0; otherwise the tick is skipped (no session, no iteration consumed) and re-checked next interval."},
                "id": {"type": "string", "description": "Loop id. Required for stop/update/get/run."}
            },
            "required": ["action"],
            "additionalProperties": false
        }
    })
}

fn schedule_tool_def() -> Value {
    json!({
        "name": "fleet__schedule",
        "description": "A one-shot scheduler: fires a prompt ONCE at an absolute future time by spawning a fresh detached session — for recurring / periodic runs use `fleet__loop` instead. Durable — survives the session, unlike Claude Code's ScheduleWakeup / CronCreate. Tip: `until` is a cheap non-LLM gate — once due, a shell probe is polled and the (paid) LLM session spawns only when it exits 0. Use this instead of the `fleet schedule` CLI. Actions: create (--prompt required, exactly one of --at/--in; --model/--effort/--until optional), cancel, list, update, get, run.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["create", "cancel", "list", "update", "get", "run"], "default": "create"},
                "prompt": {"type": "string", "description": "The prompt to fire. Required for create."},
                "at": {"type": "string", "description": "Absolute time, e.g. \"2026-07-25 09:00\"."},
                "in": {"type": "string", "description": "Relative delay, e.g. 5d."},
                "model": {"type": "string", "description": "Override model (else inherits this session's)."},
                "effort": {"type": "string", "description": "Override effort."},
                "until": {"type": "string", "description": "Optional non-LLM gate: once due, this shell command is polled and the session spawns only when it exits 0. If it never passes within the timeout the schedule is abandoned (no session)."},
                "poll": {"type": "string", "description": "Seconds between gate polls once due, e.g. 30s / 2m (min 5s, default 30s). Only with `until`."},
                "timeout": {"type": "string", "description": "Give up on an unmet gate after this long past due, e.g. 30m / 2h (default 2h, max 7d). Only with `until`."},
                "id": {"type": "string", "description": "Schedule id. Required for cancel/update/get/run."}
            },
            "required": ["action"],
            "additionalProperties": false
        }
    })
}

fn wiki_tool_def() -> Value {
    json!({
        "name": "fleet__wiki",
        "description": "Publish and browse docs in the Fleet wiki knowledge base. Use this instead of the `fleet wiki` CLI. Actions: publish (--path required; --slug/--title optional), cat (--slug required; --version/--file optional), list, search (--query required). Note: `publish` reads a LOCAL file path; in a remote workspace the file the agent wrote may live on the remote fs.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["publish", "cat", "list", "search"]},
                "path": {"type": "string", "description": "File or dir to publish. Required for publish."},
                "slug": {"type": "string", "description": "Doc slug (virtual path with /). Required for cat; optional for publish."},
                "title": {"type": "string", "description": "Doc title (publish only)."},
                "version": {"type": "string", "description": "Version id (cat only; defaults to current)."},
                "file": {"type": "string", "description": "Relative file within a dir doc (cat only; defaults to entry)."},
                "query": {"type": "string", "description": "Search terms. Required for search."}
            },
            "required": ["action"],
            "additionalProperties": false
        }
    })
}

// ── Dispatch ─────────────────────────────────────────────────────────────────

/// Execute a control tool. Returns `Ok(text)` for the agent on success, or
/// `Err(text)` on a user/logic error (the caller renders it as an MCP tool error
/// with `isError:true`). `session_id` is the invoking session (None when the env
/// carries no id); `cwd` is the resolved workspace (CLAUDE_PROJECT_DIR, else the
/// server's current dir) used to locate TASKS.md and scope wiki listings.
pub fn handle(
    name: &str,
    args: &Value,
    session_id: Option<&str>,
    cwd: &Path,
) -> Result<String, String> {
    match name {
        "fleet__plan" => handle_plan(args, session_id, cwd),
        "fleet__handoff" => handle_handoff(args, session_id, cwd),
        "fleet__watch" => handle_watch(args, session_id),
        "fleet__loop" => handle_loop(args, session_id),
        "fleet__schedule" => handle_schedule(args, session_id),
        "fleet__wiki" => handle_wiki(args, cwd),
        other => Err(format!("unknown control tool: {other}")),
    }
}

// ── arg helpers ──────────────────────────────────────────────────────────────

/// A non-empty string arg, coercing a JSON number to its decimal string so
/// `interval: 300` and `interval: "300"` both work.
fn arg(args: &Value, key: &str) -> Option<String> {
    match args.get(key) {
        Some(Value::String(v)) if !v.trim().is_empty() => Some(v.trim().to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

fn req(args: &Value, key: &str) -> Result<String, String> {
    arg(args, key).ok_or_else(|| format!("`{key}` is required for this action"))
}

fn action_of(args: &Value) -> Result<String, String> {
    arg(args, "action").ok_or_else(|| "`action` is required".to_string())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Render a [`crate::plan_ops::PlanOutcome`] into agent text: warnings first
/// (prefixed like the CLI's stderr), then the message.
fn render_plan_outcome(o: crate::plan_ops::PlanOutcome) -> String {
    let mut out = String::new();
    for w in &o.warnings {
        out.push_str(&format!("warning: {w}\n"));
    }
    out.push_str(&o.message);
    out
}

// ── plan ─────────────────────────────────────────────────────────────────────

fn handle_plan(args: &Value, sid: Option<&str>, cwd: &Path) -> Result<String, String> {
    use crate::plan_ops as po;
    let action = action_of(args)?;
    match action.as_str() {
        "check" => po::mutate_checkbox(cwd, &req(args, "plan_id")?, &req(args, "task")?, true, sid)
            .map(render_plan_outcome),
        "uncheck" => {
            po::mutate_checkbox(cwd, &req(args, "plan_id")?, &req(args, "task")?, false, sid)
                .map(render_plan_outcome)
        }
        "create" => po::create(
            cwd,
            &req(args, "plan_id")?,
            &req(args, "title")?,
            arg(args, "parent").as_deref(),
            args.get("root").and_then(|v| v.as_bool()).unwrap_or(false),
            crate::prd_tasks::PlanKind::from_attr(arg(args, "kind").as_deref()),
            sid,
        )
        .map(render_plan_outcome),
        "add" => po::add(
            cwd,
            &req(args, "plan_id")?,
            &req(args, "task")?,
            &req(args, "text")?,
        )
        .map(render_plan_outcome),
        "resume" => po::resume(cwd, &req(args, "plan_id")?, arg(args, "task").as_deref(), sid)
            .map(render_plan_outcome),
        "migrate" => po::migrate(cwd, None).map(render_plan_outcome),
        "list" => {
            let plans = crate::prd_tasks::list_workspace_task_plans(cwd, None);
            if plans.is_empty() {
                return Ok("(no plans)".to_string());
            }
            let mut out = String::new();
            for p in plans {
                let done = p.items.iter().filter(|i| i.done).count();
                let src = p.source.map(|s| format!(" — {s}")).unwrap_or_default();
                out.push_str(&format!(
                    "{} [{}/{}]{}\n",
                    p.id.as_deref().unwrap_or("(anonymous)"),
                    done,
                    p.items.len(),
                    src
                ));
            }
            Ok(out.trim_end().to_string())
        }
        "get" => {
            let plan_id = req(args, "plan_id")?;
            let plans = crate::prd_tasks::list_workspace_task_plans(cwd, None);
            let p = plans
                .iter()
                .find(|p| p.id.as_deref() == Some(plan_id.as_str()))
                .ok_or_else(|| format!("plan '{plan_id}' not found"))?;
            let mut out = String::new();
            for it in &p.items {
                out.push_str(&format!("{} {}\n", if it.done { "[x]" } else { "[ ]" }, it.text));
            }
            Ok(out.trim_end().to_string())
        }
        other => Err(format!("unknown plan action: {other}")),
    }
}

// ── handoff ──────────────────────────────────────────────────────────────────

fn handle_handoff(args: &Value, sid: Option<&str>, cwd: &Path) -> Result<String, String> {
    use crate::handoff;
    let action = action_of(args)?;
    match action.as_str() {
        "register" => {
            let sid = sid.ok_or(
                "no session id (neither FLEET_SESSION_ID nor CLAUDE_CODE_SESSION_ID set)",
            )?;
            let note = req(args, "note").map_err(|_| {
                "`note` is required — the successor only knows what you write here.".to_string()
            })?;
            let plan = arg(args, "plan");
            let next = arg(args, "next");
            if next.is_some() && plan.is_none() {
                return Err("`next` requires `plan`.".to_string());
            }
            if let Some(plan_id) = plan.as_deref() {
                if crate::prd_tasks::find_plan_source(cwd, plan_id).is_none() {
                    return Err(format!(
                        "plan '{plan_id}' not found in any TASKS.md reachable from {}.",
                        cwd.display()
                    ));
                }
            }
            let ctx = crate::session::inherit_launch_context(Some(sid));
            let model = arg(args, "model").or(ctx.model);
            let effort = arg(args, "effort").or(ctx.effort);
            let agent_source = ctx.source.unwrap_or_else(|| "claude-code".to_string());
            let rec = handoff::register(
                sid,
                &ctx.workspace,
                Some(&ctx.shell_cwd),
                &note,
                plan.as_deref(),
                next.as_deref(),
                model.as_deref(),
                effort.as_deref(),
                &agent_source,
            )?;
            Ok(format!(
                "ok: handoff registered (chain {}, 第 {} 棒, model={}, effort={}). \
                 接力 session 将在本 session 结束 turn 后由 Stop hook 自动启动；请尽快结束当前 turn。",
                rec.chain_id,
                rec.hop,
                rec.model.as_deref().unwrap_or("<CLI 默认>"),
                rec.effort.as_deref().unwrap_or("<CLI 默认>"),
            ))
        }
        "show" => {
            // Defaults to the caller's own chain: the common need is "where did
            // this work start", and a relayed session knows its own id but not
            // its chain id.
            let target = arg(args, "session")
                .or_else(|| sid.map(str::to_string))
                .ok_or(
                    "no session id (neither FLEET_SESSION_ID nor CLAUDE_CODE_SESSION_ID set) — \
                     pass `session` to show another session's chain",
                )?;
            match handoff::chain_containing(&target) {
                // Notes in full: this action *is* the "read the whole chain"
                // entry point, so clipping here would leave no way to reach the
                // predecessors' briefings.
                Some(c) => Ok(handoff::render_chain(&c, Some(&target), None)),
                None => Err(format!(
                    "session {target} is not on any relay chain (it was not handed off to, and \
                     has not handed off yet)"
                )),
            }
        }
        "cancel" => {
            let sid = sid.ok_or(
                "no session id (neither FLEET_SESSION_ID nor CLAUDE_CODE_SESSION_ID set)",
            )?;
            handoff::cancel_pending(sid);
            Ok("ok: pending handoff cancelled (if any)".to_string())
        }
        "list" => {
            let chains = handoff::list_chains();
            if chains.is_empty() {
                return Ok("no handoff chains recorded".to_string());
            }
            serde_json::to_string_pretty(&chains).map_err(|e| e.to_string())
        }
        other => Err(format!("unknown handoff action: {other}")),
    }
}

// ── watch ────────────────────────────────────────────────────────────────────

fn handle_watch(args: &Value, sid: Option<&str>) -> Result<String, String> {
    use crate::watch;
    let action = action_of(args)?;
    match action.as_str() {
        "create" => {
            let until = req(args, "until")?;
            let sid = sid.ok_or(
                "cannot resolve this session's id (FLEET_SESSION_ID / CLAUDE_CODE_SESSION_ID \
                 unset). `fleet watch` reanimates the calling session.",
            )?;
            // Subagent transcripts (`agent-*`) can't be independently resumed.
            // (The IDE-attached guard the CLI runs is moot here: control tools are
            // registered only for Fleet-owned sessions, whose `ide_name` is
            // stripped, so it would always pass — we skip the full scan.)
            if sid.starts_with("agent-") {
                return Err(format!(
                    "this looks like a subagent session ({sid}); subagents are awaited by their \
                     parent and cannot be resumed. Register the watch from the top-level session."
                ));
            }
            let poll_secs = match arg(args, "poll") {
                Some(s) => watch::parse_poll(&s).map_err(|e| format!("poll: {e}"))?,
                None => watch::DEFAULT_POLL_SECS,
            };
            let timeout_secs = match arg(args, "timeout") {
                Some(s) => watch::parse_timeout(&s).map_err(|e| format!("timeout: {e}"))?,
                None => watch::DEFAULT_TIMEOUT_SECS,
            };
            let ctx = crate::session::inherit_launch_context(Some(sid));
            let rec = watch::create(
                sid,
                &ctx.workspace,
                &until,
                arg(args, "capture").as_deref(),
                arg(args, "note").as_deref(),
                poll_secs,
                timeout_secs,
                ctx.model.as_deref(),
                ctx.effort.as_deref(),
                ctx.source.as_deref(),
            )?;
            let armed = match watch::arm_timer(&rec) {
                Ok(pid) => format!("计时器已启动 (pid {pid})"),
                Err(e) => format!("但计时器启动失败: {e}(Stop hook reconcile 会补上)"),
            };
            Ok(format!(
                "ok: watch {} created — polling, resumes session {}. {armed}。\
                 现在可以正常结束这个 turn。停止用 action=stop id={}。",
                rec.id, rec.session_id, rec.id
            ))
        }
        "stop" => {
            let id = req(args, "id")?;
            if watch::stop(&id) {
                Ok(format!("ok: watch {id} stopped (its timer exits on next poll)"))
            } else {
                Err(format!("no watch with id {id}"))
            }
        }
        "list" => {
            let watches = watch::list();
            if watches.is_empty() {
                return Ok("no watches registered".to_string());
            }
            serde_json::to_string_pretty(&watches).map_err(|e| e.to_string())
        }
        other => Err(format!("unknown watch action: {other}")),
    }
}

// ── loop ─────────────────────────────────────────────────────────────────────

fn handle_loop(args: &Value, sid: Option<&str>) -> Result<String, String> {
    use crate::agent_loop;
    let action = action_of(args)?;
    match action.as_str() {
        "create" => {
            let prompt = req(args, "prompt")?;
            let interval_secs =
                agent_loop::parse_interval(&req(args, "interval")?).map_err(|e| e.to_string())?;
            let max = args.get("max").and_then(Value::as_u64).map(|v| v as u32);
            let until = arg(args, "until").filter(|c| !c.trim().is_empty());
            let ctx = crate::session::inherit_launch_context(sid);
            let rec = agent_loop::create(
                &ctx.workspace,
                &prompt,
                interval_secs,
                max,
                ctx.model.as_deref(),
                ctx.effort.as_deref(),
                ctx.source.as_deref(),
                sid,
                until.as_deref(),
            )?;
            let armed = match agent_loop::arm_timer(&rec) {
                Ok(pid) => format!("计时器已启动 (pid {pid})"),
                Err(e) => format!("但计时器启动失败: {e}(Stop hook reconcile 会补上)"),
            };
            Ok(format!(
                "ok: loop {} created — model={}. {armed}。停止用 action=stop id={}。",
                rec.id,
                rec.model.as_deref().unwrap_or("<CLI 默认>"),
                rec.id
            ))
        }
        "stop" => {
            let id = req(args, "id")?;
            if agent_loop::stop(&id) {
                Ok(format!("ok: loop {id} stopped (its timer exits on next wake)"))
            } else {
                Err(format!("no loop with id {id}"))
            }
        }
        "update" => {
            let id = req(args, "id")?;
            let interval_secs = match arg(args, "interval") {
                Some(i) => Some(agent_loop::parse_interval(&i).map_err(|e| e.to_string())?),
                None => None,
            };
            let max = args.get("max").and_then(Value::as_u64).map(|v| v as u32);
            let rec = agent_loop::update(&id, interval_secs, arg(args, "prompt").as_deref(), max)?;
            let _ = agent_loop::arm_timer(&rec);
            Ok(format!("ok: loop {} updated。计时器已重挂。", rec.id))
        }
        "get" => {
            let id = req(args, "id")?;
            match agent_loop::get(&id) {
                Some(rec) => serde_json::to_string_pretty(&rec).map_err(|e| e.to_string()),
                None => Err(format!("no loop with id {id}")),
            }
        }
        "run" => {
            let id = req(args, "id")?;
            match agent_loop::run_now(&id)? {
                Some(s) => Ok(format!("ok: loop {id} run now -> session {s}")),
                None => Ok(format!("ok: loop {id} run now")),
            }
        }
        "list" => {
            let loops = agent_loop::list();
            if loops.is_empty() {
                return Ok("no loops registered".to_string());
            }
            serde_json::to_string_pretty(&loops).map_err(|e| e.to_string())
        }
        other => Err(format!("unknown loop action: {other}")),
    }
}

// ── schedule ─────────────────────────────────────────────────────────────────

/// Parse the at/in pair into an absolute fire time. `require` controls whether
/// missing both is an error (create) or means "no change" (update).
fn resolve_fire_at(args: &Value, require: bool) -> Result<Option<u64>, String> {
    use crate::schedule;
    let now = now_ms();
    match (arg(args, "at"), arg(args, "in")) {
        (Some(_), Some(_)) => Err("pass at most one of `at` or `in`, not both.".to_string()),
        (Some(a), None) => schedule::parse_at(&a, now).map(Some),
        (None, Some(i)) => schedule::parse_in(&i, now).map(Some),
        (None, None) => {
            if require {
                Err("pass `at` (\"2026-07-25 09:00\") or `in` (5d).".to_string())
            } else {
                Ok(None)
            }
        }
    }
}

/// Build the optional `--until` gate from the `until` / `poll` / `timeout`
/// args, reusing [`crate::watch`]'s duration grammar/floors so the schedule gate
/// and `fleet watch` accept identical values. `poll` / `timeout` without
/// `until` is an error. Returns `Ok(None)` when there's no gate.
fn build_schedule_gate(args: &Value) -> Result<Option<crate::schedule::ScheduleGate>, String> {
    let until = arg(args, "until").filter(|c| !c.trim().is_empty());
    let poll = arg(args, "poll");
    let timeout = arg(args, "timeout");
    let Some(until_cmd) = until else {
        if poll.is_some() || timeout.is_some() {
            return Err("`poll` / `timeout` require `until`.".to_string());
        }
        return Ok(None);
    };
    let poll_secs = match poll.as_deref() {
        Some(s) => crate::watch::parse_poll(s).map_err(|e| format!("poll: {e}"))?,
        None => crate::schedule::DEFAULT_GATE_POLL_SECS,
    };
    let timeout_secs = match timeout.as_deref() {
        Some(s) => crate::watch::parse_timeout(s).map_err(|e| format!("timeout: {e}"))?,
        None => crate::schedule::DEFAULT_GATE_TIMEOUT_SECS,
    };
    Ok(Some(crate::schedule::ScheduleGate {
        until_cmd,
        poll_secs,
        timeout_secs,
    }))
}

fn handle_schedule(args: &Value, sid: Option<&str>) -> Result<String, String> {
    use crate::schedule;
    let action = action_of(args)?;
    match action.as_str() {
        "create" => {
            let prompt = req(args, "prompt")?;
            let fire_at = resolve_fire_at(args, true)?.expect("create requires at/in");
            let ctx = crate::session::inherit_launch_context(sid);
            let model = arg(args, "model").or(ctx.model);
            let effort = arg(args, "effort").or(ctx.effort);
            let gate = build_schedule_gate(args)?;
            let rec = schedule::create(
                &ctx.workspace,
                &prompt,
                fire_at,
                model.as_deref(),
                effort.as_deref(),
                ctx.source.as_deref(),
                sid,
                gate,
            )?;
            let armed = match schedule::arm_timer(&rec) {
                Ok(pid) => format!("计时器已启动 (pid {pid})"),
                Err(e) => format!("但计时器启动失败: {e}(Stop hook reconcile 会补上)"),
            };
            Ok(format!(
                "ok: schedule {} created — fires at epoch-ms {}, model={}. {armed}。取消用 action=cancel id={}。",
                rec.id,
                rec.fire_at,
                rec.model.as_deref().unwrap_or("<CLI 默认>"),
                rec.id
            ))
        }
        "cancel" => {
            let id = req(args, "id")?;
            if schedule::cancel(&id) {
                Ok(format!("ok: schedule {id} cancelled (its timer exits on next wake)"))
            } else {
                Err(format!("no schedule with id {id}"))
            }
        }
        "update" => {
            let id = req(args, "id")?;
            let fire_at = resolve_fire_at(args, false)?;
            let prompt = arg(args, "prompt");
            let model = arg(args, "model");
            let effort = arg(args, "effort");
            if fire_at.is_none() && prompt.is_none() && model.is_none() && effort.is_none() {
                return Err("nothing to update — pass at/in, prompt, model and/or effort.".to_string());
            }
            let rec = schedule::update(&schedule::ScheduleUpdate {
                id: id.clone(),
                fire_at,
                prompt,
                model,
                effort,
                ..Default::default()
            })?;
            let _ = schedule::arm_timer(&rec);
            Ok(format!("ok: schedule {} updated — fires at epoch-ms {}。计时器已重挂。", rec.id, rec.fire_at))
        }
        "get" => {
            let id = req(args, "id")?;
            match schedule::get(&id) {
                Some(rec) => serde_json::to_string_pretty(&rec).map_err(|e| e.to_string()),
                None => Err(format!("no schedule with id {id}")),
            }
        }
        "run" => {
            let id = req(args, "id")?;
            match schedule::run_now(&id)? {
                Some(s) => Ok(format!("ok: schedule {id} run now -> session {s}")),
                None => Ok(format!("ok: schedule {id} run now")),
            }
        }
        "list" => {
            let items = schedule::list();
            if items.is_empty() {
                return Ok("no schedules registered".to_string());
            }
            serde_json::to_string_pretty(&items).map_err(|e| e.to_string())
        }
        other => Err(format!("unknown schedule action: {other}")),
    }
}

// ── wiki ─────────────────────────────────────────────────────────────────────

fn handle_wiki(args: &Value, cwd: &Path) -> Result<String, String> {
    use crate::wiki;
    let action = action_of(args)?;
    match action.as_str() {
        "publish" => {
            let path = req(args, "path")?;
            let doc = wiki::publish(
                Path::new(&path),
                arg(args, "slug").as_deref(),
                arg(args, "title").as_deref(),
                cwd,
            )?;
            Ok(format!(
                "Published {} (version {}, {} total). title: {}",
                doc.slug,
                doc.current_version,
                doc.versions.len(),
                doc.title
            ))
        }
        "cat" => {
            let slug = req(args, "slug")?;
            let doc = wiki::get_doc(&slug)?;
            let version = arg(args, "version").unwrap_or_else(|| doc.current_version.clone());
            let relpath = arg(args, "file").unwrap_or_else(|| doc.entry.clone());
            let f = wiki::get_file(&slug, &version, &relpath)?;
            Ok(String::from_utf8_lossy(&f.bytes).into_owned())
        }
        "list" => {
            let scope = wiki::resolve_workspace_path(cwd);
            let docs: Vec<_> = wiki::list_docs()
                .into_iter()
                .filter(|d| wiki::workspace_contains(&d.workspace_path, &scope))
                .collect();
            if docs.is_empty() {
                return Ok(format!(
                    "No wiki docs published from {scope}. (search/cat can still reach other workspaces.)"
                ));
            }
            let mut out = String::new();
            for d in &docs {
                out.push_str(&format!(
                    "{}  [{}]  v{}  {}\n",
                    d.slug,
                    d.kind,
                    d.versions.len(),
                    d.title
                ));
            }
            Ok(out.trim_end().to_string())
        }
        "search" => {
            let query = req(args, "query")?;
            let by_slug: std::collections::HashMap<String, wiki::WikiDoc> =
                wiki::list_docs().into_iter().map(|d| (d.slug.clone(), d)).collect();
            let hits: Vec<_> = wiki::search_docs(&query)
                .into_iter()
                .filter(|h| by_slug.contains_key(&h.slug))
                .collect();
            if hits.is_empty() {
                return Ok(format!("No doc matches '{query}'."));
            }
            let mut out = String::new();
            for h in &hits {
                let doc = &by_slug[&h.slug];
                let matched = if h.snippet.is_empty() { &doc.title } else { &h.snippet };
                out.push_str(&format!("{}  [{}]  {}\n", h.slug, h.field, matched));
            }
            Ok(out.trim_end().to_string())
        }
        other => Err(format!("unknown wiki action: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_tool_defs_are_the_six_named_groups() {
        let defs = control_tool_defs();
        let names: Vec<&str> = defs.iter().filter_map(|d| d["name"].as_str()).collect();
        assert_eq!(names, CONTROL_TOOL_NAMES);
        // Each carries an object inputSchema with a required `action`.
        for d in &defs {
            assert_eq!(d["inputSchema"]["type"], "object");
            assert_eq!(d["inputSchema"]["required"][0], "action");
        }
    }

    #[test]
    fn is_control_tool_matches_only_the_six() {
        assert!(is_control_tool("fleet__plan"));
        assert!(is_control_tool("fleet__wiki"));
        assert!(!is_control_tool("fleet__ask"));
        assert!(!is_control_tool("fleet__set_session_title"));
    }

    /// The MCP surface enforces the same tree-position declaration the CLI does
    /// — the whole reason plan mutations were lifted into `plan_ops`. Without
    /// this, an agent on a remote-workspace session (which can only reach the
    /// MCP tool) would keep authoring flat plans.
    #[test]
    fn plan_create_via_mcp_requires_parent_or_root() {
        let tmp = tempfile::tempdir().unwrap();
        let err = handle(
            "fleet__plan",
            &json!({"action": "create", "plan_id": "demo", "title": "Demo"}),
            None,
            tmp.path(),
        )
        .unwrap_err();
        assert!(err.contains("--parent"), "{err}");
        assert!(err.contains("--root"), "{err}");
        assert!(
            !tmp.path().join("TASKS.md").exists(),
            "a refused create must not create the file"
        );
    }

    #[test]
    fn plan_check_maps_args_and_mutates_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        // create → add → check, all via the MCP arg surface.
        handle(
            "fleet__plan",
            &json!({"action": "create", "plan_id": "demo", "title": "Demo", "root": true}),
            None,
            cwd,
        )
        .unwrap();
        handle(
            "fleet__plan",
            &json!({"action": "add", "plan_id": "demo", "task": "P1", "text": "first"}),
            None,
            cwd,
        )
        .unwrap();
        let out = handle(
            "fleet__plan",
            &json!({"action": "check", "plan_id": "demo", "task": "P1"}),
            None,
            cwd,
        )
        .unwrap();
        assert!(out.contains("ok"));
        let body = std::fs::read_to_string(cwd.join("TASKS.md")).unwrap();
        assert!(body.contains("[x] **P1**"));
    }

    #[test]
    fn plan_missing_required_arg_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let err = handle("fleet__plan", &json!({"action": "check"}), None, tmp.path()).unwrap_err();
        assert!(err.contains("plan_id"));
    }

    #[test]
    fn handoff_register_without_note_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let err = handle(
            "fleet__handoff",
            &json!({"action": "register"}),
            Some("sess-1"),
            tmp.path(),
        )
        .unwrap_err();
        assert!(err.contains("note"));
    }

    #[test]
    fn handoff_register_without_session_id_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let err = handle(
            "fleet__handoff",
            &json!({"action": "register", "note": "hi"}),
            None,
            tmp.path(),
        )
        .unwrap_err();
        assert!(err.contains("no session id"));
    }

    #[test]
    fn handoff_show_without_session_id_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let err = handle("fleet__handoff", &json!({"action": "show"}), None, tmp.path())
            .unwrap_err();
        assert!(err.contains("no session id"), "{err}");
    }

    /// A session that was never relayed must be told so plainly, not handed an
    /// empty render it could mistake for "the chain starts at me".
    #[test]
    fn handoff_show_off_chain_session_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let err = handle(
            "fleet__handoff",
            &json!({"action": "show", "session": "sess-never-relayed-9d1c"}),
            None,
            tmp.path(),
        )
        .unwrap_err();
        assert!(err.contains("not on any relay chain"), "{err}");
    }

    #[test]
    fn watch_create_without_until_errors() {
        let err = handle(
            "fleet__watch",
            &json!({"action": "create"}),
            Some("sess-1"),
            Path::new("/tmp"),
        )
        .unwrap_err();
        assert!(err.contains("until"));
    }

    #[test]
    fn watch_create_from_subagent_is_refused() {
        let err = handle(
            "fleet__watch",
            &json!({"action": "create", "until": "true"}),
            Some("agent-abc"),
            Path::new("/tmp"),
        )
        .unwrap_err();
        assert!(err.contains("subagent"));
    }

    #[test]
    fn schedule_create_without_time_errors() {
        let err = handle(
            "fleet__schedule",
            &json!({"action": "create", "prompt": "do it"}),
            Some("sess-1"),
            Path::new("/tmp"),
        )
        .unwrap_err();
        assert!(err.contains("at") && err.contains("in"));
    }

    #[test]
    fn loop_create_coerces_numeric_interval() {
        // `interval` given as a JSON number must be accepted (coerced to string).
        let err = handle(
            "fleet__loop",
            &json!({"action": "create", "prompt": "x", "interval": "notaduration"}),
            None,
            Path::new("/tmp"),
        )
        .unwrap_err();
        // Reaches parse_interval (proves arg extraction worked), which rejects it.
        assert!(!err.contains("required"));
    }

    #[test]
    fn unknown_action_errors() {
        let err = handle(
            "fleet__plan",
            &json!({"action": "frobnicate"}),
            None,
            Path::new("/tmp"),
        )
        .unwrap_err();
        assert!(err.contains("unknown plan action"));
    }

    #[test]
    fn build_schedule_gate_absent_is_none() {
        assert_eq!(build_schedule_gate(&json!({"action": "create"})).unwrap(), None);
        // blank until ⇒ no gate
        assert_eq!(build_schedule_gate(&json!({"until": "   "})).unwrap(), None);
    }

    #[test]
    fn build_schedule_gate_defaults_and_parses() {
        // until alone ⇒ defaults for poll/timeout
        let g = build_schedule_gate(&json!({"until": "test -f /tmp/x"})).unwrap().unwrap();
        assert_eq!(g.until_cmd, "test -f /tmp/x");
        assert_eq!(g.poll_secs, crate::schedule::DEFAULT_GATE_POLL_SECS);
        assert_eq!(g.timeout_secs, crate::schedule::DEFAULT_GATE_TIMEOUT_SECS);
        // explicit poll/timeout parsed via the watch grammar
        let g = build_schedule_gate(&json!({"until": "true", "poll": "10s", "timeout": "45m"}))
            .unwrap()
            .unwrap();
        assert_eq!(g.poll_secs, 10);
        assert_eq!(g.timeout_secs, 45 * 60);
    }

    #[test]
    fn build_schedule_gate_poll_without_until_errors() {
        let err = build_schedule_gate(&json!({"poll": "10s"})).unwrap_err();
        assert!(err.contains("require"));
        let err = build_schedule_gate(&json!({"timeout": "1h"})).unwrap_err();
        assert!(err.contains("require"));
    }
}
