//! An interactive dsh read must not queue behind the background roster poll.
//!
//! `DshSource::with_client` owns one process-global mutex guarding the shared
//! `dsh web` handle. If that lock is held for the whole RPC — and not just for
//! the start/restart it exists to protect — then every dsh call in the process
//! is serialized: the `session.history` fired when the user opens a session's
//! 对话 tab waits out however many `session.list` polls (`WatchStrategy::Poll(3s)`,
//! issued from several call sites at once) happen to hold or barge the lock.
//!
//! The dsh side here is a fixture server (`tests/fixtures/fake-dsh.js`) rather
//! than a real `dsh web`: it answers `session.list` slowly and `session.history`
//! quickly, and — being an ordinary concurrent HTTP server — will answer both at
//! once. So any wait the reader observes is Fleet-side by construction, which is
//! what makes this an isolation test rather than a benchmark.
//!
//! Measured against the pre-fix code (`--ignored` measurement below): baseline
//! 54ms, one poller 55–3025ms, three pollers 5354–8555ms — i.e. the reader was
//! made to wait out two to three whole polls, each far longer than its own 50ms
//! call.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use claw_fleet_core::agent_source::AgentSource;
use claw_fleet_core::dsh_source::DshSource;

/// How many background pollers to run against the lock. Three is what the real
/// process has in flight: the registry watch loop, the desktop's rescan, and a
/// `fleet serve` route that scans on request.
const POLLERS: usize = 3;

/// The fixture's `session.list` latency — the "slow background call".
const LIST_DELAY_MS: u64 = 1500;
/// The fixture's `session.history` latency — what an uncontended interactive
/// read actually costs.
const HISTORY_DELAY_MS: u64 = 50;

/// The ceiling an interactive read must stay under while the pollers run.
///
/// Generously above the 50ms the call itself costs (CI is noisy) and far below
/// the 1500ms a *single* queued poll would add, so the assertion can only fail
/// by queueing, not by the machine being slow.
const READ_BUDGET: Duration = Duration::from_millis(800);

/// Both tests drive the same process-global `dsh web` and the same environment,
/// so they may never overlap.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-dsh.js")
}

/// Point Fleet at the fixture server and at a throwaway `~/.fleet`, so the
/// server-ownership registry this writes never touches the real one.
///
/// Returns `None` when the fixture cannot run here (it is a `node` script), so a
/// machine without node skips rather than fails.
fn arrange(list_delay_ms: u64, log: Option<&std::path::Path>) -> Option<tempfile::TempDir> {
    claw_fleet_core::process_util::which("node")?;
    let fleet_home = tempfile::tempdir().expect("temp fleet home");
    std::env::set_var("FLEET_HOME", fleet_home.path());
    std::env::set_var("FLEET_DSH_BIN", fixture());
    std::env::set_var("FAKE_DSH_LIST_DELAY_MS", list_delay_ms.to_string());
    std::env::set_var("FAKE_DSH_HISTORY_DELAY_MS", HISTORY_DELAY_MS.to_string());
    match log {
        Some(path) => std::env::set_var("FAKE_DSH_LOG", path),
        None => std::env::remove_var("FAKE_DSH_LOG"),
    }
    Some(fleet_home)
}

/// One interactive read, timed end to end.
fn timed_read(source: &DshSource) -> Duration {
    let started = Instant::now();
    let _ = source.get_messages_tail("dsh://session-probe", 50);
    started.elapsed()
}

/// Run `body` while `POLLERS` threads poll the session roster on Fleet's real
/// cadence, then stop them.
fn under_poll_load<T>(pollers: usize, body: impl FnOnce() -> T) -> T {
    let stop = Arc::new(AtomicBool::new(false));
    let handles: Vec<_> = (0..pollers)
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let poller = DshSource::new();
                while !stop.load(Ordering::Relaxed) {
                    poller.scan_sessions();
                    std::thread::sleep(Duration::from_secs(3));
                }
            })
        })
        .collect();
    // Let the pollers get into their rhythm before the measurement starts.
    std::thread::sleep(Duration::from_millis(400));

    let out = body();

    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }
    out
}

#[test]
fn an_interactive_read_does_not_queue_behind_the_roster_poll() {
    let _serial = serial();
    let Some(_fleet_home) = arrange(LIST_DELAY_MS, None) else {
        eprintln!("skipped: node not on PATH, the dsh fixture cannot run");
        return;
    };

    let source = DshSource::new();
    // The first RPC boots the fixture server; keep that out of the numbers.
    let _ = source.scan_sessions();

    let samples = under_poll_load(POLLERS, || {
        (0..5)
            .map(|_| {
                let d = timed_read(&source);
                std::thread::sleep(Duration::from_millis(300));
                d
            })
            .collect::<Vec<_>>()
    });

    claw_fleet_core::dsh_source::shutdown();

    let worst = samples.iter().copied().max().unwrap_or_default();
    assert!(
        worst <= READ_BUDGET,
        "an interactive read waited {worst:?} while {POLLERS} roster polls were running \
         (its own call costs {HISTORY_DELAY_MS}ms and the fixture answers concurrently, \
         so this is Fleet-side queueing); budget {READ_BUDGET:?}, samples: {samples:?}"
    );
}

fn summarize(label: &str, samples: &[Duration]) {
    let ms: Vec<u128> = samples.iter().map(Duration::as_millis).collect();
    let max = ms.iter().copied().max().unwrap_or(0);
    let mean = ms.iter().sum::<u128>() / ms.len().max(1) as u128;
    println!(
        "{label}: n={} mean={mean}ms max={max}ms samples={ms:?}",
        ms.len()
    );
}

/// The measurement the assertion above was derived from: the same setup, but it
/// reports latencies at 0 / 1 / 3 pollers instead of asserting one budget.
#[test]
#[ignore = "timing measurement; run manually with --ignored --nocapture"]
fn measure_interactive_read_latency_under_scan_load() {
    let _serial = serial();
    let log = std::env::temp_dir().join("fake-dsh-requests.log");
    let _ = std::fs::remove_file(&log);
    let Some(_fleet_home) = arrange(3000, Some(&log)) else {
        eprintln!("skipped: node not on PATH, the dsh fixture cannot run");
        return;
    };

    let source = DshSource::new();
    let _ = source.scan_sessions();
    println!("fixture server port: {:?}", source.server_port());

    let baseline: Vec<Duration> = (0..5).map(|_| timed_read(&source)).collect();
    summarize("baseline (no poller)", &baseline);

    for pollers in [1usize, 3] {
        let samples = under_poll_load(pollers, || {
            (0..5)
                .map(|_| {
                    let d = timed_read(&source);
                    std::thread::sleep(Duration::from_millis(700));
                    d
                })
                .collect::<Vec<_>>()
        });
        summarize(&format!("{pollers} poller(s)"), &samples);
    }

    claw_fleet_core::dsh_source::shutdown();
    println!("request log: {}", log.display());
}
