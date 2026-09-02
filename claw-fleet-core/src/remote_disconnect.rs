//! Stop-loss for a remote workspace whose transport died mid-session.
//!
//! # Why this exists at all
//!
//! A session on a registered remote workspace runs its file I/O through `rca`
//! over an ssh tunnel. When that tunnel dies, the agent does **not**. Measured
//! twice on 2026-09-02 (`docs/rca-ux-review.md` §7):
//!
//! ```text
//! rca remote recv failed: stream reset: connection closed: EOF
//! tick 2 FAILED: FileNotFoundError: No such file or directory: '…/marker.txt'
//! ```
//!
//! The agent process keeps running and, from its next syscall on, sees the
//! **empty local mirror directory** rca created at launch. What reaches the
//! agent is `FileNotFoundError` — indistinguishable, from inside the turn, from
//! "someone deleted the repository". An agent that believes the repo is gone can
//! reasonably decide to write it again from scratch. That is the damage this
//! module exists to prevent: the first duty on detecting a dead transport is to
//! **stop the agent**, and only then to explain why.
//!
//! # The signal
//!
//! rca already knows. It prints one of three lines on its own stderr — the
//! format strings are in the shipped binary (`remote dial failed: %v`,
//! `remote send failed: %v`, `remote recv failed: %v`) and each one means the
//! link to the remote is gone. Fleet already redirects every spawned agent's
//! stderr to a log file; for rca-wrapped launches [`crate::session_launch`]
//! pipes it through [`watch_stderr`] instead, which tees every line to that same
//! log **and** watches for these markers. Nothing about rca changes.
//!
//! # Where the verdict lands
//!
//! The transport dies outside the transcript, so the transcript cannot carry the
//! news — `session/detect.rs` has nothing to read. Instead this is the same
//! side-channel shape as [`crate::session_mark`] / [`crate::handoff`]: a
//! Fleet-owned record keyed by session id under
//! `~/.fleet/remote-disconnect/<session_id>.json`, folded into `SessionInfo` by
//! [`enrich_sessions`] on the scan path (never in the cached deep parse — the
//! record appears while the jsonl does not change). Presence of a record also
//! overrides the session's status to
//! [`SessionStatus::RemoteDisconnected`](crate::session::SessionStatus::RemoteDisconnected),
//! because every transcript-derived status would otherwise describe a session
//! that no longer exists ("streaming", then quietly "idle").

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The rca stderr markers that mean "the link to the remote host is gone".
///
/// Verbatim prefixes of rca's own `fmt` output — confirmed against the format
/// strings in the shipped binary (`strings ~/.fleet/bin/rca`), not inferred from
/// one observed line. All three are terminal for the session: `dial` never got a
/// link, `send`/`recv` lost one mid-stream. In every case the agent's next file
/// operation hits the empty local mirror.
const TRANSPORT_MARKERS: [&str; 3] = [
    "remote dial failed:",
    "remote send failed:",
    "remote recv failed:",
];

/// Does this rca stderr line announce a dead transport? Returns the matched
/// marker so the caller can record which of the three it was.
///
/// Deliberately narrow: it matches rca's own three transport-failure lines and
/// nothing else. A broader "looks like a network error" heuristic would kill
/// sessions over a `ssh: connect to host … Connection refused` printed by a
/// *tool the agent ran*, which is the agent working correctly.
pub fn transport_failure_marker(line: &str) -> Option<&'static str> {
    TRANSPORT_MARKERS
        .into_iter()
        .find(|m| line.contains(m))
}

