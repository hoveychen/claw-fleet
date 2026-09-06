//! Live proof that `$DSH_HOME/AGENTS.md` is a real injection point.
//!
//! Ignored by default: needs a real `dsh` binary and starts a real `dsh web`.
//!   FLEET_DSH_BIN=$(ls ~/.npm/_npx/*/node_modules/.bin/dsh | head -1) \
//!   cargo test -p claw-fleet-core --test dsh_guidance_live -- --ignored --nocapture
//!
//! The whole of [`claw_fleet_core::dsh_guidance`] rests on one claim: what Fleet
//! writes to that file reaches the model. The unit tests can only check the
//! file's *contents*; this checks that dsh actually reads it. No model
//! credentials are needed — the instruction baseline enters durable history
//! before the first request, so the assertion holds even when the request itself
//! fails for want of a key.

use std::time::{Duration, Instant};

use claw_fleet_core::agent_source::{AgentSource, SpawnSpec};
use claw_fleet_core::dsh_guidance::{reconcile_dsh_agents_md, DshGuidanceSet};
use claw_fleet_core::dsh_source::DshSource;

/// Stops Fleet's process-global `dsh web` however the test ends — it outlives
/// every `DshSource` by design, so a test binary must reclaim it itself.
struct ServerGuard;

impl Drop for ServerGuard {
    fn drop(&mut self) {
        claw_fleet_core::dsh_source::shutdown();
    }
}

#[test]
#[ignore = "starts a real dsh web against a temp DSH_HOME; run manually with --ignored"]
fn live_agents_md_reaches_the_session_as_a_durable_instruction() {
    let base = std::env::temp_dir().join(format!("fleet-dsh-guidance-live-{}", std::process::id()));
    let home = base.join("dsh-home");
    let ws = base.join("ws");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&ws).unwrap();

    // Repointed before the server starts: the child inherits this env, and the
    // agent-instructions plugin resolves `$DSH_HOME` the same way Fleet does.
    std::env::set_var("DSH_HOME", &home);

    reconcile_dsh_agents_md(
        DshGuidanceSet {
            prd: true,
            interaction: true,
            ..Default::default()
        },
        "老板",
        "zh",
    )
    .expect("write AGENTS.md");
    assert!(home.join("AGENTS.md").exists(), "reconcile must have written the file");

    let _guard = ServerGuard;
    let source = DshSource::new();
    let session_id = source
        .spawn(&SpawnSpec {
            workspace_path: ws.to_string_lossy().to_string(),
            prompt: "say hi".into(),
            ..Default::default()
        })
        .expect("spawn")
        .session_id
        .expect("spawn must report an id");

    // The baseline is composed on the first `agent/pre-step`, which happens a
    // beat after the prompt is admitted. Read it back the way the desktop does
    // — `get_messages` walks `session/page`, which since 0.1.2 needs a cursor
    // the follow stream publishes, so this also exercises that path.
    let uri = format!("dsh://{session_id}");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut events = Vec::new();
    while Instant::now() < deadline {
        events = source.get_messages(&uri).unwrap_or_default();
        if serde_json::to_string(&events)
            .unwrap_or_default()
            .contains("agent-instructions")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    let injected = events
        .iter()
        .find(|e| {
            serde_json::to_string(e)
                .unwrap_or_default()
                .contains("agent-instructions")
        })
        .unwrap_or_else(|| {
            panic!("no agent-instructions message in history: {} event(s)", events.len());
        });

    let text = serde_json::to_string(injected).unwrap_or_default();

    assert!(
        text.contains("<system-reminder>"),
        "the plugin owns the framing: {text}"
    );
    assert!(
        text.contains("Fleet PRD Discipline for dsh"),
        "the PRD block Fleet wrote must be in the injected text: {text}"
    );
    assert!(
        text.contains("ask_user_question"),
        "the interaction block must be there too: {text}"
    );
    assert!(
        text.contains("老板"),
        "the user title must survive into the model-visible text"
    );

    let _ = std::fs::remove_dir_all(&base);
}
