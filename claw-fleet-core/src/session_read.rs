//! Per-session read/unread state — "have I, the human, looked at the latest
//! messages in this session?".  This is a deliberate axis, orthogonal to both
//! the auto-computed run status (`SessionStatus`) and the manual review mark
//! (`SessionMark`, done/pending): status says what the agent is doing, the mark
//! says whether I've decided it's finished, and *this* says whether I've seen
//! its most recent activity.
//!
//! Only one thing is persisted: the epoch-ms timestamp of the last time the
//! human read the session. "Unread" is *derived*, not stored — a session is
//! unread when its `last_activity_ms` (newest message) is greater than its
//! `last_read_ms` (or it was never read at all). This mirrors how the frontend
//! wants it: mark read once, and any newer message flips it back to unread
//! without another write.
//!
//! Same side-channel pattern as `session_mark`: a Fleet-maintained file keyed by
//! session id, read back into `SessionInfo` at scan time. It lives outside the
//! transcript because the read state changes while the jsonl doesn't — so it's
//! applied in `scan_all_sources` (the non-cached enrich stage), never in the
//! cached deep parse.
//!
//! File layout: `~/.fleet/session-read/<session_id>.json`.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionReadRecord {
    /// Epoch milliseconds of the last read. `SessionInfo.last_read_ms` is stamped
    /// from this; the frontend compares it against `last_activity_ms`.
    pub last_read_ms: u64,
    /// Workspace the session lived in — not load-bearing (session id is the
    /// unique key) but kept for parity with `session_mark` and so a human
    /// inspecting the file can tell what it refers to.
    pub workspace_path: String,
}

/// One session to mark read. Batched so "mark all read" is a single call over
/// the `fleet serve` boundary instead of N round-trips.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionReadItem {
    pub session_id: String,
    pub workspace_path: String,
}

/// HTTP request body for marking sessions read over the `fleet serve` boundary
/// (`RemoteBackend` → `/session_read`). Always a batch (a single mark is a
/// batch of one), stamped with the server's clock on receipt.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MarkSessionsReadRequest {
    pub items: Vec<SessionReadItem>,
}

pub(crate) fn read_dir_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("session-read"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Mark a batch of sessions read as of *now* (server clock). Idempotent; a later
/// message with a greater `last_activity_ms` re-derives them as unread without
/// any write here.
pub fn mark_read(items: &[SessionReadItem]) -> Result<(), String> {
    let dir = read_dir_path().ok_or("cannot determine home dir")?;
    mark_read_in(&dir, items, now_ms())
}

/// Stamp each session's `last_read_ms` from the on-disk records. Mirrors
/// `session_mark::enrich_sessions`: one directory scan into an index, then a map
/// over the sessions — cheaper than a file read per session.
pub fn enrich_sessions(sessions: &mut [crate::session::SessionInfo]) {
    let Some(dir) = read_dir_path() else { return };
    let idx = read_index(&dir);
    if idx.is_empty() {
        return;
    }
    for s in sessions.iter_mut() {
        s.last_read_ms = idx.get(&s.id).copied();
    }
}

// ── Directory-injecting variants ──────────────────────────────────────────────
//
// Same split as `session_mark`: the public wrappers resolve the directory and
// clock from process-global state, tests take a tempdir + fixed clock via these
// `_in` forms so they neither race `$FLEET_HOME` nor touch the real `~/.fleet`.

pub(crate) fn mark_read_in(
    dir: &std::path::Path,
    items: &[SessionReadItem],
    ts_ms: u64,
) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(dir).map_err(|e| format!("create session-read dir: {e}"))?;
    for item in items {
        let rec = SessionReadRecord {
            last_read_ms: ts_ms,
            workspace_path: item.workspace_path.clone(),
        };
        let json = serde_json::to_string(&rec).map_err(|e| format!("serialize: {e}"))?;
        let path = dir.join(format!("{}.json", item.session_id));
        fs::write(&path, json).map_err(|e| format!("write session-read: {e}"))?;
    }
    Ok(())
}

pub(crate) fn read_in(dir: &std::path::Path, session_id: &str) -> Option<SessionReadRecord> {
    let s = fs::read_to_string(dir.join(format!("{session_id}.json"))).ok()?;
    serde_json::from_str(&s).ok()
}

/// Read the whole `session-read/` directory into `session_id → last_read_ms`.
/// Malformed or non-`.json` entries are skipped.
fn read_index(dir: &std::path::Path) -> HashMap<String, u64> {
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
            idx.insert(id.to_string(), rec.last_read_ms);
        }
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str) -> SessionReadItem {
        SessionReadItem {
            session_id: id.to_string(),
            workspace_path: "/ws".to_string(),
        }
    }

    #[test]
    fn record_roundtrips_camelcase() {
        let rec = SessionReadRecord {
            last_read_ms: 123,
            workspace_path: "/ws".to_string(),
        };
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains("\"lastReadMs\""));
        assert!(json.contains("\"workspacePath\""));
        let back: SessionReadRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rec);
    }

    #[test]
    fn mark_then_read_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        mark_read_in(dir.path(), &[item("sess-a")], 999).unwrap();
        assert_eq!(read_in(dir.path(), "sess-a").unwrap().last_read_ms, 999);
    }

    #[test]
    fn batch_marks_every_item() {
        let dir = tempfile::tempdir().unwrap();
        mark_read_in(dir.path(), &[item("aaa"), item("bbb"), item("ccc")], 500).unwrap();
        let idx = read_index(dir.path());
        assert_eq!(idx.get("aaa"), Some(&500));
        assert_eq!(idx.get("bbb"), Some(&500));
        assert_eq!(idx.get("ccc"), Some(&500));
        assert_eq!(idx.get("ddd"), None);
    }

    #[test]
    fn empty_batch_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        mark_read_in(dir.path(), &[], 1).unwrap();
        // No directory need even exist for an empty batch.
        assert!(read_index(dir.path()).is_empty());
    }

    #[test]
    fn re_marking_overwrites_with_the_newer_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        mark_read_in(dir.path(), &[item("sess-x")], 100).unwrap();
        mark_read_in(dir.path(), &[item("sess-x")], 200).unwrap();
        assert_eq!(read_in(dir.path(), "sess-x").unwrap().last_read_ms, 200);
    }
}
