//! One-off probe (P1 of `today-usage-cold-start`): decompose the 64s cold
//! `today_usage` into (a) the scan, (b) how many sessions miss the persisted
//! projection cache's fingerprint, and (c) what re-folding those costs.
//!
//! Read-only: it never writes `~/.fleet/usage-breakdown-cache.json`.
//!
//! Run: `cargo run -p claw-fleet-core --example usage_fold_probe`

use std::collections::HashMap;
use std::time::Instant;

fn main() {
    let home = std::env::var("HOME").expect("HOME");
    let cache_path = format!("{home}/.fleet/usage-breakdown-cache.json");

    // ── persisted cache: id -> fingerprint ────────────────────────────────
    let raw = std::fs::read(&cache_path).expect("read cache");
    let v: serde_json::Value = serde_json::from_slice(&raw).expect("parse cache");
    println!(
        "cache: version={} tz={} entries={}",
        v["version"],
        v["tz"],
        v["entries"].as_object().map(|o| o.len()).unwrap_or(0)
    );
    let mut persisted: HashMap<String, (u64, u64, u64)> = HashMap::new();
    if let Some(obj) = v["entries"].as_object() {
        for (id, e) in obj {
            let f = &e["fingerprint"];
            persisted.insert(
                id.clone(),
                (
                    f[0].as_u64().unwrap_or(0),
                    f[1].as_u64().unwrap_or(0),
                    f[2].as_u64().unwrap_or(0),
                ),
            );
        }
    }

    // ── fresh scan, same call the desktop makes ───────────────────────────
    let t = Instant::now();
    let sources = claw_fleet_core::agent_source::build_sources();
    let sessions = claw_fleet_core::session::scan_all_sources(&sources);
    println!(
        "scan_all_sources: {:.1}s, {} sessions",
        t.elapsed().as_secs_f64(),
        sessions.len()
    );

    // ── fingerprint hit / miss ────────────────────────────────────────────
    let mut hit = 0usize;
    let mut miss_absent = 0usize;
    let mut miss_changed = 0usize;
    let mut miss_bytes = 0u64;
    let mut miss_list: Vec<(u64, String, String)> = Vec::new();
    for s in &sessions {
        let fp = (
            s.last_activity_ms,
            s.total_input_tokens,
            s.total_output_tokens,
        );
        match persisted.get(&s.id) {
            Some(p) if *p == fp => hit += 1,
            Some(p) => {
                miss_changed += 1;
                let bytes = std::fs::metadata(&s.jsonl_path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                miss_bytes += bytes;
                miss_list.push((
                    bytes,
                    s.agent_source.clone(),
                    format!(
                        "{} cached={:?} fresh={:?}",
                        &s.id[..8.min(s.id.len())],
                        p,
                        fp
                    ),
                ));
            }
            None => {
                miss_absent += 1;
                let bytes = std::fs::metadata(&s.jsonl_path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                miss_bytes += bytes;
                miss_list.push((
                    bytes,
                    s.agent_source.clone(),
                    format!("{} ABSENT", &s.id[..8.min(s.id.len())]),
                ));
            }
        }
    }
    println!(
        "fingerprint: hit={hit} miss_changed={miss_changed} miss_absent={miss_absent} \
         miss_bytes={:.1} MB",
        miss_bytes as f64 / 1e6
    );
    // Steady-state miss count under the P2 rule (dsh gates on token counters
    // only). The persisted file still carries the old dsh fingerprints, so the
    // very first run after P2 re-folds every dsh session once; this is what it
    // costs from the second run on.
    let mut miss_new_rule = 0usize;
    for s in &sessions {
        let same = persisted.get(&s.id).is_some_and(|p| {
            if s.agent_source == "dsh" {
                (p.1, p.2) == (s.total_input_tokens, s.total_output_tokens)
            } else {
                *p == (
                    s.last_activity_ms,
                    s.total_input_tokens,
                    s.total_output_tokens,
                )
            }
        });
        if !same {
            miss_new_rule += 1;
        }
    }
    println!("fingerprint misses under the P2 rule (steady state): {miss_new_rule}");

    miss_list.sort_by(|a, b| b.0.cmp(&a.0));
    for (bytes, src, why) in miss_list.iter().take(15) {
        println!("  {:>9.1} MB  {:<8} {}", *bytes as f64 / 1e6, src, why);
    }

    // ── what re-folding the misses costs (read + JSON parse, per line) ────
    let t = Instant::now();
    let mut lines = 0u64;
    for s in &sessions {
        let fp = (
            s.last_activity_ms,
            s.total_input_tokens,
            s.total_output_tokens,
        );
        if persisted.get(&s.id) == Some(&fp) {
            continue;
        }
        if s.agent_source == "dsh" {
            continue; // dsh folds over RPC, not a local file
        }
        let Ok(text) = std::fs::read_to_string(&s.jsonl_path) else {
            continue;
        };
        for line in text.lines() {
            lines += 1;
            let _ = serde_json::from_str::<serde_json::Value>(line);
        }
    }
    println!(
        "re-fold of misses: {:.1}s over {lines} lines",
        t.elapsed().as_secs_f64()
    );

    // ── the other half: what a FULL cold fold would cost (cache ignored) ──
    let t = Instant::now();
    let mut all_bytes = 0u64;
    let mut all_lines = 0u64;
    for s in &sessions {
        if s.agent_source == "dsh" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&s.jsonl_path) else {
            continue;
        };
        all_bytes += text.len() as u64;
        for line in text.lines() {
            all_lines += 1;
            let _ = serde_json::from_str::<serde_json::Value>(line);
        }
    }
    println!(
        "full cold fold (every session): {:.1}s over {all_lines} lines / {:.1} MB",
        t.elapsed().as_secs_f64(),
        all_bytes as f64 / 1e6
    );

    let dsh: Vec<_> = sessions
        .iter()
        .filter(|s| s.agent_source == "dsh")
        .collect();
    println!("\ndsh sessions in scan: {}", dsh.len());

    // ── the same fold, N threads wide ─────────────────────────────────────
    //
    // Measured BEFORE the serial pass on purpose: `dsh_cost` memoises prices to
    // disk, so whichever pass runs second inherits a warmer cache and would look
    // artificially fast.
    //
    // `with_client` drops the server lock before the request goes out, so these
    // round trips can overlap. This is the ceiling a parallel fold would buy for
    // the cache-invalid case (schema bump, tz move, first run after a fingerprint
    // change) — the only case that still folds all 424.
    for width in [8usize] {
        let t = Instant::now();
        let queue = std::sync::Mutex::new(dsh.clone());
        std::thread::scope(|scope| {
            for _ in 0..width {
                scope.spawn(|| loop {
                    let Some(s) = queue.lock().unwrap().pop() else {
                        break;
                    };
                    let _ = claw_fleet_core::dsh_cost::dsh_session_calls(&s.jsonl_path);
                });
            }
        });
        println!(
            "dsh fold of all {} sessions, {width} threads: {:.1}s",
            dsh.len(),
            t.elapsed().as_secs_f64()
        );
    }

    // ── the suspect: dsh sessions fold over RPC, one round trip each ──────
    let t = Instant::now();
    let mut slow: Vec<(u128, String)> = Vec::new();
    for s in &dsh {
        let one = Instant::now();
        let n = claw_fleet_core::dsh_cost::dsh_session_calls(&s.jsonl_path)
            .map(|c| c.len())
            .unwrap_or(0);
        slow.push((
            one.elapsed().as_millis(),
            format!("{} calls={n}", &s.id[..8.min(s.id.len())]),
        ));
    }
    let total = t.elapsed().as_secs_f64();
    slow.sort_by(|a, b| b.0.cmp(&a.0));
    println!("dsh fold of all {} sessions: {:.1}s", dsh.len(), total);
    for (ms, who) in slow.iter().take(10) {
        println!("  {ms:>6} ms  {who}");
    }

    // ── is the dsh fingerprint even stable between two scans? ─────────────
    let sessions2 = claw_fleet_core::session::scan_all_sources(&sources);
    let map2: HashMap<&str, (u64, u64, u64)> = sessions2
        .iter()
        .map(|s| {
            (
                s.id.as_str(),
                (
                    s.last_activity_ms,
                    s.total_input_tokens,
                    s.total_output_tokens,
                ),
            )
        })
        .collect();
    let mut unstable = 0;
    let mut unstable_dsh = 0;
    for s in &sessions {
        let fp = (
            s.last_activity_ms,
            s.total_input_tokens,
            s.total_output_tokens,
        );
        if map2.get(s.id.as_str()).is_some_and(|f| *f != fp) {
            unstable += 1;
            if s.agent_source == "dsh" {
                unstable_dsh += 1;
            }
        }
    }
    println!("fingerprints that changed between two back-to-back scans: {unstable} (dsh: {unstable_dsh})");
}
