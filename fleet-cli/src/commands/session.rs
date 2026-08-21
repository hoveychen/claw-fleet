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

    // Timing starts once we have a session id to attribute it to. This hook runs
    // on a 5s Claude Code budget and gets SIGKILLed on overrun, so a `begin` with
    // no matching `end` in ~/.fleet/hook-timing.jsonl is the one durable signal
    // that it was killed rather than deciding not to act. Every return path below
    // ends the record explicitly.
    let mut timing = claw_fleet_core::hook_timing::HookTiming::begin("session-idle", &sid);
    // Decision inputs worth correlating with the timing when a turn slips
    // through: these are exactly what `block_reason` gates on.
    let (cron_count, bg_task_count) = stop_payload
        .as_ref()
        .map(|p| (p.session_crons.len(), p.background_tasks.len()))
        .unwrap_or((0, 0));

    // Refuse the stop *before* marking idle or firing the relay: the turn isn't
    // actually over. Marking it idle would flip the kanban card to Pending while
    // the agent keeps working, and consuming the handoff here would hand a
    // successor work this session is about to resume.
    if let Some(payload) = &stop_payload {
        // Prime suspect for budget overrun: this scans every process on the box.
        let headless = claw_fleet_core::session::is_headless_session(&sid);
        timing.phase("is_headless");
        if let Some(reason) = claw_fleet_core::bg_guard::block_reason(payload, headless) {
            // Explicitly before `exit`, which skips destructors.
            timing.end(
                2,
                serde_json::json!({
                    "blocked": true,
                    "is_headless": headless,
                    "session_crons": cron_count,
                    "background_tasks": bg_task_count,
                }),
            );
            eprintln!("{reason}");
            std::process::exit(2);
        }

        // Plan gate: same exit-2 channel, but for plan work rather than
        // background work. Must stay ahead of `mark_idle` (it compares against
        // the *previous* yield's timestamp) and ahead of the handoff relay (a
        // registered relay is one of its escape hatches). Unlike bg_guard this
        // is not headless-only: an interactive session drifting off a
        // half-finished plan tree is exactly the case it exists for.
        let cwd = if payload.cwd.is_empty() {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        } else {
            std::path::PathBuf::from(&payload.cwd)
        };
        if let Some(reason) =
            claw_fleet_core::plan_gate::gate_reason(&cwd, &sid, payload.stop_hook_active)
        {
            timing.end(
                2,
                serde_json::json!({
                    "blocked": true,
                    "blocked_by": "plan_gate",
                    "is_headless": headless,
                }),
            );
            eprintln!("{reason}");
            std::process::exit(2);
        }
    }

    if let Err(e) = claw_fleet_core::idle::mark_idle(&sid) {
        eprintln!("fleet session idle: {e}");
    }
    timing.phase("mark_idle");
    // Handoff relay: the session just yielded its turn — exactly the moment a
    // registered handoff should fire. Errors are logged, never propagated:
    // the Stop hook must not block or fail the session over a relay problem.
    let handoff_fired = match claw_fleet_core::handoff::consume_and_spawn(&sid) {
        Ok(Some(to_sid)) => {
            println!("handoff: successor session {to_sid} spawned");
            true
        }
        Ok(None) => false,
        Err(e) => {
            eprintln!("fleet session idle: handoff relay failed: {e}");
            false
        }
    };
    timing.phase("handoff");

    // Loop reconcile: re-arm timers for any agent loop left stranded (its
    // detached timer died on a reboot / kill). Cheap, idempotent, duplicate-safe
    // via the loop's generation. Piggy-backs on the Stop hook rather than a
    // daemon so a loop resumes the next time any session yields, with the desktop
    // app closed — same rationale as the handoff relay above.
    let rearmed = claw_fleet_core::agent_loop::reconcile();
    if !rearmed.is_empty() {
        println!("loop: re-armed {} stranded timer(s): {}", rearmed.len(), rearmed.join(", "));
    }
    timing.phase("loop_reconcile");

    // Watch reconcile: same contract as the loop reconcile above, for condition
    // watches whose detached timer died. Re-arms only watches whose heartbeat has
    // gone stale, so a watch keeps polling (and can still fire its resume) even
    // after a reboot, the next time any session yields.
    let watch_rearmed = claw_fleet_core::watch::reconcile();
    if !watch_rearmed.is_empty() {
        println!(
            "watch: re-armed {} stranded timer(s): {}",
            watch_rearmed.len(),
            watch_rearmed.join(", ")
        );
    }
    timing.phase("watch_reconcile");

    // Schedule reconcile: same contract as the loop/watch reconciles above, for
    // one-shot schedules whose detached timer died — and, crucially, for fires
    // the machine slept through: on the next yield after the due time, an overdue
    // pending schedule gets a fresh timer that fires it immediately.
    let schedule_rearmed = claw_fleet_core::schedule::reconcile();
    if !schedule_rearmed.is_empty() {
        println!(
            "schedule: re-armed {} stranded timer(s): {}",
            schedule_rearmed.len(),
            schedule_rearmed.join(", ")
        );
    }
    timing.phase("schedule_reconcile");

    timing.end(
        0,
        serde_json::json!({
            "blocked": false,
            "session_crons": cron_count,
            "background_tasks": bg_task_count,
            "handoff_fired": handoff_fired,
        }),
    );
    // Trim here rather than in `append`: once per invocation, on the path that
    // already finished its work, instead of on every line written.
    claw_fleet_core::hook_timing::maybe_truncate_timing_file();
}

