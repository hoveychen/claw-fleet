//! Per-session **manual title override** — "I, the human, want this session to
//! read as *this* in the task list, regardless of what the AI/slug derived".
//!
//! The displayed session title is normally auto-derived
//! (`ai_title ?? slug ?? last_message_preview`). This side-channel lets the
//! human pin a title on top of that. Clearing the override (empty string)
//! removes the file and the display falls back to the auto-derived title.
//!
//! Same Fleet side-channel pattern as [`crate::session_mark`]: a file keyed by
//! session id, read back into `SessionInfo` at scan time. It lives outside the
//! transcript because the override changes while the jsonl doesn't — so, like
//! the mark, it's applied in the non-cached enrich stage
//! (`scan.rs::enrich_sessions` and the desktop `restamp` path), never in the
//! cached deep parse.
//!
//! Crucially the override is stamped onto its **own** `SessionInfo` field
//! (`title_override`), NOT written over `ai_title`: overwriting `ai_title`
//! would lose the pristine value from the cache, so a later "reset to auto"
//! could never restore it during a `restamp` (the original is gone). Keeping a
//! separate field means an empty index cleanly clears the override and the
//! frontend re-derives from the untouched `ai_title`.
//!
//! File layout: `~/.fleet/session-title/<session_id>.json`.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionTitleRecord {
    /// The human-pinned title. Never empty on disk — an empty override clears
    /// the record (see [`set_title_in`]).
    pub title: String,
    /// Workspace the session lived in — not load-bearing (session id is the
    /// unique key) but kept for parity with `session_mark` and so a human
    /// inspecting the file can tell what it refers to.
    pub workspace_path: String,
    /// Epoch milliseconds of the update.
    pub updated: u64,
}

/// HTTP request body for setting a title over the `fleet serve` boundary
/// (`RemoteBackend` → `/session_title`). An absent/empty `title` clears.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionTitleRequest {
    pub session_id: String,
    pub workspace_path: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<String>,
}

