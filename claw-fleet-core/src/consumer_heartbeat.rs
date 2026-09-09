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
//!   line 3: writer kind, `desktop` or `server` (optional — absent means
//!           `desktop`, which is what every build that predates the line was)
//!
//! The pid line was added because `Instant`-based heartbeats can't see
//! whole-process freezes (system sleep / power nap): the writer thread
//! happily resumes after the freeze with no observable monotonic gap, but
//! the wall-clock timestamp it last wrote may be 30s+ behind real time.
//! Hooks that only check freshness then incorrectly conclude the consumer
//! is gone. With the pid we can fall back to "is the consumer process
//! still alive?" for the stale-but-frozen case.
//!
//! The kind line exists because that fallback is only sound for the desktop
//! app, where the writing process *is* the UI: if it is alive, the window
//! exists and the user can answer, whatever the clock says. For
//! `fleet serve` / `fleet webui` the writer is a daemon and the UI is a
//! browser somewhere else, so the loop writes only while
//! `sse.client_count() > 0` — a stale timestamp there means exactly "no head
//! has been attached for a while", and the pid says nothing about it.
//!
//! Letting the daemon borrow the desktop's fallback made the whole check
//! vacuous on a server: `systemd` keeps `fleet webui` up forever, so
//! `consumer_status` answered `Alive` with no UI in sight, and `fleet__ask`
//! would write a card and block its agent for the full 600s wait with nobody
//! able to see it. That is how two cards were lost on Boss's box on
//! 2026-09-08 (21:54 and 21:57, both `outcome: timeout` at exactly 600s).

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn heartbeat_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("consumer.heartbeat"))
}

/// How late the heartbeat loop may be before its lateness is worth a log line.
pub const STALL_WARN: Duration = Duration::from_millis(2000);

/// How long a single [`write_heartbeat`] may take before it is worth a log line.
pub const SLOW_WRITE_WARN: Duration = Duration::from_millis(500);

/// Scheduling latency for one heartbeat iteration, with the file write removed.
///
/// The loop measures `gap` from one iteration's top to the next, and the
/// previous iteration's [`write_heartbeat`] runs *inside* that span. So a slow
/// filesystem lands in `gap` looking exactly like the thread being descheduled
/// — which is how 2532 log lines came to assert "process likely
/// suspended/throttled" about what an isolation experiment showed to be write
/// latency (`~/.fleet/` writes of 3.2s while a bare-sleep thread in the same
/// process saw zero drift; wiki `desktop/heartbeat-stall-is-io`).
///
/// Subtracting `write_in_gap` leaves scheduling alone, so the two causes get
/// reported separately instead of one impersonating the other.
pub fn scheduling_gap(gap: Duration, write_in_gap: Duration) -> Duration {
    gap.saturating_sub(write_in_gap)
}

/// Which kind of process is claiming to be the consumer.
///
/// Decides one thing: whether a stale timestamp may be rescued by the recorded
/// pid still being alive. See the module docs — sound for [`Desktop`], vacuous
/// for [`Server`].
///
/// [`Desktop`]: WriterKind::Desktop
/// [`Server`]: WriterKind::Server
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriterKind {
    /// The desktop app: the writing process is the UI itself.
    Desktop,
    /// `fleet serve` / `fleet webui` / an ACP connection: a daemon that writes
    /// only while a head (SSE client, phone on the relay, ACP peer) is
    /// attached, and whose own liveness says nothing about whether one is.
    Server,
}

impl WriterKind {
    fn tag(self) -> &'static str {
        match self {
            WriterKind::Desktop => "desktop",
            WriterKind::Server => "server",
        }
    }

    /// Parse the file's third line. Anything unrecognised — including the line
    /// being absent, which is every build older than this one — reads as
    /// `Desktop`, preserving the previous behaviour for those writers.
    fn parse(line: Option<&str>) -> Self {
        match line.map(str::trim) {
            Some("server") => WriterKind::Server,
            _ => WriterKind::Desktop,
        }
    }
}

/// Record the desktop app as the live consumer.
pub fn write_heartbeat() {
    write_heartbeat_as(WriterKind::Desktop);
}