/// What Fleet knows about a session whose remote transport died.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct RemoteDisconnect {
    /// Stable code for the UI's localisation table — always
    /// [`codes::TRANSPORT_LOST`](crate::remote_workspace::codes::TRANSPORT_LOST)
    /// today; a field rather than a constant so a later cause (remote OOM, rca
    /// version mismatch) can be told apart without a schema change.
    pub code: String,
    /// The rca stderr line that triggered this, verbatim. Shown as the technical
    /// detail behind the human sentence — never the only thing shown.
    pub detail: String,
    /// Workspace the session was running against.
    pub workspace_path: String,
    /// Display label of the remote workspace, when it has one. Lets a client
    /// with no access to the workspace registry (the CLI, mobile) still name the
    /// host that went away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_label: Option<String>,
    /// Epoch milliseconds the disconnect was detected.
    pub detected_at_ms: u64,
    /// Did Fleet manage to stop the agent? `false` means the kill failed and the
    /// agent may still be running against the empty mirror — the one case where
    /// the user has to intervene by hand, so it must be visible, not swallowed.
    pub agent_stopped: bool,
}

/// `~/.fleet/remote-disconnect`.
pub(crate) fn disconnect_dir() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("remote-disconnect"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Record a disconnect for `session_id`. Overwrites any previous record for the
/// same session (a re-run that dies again is the newer truth).
pub fn record(session_id: &str, rec: &RemoteDisconnect) -> Result<(), String> {
    let dir = disconnect_dir().ok_or("cannot determine home dir")?;
    record_in(&dir, session_id, rec)
}

/// Read a session's disconnect record, if any.
pub fn read(session_id: &str) -> Option<RemoteDisconnect> {
    read_in(&disconnect_dir()?, session_id)
}

/// Forget a session's disconnect. Called when the user restarts the session —
/// the new run either connects (so the old verdict is stale) or fails again and
/// writes a fresh record. Idempotent.
pub fn clear(session_id: &str) {
    if let Some(dir) = disconnect_dir() {
        clear_in(&dir, session_id);
    }
}

/// Stamp each session's `remote_disconnect` from the on-disk records, and
/// override the status of the ones that have a record.
///
/// The status override is the point: a killed session's transcript decays to
/// `Idle` within a minute, which on screen is indistinguishable from a session
/// that finished cleanly. Mirrors [`crate::session_mark::enrich_sessions`] —
/// one directory scan into an index, then a map over the sessions.
pub fn enrich_sessions(sessions: &mut [crate::session::SessionInfo]) {
    let Some(dir) = disconnect_dir() else { return };
    enrich_sessions_in(&dir, sessions);
}

// ── Directory-injecting variants ──────────────────────────────────────────────
//
// Same split as `session_mark` / `handoff`: the public wrappers resolve the
// directory from process-global state, tests take a tempdir via these `_in`
// forms so they neither race `$FLEET_HOME` nor touch the real `~/.fleet`.

pub(crate) fn record_in(
    dir: &Path,
    session_id: &str,
    rec: &RemoteDisconnect,
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create remote-disconnect dir: {e}"))?;
    let json = serde_json::to_vec(rec).map_err(|e| format!("serialize: {e}"))?;
    crate::atomic_json::write_atomic(&dir.join(format!("{session_id}.json")), &json)
        .map_err(|e| format!("write remote-disconnect: {e}"))
}

pub(crate) fn read_in(dir: &Path, session_id: &str) -> Option<RemoteDisconnect> {
    let s = fs::read_to_string(dir.join(format!("{session_id}.json"))).ok()?;
    serde_json::from_str(&s).ok()
}

pub(crate) fn clear_in(dir: &Path, session_id: &str) {
    let _ = fs::remove_file(dir.join(format!("{session_id}.json")));
}

pub(crate) fn enrich_sessions_in(dir: &Path, sessions: &mut [crate::session::SessionInfo]) {
    // The directory is the whole truth: a session missing from it is connected,
    // so an absent/empty dir clears every record rather than being a no-op —
    // otherwise a cleared record would stay stuck on an already-enriched list.
    let Ok(entries) = fs::read_dir(dir) else {
        for s in sessions.iter_mut() {
            s.remote_disconnect = None;
        }
        return;
    };
    let mut idx = std::collections::HashMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Some(rec) = read_in(dir, id) {
            idx.insert(id.to_string(), rec);
        }
    }
    for s in sessions.iter_mut() {
        match idx.get(&s.id) {
            Some(rec) => {
                s.remote_disconnect = Some(rec.clone());
                s.status = crate::session::SessionStatus::RemoteDisconnected;
            }
            None => s.remote_disconnect = None,
        }
    }
}

