//! `fleet bootstrap` — install a headless Fleet host's control plane.
//!
//! `fleet serve` alone is only an HTTP probe: it injects the permissions
//! allow-list and (since the debug-only gate was dropped) registers the fleet
//! MCP server, but it never installs the guard / elicitation / plan-approval
//! hooks or the `~/.claude/CLAUDE.md` guidance a desktop Fleet installs via its
//! onboarding UI. That combination is unsafe on its own: `serve` injects the
//! `Bash(*)` permissions allow rule (which suppresses Claude Code's native
//! command prompt) but installs no `fleet guard` hook to intercept commands, so
//! the agent would run shell commands with no audit gate.
//!
//! `fleet bootstrap` closes that gap. It idempotently installs the control-plane
//! hooks + guidance so a `fleet serve` container is a *controlled* Fleet host,
//! not just a probe. Intended for headless / Fleet Cloud hosts — the lean
//! container runs it before `fleet serve`. Every step is idempotent (hooks use
//! retain-then-push; guidance uses sentinel-strip-then-reinsert), and the
//! container's `~/.claude` is on the ephemeral layer, so it is meant to run on
//! every start. Desktop users manage these modes per-toggle via the app instead.
//!
//! MCP registration and the permissions allow-list are intentionally NOT done
//! here — `fleet serve` performs both on startup, and the entrypoint runs
//! `fleet bootstrap` before `fleet serve`, so both are live by the time serve
//! accepts requests.

use std::io::Write;

/// One control-plane step: a stable label plus its apply result.
pub(crate) struct Step {
    pub(crate) name: &'static str,
    pub(crate) result: Result<(), String>,
}

/// The three inputs every control-plane install needs, already defaulted.
///
/// Resolved once by [`resolve_settings`] so the `fleet bootstrap` subcommand and
/// the `fleet webui` first-run install agree on the fallbacks instead of each
/// re-deriving them from the environment.
pub(crate) struct BootstrapSettings {
    pub(crate) locale: String,
    pub(crate) title: String,
    pub(crate) model: String,
}

/// `locale` falls back to `$FLEET_LOCALE` then `"en"`; `title` defaults to empty
/// (which renders the locale-correct Boss/老板 — a literal value would force that
/// same string across all locales); `model` falls back to `$FLEET_CLAUDE_MODEL`
/// and, when both are absent, is left alone so the host keeps the Claude Code
/// CLI's own default.
pub(crate) fn resolve_settings(
    locale: Option<String>,
    title: Option<String>,
    model: Option<String>,
) -> BootstrapSettings {
    BootstrapSettings {
        locale: locale
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("FLEET_LOCALE").ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| "en".to_string()),
        title: title.unwrap_or_default(),
        model: model
            .or_else(|| std::env::var("FLEET_CLAUDE_MODEL").ok())
            .unwrap_or_default(),
    }
}

/// Install the headless control plane and report what each step did.
///
/// Every step is idempotent (hooks use retain-then-push; guidance uses
/// sentinel-strip-then-reinsert), so re-running this is safe. Split out of
/// [`cmd_bootstrap`] so `fleet webui` can perform the same install on a host
/// that never ran the subcommand — see `commands::serve::first_run_bootstrap`.
pub(crate) fn install_control_plane(s: &BootstrapSettings) -> Vec<Step> {
    vec![
        Step {
            name: "guard_hook",
            result: claw_fleet_core::hooks::apply_guard_hook(),
        },
        Step {
            name: "elicitation_hook",
            result: claw_fleet_core::hooks::apply_elicitation_hook(),
        },
        Step {
            name: "plan_approval_hook",
            result: claw_fleet_core::hooks::apply_plan_approval_hook(),
        },
        Step {
            // Wires Claude Code's Stop hook → `fleet session idle`, the trigger
            // that consumes registered handoffs (spawns the successor session)
            // and reconciles loop/watch. Without it a headless host silently
            // drops every Claude handoff/loop/watch — the desktop half of this
            // same install lives in gui/mod.rs setup().
            name: "idle_hooks",
            result: claw_fleet_core::hooks::apply_idle_hooks(),
        },
        Step {
            name: "prd_discipline",
            result: claw_fleet_core::prd_discipline::apply_prd_discipline(&s.title, &s.locale)
                .and_then(|()| claw_fleet_core::hooks::apply_prd_context_hook())
                .and_then(|()| claw_fleet_core::hooks::apply_wakeup_guard_hook()),
        },
        Step {
            name: "interaction_mode",
            result: claw_fleet_core::interaction_mode::apply_interaction_mode(&s.title, &s.locale),
        },
        Step {
            name: "wiki_guidance",
            result: claw_fleet_core::wiki_guidance::apply_wiki_guidance(&s.locale),
        },
        Step {
            name: "model_guidance",
            result: claw_fleet_core::model_guidance::apply_model_guidance(&s.locale),
        },
        Step {
            // No-op when `model` is empty — a host that named no model keeps
            // whatever the Claude Code CLI defaults to.
            name: "default_model",
            result: claw_fleet_core::hooks::apply_default_model(&s.model),
        },
    ]
}

/// `fleet bootstrap` — install the control plane and report it.
pub(crate) fn cmd_bootstrap(
    locale: Option<String>,
    title: Option<String>,
    model: Option<String>,
    json: bool,
) {
    let settings = resolve_settings(locale, title, model);
    let BootstrapSettings { locale, title, model } = &settings;
    let steps = install_control_plane(&settings);

    let failed = steps.iter().filter(|s| s.result.is_err()).count();

    if json {
        let arr: Vec<_> = steps
            .iter()
            .map(|s| {
                serde_json::json!({
                    "step": s.name,
                    "ok": s.result.is_ok(),
                    "error": s.result.as_ref().err(),
                })
            })
            .collect();
        let out = serde_json::json!({
            "ok": failed == 0,
            "locale": locale,
            "title": title,
            "model": model,
            "steps": arr,
        });
        println!("{out}");
    } else {
        for s in &steps {
            match &s.result {
                Ok(()) => println!("ok:   {}", s.name),
                Err(e) => eprintln!("FAIL: {} — {e}", s.name),
            }
        }
        if failed == 0 {
            println!(
                "fleet bootstrap: control plane installed (locale={locale:?}, title={title:?}, model={model:?})"
            );
        } else {
            eprintln!("fleet bootstrap: {failed} step(s) failed");
        }
    }

    let _ = std::io::stdout().flush();
    if failed > 0 {
        std::process::exit(1);
    }
}
