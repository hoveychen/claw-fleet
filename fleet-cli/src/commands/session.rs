//! `fleet session idle` / `resume` — hook entrypoints called from inside a
//! managed session, plus the session-id resolution used across `fleet plan` /
//! `fleet handoff` / `fleet loop`.

/// `fleet session idle` — Stop-hook entrypoint. Marks the current session idle
/// so the supervisor's tick flips the kanban card from Running → Pending.
///
/// Also the background-task guard (see [`claw_fleet_core::bg_guard`]): when a
/// *headless* session tries to end a turn with background work still running,
/// this exits 2, which Claude Code reads as "don't stop" and feeds the stderr
/// text back to the agent as its next instruction. That keeps the CLI process —
/// and therefore the background shells, which a `-p` run kills ~5s after its
/// final result — alive until the agent collects the results in the foreground.
///
/// Silently exits 0 if FLEET_SESSION_ID is unset (e.g. user has global Fleet
/// hooks installed but ran claude outside the supervisor) — never blocks the hook.
pub(crate) fn cmd_session_idle() {
    // Hooks receive their payload as JSON on stdin. Guard against a terminal:
    // a hand-run `fleet session idle` has no piped stdin and would otherwise
    // block on a read that never EOFs (the Stop hook's budget is 5s).
    let stop_payload = {
        use std::io::{IsTerminal, Read};
        let mut buf = String::new();
        if !std::io::stdin().is_terminal() {
            let _ = std::io::stdin().read_to_string(&mut buf);
        }
        claw_fleet_core::bg_guard::parse_stop_payload(&buf)
    };
    let Some(sid) = read_fleet_session_id() else {
        return;
    };

    // Refuse the stop *before* marking idle or firing the relay: the turn isn't
    // actually over. Marking it idle would flip the kanban card to Pending while
    // the agent keeps working, and consuming the handoff here would hand a
    // successor work this session is about to resume.
    if let Some(payload) = &stop_payload {
        let headless = claw_fleet_core::session::is_headless_session(&sid);
        if let Some(reason) = claw_fleet_core::bg_guard::block_reason(payload, headless) {
            eprintln!("{reason}");
            std::process::exit(2);
        }
    }

    if let Err(e) = claw_fleet_core::idle::mark_idle(&sid) {
        eprintln!("fleet session idle: {e}");
    }
    // Handoff relay: the session just yielded its turn — exactly the moment a
    // registered handoff should fire. Errors are logged, never propagated:
    // the Stop hook must not block or fail the session over a relay problem.
    match claw_fleet_core::handoff::consume_and_spawn(&sid) {
        Ok(Some(to_sid)) => println!("handoff: successor session {to_sid} spawned"),
        Ok(None) => {}
        Err(e) => eprintln!("fleet session idle: handoff relay failed: {e}"),
    }

    // Loop reconcile: re-arm timers for any agent loop left stranded (its
    // detached timer died on a reboot / kill). Cheap, idempotent, duplicate-safe
    // via the loop's generation. Piggy-backs on the Stop hook rather than a
    // daemon so a loop resumes the next time any session yields, with the desktop
    // app closed — same rationale as the handoff relay above.
    let rearmed = claw_fleet_core::agent_loop::reconcile();
    if !rearmed.is_empty() {
        println!("loop: re-armed {} stranded timer(s): {}", rearmed.len(), rearmed.join(", "));
    }
}

/// `fleet session resume` — UserPromptSubmit-hook entrypoint. Clears the idle
/// sentinel so the supervisor flips the card back to Running on next tick.
pub(crate) fn cmd_session_resume() {
    let Some(sid) = read_fleet_session_id() else {
        return;
    };
    claw_fleet_core::idle::clear_idle(&sid);
    // A fresh user prompt means the user took the session back over — a still
    // pending (i.e. never-consumed) handoff is stale intent; drop it so it
    // can't fire surprisingly on a later Stop.
    claw_fleet_core::handoff::cancel_pending(&sid);
}

/// The current session's id, for attributing `fleet plan` / `fleet session`
/// actions. Prefers `FLEET_SESSION_ID` (set by the Fleet supervisor) and falls
/// back to `CLAUDE_CODE_SESSION_ID` (set by Claude Code for *every* session,
/// Fleet-managed or not). Both carry the same value under Fleet — the supervisor
/// launches claude with `--session-id <id>` and exports `FLEET_SESSION_ID=<id>`,
/// and that id equals the session's jsonl stem (i.e. `SessionInfo.id`), so the
/// side-channel key always matches what the card looks up.
pub(crate) fn read_fleet_session_id() -> Option<String> {
    resolve_session_id(
        std::env::var("FLEET_SESSION_ID").ok(),
        std::env::var("CLAUDE_CODE_SESSION_ID").ok(),
    )
}

/// Pure resolution: first non-empty of (`FLEET_SESSION_ID`, `CLAUDE_CODE_SESSION_ID`).
fn resolve_session_id(fleet: Option<String>, claude: Option<String>) -> Option<String> {
    fleet
        .filter(|s| !s.is_empty())
        .or_else(|| claude.filter(|s| !s.is_empty()))
}

#[cfg(test)]
mod session_id_tests {
    use super::resolve_session_id;

    #[test]
    fn falls_back_to_claude_code_session_id() {
        // Non-Fleet-managed session: FLEET_SESSION_ID absent, but Claude Code
        // always sets CLAUDE_CODE_SESSION_ID — attribution must still resolve.
        assert_eq!(
            resolve_session_id(None, Some("claude-sid".into())),
            Some("claude-sid".into())
        );
        // Empty FLEET_SESSION_ID also falls back.
        assert_eq!(
            resolve_session_id(Some("".into()), Some("claude-sid".into())),
            Some("claude-sid".into())
        );
        // FLEET_SESSION_ID wins when present.
        assert_eq!(
            resolve_session_id(Some("fleet-sid".into()), Some("claude-sid".into())),
            Some("fleet-sid".into())
        );
        // Neither → None.
        assert_eq!(resolve_session_id(None, None), None);
    }
}
