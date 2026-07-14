use super::*;

// ── CLI process scanning ─────────────────────────────────────────────────────

/// A running `claude` process discovered by sysinfo.
#[derive(Debug, Clone)]
pub struct CliProcess {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub cwd: String,
    /// Session ID parsed from `--resume <id>` or `--session-id <id>` in the
    /// process argv, if present. Fleet's own headless spawns always carry one
    /// of the two (launchpad spawns pass `--session-id`, follow-up turns pass
    /// `--resume`), so their pids resolve to exactly one session.
    pub resume_session_id: Option<String>,
    /// True when the process was launched in headless print mode (`-p` /
    /// `--print`) — the shape Fleet uses for every session it spawns.
    ///
    /// Load-bearing for the background-task guard: a headless run kills its
    /// background shells ~5s after the final result and nothing can re-invoke
    /// the model afterwards, whereas an interactive CLI keeps them alive across
    /// turns. See `crate::bg_guard`.
    pub headless: bool,
}

/// Was this `claude` process started in headless print mode?
///
/// Matches the flag as a standalone argv element, so a prompt that merely
/// *mentions* `-p` (prompts arrive as a single argv element) can't trip it.
///
/// Deliberately *not* `session_launch::is_fleet_owned_entrypoint`, which answers
/// a different question: that one asks "did Fleet spawn this?" (an ownership
/// check, used to gate interrupts) by reading the entrypoint stamped in the
/// transcript. The guard needs "will this process kill its background shells on
/// exit?", and that is a property of `-p`, not of who launched it. Fleet's hooks
/// are installed globally, so they also fire for a `claude -p` the user ran by
/// hand in a terminal — same dead end, no Fleet entrypoint.
pub(crate) fn is_headless_argv(cmd: &[std::ffi::OsString]) -> bool {
    cmd.iter()
        .any(|arg| arg == "-p" || arg == "--print")
}

pub(crate) fn extract_resume_id(cmd: &[std::ffi::OsString]) -> Option<String> {
    let mut iter = cmd.iter();
    while let Some(arg) = iter.next() {
        let s = arg.to_string_lossy();
        if s == "--resume" || s == "-r" || s == "--session-id" {
            return iter.next().map(|v| v.to_string_lossy().into_owned());
        }
        if let Some(val) = s.strip_prefix("--resume=") {
            return Some(val.to_owned());
        }
        if let Some(val) = s.strip_prefix("--session-id=") {
            return Some(val.to_owned());
        }
    }
    None
}

/// Resolve a PID for a specific session given all processes sharing the same cwd.
///
/// Matching priority (highest → lowest):
/// 1. Exact `--resume <session_id>` / `--session-id <session_id>` match →
///    always precise.
/// 2. Parent-child filtering: drop any claude process whose parent is also a
///    claude process in this workspace (those are subagent child processes).
///    If exactly one "root" process remains → precise.
/// 3. Single process → precise regardless.
/// 4. Multiple unresolvable processes → imprecise (first as representative).
pub(crate) fn resolve_pid(procs: &[CliProcess], session_id: &str) -> (Option<u32>, bool) {
    if procs.is_empty() {
        return (None, false);
    }

    // Rule 1: exact --resume match.
    if let Some(p) = procs.iter().find(|p| {
        p.resume_session_id.as_deref() == Some(session_id)
    }) {
        return (Some(p.pid), true);
    }

    // Rule 2: filter out child claude processes (subagents).
    // A process is a "child" if its parent PID is also in this workspace's process set.
    let pid_set: std::collections::HashSet<u32> = procs.iter().map(|p| p.pid).collect();
    let roots: Vec<&CliProcess> = procs.iter().filter(|p| {
        !p.ppid.map_or(false, |ppid| pid_set.contains(&ppid))
    }).collect();

    match roots.len() {
        0 => (Some(procs[0].pid), false), // shouldn't happen; fall back
        1 => (Some(roots[0].pid), true),
        _ => (Some(roots[0].pid), false), // still ambiguous after filtering
    }
}

