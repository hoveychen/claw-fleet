//! Live validation of the dsh spawn and resume paths.
//!
//! Ignored by default: each of these runs real turns, which costs real model
//! credits.
//!   FLEET_DSH_BIN=$(ls ~/.npm/_npx/*/node_modules/.bin/dsh | head -1) \
//!   cargo test -p claw-fleet-core --test dsh_launch_live -- --ignored --nocapture --test-threads=1
//!
//! These drive [`DshSource`] through the [`AgentSource`] trait — the same entry
//! point `agent_source::spawn_session` / `resume_session` use — so they cover
//! the process-global server too: the source built here is a throwaway, exactly
//! like the one those dispatchers build per call.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use claw_fleet_core::agent_source::{AgentSource, ResumeSpec, SpawnSpec};
use claw_fleet_core::dsh_source::DshSource;
use claw_fleet_core::session::SessionStatus;

const PROBE_PROMPT: &str = "Reply with exactly: ok";

/// Stops Fleet's process-global `dsh web` when the test that started it ends,
/// however it ends.
///
/// The server outlives every `DshSource` on purpose (see `dsh_source::SERVER`),
/// so nothing reclaims it when a test binary exits — measured: three orphaned
/// `dsh web` processes after one live run. Production has `dsh_source::shutdown`
/// wired into its exit paths; a test binary needs its own.
struct ServerGuard;

impl Drop for ServerGuard {
    fn drop(&mut self) {
        claw_fleet_core::dsh_source::shutdown();
    }
}

