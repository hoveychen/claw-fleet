//! Idle — agent self-reported "turn ended, awaiting next user prompt" sentinel.
//!
//! Produced by `Stop` / `UserPromptSubmit` Claude Code hooks (which call
//! `fleet session idle` / `fleet session resume`). The `Stop` hook doubles as
//! the trigger for handoff relays (`handoff::consume_and_spawn`), which is why
//! the write side survives the removal of the fleet-session supervisor that
//! used to consume these sentinels.
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
    let rec = IdleRecord { since: now_ms() };
    let json = serde_json::to_string(&rec).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write turn-start sentinel: {e}"))
}

/// When this session's current turn began, in epoch ms. `None` when no prompt
/// has been stamped yet (a session predating this sentinel, or a spawn whose
/// opening prompt bypassed the hook).
pub fn read_turn_start(session_id: &str) -> Option<u64> {
    let path = turn_start_path(session_id)?;
    let body = fs::read_to_string(path).ok()?;
    serde_json::from_str::<IdleRecord>(&body).ok().map(|r| r.since)
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
        let rec = IdleRecord { since: 1_700_000_000_000 };
        let s = serde_json::to_string(&rec).unwrap();
        let back: IdleRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(back.since, rec.since);
    }
}
