//! End-to-end for the parked-decision-card path, driven through the real
//! `fleet-cli mcp` binary rather than a library call.
//!
//! What it pins down, in the order the bug used to bite:
//!
//! 1. A `fleet__ask` nobody answers **does not delete the card**. Before this,
//!    the timeout deleted the request file and the question was gone from the
//!    desktop and the phone.
//! 2. The turn that asked **is stopped**, by a real SIGINT to the session's
//!    `claude` process — not by asking the agent nicely. The fake `claude` here
//!    is a real process with the real argv shape, so `session_pid`'s exact-match
//!    lookup and the signal are both exercised for real.
//! 3. Answering the parked card **resumes the session with the reply attached**.
//!    The resume goes through `claude --resume`, so the assertion is on the argv
//!    a stand-in binary actually received.
//!
//! Ignored by default: `DecisionPanelConfig` clamps `wait_seconds` to a 60s
//! floor, so the timeout this test waits out is a real minute.
//!
//!     cargo test -p fleet-cli --test parked_decision_e2e -- --ignored --nocapture

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SESSION_ID: &str = "e2e-parked-11111111-2222-3333";
const QUESTION: &str = "要不要保留向后兼容？";

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// A `claude` stand-in that outlives its args and dies on SIGINT — i.e. exactly
/// the two properties the parked path depends on. The process **name** has to be
/// `claude`: `session::scan_cli_processes` filters on it before it even looks at
/// argv, so a shell script (which would report as `sh`) can't stand in here.
fn build_fake_claude(dir: &Path) -> PathBuf {
    let src = dir.join("fake_claude.c");
    std::fs::write(&src, "#include <unistd.h>\nint main(void){for(;;)pause();return 0;}\n").unwrap();
    let bin = dir.join("claude");
    let out = Command::new("cc")
        .args(["-o", bin.to_str().unwrap(), src.to_str().unwrap()])
        .output()
        .expect("cc must be available to build the fake claude");
    assert!(
        out.status.success(),
        "cc failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    bin
}

/// A second `claude` stand-in, this one for the *resume* leg: it records the
/// argv it was handed and exits, so the test can assert on what Fleet actually
/// asked the CLI to do.
fn write_resume_recorder(dir: &Path, log: &Path) -> PathBuf {
    let path = dir.join("claude-recorder");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s@@ARG@@' \"$a\" >> {}; done\n",
            log.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

fn wait_until(budget: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < budget {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    done()
}

/// Whether the fake session process is still *running*.
///
/// Deliberately `try_wait` and not `kill(pid, 0)`: the process is this test's
/// own child, so once it dies it lingers as a zombie until reaped — and a zombie
/// answers `kill(pid, 0)` just fine. Signalling it would look like it never
/// worked.
fn still_running(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(None))
}

struct Reaper(Child);
impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "waits out the real 60s decision-panel timeout; run with --ignored"]
fn timed_out_fleet_ask_parks_the_card_interrupts_the_turn_and_resumes_with_the_answer() {
    let home = std::env::temp_dir().join(format!("fleet-parked-e2e-{}", std::process::id()));
    let fleet_dir = home.join(".fleet");
    let workspace = home.join("workspace");
    let projects = home.join(".claude").join("projects").join("proj");
    for d in [&fleet_dir, &workspace, &projects] {
        std::fs::create_dir_all(d).unwrap();
    }

    // The session Fleet "launched": a transcript stamped with the Fleet
    // entrypoint is the only thing that makes a card parkable at all.
    std::fs::write(
        projects.join(format!("{SESSION_ID}.jsonl")),
        format!(
            r#"{{"type":"user","entrypoint":"{}","cwd":"{}"}}"#,
            claw_fleet_core::session_launch::NEW_SESSION_ENTRYPOINT,
            workspace.display()
        ) + "\n",
    )
    .unwrap();

    // 60s is the floor `DecisionPanelConfig` clamps to — asking for less is
    // silently raised, so the test has to actually wait it out.
    std::fs::write(
        fleet_dir.join("decision-panel.json"),
        r#"{"wait_seconds":60,"poll_ms":200,"heartbeat_window_seconds":30}"#,
    )
    .unwrap();

    // The resume leg must land on the recorder, not on the user's real CLI.
    let resume_log = home.join("resume-argv.txt");
    let recorder = write_resume_recorder(&home, &resume_log);
    std::fs::write(
        fleet_dir.join("claude-binary.json"),
        serde_json::json!({ "override_path": recorder.to_string_lossy() }).to_string(),
    )
    .unwrap();

    // `fleet__ask` refuses to queue anything unless a Fleet consumer is alive,
    // and bails out mid-wait if the heartbeat goes stale — so keep beating.
    let hb_path = fleet_dir.join("consumer.heartbeat");
    let hb = hb_path.clone();
    let beating = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let beating_thread = beating.clone();
    std::fs::write(&hb_path, format!("{}\n{}\n", now_ms(), std::process::id())).unwrap();
    let beat = std::thread::spawn(move || {
        while beating_thread.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = std::fs::write(&hb, format!("{}\n{}\n", now_ms(), std::process::id()));
            std::thread::sleep(Duration::from_millis(500));
        }
    });

    // The session's own `claude` process, with the argv shape Fleet's headless
    // spawns carry (`-p` + `--session-id`) so `session_pid` can find exactly it.
    let fake_claude = build_fake_claude(&home);
    let session_proc = Reaper(
        Command::new(&fake_claude)
            .args(["-p", "--session-id", SESSION_ID])
            .current_dir(&workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut session_proc = session_proc;
    assert!(still_running(&mut session_proc.0), "fake claude must be up");

    // ── Act: a fleet__ask through the real MCP server, which nobody answers ──
    let mut mcp = Reaper(
        Command::new(env!("CARGO_BIN_EXE_fleet-cli"))
            .arg("mcp")
            .env("FLEET_HOME", &home)
            .env("CLAUDE_CODE_SESSION_ID", SESSION_ID)
            .env("CLAUDE_PROJECT_DIR", &workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut stdin = mcp.0.stdin.take().unwrap();
    let mut stdout = BufReader::new(mcp.0.stdout.take().unwrap());

    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "fleet__ask",
            "arguments": {
                "questions": [{
                    "question": QUESTION,
                    "header": "兼容性",
                    "multiSelect": false,
                    "options": [
                        { "label": "保留", "description": "keep it" },
                        { "label": "丢掉", "description": "drop it" }
                    ]
                }]
            }
        }
    });
    writeln!(stdin, "{call}").unwrap();
    stdin.flush().unwrap();

    // The card is live first — this is the window in which the user could have
    // answered. Nobody does.
    let parked_dir = fleet_dir.join("parked");
    let ask_dir = fleet_dir.join("fleet-ask");
    assert!(
        wait_until(Duration::from_secs(10), || {
            std::fs::read_dir(&ask_dir).map(|d| d.count() > 0).unwrap_or(false)
        }),
        "the ask should have been queued as a live card first"
    );

    // ── Assert 1: the tool comes back telling the agent to stop, not to retry ─
    let mut line = String::new();
    stdout.read_line(&mut line).expect("MCP must answer the call");
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert_eq!(resp["result"]["isError"], serde_json::json!(true), "{resp}");
    assert!(
        text.contains("挂起") && text.contains("不要重试"),
        "the agent must be told to stop, not handed a bare timeout: {text}"
    );

    // ── Assert 2: the question survived; it did not vanish with the timeout ──
    let parked: Vec<PathBuf> = std::fs::read_dir(&parked_dir)
        .expect("parked dir must exist")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    assert_eq!(parked.len(), 1, "exactly one parked card, got {parked:?}");
    let card: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&parked[0]).unwrap()).unwrap();
    let card_id = card["id"].as_str().unwrap().to_string();
    assert_eq!(card["kind"], serde_json::json!("fleetAsk"));
    assert_eq!(card["sessionId"], serde_json::json!(SESSION_ID));
    assert_eq!(
        card["workspacePath"].as_str().unwrap(),
        workspace.to_str().unwrap(),
        "the resume needs the session's launch dir"
    );
    assert_eq!(
        card["request"]["questions"][0]["question"],
        serde_json::json!(QUESTION),
        "the question itself must be preserved verbatim"
    );
    assert_eq!(
        card["request"]["parked"],
        serde_json::json!(true),
        "the stored request must carry the flag the UI badges off"
    );
    // And it is no longer sitting in the live channel dir, so nothing polls it.
    assert_eq!(
        std::fs::read_dir(&ask_dir).map(|d| d.count()).unwrap_or(0),
        0,
        "the live request must have been moved out, not left as an orphan"
    );

    // ── Assert 3: the turn that asked was actually stopped ──────────────────
    assert!(
        wait_until(Duration::from_secs(10), || !still_running(&mut session_proc.0)),
        "the session's claude process should have been SIGINT'd, not left running"
    );
    let status = session_proc.0.wait().unwrap();
    assert_eq!(
        std::os::unix::process::ExitStatusExt::signal(&status),
        Some(libc::SIGINT),
        "the turn must be stopped with SIGINT (which a headless CLI unwinds and \
         resumes from), not killed outright: {status:?}"
    );

    // ── Act 2: the boss finally answers, hours later ────────────────────────
    // Same in-process entry point the desktop's `respond_to_fleet_ask` and the
    // probe's `/fleet-ask/respond` both funnel into.
    unsafe { std::env::set_var("FLEET_HOME", &home) };
    let answer = serde_json::json!({
        "id": card_id,
        "answers": { QUESTION: "保留" },
        "cancelled": false
    });
    let routed = claw_fleet_core::parked::try_resolve(&card_id, &answer, false);
    routed
        .expect("a parked id must route to the resume path, not to a response file")
        .expect("resume must succeed");

    // ── Assert 4: the session was resumed, carrying the reply ───────────────
    assert!(
        wait_until(Duration::from_secs(10), || resume_log.is_file()),
        "answering a parked card must spawn `claude --resume`"
    );
    let argv = std::fs::read_to_string(&resume_log).unwrap();
    // Split on the recorder's own delimiter, not on newlines: the resume prompt
    // is a multi-line string travelling as a single argv element, and splitting
    // it by line would shred exactly the thing under test.
    let args: Vec<&str> = argv.split("@@ARG@@").filter(|a| !a.is_empty()).collect();
    assert!(
        args.iter().any(|a| *a == "--resume"),
        "must resume the same session, not start a new one: {args:?}"
    );
    assert!(
        args.iter().any(|a| *a == SESSION_ID),
        "must resume THIS session: {args:?}"
    );
    let prompt = args
        .iter()
        .find(|a| a.contains("老板"))
        .expect("the resume prompt must be passed to the CLI");
    assert!(
        prompt.contains(QUESTION),
        "the agent's own question must be restated — its tool call was interrupted \
         and never returned a result: {prompt}"
    );
    assert!(
        prompt.contains("保留"),
        "the boss's actual answer must reach the agent: {prompt}"
    );

    // ── Assert 5: the card is resolved and gone ─────────────────────────────
    assert!(
        !parked[0].is_file(),
        "an answered card must not stay parked, or it would resume again"
    );

    beating.store(false, std::sync::atomic::Ordering::SeqCst);
    let _ = beat.join();
    let _ = std::fs::remove_dir_all(&home);
}
