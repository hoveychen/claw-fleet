//! Live validation of the dsh Decision Card bridge.
//!
//! Ignored by default: each of these runs real turns, which costs real model
//! credits.
//!   FLEET_DSH_BIN=$(ls ~/.npm/_npx/*/node_modules/.bin/dsh | head -1) \
//!   cargo test -p claw-fleet-core --test dsh_decisions_live -- --ignored --nocapture --test-threads=1
//!
//! Every test relocates Fleet's card directories under a throwaway `FLEET_HOME`
//! before it starts. That is not only isolation from a running desktop app —
//! without it these tests would raise real Decision Cards on the user's screen
//! and race the user for the answer.
//!
//! What each test proves end to end: dsh parks a turn on a human decision → the
//! bridge turns the frame into a Fleet card → a card answer written the way the
//! desktop writes it → `/api/respond` accepts it → **dsh acts on it**. The last
//! link is the one that matters, so the assertions are on observable effects
//! (a file the agent was told to create) rather than on the receipt.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use claw_fleet_core::agent_source::{AgentSource, SpawnSpec};
use claw_fleet_core::dsh_source::DshSource;

/// How long to wait for the agent to reach the decision. It has to boot, plan,
/// and make one tool call first, so this is minutes rather than seconds.
const DECISION_BUDGET: Duration = Duration::from_secs(180);

/// How long to wait for an effect after the answer goes back.
const EFFECT_BUDGET: Duration = Duration::from_secs(120);

/// Stops Fleet's process-global `dsh web` when the test that started it ends.
/// See `dsh_launch_live`'s copy for why a test binary needs its own.
struct ServerGuard;

impl Drop for ServerGuard {
    fn drop(&mut self) {
        claw_fleet_core::dsh_source::shutdown();
    }
}

/// Point Fleet's `~/.fleet` at a throwaway directory, once per test binary.
///
/// Set before any card is written, and never changed afterwards: the bridge
/// resolves the directory on each write, so moving it mid-run would strand
/// cards in the old one.
fn isolated_fleet_home() -> &'static Path {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("fleet-dsh-decisions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create isolated FLEET_HOME");
        std::env::set_var("FLEET_HOME", &dir);
        dir
    })
}

/// A path outside the session's workspace, so writing to it needs an approval.
///
/// The session runs with cwd `/tmp` under the default `workspace-write` preset,
/// which lets it write anywhere under `/tmp` without asking — the probe has to
/// live somewhere else for the turn to park.
fn probe_path(tag: &str) -> PathBuf {
    let home = dirs::home_dir().expect("home dir");
    home.join(format!("fleet-dsh-approval-probe-{}-{tag}.txt", std::process::id()))
}

fn wait_for<T>(budget: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if let Some(v) = f() {
            return Some(v);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    None
}

/// Start a session whose first tool call needs an approval, and return its id
/// alongside the card the bridge raised for it.
fn park_a_turn_on_an_approval(
    probe: &Path,
) -> (
    String,
    claw_fleet_core::permission_prompt_ipc::PermissionPromptRequest,
) {
    isolated_fleet_home();
    let _ = std::fs::remove_file(probe);

    let source = DshSource::new();
    let spawned = source
        .spawn(&SpawnSpec {
            workspace_path: "/tmp".into(),
            prompt: format!(
                "Run this exact shell command and nothing else: touch {}",
                probe.display()
            ),
            ..Default::default()
        })
        .expect("spawn");
    let session_id = spawned.session_id.expect("spawn must report an id");

    let card = wait_for(DECISION_BUDGET, || {
        claw_fleet_core::permission_prompt_ipc::list_pending_requests()
            .into_iter()
            .filter_map(|id| claw_fleet_core::permission_prompt_ipc::read_request(&id))
            .find(|r| r.session_id == session_id)
    })
    .unwrap_or_else(|| panic!("no approval card for {session_id} within {DECISION_BUDGET:?}"));

    (session_id, card)
}

/// Wait for the bridge to consume the answer: it removes both card files only
/// after `/api/respond` has been POSTed.
fn wait_until_card_is_consumed(id: &str) {
    let gone = wait_for(Duration::from_secs(30), || {
        claw_fleet_core::permission_prompt_ipc::read_request(id)
            .is_none()
            .then_some(())
    });
    assert!(
        gone.is_some(),
        "bridge never consumed the answer to card {id}"
    );
}

#[test]
#[ignore = "runs a real dsh turn (costs model credits); run manually with --ignored"]
fn live_an_allowed_approval_lets_the_tool_call_through() {
    let _guard = ServerGuard;
    let probe = probe_path("allow");
    let (session_id, card) = park_a_turn_on_an_approval(&probe);

    // The card has to carry enough for a human to judge the action, and enough
    // for the bridge to answer it: dsh refuses an answer without `approvalId`.
    assert_eq!(card.session_id, session_id);
    assert!(
        !card.tool_name.is_empty(),
        "card names no tool: {:?}",
        card.tool_name
    );
    assert!(
        card.tool_input
            .get("approvalId")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty()),
        "card carries no approvalId: {}",
        card.tool_input
    );
    assert_eq!(card.tool_input["agent"], "dsh");
    println!(
        "card: tool={} reason={:?}",
        card.tool_name,
        card.tool_input.get("reason")
    );

    claw_fleet_core::permission_prompt_ipc::write_response(
        &claw_fleet_core::permission_prompt_ipc::PermissionPromptResponse {
            id: card.id.clone(),
            decision: claw_fleet_core::permission_prompt_ipc::PermissionPromptDecision::Allow,
            reason: None,
        },
    )
    .expect("write allow");
    wait_until_card_is_consumed(&card.id);

    // The only assertion that proves the answer reached dsh rather than just
    // the file system: the command the approval was blocking actually ran.
    let created = wait_for(EFFECT_BUDGET, || probe.exists().then_some(()));
    assert!(
        created.is_some(),
        "allowed the tool call but {} was never created",
        probe.display()
    );
    let _ = std::fs::remove_file(&probe);
}

