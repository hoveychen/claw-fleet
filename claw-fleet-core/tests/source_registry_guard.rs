//! Drift-guard: every agent source the registry can build must also be listed
//! in the settings panel.
//!
//! `agent_source.rs` holds the same roster twice:
//!
//! * [`build_sources`] decides which sources actually get scanned, branching on
//!   `config.is_enabled("<name>")`.
//! * `get_sources_config_local` builds the `SourceInfo` list the desktop's
//!   "agents to monitor" section renders — from a **hardcoded array**.
//!
//! When dsh was added, only the first was updated. The consequence was not a
//! crash but something worse: because `SourcesConfig::is_enabled` defaults a
//! missing entry to `true`, dsh was implicitly *on* while having no row in the
//! panel — the user could neither see it nor turn it off. It took a user asking
//! "why is there no deepseek harness option in settings?" to find it.
//!
//! A source-text scan rather than a runtime comparison, deliberately:
//! `build_sources` additionally gates dsh on the binary existing, so a runtime
//! set-equality check would pass on a machine without dsh installed — exactly
//! the machine where this bug was invisible.

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

fn agent_source_rs() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("agent_source.rs")
}

/// The brace-matched body of the named top-level `fn`.
fn fn_body(src: &str, name: &str) -> String {
    let needle = format!("pub fn {name}(");
    let start = src
        .find(&needle)
        .unwrap_or_else(|| panic!("`pub fn {name}` not found — was it renamed?"));
    let mut depth = 0i32;
    let mut body = String::new();
    let mut seen_open = false;
    for ch in src[start..].chars() {
        if ch == '{' {
            depth += 1;
            seen_open = true;
        } else if ch == '}' {
            depth -= 1;
        }
        body.push(ch);
        if seen_open && depth == 0 {
            break;
        }
    }
    body
}

#[test]
fn every_buildable_source_has_a_settings_row() {
    let src = fs::read_to_string(agent_source_rs()).expect("read agent_source.rs");

    // Names `build_sources` can register.
    let gate = Regex::new(r#"is_enabled\("([a-z0-9-]+)"\)"#).expect("gate regex");
    let build_body = fn_body(&src, "build_sources");
    let mut buildable: Vec<String> = gate
        .captures_iter(&build_body)
        .map(|c| c[1].to_string())
        .collect();
    buildable.sort();
    buildable.dedup();
    assert!(
        buildable.len() >= 2,
        "the scanner found {buildable:?} — it has probably stopped recognising \
         `config.is_enabled(\"…\")` in build_sources"
    );

    // Names the settings panel is offered.
    let listed_body = fn_body(&src, "get_sources_config_local");
    let listed = Regex::new(r#""([a-z0-9-]+)""#).expect("listed regex");
    let listed: Vec<String> = listed
        .captures_iter(&listed_body)
        .map(|c| c[1].to_string())
        .collect();

    let missing: Vec<&String> = buildable.iter().filter(|n| !listed.contains(n)).collect();
    assert!(
        missing.is_empty(),
        "these sources can be built but have no row in the settings panel: {missing:?}\n\
         `SourcesConfig::is_enabled` defaults a missing entry to `true`, so each of \
         them is silently ENABLED with no way for the user to see or disable it.\n\
         Fix by adding it to `get_sources_config_local`'s roster (and its \
         `settings.source_name.<name>` i18n key plus an `AgentSourceIcon` branch)."
    );
}
