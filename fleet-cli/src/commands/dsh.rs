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
//! session once while the dynamic plan section re-enters when it has changed —
//! and, like the Claude hook, only on a step that opens a turn with a user
//! prompt, never mid-turn. An empty array means "nothing to inject this step".
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

/// How long the recent-sessions block may take before this command gives up on
/// it. Two thirds of the plugin's 5-second ceiling, leaving room for the
/// guidance sections that share the reply.
const RECENT_SESSIONS_BUDGET: std::time::Duration = std::time::Duration::from_millis(3000);

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
///
/// `ctx_used` / `ctx_window` / `ctx_model` are this session's context occupancy,
/// measured by the plugin off the in-memory dsh session log — see
/// `readContextPressure` there for why the measurement lives on that side and
/// the tier policy on this one.
pub(crate) fn cmd_dsh_context(
    cwd: Option<PathBuf>,
    session: Option<String>,
    title: &str,
    locale: &str,
    ctx_used: Option<u64>,
    ctx_window: Option<u64>,
    ctx_model: Option<String>,
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

    // The session's own id. Not part of `render_dsh_sections` on purpose: those
    // bodies are also written to the machine-wide AGENTS.md, and this one is
    // per-session. It sits next to the plan reminder because that is the section
    // whose commands need the `--session` flag it explains.
    if let Some(body) = session
        .as_deref()
        .and_then(claw_fleet_core::dsh_guidance::render_dsh_session_id_block)
    {
        sections.push(serde_json::json!({
            "name": claw_fleet_core::dsh_guidance::SECTION_SESSION_ID,
            "text": body,
        }));
    }

    // PRD / TASKS.md — the same renderer the Claude hook and the codex
    // prompt-prepend path use. `None` means no TASKS.md, or a clean file with
    // no active plan: inject nothing rather than an empty header.
    if let Some(reminder) =
        claw_fleet_core::prd_tasks::render_active_plans_reminder(&cwd, session.as_deref())
    {
        sections.push(serde_json::json!({ "name": "fleet-prd", "text": reminder }));
    }

    // What this repository has been worked on lately — the dsh arm of the
    // `fleet recent-sessions` SessionStart hook, same renderer so all three
    // harnesses inject identical text.
    //
    // Claimed once per session on this side rather than left to the plugin's
    // dedup: that compares section text, and this block's timestamps and
    // `[running]` marker change between steps, so it would look fresh all day.
    // Claiming first also skips the session scan, which is the costly half of
    // rendering it.
    //
    // Rendering is also kept on a short leash. The plugin allows this whole
    // command 5 seconds, and the scan behind the block takes ~3.7s warm and far
    // longer cold — overrunning would drop every guidance section in this
    // reply, not just this one. A render that misses the budget hands its claim
    // back so the next step can try again.
    if let Some(sid) = session.as_deref() {
        if claw_fleet_core::recent_sessions::claim_once(sid) {
            match claw_fleet_core::recent_sessions::render_within(
                &cwd.to_string_lossy(),
                Some(sid),
                RECENT_SESSIONS_BUDGET,
            ) {
                Some(block) => sections
                    .push(serde_json::json!({ "name": "fleet-recent-sessions", "text": block })),
                None => claw_fleet_core::recent_sessions::forget(sid),
            }
        }
    }

    // Context pressure — the dsh arm of the reminder Claude gets from the
    // `fleet ctx-reminder` PostToolUse hook.
    //
    // Deliberately NOT turn-scoped on the plugin side: unlike the plan block,
    // which would otherwise re-enter mid-turn every time anyone ticked a box in
    // the workspace, this is emitted at most once per tier per session — and a
    // session that crosses 750K mid-turn is exactly the one that should not
    // wait for its next prompt to hear about it.
    //
    // `claim_tier` is consuming: the tier is recorded as announced as soon as
    // the section is emitted here. A plugin that then drops it (an aborted
    // step) loses that one announcement rather than repeating it forever; the
    // tiers above it still fire, and a compaction re-arms all of them.
    if let (Some(used), Some(window), Some(session)) = (ctx_used, ctx_window, session.as_deref()) {
        let pressure = claw_fleet_core::context_pressure::ContextPressure {
            used,
            window,
            model: ctx_model.unwrap_or_default(),
        };
        if let Some(tier) = claw_fleet_core::context_pressure::claim_tier(session, &pressure) {
            sections.push(serde_json::json!({
                "name": "fleet-ctx",
                "text": claw_fleet_core::context_pressure::reminder_text(&pressure, tier),
            }));
        }
    }

    // The sandbox mode this session should switch to, if any. Sent alongside the
    // sections rather than inside one: it is an instruction for the plugin, not
    // text for the model. Absent for a session Fleet did not spawn — see
    // `dsh_guidance::sandbox_mode_for_session` for why ownership is the gate.
    let mut payload = serde_json::json!({ "sections": sections });
    if let Some(mode) = session
        .as_deref()
        .and_then(claw_fleet_core::dsh_guidance::sandbox_mode_for_session)
    {
        payload["sandboxMode"] = serde_json::Value::String(mode.to_string());
    }

    // A side-question fork (`session_explain`) is a one-step turn by contract:
    // the prompt forbids tools, and this flag is the hard stop behind it — the
    // plugin rejects any step after the first, so a model that reaches for a
    // tool anyway cannot run it. Sent as a sibling of `sandboxMode` rather than
    // as a new required flag on purpose: an older `fleet` build never rejects
    // the invocation, it just omits the field, and the plugin treats absence
    // as "not a one-shot session". The marker is written by `dsh_fork_ask`
    // before the child's prompt goes in, so the first step already sees it.
    if session
        .as_deref()
        .is_some_and(claw_fleet_core::session_explain::is_fork_session)
    {
        payload["oneShot"] = serde_json::Value::Bool(true);
    }

    println!("{payload}");
}
