//! Idle — agent self-reported "turn ended, awaiting next user prompt" sentinel.
//!
//! Produced by `Stop` / `UserPromptSubmit` Claude Code hooks (which call
//! `fleet session idle` / `fleet session resume`). The `Stop` hook doubles as
//! the trigger for handoff relays (`handoff::consume_and_spawn`), which is why
//! the write side survives the removal of the fleet-session supervisor that
//! used to consume these sentinels.
//!
//! File layout: `~/.fleet/idle/<session_id>.json` containing `{ "since": <ms> }`,
//! plus `"background": [...]` when the turn ended with background tasks still
//! running (see [`IdleRecord::background`]).

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IdleRecord {
    pub since: u64,
    /// Background tasks (`Stop` payload `background_tasks`) still running when
    /// the turn ended. A Stop with entries here is not the session's last word:
    /// a headless `-p` run stays alive waiting for its subagents, and an
    /// interactive one is re-invoked when they report, so either way another
    /// turn is coming. Read by [`crate::session_probe`] so a session watch does
    /// not call such a session finished.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub background: Vec<crate::bg_guard::BackgroundTask>,
}

fn idle_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("idle"))
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
/// Idempotent — re-marking refreshes the `since` timestamp and replaces the
/// background-task list. Only tasks still `running` are kept.
pub fn mark_idle(
    session_id: &str,
    background: &[crate::bg_guard::BackgroundTask],
) -> Result<(), String> {
    let dir = idle_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create idle dir: {e}"))?;
    let path = record_path(session_id).unwrap();
    let rec = IdleRecord {
        since: now_ms(),
        background: background.iter().filter(|t| t.is_running()).cloned().collect(),
    };
    let json = serde_json::to_string(&rec).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write idle sentinel: {e}"))
}

fn turn_start_path(id: &str) -> Option<PathBuf> {
    idle_dir().map(|d| d.join(format!("{id}.turn.json")))
}

/// Stamp the moment this session's current turn began (a fresh user prompt).
/// Written by the `UserPromptSubmit` hook alongside clearing the idle sentinel.
///
/// [`crate::plan_gate`] compares the plan-focus timestamp against this to tell
/// "the agent ran `fleet plan …` during *this* turn" from "there is an old focus
/// record lying around from an earlier turn". Using the turn's *start* rather
/// than the previous turn's *end* matters for the interrupt case: when the boss
/// cuts in mid-plan to say "stop", the new turn touches no plan, so the focus
/// record stays older than the turn start and the gate stands down.
pub fn mark_turn_start(session_id: &str) -> Result<(), String> {
    let dir = idle_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create idle dir: {e}"))?;
    let path = turn_start_path(session_id).unwrap();
    let rec = IdleRecord {
        since: now_ms(),
        background: Vec::new(),
    };
    let json = serde_json::to_string(&rec).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write turn-start sentinel: {e}"))
}

/// When this session's current turn began, in epoch ms. `None` when no prompt
/// has been stamped yet (a session predating this sentinel, or a spawn whose
/// opening prompt bypassed the hook).
pub fn read_turn_start(session_id: &str) -> Option<u64> {
    let path = turn_start_path(session_id)?;
    let body = fs::read_to_string(path).ok()?;
    serde_json::from_str::<IdleRecord>(&body)
        .ok()
        .map(|r| r.since)
}

/// The session's idle sentinel, when its last turn ended and no new prompt has
/// arrived since (the `UserPromptSubmit` hook clears it).
pub fn read_idle(session_id: &str) -> Option<IdleRecord> {
    let body = fs::read_to_string(record_path(session_id)?).ok()?;
    serde_json::from_str(&body).ok()
}

/// Clear a session's idle sentinel. Idempotent — missing file is OK.
pub fn clear_idle(session_id: &str) {
    if let Some(p) = record_path(session_id) {
        let _ = fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_record_round_trips_through_json() {
        let rec = IdleRecord {
            since: 1_700_000_000_000,
            background: Vec::new(),
        };
        let s = serde_json::to_string(&rec).unwrap();
        assert_eq!(s, r#"{"since":1700000000000}"#, "no background key when empty");
        let back: IdleRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(back.since, rec.since);
        assert!(back.background.is_empty());
    }

    #[test]
    fn idle_record_keeps_background_tasks() {
        let s = r#"{"since":1,"background":[{"id":"a2cb887591e038bbd","type":"subagent","status":"running","description":"d"}]}"#;
        let back: IdleRecord = serde_json::from_str(s).unwrap();
        assert_eq!(back.background.len(), 1);
        assert!(back.background[0].is_running());
    }
}
