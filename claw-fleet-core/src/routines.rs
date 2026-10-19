//! Routines (定时): the user-facing name for Fleet's future tasks.
//!
//! Underneath a routine is either a one-shot [`crate::schedule`] or a recurring
//! [`crate::agent_loop`]; both stores and their timers stay as they are. This
//! module is the one creation path the UI clients share — the desktop project
//! page, `fleet serve`'s `/routines` route — so "create" means the same thing
//! everywhere: register the record, then arm its timer. A timer that fails to
//! arm is not an error: the desktop app or `fleet serve` re-arms every
//! registered routine within 30 seconds (see their reconcile loops).

use serde::{Deserialize, Serialize};

use crate::agent_loop::LoopRecord;
use crate::schedule::ScheduleRecord;

/// What the user filled in. Exactly one of `fire_at` / `interval` is set:
/// `fire_at` makes a one-shot schedule, `interval` (`30m`, `6h`, `1d`, …, the
/// same grammar as `fleet loop --interval`) a recurring loop.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RoutineSpec {
    pub workspace_path: String,
    pub prompt: String,
    #[serde(default)]
    pub title: Option<String>,
    /// Epoch ms.
    #[serde(default)]
    pub fire_at: Option<u64>,
    #[serde(default)]
    pub interval: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub agent_source: Option<String>,
}

/// The record the routine was stored as.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", content = "record", rename_all = "camelCase")]
pub enum CreatedRoutine {
    Schedule(ScheduleRecord),
    Loop(LoopRecord),
}

fn non_blank(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// Which store a spec goes to, after validation that needs no I/O.
#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Once(u64),
    Every(u64),
}

fn kind_of(spec: &RoutineSpec) -> Result<Kind, String> {
    if spec.workspace_path.trim().is_empty() {
        return Err("a routine needs a project".to_string());
    }
    match (spec.fire_at, non_blank(&spec.interval)) {
        (Some(_), Some(_)) => Err("set either a time or an interval, not both".to_string()),
        (None, None) => Err("a routine needs a time or an interval".to_string()),
        (Some(at), None) => Ok(Kind::Once(at)),
        (None, Some(i)) => crate::agent_loop::parse_interval(i)
            .map(Kind::Every)
            .map_err(|e| e.to_string()),
    }
}

/// Register a routine and arm its timer.
pub fn create(spec: &RoutineSpec) -> Result<CreatedRoutine, String> {
    let workspace = spec.workspace_path.trim();
    match kind_of(spec)? {
        Kind::Once(fire_at) => {
            let rec = crate::schedule::create(
                workspace,
                &spec.prompt,
                non_blank(&spec.title),
                fire_at,
                non_blank(&spec.model),
                non_blank(&spec.effort),
                non_blank(&spec.agent_source),
                None,
                None,
            )?;
            if let Err(e) = crate::schedule::arm_timer(&rec) {
                eprintln!("[routines] routine {}: schedule timer not armed yet: {e}", rec.id);
            }
            Ok(CreatedRoutine::Schedule(rec))
        }
        Kind::Every(interval_secs) => {
            let rec = crate::agent_loop::create(
                workspace,
                &spec.prompt,
                non_blank(&spec.title),
                interval_secs,
                None,
                non_blank(&spec.model),
                non_blank(&spec.effort),
                non_blank(&spec.agent_source),
                None,
                None,
            )?;
            if let Err(e) = crate::agent_loop::arm_timer(&rec) {
                eprintln!("[routines] routine {}: loop timer not armed yet: {e}", rec.id);
            }
            Ok(CreatedRoutine::Loop(rec))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> RoutineSpec {
        RoutineSpec {
            workspace_path: "/work/proj".into(),
            prompt: "check CI".into(),
            ..Default::default()
        }
    }

    #[test]
    fn time_makes_a_schedule_and_interval_a_loop() {
        let once = RoutineSpec { fire_at: Some(42), ..spec() };
        assert_eq!(kind_of(&once).unwrap(), Kind::Once(42));
        let every = RoutineSpec { interval: Some("6h".into()), ..spec() };
        assert_eq!(kind_of(&every).unwrap(), Kind::Every(6 * 3600));
    }

    #[test]
    fn needs_exactly_one_of_time_and_interval() {
        assert!(kind_of(&spec()).is_err());
        let blank_interval = RoutineSpec { interval: Some("  ".into()), ..spec() };
        assert!(kind_of(&blank_interval).is_err());
        let both = RoutineSpec { fire_at: Some(1), interval: Some("1d".into()), ..spec() };
        assert!(kind_of(&both).is_err());
    }

    #[test]
    fn needs_a_project_and_a_valid_interval() {
        let no_project = RoutineSpec { workspace_path: " ".into(), fire_at: Some(1), ..spec() };
        assert!(kind_of(&no_project).is_err());
        let bad = RoutineSpec { interval: Some("soon".into()), ..spec() };
        assert!(kind_of(&bad).is_err());
    }

    #[test]
    fn spec_round_trips_camel_case() {
        let json = r#"{"workspacePath":"/p","prompt":"x","interval":"1d","agentSource":"codex"}"#;
        let s: RoutineSpec = serde_json::from_str(json).unwrap();
        assert_eq!(s.interval.as_deref(), Some("1d"));
        assert_eq!(s.agent_source.as_deref(), Some("codex"));
        assert_eq!(serde_json::from_str::<RoutineSpec>(&serde_json::to_string(&s).unwrap()).unwrap(), s);
    }
}
