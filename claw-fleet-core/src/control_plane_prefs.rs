//! Which control-plane features the user has *deliberately* switched off.
//!
//! Self-healing needs to tell two states apart that look identical on disk:
//! a hook that was never installed, and a hook the user turned off on purpose.
//! Both read as "absent" in `settings.json`. Reinstalling the first is the whole
//! point; reinstalling the second overrides a decision the user made.
//!
//! The distinction used to live only in the desktop frontend's store
//! (`claw-fleet-desktop/app/storage.ts`), which is why the frontend can already
//! self-heal on mount (`SettingsPanel.tsx`: toggle on + not installed → apply)
//! while a headless `fleet webui` cannot. This module records the same fact
//! where every process can see it: `~/.fleet/control-plane-prefs.json`,
//! alongside the permissions/MCP injector configs.
//!
//! **Absence is not disablement.** A feature is only listed here once something
//! actually removed it. A fresh host has an empty (or missing) file, so heal
//! installs everything — the tristate default is ON, and that has not changed.
//!
//! The recording lives in the bottom-level `remove_*` / `apply_*` functions in
//! [`crate::hooks`] and the guidance modules, so every caller — desktop Tauri
//! command, `fleet serve` HTTP route, CLI — is covered without repeating itself.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use crate::session::get_fleet_dir;

const PREFS_FILE_NAME: &str = "control-plane-prefs.json";

/// One healable control-plane feature.
///
/// Stored as the string form so the file stays readable and an unknown value
/// written by a newer Fleet degrades to "ignored" rather than corrupting the
/// whole file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Feature {
    GuardHook,
    ElicitationHook,
    PlanApprovalHook,
    IdleHooks,
    PrdContextHook,
    WakeupGuardHook,
    InteractionMode,
    PrdDiscipline,
    WikiGuidance,
    ModelGuidance,
}

impl Feature {
    /// The stable on-disk key. Never rename one of these without a migration —
    /// a renamed key reads as "not disabled" and silently reinstalls a feature
    /// the user turned off.
    pub fn key(self) -> &'static str {
        match self {
            Feature::GuardHook => "guard_hook",
            Feature::ElicitationHook => "elicitation_hook",
            Feature::PlanApprovalHook => "plan_approval_hook",
            Feature::IdleHooks => "idle_hooks",
            Feature::PrdContextHook => "prd_context_hook",
            Feature::WakeupGuardHook => "wakeup_guard_hook",
            Feature::InteractionMode => "interaction_mode",
            Feature::PrdDiscipline => "prd_discipline",
            Feature::WikiGuidance => "wiki_guidance",
            Feature::ModelGuidance => "model_guidance",
        }
    }

    /// Every feature heal knows how to install.
    pub const ALL: [Feature; 10] = [
        Feature::GuardHook,
        Feature::ElicitationHook,
        Feature::PlanApprovalHook,
        Feature::IdleHooks,
        Feature::PrdContextHook,
        Feature::WakeupGuardHook,
        Feature::InteractionMode,
        Feature::PrdDiscipline,
        Feature::WikiGuidance,
        Feature::ModelGuidance,
    ];
}

/// The persisted preferences: the set of features the user switched off.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPlanePrefs {
    /// Feature keys ([`Feature::key`]) the user deliberately disabled. A
    /// `BTreeSet` so the file has a stable order and repeated writes produce no
    /// spurious diffs.
    #[serde(default)]
    pub disabled: BTreeSet<String>,
}

fn prefs_path() -> Option<PathBuf> {
    get_fleet_dir().map(|d| d.join(PREFS_FILE_NAME))
}

