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
    let _ = atomic_write_string(&path, &format!("{}\n{}\n", ts_ms, pid));
}

/// Write `content` to `path` such that a concurrent reader either sees the
/// previous file contents or the new contents, never an intermediate empty
/// or partial state.
///
/// `fs::write` alone is `open(O_TRUNC) → write → close`, which exposes a
/// brief window where the file exists but has zero bytes. Hooks polling
/// `is_consumer_alive` would race that window, conclude "consumer dead",
/// and tear down the in-flight elicitation/plan/guard request — closing
/// the user's decision panel out from under them.
///
/// `rename(2)` is atomically replace-or-fail on POSIX: a reader of `path`
/// only ever sees the old content (until rename) or the full new content
/// (after rename), never a half-written intermediate. The pid-suffixed
/// tmp name keeps two writers (desktop + `fleet serve`) from fighting
/// over the same tmp file.
fn atomic_write_string(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "atomic_write: path has no parent")
    })?;
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("heartbeat");
    let tmp = parent.join(format!(".{}.tmp.{}", file_name, std::process::id()));
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)
}

/// Structured reason for `consumer_status` — used by hooks to log *why* the
/// liveness check failed instead of swallowing the result behind a bool.
///
/// `Alive` is the only "consumer is reachable" variant; everything else is a
/// distinct failure mode and we want them as separate log lines so we can
/// tell a "file truncated mid-write" race apart from a "consumer really
/// gone" case.
#[derive(Debug, Clone)]
pub enum ConsumerStatus {
    /// Consumer reachable. `fresh = true` means the timestamp is within
    /// `stale_after`; `fresh = false` means the timestamp is stale but the
    /// recorded pid still exists (frozen-process case).
    Alive { fresh: bool, pid: Option<u32> },
    /// `real_home_dir()` returned None — we can't even compute the path.
    HomeDirUnknown,
    /// Heartbeat file does not exist or `read_to_string` errored. We don't
    /// distinguish further because both end up at the same `Err(io::Error)`
    /// from the kernel and reading the kind requires more code than the
    /// telemetry value warrants.
    FileUnreadable(String),
    /// File exists but contains no usable bytes. Most likely cause: the
    /// writer is mid-`fs::write` (truncate-then-write race).
    Empty,
    /// First line is present but not a u128. Likely a partial write.
    UnparseableTimestamp { snippet: String },
    /// Timestamp parsed and is older than `stale_after`, and the file has no
    /// pid line (legacy format from older desktop builds).
    StaleNoPid { age_ms: u128 },
    /// Timestamp stale and the pid line is unparseable.
    StalePidUnparseable { age_ms: u128, snippet: String },
    /// Timestamp stale and `kill(pid, 0)` reports the pid is gone.
    StalePidDead { age_ms: u128, pid: u32 },
}

impl ConsumerStatus {
    pub fn is_alive(&self) -> bool {
        matches!(self, ConsumerStatus::Alive { .. })
    }
}

impl std::fmt::Display for ConsumerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConsumerStatus::Alive { fresh: true, pid } => {
                write!(f, "alive(fresh, pid={:?})", pid)
            }
            ConsumerStatus::Alive { fresh: false, pid } => {
                write!(f, "alive(stale-but-pid-live, pid={:?})", pid)
            }
            ConsumerStatus::HomeDirUnknown => write!(f, "home-dir-unknown"),
            ConsumerStatus::FileUnreadable(e) => write!(f, "file-unreadable: {}", e),
            ConsumerStatus::Empty => write!(f, "empty (likely mid-write race)"),
            ConsumerStatus::UnparseableTimestamp { snippet } => {
                write!(f, "ts-unparseable: {:?}", snippet)
            }
            ConsumerStatus::StaleNoPid { age_ms } => write!(f, "stale-no-pid (age={}ms)", age_ms),
            ConsumerStatus::StalePidUnparseable { age_ms, snippet } => write!(
                f,
                "stale-pid-unparseable (age={}ms, snippet={:?})",
                age_ms, snippet
            ),
            ConsumerStatus::StalePidDead { age_ms, pid } => {
                write!(f, "stale-pid-dead (age={}ms, pid={})", age_ms, pid)
            }
        }
    }
}

