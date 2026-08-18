//! Measurement harness for the dsh read-starvation claim.
//!
//! The claim under test: `DshSource::with_client` holds one process-global
//! mutex for the whole duration of every dsh RPC, so an *interactive* read
//! (`session.history`, issued when the user opens a session's 对话 tab) queues
//! behind the *background* roster poll (`session.list`, every 3s) instead of
//! running alongside it. If true, the latency the user sees is queueing, not the
//! cost of their own call.
//!
//! The dsh side is a fixture server (`tests/fixtures/fake-dsh.js`) rather than a
//! real `dsh web`: it answers `session.list` slowly and `session.history`
//! quickly, and — being a normal concurrent HTTP server — will happily answer
//! both at once. So any wait the reader observes here is Fleet-side by
//! construction.
//!
//! Run:
//!   cargo test -p claw-fleet-core --test dsh_read_starvation -- --ignored --nocapture --test-threads=1

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use claw_fleet_core::agent_source::AgentSource;
use claw_fleet_core::dsh_source::DshSource;

/// The fake server's `session.list` latency — long enough to dominate, short
/// enough to keep a measurement run under a minute.
const LIST_DELAY_MS: u64 = 3000;
/// The fake server's `session.history` latency: what an uncontended
/// interactive read costs.
const HISTORY_DELAY_MS: u64 = 50;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-dsh.js")
}

/// Point Fleet at the fixture server and at a throwaway `~/.fleet`, so the
/// server-ownership registry this writes never touches the real one.
fn arrange(log: &std::path::Path) -> tempfile::TempDir {
    let fleet_home = tempfile::tempdir().expect("temp fleet home");
    std::env::set_var("FLEET_HOME", fleet_home.path());
    std::env::set_var("FLEET_DSH_BIN", fixture());
    std::env::set_var("FAKE_DSH_LIST_DELAY_MS", LIST_DELAY_MS.to_string());
    std::env::set_var("FAKE_DSH_HISTORY_DELAY_MS", HISTORY_DELAY_MS.to_string());
    std::env::set_var("FAKE_DSH_LOG", log);
    fleet_home
}

/// One interactive read, timed end to end.
fn timed_read(source: &DshSource) -> Duration {
    let started = Instant::now();
    let _ = source.get_messages_tail("dsh://session-probe", 50);
    started.elapsed()
}

fn summarize(label: &str, samples: &[Duration]) {
    let ms: Vec<u128> = samples.iter().map(Duration::as_millis).collect();
    let max = ms.iter().copied().max().unwrap_or(0);
    let mean = ms.iter().sum::<u128>() / ms.len().max(1) as u128;
    println!("{label}: n={} mean={mean}ms max={max}ms samples={ms:?}", ms.len());
}

#[test]
#[ignore = "timing measurement; run manually with --ignored --test-threads=1"]
fn measure_interactive_read_latency_under_scan_load() {
    let log = std::env::temp_dir().join("fake-dsh-requests.log");
    let _ = std::fs::remove_file(&log);
    let _fleet_home = arrange(&log);

    let source = DshSource::new();

    // Warm start: the first RPC boots the fixture server, so keep it out of the
    // numbers.
    let _ = source.scan_sessions();
    println!("fixture server port: {:?}", source.server_port());

    // ── Baseline: nothing else is talking to dsh ────────────────────────────
    let baseline: Vec<Duration> = (0..5).map(|_| timed_read(&source)).collect();
    summarize("baseline (no scanner)", &baseline);

    // ── Contended: background pollers on the same cadence Fleet uses ────────
    for scanners in [1usize, 3] {
        let stop = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::new();
        for _ in 0..scanners {
            let stop = stop.clone();
            handles.push(std::thread::spawn(move || {
                let scanner = DshSource::new();
                let mut n = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    scanner.scan_sessions();
                    n += 1;
                    // `WatchStrategy::Poll(3s)` — the registry's cadence.
                    std::thread::sleep(Duration::from_secs(3));
                }
                n
            }));
        }
        // Let the pollers get into their rhythm before measuring.
        std::thread::sleep(Duration::from_millis(500));

        let mut samples = Vec::new();
        for _ in 0..5 {
            samples.push(timed_read(&source));
            std::thread::sleep(Duration::from_millis(700));
        }
        stop.store(true, Ordering::Relaxed);
        let scans: u32 = handles.into_iter().map(|h| h.join().unwrap_or(0)).sum();
        summarize(&format!("{scanners} scanner(s) [{scans} scans]"), &samples);
    }

    claw_fleet_core::dsh_source::shutdown();
    println!("request log: {}", log.display());
}