/// Scan all running `claude` processes.
/// Uses sysinfo for cross-platform support (macOS, Linux, Windows).
pub fn scan_cli_processes() -> Vec<CliProcess> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let mut result = Vec::new();
    let mut sys = System::new();

    // Phase 1: scan all processes for cmd only (no cwd) to avoid triggering
    // macOS TCC permission dialogs for unrelated processes whose cwd may be
    // in protected directories (~/Documents, ~/Music, network volumes, etc.).
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always),
    );
    let matched_pids: Vec<_> = sys
        .processes()
        .iter()
        .filter(|(_, p)| {
            let name = p.name().to_string_lossy();
            name == "claude" || name == "claude.exe"
        })
        .map(|(pid, _)| *pid)
        .collect();

    // Phase 2: read cwd only for matched processes.
    if !matched_pids.is_empty() {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&matched_pids),
            true,
            ProcessRefreshKind::nothing()
                .with_cwd(UpdateKind::Always),
        );
    }

    for pid in &matched_pids {
        if let Some(process) = sys.process(*pid) {
            if let Some(cwd) = process.cwd() {
                if let Some(path) = cwd.to_str() {
                    let resume_session_id = extract_resume_id(process.cmd());
                    let headless = is_headless_argv(process.cmd());
                    let ppid = process.parent().map(|p| p.as_u32());
                    result.push(CliProcess {
                        pid: pid.as_u32(),
                        ppid,
                        cwd: path.to_string(),
                        resume_session_id,
                        headless,
                    });
                }
            }
        }
    }
    result
}

/// Is `session_id` being run by a headless (`claude -p`) process right now?
///
/// Pure half, so the decision is testable without real processes. Only an argv
/// that names this exact session counts — `resolve_pid`'s looser cwd-based
/// heuristics would happily hand back a *sibling* session's process, and
/// mistaking an interactive session for a headless one would block a turn that
/// had every right to end.
///
/// Unknown ⇒ `false`: when no process names the session (already exited, or the
/// scan came back empty), the guard stays out of the way. Failing to block costs
/// a lost background task; blocking by mistake wedges a session that was fine.
pub fn is_headless_session_in(procs: &[CliProcess], session_id: &str) -> bool {
    procs
        .iter()
        .find(|p| p.resume_session_id.as_deref() == Some(session_id))
        .map(|p| p.headless)
        .unwrap_or(false)
}

/// Live-process version of [`is_headless_session_in`], for hook entrypoints that
/// only know their own session id.
pub fn is_headless_session(session_id: &str) -> bool {
    is_headless_session_in(&scan_cli_processes(), session_id)
}

// ── IDE session scanning ─────────────────────────────────────────────────────

