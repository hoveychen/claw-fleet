//! `fleet harness` — install / update an agent harness on *this* machine.
//!
//! The desktop app has had these actions since the environment wizard landed,
//! but only as Tauri commands: `install_harness` / `update_harness` /
//! `install_node_runtime` in `claw-fleet-desktop/src/gui/setup.rs`. That left
//! the other two clients of the data plane with no way to run them, which is
//! why the browser build greys the environment panel's buttons out.
//!
//! This module is the third leg. Putting the action behind a CLI subcommand
//! rather than a bespoke HTTP handler is what lets `fleet serve` stream it:
//! `/harness_install` spawns `fleet harness install <source>` through
//! `proc_runner` and the client tails `/proc_output`, exactly like
//! `/git_clone_stream` does for a clone. No second streaming mechanism, and
//! the same command works by hand on a headless box.
//!
//! ## The result marker
//!
//! A tailed proc only gives the client bytes plus an exit code, but the
//! frontend needs the *typed* outcome: `HarnessStatus` to refresh the card,
//! and — critically — `InstallErrorCode::NodeMissing`, which is what makes the
//! panel offer the Node bootstrap instead of just printing a failure. So the
//! last line of output is
//!
//! ```text
//! __FLEET_HARNESS_RESULT__ {"ok":{…}}     // or {"err":{"code":…,"message":…}}
//! ```
//!
//! and the exit code mirrors it (0 / 1) so a human reading the terminal sees
//! the normal thing. Progress lines go to stdout as they arrive; stdout is
//! line-buffered on a tty, so they reach the tail live rather than in one dump
//! at exit.

use claw_fleet_core::harness_install::{self, InstallError, InstallErrorCode};

/// Prefix of the machine-readable final line. Also parsed by the browser
/// build's transport (`harnessInstall.ts`) and asserted by tests on both
/// sides — treat it as wire format, not a debug affordance.
pub(crate) const RESULT_MARKER: &str = "__FLEET_HARNESS_RESULT__";

/// Print `RESULT_MARKER` plus the serialized outcome, then exit 0/1.
///
/// Never returns: the exit code is part of the contract (the serve route's
/// client uses the marker for the payload but a human uses `$?`).
fn finish<T: serde::Serialize>(result: Result<T, InstallError>) -> ! {
    let (payload, code) = match &result {
        Ok(value) => (serde_json::json!({ "ok": value }), 0),
        Err(e) => (serde_json::json!({ "err": e }), 1),
    };
    // One line, no trailing prose after it — the parser takes the remainder of
    // the line and nothing else may follow on it.
    println!("{RESULT_MARKER} {payload}");
    if let Err(e) = &result {
        eprintln!("harness action failed: {}", e.message);
    }
    std::process::exit(code);
}

/// Forward one installer output line to stdout.
///
/// Deliberately plain `println!` rather than the `fmt::` helpers: this stream
/// is read by a machine as often as by a person, and colour codes in the pty
/// log would have to be stripped by every consumer.
fn progress(line: &str) {
    println!("{line}");
}

/// `fleet harness install <source>` — install through the source's official
/// channel and re-probe.
pub(crate) fn cmd_harness_install(source: &str) -> ! {
    let result = harness_install::install_harness(source, &progress);
    // Same lifecycle edge the desktop command handles: an already-running dsh
    // service still has the old JavaScript loaded, so the freshly installed
    // one only takes effect after the machine service is stopped.
    if source == "dsh" && result.is_ok() {
        claw_fleet_core::dsh_source::shutdown();
    }
    finish(result)
}

/// `fleet harness update <source>` — drive the source's own updater and report
/// the before/after version transition.
pub(crate) fn cmd_harness_update(source: &str) -> ! {
    let result = harness_install::update_harness(source, &progress);
    if source == "dsh" && result.is_ok() {
        claw_fleet_core::dsh_source::shutdown();
    }
    finish(result)
}

/// `fleet harness install-node` — bootstrap Node.js into `~/.fleet/node`,
/// dsh's npm prerequisite on a blank machine. Prints the npm path as its ok
/// payload.
pub(crate) fn cmd_harness_install_node() -> ! {
    let result = harness_install::install_node(&progress)
        .map(|npm| npm.to_string_lossy().into_owned());
    finish(result)
}

/// `fleet harness status` — the same probe the panel shows, as JSON. Included
/// because a headless operator who can run the installer should be able to see
/// what it changed without opening a browser.
pub(crate) fn cmd_harness_status() {
    let statuses = claw_fleet_core::harness_status::probe_all();
    println!("{}", serde_json::to_string_pretty(&statuses).unwrap_or_default());
}

/// Shape the marker line carries, so both sides of the wire agree in one place.
/// Only used by tests here; the browser build has its own parser.
#[cfg(test)]
fn parse_marker(stdout: &str) -> Option<serde_json::Value> {
    stdout
        .lines()
        .rev()
        .find_map(|l| l.trim_end().strip_prefix(RESULT_MARKER))
        .and_then(|rest| serde_json::from_str(rest.trim()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The marker is found on the *last* matching line and parses back to the
    /// same JSON — the property the browser transport depends on. Guards the
    /// two ways this silently breaks: someone printing after the marker, and
    /// the pty's `\r\n` line ending defeating a naive `strip_prefix` +
    /// `from_str` (hence the `trim_end`).
    #[test]
    fn marker_line_round_trips_through_a_pty_line_ending() {
        let stdout = format!(
            "$ npm install -g dsh\nverifying installation…\n{RESULT_MARKER} {}\r\n",
            serde_json::json!({ "ok": { "source": "dsh", "installed": true } })
        );
        let parsed = parse_marker(&stdout).expect("marker parses");
        assert_eq!(parsed["ok"]["source"], "dsh");
        assert_eq!(parsed["ok"]["installed"], true);
    }

    /// A failure carries the *code*, not just prose. `NodeMissing` is the one
    /// the panel branches on to offer the Node bootstrap, so losing the code
    /// would turn a recoverable state into a dead end.
    #[test]
    fn error_payload_keeps_the_install_error_code() {
        let err = InstallError {
            code: InstallErrorCode::NodeMissing,
            message: "npm not found".into(),
        };
        let line = format!("{RESULT_MARKER} {}", serde_json::json!({ "err": err }));
        let parsed = parse_marker(&line).expect("marker parses");
        // kebab-case on the wire — the frontend compares against that spelling.
        assert_eq!(parsed["err"]["code"], "node-missing");
        assert_eq!(parsed["err"]["message"], "npm not found");
    }

    /// Progress lines are not mistaken for the result, even when one of them
    /// mentions the marker's name (an installer echoing our own command line).
    #[test]
    fn progress_lines_are_not_parsed_as_the_result() {
        assert!(parse_marker("installing…\nstill going\n").is_none());
        assert!(parse_marker(&format!("echo {RESULT_MARKER}-ish\n")).is_none());
    }
}
