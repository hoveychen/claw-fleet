//! Hook entrypoints wired into Claude Code: the Bash guard, the AskUserQuestion
//! elicitation bridge, the ExitPlanMode plan-approval bridge, the `fleet__ask`
//! MCP server, plus the guard allow-list management subcommands.

// ── Guard CLI (hook entrypoint) ─────────────────────────────────────────────

fn is_headless_codex_source(source: Option<&str>) -> bool {
    source == Some("codex")
}

pub(crate) fn cmd_guard_list_rules(json: bool) {
    let rules = claw_fleet_core::audit::list_guard_allow_rules();
    if json {
        let body = serde_json::to_string_pretty(&rules)
            .unwrap_or_else(|_| "[]".to_string());
        println!("{}", body);
        return;
    }
    if rules.is_empty() {
        println!("No guard allow rules configured.");
        println!("(They get added when you click \"始终允许\" / \"Always allow\" on a guard card.)");
        return;
    }
    println!("{:<38}  {:<24}  {}", "ID", "SOURCE TAG", "PREFIX");
    for r in &rules {
        let tag = r.source_tag.as_deref().unwrap_or("-");
        println!("{:<38}  {:<24}  {}", r.id, tag, r.prefix);
    }
}

pub(crate) fn cmd_guard_remove_rule(id: &str) {
    match claw_fleet_core::audit::remove_guard_allow_rule(id) {
        Ok(()) => println!("Removed guard allow rule {}", id),
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

pub(crate) fn cmd_guard() {
    use claw_fleet_core::consumer_heartbeat;
    use claw_fleet_core::decision_panel_config::DecisionPanelConfig;
    use claw_fleet_core::guard::{
        self, GuardClassification, GuardDecision, GuardRequest, HookInput,
    };
    use std::io::Read;
    use std::time::Instant;

    // Read hook JSON from stdin.
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        // Can't read stdin — allow silently.
        return;
    }

    let hook_input: HookInput = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return, // Malformed input — allow silently.
    };
    // Claude can fall back to its native permission UI when Fleet has no live
    // consumer. Fleet-spawned Codex runs headlessly with approval bypassed, so
    // there is no native approver: Critical commands must fail closed instead.
    let agent_source = std::env::var("FLEET_AGENT_SOURCE").ok();
    let codex_fail_closed = is_headless_codex_source(agent_source.as_deref());

    // Classify the command.
    match guard::classify_hook_input(&hook_input) {
        GuardClassification::Allow => {
            // Not critical — allow. Codex's outer code-mode `exec` has no
            // description field, so inspect its rollout and inject the Rule 7
            // correction instead. Claude keeps the original Bash-description
            // reminder. Both paths are once-per-session and never block tools.
            let reminder = if codex_fail_closed {
                guard::missing_exec_note_reminder_output(&hook_input)
            } else {
                guard::missing_description_reminder_output(&hook_input)
            };
            if let Some(out) = reminder {
                println!("{out}");
            }
            return;
        }
        GuardClassification::NeedsConfirmation {
            command,
            risk_tags,
        } => {
            // No live consumer (Fleet app not running / no SSE client on
            // `fleet serve`) — fall through silently so Claude isn't blocked
            // by a request nobody will answer.
            let cfg = DecisionPanelConfig::load();
            let liveness_window = cfg.heartbeat_window();
            let status = consumer_heartbeat::consumer_status(liveness_window);
            if !status.is_alive() {
                if codex_fail_closed {
                    println!("{}", serde_json::json!({
                        "decision": "block",
                        "reason": "Fleet Guard: no live Fleet decision consumer for headless Codex"
                    }));
                    return;
                }
                claw_fleet_core::log_debug(&format!(
                    "[guard hook] no live consumer at startup (status={status}); falling back to Claude Code's native UI",
                ));
                return;
            }

            let session_id = hook_input
                .session_id
                .clone()
                .unwrap_or_else(|| "unknown".to_string());

            let request_id = guard::new_request_id();

            let mut structured = claw_fleet_core::cmd_ast::extract_structured_view(&command);
            let user_rules = claw_fleet_core::audit::load_user_rules();
            claw_fleet_core::audit::annotate_view_with_flags(&mut structured, &user_rules);

            let req = GuardRequest {
                id: request_id.clone(),
                session_id,
                workspace_name: String::new(), // Desktop app resolves from session_id
                ai_title: None, // Desktop app resolves from session_id
                tool_name: "Bash".to_string(),
                command: command.clone(),
                command_summary: guard::truncate_command(&command, 120),
                risk_tags,
                timestamp: chrono::Utc::now().to_rfc3339(),
                structured_command: Some(structured),
            };

            // Write request for the desktop app to pick up.
            if let Err(e) = guard::write_request(&req) {
                eprintln!("guard: failed to write request: {e}");
                // On error, block by default for Critical commands.
                let out = serde_json::json!({
                    "decision": "block",
                    "reason": "Fleet Guard: failed to communicate with Fleet app"
                });
                println!("{}", out);
                return;
            }

            // Poll for response (configured wait_seconds, default 600s),
            // bailing out early if the consumer dies mid-flight.
            let timeout = cfg.wait_duration();
            let poll_interval = cfg.poll_duration();
            let start = Instant::now();
            loop {
                if let Some(resp) = guard::try_read_response(&request_id) {
                    guard::cleanup(&request_id);
                    match resp.decision {
                        GuardDecision::Allow => {}
                        GuardDecision::Block => {
                            let user_reason = resp
                                .reason
                                .as_deref()
                                .map(str::trim)
                                .filter(|s| !s.is_empty());
                            let reason_text = match user_reason {
                                Some(r) => {
                                    format!("Fleet Guard: blocked by user — {r}")
                                }
                                None => "Fleet Guard: blocked by user".to_string(),
                            };
                            let out = serde_json::json!({
                                "decision": "block",
                                "reason": reason_text,
                            });
                            println!("{}", out);
                        }
                    }
                    return;
                }
                if start.elapsed() > timeout {
                    guard::cleanup(&request_id);
                    let out = serde_json::json!({
                        "decision": "block",
                        "reason": "Fleet Guard: no response from Fleet app (timeout)"
                    });
                    println!("{}", out);
                    return;
                }
                let status = consumer_heartbeat::consumer_status(liveness_window);
                if !status.is_alive() {
                    if codex_fail_closed {
                        guard::cleanup(&request_id);
                        println!("{}", serde_json::json!({
                            "decision": "block",
                            "reason": "Fleet Guard: Fleet decision consumer disconnected while headless Codex was waiting"
                        }));
                        return;
                    }
                    // Head went away while we waited — fall through silently.
                    claw_fleet_core::log_debug(&format!(
                        "[guard hook] consumer heartbeat lost after {:.1}s (request {}, status={status}); deleting request file and falling back to native UI",
                        start.elapsed().as_secs_f32(),
                        request_id,
                    ));
                    guard::cleanup(&request_id);
                    return;
                }
                std::thread::sleep(poll_interval);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_headless_codex_source;

    #[test]
    fn only_codex_guard_hook_requires_fail_closed_fallback() {
        assert!(is_headless_codex_source(Some("codex")));
        assert!(!is_headless_codex_source(Some("claude-code")));
        assert!(!is_headless_codex_source(None));
    }
}

// ── Elicitation CLI (hook entrypoint for AskUserQuestion) ───────────────

pub(crate) fn cmd_elicitation() {
    use claw_fleet_core::consumer_heartbeat;
    use claw_fleet_core::decision_history::{
        self, DecisionHistoryRecord, ElicitationOutcome, build_elicitation_record,
    };
    use claw_fleet_core::decision_panel_config::DecisionPanelConfig;
    use claw_fleet_core::elicitation::{self, ElicitationRequest};
    use claw_fleet_core::guard::{self, HookInput};
    use std::io::Read;
    use std::time::Instant;

    // Read hook JSON from stdin.
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return;
    }

    let hook_input: HookInput = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Only handle AskUserQuestion.
    let tool_name = hook_input.tool_name.as_deref().unwrap_or("");
    if tool_name != "AskUserQuestion" {
        return;
    }

    let tool_input = match &hook_input.tool_input {
        Some(v) => v.clone(),
        None => return,
    };

    // Parse questions from tool_input.
    let questions_val = match tool_input.get("questions") {
        Some(q) => q.clone(),
        None => return,
    };

    let questions: Vec<elicitation::ElicitationQuestion> =
        match serde_json::from_value(questions_val.clone()) {
            Ok(q) => q,
            Err(_) => return,
        };

    if questions.is_empty() {
        return;
    }

    // No live consumer — fall through silently; Claude Code will use its
    // native AskUserQuestion UI.
    let cfg = DecisionPanelConfig::load();
    let liveness_window = cfg.heartbeat_window();
    let startup_status = consumer_heartbeat::consumer_status(liveness_window);
    if !startup_status.is_alive() {
        claw_fleet_core::log_debug(&format!(
            "[elicitation hook] no live consumer at startup (status={startup_status}); falling back to Claude Code's native UI",
        ));
        return;
    }

    let session_id = hook_input
        .session_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    // This session already has a question parked and waiting for the user (an
    // earlier card timed out). Asking again is exactly what the stop notice
    // told the agent not to do — repeat the notice instead of queueing a second
    // card behind the first.
    if claw_fleet_core::parked::has_parked_for_session(&session_id) {
        println!("{}", deny_hook_output(claw_fleet_core::parked::STOP_NOTICE));
        return;
    }

    let request_id = guard::new_request_id();

    let req = ElicitationRequest {
        id: request_id.clone(),
        session_id,
        workspace_name: String::new(),
        ai_title: None,
        questions,
        timestamp: chrono::Utc::now().to_rfc3339(),
        parked: false,
    };

    if let Err(e) = elicitation::write_request(&req) {
        eprintln!("elicitation: failed to write request: {e}");
        // On error, deny so Claude Code falls back to its own UI.
        let out = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "Fleet: failed to communicate with Fleet app"
            }
        });
        println!("{}", out);
        return;
    }

    // Poll for response (configured wait_seconds, default 600s), bailing out
    // early if the consumer dies mid-flight so Claude Code can take over
    // with its native UI.
    let timeout = cfg.wait_duration();
    let poll_interval = cfg.poll_duration();
    let start = Instant::now();
    let resp = loop {
        if let Some(r) = elicitation::try_read_response(&request_id) {
            break Some(r);
        }
        if start.elapsed() > timeout {
            break None;
        }
        let loop_status = consumer_heartbeat::consumer_status(liveness_window);
        if !loop_status.is_alive() {
            // Head went away — silently fall through to Claude's native UI.
            claw_fleet_core::log_debug(&format!(
                "[elicitation hook] consumer heartbeat lost after {:.1}s (request {}, status={loop_status}); deleting request file and falling back to native UI",
                start.elapsed().as_secs_f32(),
                request_id,
            ));
            let rec = build_elicitation_record(
                &req,
                ElicitationOutcome::HeartbeatLost,
                &std::collections::HashMap::new(),
                chrono::Utc::now().to_rfc3339(),
            );
            if let Err(e) = decision_history::append_record(&DecisionHistoryRecord::Elicitation(rec)) {
                eprintln!("decision_history append (heartbeat-lost): {e}");
            }
            elicitation::cleanup(&request_id);
            return;
        }
        std::thread::sleep(poll_interval);
    };
    match resp {
        Some(resp) => {
            let outcome = if resp.declined {
                ElicitationOutcome::Declined
            } else {
                ElicitationOutcome::Answered
            };
            let rec = build_elicitation_record(
                &req,
                outcome,
                &resp.answers,
                chrono::Utc::now().to_rfc3339(),
            );
            if let Err(e) = decision_history::append_record(&DecisionHistoryRecord::Elicitation(rec)) {
                eprintln!("decision_history append: {e}");
            }
            elicitation::cleanup(&request_id);
            if resp.declined {
                // User declined — deny so Claude Code knows.
                let out = serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": "Fleet: user declined to answer"
                    }
                });
                println!("{}", out);
            } else {
                // Build updatedInput with original questions + user answers.
                let mut updated_input = tool_input.clone();
                updated_input["answers"] =
                    serde_json::to_value(&resp.answers).unwrap_or_default();
                let out = serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "allow",
                        "updatedInput": updated_input
                    }
                });
                println!("{}", out);
            }
        }
        None => {
            claw_fleet_core::log_debug(&format!(
                "[elicitation hook] timed out after {:.1}s waiting for user response (request {})",
                start.elapsed().as_secs_f32(),
                request_id,
            ));
            let rec = build_elicitation_record(
                &req,
                ElicitationOutcome::Timeout,
                &std::collections::HashMap::new(),
                chrono::Utc::now().to_rfc3339(),
            );
            if let Err(e) = decision_history::append_record(&DecisionHistoryRecord::Elicitation(rec)) {
                eprintln!("decision_history append (timeout): {e}");
            }
            elicitation::cleanup(&request_id);
            // Fleet-owned session: keep the question alive as a parked card and
            // SIGINT the turn that asked it, so the agent can't answer its own
            // question. The CLI may well be gone before this deny is read — the
            // parked card, already on disk, is what carries the state forward.
            if claw_fleet_core::parked::park_and_stop(
                &request_id,
                claw_fleet_core::parked::ParkedKind::Elicitation,
                &req.session_id,
                &req,
            ) {
                println!("{}", deny_hook_output(claw_fleet_core::parked::STOP_NOTICE));
                return;
            }
            // Everyone else: deny, and Claude Code falls back to its native UI.
            println!(
                "{}",
                deny_hook_output("Fleet: no response from Fleet app (timeout)")
            );
        }
    }
}