pub fn scan_ide_sessions(claude_dir: &Path) -> Vec<IdeSession> {
    let ide_dir = claude_dir.join("ide");
    let mut sessions = Vec::new();

    let Ok(entries) = fs::read_dir(&ide_dir) else {
        return sessions;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("lock") {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(lock): Result<LockFile, _> = serde_json::from_str(&content) else {
            continue;
        };
        if is_process_alive(lock.pid) {
            sessions.push(IdeSession {
                pid: lock.pid,
                workspace_folders: lock.workspace_folders,
                ide_name: lock.ide_name,
            });
        }
    }
    sessions
}

/// A workspace-level IDE lock describes the interactive session running
/// *inside* that IDE — not every session that happens to share the workspace.
/// The scan stamps the lock's `ide_name` per workspace, so without this pass a
/// launchpad-spawned headless session in a workspace that also has VS Code
/// open would wear a "Visual Studio Code" badge — and be skipped by
/// auto-resume, which treats `ide_name.is_some()` as "IDE-attached".
/// Fleet-owned entrypoints are headless by construction, so they never keep
/// the badge.
pub(crate) fn strip_ide_name_from_fleet_spawns(sessions: &mut [SessionInfo]) {
    for session in sessions {
        if crate::session_launch::is_fleet_owned_entrypoint(session.entrypoint.as_deref()) {
            session.ide_name = None;
        }
    }
}

// ── JSONL parsing ────────────────────────────────────────────────────────────

/// Compute seconds between now and the most recent `user` or `assistant`
/// entry's `timestamp` field. Returns `None` if no such entry exists or the
/// timestamp can't be parsed — callers should fall back to file mtime.
///
/// This is the key signal for distinguishing "session is fresh because the
/// user just replied" from "session is stale but mtime got bumped by
/// `claude --resume` appending `last-prompt` / `file-history-snapshot`
/// housekeeping records".
pub(crate) fn last_real_message_age_secs(last_lines: &[Value]) -> Option<f64> {
    let ts_str = last_lines.iter().rev().find_map(|v| {
        let t = v.get("type").and_then(|t| t.as_str())?;
        if t != "user" && t != "assistant" {
            return None;
        }
        v.get("timestamp").and_then(|t| t.as_str())
    })?;
    let ts = chrono::DateTime::parse_from_rfc3339(ts_str).ok()?;
    let now = chrono::Utc::now();
    let delta = (now - ts.with_timezone(&chrono::Utc)).num_milliseconds() as f64 / 1000.0;
    if delta < 0.0 { Some(0.0) } else { Some(delta) }
}

/// Detect a terminal `error: "rate_limit"` entry in the last assistant messages.
///
/// Claude Code persists API errors as synthetic assistant messages with
/// `isApiErrorMessage: true` and an `error` enum. When `rate_limit` is the
/// last such entry AND no subsequent real user/assistant turn has started,
/// the session is stuck waiting for quota reset. Returns `None` otherwise.
pub(crate) fn detect_rate_limit(last_lines: &[Value]) -> Option<RateLimitState> {
    // Walk from the end; stop at the first real (non-API-error) user/assistant
    // line — that means the user already resumed past the error.
    for v in last_lines.iter().rev() {
        let t = v.get("type").and_then(|t| t.as_str());
        if t != Some("assistant") && t != Some("user") {
            continue;
        }
        let is_api_err = v
            .get("isApiErrorMessage")
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
        if !is_api_err {
            // First real turn we hit going backwards is fresh activity —
            // any earlier rate_limit is stale.
            return None;
        }
        let err = v.get("error").and_then(|e| e.as_str());
        if err != Some("rate_limit") {
            // A different API error (auth, unknown, …) — not our concern.
            return None;
        }
        let text = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
            .and_then(|blocks| {
                blocks
                    .iter()
                    .find_map(|b| b.get("text").and_then(|t| t.as_str()))
            })
            .unwrap_or("");
        let ts_str = v.get("timestamp").and_then(|t| t.as_str())?;
        let error_timestamp = chrono::DateTime::parse_from_rfc3339(ts_str)
            .ok()?
            .with_timezone(&chrono::Utc);
        let parsed = crate::rate_limit_parser::parse_rate_limit_content(text, error_timestamp);
        return Some(RateLimitState {
            resets_at: parsed.resets_at,
            limit_type: parsed.limit_type,
            parsed: parsed.parsed,
            error_timestamp,
        });
    }
    None
}

/// True iff the last meaningful (user/assistant) record is a synthetic
/// "[Request interrupted by user]" / "...for tool use" user message, which
/// claude-code writes when Esc is pressed mid-turn.
fn is_last_meaningful_an_interrupt(last_lines: &[Value]) -> bool {
    let last = last_lines.iter().rev().find(|v| {
        matches!(
            v.get("type").and_then(|t| t.as_str()),
            Some("user") | Some("assistant")
        )
    });
    let Some(last) = last else { return false };
    if last.get("type").and_then(|t| t.as_str()) != Some("user") {
        return false;
    }
    let Some(blocks) = last
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    else {
        return false;
    };
    blocks.iter().any(|b| {
        b.get("type").and_then(|t| t.as_str()) == Some("text")
            && matches!(
                b.get("text").and_then(|t| t.as_str()),
                Some("[Request interrupted by user]")
                    | Some("[Request interrupted by user for tool use]")
            )
    })
}

/// Minutes-scale floor below which an unresolved tool batch is NOT treated as
/// stuck. Must be far longer than any real single tool round-trip (a WebFetch,
/// a Bash build, a WebSearch) so a merely-slow tool never trips it; it exists to
/// catch a batch that has been frozen for many minutes with a live process
/// behind it. Also longer than a typical subagent run to blunt the `Agent`
/// false-positive noted on [`has_pending_noninteractive_tool_batch`].
pub const STUCK_TOOL_BATCH_FLOOR_SECS: f64 = 1200.0; // 20 minutes

/// Tools that legitimately keep a turn open indefinitely while blocked on the
/// user — a decision card or a permission prompt. An unresolved `tool_use` for
/// one of these is a normal wait, never a deadlock, so it must NOT count toward
/// stuck detection.
fn is_interactive_wait_tool(name: &str) -> bool {
    name == "AskUserQuestion"
        || name == "ExitPlanMode"
        || name.ends_with("__ask") // mcp__fleet__fleet__ask
        || name.contains("permission") // mcp__fleet__fleet__permission_prompt
}

/// Detects a turn wedged mid tool-batch: the most recent assistant message that
/// issued `tool_use` blocks has at least one block whose `tool_use_id` never
/// received a matching `tool_result` in the records that follow it, AND that
/// unresolved block is a *non-interactive* tool.
///
/// This is the signal the plain status machine lacks. [`determine_status`] only
/// inspects the last user/assistant record's type + age, so a batch left one
/// result short — one tool hung and never wrote its `tool_result` (e.g. a
/// `WebFetch` whose timeout never fired) — reads as an ordinary quiet session.
/// The Anthropic Messages API requires every `tool_use_id` in a batch to have a
/// `tool_result` before the model is re-invoked, so such a session is deadlocked
/// inside the turn: the model never resumes and there is nothing to wake.
///
/// Pure over `last_lines`; the `proc_alive` + age-floor gate lives at the call
/// site ([`apply_pid_liveness`]).
///
/// Caveat: a legitimately long-running subagent (an `Agent` tool_use) also
/// presents as an unresolved batch while it runs. [`STUCK_TOOL_BATCH_FLOOR_SECS`]
/// (minutes, far longer than any real tool round-trip) is what keeps that from
/// flagging in the common case.
pub(crate) fn has_pending_noninteractive_tool_batch(last_lines: &[Value]) -> bool {
    let msg_blocks = |v: &Value| -> Option<Vec<Value>> {
        v.get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
            .cloned()
    };

    // Index of the last assistant message carrying >=1 tool_use block.
    let Some(asst_idx) = last_lines.iter().rposition(|v| {
        v.get("type").and_then(|t| t.as_str()) == Some("assistant")
            && msg_blocks(v).is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
            })
    }) else {
        return false;
    };

    // (tool_use_id, tool_name) issued by that assistant message.
    let issued: Vec<(String, String)> = msg_blocks(&last_lines[asst_idx])
        .unwrap_or_default()
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
        .filter_map(|b| {
            let id = b.get("id").and_then(|i| i.as_str())?.to_string();
            let name = b.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
            Some((id, name))
        })
        .collect();
    if issued.is_empty() {
        return false;
    }

    // tool_use_ids resolved by any tool_result in the records AFTER the batch.
    let mut resolved: std::collections::HashSet<String> = std::collections::HashSet::new();
    for v in &last_lines[asst_idx + 1..] {
        if let Some(blocks) = msg_blocks(v) {
            for b in &blocks {
                if b.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                    if let Some(id) = b.get("tool_use_id").and_then(|i| i.as_str()) {
                        resolved.insert(id.to_string());
                    }
                }
            }
        }
    }

    // Stuck iff some issued tool_use is unresolved AND non-interactive.
    issued
        .iter()
        .any(|(id, name)| !resolved.contains(id) && !is_interactive_wait_tool(name))
}

