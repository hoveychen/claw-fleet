//! What Fleet actually launched a session with.
//!
//! Reconstructing a session's `--model` from its transcript is lossy by
//! construction: Claude Code records the *resolved* id (`claude-opus-4-8`) and
//! drops bracketed opt-in suffixes, so a 1M-context `opus[1m]` session is
//! indistinguishable on disk from a 200K `opus` one.
//! [`crate::session::reconcile_model_spec`] papers over the common case by
//! re-applying the suffix from `~/.claude/settings.json` when the family
//! matches — but a session launched with an *explicit* `--model opus[1m]` while
//! settings default to something else has no signal left at all, and would come
//! back on the wrong model.
//!
//! For the sessions Fleet spawns, none of that guessing is necessary: Fleet
//! chose the flags. It just has to write them down. This module is that note —
//! `~/.fleet/launch-spec/<session_id>.json`, recorded at spawn, read back by
//! [`crate::session::resolve_session_model_spec`], and therefore inherited for
//! free by everything that relaunches a session (`handoff`, `parked`,
//! auto-resume).
//!
//! Sessions Fleet did not spawn have no note here, and fall back to the
//! transcript-plus-settings reconstruction exactly as before.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The launch flags a session was started with, as Fleet passed them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchSpec {
    /// The `--model` spec **verbatim**, suffix and all (`claude-opus-4-8[1m]`).
    /// `None` when the spawn passed no override and the CLI picked the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The `--effort` value, same contract as `model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

fn spec_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("launch-spec"))
}

fn spec_path(session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty()
        || session_id.contains('/')
        || session_id.contains('\\')
        || session_id.contains("..")
    {
        return None;
    }
    spec_dir().map(|d| d.join(format!("{session_id}.json")))
}

/// Write down what a session was launched with. Called from every Fleet spawn
/// path (new session, resume, handoff relay) right after the flags are settled.
///
/// The note is written **unconditionally** — even when Fleet passed no
/// model/effort override, so an empty `{}` is stored. Its mere presence is the
/// ground truth for [`was_fleet_spawned`]: the Tasks list needs to tell a
/// session Fleet actually spawned from a `claude -p` child that only *inherited*
/// `CLAUDE_CODE_ENTRYPOINT` from a Fleet-spawned parent's environment. An empty
/// note still reads back as "no override" (`model_of`/`effort_of` return `None`),
/// so the CLI default stays dynamic — the two concerns are decoupled.
pub fn record(session_id: &str, model: Option<&str>, effort: Option<&str>) {
    let spec = LaunchSpec {
        model: model.map(str::trim).filter(|m| !m.is_empty()).map(str::to_string),
        effort: effort.map(str::trim).filter(|e| !e.is_empty()).map(str::to_string),
    };
    let Some(path) = spec_path(session_id) else {
        return;
    };
    // Pin the machine-local cutoff on the first spawn so sessions predating the
    // marker feature can be grandfathered into the Tasks list.
    ensure_marker_since();
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            crate::log_debug(&format!("launch_spec: create dir: {e}"));
            return;
        }
    }
    match serde_json::to_string(&spec) {
        Ok(json) => {
            if let Err(e) = fs::write(&path, json) {
                crate::log_debug(&format!("launch_spec: write {session_id}: {e}"));
            }
        }
        Err(e) => crate::log_debug(&format!("launch_spec: serialize {session_id}: {e}")),
    }
}

/// What Fleet launched this session with, or `None` for a session Fleet didn't
/// spawn (or one spawned before this note existed).
pub fn get(session_id: &str) -> Option<LaunchSpec> {
    let path = spec_path(session_id)?;
    serde_json::from_str(&fs::read_to_string(&path).ok()?).ok()
}

/// The recorded `--model` spec, suffix intact.
pub fn model_of(session_id: &str) -> Option<String> {
    get(session_id)?.model
}

/// The recorded `--effort`.
pub fn effort_of(session_id: &str) -> Option<String> {
    get(session_id)?.effort
}

/// Path of the machine-local sentinel that pins the moment the always-write
/// spawn marker went live on this host (see [`spawn_marker_cutoff_ms`]).
fn marker_since_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("spawn-marker-since"))
}

/// Did Fleet spawn this exact session id? True iff a per-session note exists —
/// written by [`record`] on every Fleet spawn path, even one with no
/// model/effort override. This is the ground truth the Tasks list uses instead
/// of trusting `CLAUDE_CODE_ENTRYPOINT`, which a plain `claude -p` child
/// *inherits* from a Fleet-spawned parent's environment and so cannot be trusted
/// on its own.
pub fn was_fleet_spawned(session_id: &str) -> bool {
    spec_path(session_id).map(|p| p.exists()).unwrap_or(false)
}

