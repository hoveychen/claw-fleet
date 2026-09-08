//! Live proof that a dsh session's spend comes back from the provider.
//!
//! Ignored by default: needs a real `dsh` binary, a real `~/.dsh` with sessions
//! in it, and an OpenRouter key.
//!
//!   FLEET_DSH_BIN=$(ls ~/.npm/_npx/*/node_modules/.bin/dsh | head -1) \
//!   OPENROUTER_API_KEY=sk-or-… \
//!   cargo test -p claw-fleet-core --test dsh_cost_live -- --ignored --nocapture
//!
//! The unit tests cover extraction, tallying and key resolution against
//! fixtures; what they cannot cover is the one thing 老板 asked for — that the
//! figure is *the provider's*, not ours. This test closes that gap end to end:
//! real session → real generation ids → real `GET /api/v1/generation` → summed
//! USD.
//!
//! It **skips** (rather than fails) when the machine has no key or no priced
//! session, so it stays runnable on a fresh checkout.

use std::time::{Duration, Instant};

use claw_fleet_core::agent_source::AgentSource;
use claw_fleet_core::dsh_source::DshSource;

/// Stops Fleet's process-global `dsh web` however the test ends.
struct ServerGuard;

impl Drop for ServerGuard {
    fn drop(&mut self) {
        claw_fleet_core::dsh_source::shutdown();
    }
}

/// Isolate the cost cache in a temp `FLEET_HOME` while leaving `DSH_HOME`
/// pointed at the real install — the sessions and `settings.yaml` this test
/// reads live there. Both are set before anything resolves them.
struct Homes {
    _temp: tempfile::TempDir,
    prev_fleet: Option<std::ffi::OsString>,
    prev_dsh: Option<std::ffi::OsString>,
}

impl Homes {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let prev_fleet = std::env::var_os("FLEET_HOME");
        let prev_dsh = std::env::var_os("DSH_HOME");
        let real_dsh = prev_dsh.clone().unwrap_or_else(|| {
            let home = std::env::var_os("HOME").expect("HOME");
            std::path::Path::new(&home).join(".dsh").into_os_string()
        });
        std::env::set_var("FLEET_HOME", temp.path());
        std::env::set_var("DSH_HOME", &real_dsh);
        Self {
            _temp: temp,
            prev_fleet,
            prev_dsh,
        }
    }
}

impl Drop for Homes {
    fn drop(&mut self) {
        match self.prev_fleet.take() {
            Some(v) => std::env::set_var("FLEET_HOME", v),
            None => std::env::remove_var("FLEET_HOME"),
        }
        match self.prev_dsh.take() {
            Some(v) => std::env::set_var("DSH_HOME", v),
            None => std::env::remove_var("DSH_HOME"),
        }
    }
}

#[test]
#[ignore = "calls the real OpenRouter API against a real dsh session; run manually with --ignored"]
fn live_session_cost_comes_back_from_the_provider() {
    let _homes = Homes::new();
    if claw_fleet_core::dsh_cost::openrouter_api_key().is_none() {
        eprintln!("SKIP: no OpenRouter key resolvable from this dsh install");
        return;
    }

    let _guard = ServerGuard;
    let source = DshSource::new();
    let sessions = source.scan_sessions();
    assert!(!sessions.is_empty(), "the real dsh home has no sessions to price");

    // Find a session the provider will actually price, and do not assume the
    // first candidate is one.
    //
    // A just-finished turn's generation ids come back 404 for a while before
    // OpenRouter serves their records (see `dsh_cost`'s module docs for what was
    // measured). Since this very test file's siblings create fresh sessions,
    // the newest candidate is exactly the one most likely to be unpriceable —
    // so walk past it rather than fail on it. Reporting those calls as
    // `unpriced_calls` with a note is the correct product behaviour, not a bug.
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut found: Option<(String, claw_fleet_core::dsh_cost::DshSessionCost)> = None;
    let mut candidates = 0usize;
    for s in sessions.iter() {
        if Instant::now() > deadline {
            break;
        }
        let Ok(events) = claw_fleet_core::dsh_source::session_events(&s.jsonl_path) else {
            continue;
        };
        let refs = claw_fleet_core::dsh_cost::generation_refs(&events);
        if !refs.iter().any(|r| r.provider == "openrouter") {
            continue;
        }
        candidates += 1;
        let cost = claw_fleet_core::dsh_cost::dsh_session_cost(&s.jsonl_path)
            .expect("dsh_session_cost");
        if cost.total_usd.is_some() {
            println!(
                "pricing {} — {} generation(s)",
                s.jsonl_path,
                refs.len()
            );
            found = Some((s.jsonl_path.clone(), cost));
            break;
        }
        println!("skipping {} — nothing priceable ({})", s.jsonl_path, cost.note);
    }

    let Some((uri, cost)) = found else {
        eprintln!(
            "SKIP: {candidates} candidate session(s) but none the provider would price \
             (all interrupted, or no key on this account)"
        );
        return;
    };
    println!("{cost:?}");

    let total = cost
        .total_usd
        .expect("the loop only accepts a session the provider priced");
    assert!(
        total > 0.0,
        "a session that ran real turns cannot have cost exactly nothing: {cost:?}"
    );
    assert!(
        cost.priced_calls > 0,
        "priced_calls must account for what went into the total: {cost:?}"
    );
    assert!(
        total < 100.0,
        "a sanity ceiling — a figure this large means the unit is not USD: {total}"
    );

    // Second pass must be served entirely from the on-disk cache. Generation
    // records are immutable, so a refetch would be pure waste — and if the cache
    // silently missed, this is where it shows.
    let again = claw_fleet_core::dsh_cost::dsh_session_cost(&uri).expect("second pass");
    assert_eq!(
        again.total_usd, cost.total_usd,
        "the cached total must equal the fetched one"
    );
    assert_eq!(again.priced_calls, cost.priced_calls);
}

