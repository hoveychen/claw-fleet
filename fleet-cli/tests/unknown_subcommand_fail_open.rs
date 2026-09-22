//! A `fleet` older than the `settings.json` that names it must not block
//! Claude Code.
//!
//! Fleet bakes an absolute path to itself into files that outlive the binary
//! they named — `~/.claude/settings.json` hook commands, `~/.claude.json`'s MCP
//! entry, dsh's `cordis.patch.yml`. `hooks::fault_tolerant_command` guards the
//! case where that path stops resolving, but not the case where it resolves to
//! a *stale* build: that binary runs, and clap kills it with exit 2 on the
//! subcommand it has never heard of.
//!
//! Exit 2 is a blocking signal on three of the five hook events Fleet uses
//! (measured on Claude Code 2.1.263 — see
//! `hooks::unknown_subcommand_exit_code`): PreToolUse denies the tool call
//! (`guard` matches `Bash|PowerShell`, so *every* shell command on the machine
//! is refused), UserPromptSubmit swallows the prompt outright while still
//! reporting success, and Stop makes the session unable to end.
//!
//! These tests drive the real `fleet-cli` binary through the exact shell string
//! `fault_tolerant_command` writes into settings.json, so what they assert is
//! what Claude Code would observe.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn bin_path(name: &str) -> PathBuf {
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // drop test exe name
    if p.ends_with("deps") {
        p.pop();
    }
    p.push(name);
    p
}

/// Subcommands no build will ever have — stand-ins for "a subcommand added
/// after this binary was built".
const FUTURE_SUBCOMMANDS: &[&str] = &[
    "ctx-reminder-v9",
    "some-hook-invented-next-year",
    "prd-context-v2",
];

/// Feed a child's stdin and close it, tolerating `BrokenPipe`.
///
/// Fail-open is precisely the case where the child exits *without* draining
/// stdin, so losing the race and getting EPIPE is the expected behaviour under
/// test, not a failure — what we assert is the exit code the hook observes.
fn feed_stdin(child: &mut std::process::Child, bytes: &[u8]) {
    let mut stdin = child.stdin.take().expect("stdin piped");
    if let Err(e) = stdin.write_all(bytes) {
        assert_eq!(
            e.kind(),
            std::io::ErrorKind::BrokenPipe,
            "unexpected error writing hook JSON to stdin: {e}"
        );
    }
}

/// Run the binary with hook-shaped stdin (Claude Code always pipes the event
/// JSON in) and return `(exit code, stderr)`.
fn run_with_piped_stdin(args: &[&str], stdin_json: &str) -> (i32, String) {
    let mut child = Command::new(bin_path("fleet-cli"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fleet-cli");
    feed_stdin(&mut child, stdin_json.as_bytes());
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn unknown_subcommand_from_a_hook_exits_zero_and_says_nothing() {
    for sub in FUTURE_SUBCOMMANDS {
        let (code, stderr) = run_with_piped_stdin(
            &[sub],
            r#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash"}"#,
        );
        assert_eq!(
            code, 0,
            "`fleet {sub}` exited {code}; anything non-zero on PreToolUse denies the tool call"
        );
        // stderr on a *non-blocking* event still reaches the model as noise, so
        // silence is part of the contract, not just the exit code.
        assert!(
            stderr.is_empty(),
            "`fleet {sub}` wrote to stderr: {stderr:?}"
        );
    }
}

#[test]
fn every_blocking_hook_event_survives_a_stale_binary() {
    // The three events where exit 2 is a block, each with the Fleet subcommand
    // that lives there, renamed to something this build cannot know.
    for (event, sub) in [
        ("PreToolUse", "guard-v2"),
        ("UserPromptSubmit", "prd-context-v2"),
        ("Stop", "session-idle-v2"),
    ] {
        let json = format!(r#"{{"session_id":"s1","hook_event_name":"{event}"}}"#);
        let (code, _) = run_with_piped_stdin(&[sub], &json);
        assert_eq!(
            code, 0,
            "{event} hook `fleet {sub}` exited {code}, which blocks the event"
        );
    }
}

#[test]
fn the_settings_json_wrapper_as_written_exits_zero_on_a_stale_binary() {
    // Reproduce the exact shell string `hooks::fault_tolerant_command` writes,
    // pointed at a binary that exists but does not know the subcommand. Before
    // the fail-open arm this returned 2.
    let bin = bin_path("fleet-cli");
    let command = format!(
        r#"if [ -x "{bin}" ]; then exec "{bin}" {sub}; else exit 0; fi"#,
        bin = bin.display(),
        sub = "a-subcommand-from-the-future",
    );
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sh");
    feed_stdin(
        &mut child,
        br#"{"hook_event_name":"PreToolUse","tool_name":"Bash"}"#,
    );
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "the wrapper Fleet writes into settings.json exited {:?}; stderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_known_subcommand_is_untouched() {
    // The external-subcommand arm must not shadow real subcommands, and a real
    // subcommand's own non-zero exits must still propagate. `--version` is the
    // cheapest thing that proves parsing still reaches the normal path.
    let out = Command::new(bin_path("fleet-cli"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("fleet"),
        "unexpected --version output"
    );
}