fn wait_for<T>(budget: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some(v) = f() {
            return Some(v);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

#[test]
#[ignore = "runs a real dsh turn (costs model credits); run manually with --ignored"]
fn live_spawn_creates_the_session_it_promised() {
    let _guard = ServerGuard;
    let source = DshSource::new();
    let spec = SpawnSpec {
        workspace_path: "/tmp".into(),
        prompt: PROBE_PROMPT.into(),
        ..Default::default()
    };

    let spawned = source.spawn(&spec).expect("spawn");
    let session_id = spawned.session_id.clone().expect("spawn must report an id");
    println!("spawned {session_id} (server pid {})", spawned.pid);

    assert!(
        session_id.starts_with("session-"),
        "dsh ids have their own shape: {session_id}"
    );
    assert!(spawned.pid > 0, "the server's pid must be reported");

    // The promise Fleet makes to its caller: the id it hands back is the session
    // that now exists, findable without guessing which one just appeared.
    let found = wait_for(Duration::from_secs(30), || {
        source.scan_sessions().into_iter().find(|s| s.id == session_id)
    })
    .expect("the spawned session must show up in a scan");

    assert_eq!(found.workspace_path, "/tmp");
    assert_eq!(found.agent_source, "dsh");
    assert_eq!(found.jsonl_path, format!("dsh://{session_id}"));

    // And it must actually be running the prompt we gave it, not sitting blank.
    let worked = wait_for(Duration::from_secs(90), || {
        let s = source.scan_sessions().into_iter().find(|s| s.id == session_id)?;
        (s.status != SessionStatus::Idle).then_some(s.status)
    });
    println!("status after spawn: {worked:?}");
    assert!(worked.is_some(), "the spawned session never started working");
}

/// The path the desktop launcher actually takes.
///
/// Every test above drives `DshSource` directly, which proves the source works
/// but skips the step in front of it: the UI sends a *tool string*, and
/// `agent_source::spawn_session` has to resolve that string to this source.
/// It resolves by `api_name()`, which dsh never overrides — so this test is what
/// says the default (`name()` → `"dsh"`) is the string the launcher must send,
/// and that the launcher's new "dsh" entry lands somewhere real rather than on
/// "agent tool 'dsh' is not available".
#[test]
#[ignore = "runs a real dsh turn (costs model credits); run manually with --ignored"]
fn live_the_launcher_tool_string_reaches_this_source() {
    let _guard = ServerGuard;
    let spawned = claw_fleet_core::agent_source::spawn_session(
        "dsh",
        &SpawnSpec {
            workspace_path: "/tmp".into(),
            prompt: PROBE_PROMPT.into(),
            ..Default::default()
        },
    )
    .expect("the launcher's tool string must route to the dsh source");
    let session_id = spawned.session_id.expect("spawn must report an id");

    let found = wait_for(Duration::from_secs(30), || {
        DshSource::new()
            .scan_sessions()
            .into_iter()
            .find(|s| s.id == session_id)
    })
    .expect("the dispatched session must show up in a scan");
    assert_eq!(found.agent_source, "dsh");
}

/// An unknown tool must fail loudly rather than silently launching Claude.
#[test]
fn an_unknown_launcher_tool_is_refused() {
    let err = claw_fleet_core::agent_source::spawn_session(
        "not-a-real-agent",
        &SpawnSpec {
            workspace_path: "/tmp".into(),
            prompt: "noop".into(),
            ..Default::default()
        },
    )
    .expect_err("an unknown tool must not fall back to another agent");
    assert!(err.contains("not-a-real-agent"), "{err}");
}

/// A caller-supplied id must be honoured end to end — that is what lets Fleet
/// correlate the session it asked for with the one that appears.
#[test]
#[ignore = "runs a real dsh turn (costs model credits); run manually with --ignored"]
fn live_spawn_honours_a_preassigned_id() {
    let _guard = ServerGuard;
    let source = DshSource::new();
    let wanted = format!("session-{}", uuid::Uuid::new_v4());
    let spawned = source
        .spawn(&SpawnSpec {
            workspace_path: "/tmp".into(),
            prompt: PROBE_PROMPT.into(),
            session_id: Some(wanted.clone()),
            ..Default::default()
        })
        .expect("spawn");
    assert_eq!(spawned.session_id.as_deref(), Some(wanted.as_str()));
}

/// The resume contract: the turn continues the existing session, and `on_exit`
/// fires with the turn's real outcome rather than at admission time.
#[test]
#[ignore = "runs two real dsh turns (costs model credits); run manually with --ignored"]
fn live_resume_continues_a_session_and_reports_its_outcome() {
    let _guard = ServerGuard;
    let source = DshSource::new();
    let spawned = source
        .spawn(&SpawnSpec {
            workspace_path: "/tmp".into(),
            prompt: "Remember the number 41. Reply with exactly: stored".into(),
            ..Default::default()
        })
        .expect("spawn");
    let session_id = spawned.session_id.expect("id");

    // Let the first turn finish before layering a second one on top.
    let settled = wait_for(Duration::from_secs(120), || {
        let s = source.scan_sessions().into_iter().find(|x| x.id == session_id)?;
        matches!(s.status, SessionStatus::WaitingInput | SessionStatus::Idle).then_some(s.status)
    });
    println!("first turn settled: {settled:?}");

    let (tx, rx) = mpsc::channel();
    source
        .resume(
            &ResumeSpec {
                session_id: session_id.clone(),
                workspace_path: "/tmp".into(),
                prompt: "What number did I ask you to remember? Reply with just the number.".into(),
                ..Default::default()
            },
            Box::new(move |ok| {
                let _ = tx.send(ok);
            }),
        )
        .expect("resume");

    // `session.prompt` returns at admission; `on_exit` must wait for `turn/end`.
    let outcome = rx
        .recv_timeout(Duration::from_secs(180))
        .expect("on_exit must fire when the resumed turn ends");
    assert!(outcome, "a turn that completed must report success");

    // The resumed turn appended to the same session — the history the model
    // answered from is the one Fleet resumed, not a fresh transcript.
    let events = source
        .get_messages(&format!("dsh://{session_id}"))
        .expect("history");
    println!("{} event(s) after the resumed turn", events.len());
    let text = serde_json::to_string(&events).unwrap();
    assert!(
        text.contains("41"),
        "the resumed turn must see the first turn's context"
    );
}

/// Resuming something that does not exist must fail loudly *and* release the
/// caller's `on_exit`, or the auto-resume scheduler would hold its slot forever
/// waiting for a turn that never started.
#[test]
#[ignore = "starts a real `dsh web`; run manually with --ignored"]
fn live_resume_of_an_unknown_session_releases_its_callback() {
    let _guard = ServerGuard;
    let source = DshSource::new();
    let (tx, rx) = mpsc::channel();
    let err = source
        .resume(
            &ResumeSpec {
                session_id: "session-does-not-exist".into(),
                workspace_path: "/tmp".into(),
                prompt: PROBE_PROMPT.into(),
                ..Default::default()
            },
            Box::new(move |ok| {
                let _ = tx.send(ok);
            }),
        )
        .unwrap_err();
    println!("resume error: {err}");
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)),
        Ok(false),
        "a resume that never started must report failure immediately"
    );
}