/// `fleet session resume` — UserPromptSubmit-hook entrypoint. Clears the idle
/// sentinel so the supervisor flips the card back to Running on next tick.
pub(crate) fn cmd_session_resume() {
    let Some(sid) = read_fleet_session_id() else {
        return;
    };
    claw_fleet_core::idle::clear_idle(&sid);
    // Stamp when this turn began, so the plan gate can tell "the agent advanced
    // a plan during this turn" from "an old focus record is lying around" —
    // notably when the boss interrupts mid-plan to redirect.
    if let Err(e) = claw_fleet_core::idle::mark_turn_start(&sid) {
        eprintln!("fleet session resume: {e}");
    }
    // A fresh user prompt means the user took the session back over — a still
    // pending (i.e. never-consumed) handoff is stale intent; drop it so it
    // can't fire surprisingly on a later Stop.
    claw_fleet_core::handoff::cancel_pending(&sid);
}

/// `fleet session codex-notify <payload…>` — the codex analogue of Claude's
/// `Stop` hook, invoked via the `-c notify=[fleet,session,codex-notify]` that
/// Fleet injects on the codex sessions it spawns/resumes. Codex appends the
/// `agent-turn-complete` JSON payload as the final arg (plus, harmlessly, any
/// fixed args); we scan for the JSON, and if it is a turn-complete event we fire
/// the same turn-exit relay the Claude `Stop` hook does — mark idle, consume any
/// pending `fleet handoff`, re-arm stranded loops. Silent no-op for any other
/// payload (never blocks or fails codex over a relay problem).
pub(crate) fn cmd_codex_notify(payload: &[String]) {
    // Codex appends the JSON as one argv; scan defensively in case fixed args
    // precede it. The first arg that parses as a turn-complete event wins.
    let Some(payload_json) = payload
        .iter()
        .find(|arg| {
            claw_fleet_core::codex_launch::parse_agent_turn_complete_thread_id(arg).is_some()
        })
        .cloned()
    else {
        // Not a turn-complete payload — still chain-forward so the user's notify
        // sees every event codex would have delivered, then stop.
        forward_user_notify(payload);
        return;
    };
    let thread_id =
        claw_fleet_core::codex_launch::parse_agent_turn_complete_thread_id(&payload_json).unwrap();
    claw_fleet_core::codex_launch::on_codex_turn_exit(&thread_id);
    // Chain-forward: Fleet's `-c notify` override replaced the user's own notify
    // for this invocation, so re-invoke the user's command (from config.toml)
    // with the same payload — their notify still runs.
    forward_user_notify(payload);
}

/// Re-invoke the user's original codex `notify` command (if any) with the same
/// argv codex passed us, so Fleet's notify injection doesn't swallow it.
/// Fire-and-forget; failures are ignored (a broken user notify must not break
/// the relay).
fn forward_user_notify(payload: &[String]) {
    let Some(cmd) = claw_fleet_core::codex_launch::read_user_codex_notify() else {
        return;
    };
    let (prog, fixed) = cmd.split_first().unwrap(); // read_user_codex_notify never returns empty
    // Window-suppressed on Windows: this relay can run under a GUI-spawned
    // detached session, where a raw spawn would flash a conhost box.
    let _ = claw_fleet_core::process_util::command(prog)
        .args(fixed)
        .args(payload)
        .spawn();
}

/// The current session's id, for attributing `fleet plan` / `fleet session`
/// actions. Prefers `FLEET_SESSION_ID` (set by the Fleet supervisor and by the
/// Codex resume launcher) and falls back to `CLAUDE_CODE_SESSION_ID` (set by
/// Claude Code for *every* session, Fleet-managed or not). For a Fleet-spawned
/// **new** Codex session neither is set — Codex exposes no session-id env and
/// mints its thread id after spawn — so a third fallback resolves the
/// `FLEET_CODEX_LAUNCH_TOKEN` the spawn stamped back to the thread id Fleet
/// recorded once `thread.started` landed. The resolved id equals the session's
/// rollout id (i.e. `SessionInfo.id`), so the side-channel key always matches
/// what the card looks up.
pub(crate) fn read_fleet_session_id() -> Option<String> {
    // Single source of truth for the FLEET_SESSION_ID → CLAUDE_CODE_SESSION_ID →
    // FLEET_CODEX_LAUNCH_TOKEN precedence, shared with the `fleet mcp` decision
    // server so a `fleet plan` call and a decision card attribute to the same
    // session id across both Claude and Codex.
    claw_fleet_core::codex_launch::resolve_fleet_session_id_from_env()
}
