//! The control plane's switches — which Fleet features its sessions carry —
//! and the two entry points that set them up.
//!
//! Nothing here writes into `~/.claude` any more. Every feature rides on each
//! Fleet launch ([`crate::claude_launch`]: `--settings` hooks, the system-prompt
//! guidance, `--mcp-config`), so a claude the user starts by hand behaves as if
//! Fleet were not installed. What is left to "install" is the switch in
//! [`crate::control_plane_prefs`] and the launch defaults:
//!
//! - [`install_all`] — every feature on, plus the default model and the
//!   guidance voice. What `fleet bootstrap` and the Fleet Cloud container's
//!   entrypoint run: typing that command is a request for the full control
//!   plane, so it overrides an earlier opt-out.
//! - [`heal`] — only the launch defaults that are missing. What `fleet webui`
//!   runs on startup. A feature nobody switched off is already on (absence in
//!   the prefs file means enabled), so heal has no switch to flip.
//!
//! `default_model` is deliberately *not* a [`Feature`]: it is a settings value,
//! not a mode you can switch on and off. [`install_all`] and [`heal`] both
//! record it as Fleet's own launch default, and it is a no-op when no model was
//! named.

use crate::control_plane_prefs::Feature;
use crate::hooks::HookSetupPlan;

/// One control-plane step: a stable label plus its outcome.
pub struct Step {
    pub name: &'static str,
    pub result: Result<(), String>,
}

/// The three inputs an install needs, already defaulted by the caller.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// Locale for generated guidance ("en", "zh", …).
    pub locale: String,
    /// What agents call the user. Empty renders the locale-correct "Boss".
    pub title: String,
    /// Default Claude Code model. Empty leaves the CLI's own default alone.
    pub model: String,
}

/// Whether `feature` is in the *global* `~/.claude` config, read off one
/// settings snapshot — which only an older Fleet build put there.
///
/// [`crate::claude_launch`] reads it so a launch does not carry a feature the
/// global config already delivers (a hook would fire twice), and the scope
/// migration reads it to decide what the user had switched on.
pub fn is_installed(feature: Feature, plan: &HookSetupPlan) -> bool {
    match feature {
        Feature::GuardHook => plan.guard_installed,
        Feature::ElicitationHook => plan.elicitation_installed,
        Feature::PlanApprovalHook => plan.plan_approval_installed,
        Feature::IdleHooks => plan.idle_hooks_installed,
        Feature::PrdContextHook => plan.prd_context_installed,
        Feature::WakeupGuardHook => plan.wakeup_guard_installed,
        Feature::InteractionMode => plan.interaction_mode_installed,
        Feature::PrdDiscipline => plan.prd_discipline_installed,
        Feature::WikiGuidance => plan.wiki_guidance_installed,
        Feature::ModelGuidance => plan.model_guidance_installed,
        Feature::SessionTitleGuidance => plan.session_title_guidance_installed,
    }
}

/// Switch every feature on and record the launch defaults.
pub fn install_all(s: &Settings) -> Vec<Step> {
    let mut steps: Vec<Step> = Feature::ALL
        .iter()
        .map(|&f| Step {
            name: f.key(),
            result: crate::control_plane_prefs::set_enabled(f, true),
        })
        .collect();
    steps.push(Step {
        name: "default_model",
        result: crate::claude_launch::set_default_model(&s.model),
    });
    steps.push(Step {
        name: "claude_launch_guidance",
        result: crate::claude_launch::reconcile_guidance(&s.title, &s.locale),
    });
    steps
}

/// Record the launch defaults that are missing. Returns a step per thing it
/// wrote — an empty vec means nothing was missing, which is the common case and
/// worth staying silent about.
pub fn heal(s: &Settings) -> Vec<Step> {
    let mut steps = Vec::new();
    // It is a no-op unless a model was named.
    if !s.model.is_empty() {
        steps.push(Step {
            name: "default_model",
            result: crate::claude_launch::set_default_model(&s.model),
        });
    }

    // Only when absent: this process's locale comes from `FLEET_LOCALE`, which
    // a hand-run `fleet webui` on a desktop host lacks, so re-rendering an
    // existing voice here would translate the desktop user's guidance.
    if !crate::claude_launch::guidance_rendered() {
        steps.push(Step {
            name: "claude_launch_guidance",
            result: crate::claude_launch::reconcile_guidance(&s.title, &s.locale),
        });
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plan with everything installed.
    fn all_installed() -> HookSetupPlan {
        HookSetupPlan {
            to_add: vec![],
            hooks_globally_disabled: false,
            already_installed: true,
            guard_installed: true,
            elicitation_installed: true,
            plan_approval_installed: true,
            interaction_mode_installed: true,
            prd_context_installed: true,
            notes_hint_installed: true,
            prd_discipline_installed: true,
            wiki_guidance_installed: true,
            model_guidance_installed: true,
            session_title_guidance_installed: true,
            idle_hooks_installed: true,
            wakeup_guard_installed: true,
        }
    }

    #[test]
    fn is_installed_covers_every_feature() {
        // A feature whose probe was never wired reads as "not installed"
        // forever, so a launch would carry it on top of the global copy and the
        // hook would fire twice.
        let plan = all_installed();
        for f in Feature::ALL {
            assert!(
                is_installed(f, &plan),
                "{} has no probe wired into is_installed",
                f.key()
            );
        }
    }

    #[test]
    fn nothing_reads_as_installed_on_a_bare_host() {
        let bare = HookSetupPlan {
            already_installed: false,
            ..Default::default()
        };
        for f in Feature::ALL {
            assert!(!is_installed(f, &bare), "{} must read as absent", f.key());
        }
    }

    #[test]
    fn neither_entry_point_touches_the_global_claude_dir() {
        // The whole point of the scope work: bootstrap and webui start used to
        // write hooks and guidance into ~/.claude, which every claude on the
        // machine then picked up.
        let fleet = crate::paths::fleet_home_guard_with(|| {
            let dir = std::env::temp_dir().join(format!(
                "fleet-cp-noglobal-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            dir
        });
        let s = Settings {
            locale: "zh".into(),
            title: String::new(),
            model: "opus".into(),
        };
        crate::control_plane_prefs::mark_disabled(Feature::GuardHook).unwrap();
        for step in install_all(&s).into_iter().chain(heal(&s)) {
            assert!(step.result.is_ok(), "{} failed: {:?}", step.name, step.result);
        }
        let claude = crate::session::get_claude_dir().expect("claude dir");
        assert!(!claude.exists(), "{} must not be created", claude.display());
        assert!(
            !crate::control_plane_prefs::is_disabled(Feature::GuardHook),
            "install_all switches everything on"
        );
        assert_eq!(
            crate::claude_launch::load_config().default_model.as_deref(),
            Some("opus")
        );
        assert!(heal(&s).iter().all(|st| st.name == "default_model"));
        let _ = std::fs::remove_dir_all(fleet.home());
    }
}