/// Load the prefs. A missing or unparseable file means "nothing was disabled" —
/// the safe direction, since it heals a feature that may already be installed
/// (idempotent) rather than skipping one that is genuinely absent.
pub fn load() -> ControlPlanePrefs {
    let Some(p) = prefs_path() else {
        return ControlPlanePrefs::default();
    };
    fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(prefs: &ControlPlanePrefs) -> Result<(), String> {
    let p = prefs_path().ok_or("cannot determine fleet dir")?;
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(prefs).map_err(|e| e.to_string())?;
    fs::write(&p, json).map_err(|e| format!("write {}: {e}", p.display()))
}

/// Record that the user switched `feature` off, so heal leaves it alone.
///
/// Called from the `remove_*` functions. Errors are returned rather than
/// swallowed, but callers treat them as non-fatal: failing to record a
/// disablement makes heal too eager, not destructive.
pub fn mark_disabled(feature: Feature) -> Result<(), String> {
    let mut prefs = load();
    if !prefs.disabled.insert(feature.key().to_string()) {
        return Ok(()); // already recorded — don't rewrite the file
    }
    save(&prefs)
}

/// Clear the disabled flag for `feature` — the user turned it back on.
///
/// Called from the `apply_*` functions. Note the asymmetry with heal: heal
/// *skips* disabled features, so it never reaches an apply for one, and can
/// therefore never clear a flag by accident. An explicit `fleet bootstrap`
/// does apply everything, and clearing there is correct — typing that command
/// is a request for the full control plane.
pub fn mark_enabled(feature: Feature) -> Result<(), String> {
    let mut prefs = load();
    if !prefs.disabled.remove(feature.key()) {
        return Ok(()); // wasn't disabled — nothing to write
    }
    save(&prefs)
}

/// Whether the user deliberately switched `feature` off.
pub fn is_disabled(feature: Feature) -> bool {
    load().disabled.contains(feature.key())
}

/// Pair a successful install/uninstall with a note about what the user meant.
///
/// Wrapped around the body of every per-feature apply/remove rather than around
/// their call sites, because there are three of those (desktop Tauri command,
/// `fleet serve` HTTP route, CLI) and a fourth added later would silently skip
/// the bookkeeping.
///
/// A failed operation records nothing — nothing changed on disk. A failed
/// *record* is logged but not propagated: the change did happen, and returning
/// an error would tell the caller otherwise. The cost of a missed record is that
/// heal is too eager (it re-runs an idempotent install), never that it destroys
/// anything.
pub fn note_intent(
    outcome: Result<(), String>,
    feature: Feature,
    now_disabled: bool,
) -> Result<(), String> {
    outcome?;
    let recorded = if now_disabled {
        mark_disabled(feature)
    } else {
        mark_enabled(feature)
    };
    if let Err(e) = recorded {
        crate::log_debug(&format!(
            "control-plane prefs: could not record {} for {}: {e}",
            if now_disabled { "disable" } else { "enable" },
            feature.key()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Claims `FLEET_HOME` for the duration, so the prefs file lands in a temp
    /// dir instead of the developer's real `~/.fleet`.
    struct HomeGuard {
        dir: PathBuf,
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn new(tag: &str) -> Self {
            let lock = crate::paths::fleet_home_lock();
            let dir = std::env::temp_dir().join(format!(
                "fleet-cpprefs-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            let prev = std::env::var_os("FLEET_HOME");
            unsafe { std::env::set_var("FLEET_HOME", &dir) };
            Self { dir, prev, _lock: lock }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => unsafe { std::env::set_var("FLEET_HOME", v) },
                None => unsafe { std::env::remove_var("FLEET_HOME") },
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_fresh_host_has_nothing_disabled() {
        // The load-bearing default: on a machine that never disabled anything,
        // heal must install everything. Reading "disabled" here would leave a
        // fresh host permanently bare.
        let _h = HomeGuard::new("fresh");
        for f in Feature::ALL {
            assert!(!is_disabled(f), "{} must not read as disabled", f.key());
        }
    }

    #[test]
    fn disabling_survives_a_reload_and_only_touches_that_feature() {
        let _h = HomeGuard::new("mark");
        mark_disabled(Feature::GuardHook).unwrap();

        assert!(is_disabled(Feature::GuardHook));
        for f in Feature::ALL.iter().filter(|f| **f != Feature::GuardHook) {
            assert!(!is_disabled(*f), "{} must be untouched", f.key());
        }
    }

    #[test]
    fn re_enabling_clears_the_flag() {
        let _h = HomeGuard::new("roundtrip");
        mark_disabled(Feature::WikiGuidance).unwrap();
        assert!(is_disabled(Feature::WikiGuidance));

        mark_enabled(Feature::WikiGuidance).unwrap();
        assert!(!is_disabled(Feature::WikiGuidance));
    }

    #[test]
    fn marking_is_idempotent() {
        let _h = HomeGuard::new("idem");
        mark_disabled(Feature::IdleHooks).unwrap();
        mark_disabled(Feature::IdleHooks).unwrap();
        assert_eq!(load().disabled.len(), 1);

        mark_enabled(Feature::IdleHooks).unwrap();
        mark_enabled(Feature::IdleHooks).unwrap();
        assert!(load().disabled.is_empty());
    }

    #[test]
    fn an_unparseable_file_reads_as_nothing_disabled() {
        // Safe direction: heal re-applies an idempotent install, versus a
        // corrupt file silently suppressing the whole control plane.
        let h = HomeGuard::new("corrupt");
        let p = h.dir.join(".fleet");
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join(PREFS_FILE_NAME), "{ not json").unwrap();

        assert!(!is_disabled(Feature::GuardHook));
    }

    #[test]
    fn feature_keys_are_unique() {
        // Two features sharing a key would make disabling one disable both.
        let keys: BTreeSet<_> = Feature::ALL.iter().map(|f| f.key()).collect();
        assert_eq!(keys.len(), Feature::ALL.len());
    }
}
