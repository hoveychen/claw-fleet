//! Installing the control plane — the hooks and `~/.claude/CLAUDE.md` guidance
//! that turn a bare Claude Code host into a Fleet-governed one.
//!
//! Two callers, one dispatch table:
//!
//! - [`install_all`] — everything, unconditionally. What `fleet bootstrap` and
//!   the Fleet Cloud container's entrypoint run: typing that command is a
//!   request for the full control plane.
//! - [`heal`] — only what is *missing* and was never deliberately switched off.
//!   What `fleet webui` runs on startup, so a host nobody ever configured
//!   through the UI still gets a guard hook, and one whose `~/.claude` was
//!   wiped gets it back.
//!
//! Keeping both on one [`Feature`] table is the point: a feature added to the
//! control plane in the future is installed by `bootstrap` and healed by
//! `webui` from the same edit, instead of being added to one list and forgotten
//! in the other.
//!
//! `default_model` is deliberately *not* a [`Feature`]: it is a settings value,
//! not a mode you can switch on and off, so there is no "is it installed" to
//! probe and nothing for heal to decide. [`install_all`] and [`heal`] both apply
//! it, and it is a no-op when no model was named.

use crate::control_plane_prefs::{is_disabled, Feature};
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
    /// What agents call the user. Empty renders the locale-correct Boss/老板.
    pub title: String,
    /// Default Claude Code model. Empty leaves the CLI's own default alone.
    pub model: String,
}

/// Install one feature. The single place that knows which function installs
/// what — [`install_all`] and [`heal`] both go through here.
fn apply(feature: Feature, s: &Settings) -> Result<(), String> {
    match feature {
        Feature::GuardHook => crate::hooks::apply_guard_hook(),
        Feature::ElicitationHook => crate::hooks::apply_elicitation_hook(),
        Feature::PlanApprovalHook => crate::hooks::apply_plan_approval_hook(),
        Feature::IdleHooks => crate::hooks::apply_idle_hooks(),
        Feature::PrdContextHook => crate::hooks::apply_prd_context_hook(),
        Feature::WakeupGuardHook => crate::hooks::apply_wakeup_guard_hook(),
        Feature::InteractionMode => {
            crate::interaction_mode::apply_interaction_mode(&s.title, &s.locale)
        }
        Feature::PrdDiscipline => crate::prd_discipline::apply_prd_discipline(&s.title, &s.locale),
        Feature::WikiGuidance => crate::wiki_guidance::apply_wiki_guidance(&s.locale),
        Feature::ModelGuidance => crate::model_guidance::apply_model_guidance(&s.locale),
    }
}

/// Whether `feature` is currently installed, read off one settings snapshot.
///
/// Takes an already-computed [`HookSetupPlan`] rather than probing per feature,
/// so heal reads `settings.json` once instead of ten times.
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
    }
}

/// Install every feature, whatever its current state.
///
/// Idempotent — hooks retain-then-push, guidance strips its sentinel block and
/// reinserts — so this is safe to run on every container start. Note it also
/// *clears* any recorded disablement, via the bookkeeping inside each apply:
/// asking for the full control plane by name overrides an earlier opt-out.
pub fn install_all(s: &Settings) -> Vec<Step> {
    let mut steps: Vec<Step> = Feature::ALL
        .iter()
        .map(|&f| Step {
            name: f.key(),
            result: apply(f, s),
        })
        .collect();
    steps.push(Step {
        name: "default_model",
        result: crate::hooks::apply_default_model(&s.model),
    });
    steps
}

