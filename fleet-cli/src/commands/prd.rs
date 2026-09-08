//! PRD Discipline mode: `fleet prd-discipline apply` (regenerate guidance) and
//! the `prd-context` UserPromptSubmit hook that re-injects TASKS.md.

// ── `fleet prd-discipline apply` — regenerate guidance (CLI parity w/ GUI) ────

pub(crate) fn cmd_prd_discipline_apply(title: &str, locale: &str) {
    let result = claw_fleet_core::prd_discipline::apply_prd_discipline(title, locale)
        .and_then(|()| claw_fleet_core::hooks::apply_prd_context_hook())
        .and_then(|()| claw_fleet_core::hooks::apply_wakeup_guard_hook());
    match result {
        Ok(()) => println!("ok: regenerated PRD guidance (title={title:?}, locale={locale:?})"),
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

// ── PRD-context CLI (hook entrypoint for UserPromptSubmit) ─────────────────

/// Re-inject the workspace's `TASKS.md` (active plan region) into the user
/// prompt as additional context. Companion to PRD Discipline mode — survives
/// context compression, since the file lives on disk.
///
/// Silent when the previous prompt already injected byte-identical text that
/// no compaction has swallowed since ([`claw_fleet_core::prd_context_dedup`]):
/// an unchanged TASKS.md used to cost a fresh 5–12 KB copy every single turn.
///
/// Multi-source: discovers the repo's main checkout root and scans both
/// `<main>/TASKS.md` and every `<main>/.worktrees/*/TASKS.md`, so a worker
/// inside a worktree still sees plans living in the main checkout (and vice
/// versa). Plans are deduped by `id`; on conflict the source whose file was
/// modified most recently wins. Legacy anonymous (no `id`) blocks are kept
/// independently — they pre-date the multi-plan format.
pub(crate) fn cmd_prd_context() {
    use std::io::Read;
    use std::path::PathBuf;

    // Read stdin payload — Claude Code sends `{ session_id, cwd, prompt, ... }`.
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);

    // Prefer the `cwd` field from stdin (authoritative for this hook firing);
    // fall back to process cwd if parsing fails.
    let parsed = serde_json::from_str::<serde_json::Value>(&input).ok();
    let cwd_from_stdin = parsed
        .as_ref()
        .and_then(|v| v.get("cwd").and_then(|c| c.as_str()).map(PathBuf::from));
    let session_id = parsed
        .as_ref()
        .and_then(|v| v.get("session_id").and_then(|s| s.as_str()))
        .map(|s| s.to_string());
    let cwd = cwd_from_stdin
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    // Single source of truth for the injected text (shared with the codex
    // prompt-prepend path). `None` → silent no-op (no TASKS.md, or a clean file
    // with no active plan and no structural problem).
    let Some(reminder) =
        claw_fleet_core::prd_tasks::render_active_plans_reminder(&cwd, session_id.as_deref())
    else {
        return;
    };

    // Nothing to say when the copy from the previous prompt is byte-identical
    // and still in front of the model — see `prd_context_dedup`. No
    // `transcript_path` (older Claude Code, or a hand-fed payload) means no
    // evidence, so the reminder goes in as it always did.
    let transcript = parsed
        .as_ref()
        .and_then(|v| v.get("transcript_path").and_then(|t| t.as_str()))
        .map(PathBuf::from);
    if let Some(transcript) = transcript {
        if !claw_fleet_core::prd_context_dedup::claude_needs_injection(&transcript, &reminder) {
            return;
        }
    }

    let out = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": reminder,
        }
    });
    println!("{out}");
}