pub(crate) fn determine_status(
    last_lines: &[Value],
    file_age_secs: f64,
    content_age_secs: f64,
    hook_state: Option<&HookState>,
) -> SessionStatus {
    // Phase -1: Esc-interrupt detection.
    // When the user presses Esc, claude-code appends a synthetic user message
    // whose only text block is "[Request interrupted by user]" (or the
    // "...for tool use" variant). Without this short-circuit, the rest of the
    // pipeline misreads it: the user-as-last-message branch returns Thinking
    // for 120s, and a stale ModelProcessing hook pins the card to Thinking
    // even longer. Treat interrupt as terminal — fall straight through to
    // content-age aging (Active <30s, Idle thereafter).
    if is_last_meaningful_an_interrupt(last_lines) {
        if content_age_secs < 30.0 {
            return SessionStatus::Active;
        }
        return SessionStatus::Idle;
    }

    // Phase 0: Hook-based overrides for stale JSONL scenarios.
    // Hooks give us definitive signals that are more reliable than file-age guessing.
    // Only apply when the JSONL is not actively streaming (file_age >= 8s),
    // so we don't override fine-grained streaming detection.
    if file_age_secs >= 8.0 {
        match hook_state {
            Some(HookState::ToolExecuting) => return SessionStatus::Executing,
            Some(HookState::ModelProcessing) => return SessionStatus::Thinking,
            // Only trust the Stopped hook when a real turn completed recently.
            // A `--resume` of an old session fires Stop and appends housekeeping
            // records (last-prompt, file-history-snapshot) that bump mtime
            // without being a new turn, so `content_age` (time since last
            // real user/assistant message) is the correct freshness signal.
            Some(HookState::Stopped) if content_age_secs < 300.0 => {
                return SessionStatus::WaitingInput;
            }
            _ => {}
        }
    }

    if file_age_secs < 8.0 {
        // Find the current turn: everything after the last user message.
        let turn_start = last_lines
            .iter()
            .rposition(|v| v.get("type").and_then(|t| t.as_str()) == Some("user"))
            .map(|i| i + 1)
            .unwrap_or(0);

        // Look at the LAST incomplete (stop_reason=null) assistant message in the turn,
        // but only if no completed assistant message exists after it. Stale partials
        // left behind after a completed response must not override the final status.
        let last_partial_idx = last_lines[turn_start..].iter().rposition(|v| {
            if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
                return false;
            }
            let stop = v
                .get("message")
                .and_then(|m| m.get("stop_reason"));
            // stop_reason absent or null → still streaming
            stop.map_or(true, |s| s.is_null())
        });

        // Check whether a completed assistant message appears after the last partial.
        // If so, the partial is stale and should be ignored.
        let last_partial = last_partial_idx.and_then(|pidx| {
            let abs_pidx = turn_start + pidx;
            let has_completed_after = last_lines[abs_pidx + 1..].iter().any(|v| {
                if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
                    return false;
                }
                let stop = v
                    .get("message")
                    .and_then(|m| m.get("stop_reason"));
                // stop_reason present and non-null → completed
                stop.map_or(false, |s| !s.is_null())
            });
            if has_completed_after {
                None
            } else {
                Some(&last_lines[abs_pidx])
            }
        });

        if let Some(partial) = last_partial {
            let block_types: Vec<&str> = partial
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|b| b.get("type").and_then(|t| t.as_str()))
                        .collect()
                })
                .unwrap_or_default();

            if block_types.contains(&"thinking") {
                return SessionStatus::Thinking;
            }
            if block_types.contains(&"tool_use") {
                return SessionStatus::Executing;
            }
            return SessionStatus::Streaming;
        }

        // No incomplete message found — model may have just finished writing.
        // Fall through to check stop_reason of the last complete message.
    }

    // Check what the last meaningful line is to distinguish "tool executing" vs "model thinking".
    let last_meaningful = last_lines.iter().rev().find(|v| {
        matches!(
            v.get("type").and_then(|t| t.as_str()),
            Some("user") | Some("assistant")
        )
    });

    if let Some(last) = last_meaningful {
        let last_type = last.get("type").and_then(|t| t.as_str());

        if last_type == Some("user") {
            // Last write was a user message — the model is thinking about it.
            // This covers both: tool_result received (model thinking after tool execution)
            // and fresh user message (model doing initial/extended thinking before first write).
            // Use content_age so a --resume touching mtime doesn't fake thinking.
            if content_age_secs < 120.0 {
                return SessionStatus::Thinking;
            }
        }

        if last_type == Some("assistant") {
            let stop_value = last
                .get("message")
                .and_then(|m| m.get("stop_reason"));
            let stop_reason = stop_value.and_then(|s| s.as_str());
            let stop_is_null = stop_value.map_or(true, |s| s.is_null());

            if stop_is_null && file_age_secs < 120.0 {
                // Still streaming (stop_reason absent or null).
                // Check content blocks to determine what the model is outputting.
                let block_types: Vec<&str> = last
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter_map(|b| b.get("type").and_then(|t| t.as_str()))
                            .collect()
                    })
                    .unwrap_or_default();

                if block_types.contains(&"tool_use") {
                    return SessionStatus::Executing;
                }
                if block_types.contains(&"thinking") {
                    return SessionStatus::Thinking;
                }
                return SessionStatus::Streaming;
            }

            match stop_reason {
                // Content-age (not file mtime) governs WaitingInput so a
                // `claude --resume` that only touches mtime cannot flip an
                // old dormant session into "waiting for user input".
                Some("end_turn" | "max_tokens" | "stop_sequence") if content_age_secs < 300.0 => {
                    return SessionStatus::WaitingInput;
                }
                // Last write was a tool_use — the tool is still executing.
                Some("tool_use") if content_age_secs < 60.0 => return SessionStatus::Executing,
                _ => {}
            }
        }
    }

    if content_age_secs < 30.0 {
        SessionStatus::Active
    } else {
        SessionStatus::Idle
    }
}

