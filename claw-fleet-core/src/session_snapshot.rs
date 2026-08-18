//! The session list, kept off the request path.
//!
//! `scan_all_sources` asks every agent source for its sessions, so it is only as
//! fast as the slowest one — a dsh answering `session.list` in seconds makes the
//! whole scan take seconds. A route that scans on the request therefore pays
//! that cost on nearly every frontend poll: dsh's own single-flight bounds it to
//! one call per `ROSTER_TTL` (2s), but the frontend polls slower than that, so
//! almost every poll lands outside the window and waits for the RPC.
//!
//! This is the same shape the desktop already uses: `LocalBackend::list_sessions`
//! clones an `Arc<Mutex<Vec<SessionInfo>>>` that its watcher and poll threads
//! fill, so a Tauri command never scans. `fleet serve` gets the equivalent here.
//!
//! ## The staleness contract
//!
//! A read never blocks on a scan once the snapshot exists — that wait is the
//! very thing being removed. So a reader can be handed a list up to one refresh
//! interval old, and, if the process sat idle long enough for the ticker to
//! stand down, older than that for exactly one read (which restarts the ticker).
//! That is the same bargain the desktop makes: its `sessions` mutex holds
//! whatever the last scan produced, and its fs watcher is equally idle while
//! nothing writes.
//!
//! The one read that *does* block is the first: an empty snapshot is not "no
//! sessions", it is "not scanned yet", and answering that with `[]` would show a
//! remote user an empty roster on connect. Concurrent first reads collapse onto
//! one scan rather than each starting their own.
//!
//! Freshness-critical callers — the `/v1` API projecting a response for a
//! session that may have been created milliseconds ago — must keep calling
//! `scan_all_sources` directly. A 404 from a two-second-old snapshot would be
//! wrong, not merely late.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::agent_source::AgentSource;
use crate::session::{scan_all_sources, SessionInfo};

/// How often the ticker rescans while anyone is reading.
///
/// Matches the SSE broadcaster's own 2s cadence, so the snapshot is never the
/// stale-est thing a client sees.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// How long after the last read the ticker keeps scanning.
///
/// A `fleet serve` with nobody attached should not poll every agent source
/// forever — on a probe host that means an RPC to every tool every 2s for
/// nothing. The next read restarts the ticker.
pub const IDLE_AFTER: Duration = Duration::from_secs(60);

struct Snap {
    sessions: Vec<SessionInfo>,
    at: Instant,
}

/// A shared, background-refreshed session list.
pub struct SessionSnapshot {
    sources: Arc<Vec<Box<dyn AgentSource>>>,
    current: Mutex<Option<Snap>>,
    /// Held for the duration of a scan, so concurrent refreshes collapse.
    scanning: Mutex<()>,
    /// Whether an async refresh is already queued, so a burst of reads spawns
    /// one refresher rather than one per read.
    refresh_queued: AtomicBool,
    last_read: Mutex<Option<Instant>>,
}

impl SessionSnapshot {
    pub fn new(sources: Arc<Vec<Box<dyn AgentSource>>>) -> Arc<Self> {
        Arc::new(Self {
            sources,
            current: Mutex::new(None),
            scanning: Mutex::new(()),
            refresh_queued: AtomicBool::new(false),
            last_read: Mutex::new(None),
        })
    }

    /// The current session list.
    ///
    /// Blocks only when nothing has been scanned yet (see the module note);
    /// otherwise returns the last scan and kicks a background refresh when it
    /// has aged past [`REFRESH_INTERVAL`].
    pub fn sessions(self: &Arc<Self>) -> Vec<SessionInfo> {
        *self.last_read.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());

