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
    let shell_cwd = cwd.to_string_lossy().to_string();
    // Spawn the successor where the *session* runs, not where the agent's shell
    // happens to stand. Under the Rule-3 worktree workflow the Bash cwd sits in
    // `<repo>/.worktrees/<task>` for most of a session — and that worktree is
    // removed when the plan merges, which would delete the successor's own cwd
    // and hide it from the session scan. The transcript's `cwd` is authoritative;
    // fall back to the shell cwd only when there is no transcript to read.
    let ws = claw_fleet_core::session::resolve_session_cwd(&sid).unwrap_or_else(|| shell_cwd.clone());
    // The successor continues this session's work, so it must continue on this
    // session's model and effort rather than the CLI default. The model is only
    // discoverable from the transcript (Claude Code exports no model env var);
    // effort is handed to us directly.
    let model = claw_fleet_core::session::resolve_session_model_spec(&sid);
    let effort = std::env::var("CLAUDE_EFFORT")
        .ok()
        .filter(|e| !e.trim().is_empty());
    match claw_fleet_core::handoff::register(
        &sid,
        &ws,
        Some(&shell_cwd),
        note,
        plan,
        next,
        model.as_deref(),
        effort.as_deref(),
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