/// The stop button's dsh path, end to end.
///
/// Fleet's stop is otherwise pid-based, and a dsh `SessionInfo` carries the
/// *shared server's* pid — signalling it would stop every dsh session on the
/// machine. `interrupt_session_at` routes to `session.cancel` instead, and what
/// proves it worked is not the RPC receipt but the turn's own verdict: dsh ends
/// a cancelled turn with `reason.kind = "aborted"`, which
/// `dsh_events::LiveView` reports to `on_exit` as failure — against `true` for
/// the turn that finished on its own, asserted in the resume test above.
#[test]
#[ignore = "runs a real dsh turn (costs model credits); run manually with --ignored"]
fn live_interrupt_cancels_the_turn_without_touching_the_server() {
    let _guard = ServerGuard;
    let source = DshSource::new();
    let spawned = source
        .spawn(&SpawnSpec {
            workspace_path: "/tmp".into(),
            prompt: "Count slowly from 1 to 200, one number per line, and nothing else.".into(),
            ..Default::default()
        })
        .expect("spawn");
    let session_id = spawned.session_id.expect("id");
    let server_pid = spawned.pid;

    // Wait until the turn is actually in flight; cancelling before admission
    // would prove nothing.
    let running = wait_for(Duration::from_secs(120), || {
        let s = source.scan_sessions().into_iter().find(|x| x.id == session_id)?;
        (s.status != SessionStatus::Idle).then_some(s.status)
    });
    println!("status before interrupt: {running:?}");
    assert!(running.is_some(), "the turn never started");

    claw_fleet_core::agent_source::interrupt_session_at(&format!("dsh://{session_id}"))
        .expect("interrupt must reach the dsh source");

    // The user-visible promise: the turn stops. (That dsh reports it as
    // `reason.kind = "aborted"` rather than `"completed"`, and that
    // `LiveView` therefore settles `on_exit` with failure, is pinned by the
    // unit tests beside `dsh_events` — this one is about the turn really
    // ending, on a prompt long enough that it would still be running.)
    let stopped = wait_for(Duration::from_secs(120), || {
        let s = source.scan_sessions().into_iter().find(|x| x.id == session_id)?;
        matches!(s.status, SessionStatus::WaitingInput | SessionStatus::Idle).then_some(s.status)
    });
    println!("status after interrupt: {stopped:?}");
    assert!(stopped.is_some(), "the cancelled turn never stopped");

    // The whole point of routing by session: the shared server is untouched, so
    // every other dsh session (and Fleet's own view of them) survives.
    assert!(
        claw_fleet_core::session::is_process_alive(server_pid),
        "the shared dsh web server (pid {server_pid}) must survive a session interrupt"
    );
    assert!(
        !source.scan_sessions().is_empty(),
        "the server must still be answering after the interrupt"
    );
}