#[test]
#[ignore = "runs a real dsh turn (costs model credits); run manually with --ignored"]
fn live_a_rejected_approval_stops_the_tool_call() {
    let _guard = ServerGuard;
    let probe = probe_path("deny");
    let (_session_id, card) = park_a_turn_on_an_approval(&probe);

    claw_fleet_core::permission_prompt_ipc::write_response(
        &claw_fleet_core::permission_prompt_ipc::PermissionPromptResponse {
            id: card.id.clone(),
            decision: claw_fleet_core::permission_prompt_ipc::PermissionPromptDecision::Deny,
            reason: Some("not in this test".into()),
        },
    )
    .expect("write deny");
    wait_until_card_is_consumed(&card.id);

    // A rejection dsh acted on means the write never happened. Give the agent
    // room to try (and fail) before concluding the file is absent.
    std::thread::sleep(Duration::from_secs(20));
    let existed = probe.exists();
    let _ = std::fs::remove_file(&probe);
    assert!(
        !existed,
        "rejected the tool call but {} was created anyway",
        probe.display()
    );
}

#[test]
#[ignore = "runs a real dsh turn (costs model credits); run manually with --ignored"]
fn live_a_question_becomes_a_card_and_its_answer_reaches_the_agent() {
    let _guard = ServerGuard;
    isolated_fleet_home();

    let source = DshSource::new();
    let spawned = source
        .spawn(&SpawnSpec {
            workspace_path: "/tmp".into(),
            prompt: "Call the ask_user_question tool exactly once, with a single \
                     question whose id is \"colour\", whose question text is \
                     \"Which colour?\", and whose options are exactly \"Crimson\" \
                     and \"Cobalt\". Do not run any other tool first. After you \
                     have the answer, reply with just the colour I picked."
                .into(),
            ..Default::default()
        })
        .expect("spawn");
    let session_id = spawned.session_id.expect("spawn must report an id");

    let card = wait_for(DECISION_BUDGET, || {
        claw_fleet_core::elicitation::list_pending_requests()
            .into_iter()
            .filter_map(|id| claw_fleet_core::elicitation::read_request(&id))
            .find(|r| r.session_id == session_id)
    })
    .unwrap_or_else(|| panic!("no question card for {session_id} within {DECISION_BUDGET:?}"));

    assert!(!card.questions.is_empty(), "question card carries no questions");
    let first = &card.questions[0];
    println!(
        "card: header={:?} question={:?} options={:?}",
        first.header,
        first.question,
        first.options.iter().map(|o| &o.label).collect::<Vec<_>>()
    );
    let picked = first
        .options
        .iter()
        .map(|o| o.label.clone())
        .find(|l| l.contains("Cobalt"))
        .expect("the agent was told to offer a Cobalt option");

    let mut answers = std::collections::HashMap::new();
    answers.insert(first.question.clone(), picked.clone());
    // Every question needs a slot: dsh validates the answer array positionally.
    for q in card.questions.iter().skip(1) {
        answers.insert(q.question.clone(), String::new());
    }
    claw_fleet_core::elicitation::write_response(&claw_fleet_core::elicitation::ElicitationResponse {
        id: card.id.clone(),
        declined: false,
        answers,
    })
    .expect("write answer");

    let gone = wait_for(Duration::from_secs(30), || {
        claw_fleet_core::elicitation::read_request(&card.id)
            .is_none()
            .then_some(())
    });
    assert!(gone.is_some(), "bridge never consumed the question answer");

    // dsh accepted the answer only if the agent received it — the tool result
    // lands in the session's durable history as
    // `{"answers":[{"id":"colour","selected":["Cobalt"]}]}` (captured live).
    //
    // The match is narrowed to `tool/result` deliberately: the label also
    // appears in the agent's own `tool/call` arguments, so a search across all
    // events would pass on the *question* and prove nothing about the answer.
    let uri = format!("dsh://{session_id}");
    let saw_answer = wait_for(EFFECT_BUDGET, || {
        let events = source.get_messages(&uri).ok()?;
        events
            .iter()
            .filter(|e| e.get("type").and_then(|t| t.as_str()) == Some("tool/result"))
            .any(|e| serde_json::to_string(e).unwrap_or_default().contains(&picked))
            .then_some(())
    });
    assert!(
        saw_answer.is_some(),
        "answered {picked:?} but no tool/result carried it back to the agent"
    );
}
