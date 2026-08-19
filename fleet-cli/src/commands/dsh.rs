//! `fleet dsh-context` — the content side of Fleet's dsh plugin.
//!
//! dsh has no hook layer, so Fleet's per-turn context reaches a dsh session
//! through a cordis plugin that listens on `agent/pre-step` and appends
//! `plugin`-sourced messages to the entering batch. That plugin owns *when* to
//! inject; this command owns *what* to inject, so every body keeps coming from
//! one renderer instead of being reimplemented in JavaScript.
//!
//! Contract with `dsh-plugin/index.js`: stdout is one JSON object
//! `{"sections":[{"name":"<id>","text":"<body>"}]}`. The plugin de-dups per
//! section name against the session log, so a static guidance section enters a
//! session once while the dynamic plan section re-enters whenever it changes. An
//! empty array means "nothing to inject this step".
//!
//! Two kinds of section come out of here:
//!
//! * **Guidance** (`fleet-guidance-*`) — the concept blocks that used to live
//!   only in `$DSH_HOME/AGENTS.md`. Delivered through the plugin because dsh's
//!   instruction loader drops the *user-global* file first under budget
//!   pressure, so in a repo with large project instructions Fleet's rules were
//!   the first thing to silently disappear.
//! * **Plans** (`fleet-prd`) — the workspace's active TASKS.md region, which
//!   changes as boxes get ticked.
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
///
/// `title` and `locale` come from the plugin's own config, which Fleet froze
/// into `cordis.patch.yml` at install time. They are *not* defaulted here on
/// purpose: falling back to `Boss` / `en` would render English guidance
/// addressing the user as "Boss" for a user whose Fleet says otherwise, so an
/// absent value keeps the CLI's declared defaults visible in one place instead.
pub(crate) fn cmd_dsh_context(
    cwd: Option<PathBuf>,
    session: Option<String>,
    title: &str,
    locale: &str,
) {
    let cwd = cwd
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    let mut sections = Vec::new();

    // Guidance blocks. `dsh_guidance_set` reads the same concept toggles the
    // AGENTS.md writer reads, so the two delivery channels cannot disagree about
    // what is switched on. `dsh_present = true`: this command only ever runs
    // from inside a live dsh session.
    let set = claw_fleet_core::dsh_guidance::dsh_guidance_set(true);
    for (name, body) in claw_fleet_core::dsh_guidance::render_dsh_sections(set, title, locale) {
        sections.push(serde_json::json!({ "name": name, "text": body }));
    }

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
