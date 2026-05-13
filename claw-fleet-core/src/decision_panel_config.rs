//! Persistent, cross-process config for the three decision-panel hooks
//! (`fleet guard` / `fleet elicitation` / `fleet plan-approval`).
//!
//! The hook binaries run as Claude Code subprocesses and cannot share memory
//! with the desktop app, so the desktop UI writes the config to
//! `~/.fleet/decision-panel.json` and the hooks read the same file on each
//! invocation. The file format is a plain JSON object with optional fields —
//! missing fields fall back to defaults, so the file can be edited by hand
//! and older Fleet builds (which only know a subset of fields) won't break.
//!
//! All three timing values are stored as plain integers and clamped on load
//! to keep an obviously-bad value (0, negative, absurdly large) from breaking
//! the poll loop. Clamps are intentionally generous — we only want to catch
//! "user typed 0 and saved" mistakes, not police taste.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

/// On-disk shape. Fields are public so Tauri commands can construct & return
/// the value directly without an extra DTO.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DecisionPanelConfig {
    /// Maximum time a hook will wait for the user to respond before timing
    /// out and returning the fall-back decision (block for guard, decline for
    /// elicitation/plan-approval).
    pub wait_seconds: u32,
    /// Poll interval used by the hook while waiting — also bounds how quickly
    /// the hook reacts to a mid-wait heartbeat loss.
    pub poll_ms: u32,
    /// How long the consumer (desktop app / `fleet serve` SSE client) is
    /// allowed to go without writing its heartbeat before the hook considers
    /// it gone and falls through to Claude Code's native UI.
    pub heartbeat_window_seconds: u32,
}

impl DecisionPanelConfig {
    pub const DEFAULT_WAIT_SECONDS: u32 = 600;
    pub const DEFAULT_POLL_MS: u32 = 200;
    pub const DEFAULT_HEARTBEAT_WINDOW_SECONDS: u32 = 30;

    pub const MIN_WAIT_SECONDS: u32 = 60;
    pub const MAX_WAIT_SECONDS: u32 = 3600;
    pub const MIN_POLL_MS: u32 = 50;
    pub const MAX_POLL_MS: u32 = 1000;
    pub const MIN_HEARTBEAT_WINDOW_SECONDS: u32 = 5;
    pub const MAX_HEARTBEAT_WINDOW_SECONDS: u32 = 60;

    pub fn defaults() -> Self {
        Self {
            wait_seconds: Self::DEFAULT_WAIT_SECONDS,
            poll_ms: Self::DEFAULT_POLL_MS,
            heartbeat_window_seconds: Self::DEFAULT_HEARTBEAT_WINDOW_SECONDS,
        }
    }

    /// Convenience wrapper so callers can `DecisionPanelConfig::load()`
    /// without also importing the free `load` function.
    pub fn load() -> Self {
        load()
    }

    pub fn wait_duration(&self) -> Duration {
        Duration::from_secs(self.wait_seconds as u64)
    }

    pub fn poll_duration(&self) -> Duration {
        Duration::from_millis(self.poll_ms as u64)
    }

    pub fn heartbeat_window(&self) -> Duration {
        Duration::from_secs(self.heartbeat_window_seconds as u64)
    }

    /// Force every field into its valid range. Called automatically by
    /// `load` and `save` so neither disk nor memory ever holds a junk value.
    pub fn clamp(&mut self) {
        self.wait_seconds = self
            .wait_seconds
            .clamp(Self::MIN_WAIT_SECONDS, Self::MAX_WAIT_SECONDS);
        self.poll_ms = self.poll_ms.clamp(Self::MIN_POLL_MS, Self::MAX_POLL_MS);
        self.heartbeat_window_seconds = self.heartbeat_window_seconds.clamp(
            Self::MIN_HEARTBEAT_WINDOW_SECONDS,
            Self::MAX_HEARTBEAT_WINDOW_SECONDS,
        );
    }
}

impl Default for DecisionPanelConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

/// On-disk shape with every field optional. Used for deserialization so we
/// can merge partial JSON with defaults instead of failing the whole load
/// when a field is missing (older Fleet build / hand-edited file).
#[derive(Debug, Deserialize)]
struct PartialDecisionPanelConfig {
    wait_seconds: Option<u32>,
    poll_ms: Option<u32>,
    heartbeat_window_seconds: Option<u32>,
}

fn config_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("decision-panel.json"))
}

/// Read `~/.fleet/decision-panel.json` if present, merge with defaults, and
/// clamp every field. Returns defaults on any error (missing file, parse
/// error, IO error) — the hooks must never fail to load this config because
/// failing to load means failing to handle the user's request.
pub fn load() -> DecisionPanelConfig {
    let mut cfg = DecisionPanelConfig::defaults();
    let Some(path) = config_path() else {
        return cfg;
    };
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return cfg,
    };
    let partial: PartialDecisionPanelConfig = match serde_json::from_str(&content) {
        Ok(p) => p,
        Err(e) => {
            crate::log_debug(&format!(
                "[decision_panel_config] failed to parse {:?}: {}; using defaults",
                path, e
            ));
            return cfg;
        }
    };
    if let Some(v) = partial.wait_seconds {
        cfg.wait_seconds = v;
    }
    if let Some(v) = partial.poll_ms {
        cfg.poll_ms = v;
    }
    if let Some(v) = partial.heartbeat_window_seconds {
        cfg.heartbeat_window_seconds = v;
    }
    cfg.clamp();
    cfg
}

