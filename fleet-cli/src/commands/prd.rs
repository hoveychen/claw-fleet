//! PRD Discipline mode: `fleet prd-discipline apply` (regenerate guidance) and
//! the `prd-context` UserPromptSubmit hook that re-injects TASKS.md.

// ── `fleet prd-discipline apply` — regenerate guidance (CLI parity w/ GUI) ────

pub(crate) fn cmd_prd_discipline_apply(title: &str, locale: &str) {
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
pub(crate) fn cmd_prd_context() {
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
