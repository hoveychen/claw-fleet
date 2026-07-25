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

/// **The receipt invariant, on real data.** Every row the UI renders is
/// `tokens × unit price`, so their sum must equal the line's own subtotal — for
/// every preset, including the 30d/All windows that backfill from the daily
/// report DB and the "fleet" lines that come from `fleet_llm_usage.jsonl`.
///
/// This is the check that would have caught the reported bug directly: opus-4-8
/// over 30 days itemised to $92066.72 under a $17609.63 subtotal, because the
/// report backfill fed the DB's all-inclusive `inputTokens` into the Input row.
#[test]
#[ignore = "reads real ~/.claude transcripts + ~/.fleet report DB; run with --ignored"]
fn real_receipt_rows_reconcile_to_their_subtotals() {
    let home = claw_fleet_core::session::real_home_dir().expect("home dir");
    let scan_cache = claw_fleet_core::session::ScanCache::new();
    let sessions = claw_fleet_core::session::scan_sessions(&home.join(".claude"), &scan_cache);
    let now = now_ms();

    // `strict` = the window is served purely by the live per-turn fold, where the
    // invariant must hold to the cent. The longer windows backfill days older
    // than the live pool from the daily-report DB, whose per-model rows are a
    // coarser projection: it books a whole session's tokens under ONE "effective
    // model" while its stored cost was summed per turn with each turn's own
    // model, so a session that switched models mid-flight can't itemise exactly
    // (and the transcripts needed to redo it are long gone). Observed residue on
    // real data: ≤0.7%. The bug this guards against was 420%.
    for (label, from, strict) in [
        ("today", now - (now % 86_400_000), true),
        ("7d", now - 6 * 86_400_000, true),
        ("30d", now - 29 * 86_400_000, false),
        ("all", 0, false),
    ] {
        let b = claw_fleet_core::today_usage::usage_range_breakdown(&sessions, from, now);
        let mut worst = 0.0f64;
        for l in &b.lines {
            let per_m = |tok: u64, price: f64| (tok as f64 / 1_000_000.0) * price;
            let rows = per_m(l.input_tokens, l.input_price)
                + per_m(l.cache_creation_tokens, l.cache_write_price)
                + per_m(l.cache_creation_1h_tokens, l.cache_write_1h_price)
                + per_m(l.cache_read_tokens, l.cache_read_price)
                + per_m(l.output_tokens, l.output_price);
            let drift = (rows - l.cost_usd).abs();
            // Fleet's own lines logged BEFORE TTL-aware accounting cannot
            // reconcile and never will: those entries recorded the CLI's
            // `usage.input_tokens` (last iteration only — 10 where the real
            // figure was 530) and no cache-write TTL split, while their
            // `cost_usd` is the CLI's true, fully-billed number. Keeping the
            // accurate cost and under-itemising is the honest trade; entries
            // written from this commit on carry both and do reconcile, so the
            // residue ages out of the window on its own. Only the transcript
            // -derived lines are held to the strict invariant.
            // A Codex-provider Fleet call is logged with `costAccurate: false`
            // and `cost_usd: 0.0` (no per-token price for a ChatGPT-plan quota),
            // so its estimated-token rows sit under a $0.00 subtotal — another
            // reason a fleet line can't be held to the invariant. Fleet lines are
            // therefore reported, not asserted.
            // `<synthetic>` is Claude Code's marker for injected control/error
            // turns (403 notices, "No response requested"), not a model — it has
            // no published price and lands on the unknown-model fallback, so its
            // rows are meaningless by construction. Reported, not asserted.
            if l.source == "fleet" || l.model == "<synthetic>" {
                worst = worst.max(drift);
                continue;
            }
            // Report-backfilled windows get a $1 absolute floor on top of the
            // 5%: the report DB's one-model-per-session attribution skews hardest
            // on cent-scale lines (a $0.51 line drifting 6c is 13% but is not a
            // scaling bug), while a mis-scaled row on a real line is thousands.
            let tolerance = if strict {
                (l.cost_usd.abs() * 0.005).max(0.01)
            } else {
                (l.cost_usd.abs() * 0.05).max(1.00)
            };
            eprintln!(
                "{label:>5} {:<34} {:<11} rows ${rows:>12.2} vs subtotal ${:>12.2}  ({:+.2}%)",
                l.model,
                l.source,
                l.cost_usd,
                if l.cost_usd != 0.0 { drift / l.cost_usd * 100.0 } else { 0.0 },
            );
            // Historical days keep whatever cost their report stored, so a small
            // residue is expected there; a mis-scaled row shows up as multiples.
            assert!(
                drift <= tolerance,
                "{label} line {} ({}) itemises to ${rows:.2} under a ${:.2} subtotal",
                l.model,
                l.source,
                l.cost_usd,
            );
            worst = worst.max(drift);
        }
        eprintln!("{label:>5} — {} lines, worst drift ${worst:.4}\n", b.lines.len());
    }
}