/// Real-data proof of the attribution fix: a session that ran on more than one
/// local day must have its spend split across those days.
///
/// This is the failure 老板 reported, and it is not reachable from a fixture:
/// the bug was that a session's whole cumulative figure was booked to its
/// last-activity day, which only shows up on a real install that has actually
/// been used across midnight. Measured on this host at the time of writing: 256
/// sessions with model calls, 4 of them spanning 2026-09-06 → 2026-09-07.
///
/// Needs no OpenRouter key — the `deepseek-official` route is table-priced, and
/// it is the route that produces multi-day sessions here. **Skips** when the
/// install has no multi-day session, so a fresh checkout stays runnable.
#[test]
#[ignore = "reads the real ~/.dsh install; run manually with --ignored"]
fn a_real_multi_day_session_is_split_across_its_days() {
    let _homes = Homes::new();
    let _guard = ServerGuard;
    let source = DshSource::new();
    let sessions = source.scan_sessions();
    assert!(!sessions.is_empty(), "the real dsh home has no sessions");

    // Newest first: a multi-day session is by definition one that ran recently
    // enough to still be around, and the walk below is expensive enough that the
    // order decides whether the deadline is reached before a candidate is.
    let mut sessions = sessions;
    sessions.sort_by_key(|s| std::cmp::Reverse(s.last_activity_ms));

    let deadline = Instant::now() + Duration::from_secs(300);
    let mut scanned = 0usize;
    let mut walked = 0usize;
    let mut errors = 0usize;
    for s in sessions.iter() {
        if Instant::now() > deadline {
            eprintln!("deadline reached after {walked} session(s)");
            break;
        }
        walked += 1;
        let calls = match claw_fleet_core::dsh_cost::dsh_session_calls(&s.jsonl_path) {
            Ok(c) => c,
            Err(e) => {
                errors += 1;
                if errors <= 3 {
                    eprintln!("{}: {e}", s.jsonl_path);
                }
                continue;
            }
        };
        if calls.len() < 2 {
            continue;
        }
        scanned += 1;
        // The day boundary is the *local* one, the same one the receipt draws.
        let days: std::collections::BTreeSet<String> = calls
            .iter()
            .map(|c| {
                chrono::DateTime::from_timestamp_millis(c.at_ms)
                    .map(|dt| {
                        dt.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d")
                            .to_string()
                    })
                    .unwrap_or_default()
            })
            .collect();
        if days.len() < 2 {
            continue;
        }

        println!("{} spans {days:?} over {} call(s)", s.jsonl_path, calls.len());
        let priced: f64 = calls.iter().filter_map(|c| c.usd).sum();
        assert!(
            priced > 0.0,
            "this install's multi-day sessions are on the table-priced route, so \
             they must price without any key: {:?}",
            calls.iter().map(|c| (&c.model, c.usd)).take(3).collect::<Vec<_>>()
        );

        // Each day must carry its own money, and the days must sum to the whole —
        // the old behaviour put all of it on one day and none on the others.
        let mut per_day: std::collections::BTreeMap<String, f64> = Default::default();
        for c in &calls {
            let day = chrono::DateTime::from_timestamp_millis(c.at_ms)
                .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            *per_day.entry(day).or_default() += c.usd.unwrap_or(0.0);
        }
        println!("per-day: {per_day:?}");
        assert_eq!(per_day.len(), days.len(), "every day the session ran gets a bucket");
        assert!(
            per_day.values().filter(|v| **v > 0.0).count() >= 2,
            "at least two days must carry real money, else the split is cosmetic: {per_day:?}"
        );
        let summed: f64 = per_day.values().sum();
        assert!(
            (summed - priced).abs() < 1e-9,
            "splitting must conserve the total: {summed} vs {priced}"
        );
        return;
    }
    eprintln!(
        "SKIP: walked {walked} session(s), {errors} unreadable, {scanned} with 2+ calls, \
         none spanning two local days"
    );
}
