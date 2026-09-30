//! On/off switches for the automatic drivers that have no config of their own.
//!
//! The rate-limit resume and the server-error retry live in
//! [`crate::auto_resume::AutoResumeConfig`], the orphan-plan reviver in
//! [`crate::plan_revive::PlanReviveConfig`]; this file holds the rest, so the
//! settings panel can offer every automatic continuation as a checkbox. All
//! default on — the behaviour before the switches existed.
//!
//! Stored in `~/.fleet/resume-triggers.json`. Read on every use (no cache), so a
//! flip in the settings panel reaches the Stop hook and the tick immediately.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase", default)]
pub struct ResumeTriggersConfig {
    /// Pressing the finish button starts a session for the next unfinished plan
    /// in the tree ([`crate::plan_revive::continue_after_finish`]).
    pub finish_continue: bool,
    /// Interrupt a Codex turn that has gone silent and nudge it on
    /// ([`crate::headless_runtime::maybe_interrupt_stalled_codex`]).
    pub codex_stall_watchdog: bool,
    /// The Stop hook refuses to end a turn while the focused plan still has
    /// unchecked tasks ([`crate::plan_gate::gate_reason`]).
    pub plan_gate: bool,
    /// A registered handoff spawns its successor when the turn ends
    /// ([`crate::handoff::consume_and_spawn`]).
    pub handoff_successor: bool,
}

impl Default for ResumeTriggersConfig {
    fn default() -> Self {
        Self {
            finish_continue: true,
            codex_stall_watchdog: true,
            plan_gate: true,
            handoff_successor: true,
        }
    }
}

fn config_path() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("resume-triggers.json"))
}

impl ResumeTriggersConfig {
    pub fn load() -> Self {
        config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path().ok_or("cannot determine fleet dir")?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        crate::atomic_json::write_atomic(&path, &bytes).map_err(|e| e.to_string())
    }
}

/// Every automatic-continuation switch in one value, for the phone's settings
/// page (one relay round trip instead of three). Each part is saved to its own
/// file; see [`save_parts`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResumeSettings {
    pub auto_resume: crate::auto_resume::AutoResumeConfig,
    pub plan_revive: crate::plan_revive::PlanReviveConfig,
    pub triggers: ResumeTriggersConfig,
}

pub fn load_all() -> ResumeSettings {
    ResumeSettings {
        auto_resume: crate::auto_resume::AutoResumeConfig::load(),
        plan_revive: crate::plan_revive::PlanReviveConfig::load(),
        triggers: ResumeTriggersConfig::load(),
    }
}

/// Save whichever parts `patch` carries (`autoResume`, `planRevive`,
/// `triggers`, each a whole config object) and answer with everything as
/// stored. A part that does not parse fails the call before anything is
/// written, so a half-applied patch cannot happen.
pub fn save_parts(patch: &serde_json::Value) -> Result<ResumeSettings, String> {
    fn part<T: serde::de::DeserializeOwned>(patch: &serde_json::Value, key: &str) -> Result<Option<T>, String> {
        match patch.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(v) => serde_json::from_value(v.clone()).map(Some).map_err(|e| format!("{key}: {e}")),
        }
    }
    let auto_resume: Option<crate::auto_resume::AutoResumeConfig> = part(patch, "autoResume")?;
    let plan_revive: Option<crate::plan_revive::PlanReviveConfig> = part(patch, "planRevive")?;
    let triggers: Option<ResumeTriggersConfig> = part(patch, "triggers")?;
    if let Some(c) = auto_resume {
        c.save()?;
    }
    if let Some(c) = plan_revive {
        c.save()?;
    }
    if let Some(c) = triggers {
        c.save()?;
    }
    Ok(load_all())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_default_on() {
        let cfg: ResumeTriggersConfig = serde_json::from_str(r#"{"planGate":false}"#).unwrap();
        assert!(!cfg.plan_gate);
        assert!(cfg.finish_continue && cfg.codex_stall_watchdog && cfg.handoff_successor);
    }

    fn temp_home() -> crate::paths::FleetHomeGuard {
        crate::paths::fleet_home_guard_with(|| {
            let d = std::env::temp_dir().join(format!("fleet-resume-triggers-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&d).unwrap();
            d
        })
    }

    fn register(sid: &str) -> Result<crate::handoff::PendingHandoff, String> {
        crate::handoff::register(sid, "/p", None, "note", None, None, None, None, None, None, "claude-code")
    }

    #[test]
    fn handoff_switch_off_refuses_registration_and_drops_an_earlier_one() {
        let _home = temp_home();
        let sid = uuid::Uuid::new_v4().to_string();
        register(&sid).expect("default on: registration succeeds");
        ResumeTriggersConfig { handoff_successor: false, ..Default::default() }.save().unwrap();

        let err = register(&uuid::Uuid::new_v4().to_string()).unwrap_err();
        assert_eq!(err, crate::handoff::HANDOFF_DISABLED_ERR);
        assert_eq!(crate::handoff::consume_and_spawn(&sid), Ok(None), "no successor spawned");
        assert!(crate::handoff::read_pending(&sid).is_none(), "the earlier registration is dropped");
    }

    #[test]
    fn save_parts_writes_only_what_it_carries_and_rejects_a_bad_part_whole() {
        let _home = temp_home();
        let got = save_parts(&serde_json::json!({"triggers": {"planGate": false}})).unwrap();
        assert!(!got.triggers.plan_gate);
        assert!(got.auto_resume.enabled && got.plan_revive.enabled, "untouched parts keep their values");

        let bad = serde_json::json!({"triggers": {"planGate": true}, "planRevive": {"enabled": "yes"}});
        assert!(save_parts(&bad).is_err());
        assert!(!load_all().triggers.plan_gate, "nothing written when one part is malformed");
    }
}
