//! `fleet dsh-context` — the content side of Fleet's dsh plugin.
//!
//! dsh has no hook layer, so Fleet's per-turn context (today: the workspace's
//! active TASKS.md plans) reaches a dsh session through a cordis plugin that
//! listens on `agent/pre-step` and appends a `plugin`-sourced message to the
//! entering batch. That plugin owns *when* to inject; this command owns *what*
//! to inject, so the text keeps coming from one renderer instead of being
//! reimplemented in JavaScript.
//!
//! Contract with `dsh-plugin/index.js`: stdout is one JSON object
//! `{"sections":[{"name":"<id>","text":"<body>"}]}`. An empty `sections` array
//! means "nothing to inject this step" and the plugin appends no message. The
//! shape is a list because the plugin renders each section as its own labelled
//! block, and more sections are expected here later.
//!
//! Why not reuse `fleet prd-context`: that command speaks Claude Code's
//! UserPromptSubmit hook protocol — it reads a hook payload on stdin and wraps
//! the body in `hookSpecificOutput.additionalContext`. Both commands call the
//! same [`claw_fleet_core::prd_tasks::render_active_plans_reminder`], so the
//! injected text cannot drift between the two harnesses.

use std::path::PathBuf;

/// Emit the sections a dsh session should receive on this step.
///
/// `cwd` is the session's working directory (the plugin reads it off
/// `agent.session.header.cwd`), `session` its dsh session id — the same id
/// `render_active_plans_reminder` uses to mark which plan this session owns.
pub(crate) fn cmd_dsh_context(cwd: Option<PathBuf>, session: Option<String>) {
    let cwd = cwd
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut sections = Vec::new();

    // PRD / TASKS.md — the same renderer the Claude hook and the codex
    // prompt-prepend path use. `None` means no TASKS.md, or a clean file with
    // no active plan: inject nothing rather than an empty header.
    if let Some(reminder) =
        claw_fleet_core::prd_tasks::render_active_plans_reminder(&cwd, session.as_deref())
    {
        sections.push(serde_json::json!({ "name": "fleet-prd", "text": reminder }));
    }

    println!("{}", serde_json::json!({ "sections": sections }));
}
