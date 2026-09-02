//! Catch a write that went to the wrong machine.
//!
//! # The hole this watches
//!
//! rca routes an agent's file operations to the remote host by **absolute path
//! prefix** — and only by absolute path. A relative path is deliberately left
//! alone (`native/macos/rcc_interpose.c`, above `resolve_at`): resolving every
//! relative `open` against the cwd destabilises the agent's own boot, and the
//! agent's file tools are expected to resolve to absolute paths before opening.
//!
//! Measured on 2026-09-02 against rca v0.2.0, that expectation fails in a
//! particularly unpleasant direction:
//!
//! | call | result |
//! |------|--------|
//! | `read("marker.txt")` | local mirror → `NotFound` |
//! | `read("<abs>/marker.txt")` | routed to the remote |
//! | **`write("out.txt")`** | **succeeds — into the local mirror** |
//!
//! A misrouted *read* fails loudly. A misrouted *write* returns success and
//! silently lands on the operator's laptop instead of the remote host, where
//! nothing will ever look for it.
//!
//! # Why "any file at all" is a safe rule
//!
//! Because the local mirror is supposed to stay empty. It is created empty at
//! launch (`remote_workspace::wrap_launch`) purely so the child has a cwd; every
//! real file lives on the remote. Two full real sessions against a remote
//! workspace on 2026-09-02 — one read-only, one that created a file — both left
//! the mirror with **zero** entries, agent state and all.
//!
//! So this does not guess at an allowlist of "legitimate" local files: it
//! reports whatever it finds, by name, and lets the human judge. If a future
//! agent legitimately writes something there, the report names it rather than
//! silently tolerating a class it was never measured against.
//!
//! Advisory only — unlike [`crate::remote_disconnect`] it does **not** override
//! the session's status. The session may well have finished perfectly; the
//! warning is that some of its output is on the wrong host.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Names beyond this are dropped from the report (`truncated` goes true). The
/// card shows a couple and the count; a thousand names would help nobody.
const MAX_NAMES: usize = 20;

/// Files found in a remote workspace's local mirror after the session ended.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct MirrorWrite {
    /// Entry names found at the top level of the mirror, sorted. Names only —
    /// the point is "these exist here and shouldn't", not their contents.
    pub files: Vec<String>,
    /// True when more than [`MAX_NAMES`] entries were found and `files` holds
    /// only the first few.
    pub truncated: bool,
    /// Total entries found, including any dropped by truncation.
    pub total: u32,
    /// The mirror directory (identical to the remote workspace path).
    pub workspace_path: String,
    /// Epoch milliseconds of the check.
    pub detected_at_ms: u64,
}

pub(crate) fn guard_dir() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("mirror-write"))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// List what is sitting in `mirror` right now. `None` when the directory is
/// empty or unreadable — i.e. nothing to report.
///
/// Split out from the recording so the rule ("anything at all") is testable
/// without a session, a filesystem layout, or a `FLEET_HOME`.
pub fn inspect_mirror(mirror: &Path) -> Option<MirrorWrite> {
    let mut names: Vec<String> = fs::read_dir(mirror)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    if names.is_empty() {
        return None;
    }
    names.sort();
    let total = names.len() as u32;
    let truncated = names.len() > MAX_NAMES;
    names.truncate(MAX_NAMES);
    Some(MirrorWrite {
        files: names,
        truncated,
        total,
        workspace_path: mirror.to_string_lossy().into_owned(),
        detected_at_ms: now_ms(),
    })
}

/// Check a finished rca-wrapped session's mirror and file a report if anything
/// is in it. Called from the spawn reaper, so it runs exactly once per session
/// and never on the hot scan path.
pub fn check_after_exit(session_id: &str, workspace_path: &str) {
    let Some(found) = inspect_mirror(Path::new(workspace_path)) else {
        // The overwhelmingly common case: the mirror is empty, as designed.
        // Clear any stale report so a session that misbehaved once and was then
        // re-run cleanly stops being flagged.
        clear(session_id);
        return;
    };
    crate::log_debug(&format!(
        "mirror_guard: {} local file(s) in the mirror for session={session_id} ws={workspace_path}: {:?}",
        found.total, found.files
    ));
    if let Some(dir) = guard_dir() {
        if let Err(e) = record_in(&dir, session_id, &found) {
            crate::log_debug(&format!("mirror_guard: record failed: {e}"));
        }
    }
}

/// Read a session's report, if any.
pub fn read(session_id: &str) -> Option<MirrorWrite> {
    read_in(&guard_dir()?, session_id)
}

/// Forget a session's report — the human has dealt with it, or the session is
/// being re-run. Idempotent.
pub fn clear(session_id: &str) {
    if let Some(dir) = guard_dir() {
        clear_in(&dir, session_id);
    }
}

/// Stamp each session's `mirror_write` from the on-disk reports. Same scan-time
/// enrich shape as [`crate::remote_disconnect::enrich_sessions`], and likewise
/// authoritative: no report on disk means no warning, so a cleared one really
/// disappears from an already-enriched list.
pub fn enrich_sessions(sessions: &mut [crate::session::SessionInfo]) {
    let Some(dir) = guard_dir() else { return };
    enrich_sessions_in(&dir, sessions);
}