/// A `PreToolUse` deny verdict — the shape Claude Code reads off a hook's
/// stdout to block the tool call and hand `reason` to the model.
fn deny_hook_output(reason: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason
        }
    })
    .to_string()
}

// ── MCP server (stdio JSON-RPC; exposes `fleet__ask`) ──────────────────

pub(crate) fn cmd_mcp() {
    if let Err(e) = claw_fleet_core::mcp_server::run() {
        eprintln!("fleet mcp: stdio error: {e}");
        std::process::exit(1);
    }
}

// ── Plan-approval CLI (hook entrypoint for ExitPlanMode) ────────────────

// ── Wakeup guard (ScheduleWakeup / CronCreate interception) ─────────────────

/// PreToolUse hook for `ScheduleWakeup` / `CronCreate`. Denies them in
/// Fleet-spawned sessions, where they silently strand the turn, and names the
/// Fleet relay to use instead. See [`claw_fleet_core::wakeup_guard`].
///
/// Fails open at every step (unreadable stdin, malformed JSON, unresolvable
/// session id): a guard that can't tell what it's looking at must not block.
pub(crate) fn cmd_wakeup_guard() {
    use claw_fleet_core::guard::HookInput;
    use claw_fleet_core::{launch_spec, wakeup_guard};
    use std::io::Read;

    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return;
    }

    let hook_input: HookInput = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return,
    };

    // The hook payload's session_id is authoritative for "which session is this";
    // `was_fleet_spawned` is the ground truth for "did Fleet spawn it" (env vars
    // like CLAUDE_CODE_ENTRYPOINT leak into plain `claude -p` children).
    let session_id = hook_input.session_id.as_deref().unwrap_or("");
    if session_id.is_empty() {
        return;
    }
    let fleet_owned = launch_spec::was_fleet_spawned(session_id);

    let Some(reason) = wakeup_guard::decide(hook_input.tool_name.as_deref(), fleet_owned)
    else {
        return;
    };

    let out = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    });
    println!("{}", out);
}