/// Install only what is missing and was not deliberately switched off.
///
/// Returns a step per feature it actually installed — an empty vec means the
/// control plane was already whole, which is the common case and worth staying
/// silent about.
///
/// The two skip reasons are not interchangeable:
/// - *already installed* — nothing to do.
/// - *deliberately disabled* — the user turned it off (recorded by
///   [`crate::control_plane_prefs`] when something called the remove path).
///   Installing it here would override that choice on every restart.
pub fn heal(s: &Settings) -> Vec<Step> {
    let plan = crate::hooks::plan_hook_setup();
    let mut steps: Vec<Step> = Feature::ALL
        .iter()
        .filter(|&&f| !is_installed(f, &plan) && !is_disabled(f))
        .map(|&f| Step {
            name: f.key(),
            result: apply(f, s),
        })
        .collect();

    // Applied outside the missing/disabled filter because it has no installed
    // state to compare against — and it is a no-op unless a model was named.
    if !s.model.is_empty() {
        steps.push(Step {
            name: "default_model",
            result: crate::hooks::apply_default_model(&s.model),
        });
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plan with everything installed — the baseline heal should no-op on.
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
            prd_discipline_installed: true,
            wiki_guidance_installed: true,
            model_guidance_installed: true,
            idle_hooks_installed: true,
            wakeup_guard_installed: true,
        }
    }

    #[test]
    fn is_installed_covers_every_feature() {
        // A feature whose probe was never wired reads as "not installed"
        // forever, so heal would reinstall it on every single start. Catch that
        // here rather than in a puzzling log full of repeated installs.
        let plan = all_installed();
        for f in Feature::ALL {
            assert!(
                is_installed(f, &plan),
                "{} has no probe wired into is_installed",
                f.key()
            );
        }
    }

    /// Claims `FLEET_HOME` so an install lands in a temp dir, never the
    /// developer's real `~/.claude` / `~/.fleet`.
    struct HomeGuard {
        dir: std::path::PathBuf,
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn new(tag: &str) -> Self {
            let lock = crate::paths::fleet_home_lock();
            let dir = std::env::temp_dir().join(format!(
                "fleet-cpheal-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let prev = std::env::var_os("FLEET_HOME");
            // SAFETY: serialised by fleet_home_lock.
            unsafe { std::env::set_var("FLEET_HOME", &dir) };
            Self { dir, prev, _lock: lock }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("FLEET_HOME", v),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// The apply paths need an installed `fleet` binary to write hook commands.
    /// Without one, treat the test as skipped rather than failed — same
    /// convention as the hooks tests.
    fn skip_without_fleet_binary(steps: &[Step]) -> bool {
        steps.iter().any(|s| {
            s.result
                .as_ref()
                .err()
                .is_some_and(|e| e.contains("Cannot find fleet binary"))
        })
    }

    #[test]
    fn heal_installs_everything_on_a_bare_host_then_goes_quiet() {
        let _h = HomeGuard::new("bare");
        let s = Settings {
            locale: "en".into(),
            title: String::new(),
            model: String::new(),
        };

        let first = heal(&s);
        if skip_without_fleet_binary(&first) {
            eprintln!("skipped: no fleet binary on this host");
            return;
        }
        assert_eq!(
            first.len(),
            Feature::ALL.len(),
            "a bare host must get the whole control plane"
        );
        for step in &first {
            assert!(step.result.is_ok(), "{} failed: {:?}", step.name, step.result);
        }

        // Second run: everything is installed, so heal must do nothing. A
        // non-empty result here means some probe never sees its own install,
        // which would reinstall that feature on every single start.
        let second = heal(&s);
        let names: Vec<&str> = second.iter().map(|s| s.name).collect();
        assert!(second.is_empty(), "heal must be quiet once whole, got {names:?}");
    }

    #[test]
    fn heal_leaves_a_deliberately_disabled_feature_alone() {
        // The contract the whole prefs file exists for: switching the guard off
        // must survive a restart. Before prefs, heal could not tell this state
        // from "never installed" and would reinstate it on every boot.
        let _h = HomeGuard::new("disabled");
        let s = Settings {
            locale: "en".into(),
            title: String::new(),
            model: String::new(),
        };

        let first = heal(&s);
        if skip_without_fleet_binary(&first) {
            eprintln!("skipped: no fleet binary on this host");
            return;
        }

        // The user switches the guard off — remove_guard_hook records that.
        crate::hooks::remove_guard_hook().expect("remove guard");
        assert!(is_disabled(Feature::GuardHook), "removal must be recorded");

        let after = heal(&s);
        assert!(
            !after.iter().any(|st| st.name == Feature::GuardHook.key()),
            "heal must not reinstate a feature the user turned off"
        );
        assert!(
            !crate::hooks::plan_hook_setup().guard_installed,
            "and it must still be absent on disk"
        );
    }

    #[test]
    fn heal_reinstalls_a_feature_that_vanished_without_being_disabled() {
        // The case a first-run marker could never fix: ~/.claude was reset (a
        // wiped container layer, a settings.json rewrite), so the control plane
        // is gone even though the host was configured once.
        let _h = HomeGuard::new("vanished");
        let s = Settings {
            locale: "en".into(),
            title: String::new(),
            model: String::new(),
        };

        let first = heal(&s);
        if skip_without_fleet_binary(&first) {
            eprintln!("skipped: no fleet binary on this host");
            return;
        }

        // Wipe ~/.claude the way a container layer reset would — no remove_*
        // call, so nothing is recorded as disabled.
        let claude = crate::session::get_claude_dir().expect("claude dir");
        std::fs::remove_dir_all(&claude).expect("wipe ~/.claude");
        assert!(!is_disabled(Feature::GuardHook), "a wipe is not a user opt-out");

        let after = heal(&s);
        assert!(
            after.iter().any(|st| st.name == Feature::GuardHook.key()),
            "heal must restore what a reset removed"
        );
        assert!(crate::hooks::plan_hook_setup().guard_installed);
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
}