/// Delete records older than `max_age_secs`. Housekeeping only — a disconnect
/// from last week explains nothing about a session the user is looking at now,
/// and the record would otherwise pin that session's status forever.
pub fn prune_old(max_age_secs: u64) {
    let Some(dir) = disconnect_dir() else { return };
    let Ok(entries) = fs::read_dir(&dir) else { return };
    let cutoff = now_ms().saturating_sub(max_age_secs * 1000);
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let stale = fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<RemoteDisconnect>(&s).ok())
            .map(|r| r.detected_at_ms < cutoff)
            // Unparseable record: it can never be rendered, so it is only litter.
            .unwrap_or(true);
        if stale {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Tee an rca-wrapped agent's stderr to `log_path` while watching for the
/// transport-failure markers. Blocks until the pipe closes (i.e. the child
/// exited), so callers run it on its own thread.
///
/// On the first marker: stop the agent (`stop`), write the record, and keep
/// draining — the pipe must be read to EOF or a chatty child would block on a
/// full pipe buffer, and the remaining lines still belong in the log.
///
/// `stop` is injected rather than called directly so the unit test can assert
/// "the agent was stopped" without spawning a process tree.
pub(crate) fn watch_stderr(
    stderr: impl std::io::Read,
    log_path: &Path,
    session_id: &str,
    workspace_path: &str,
    stop: impl FnOnce() -> bool,
) {
    let mut log = fs::OpenOptions::new().create(true).append(true).open(log_path).ok();
    let mut stop = Some(stop);
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };
        if let Some(f) = log.as_mut() {
            let _ = writeln!(f, "{line}");
        }
        let Some(marker) = transport_failure_marker(&line) else {
            continue;
        };
        // Only the FIRST marker acts: rca can print several as the teardown
        // cascades, and re-killing / re-recording each time would overwrite the
        // record that named the original cause.
        let Some(stop_fn) = stop.take() else { continue };
        crate::log_debug(&format!(
            "remote_disconnect: {marker} on session={session_id} ws={workspace_path} — stopping agent"
        ));
        let agent_stopped = stop_fn();
        let rec = RemoteDisconnect {
            code: crate::remote_workspace::codes::TRANSPORT_LOST.to_string(),
            detail: line.clone(),
            workspace_path: workspace_path.to_string(),
            host_label: crate::remote_workspace::find_for_path(workspace_path)
                .and_then(|w| w.label),
            detected_at_ms: now_ms(),
            agent_stopped,
        };
        if let Err(e) = record(session_id, &rec) {
            crate::log_debug(&format!("remote_disconnect: record failed: {e}"));
        }
        if let Some(f) = log.as_mut() {
            let _ = writeln!(
                f,
                "[{}] fleet remote-disconnect: transport lost ({marker}) — agent {}",
                chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                if agent_stopped { "stopped" } else { "COULD NOT BE STOPPED" }
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "fleet-rdisc-{tag}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn mk_rec() -> RemoteDisconnect {
        RemoteDisconnect {
            code: crate::remote_workspace::codes::TRANSPORT_LOST.to_string(),
            detail: "rca remote recv failed: stream reset: connection closed: EOF".to_string(),
            workspace_path: "/srv/repo".to_string(),
            host_label: Some("own-api-ko".to_string()),
            detected_at_ms: now_ms(),
            agent_stopped: true,
        }
    }

    /// The three lines rca actually prints must be recognised; a line that
    /// merely mentions ssh/network — the shape an *agent's own tool output*
    /// takes — must not be, or Fleet would kill sessions that are working fine.
    #[test]
    fn marker_matching_is_narrow() {
        assert_eq!(
            transport_failure_marker(
                "rca remote recv failed: stream reset: connection closed: EOF"
            ),
            Some("remote recv failed:")
        );
        assert_eq!(
            transport_failure_marker("rca remote send failed: write: broken pipe"),
            Some("remote send failed:")
        );
        assert_eq!(
            transport_failure_marker("rca remote dial failed: dial tcp: i/o timeout"),
            Some("remote dial failed:")
        );

        for benign in [
            "ssh: connect to host example.com port 22: Connection refused",
            "error: Could not resolve host: github.com",
            "FileNotFoundError: No such file or directory: '/srv/repo/x'",
            "rca serve [fs] PREAD handle=2 off=0 len=6 -> 6",
            "",
        ] {
            assert_eq!(transport_failure_marker(benign), None, "must not match: {benign}");
        }
    }

    #[test]
    fn record_read_clear_roundtrip() {
        let dir = tmpdir("rt");
        let rec = mk_rec();
        assert_eq!(read_in(&dir, "s1"), None);
        record_in(&dir, "s1", &rec).unwrap();
        assert_eq!(read_in(&dir, "s1"), Some(rec));
        clear_in(&dir, "s1");
        assert_eq!(read_in(&dir, "s1"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    fn mk_session(id: &str) -> crate::session::SessionInfo {
        crate::session::SessionInfo {
            id: id.to_string(),
            status: crate::session::SessionStatus::Idle,
            ..Default::default()
        }
    }

    /// A session with a record is force-marked disconnected; one without is
    /// left alone AND has any stale field cleared.
    #[test]
    fn enrich_overrides_status_and_clears_stale() {
        let dir = tmpdir("enrich");
        record_in(&dir, "dead", &mk_rec()).unwrap();

        let mut sessions = vec![mk_session("dead"), mk_session("alive")];
        // "alive" arrives already carrying a record from a previous enrich —
        // it must be cleared, not left pinned.
        sessions[1].remote_disconnect = Some(mk_rec());
        sessions[1].status = crate::session::SessionStatus::RemoteDisconnected;

        enrich_sessions_in(&dir, &mut sessions);

        assert_eq!(sessions[0].status, crate::session::SessionStatus::RemoteDisconnected);
        assert_eq!(sessions[0].remote_disconnect.as_ref().map(|r| r.detail.clone()),
                   Some(mk_rec().detail));
        assert_eq!(sessions[1].remote_disconnect, None);
        // Status is not restored (we don't know the old one) but it stops being
        // stamped; the next scan's transcript parse owns it again.
        let _ = fs::remove_dir_all(&dir);
    }

    /// An absent directory (nothing ever disconnected) must clear, not skip.
    #[test]
    fn enrich_with_no_dir_clears() {
        let mut sessions = vec![mk_session("s")];
        sessions[0].remote_disconnect = Some(mk_rec());
        enrich_sessions_in(Path::new("/nonexistent/fleet/remote-disconnect"), &mut sessions);
        assert_eq!(sessions[0].remote_disconnect, None);
    }

    /// The full monitor loop: every line reaches the log, the marker fires the
    /// stop exactly once, and the record names the line that caused it.
    #[test]
    fn watch_stderr_tees_stops_once_and_records() {
        let lock = crate::session::fleet_home_lock();
        let home = tmpdir("watch");
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialized on the process-wide FLEET_HOME lock.
        unsafe { std::env::set_var("FLEET_HOME", &home) };

        let log = home.join("stderr.log");
        let stops = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stops_c = stops.clone();
        let input = "rca serve [fs] PREAD handle=2 -> 6\n\
                     rca remote recv failed: stream reset: connection closed: EOF\n\
                     rca remote recv failed: again\n\
                     trailing line\n";
        watch_stderr(input.as_bytes(), &log, "sess-1", "/srv/repo", move || {
            stops_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            true
        });

        let logged = fs::read_to_string(&log).unwrap();
        for line in ["PREAD handle=2", "stream reset", "again", "trailing line"] {
            assert!(logged.contains(line), "stderr line lost from log: {line}\n{logged}");
        }
        assert_eq!(
            stops.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the agent must be stopped exactly once, not per marker line"
        );
        let rec = read("sess-1").expect("a record must be written");
        assert_eq!(rec.code, crate::remote_workspace::codes::TRANSPORT_LOST);
        assert!(rec.detail.contains("stream reset"), "record must name the first line");
        assert!(rec.agent_stopped);

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
