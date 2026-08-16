//! Live validation that [`DshSource`] really lists and reads dsh sessions
//! through a server it starts itself.
//!
//! Ignored by default because it starts a real `dsh web`. Run:
//!   FLEET_DSH_BIN=$(ls ~/.npm/_npx/*/node_modules/.bin/dsh | head -1) \
//!   cargo test -p claw-fleet-core --test dsh_source_live -- --ignored --nocapture
//!
//! The unit tests beside `dsh_source.rs` map a recorded wire payload; this one
//! proves the payload still looks like that, that the source starts its own
//! server without help, and that it hands back the durable events of a session
//! written by a *different* dsh profile.

use claw_fleet_core::agent_source::AgentSource;
use claw_fleet_core::dsh_source::DshSource;

/// Stops Fleet's process-global `dsh web` when the test that started it ends.
///
/// The server outlives every `DshSource` on purpose (see `dsh_source::SERVER`),
/// so a test binary that starts one and exits leaves it running — measured: one
/// orphaned `dsh web` after a run of this file. `dsh web` has no authentication
/// layer, so an orphan is an open door onto every session on the machine.
struct ServerGuard;

impl Drop for ServerGuard {
    fn drop(&mut self) {
        claw_fleet_core::dsh_source::shutdown();
    }
}

#[test]
#[ignore = "starts a real `dsh web`; run manually with --ignored"]
fn live_scan_lists_sessions_with_a_self_started_server() {
    let _guard = ServerGuard;
    let source = DshSource::new();
    assert!(
        source.is_available(),
        "set FLEET_DSH_BIN to a dsh executable"
    );

    let sessions = source.scan_sessions();
    println!("scanned {} dsh session(s)", sessions.len());
    for s in sessions.iter().take(5) {
        println!(
            "  {} | ws={} | title={:?} | in={} out={}",
            s.id, s.workspace_path, s.ai_title, s.total_input_tokens, s.total_output_tokens
        );
    }

    assert!(
        !sessions.is_empty(),
        "expected at least one session in ~/.dsh/sessions"
    );
    for s in &sessions {
        assert_eq!(s.agent_source, "dsh");
        assert!(
            s.jsonl_path.starts_with("dsh://"),
            "uri must carry the scheme: {}",
            s.jsonl_path
        );
        assert!(source.owns_path(&s.jsonl_path));
    }
}

#[test]
#[ignore = "starts a real `dsh web`; run manually with --ignored"]
fn live_history_returns_renderable_records() {
    let _guard = ServerGuard;
    let source = DshSource::new();
    assert!(
        source.is_available(),
        "set FLEET_DSH_BIN to a dsh executable"
    );

    // Walk the roster for a session that actually held a conversation: a blank
    // or bookkeeping-only session normalises to nothing, which is correct but
    // proves nothing.
    let sessions = source.scan_sessions();
    let mut checked = 0usize;
    for target in &sessions {
        let records = source.get_messages(&target.jsonl_path).expect("history");
        if records.is_empty() {
            continue;
        }
        checked += 1;
        println!(
            "{} → {} record(s): {:?}",
            target.jsonl_path,
            records.len(),
            records
                .iter()
                .filter_map(|r| r["type"].as_str())
                .collect::<Vec<_>>()
        );

        // The contract every Fleet client reads: Claude Code's vocabulary, not
        // dsh's own. A raw `user/message` / `assistant/chunk` record here means
        // `dsh_messages::normalize` was bypassed and the session detail view
        // would open empty.
        for r in &records {
            let kind = r["type"].as_str().unwrap_or_default();
            assert!(
                matches!(kind, "user" | "assistant"),
                "not a renderable record type: {r}"
            );
            assert!(
                r["message"]["role"].is_string(),
                "record carries no message.role: {r}"
            );
            assert!(
                r["message"]["content"].is_array(),
                "record carries no content array: {r}"
            );
        }

        // Every tool result must name the call it belongs to, or the renderer
        // pairs nothing and every tool card shows an empty result.
        for r in &records {
            for block in r["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "tool_result" {
                    assert!(
                        block["tool_use_id"].as_str().is_some_and(|s| !s.is_empty()),
                        "tool_result with no tool_use_id: {block}"
                    );
                }
            }
        }
        if checked >= 3 {
            break;
        }
    }
    assert!(
        checked > 0,
        "no session in ~/.dsh produced any renderable record — \
         either the harness home is empty or normalisation dropped everything"
    );
}

