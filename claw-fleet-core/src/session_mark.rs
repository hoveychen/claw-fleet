//! Per-session manual review mark — "have I, the human, looked at this session
//! and is it truly done?".  This is a deliberate second axis, orthogonal to the
//! auto-computed run status (`SessionStatus`, active/idle): status says what the
//! agent is doing right now, the mark says what *I* have decided about it.
//!
//! Three buckets, only two of which are persisted:
//!   * no file        → unmarked = "new / needs review" (the default for every
//!                       freshly-scanned session)
//!   * `mark: pending`→ "I've seen it, not finished"
//!   * `mark: done`   → "truly complete"
//! Clearing a mark (back to unmarked) removes the file.
//!
//! Same side-channel pattern as `task_progress` / `idle`: a Fleet-maintained
//! file keyed by session id, read back into `SessionInfo` at scan time. It lives
//! outside the transcript because the mark changes while the jsonl doesn't — so,
//! like `handoff`, it's applied in `scan_all_sources` (the non-cached enrich
//! stage), never in the cached deep parse.
//!
//! File layout: `~/.fleet/session-mark/<session_id>.json`.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The two persisted mark values. Unmarked is represented by the *absence* of a
/// record, not a variant here, so the frontend's `userMark === undefined` means
/// "new / needs review".
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionMark {
    /// Seen by the human, work not finished.
    Pending,
    /// Reviewed and truly complete.
    Done,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionMarkRecord {
    /// The mark itself.
    pub mark: SessionMark,
    /// Workspace the session lived in — not load-bearing (session id is the
    /// unique key) but kept for parity with `task_progress` and so a human
    /// inspecting the file can tell what it refers to.
    pub workspace_path: String,
    /// Epoch milliseconds of the update.
    pub updated: u64,
}

pub(crate) fn mark_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("session-mark"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Set (or clear) a session's manual mark. `Some(mark)` writes/overwrites the
/// record; `None` clears it (removing the file). Idempotent either way.
pub fn set_mark(
    session_id: &str,
    workspace_path: &str,
    mark: Option<SessionMark>,
) -> Result<(), String> {
    let dir = mark_dir().ok_or("cannot determine home dir")?;
    set_mark_in(&dir, session_id, workspace_path, mark)
}

/// Read a session's mark, if any.
pub fn read(session_id: &str) -> Option<SessionMark> {
    read_in(&mark_dir()?, session_id).map(|r| r.mark)
}

/// Stamp each session's `user_mark` from the on-disk marks. Mirrors
/// `handoff::enrich_sessions`: one directory scan into an index, then a map over
/// the sessions — cheaper than a file read per session.
pub fn enrich_sessions(sessions: &mut [crate::session::SessionInfo]) {
    let Some(dir) = mark_dir() else { return };
    let idx = mark_index(&dir);
    if idx.is_empty() {
        return;
    }
    for s in sessions.iter_mut() {
        s.user_mark = idx.get(&s.id).copied();
    }
}

// ── Directory-injecting variants ──────────────────────────────────────────────
//
// Same split as `task_progress` / `handoff`: the public wrappers resolve the
// directory from process-global state, tests take a tempdir via these `_in`
// forms so they neither race `$FLEET_HOME` nor touch the real `~/.fleet`.

pub(crate) fn set_mark_in(
    dir: &std::path::Path,
    session_id: &str,
    workspace_path: &str,
    mark: Option<SessionMark>,
) -> Result<(), String> {
    let path = dir.join(format!("{session_id}.json"));
    match mark {
        None => {
            let _ = fs::remove_file(&path);
            Ok(())
        }
        Some(mark) => {
            fs::create_dir_all(dir).map_err(|e| format!("create session-mark dir: {e}"))?;
            let rec = SessionMarkRecord {
                mark,
                workspace_path: workspace_path.to_string(),
                updated: now_ms(),
            };
            let json = serde_json::to_string(&rec).map_err(|e| format!("serialize: {e}"))?;
            fs::write(&path, json).map_err(|e| format!("write session-mark: {e}"))
        }
    }
}

pub(crate) fn read_in(dir: &std::path::Path, session_id: &str) -> Option<SessionMarkRecord> {
    let s = fs::read_to_string(dir.join(format!("{session_id}.json"))).ok()?;
    serde_json::from_str(&s).ok()
}

/// Read the whole `session-mark/` directory into `session_id → mark`. Malformed
/// or non-`.json` entries are skipped.
fn mark_index(dir: &std::path::Path) -> HashMap<String, SessionMark> {
    let mut idx = HashMap::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return idx;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Some(rec) = read_in(dir, id) {
            idx.insert(id.to_string(), rec.mark);
        }
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_roundtrips_camelcase() {
        let rec = SessionMarkRecord {
            mark: SessionMark::Pending,
            workspace_path: "/ws".to_string(),
            updated: 123,
        };
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains("\"workspacePath\""));
        assert!(json.contains("\"mark\":\"pending\""));
        let back: SessionMarkRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rec);
    }

    #[test]
    fn set_then_read_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        set_mark_in(dir.path(), "sess-a", "/ws", Some(SessionMark::Done)).unwrap();
        assert_eq!(read_in(dir.path(), "sess-a").unwrap().mark, SessionMark::Done);
    }

    #[test]
    fn clearing_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        set_mark_in(dir.path(), "sess-b", "/ws", Some(SessionMark::Pending)).unwrap();
        set_mark_in(dir.path(), "sess-b", "/ws", None).unwrap();
        assert!(read_in(dir.path(), "sess-b").is_none());
        // Clearing an already-absent mark is a no-op, not an error.
        set_mark_in(dir.path(), "sess-b", "/ws", None).unwrap();
    }

    #[test]
    fn index_skips_unmarked_and_maps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        set_mark_in(dir.path(), "aaa", "/ws", Some(SessionMark::Pending)).unwrap();
        set_mark_in(dir.path(), "bbb", "/ws", Some(SessionMark::Done)).unwrap();
        let idx = mark_index(dir.path());
        assert_eq!(idx.get("aaa"), Some(&SessionMark::Pending));
        assert_eq!(idx.get("bbb"), Some(&SessionMark::Done));
        assert_eq!(idx.get("ccc"), None);
    }
}