pub(crate) fn title_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("session-title"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Set (or clear) a session's manual title override. A non-empty `title`
/// writes/overwrites the record; `None` or an all-whitespace title clears it
/// (removing the file). Idempotent either way. The stored title is trimmed.
pub fn set_title(
    session_id: &str,
    workspace_path: &str,
    title: Option<String>,
) -> Result<(), String> {
    let dir = title_dir().ok_or("cannot determine home dir")?;
    set_title_in(&dir, session_id, workspace_path, title)
}

/// Read a session's title override, if any.
pub fn read(session_id: &str) -> Option<String> {
    read_in(&title_dir()?, session_id).map(|r| r.title)
}

/// Stamp each session's `title_override` from the on-disk overrides. Mirrors
/// [`crate::session_mark::enrich_sessions`]: one directory scan into an index,
/// then a map over the sessions — cheaper than a file read per session.
pub fn enrich_sessions(sessions: &mut [crate::session::SessionInfo]) {
    let Some(dir) = title_dir() else { return };
    enrich_sessions_in(&dir, sessions);
}

/// The index is the whole truth: a session missing from it has *no* override,
/// so an empty index clears every override rather than being a no-op. This is
/// what makes "reset to auto title" work on the cached (already-enriched) list
/// during a desktop `restamp` — the same reasoning as
/// [`crate::session_mark::enrich_sessions_in`].
pub(crate) fn enrich_sessions_in(
    dir: &std::path::Path,
    sessions: &mut [crate::session::SessionInfo],
) {
    let idx = title_index(dir);
    for s in sessions.iter_mut() {
        s.title_override = idx.get(&s.id).cloned();
    }
}

// ── Directory-injecting variants ──────────────────────────────────────────────
//
// Same split as `session_mark`: the public wrappers resolve the directory from
// process-global state, tests take a tempdir via these `_in` forms so they
// neither race `$FLEET_HOME` nor touch the real `~/.fleet`.

pub(crate) fn set_title_in(
    dir: &std::path::Path,
    session_id: &str,
    workspace_path: &str,
    title: Option<String>,
) -> Result<(), String> {
    let path = dir.join(format!("{session_id}.json"));
    let trimmed = title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    match trimmed {
        None => {
            let _ = fs::remove_file(&path);
            Ok(())
        }
        Some(title) => {
            fs::create_dir_all(dir).map_err(|e| format!("create session-title dir: {e}"))?;
            let rec = SessionTitleRecord {
                title,
                workspace_path: workspace_path.to_string(),
                updated: now_ms(),
            };
            let json = serde_json::to_string(&rec).map_err(|e| format!("serialize: {e}"))?;
            fs::write(&path, json).map_err(|e| format!("write session-title: {e}"))
        }
    }
}

pub(crate) fn read_in(dir: &std::path::Path, session_id: &str) -> Option<SessionTitleRecord> {
    let s = fs::read_to_string(dir.join(format!("{session_id}.json"))).ok()?;
    serde_json::from_str(&s).ok()
}

/// Read the whole `session-title/` directory into `session_id → title`.
/// Malformed or non-`.json` entries are skipped.
fn title_index(dir: &std::path::Path) -> HashMap<String, String> {
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
            idx.insert(id.to_string(), rec.title);
        }
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_roundtrips_camelcase() {
        let rec = SessionTitleRecord {
            title: "My renamed session".to_string(),
            workspace_path: "/ws".to_string(),
            updated: 123,
        };
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains("\"workspacePath\""));
        assert!(json.contains("\"title\":\"My renamed session\""));
        let back: SessionTitleRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rec);
    }

    #[test]
    fn set_then_read_roundtrips_and_trims() {
        let dir = tempfile::tempdir().unwrap();
        set_title_in(dir.path(), "sess-a", "/ws", Some("  Fix the bug  ".into())).unwrap();
        assert_eq!(read_in(dir.path(), "sess-a").unwrap().title, "Fix the bug");
    }

    #[test]
    fn empty_or_whitespace_title_clears_the_file() {
        let dir = tempfile::tempdir().unwrap();
        set_title_in(dir.path(), "sess-b", "/ws", Some("hello".into())).unwrap();
        set_title_in(dir.path(), "sess-b", "/ws", Some("   ".into())).unwrap();
        assert!(read_in(dir.path(), "sess-b").is_none());
        // Explicit None clears too, and clearing an already-absent override is a
        // no-op, not an error.
        set_title_in(dir.path(), "sess-b", "/ws", None).unwrap();
    }

    #[test]
    fn index_skips_unset_and_maps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        set_title_in(dir.path(), "aaa", "/ws", Some("Alpha".into())).unwrap();
        set_title_in(dir.path(), "bbb", "/ws", Some("Beta".into())).unwrap();
        let idx = title_index(dir.path());
        assert_eq!(idx.get("aaa"), Some(&"Alpha".to_string()));
        assert_eq!(idx.get("bbb"), Some(&"Beta".to_string()));
        assert_eq!(idx.get("ccc"), None);
    }

    #[test]
    fn enrich_stamps_from_the_index() {
        let dir = tempfile::tempdir().unwrap();
        set_title_in(dir.path(), "aaa", "/ws", Some("Renamed".into())).unwrap();
        let mut sessions = vec![
            crate::session::test_session("aaa"),
            crate::session::test_session("bbb"),
        ];
        enrich_sessions_in(dir.path(), &mut sessions);
        assert_eq!(sessions[0].title_override.as_deref(), Some("Renamed"));
        assert_eq!(sessions[1].title_override, None, "un-overridden stays None");
    }

    /// Clearing the *last* override empties the index. Enrich must still clear
    /// the field — an early return on an empty index would strand the stale
    /// override on an already-enriched (cached) list forever, so "reset to auto
    /// title" would never take effect until a full rescan.
    #[test]
    fn enrich_clears_when_the_index_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mut sessions = vec![crate::session::test_session("aaa")];
        sessions[0].title_override = Some("stale".into());
        enrich_sessions_in(dir.path(), &mut sessions);
        assert_eq!(sessions[0].title_override, None);
    }
}
