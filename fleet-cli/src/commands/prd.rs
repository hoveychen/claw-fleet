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

// ── Context-pressure CLI (hook entrypoint for PostToolUse) ─────────────────

/// Announce context-window occupancy at 25% / 50% / 75%, and name `fleet
/// handoff` at the last one.
///
/// **Why PostToolUse and not UserPromptSubmit.** The sessions that most need
/// this are the ones that never come back for another prompt: a headless `-p`
/// turn can run for hours, fill the window, get silently auto-compacted and
/// keep going, and a prompt-time hook fires exactly zero times in all of that.
/// A tool call is the only event that recurs inside such a turn.
///
/// Silent unless a tier is newly crossed ([`claw_fleet_core::context_pressure::claim_tier`]),
/// so the per-tool-call cost is a tail read and nothing in context.
pub(crate) fn cmd_ctx_reminder() {
    use std::io::Read;
    use std::path::PathBuf;

    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&input) else {
        return;
    };

    let Some(transcript) = parsed
        .get("transcript_path")
        .and_then(|t| t.as_str())
        .map(PathBuf::from)
    else {
        return;
    };
    let Some(session_id) = parsed.get("session_id").and_then(|s| s.as_str()) else {
        return;
    };
    // A subagent's tool calls carry the parent's session id, but its context is
    // a different window; announcing the parent's tier inside it would be both
    // wrong and unactionable (a subagent cannot hand off).
    if parsed
        .get("isSidechain")
        .and_then(|b| b.as_bool())
        .unwrap_or(false)
    {
        return;
    }

    let Some(pressure) = claw_fleet_core::context_pressure::read_pressure(&transcript) else {
        return;
    };
    let Some(tier) = claw_fleet_core::context_pressure::claim_tier(session_id, &pressure) else {
        return;
    };

    let out = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": claw_fleet_core::context_pressure::reminder_text(&pressure, tier),
        }
    });
    println!("{out}");
}