pub fn consumer_status(stale_after: Duration) -> ConsumerStatus {
    let Some(path) = heartbeat_path() else {
        return ConsumerStatus::HomeDirUnknown;
    };
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return ConsumerStatus::FileUnreadable(e.to_string()),
    };
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    classify(&content, now_ms, stale_after.as_millis())
}

pub fn is_consumer_alive(stale_after: Duration) -> bool {
    consumer_status(stale_after).is_alive()
}

fn classify(content: &str, now_ms: u128, stale_after_ms: u128) -> ConsumerStatus {
    if content.trim().is_empty() {
        return ConsumerStatus::Empty;
    }
    let mut lines = content.lines();
    let Some(ts_line) = lines.next() else {
        return ConsumerStatus::Empty;
    };
    let ts_trimmed = ts_line.trim();
    let Ok(ts_ms) = ts_trimmed.parse::<u128>() else {
        return ConsumerStatus::UnparseableTimestamp {
            snippet: ts_trimmed.chars().take(40).collect(),
        };
    };
    let age_ms = now_ms.saturating_sub(ts_ms);
    if age_ms < stale_after_ms {
        let pid = lines.next().and_then(|l| l.trim().parse::<u32>().ok());
        return ConsumerStatus::Alive { fresh: true, pid };
    }
    // Heartbeat is stale. If the writer recorded its pid, treat the
    // consumer as alive whenever the process still exists — covers the
    // system-sleep case where the writer thread is frozen along with the
    // rest of the app.
    let Some(pid_line) = lines.next() else {
        return ConsumerStatus::StaleNoPid { age_ms };
    };
    let pid_trimmed = pid_line.trim();
    let Ok(pid) = pid_trimmed.parse::<u32>() else {
        return ConsumerStatus::StalePidUnparseable {
            age_ms,
            snippet: pid_trimmed.chars().take(40).collect(),
        };
    };
    if process_alive(pid) {
        ConsumerStatus::Alive {
            fresh: false,
            pid: Some(pid),
        }
    } else {
        ConsumerStatus::StalePidDead { age_ms, pid }
    }
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

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    use std::ffi::c_void;
    type Handle = *mut c_void;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    extern "system" {
        fn OpenProcess(
            dw_desired_access: u32,
            b_inherit_handle: i32,
            dw_process_id: u32,
        ) -> Handle;
        fn CloseHandle(h_object: Handle) -> i32;
        fn GetExitCodeProcess(h_process: Handle, lp_exit_code: *mut u32) -> i32;
    }
    // SAFETY: OpenProcess returns NULL on failure (no handle to close);
    // on success we always pair the handle with CloseHandle. A pid that
    // has exited but whose handle is still openable reports STILL_ACTIVE
    // only while running — once exited, GetExitCodeProcess returns the
    // real exit code.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let mut exit_code: u32 = 0;
        let got = GetExitCodeProcess(h, &mut exit_code) != 0;
        CloseHandle(h);
        got && exit_code == STILL_ACTIVE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STALE_AFTER_MS: u128 = 30_000;

    #[test]
    fn fresh_heartbeat_is_alive() {
        let now: u128 = 1_000_000_000;
        let content = format!("{}\n", now - 1_000); // 1s old
        let s = classify(&content, now, STALE_AFTER_MS);
        assert!(s.is_alive(), "expected alive, got {s}");
        assert!(matches!(s, ConsumerStatus::Alive { fresh: true, .. }));
    }

    #[test]
    fn stale_heartbeat_without_pid_reports_stale_no_pid() {
        let now: u128 = 1_000_000_000;
        let content = format!("{}\n", now - 60_000);
        let s = classify(&content, now, STALE_AFTER_MS);
        assert!(!s.is_alive());
        assert!(matches!(s, ConsumerStatus::StaleNoPid { age_ms } if age_ms == 60_000));
    }

    #[test]
    fn stale_heartbeat_with_live_pid_is_alive() {
        let now: u128 = 1_000_000_000;
        let our_pid = std::process::id();
        let content = format!("{}\n{}\n", now - 60_000, our_pid);
        let s = classify(&content, now, STALE_AFTER_MS);
        assert!(s.is_alive());
        assert!(matches!(s, ConsumerStatus::Alive { fresh: false, pid: Some(p) } if p == our_pid));
    }

    #[test]
    fn stale_heartbeat_with_dead_pid_reports_stale_pid_dead() {
        let now: u128 = 1_000_000_000;
        let content = format!("{}\n0\n", now - 60_000);
        let s = classify(&content, now, STALE_AFTER_MS);
        assert!(!s.is_alive());
        assert!(matches!(s, ConsumerStatus::StalePidDead { pid: 0, age_ms } if age_ms == 60_000));
    }

    #[test]
    fn unparseable_timestamp_reports_unparseable() {
        let s = classify("not-a-number\n", 1_000_000_000, STALE_AFTER_MS);
        assert!(!s.is_alive());
        assert!(matches!(s, ConsumerStatus::UnparseableTimestamp { .. }));
    }

    #[test]
    fn empty_content_reports_empty() {
        // The mid-write race signature: file truncated, no bytes yet.
        let s = classify("", 1_000_000_000, STALE_AFTER_MS);
        assert!(!s.is_alive());
        assert!(matches!(s, ConsumerStatus::Empty));
    }

    #[test]
    fn whitespace_only_reports_empty() {
        let s = classify("\n", 1_000_000_000, STALE_AFTER_MS);
        assert!(!s.is_alive());
        assert!(matches!(s, ConsumerStatus::Empty));
    }

    #[test]
    fn stale_pid_line_unparseable_reports_pid_unparseable() {
        let now: u128 = 1_000_000_000;
        let content = format!("{}\nnot-a-pid\n", now - 60_000);
        let s = classify(&content, now, STALE_AFTER_MS);
        assert!(!s.is_alive());
        assert!(matches!(s, ConsumerStatus::StalePidUnparseable { age_ms, .. } if age_ms == 60_000));
    }

    #[test]
    fn process_alive_detects_self() {
        assert!(process_alive(std::process::id()));
    }

    #[test]
    fn process_alive_rejects_zero() {
        assert!(!process_alive(0));
    }

    /// Race repro: a concurrent reader must never see an empty heartbeat
    /// file mid-write. With `fs::write` (which is `open(O_TRUNC) → write →
    /// close`) this fails because the truncate is visible before the write
    /// completes; with a tmp + `fs::rename` it passes because rename is
    /// atomic on POSIX.
    #[test]
    fn atomic_write_never_exposes_empty_to_reader() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

        let dir = std::env::temp_dir().join(format!(
            "fleet-hb-race-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("heartbeat");
        // Seed with non-empty content so the reader sees a known-good
        // baseline before any concurrent write happens.
        atomic_write_string(&path, "0\n0\n").unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let writes = Arc::new(AtomicU64::new(0));
        let saw_empty = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(AtomicU64::new(0));

        let path_w = path.clone();
        let stop_w = stop.clone();
        let writes_w = writes.clone();
        let writer = std::thread::spawn(move || {
            let mut i: u64 = 0;
            while !stop_w.load(Ordering::Relaxed) {
                i += 1;
                let _ = atomic_write_string(&path_w, &format!("{}\n12345\n", i));
                writes_w.fetch_add(1, Ordering::Relaxed);
            }
        });

        let path_r = path.clone();
        let stop_r = stop.clone();
        let saw_empty_r = saw_empty.clone();
        let reads_r = reads.clone();
        let reader = std::thread::spawn(move || {
            while !stop_r.load(Ordering::Relaxed) {
                if let Ok(c) = fs::read_to_string(&path_r) {
                    reads_r.fetch_add(1, Ordering::Relaxed);
                    if c.is_empty() {
                        saw_empty_r.store(true, Ordering::Relaxed);
                        // Don't break — keep reading so we don't bias the
                        // writer's loop count, but the bool is now sticky.
                    }
                }
            }
        });

        // 1.5s is enough for tens of thousands of writes; the truncate
        // window only needs to be hit once for `saw_empty` to flip.
        std::thread::sleep(std::time::Duration::from_millis(1500));
        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
        reader.join().unwrap();
        let _ = fs::remove_dir_all(&dir);

        assert!(
            !saw_empty.load(Ordering::Relaxed),
            "concurrent reader saw an empty heartbeat file after {} writes / {} reads — atomic_write_string is leaking the truncate window",
            writes.load(Ordering::Relaxed),
            reads.load(Ordering::Relaxed),
        );
    }
}