/// Epoch-ms after which an entrypoint-Fleet-owned session that carries no spawn
/// marker is a leaked `claude -p` child rather than a real Fleet session.
/// Established on this machine at the first [`record`] once the marker feature
/// shipped; `u64::MAX` (grandfather every prior session) until then.
pub fn spawn_marker_cutoff_ms() -> u64 {
    let Some(path) = marker_since_path() else {
        return u64::MAX;
    };
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(u64::MAX)
}

/// Stamp the cutoff sentinel the first time Fleet records a spawn on this host.
/// Idempotent: later spawns leave the original moment intact.
fn ensure_marker_since() {
    let Some(path) = marker_since_path() else {
        return;
    };
    if path.exists() {
        return;
    }
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            crate::log_debug(&format!("launch_spec: create marker dir: {e}"));
            return;
        }
    }
    if let Err(e) = fs::write(&path, now_ms.to_string()) {
        crate::log_debug(&format!("launch_spec: write marker sentinel: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TmpHome {
        dir: PathBuf,
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl TmpHome {
        fn new(tag: &str) -> Self {
            let lock = crate::session::fleet_home_lock();
            let dir = std::env::temp_dir().join(format!(
                "fleet-launchspec-{}-{}-{}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            let prev = std::env::var_os("FLEET_HOME");
            // SAFETY: serialized on the process-wide FLEET_HOME lock.
            unsafe { std::env::set_var("FLEET_HOME", &dir) };
            Self { dir, prev, _lock: lock }
        }
    }

    impl Drop for TmpHome {
        fn drop(&mut self) {
            unsafe {
                match self.prev.take() {
                    Some(p) => std::env::set_var("FLEET_HOME", p),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// The whole point: the bracketed suffix, which the transcript throws away,
    /// survives here verbatim.
    #[test]
    fn records_the_model_spec_with_its_suffix_intact() {
        let _home = TmpHome::new("suffix");
        record("s1", Some("claude-opus-4-8[1m]"), Some("high"));
        assert_eq!(model_of("s1").as_deref(), Some("claude-opus-4-8[1m]"));
        assert_eq!(effort_of("s1").as_deref(), Some("high"));
    }

    /// A spawn that passed no overrides must still leave a per-session marker —
    /// that marker is the ground truth for "Fleet spawned this exact id", which
    /// the Tasks list uses to reject `claude -p` children that merely *inherited*
    /// `CLAUDE_CODE_ENTRYPOINT` from a Fleet-spawned parent. But an empty note
    /// must NOT read back as a model/effort override: the CLI default stays
    /// dynamic.
    #[test]
    fn a_default_flag_spawn_still_marks_fleet_spawned_without_a_phantom_override() {
        let _home = TmpHome::new("empty");
        record("s2", None, None);
        record("s3", Some("  "), Some(""));
        // Marker is present for both — Fleet spawned them.
        assert!(spec_path("s2").unwrap().exists(), "default-flag spawn must leave a marker");
        assert!(spec_path("s3").unwrap().exists(), "blank-override spawn must leave a marker");
        assert!(was_fleet_spawned("s2"));
        assert!(was_fleet_spawned("s3"));
        // …but no phantom override leaks back out.
        assert_eq!(model_of("s2"), None);
        assert_eq!(effort_of("s2"), None);
        assert_eq!(model_of("s3"), None);
        // A session Fleet never spawned has no marker and no override.
        assert!(!was_fleet_spawned("never-spawned"));
        assert_eq!(model_of("never-spawned"), None);
    }

    /// The cutoff sentinel is established on this machine at the first `record`,
    /// so sessions that predate the marker feature can be grandfathered in.
    #[test]
    fn first_record_establishes_the_marker_cutoff() {
        let _home = TmpHome::new("cutoff");
        assert_eq!(spawn_marker_cutoff_ms(), u64::MAX, "no cutoff before any spawn");
        record("c1", None, None);
        assert_ne!(spawn_marker_cutoff_ms(), u64::MAX, "cutoff stamped after first spawn");
    }

    /// Only one of the two flags is common (model set, effort left to default).
    #[test]
    fn records_a_partial_spec() {
        let _home = TmpHome::new("partial");
        record("s4", Some("claude-fable-5"), None);
        assert_eq!(model_of("s4").as_deref(), Some("claude-fable-5"));
        assert_eq!(effort_of("s4"), None);
    }

    /// A session id is a filename here; it must never be able to climb out.
    #[test]
    fn rejects_ids_that_would_escape_the_store() {
        let _home = TmpHome::new("traversal");
        for bad in ["../evil", "a/b", "", "..\\evil"] {
            assert_eq!(spec_path(bad), None, "id {bad:?} must be refused");
            record(bad, Some("claude-opus-4-8"), None);
            assert_eq!(get(bad), None);
        }
    }
}
