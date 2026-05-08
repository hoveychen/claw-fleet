//! Idle — agent self-reported "turn ended, awaiting next user prompt" sentinel.
//!
//! Produced by `Stop` / `UserPromptSubmit` Claude Code hooks (which call
//! `fleet session idle` / `fleet session resume`). Consumed by the supervisor's
//! `pending_session_ids()` to flip the kanban card from Running → Pending when
//! the agent has yielded its turn.
//!
//! Why a separate file-based sentinel instead of `fleet session status pending`:
//! the supervisor's `tick_macos` reconciles `FleetSession.status` every 2–5s by
//! reading guard / elicitation pending requests, and would clobber any status
//! the agent set directly back to `running`. Routing through the same
//! `pending_session_ids()` set as guard/elicitation makes the signal a
//! first-class peer rather than a value that gets overwritten.
//!
//! File layout: `~/.fleet/idle/<session_id>.json` containing `{ "since": <ms> }`.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IdleRecord {
    pub since: u64,
}

fn idle_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("idle"))
}

#[cfg(test)]
fn idle_dir_in(home: &std::path::Path) -> PathBuf {
    home.join(".fleet").join("idle")
}

fn record_path(id: &str) -> Option<PathBuf> {
    idle_dir().map(|d| d.join(format!("{id}.json")))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Mark a session idle (agent's turn ended, awaiting next user prompt).
/// Idempotent — re-marking refreshes the `since` timestamp.
pub fn mark_idle(session_id: &str) -> Result<(), String> {
    let dir = idle_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create idle dir: {e}"))?;
    let path = record_path(session_id).unwrap();
    let rec = IdleRecord { since: now_ms() };
    let json = serde_json::to_string(&rec).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write idle sentinel: {e}"))
}

/// Clear a session's idle sentinel. Idempotent — missing file is OK.
pub fn clear_idle(session_id: &str) {
    if let Some(p) = record_path(session_id) {
        let _ = fs::remove_file(p);
    }
}

/// List session IDs currently marked idle.
pub fn list_idle_sessions() -> Vec<String> {
    let Some(dir) = idle_dir() else {
        return Vec::new();
    };
    list_idle_in_dir(&dir)
}

/// Read a session's idle sentinel, if any. Used by the wait-for-input
/// DecisionPanel card to surface "since X minutes ago".
pub fn read_idle(session_id: &str) -> Option<IdleRecord> {
    let path = record_path(session_id)?;
    let s = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&s).ok()
}

fn list_idle_in_dir(dir: &std::path::Path) -> Vec<String> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    entries
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            name.strip_suffix(".json").map(|s| s.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_tmp_home(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "fleet-idle-{}-{}-{}",
            tag,
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn list_in_missing_dir_returns_empty() {
        let home = fresh_tmp_home("missing");
        let dir = idle_dir_in(&home);
        assert!(!dir.exists());
        let ids = list_idle_in_dir(&dir);
        assert!(ids.is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn mark_then_list_then_clear() {
        let home = fresh_tmp_home("roundtrip");
        let dir = idle_dir_in(&home);
        fs::create_dir_all(&dir).unwrap();

        // mark by hand (avoid HOME-dependent helper)
        let path = dir.join("sess-a.json");
        let rec = IdleRecord { since: 12345 };
        fs::write(&path, serde_json::to_string(&rec).unwrap()).unwrap();
        // also a non-json file that must be ignored
        fs::write(dir.join("ignored.txt"), "x").unwrap();

        let ids = list_idle_in_dir(&dir);
        assert_eq!(ids, vec!["sess-a".to_string()]);

        // clear by hand
        fs::remove_file(&path).unwrap();
        let ids = list_idle_in_dir(&dir);
        assert!(ids.is_empty(), "after clear: {ids:?}");

        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn list_skips_non_json_entries() {
        let home = fresh_tmp_home("filter");
        let dir = idle_dir_in(&home);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.json"), "{\"since\":1}").unwrap();
        fs::write(dir.join("b.json"), "{\"since\":2}").unwrap();
        fs::write(dir.join("c.tmp"), "tmp").unwrap();
        fs::write(dir.join("README"), "readme").unwrap();

        let mut ids = list_idle_in_dir(&dir);
        ids.sort();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn idle_record_round_trips_through_json() {
        let rec = IdleRecord { since: 1_700_000_000_000 };
        let s = serde_json::to_string(&rec).unwrap();
        let back: IdleRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(back.since, rec.since);
    }
}
