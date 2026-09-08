//! Nothing that can block for a human-visible time may run on the event loop.
//!
//! A plain `#[tauri::command] fn` (no `(async)`, not an `async fn`) is
//! classified `ExecutionContext::Blocking` by tauri-macros and runs **inlined
//! on the main thread**. The main thread is also what delivers every other
//! invoke's *response* into the webview, what `emit()` ends in, and what macOS
//! presents native panels on — so a slow sync command freezes unrelated,
//! already-finished work and keeps a save dialog from appearing at all. That is
//! not hypothetical: on 2026-09-08 `check_app_version` (blocking HTTP, 10s
//! timeout, two sources tried in turn) was a plain sync command, and the boss
//! lost two buttons and a working 导出 in the minute it refreshed its cache.
//! Wiki: `desktop/ipc-stall-forensics`.
//!
//! A source scan rather than a runtime assertion because the hazard *is* the
//! annotation: by the time such a command runs, the freeze has already
//! happened, and nothing in the log would name it.

use std::fs;
use std::path::Path;

/// Bodies of the commands that run inlined on the event loop, as
/// `(file, fn name, body)`.
fn sync_command_bodies() -> Vec<(String, String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("gui");
    let mut out = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .expect("src/gui must exist")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .collect();
    entries.sort();
    for path in entries {
        let text = fs::read_to_string(&path).expect("read gui source");
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            // Exactly the sync form: `(async)` goes to the threadpool instead.
            if line.trim() != "#[tauri::command]" {
                continue;
            }
            // The signature can sit a few lines down, behind doc comments or
            // other attributes.
            let Some(sig_at) = (i + 1..(i + 8).min(lines.len()))
                .find(|&j| lines[j].contains("fn "))
            else {
                continue;
            };
            let sig = lines[sig_at];
            // `async fn` is dispatched onto tokio, not the event loop.
            if sig.contains("async fn") {
                continue;
            }
            let name = sig
                .split("fn ")
                .nth(1)
                .and_then(|rest| rest.split(['(', '<', ' ']).next())
                .unwrap_or("")
                .to_string();
            // Body: up to the next item at column 0 (a `}` or a new attribute).
            let mut body = String::new();
            for line in &lines[sig_at..] {
                body.push_str(line);
                body.push('\n');
                if *line == "}" {
                    break;
                }
            }
            out.push((
                path.file_name().unwrap().to_string_lossy().to_string(),
                name,
                body,
            ));
        }
    }
    out
}

#[test]
fn the_scan_actually_finds_the_sync_commands() {
    let found = sync_command_bodies();
    // A scan that silently matches nothing would make the guard below vacuous.
    assert!(
        found.len() > 10,
        "expected the event-loop commands to still be found, got {}",
        found.len()
    );
    assert!(
        found.iter().any(|(_, name, _)| name == "get_platform"),
        "a known trivial sync command must appear in the scan"
    );
    // And the one this guard was written for must NOT be in the list any more.
    assert!(
        !found.iter().any(|(_, name, _)| name == "check_app_version"),
        "check_app_version belongs off the event loop — see this file's header"
    );
}

#[test]
fn no_event_loop_command_does_blocking_http() {
    let offenders: Vec<String> = sync_command_bodies()
        .into_iter()
        .filter(|(_, _, body)| body.contains("reqwest::blocking") || body.contains("off_runtime"))
        .map(|(file, name, _)| format!("{file}::{name}"))
        .collect();
    assert!(
        offenders.is_empty(),
        "these run inlined on the event loop and reach blocking HTTP: {}.\n\
         Make them `#[tauri::command(async)]` AND wrap the blocking call in \
         `claw_fleet_core::off_runtime::off_runtime` — `(async)` alone lands in \
         a tokio worker, where reqwest::blocking panics and the invoke promise \
         never settles.",
        offenders.join(", ")
    );
}