/// Persist the config to `~/.fleet/decision-panel.json` atomically (tmp +
/// rename, same recipe as `consumer_heartbeat::atomic_write_string`). Clamps
/// the in-memory value too so callers never end up with a different view
/// than what's on disk.
pub fn save(cfg: &mut DecisionPanelConfig) -> std::io::Result<()> {
    cfg.clamp();
    let path = config_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "decision_panel_config: home dir unknown",
        )
    })?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    let parent = path.parent().unwrap();
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("decision-panel.json");
    let tmp = parent.join(format!(".{}.tmp.{}", file_name, std::process::id()));
    fs::write(&tmp, &content)?;
    fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> DecisionPanelConfig {
        let mut cfg = DecisionPanelConfig::defaults();
        let partial: PartialDecisionPanelConfig = serde_json::from_str(json).unwrap();
        if let Some(v) = partial.wait_seconds {
            cfg.wait_seconds = v;
        }
        if let Some(v) = partial.poll_ms {
            cfg.poll_ms = v;
        }
        if let Some(v) = partial.heartbeat_window_seconds {
            cfg.heartbeat_window_seconds = v;
        }
        cfg.clamp();
        cfg
    }

    #[test]
    fn defaults_match_legacy_hardcoded_values() {
        let cfg = DecisionPanelConfig::defaults();
        assert_eq!(cfg.wait_seconds, 600);
        assert_eq!(cfg.poll_ms, 200);
        assert_eq!(cfg.heartbeat_window_seconds, 30);
    }

    #[test]
    fn empty_object_yields_defaults() {
        let cfg = parse("{}");
        assert_eq!(cfg, DecisionPanelConfig::defaults());
    }

    #[test]
    fn partial_json_merges_with_defaults() {
        // Only wait_seconds is specified; the other two must fall back.
        let cfg = parse(r#"{"wait_seconds": 120}"#);
        assert_eq!(cfg.wait_seconds, 120);
        assert_eq!(cfg.poll_ms, 200);
        assert_eq!(cfg.heartbeat_window_seconds, 30);
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        // Zero wait would mean "give up immediately" — disastrous; clamp up.
        // 5-minute poll would brick the loop; clamp down.
        let cfg = parse(r#"{"wait_seconds": 0, "poll_ms": 999999, "heartbeat_window_seconds": 0}"#);
        assert_eq!(cfg.wait_seconds, DecisionPanelConfig::MIN_WAIT_SECONDS);
        assert_eq!(cfg.poll_ms, DecisionPanelConfig::MAX_POLL_MS);
        assert_eq!(
            cfg.heartbeat_window_seconds,
            DecisionPanelConfig::MIN_HEARTBEAT_WINDOW_SECONDS
        );
    }

    #[test]
    fn upper_bounds_clamp_too() {
        let cfg = parse(r#"{"wait_seconds": 999999, "heartbeat_window_seconds": 999999}"#);
        assert_eq!(cfg.wait_seconds, DecisionPanelConfig::MAX_WAIT_SECONDS);
        assert_eq!(
            cfg.heartbeat_window_seconds,
            DecisionPanelConfig::MAX_HEARTBEAT_WINDOW_SECONDS
        );
    }

    #[test]
    fn save_then_load_roundtrips() {
        // Serialise against any other test that mutates FLEET_HOME — without
        // the lock, a concurrent test's override could leak in between our
        // save() and load() and steer load() to a different directory.
        let _g = crate::session::fleet_home_lock();

        // Use a private home dir so this test doesn't stomp on the user's
        // real `~/.fleet/decision-panel.json`. Override FLEET_HOME (rather
        // than HOME) because real_home_dir() checks FLEET_HOME first on
        // every platform; HOME is ignored on Windows (dirs::home_dir() uses
        // the Known Folder API) and on macOS (we call getpwuid).
        let dir = std::env::temp_dir().join(format!(
            "fleet-decision-panel-cfg-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();

        let prev_fleet_home = std::env::var_os("FLEET_HOME");
        unsafe {
            std::env::set_var("FLEET_HOME", &dir);
        }

        let mut cfg = DecisionPanelConfig {
            wait_seconds: 180,
            poll_ms: 250,
            heartbeat_window_seconds: 15,
        };
        save(&mut cfg).expect("save should succeed");
        let loaded = load();
        assert_eq!(loaded, cfg);

        unsafe {
            match prev_fleet_home {
                Some(v) => std::env::set_var("FLEET_HOME", v),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_json_falls_back_to_defaults() {
        // We can't easily test `load()` against malformed content without
        // sandboxing HOME (covered above), but we can verify the in-process
        // partial path: an entirely bogus blob fails to deserialize, the
        // caller falls back, no panic.
        let result = serde_json::from_str::<PartialDecisionPanelConfig>("not json at all");
        assert!(result.is_err());
    }
}
