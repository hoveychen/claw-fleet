//! `fleet handoff` — register (or manage) a session relay (接力). The successor
//! session is spawned by the Stop hook when this session next yields its turn.

use crate::commands::session::read_fleet_session_id;
use crate::HandoffCommands;

/// Bare `fleet handoff --note ...` registers; subcommands manage/inspect.
/// Registration is a promise, not an action: the successor session is spawned
/// by the Stop hook (`fleet session idle`) the moment this session yields.
pub(crate) fn cmd_handoff(
    note: Option<&str>,
    plan: Option<&str>,
    next: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
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
            // Same rendering as the `fleet__handoff` MCP `show` action, so the
            // CLI and the tool agents are told to prefer agree byte for byte.
            // Nothing consumes the old pretty-JSON form.
            match claw_fleet_core::handoff::chain_containing(&session_id) {
                Some(c) => print!(
                    "{}",
                    claw_fleet_core::handoff::render_chain(&c, Some(&session_id), None)
                ),
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
    // Spawn the successor where the *session* runs, on the same model / effort /
    // agent tool — the shared inheritance the CLI and the `fleet__*` MCP tools
    // both use (see `session::inherit_launch_context`). Under the Rule-3 worktree
    // workflow the Bash cwd sits in `<repo>/.worktrees/<task>` for most of a
    // session — and that worktree is removed when the plan merges, which would
    // delete the successor's own cwd; the resolved workspace reads the
    // transcript's authoritative cwd instead, falling back to the shell cwd.
    let ctx = claw_fleet_core::session::inherit_launch_context(Some(&sid));
    // An explicit `--model` / `--effort` flag wins over the inherited value. The
    // model override matters because the auto-resolved value is unreliable when
    // the last turn ran on a rate-limit fallback — exactly why the flag exists.
    let model = model
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .or(ctx.model);
    let effort = effort
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .or(ctx.effort);
    // Fleet stamps `FLEET_AGENT_SOURCE` on the sessions it launches (Codex sets
    // it to "codex"; Claude sessions have no stamp). Absent → "claude-code", the
    // historical default, so the successor is relayed on the same tool.
    let agent_source = ctx.source.unwrap_or_else(|| "claude-code".to_string());
    match claw_fleet_core::handoff::register(
        &sid,
        &ctx.workspace,
        Some(&ctx.shell_cwd),
        note,
        plan,
        next,
        model.as_deref(),
        effort.as_deref(),
        &agent_source,
    ) {
        Ok(rec) => println!(
            "ok: handoff registered (chain {}, 第 {} 棒, model={}, effort={}). \
             接力 session 将在本 session 结束 turn 后由 Stop hook 自动启动；请尽快结束当前 turn。",
            rec.chain_id,
            rec.hop,
            rec.model.as_deref().unwrap_or("<CLI 默认>"),
            rec.effort.as_deref().unwrap_or("<CLI 默认>"),
        ),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}