// ── Directory-injecting variants (tests take a tempdir) ──────────────────────

pub(crate) fn record_in(dir: &Path, session_id: &str, rec: &MirrorWrite) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create mirror-write dir: {e}"))?;
    let json = serde_json::to_vec(rec).map_err(|e| format!("serialize: {e}"))?;
    crate::atomic_json::write_atomic(&dir.join(format!("{session_id}.json")), &json)
        .map_err(|e| format!("write mirror-write: {e}"))
}

pub(crate) fn read_in(dir: &Path, session_id: &str) -> Option<MirrorWrite> {
    let s = fs::read_to_string(dir.join(format!("{session_id}.json"))).ok()?;
    serde_json::from_str(&s).ok()
}

pub(crate) fn clear_in(dir: &Path, session_id: &str) {
    let _ = fs::remove_file(dir.join(format!("{session_id}.json")));
}

pub(crate) fn enrich_sessions_in(dir: &Path, sessions: &mut [crate::session::SessionInfo]) {
    let Ok(entries) = fs::read_dir(dir) else {
        for s in sessions.iter_mut() {
            s.mirror_write = None;
        }
        return;
    };
    let mut idx = std::collections::HashMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        if let Some(rec) = read_in(dir, id) {
            idx.insert(id.to_string(), rec);
        }
    }
    for s in sessions.iter_mut() {
        s.mirror_write = idx.get(&s.id).cloned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "fleet-mirrorguard-{tag}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// The measured baseline: a healthy remote session leaves the mirror empty,
    /// so an empty mirror must produce no report at all. If this ever reported,
    /// every remote session on the board would carry a false warning.
    #[test]
    fn an_empty_mirror_is_silent() {
        let dir = tmpdir("empty");
        assert_eq!(inspect_mirror(&dir), None);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A single misrouted write is the whole point — it must not need a
    /// threshold to be worth reporting.
    #[test]
    fn one_stray_file_is_reported_by_name() {
        let dir = tmpdir("one");
        fs::write(dir.join("out.txt"), "oops").unwrap();
        let got = inspect_mirror(&dir).expect("a stray file must be reported");
        assert_eq!(got.files, vec!["out.txt"]);
        assert_eq!(got.total, 1);
        assert!(!got.truncated);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Names are capped but the count is not: "3 files" and "300 files" are very
    /// different situations and the card must be able to tell them apart.
    #[test]
    fn many_files_truncate_names_but_keep_the_count() {
        let dir = tmpdir("many");
        for i in 0..(MAX_NAMES + 5) {
            fs::write(dir.join(format!("f{i:03}.txt")), "x").unwrap();
        }
        let got = inspect_mirror(&dir).unwrap();
        assert_eq!(got.total as usize, MAX_NAMES + 5);
        assert_eq!(got.files.len(), MAX_NAMES);
        assert!(got.truncated);
        // Sorted, so the names shown are stable between runs.
        assert_eq!(got.files[0], "f000.txt");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A missing mirror directory is not a warning — it means the session never
    /// launched, not that something was misrouted.
    #[test]
    fn a_missing_mirror_is_silent() {
        assert_eq!(inspect_mirror(Path::new("/nonexistent/fleet/mirror")), None);
    }

    fn mk_session(id: &str) -> crate::session::SessionInfo {
        crate::session::SessionInfo { id: id.to_string(), ..Default::default() }
    }

    #[test]
    fn enrich_stamps_and_clears() {
        let dir = tmpdir("enrich");
        let rec = MirrorWrite {
            files: vec!["out.txt".into()],
            truncated: false,
            total: 1,
            workspace_path: "/srv/repo".into(),
            detected_at_ms: now_ms(),
        };
        record_in(&dir, "dirty", &rec).unwrap();

        let mut sessions = vec![mk_session("dirty"), mk_session("clean")];
        // "clean" arrives carrying a stale report from a previous enrich.
        sessions[1].mirror_write = Some(rec.clone());
        enrich_sessions_in(&dir, &mut sessions);

        assert_eq!(sessions[0].mirror_write, Some(rec));
        assert_eq!(sessions[1].mirror_write, None);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A session that misbehaved once and is then re-run cleanly must stop being
    /// flagged — otherwise the warning is permanent and people learn to ignore
    /// it.
    #[test]
    fn a_later_clean_exit_clears_the_old_report() {
        let lock = crate::session::fleet_home_lock();
        let home = tmpdir("clean-exit");
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialized on the process-wide FLEET_HOME lock.
        unsafe { std::env::set_var("FLEET_HOME", &home) };

        let mirror = home.join("mirror");
        fs::create_dir_all(&mirror).unwrap();
        fs::write(mirror.join("stray.txt"), "x").unwrap();
        check_after_exit("s1", mirror.to_str().unwrap());
        assert!(read("s1").is_some(), "the stray file must be reported");

        fs::remove_file(mirror.join("stray.txt")).unwrap();
        check_after_exit("s1", mirror.to_str().unwrap());
        assert_eq!(read("s1"), None, "a clean exit must retract the warning");

        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        drop(lock);
        let _ = fs::remove_dir_all(&home);
    }
}