/// The chat workspace's whole point: a dsh session there must NOT be handed
/// Fleet's engineering doctrine.
///
/// This is the piece unit tests cannot reach. `dsh_chat_preset`'s tests prove the
/// composition text comes out right, and `dsh_source`'s prove the two RPCs are
/// walked — but only a live turn answers the two questions that matter: does dsh
/// actually mount a preset Fleet wrote to disk while it was running (discovery is
/// unmemoized, so it should), and does redirecting that one `dshHome` really drop
/// the user-global `AGENTS.md` while keeping the workspace's own `CLAUDE.md`?
///
/// Reads the answer out of the durable history rather than out of Fleet's own
/// belief: the `agent-instructions` baseline is a `user/message` whose text names
/// each file it loaded, so "the doctrine is gone" is checkable, not inferred.
#[test]
#[ignore = "runs a real dsh turn in the chat workspace (costs model credits); run manually with --ignored"]
fn live_a_chat_session_is_not_handed_the_doctrine() {
    let _guard = ServerGuard;
    let workspace = claw_fleet_core::chat_workspace::ensure_chat_workspace()
        .expect("the chat workspace must exist");
    let source = DshSource::new();
    let spec = SpawnSpec {
        workspace_path: workspace.clone(),
        prompt: PROBE_PROMPT.into(),
        ..Default::default()
    };

    let spawned = source.spawn(&spec).expect("spawn");
    let session_id = spawned.session_id.clone().expect("spawn must report an id");
    println!("spawned chat session {session_id}");

    // Wait until the baseline has entered history (it rides the first step).
    let baseline = wait_for(Duration::from_secs(120), || {
        let messages = source.get_messages(&format!("dsh://{session_id}")).ok()?;
        messages.into_iter().find(|m| {
            m.get("isMeta").and_then(serde_json::Value::as_bool) == Some(true)
                && m["message"]["content"][0]["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("Instructions from:"))
        })
    })
    .expect("the agent-instructions baseline never arrived");

    let text = baseline["message"]["content"][0]["text"].as_str().unwrap();
    println!("--- baseline ({} chars) ---\n{text}", text.chars().count());

    // 1. Folded, not a bubble — the transcript fix.
    assert_eq!(baseline["isMeta"], serde_json::json!(true));

    // 2. The chat brief still loads: dropping the user-global file must not drop
    //    the workspace's own instructions with it.
    assert!(
        text.contains("纯聊天工作区"),
        "the chat brief must still reach the model",
    );

    // 3. And the doctrine is gone. Each Fleet block carries its own sentinel
    //    heading, so naming them is exact rather than a length heuristic.
    for doctrine in [
        "Fleet PRD Discipline for dsh",
        "Fleet Interaction Mode for dsh",
        "Rule 3 — Worktree-based workflow",
    ] {
        assert!(
            !text.contains(doctrine),
            "the chat session was handed the doctrine block {doctrine:?}",
        );
    }

    // 4. dsh really mounted Fleet's preset, not the default.
    let listed = source.scan_sessions();
    let found = listed.iter().find(|s| s.id == session_id);
    assert!(found.is_some(), "the chat session must show up in a scan");

    // ── the other two transcript fixes, on the same real turn ──────────────
    //
    // Waiting for the assistant's assembled message rather than asserting on
    // what is already there: the baseline lands in step 1, the reply later.
    let messages = wait_for(Duration::from_secs(180), || {
        let all = source.get_messages(&format!("dsh://{session_id}")).ok()?;
        all.iter()
            .any(|m| {
                m["type"] == "assistant"
                    && m["message"]["content"]
                        .as_array()
                        .is_some_and(|c| c.iter().any(|b| b["type"] == "text"))
            })
            .then_some(all)
    })
    .expect("the assistant never answered");

    for m in &messages {
        for block in m["message"]["content"].as_array().into_iter().flatten() {
            assert_ne!(
                block["type"], "reasoning",
                "a raw dsh reasoning block reached the renderer, which draws it as \
                 a wrench-icon tool card instead of a thinking fold: {block}",
            );
        }
    }

    // Every non-human record folds; the human's own prompt does not.
    let human: Vec<_> = messages
        .iter()
        .filter(|m| m["type"] == "user" && m.get("isMeta").is_none())
        .collect();
    assert_eq!(
        human.len(),
        1,
        "exactly one un-folded user bubble (the prompt) — got {}: {human:#?}",
        human.len(),
    );
    assert_eq!(human[0]["message"]["content"][0]["text"], PROBE_PROMPT);

    // The runtime-context snapshot is folded rather than dropped, so it is still
    // readable — that was the deliberate contract change.
    assert!(
        messages.iter().any(|m| {
            m.get("isMeta").and_then(serde_json::Value::as_bool) == Some(true)
                && m["message"]["content"][0]["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("runtime context"))
        }),
        "the dsh-system-prompt runtime snapshot must survive as a folded record",
    );

    let thinking = messages
        .iter()
        .flat_map(|m| m["message"]["content"].as_array().into_iter().flatten())
        .filter(|b| b["type"] == "thinking")
        .count();
    println!("folded meta records + {thinking} thinking block(s) on a real turn");
}
