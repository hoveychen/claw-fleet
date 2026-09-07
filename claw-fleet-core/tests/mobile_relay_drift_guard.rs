//! Drift-guard for the mobile-web → relay dispatch surface.
//!
//! The mobile web app calls relay methods by string (`client.request("m", …)`)
//! and `mobile_relay` dispatches on the same strings in a hand-maintained
//! `match`. Nothing ties the two together at compile time, so a renamed or
//! dropped arm does not break the build — the mobile transport just errors at
//! runtime. This test parses the current source text on both sides (not a
//! frozen snapshot) and turns that silent drift into a CI failure.
//!
//! It used to be one of three checks in `backend_drift_guard.rs`; the other
//! two policed the desktop's remote `Backend` implementation against the
//! local one and against `fleet serve`'s route table. That remote backend is
//! gone, so only this check remains.

use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

// ── Source locations (relative to claw-fleet-core/) ─────────────────────────
const MOBILE_RELAY_RS: &str = "src/mobile_relay.rs";
const MOBILE_WEB_SRC: &str = "../mobile-web/src";

/// mobile-web request methods not handled by the relay dispatcher. Empty — the
/// mobile surface is currently fully covered; keep it that way.
const KNOWN_DRIFT: &[&str] = &[];

fn manifest_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read(rel: &str) -> String {
    let p = manifest_path(rel);
    fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("drift-guard: cannot read {}: {e}", p.display()))
}

/// Method-name literals the `mobile_relay` dispatcher matches on.
fn mobile_relay_methods() -> HashSet<String> {
    let src = read(MOBILE_RELAY_RS);
    let arm = Regex::new(r#""([a-z_][a-z0-9_]*)"\s*=>"#).unwrap();
    let mut out: HashSet<String> = arm.captures_iter(&src).map(|c| c[1].to_string()).collect();
    // Multi-pattern arms: "a" | "b" => …
    let alt = Regex::new(r#""([a-z_][a-z0-9_]*)""#).unwrap();
    for line in src.lines() {
        if line.contains('|') && line.contains("=>") {
            for c in alt.captures_iter(line) {
                out.insert(c[1].to_string());
            }
        }
    }
    out
}

/// Method-name literals mobile-web passes to the relay client (`client.request`).
fn mobile_web_request_methods() -> HashSet<String> {
    let re = Regex::new(r#"client\.request(?:<[^>]*>)?\(\s*["'`]([a-z_][a-z0-9_]*)"#).unwrap();
    let mut out = HashSet::new();
    collect_ts(&manifest_path(MOBILE_WEB_SRC), &re, &mut out);
    out
}

fn collect_ts(dir: &Path, re: &Regex, out: &mut HashSet<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => panic!("drift-guard: cannot read mobile-web dir {}: {e}", dir.display()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some("node_modules") {
                continue;
            }
            collect_ts(&path, re, out);
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let is_ts = name.ends_with(".ts") || name.ends_with(".tsx");
        if !is_ts || name.contains(".test.") {
            continue;
        }
        let src = fs::read_to_string(&path).unwrap_or_default();
        for c in re.captures_iter(&src) {
            out.insert(c[1].to_string());
        }
    }
}

fn sorted(set: &HashSet<String>) -> Vec<String> {
    let mut v: Vec<String> = set.iter().cloned().collect();
    v.sort();
    v
}

#[test]
fn mobile_web_methods_are_handled_by_relay() {
    let called = mobile_web_request_methods();
    let handled = mobile_relay_methods();
    assert!(
        called.len() > 15 && handled.len() > 15,
        "drift-guard: mobile parse looks wrong ({} called, {} handled)",
        called.len(),
        handled.len()
    );
    let known: HashSet<&str> = KNOWN_DRIFT.iter().copied().collect();

    let mut new_drift = Vec::new();
    for m in sorted(&called) {
        if !handled.contains(&m) && !known.contains(m.as_str()) {
            new_drift.push(m);
        }
    }
    let stale: Vec<&&str> = KNOWN_DRIFT
        .iter()
        .filter(|m| handled.contains(**m) || !called.contains(**m))
        .collect();

    assert!(
        stale.is_empty(),
        "drift-guard: these methods are in KNOWN_DRIFT but are now handled (or no longer \
         called by mobile-web). Remove them from the allow-list: {stale:?}"
    );
    assert!(
        new_drift.is_empty(),
        "drift-guard: mobile-web calls `client.request(\"m\", …)` for these methods but \
         the `mobile_relay` dispatcher does not handle them, so the mobile app errors at runtime. \
         Add the arm to mobile_relay, fix the method name, or allow-list in KNOWN_DRIFT: \
         {new_drift:?}"
    );
}
