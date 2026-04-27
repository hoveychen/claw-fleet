//! Consumer-presence heartbeat.
//!
//! The party that polls `~/.fleet/guard/` and `~/.fleet/elicitation/` for
//! pending requests (the desktop app, or `fleet serve` when a SSE client is
//! connected) writes this file periodically.  The `fleet guard` /
//! `fleet elicitation` hook CLIs check it before blocking Claude Code on a
//! request that might never be consumed.
//!
//! File format (line-based, written atomically by `fs::write`):
//!   line 1: wall-clock timestamp in ms since epoch
//!   line 2: process id of the writing consumer (optional — older
//!           desktop builds omit it; readers must tolerate that)
//!
//! The pid line was added because `Instant`-based heartbeats can't see
//! whole-process freezes (system sleep / power nap): the writer thread
//! happily resumes after the freeze with no observable monotonic gap, but
//! the wall-clock timestamp it last wrote may be 30s+ behind real time.
//! Hooks that only check freshness then incorrectly conclude the consumer
//! is gone. With the pid we can fall back to "is the consumer process
//! still alive?" for the stale-but-frozen case.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn heartbeat_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("consumer.heartbeat"))
}

pub fn write_heartbeat() {
    let Some(path) = heartbeat_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let pid = std::process::id();
    let _ = fs::write(&path, format!("{}\n{}\n", ts_ms, pid));
}

pub fn is_consumer_alive(stale_after: Duration) -> bool {
    let Some(path) = heartbeat_path() else {
        return false;
    };
    let Ok(content) = fs::read_to_string(&path) else {
        return false;
    };
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    check_liveness(&content, now_ms, stale_after.as_millis())
}

fn check_liveness(content: &str, now_ms: u128, stale_after_ms: u128) -> bool {
    let mut lines = content.lines();
    let Some(ts_line) = lines.next() else {
        return false;
    };
    let Ok(ts_ms) = ts_line.trim().parse::<u128>() else {
        return false;
    };
    if now_ms.saturating_sub(ts_ms) < stale_after_ms {
        return true;
    }
    // Heartbeat is stale. If the writer recorded its pid, treat the
    // consumer as alive whenever the process still exists — covers the
    // system-sleep case where the writer thread is frozen along with the
    // rest of the app.
    if let Some(pid_line) = lines.next() {
        if let Ok(pid) = pid_line.trim().parse::<u32>() {
            return process_alive(pid);
        }
    }
    false
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: kill(pid, 0) is the standard liveness probe on Unix —
    // returns 0 if the process exists (and we have permission to signal
    // it), -1/ESRCH if it does not. No signal is actually delivered.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    // Windows isn't supported yet; assume alive so the heartbeat-only
    // flow remains unchanged on that platform.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const STALE_AFTER_MS: u128 = 30_000;

    #[test]
    fn fresh_heartbeat_is_alive() {
        let now: u128 = 1_000_000_000;
        let content = format!("{}\n", now - 1_000); // 1s old
        assert!(check_liveness(&content, now, STALE_AFTER_MS));
    }

    #[test]
    fn stale_heartbeat_without_pid_is_dead() {
        // Legacy format: only a timestamp, no pid line.
        let now: u128 = 1_000_000_000;
        let content = format!("{}\n", now - 60_000); // 60s old
        assert!(!check_liveness(&content, now, STALE_AFTER_MS));
    }

    #[test]
    fn stale_heartbeat_with_live_pid_is_alive() {
        // The frozen-process case: timestamp is old, but the writing
        // process is still around.
        let now: u128 = 1_000_000_000;
        let our_pid = std::process::id();
        let content = format!("{}\n{}\n", now - 60_000, our_pid);
        assert!(check_liveness(&content, now, STALE_AFTER_MS));
    }

    #[test]
    fn stale_heartbeat_with_dead_pid_is_dead() {
        let now: u128 = 1_000_000_000;
        // pid 0 is reserved on every Unix and never refers to a
        // user-space process; process_alive treats it as dead.
        let content = format!("{}\n0\n", now - 60_000);
        assert!(!check_liveness(&content, now, STALE_AFTER_MS));
    }

    #[test]
    fn unparseable_timestamp_is_dead() {
        let content = "not-a-number\n";
        assert!(!check_liveness(content, 1_000_000_000, STALE_AFTER_MS));
    }

    #[test]
    fn empty_content_is_dead() {
        assert!(!check_liveness("", 1_000_000_000, STALE_AFTER_MS));
    }

    #[test]
    fn process_alive_detects_self() {
        assert!(process_alive(std::process::id()));
    }

    #[test]
    fn process_alive_rejects_zero() {
        assert!(!process_alive(0));
    }
}