pub fn write_heartbeat_as(kind: WriterKind) {
    let Some(path) = heartbeat_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let pid = std::process::id();
    let _ = atomic_write_string(&path, &format!("{}\n{}\n{}\n", ts_ms, pid, kind.tag()));
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
    /// Timestamp stale and the writer is a daemon (`fleet serve` / `webui` /
    /// ACP). No pid check is attempted: that process only writes while a head
    /// is attached, so a stale timestamp *is* the answer — nothing is watching.
    StaleServerNoHead { age_ms: u128, pid: u32 },
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
            ConsumerStatus::StaleServerNoHead { age_ms, pid } => write!(
                f,
                "stale-server-no-head (age={}ms, pid={}) — fleet serve/webui is running but no UI is attached",
                age_ms, pid
            ),
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
    // …but only for the desktop app, where that process *is* the UI. A daemon
    // writes this file only while a head is attached, so a stale timestamp
    // already means "no head attached" and its own liveness is not evidence of
    // anything. Borrowing the fallback there made the check vacuous: `systemd`
    // keeps `fleet webui` alive forever, so every `fleet__ask` blocked its
    // agent for the full wait on a card nobody could see.
    if WriterKind::parse(lines.next()) == WriterKind::Server {
        return ConsumerStatus::StaleServerNoHead { age_ms, pid };
    }
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
pub(crate) fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: kill(pid, 0) is the standard liveness probe on Unix —
    // returns 0 if the process exists (and we have permission to signal
    // it), -1/ESRCH if it does not. No signal is actually delivered.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(windows)]
pub(crate) fn process_alive(pid: u32) -> bool {
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

    /// The bug this branch exists for. A daemon's stale heartbeat used to be
    /// rescued by its own pid — and `systemd`'s `Restart=always` means that pid
    /// is always alive, so `consumer_status` answered `Alive` on a box with no
    /// UI attached at all. `fleet__ask` then wrote a card and blocked its agent
    /// for the full 600s wait with nobody able to see it (twice on 2026-09-08).
    #[test]
    fn stale_server_heartbeat_is_not_rescued_by_its_own_live_pid() {
        let now: u128 = 1_000_000_000;
        let our_pid = std::process::id();
        let content = format!("{}\n{}\nserver\n", now - 60_000, our_pid);
        let s = classify(&content, now, STALE_AFTER_MS);
        assert!(!s.is_alive(), "a daemon with no head attached must not read as alive: {s}");
        assert!(
            matches!(s, ConsumerStatus::StaleServerNoHead { pid, age_ms } if pid == our_pid && age_ms == 60_000),
            "got {s}",
        );
    }

    /// …while the desktop keeps the fallback, which is the case it was written
    /// for: the writing process *is* the UI, so a frozen (power-napped) app is
    /// still a reachable head.
    #[test]
    fn stale_desktop_heartbeat_still_gets_the_pid_fallback() {
        let now: u128 = 1_000_000_000;
        let our_pid = std::process::id();
        let content = format!("{}\n{}\ndesktop\n", now - 60_000, our_pid);
        let s = classify(&content, now, STALE_AFTER_MS);
        assert!(s.is_alive(), "got {s}");
    }

    /// A heartbeat written by a build that predates the kind line has two lines
    /// and must keep reading as the desktop — the writer it in fact was.
    #[test]
    fn stale_heartbeat_without_a_kind_line_is_treated_as_desktop() {
        let now: u128 = 1_000_000_000;
        let our_pid = std::process::id();
        let content = format!("{}\n{}\n", now - 60_000, our_pid);
        assert!(classify(&content, now, STALE_AFTER_MS).is_alive());
    }

    /// A fresh heartbeat is alive whoever wrote it: the kind only gates the
    /// stale-path fallback.
    #[test]
    fn fresh_server_heartbeat_is_alive() {
        let now: u128 = 1_000_000_000;
        let content = format!("{}\n{}\nserver\n", now - 1_000, std::process::id());
        let s = classify(&content, now, STALE_AFTER_MS);
        assert!(matches!(s, ConsumerStatus::Alive { fresh: true, .. }), "got {s}");
    }

    /// The writer and the reader have to agree on the tag, and a typo in either
    /// literal silently reverts the daemon to the desktop's fallback — the exact
    /// bug, back with no test failing.
    #[test]
    fn the_kind_the_server_writes_is_the_kind_the_reader_recognises() {
        let now: u128 = 1_000_000_000;
        let content = format!("{}\n{}\n{}\n", now - 60_000, std::process::id(), WriterKind::Server.tag());
        assert!(matches!(
            classify(&content, now, STALE_AFTER_MS),
            ConsumerStatus::StaleServerNoHead { .. }
        ));
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

    /// The measured case from the 2026-09-06 isolation run: a `write_heartbeat`
    /// that itself took 3.224s produced a 3.730s loop gap. The thread was never
    /// starved — a bare-sleep thread in the same process saw zero drift in the
    /// same window — so this must NOT be reported as a stall.
    #[test]
    fn a_slow_write_is_not_a_scheduling_stall() {
        let gap = Duration::from_millis(3730);
        let write = Duration::from_millis(3224);

        let sched = scheduling_gap(gap, write);

        assert!(
            sched < STALL_WARN,
            "a 3.224s write inside a 3.730s gap left {sched:?} of scheduling latency, \
             which was reported as a stall — the write is impersonating starvation",
        );
    }

    /// The symmetric guard: with the write subtracted out, a genuinely starved
    /// iteration must still be reported. Otherwise the fix would simply blind
    /// the watchdog to the thing it was built for.
    #[test]
    fn real_starvation_still_reports() {
        let gap = Duration::from_millis(3000);
        let write = Duration::from_millis(4);

        let sched = scheduling_gap(gap, write);

        assert!(
            sched >= STALL_WARN,
            "a 3s gap containing only a 4ms write is real starvation, but {sched:?} \
             fell under the warn threshold",
        );
    }

    /// A write slower than the whole gap (clock skew, or a write that outran the
    /// next tick) must clamp to zero rather than wrap around.
    #[test]
    fn write_longer_than_gap_clamps_to_zero() {
        let sched = scheduling_gap(Duration::from_millis(500), Duration::from_millis(900));
        assert_eq!(sched, Duration::ZERO);
    }
}