pub(crate) fn cmd_plan_approval() {
    use claw_fleet_core::consumer_heartbeat;
    use claw_fleet_core::decision_history::{
        self, DecisionHistoryRecord, PlanApprovalOutcome, build_plan_approval_record,
    };
    use claw_fleet_core::decision_panel_config::DecisionPanelConfig;
    use claw_fleet_core::guard::{self, HookInput};
    use claw_fleet_core::plan_approval::{self, PlanApprovalRequest};
    use std::io::Read;
    use std::time::Instant;

    // Read hook JSON from stdin.
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return;
    }

    let hook_input: HookInput = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return,
    };

    // Only handle ExitPlanMode.
    let tool_name = hook_input.tool_name.as_deref().unwrap_or("");
    if tool_name != "ExitPlanMode" {
        return;
    }

    let tool_input = match &hook_input.tool_input {
        Some(v) => v.clone(),
        None => return,
    };

    // Pull plan content and file path. Claude Code's `normalizeToolInput`
    // injects these into the tool_input before hooks fire, so we can display
    // the plan without having to re-read from disk.
    let plan_content = tool_input
        .get("plan")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let plan_file_path = tool_input
        .get("planFilePath")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // No live consumer — fall through silently so Claude Code keeps its
    // native plan-approval UI as a fallback.
    let cfg = DecisionPanelConfig::load();
    let liveness_window = cfg.heartbeat_window();
    let startup_status = consumer_heartbeat::consumer_status(liveness_window);
    if !startup_status.is_alive() {
        claw_fleet_core::log_debug(&format!(
            "[plan-approval hook] no live consumer at startup (status={startup_status}); falling back to Claude Code's native UI",
        ));
        return;
    }

    let session_id = hook_input
        .session_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    // A parked question from an earlier timeout is still waiting for the user;
    // don't stack a plan card behind it (see the elicitation hook's guard).
    if claw_fleet_core::parked::has_parked_for_session(&session_id) {
        println!("{}", deny_hook_output(claw_fleet_core::parked::STOP_NOTICE));
        return;
    }

    let request_id = guard::new_request_id();

    let req = PlanApprovalRequest {
        id: request_id.clone(),
        session_id,
        workspace_name: String::new(),
        ai_title: None,
        plan_content,
        plan_file_path,
        timestamp: chrono::Utc::now().to_rfc3339(),
        parked: false,
    };

    if let Err(e) = plan_approval::write_request(&req) {
        eprintln!("plan-approval: failed to write request: {e}");
        // On error, deny so Claude Code falls back to its own UI.
        let out = serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "Fleet: failed to communicate with Fleet app"
            }
        });
        println!("{}", out);
        return;
    }

    // Poll for response (configured wait_seconds, default 600s), bailing out
    // early if the consumer dies mid-flight so Claude Code can take over
    // with its native UI.
    let timeout = cfg.wait_duration();
    let poll_interval = cfg.poll_duration();
    let start = Instant::now();
    let resp = loop {
        if let Some(r) = plan_approval::try_read_response(&request_id) {
            break Some(r);
        }
        if start.elapsed() > timeout {
            break None;
        }
        let loop_status = consumer_heartbeat::consumer_status(liveness_window);
        if !loop_status.is_alive() {
            claw_fleet_core::log_debug(&format!(
                "[plan-approval hook] consumer heartbeat lost after {:.1}s (request {}, status={loop_status}); deleting request file and falling back to native UI",
                start.elapsed().as_secs_f32(),
                request_id,
            ));
            let rec = build_plan_approval_record(
                &req,
                PlanApprovalOutcome::HeartbeatLost,
                None,
                chrono::Utc::now().to_rfc3339(),
            );
            if let Err(e) = decision_history::append_record(&DecisionHistoryRecord::PlanApproval(rec)) {
                eprintln!("decision_history append (heartbeat-lost): {e}");
            }
            plan_approval::cleanup(&request_id);
            return;
        }
        std::thread::sleep(poll_interval);
    };

    match resp {
        Some(resp) => {
            let outcome = if resp.decision == "approve" {
                if resp.edited_plan.is_some() {
                    PlanApprovalOutcome::ApprovedWithEdits
                } else {
                    PlanApprovalOutcome::Approved
                }
            } else {
                PlanApprovalOutcome::Rejected
            };
            let rec = build_plan_approval_record(
                &req,
                outcome,
                Some(&resp),
                chrono::Utc::now().to_rfc3339(),
            );
            if let Err(e) = decision_history::append_record(&DecisionHistoryRecord::PlanApproval(rec)) {
                eprintln!("decision_history append: {e}");
            }
            plan_approval::cleanup(&request_id);
            if resp.decision == "approve" {
                // Approve — optionally echo back an edited plan via updatedInput.
                let mut updated_input = tool_input.clone();
                if let Some(edited) = resp.edited_plan.as_deref() {
                    updated_input["plan"] = serde_json::Value::String(edited.to_string());
                }
                let out = serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "allow",
                        "updatedInput": updated_input
                    }
                });
                println!("{}", out);
            } else {
                // Reject — pass user feedback (if any) back to the model.
                let reason = resp
                    .feedback
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| "User rejected plan".to_string());
                let out = serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": reason
                    }
                });
                println!("{}", out);
            }
        }
        None => {
            claw_fleet_core::log_debug(&format!(
                "[plan-approval hook] timed out after {:.1}s waiting for user response (request {})",
                start.elapsed().as_secs_f32(),
                request_id,
            ));
            let rec = build_plan_approval_record(
                &req,
                PlanApprovalOutcome::Timeout,
                None,
                chrono::Utc::now().to_rfc3339(),
            );
            if let Err(e) = decision_history::append_record(&DecisionHistoryRecord::PlanApproval(rec)) {
                eprintln!("decision_history append (timeout): {e}");
            }
            plan_approval::cleanup(&request_id);
            // Fleet-owned session: the plan stays on the Decision Panel as a
            // parked card and this turn is cut short, rather than the agent
            // taking a timeout-deny as licence to start executing the plan.
            if claw_fleet_core::parked::park_and_stop(
                &request_id,
                claw_fleet_core::parked::ParkedKind::PlanApproval,
                &req.session_id,
                &req,
            ) {
                println!("{}", deny_hook_output(claw_fleet_core::parked::STOP_NOTICE));
                return;
            }
            println!(
                "{}",
                deny_hook_output("Fleet: no response from Fleet app (timeout)")
            );
        }
    }
}
