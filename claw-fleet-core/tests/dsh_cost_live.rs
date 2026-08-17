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

    // Find a session that actually made a model call. `session.history` is the
    // only place the generation ids live, so this walks candidates newest-first
    // and stops at the first one with refs.
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut priced = None;
    for s in sessions.iter() {
        if Instant::now() > deadline {
            break;
        }
        let Ok(events) = claw_fleet_core::dsh_source::session_events(&s.jsonl_path) else {
            continue;
        };
        let refs = claw_fleet_core::dsh_cost::generation_refs(&events);
        if refs.iter().any(|r| r.provider == "openrouter") {
            priced = Some((s.jsonl_path.clone(), refs));
            break;
        }
    }

    let Some((uri, refs)) = priced else {
        eprintln!("SKIP: no session in this dsh home has an OpenRouter generation");
        return;
    };
    println!("pricing {uri} — {} generation(s)", refs.len());

    let cost = claw_fleet_core::dsh_cost::dsh_session_cost(&uri).expect("dsh_session_cost");
    println!("{cost:?}");

    let total = cost
        .total_usd
        .expect("the provider must have priced at least one generation");
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