#[test]
#[ignore = "starts a real `dsh web`; run manually with --ignored"]
fn live_bad_uri_is_rejected_without_touching_the_server() {
    let source = DshSource::new();
    let err = source.get_messages("codex://not-ours").unwrap_err();
    assert!(err.contains("invalid dsh URI"), "{err}");
}

/// The Token tab's data path, end to end against a real server.
///
/// The unit tests beside `dsh_source.rs` map a recorded projections block; this
/// one proves a live server still ships that block, that the four billed buckets
/// really sum to the total, and — the reading the panel exists to keep straight —
/// that context occupancy is a *snapshot* that stays well under the cumulative
/// billed figure on any session that has re-read a cached prefix.
#[test]
#[ignore = "starts a real `dsh web`; run manually with --ignored"]
fn live_token_breakdown_reads_projections_off_a_real_server() {
    let _guard = ServerGuard;
    let source = DshSource::new();
    assert!(
        source.is_available(),
        "set FLEET_DSH_BIN to a dsh executable"
    );

    let sessions = source.scan_sessions();
    assert!(!sessions.is_empty(), "no dsh sessions to read");

    // Pick the busiest session: a blank one would pass every assertion vacuously.
    let busiest = sessions
        .iter()
        .max_by_key(|s| s.total_input_tokens + s.total_output_tokens)
        .expect("at least one");
    assert!(
        busiest.total_input_tokens + busiest.total_output_tokens > 0,
        "every dsh session on this machine is blank — run a turn first"
    );

    let b = claw_fleet_core::dsh_source::dsh_token_breakdown(&busiest.jsonl_path)
        .expect("breakdown");
    println!(
        "{} | billed {} (uncached {} / read {} / write {} / out {}) | ctx {:?}/{:?}",
        busiest.id,
        b.total_tokens,
        b.uncached_input_tokens,
        b.cache_read_tokens,
        b.cache_write_tokens,
        b.output_tokens,
        b.projected_tokens,
        b.context_window
    );

    assert_eq!(
        b.uncached_input_tokens + b.cache_read_tokens + b.cache_write_tokens + b.output_tokens,
        b.total_tokens,
        "rows must sum to the total or the panel's percentages lie"
    );
    // Cross-check against the same numbers as reached through `scan_sessions`,
    // which folds the input buckets together — the two paths must agree.
    assert_eq!(b.output_tokens, busiest.total_output_tokens);
    assert_eq!(
        b.uncached_input_tokens + b.cache_read_tokens + b.cache_write_tokens,
        busiest.total_input_tokens
    );

    let window = b.context_window.expect("a used session knows its window");
    let projected = b.projected_tokens.expect("a used session has pressure");
    assert!(projected > 0 && projected <= window, "{projected}/{window}");
    let pct = b.context_percent.expect("percent");
    assert!(
        (pct - projected as f64 / window as f64).abs() < 1e-9,
        "percent must be derived from the pair it is shown beside"
    );
    // The window holds the live conversation, not the session's whole spend.
    assert!(
        projected <= b.total_tokens,
        "context {projected} exceeds cumulative billed {} — the two readings \
         got conflated somewhere",
        b.total_tokens
    );
}

/// A URI whose id no longer exists must say so, not render an all-zero panel
/// that reads like a brand-new session.
#[test]
#[ignore = "starts a real `dsh web`; run manually with --ignored"]
fn live_token_breakdown_reports_an_unknown_session() {
    let _guard = ServerGuard;
    let err = claw_fleet_core::dsh_source::dsh_token_breakdown("dsh://session-does-not-exist")
        .expect_err("unknown id");
    assert!(err.contains("not found"), "{err}");
}
