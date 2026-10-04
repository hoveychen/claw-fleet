//! Sessions this process just spawned that no scan has surfaced yet.
//!
//! A new Claude session reaches a client's session list only after the CLI has
//! booted, run its SessionStart hooks and written the first transcript record
//! (2–6s), the watcher's coalescing window has passed (≤2s) and a rescan has
//! run. Clients wait for that row before they switch to the session, so the
//! "starting…" spinner sat there for most of the ~10s before the first reply.
//!
//! Two things live here:
//!
//! - **A provisional row.** The spawn already knows the id, workspace, pid and
//!   transcript path, so [`overlay`] adds a stand-in `SessionInfo` to any list
//!   about to be published while the real row is missing. It is never stored in
//!   the shared list — publishers overlay it on the way out — so the incremental
//!   merge never has to tell it apart from a scanned row. The real row replaces
//!   it the moment a scan finds the transcript.
//! - **Latency evidence.** "New sessions are slow" was patched several times
//!   without a way to tell which leg was slow. [`observe`] logs
//!   `[SPAWN-VISIBLE] <id> +<ms>ms` the first time a published list contains a
//!   scanned row for the id (read by `scripts/startup-latency.sh`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::session::{SessionInfo, SessionStatus};

/// How long a provisional row stands in for a session no scan has found. Past
/// this the process most likely died before writing a transcript, and a row
/// that claims it is running would be a lie.
const PROVISIONAL_TTL: Duration = Duration::from_secs(60);

/// Spawns not seen within this window are forgotten without a latency line.
const FORGET_AFTER: Duration = Duration::from_secs(600);

struct Spawn {
    at: Instant,
    row: SessionInfo,
}

static PENDING: Mutex<Option<HashMap<String, Spawn>>> = Mutex::new(None);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// The stand-in row for a Claude session spawned in `workspace_path`.
pub fn provisional_claude_row(
    session_id: &str,
    workspace_path: &str,
    pid: u32,
    entrypoint: &str,
    prompt: &str,
) -> SessionInfo {
    let jsonl_path = crate::session::get_claude_dir()
        .map(|d| {
            d.join("projects")
                .join(crate::session::encode_workspace_path(workspace_path))
                .join(format!("{session_id}.jsonl"))
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    let now = now_ms();
    SessionInfo {
        id: session_id.to_string(),
        workspace_path: workspace_path.to_string(),
        workspace_name: crate::session::workspace_name(workspace_path),
        entrypoint: Some(entrypoint.to_string()),
        fleet_spawned: true,
        status: SessionStatus::Processing,
        last_activity_ms: now,
        agent_last_activity_ms: now,
        created_at_ms: now,
        last_message_preview: Some(prompt.chars().take(200).collect()),
        jsonl_path,
        pid: Some(pid),
        proc_alive: true,
        agent_source: "claude-code".to_string(),
        ..Default::default()
    }
}

type Listener = std::sync::Arc<dyn Fn() + Send + Sync>;
static LISTENER: Mutex<Option<Listener>> = Mutex::new(None);

/// Register the publisher to run right after every spawn, so the provisional
/// row goes out now instead of riding the next rescan. One per process (the
/// desktop backend); a later call replaces the earlier one.
pub fn set_listener(f: impl Fn() + Send + Sync + 'static) {
    *LISTENER.lock().unwrap() = Some(std::sync::Arc::new(f));
}

/// Record that this process just spawned `row.id`, then wake the publisher.
pub fn note_spawned(row: SessionInfo) {
    {
        let mut guard = PENDING.lock().unwrap();
        guard.get_or_insert_with(HashMap::new).insert(
            row.id.clone(),
            Spawn {
                at: Instant::now(),
                row,
            },
        );
    }
    let listener = LISTENER.lock().unwrap().clone();
    if let Some(f) = listener {
        f();
    }
}

/// Check a scanned list about to be published. Logs and forgets every pending
/// spawn whose id is in it; returns those `(id, elapsed)` pairs.
pub fn observe(sessions: &[SessionInfo]) -> Vec<(String, Duration)> {
    let mut guard = PENDING.lock().unwrap();
    let Some(pending) = guard.as_mut() else {
        return Vec::new();
    };
    if pending.is_empty() {
        return Vec::new();
    }
    pending.retain(|_, s| s.at.elapsed() < FORGET_AFTER);
    let mut seen = Vec::new();
    for sess in sessions {
        if let Some(s) = pending.remove(&sess.id) {
            seen.push((sess.id.clone(), s.at.elapsed()));
        }
    }
    drop(guard);
    for (id, took) in &seen {
        crate::log_debug(&format!("[SPAWN-VISIBLE] {} +{}ms", id, took.as_millis()));
    }
    seen
}

/// Is `path` the transcript of a spawn no scan has surfaced yet?
pub fn is_pending_transcript(path: &str) -> bool {
    let guard = PENDING.lock().unwrap();
    guard
        .as_ref()
        .is_some_and(|p| p.values().any(|s| s.row.jsonl_path == path))
}

/// `sessions` plus a provisional row for every recent spawn it lacks, or
/// `None` when there is nothing to add. Call [`observe`] on the scanned list
/// first so a spawn that just became visible is not doubled.
pub fn overlay(sessions: &[SessionInfo]) -> Option<Vec<SessionInfo>> {
    let guard = PENDING.lock().unwrap();
    let pending = guard.as_ref()?;
    let extra: Vec<SessionInfo> = pending
        .values()
        .filter(|s| s.at.elapsed() < PROVISIONAL_TTL)
        .filter(|s| !sessions.iter().any(|x| x.id == s.row.id))
        .map(|s| s.row.clone())
        .collect();
    if extra.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(sessions.len() + extra.len());
    out.extend(extra);
    out.extend_from_slice(sessions);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str) -> SessionInfo {
        provisional_claude_row(id, "/tmp/ws", 1, "sdk-cli", "hi")
    }

    fn scanned(id: &str) -> SessionInfo {
        SessionInfo {
            id: id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn provisional_row_stands_in_until_the_scan_finds_the_session() {
        let id = "fresh-spawns-test-a";
        note_spawned(row(id));
        let before = overlay(&[scanned("other")]).expect("overlaid");
        assert!(before.iter().any(|s| s.id == id && s.fleet_spawned && s.proc_alive));

        let real = [scanned(id)];
        let seen = observe(&real);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, id);
        // Visible now: no second sighting, and no provisional duplicate.
        assert!(observe(&real).is_empty());
        let after = overlay(&real);
        assert!(after.map_or(true, |v| v.iter().filter(|s| s.id == id).count() == 1));
    }

    #[test]
    fn provisional_row_points_at_the_transcript_claude_will_write() {
        let r = row("fresh-spawns-test-b");
        assert!(r.jsonl_path.ends_with("/projects/-tmp-ws/fresh-spawns-test-b.jsonl"));
        assert_eq!(r.agent_source, "claude-code");
    }
}