        let cached = {
            let cur = self.current.lock().unwrap_or_else(|e| e.into_inner());
            cur.as_ref()
                .map(|snap| (snap.sessions.clone(), snap.at.elapsed()))
        };
        match cached {
            Some((sessions, age)) => {
                if age >= REFRESH_INTERVAL {
                    self.refresh_in_background();
                }
                sessions
            }
            // Cold: the caller waits, because `[]` here would be a lie.
            None => self.refresh(),
        }
    }

    /// Rescan now and store the result. Concurrent callers share one scan.
    pub fn refresh(&self) -> Vec<SessionInfo> {
        let _one_at_a_time = self.scanning.lock().unwrap_or_else(|e| e.into_inner());
        // Someone may have finished a scan while this thread queued on the lock;
        // that result is new enough to be this call's answer too.
        {
            let cur = self.current.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(snap) = cur.as_ref() {
                if snap.at.elapsed() < REFRESH_INTERVAL {
                    return snap.sessions.clone();
                }
            }
        }

        let sessions = scan_all_sources(&self.sources);
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = Some(Snap {
            sessions: sessions.clone(),
            at: Instant::now(),
        });
        sessions
    }

    /// Mark the stored scan as out of date and rescan behind the caller.
    ///
    /// For routes that add to the session set — a spawn, a resume — where
    /// nothing else would surface the new session before the next tick. The
    /// kill/interrupt routes deliberately do *not* call this: the status change
    /// is written by the process being stopped, after it has been signalled, so
    /// an immediate rescan would only re-read the pre-change state and the
    /// ticker is what actually surfaces it. (The desktop makes the same
    /// distinction — see the "No rescan kick here" note on
    /// `LocalBackend::interrupt_agent_session`.)
    ///
    /// Scanning on the request would put the slow path back into the
    /// very handler that was fast before, and simply waiting for the ticker
    /// would make a just-spawned session take up to [`REFRESH_INTERVAL`] to
    /// appear, which is a regression against the scan-per-request behaviour this
    /// replaced. So the timestamp is backdated (both so the next read sees the
    /// snapshot as aged and so the queued refresh cannot short-circuit on
    /// freshness) and the rescan runs on a thread.
    pub fn invalidate(self: &Arc<Self>) {
        {
            let mut cur = self.current.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(snap) = cur.as_mut() {
                // Monotonic clocks start at boot, so this can underflow in the
                // first seconds of uptime; leaving the stamp alone then is fine
                // — a snapshot that young is about to be refreshed anyway.
                if let Some(backdated) = snap.at.checked_sub(REFRESH_INTERVAL) {
                    snap.at = backdated;
                }
            }
        }
        self.refresh_in_background();
    }

    /// Refresh on a background thread, at most one queued at a time.
    ///
    /// Used both by an aged read and by [`Self::invalidate`].
    pub fn refresh_in_background(self: &Arc<Self>) {
        if self.refresh_queued.swap(true, Ordering::AcqRel) {
            return;
        }
        let this = self.clone();
        std::thread::spawn(move || {
            this.refresh();
            this.refresh_queued.store(false, Ordering::Release);
        });
    }

    /// Keep the snapshot warm while anyone is reading it. Runs for the life of
    /// the process.
    pub fn start_ticker(self: &Arc<Self>) {
        let this = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(REFRESH_INTERVAL);
            let idle = this
                .last_read
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .map(|t| t.elapsed() >= IDLE_AFTER)
                .unwrap_or(true);
            if !idle {
                this.refresh();
            }
        });
    }

    /// Age of the stored scan, `None` if nothing has been scanned yet.
    pub fn age(&self) -> Option<Duration> {
        self.current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|snap| snap.at.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_source::WatchStrategy;
    use std::sync::atomic::AtomicUsize;

    /// A source whose scan is slow and counted — the two properties every claim
    /// here is about.
    struct SlowSource {
        delay: Duration,
        scans: Arc<AtomicUsize>,
    }

    impl AgentSource for SlowSource {
        fn name(&self) -> &'static str {
            "slow"
        }
        fn uri_prefix(&self) -> &'static str {
            "slow://"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn scan_sessions(&self) -> Vec<SessionInfo> {
            self.scans.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            vec![session("slow://one")]
        }
        fn get_messages(&self, _path: &str) -> Result<Vec<serde_json::Value>, String> {
            Ok(Vec::new())
        }
        fn watch_strategy(&self) -> WatchStrategy {
            WatchStrategy::Poll(Duration::from_secs(5))
        }
    }

    fn session(path: &str) -> SessionInfo {
        let mut s = SessionInfo::default();
        s.id = path.to_string();
        s.jsonl_path = path.to_string();
        s
    }

    fn snapshot(delay: Duration) -> (Arc<SessionSnapshot>, Arc<AtomicUsize>) {
        let scans = Arc::new(AtomicUsize::new(0));
        let sources: Vec<Box<dyn AgentSource>> = vec![Box::new(SlowSource {
            delay,
            scans: scans.clone(),
        })];
        (SessionSnapshot::new(Arc::new(sources)), scans)
    }

    #[test]
    fn the_first_read_waits_and_gets_the_sessions() {
        let (snap, scans) = snapshot(Duration::from_millis(200));
        let started = Instant::now();
        let sessions = snap.sessions();
        assert_eq!(sessions.len(), 1, "the cold read must return real data");
        assert!(
            started.elapsed() >= Duration::from_millis(200),
            "the cold read must have waited for the scan, not answered empty"
        );
        assert_eq!(scans.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_warm_read_costs_no_scan() {
        let (snap, scans) = snapshot(Duration::from_millis(200));
        let _ = snap.sessions();
        assert_eq!(scans.load(Ordering::SeqCst), 1);

        for _ in 0..5 {
            let started = Instant::now();
            let sessions = snap.sessions();
            assert_eq!(sessions.len(), 1);
            assert!(
                started.elapsed() < Duration::from_millis(100),
                "a warm read waited {:?}; it must not scan",
                started.elapsed()
            );
        }
        assert_eq!(
            scans.load(Ordering::SeqCst),
            1,
            "reads inside the refresh interval must not rescan"
        );
    }

    #[test]
    fn concurrent_first_reads_collapse_onto_one_scan() {
        let (snap, scans) = snapshot(Duration::from_millis(300));
        let gate = Arc::new(std::sync::Barrier::new(6));
        let readers: Vec<_> = (0..6)
            .map(|_| {
                let snap = snap.clone();
                let gate = gate.clone();
                std::thread::spawn(move || {
                    gate.wait();
                    snap.sessions()
                })
            })
            .collect();
        let results: Vec<_> = readers
            .into_iter()
            .map(|h| h.join().expect("reader thread"))
            .collect();

        assert!(
            results.iter().all(|r| r.len() == 1),
            "every reader must get the sessions, not an empty list: {:?}",
            results.iter().map(Vec::len).collect::<Vec<_>>()
        );
        assert_eq!(
            scans.load(Ordering::SeqCst),
            1,
            "6 simultaneous cold reads must share one scan"
        );
    }

    #[test]
    fn an_aged_read_returns_immediately_and_refreshes_behind_it() {
        let (snap, scans) = snapshot(Duration::from_millis(400));
        let _ = snap.sessions();
        assert_eq!(scans.load(Ordering::SeqCst), 1);

        // Let the snapshot age past the refresh interval.
        std::thread::sleep(REFRESH_INTERVAL + Duration::from_millis(100));

        let started = Instant::now();
        let sessions = snap.sessions();
        assert_eq!(sessions.len(), 1, "the aged read still answers with data");
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "an aged read waited {:?}; staleness must be repaired behind the \
             reader, not in front of it",
            started.elapsed()
        );

        // The refresh it kicked lands shortly after, without another read.
        let deadline = Instant::now() + Duration::from_secs(3);
        while scans.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            scans.load(Ordering::SeqCst),
            2,
            "the aged read must have kicked a background refresh"
        );
    }

    /// A spawn/stop route must not have to wait a refresh interval for its own
    /// change to show up.
    #[test]
    fn invalidate_rescans_even_when_the_snapshot_is_fresh() {
        let (snap, scans) = snapshot(Duration::from_millis(50));
        let _ = snap.sessions();
        assert_eq!(scans.load(Ordering::SeqCst), 1);

        // Fresh snapshot: a plain background refresh would short-circuit here.
        snap.refresh_in_background();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            scans.load(Ordering::SeqCst),
            1,
            "a refresh inside the interval must reuse the scan"
        );

        snap.invalidate();
        let deadline = Instant::now() + Duration::from_secs(3);
        while scans.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(
            scans.load(Ordering::SeqCst),
            2,
            "invalidate must rescan despite the snapshot being fresh"
        );
    }

    #[test]
    fn the_ticker_keeps_scanning_while_read_and_stands_down_when_idle() {
        let (snap, scans) = snapshot(Duration::from_millis(10));
        snap.start_ticker();

        // No read yet — the ticker must not scan on its own.
        std::thread::sleep(REFRESH_INTERVAL * 2 + Duration::from_millis(200));
        assert_eq!(
            scans.load(Ordering::SeqCst),
            0,
            "the ticker must stay quiet until someone reads the snapshot"
        );

        // One read arms it; the ticker then refreshes on its own cadence.
        let _ = snap.sessions();
        let after_read = scans.load(Ordering::SeqCst);
        std::thread::sleep(REFRESH_INTERVAL * 2 + Duration::from_millis(400));
        assert!(
            scans.load(Ordering::SeqCst) > after_read,
            "the ticker must refresh after a read armed it: {} scans",
            scans.load(Ordering::SeqCst)
        );
    }
}
