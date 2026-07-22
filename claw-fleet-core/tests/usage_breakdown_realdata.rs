//! Real-data smoke test for the cost-breakdown ("花费明细") cache.
//!
//! Ignored by default — it reads THIS machine's `~/.claude` transcripts and
//! writes the real `~/.fleet/usage-breakdown-cache.json`, so it runs only when
//! invoked explicitly:
//!
//! ```text
//! cargo test -p claw-fleet-core --test usage_breakdown_realdata -- --ignored --nocapture
//! ```
//!
//! It drives the REAL code path on REAL sessions and verifies the cache makes a
//! repeat receipt dramatically cheaper while the folded totals stay identical.

use std::time::Instant;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[test]
#[ignore = "reads real ~/.claude transcripts; run with --ignored on demand"]
fn real_range_breakdown_is_fast_on_second_call() {
    let home = claw_fleet_core::session::real_home_dir().expect("home dir");
    let claude_dir = home.join(".claude");
    let scan_cache = claw_fleet_core::session::ScanCache::new();
    let sessions = claw_fleet_core::session::scan_sessions(&claude_dir, &scan_cache);
    eprintln!("scanned {} real sessions", sessions.len());
    assert!(!sessions.is_empty(), "no real sessions found to exercise");

    // Last 7 local days, matching the modal's "近 7 天" preset (to = now).
    let now = now_ms();
    let from = now - 6 * 86_400_000;

    let t0 = Instant::now();
    let cold = claw_fleet_core::today_usage::usage_range_breakdown(&sessions, from, now);
    let cold_ms = t0.elapsed().as_millis();

    let t1 = Instant::now();
    let warm = claw_fleet_core::today_usage::usage_range_breakdown(&sessions, from, now);
    let warm_ms = t1.elapsed().as_millis();

    let codex_lines = cold.lines.iter().filter(|l| l.source == "codex").count();
    eprintln!(
        "7d receipt: cold {cold_ms}ms, warm {warm_ms}ms | total ${:.2} · {} lines ({} codex) · {} trend days",
        cold.total_cost_usd,
        cold.lines.len(),
        codex_lines,
        cold.daily.len()
    );

    // Same inputs → identical aggregate across calls (cache must not drift).
    assert!(
        (cold.total_cost_usd - warm.total_cost_usd).abs() < 1e-6,
        "cold ${} vs warm ${} diverged",
        cold.total_cost_usd,
        warm.total_cost_usd
    );
    assert_eq!(cold.lines.len(), warm.lines.len());
    assert_eq!(cold.daily.len(), warm.daily.len());

    // The whole point: the warm call skips re-parsing every transcript. Assert an
    // order-of-magnitude win (with slack for a trivially small dataset where both
    // calls are already sub-20ms and the ratio is noise).
    if cold_ms > 20 {
        assert!(
            warm_ms.saturating_mul(5) < cold_ms,
            "warm ({warm_ms}ms) not much faster than cold ({cold_ms}ms) — cache not helping"
        );
    }
}
