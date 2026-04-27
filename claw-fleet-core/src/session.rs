use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hooks::HookState;

// ── Lock file ────────────────────────────────────────────────────────────────

#[derive(Deserialize, Clone, Debug)]
pub struct LockFile {
    pub pid: u32,
    #[serde(rename = "workspaceFolders", default)]
    pub workspace_folders: Vec<String>,
    #[serde(rename = "ideName", default)]
    pub ide_name: String,
}

pub struct IdeSession {
    pub pid: u32,
    pub workspace_folders: Vec<String>,
    pub ide_name: String,
}

// ── Exported types ───────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    Thinking,     // streaming: last partial assistant msg has thinking blocks
    Executing,    // streaming: last partial assistant msg has tool_use blocks
    Streaming,    // file written < 2s ago (text output)
    Delegating,   // main session with at least one active subagent
    Processing,   // last stop_reason = tool_use, recent activity (waiting for tool result)
    WaitingInput, // last stop_reason = end_turn
    Active,       // file written < 30s ago
    Idle,         // no recent activity
    RateLimited,  // last assistant message was isApiErrorMessage + error=rate_limit;
                  // details (resets_at, limit_type) live on SessionInfo.rate_limit
}

/// Populated when `SessionStatus::RateLimited`. Carries the information needed
/// for the UI countdown and the auto-resume scheduler. `parsed` is `false` when
/// `resets_at` is an estimate derived from `error_timestamp + fallback_duration`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitState {
    pub resets_at: chrono::DateTime<chrono::Utc>,
    pub limit_type: crate::rate_limit_parser::RateLimitType,
    pub parsed: bool,
    pub error_timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub workspace_path: String,
    pub workspace_name: String,
    pub ide_name: Option<String>,
    pub is_subagent: bool,
    pub parent_session_id: Option<String>,
    pub agent_type: Option<String>,
    pub agent_description: Option<String>,
    pub slug: Option<String>,
    pub ai_title: Option<String>,
    pub status: SessionStatus,
    pub token_speed: f64,
    pub total_output_tokens: u64,
    /// Cumulative USD cost for this session alone (main or subagent).
    pub total_cost_usd: f64,
    /// Cost of this session + all its subagents' costs (main sessions only).
    /// For subagents this equals `total_cost_usd`.
    pub agent_total_cost_usd: f64,
    /// USD/min cost rate over the last 5-minute window.
    pub cost_speed_usd_per_min: f64,
    pub last_message_preview: Option<String>,
    pub last_activity_ms: u64,
    pub created_at_ms: u64,
    pub jsonl_path: String,
    pub model: Option<String>,
    pub thinking_level: Option<String>,
    pub pid: Option<u32>,
    /// True when the PID is unambiguously matched to this specific session.
    /// False when multiple claude processes share the same cwd and none carries
    /// a matching --resume flag — stopping may affect sibling sessions.
    pub pid_precise: bool,
    pub last_skill: Option<String>,
    /// Approximate context-window utilisation (0.0 – 1.0) derived from the
    /// last finalized assistant message's usage fields.  `None` when no
    /// usage data is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_percent: Option<f64>,
    /// Source of this session: "claude-code" or "cursor"
    pub agent_source: String,
    /// Semantic outcome tags from the last completed turn (e.g. "bug_fixed",
    /// "needs_input").  Set by background analysis, cleared when a new turn
    /// starts.  `None` means no analysis has run yet or the session is busy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<Vec<String>>,
    /// Populated when `status == RateLimited`. Carries reset time and limit
    /// type for the UI countdown and the auto-resume scheduler.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rate_limit: Option<RateLimitState>,
    /// Snapshot of the most recent TodoWrite invocation (`None` = session has
    /// never invoked TodoWrite).  Drives the compact progress row on the
    /// session card.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub todos: Option<crate::session_todos::TodoSummary>,
    /// Number of times this session was context-compacted (auto or manual /compact).
    #[serde(default)]
    pub compact_count: u32,
    /// Sum of context sizes (in tokens) right before each compaction.
    #[serde(default)]
    pub compact_pre_tokens: u64,
    /// Sum of summary sizes (in tokens) produced by each compaction.
    #[serde(default)]
    pub compact_post_tokens: u64,
    /// Estimated USD cost of the compact LLM calls. Approximation —
    /// the compact invocation is not recorded as a standalone assistant
    /// turn, so this is computed as `cache_read_price × pre + output_price × post`.
    #[serde(default)]
    pub compact_cost_usd: f64,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Returns the real user home directory, bypassing sandbox container redirection.
///
/// In a sandboxed macOS app, `dirs::home_dir()` returns the container path
/// (`~/Library/Containers/<id>/Data/`).  This function uses `getpwuid` to
/// obtain the real home from the passwd database so that `~/.claude/`,
/// `~/.fleet/`, `~/.ssh/` etc. resolve to the actual user directories.
///
/// For integration tests, setting `FLEET_HOME` overrides the detected home
/// so the test can operate on a temp dir without touching the real user's
/// `~/.fleet/`. Intended for tests only — production code never sets it.
pub fn real_home_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("FLEET_HOME") {
        let path = PathBuf::from(dir);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CStr;
        let pw = unsafe { libc::getpwuid(libc::getuid()) };
        if !pw.is_null() {
            let home = unsafe { CStr::from_ptr((*pw).pw_dir) };
            return Some(PathBuf::from(home.to_string_lossy().into_owned()));
        }
    }
    dirs::home_dir()
}

pub fn get_claude_dir() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".claude"))
}

/// Fleet's own data directory: `~/.fleet/`.
pub fn get_fleet_dir() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".fleet"))
}

#[cfg(unix)]
pub fn is_process_alive(pid: u32) -> bool {
    let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if ret == 0 {
        return true;
    }
    // EPERM = process exists but we lack permission to signal it → alive
    // ESRCH = no such process → dead
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(not(unix))]
pub fn is_process_alive(_pid: u32) -> bool {
    true // Fallback: assume alive
}

fn decode_workspace_path(encoded: &str) -> String {
    // "-Users-hoveychen-workspace-netferry" → "/Users/hoveychen/workspace/netferry"
    //
    // Dashes are ambiguous (path separator vs literal dash in a directory name).
    // We resolve this by greedily checking the filesystem: at each level, try the
    // longest remaining dash-joined segment first, and shorten until we find a real
    // directory.  Fall back to the naive one-dash-per-slash decode if nothing matches.
    let stripped = encoded.trim_start_matches('-');
    let parts: Vec<&str> = stripped.split('-').collect();
    if parts.is_empty() {
        return "/".to_string();
    }
    decode_workspace_path_with_parts(&parts)
}

pub fn decode_workspace_path_with_parts(parts: &[&str]) -> String {
    let mut current = String::new(); // built path so far (e.g. "/Users/hoveychen")
    let mut i = 0;
    while i < parts.len() {
        // Try longest remaining segment first: join parts[i..] with '-', then parts[i..len-1], etc.
        let mut matched = false;
        // Try from longest (all remaining parts) down to single part
        for end in (i + 1..=parts.len()).rev() {
            let candidate_segment = parts[i..end].join("-");
            let candidate_path = format!("{}/{}", current, candidate_segment);
            // Use TCC-safe exists check to avoid triggering macOS permission
            // dialogs for protected directories (~/Music, ~/Pictures, etc.).
            if crate::tcc::safe_exists(std::path::Path::new(&candidate_path)) {
                current = candidate_path;
                i = end;
                matched = true;
                break;
            }
        }
        if !matched {
            // Nothing exists on disk — use single part (original naive behavior)
            current = format!("{}/{}", current, parts[i]);
            i += 1;
        }
    }
    current
}

fn encode_workspace_path(path: &str) -> String {
    // "/Users/foo/bar-baz" → "-Users-foo-bar-baz"  (inverse of decode, but lossless for matching)
    path.replace('/', "-")
}

fn workspace_name(path: &str) -> String {
    path.split('/')
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or(path)
        .to_string()
}

// ── CLI process scanning ─────────────────────────────────────────────────────

/// A running `claude` process discovered by sysinfo.
#[derive(Debug, Clone)]
pub struct CliProcess {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub cwd: String,
    /// Session ID parsed from `--resume <id>` in the process argv, if present.
    pub resume_session_id: Option<String>,
}

fn extract_resume_id(cmd: &[std::ffi::OsString]) -> Option<String> {
    let mut iter = cmd.iter();
    while let Some(arg) = iter.next() {
        let s = arg.to_string_lossy();
        if s == "--resume" || s == "-r" {
            return iter.next().map(|v| v.to_string_lossy().into_owned());
        }
        if let Some(val) = s.strip_prefix("--resume=") {
            return Some(val.to_owned());
        }
    }
    None
}

/// Resolve a PID for a specific session given all processes sharing the same cwd.
///
/// Matching priority (highest → lowest):
/// 1. Exact `--resume <session_id>` match → always precise.
/// 2. Parent-child filtering: drop any claude process whose parent is also a
///    claude process in this workspace (those are subagent child processes).
///    If exactly one "root" process remains → precise.
/// 3. Single process → precise regardless.
/// 4. Multiple unresolvable processes → imprecise (first as representative).
fn resolve_pid(procs: &[CliProcess], session_id: &str) -> (Option<u32>, bool) {
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
                    let ppid = process.parent().map(|p| p.as_u32());
                    result.push(CliProcess {
                        pid: pid.as_u32(),
                        ppid,
                        cwd: path.to_string(),
                        resume_session_id,
                    });
                }
            }
        }
    }
    result
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

// ── JSONL parsing ────────────────────────────────────────────────────────────

/// Compute seconds between now and the most recent `user` or `assistant`
/// entry's `timestamp` field. Returns `None` if no such entry exists or the
/// timestamp can't be parsed — callers should fall back to file mtime.
///
/// This is the key signal for distinguishing "session is fresh because the
/// user just replied" from "session is stale but mtime got bumped by
/// `claude --resume` appending `last-prompt` / `file-history-snapshot`
/// housekeeping records".
fn last_real_message_age_secs(last_lines: &[Value]) -> Option<f64> {
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
fn detect_rate_limit(last_lines: &[Value]) -> Option<RateLimitState> {
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

fn determine_status(
    last_lines: &[Value],
    file_age_secs: f64,
    content_age_secs: f64,
    hook_state: Option<&HookState>,
) -> SessionStatus {
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

// ── Context window helpers ────────────────────────────────────────────────────

/// Whether a Claude model belongs to the family that can be opted in to a
/// 1M-token context window (Sonnet 4.x and Opus 4.6, per
/// [`modelSupports1M`](../claude-code-fork/src/utils/context.ts) in
/// Claude Code). Other Claude families are always 200K.
fn claude_model_supports_1m(model_lower: &str) -> bool {
    model_lower.contains("sonnet-4") || model_lower.contains("opus-4-6")
}

/// Best-effort lookup of a model's input-context-window size (in tokens).
///
/// `observed_max_input_tokens` is the max `input + cache_creation + cache_read`
/// seen across all assistant turns in the session. We use it as a fallback
/// signal because Claude Code **never writes the `[1m]` flag to the on-disk
/// transcript** — it lives only in the running process. So if a session has
/// any turn whose total input clearly exceeds the 200K window AND the model
/// is from a 1M-capable family, we know it must be a 1M session.
///
/// Pass `0` if you don't have this information; you'll get the conservative
/// 200K window.
///
/// Returns `None` when the model family is unrecognised so the caller can
/// decide whether to fall back to a default or skip the computation entirely.
pub fn context_window_for_model(model: &str, observed_max_input_tokens: u64) -> Option<u64> {
    let m = model.to_lowercase();

    // ── Anthropic / Claude ──────────────────────────────────────────────
    if m.starts_with("claude-") || m == "opus" || m == "sonnet" || m == "haiku" {
        // Explicit `[1m]` suffix — the canonical Claude Code marker.
        // Doesn't appear in JSONL today, but kept for correctness if that
        // ever changes.
        if m.contains("[1m]") {
            return Some(1_000_000);
        }
        // Inferred 1M: a turn's total input exceeds the 200K window. Only
        // valid for families that actually support 1M; others stay at 200K
        // (and the over-200K reading would itself indicate a bug elsewhere).
        // Threshold is a hair under 200K to absorb tokenizer rounding.
        if observed_max_input_tokens > 195_000 && claude_model_supports_1m(&m) {
            return Some(1_000_000);
        }
        // All other Claude 3 / 3.5 / 4.x models: 200 000 input tokens.
        return Some(200_000);
    }

    // ── OpenAI ──────────────────────────────────────────────────────────
    // o3 / o4-mini: 200 000
    if m.starts_with("o3") || m.starts_with("o4") {
        return Some(200_000);
    }
    // GPT-4o / GPT-4o-mini: 128 000
    if m.starts_with("gpt-4o") {
        return Some(128_000);
    }
    // GPT-4.1: 1 048 576
    if m.starts_with("gpt-4.1") {
        return Some(1_048_576);
    }
    // GPT-4-turbo / GPT-4-1106+: 128 000
    if m.starts_with("gpt-4-turbo") || m.starts_with("gpt-4-1106") || m.starts_with("gpt-4-0125") {
        return Some(128_000);
    }
    // GPT-4 (base 8k)
    if m.starts_with("gpt-4") {
        return Some(8_192);
    }

    // ── Google ──────────────────────────────────────────────────────────
    if m.contains("gemini") {
        return Some(1_000_000);
    }

    None
}

/// Compute context-window utilisation (0.0 – 1.0) from raw token counts.
///
/// `observed_max_input_tokens` is the largest single-turn total input
/// (`input + cache_creation + cache_read`) seen across the session. It feeds
/// the 1M-context inference in [`context_window_for_model`]. Pass `0` if
/// unknown; the result will use the conservative 200K window for Claude.
///
/// Returns `None` when the model is unrecognised.
pub fn compute_context_percent(
    input_tokens: u64,
    model: Option<&str>,
    observed_max_input_tokens: u64,
) -> Option<f64> {
    let window = context_window_for_model(model?, observed_max_input_tokens)?;
    if window == 0 {
        return None;
    }
    Some((input_tokens as f64 / window as f64).min(1.0))
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SessionStats {
    /// Tokens/sec over the last 5-minute window.
    pub token_speed: f64,
    /// Cumulative output tokens across all finalized assistant turns.
    pub total_output_tokens: u64,
    /// Cumulative USD cost across all finalized assistant turns.
    pub total_cost_usd: f64,
    /// USD/min over the last 5-minute window.
    pub cost_speed_usd_per_min: f64,
    /// Number of `compact_boundary` events in the transcript.
    pub compact_count: u32,
    /// Sum of `compactMetadata.preTokens` across all compact events
    /// (context size before each compaction).
    pub compact_pre_tokens: u64,
    /// Sum of `compactMetadata.postTokens` across all compact events
    /// (summary size produced by each compaction).
    pub compact_post_tokens: u64,
    /// Estimated USD cost spent on compact LLM calls. The compact
    /// invocation itself is not recorded as a separate assistant turn,
    /// so we approximate as `cache_read_price × preTokens +
    /// output_price × postTokens` using the model that was active just
    /// before each compaction.
    pub compact_cost_usd: f64,
}

fn compute_session_stats(lines: &[&str]) -> SessionStats {
    use crate::model_cost::{get_model_costs, turn_cost_usd, TurnUsage};

    let mut total_output: u64 = 0;
    let mut total_cost: f64 = 0.0;
    // (timestamp_secs, output_tokens, turn_cost_usd)
    let mut timed: Vec<(f64, u64, f64)> = Vec::new();
    let mut seen_msg_ids: HashSet<String> = HashSet::new();
    let mut last_model: Option<String> = None;

    let mut compact_count: u32 = 0;
    let mut compact_pre_tokens: u64 = 0;
    let mut compact_post_tokens: u64 = 0;
    let mut compact_cost_usd: f64 = 0.0;

    for line in lines {
        let Ok(v): Result<Value, _> = serde_json::from_str(line) else {
            continue;
        };

        // `compact_boundary` is a system meta event Claude Code emits each time
        // it summarises the conversation. The summary LLM call itself is not
        // logged as a standalone assistant turn, so its true cost is not in
        // the transcript — we approximate from `compactMetadata`.
        if v.get("type").and_then(|t| t.as_str()) == Some("system")
            && v.get("subtype").and_then(|s| s.as_str()) == Some("compact_boundary")
        {
            compact_count += 1;
            let meta = v.get("compactMetadata");
            let pre = meta
                .and_then(|m| m.get("preTokens"))
                .and_then(|t| t.as_u64())
                .unwrap_or(0);
            let post = meta
                .and_then(|m| m.get("postTokens"))
                .and_then(|t| t.as_u64())
                .unwrap_or(0);
            compact_pre_tokens += pre;
            compact_post_tokens += post;

            // Price the compact call against the most recently seen model.
            // `get_model_costs("")` falls back to the default tier when no
            // assistant turn has been seen yet (defensive — compact almost
            // never precedes the first assistant turn).
            let pricing_model = last_model.as_deref().unwrap_or("");
            let costs = get_model_costs(pricing_model);
            compact_cost_usd += (pre as f64 / 1_000_000.0) * costs.cache_read
                + (post as f64 / 1_000_000.0) * costs.output;
            continue;
        }

        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(msg) = v.get("message").and_then(|m| m.as_object()) else {
            continue;
        };
        // Only count finalized messages
        if msg.get("stop_reason").map_or(true, |s| s.is_null()) {
            continue;
        }
        let msg_id = msg
            .get("id")
            .and_then(|i| i.as_str())
            .unwrap_or_default()
            .to_string();
        if !msg_id.is_empty() {
            if seen_msg_ids.contains(&msg_id) {
                continue;
            }
            seen_msg_ids.insert(msg_id);
        }

        let usage = msg.get("usage");
        let input_tokens = usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let output_tokens = usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cache_creation_tokens = usage
            .and_then(|u| u.get("cache_creation_input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cache_read_tokens = usage
            .and_then(|u| u.get("cache_read_input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let web_search_requests = usage
            .and_then(|u| u.get("server_tool_use"))
            .and_then(|s| s.get("web_search_requests"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);

        total_output += output_tokens;

        // Per-turn cost uses this turn's own model; fall back to most-recently-
        // seen model when a turn omits it (model can change mid-session).
        let turn_model = msg.get("model").and_then(|m| m.as_str());
        if let Some(m) = turn_model {
            last_model = Some(m.to_string());
        }
        let cost_model = turn_model.or(last_model.as_deref()).unwrap_or("");
        let turn_cost = turn_cost_usd(
            cost_model,
            &TurnUsage {
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cache_read_tokens,
                web_search_requests,
            },
        );
        total_cost += turn_cost;

        // Timestamp for speed
        if let Some(ts_str) = v.get("timestamp").and_then(|t| t.as_str()) {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                timed.push((dt.timestamp() as f64, output_tokens, turn_cost));
            }
        }
    }

    // Speed: tokens/s and cost/min over the last 5-minute window.
    //
    // Divide by `now - first_ts`, not `last_ts - first_ts`. The inter-turn
    // gap version makes speed a step function that holds the old rate until
    // the oldest turn slides out of the window — so a session that finished
    // a burst 4 minutes ago still reports the burst's speed, inflating
    // fleet-wide totals while nothing is actually generating. Measuring
    // against "now" lets speed decay smoothly as the idle tail grows.
    let (token_speed, cost_speed_usd_per_min) = if timed.len() >= 2 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let window_start = now - 300.0;

        let recent: Vec<_> = timed.iter().filter(|(ts, _, _)| *ts > window_start).collect();

        if recent.len() >= 2 {
            let total_recent_tokens: u64 = recent.iter().map(|(_, t, _)| t).sum();
            let total_recent_cost: f64 = recent.iter().map(|(_, _, c)| c).sum();
            let first_ts = recent.first().map(|(ts, _, _)| *ts).unwrap_or(0.0);
            let duration = now - first_ts;
            if duration > 0.0 {
                (
                    total_recent_tokens as f64 / duration,
                    total_recent_cost * 60.0 / duration,
                )
            } else {
                (0.0, 0.0)
            }
        } else {
            (0.0, 0.0)
        }
    } else {
        (0.0, 0.0)
    };

    SessionStats {
        token_speed,
        total_output_tokens: total_output,
        total_cost_usd: total_cost,
        cost_speed_usd_per_min,
        compact_count,
        compact_pre_tokens,
        compact_post_tokens,
        compact_cost_usd,
    }
}

/// Extract context-window usage from a Claude-Code JSONL session.
///
/// Returns `(input_tokens_used, model_name, session_max_input_tokens)` for
/// the **most recent assistant turn** — scanning backward, matching Claude
/// Code's own `getCurrentUsage()` in `claude-code-fork/src/utils/tokens.ts`.
///
/// `session_max_input_tokens` is the largest single-turn total input ever
/// seen in the session (across all turns, not just the latest). It feeds
/// 1M-context inference downstream because the JSONL never records the
/// `[1m]` flag — see [`context_window_for_model`].
///
/// Key behaviors (by intent, not accident):
///
/// 1. **Backward scan.** Walk lines from the end. This is what Claude Code
///    does; forward-scan "last non-zero wins" gives the same answer only
///    when sidechain/compact complications are absent.
///
/// 2. **Compact boundary reset.** If we see a `user` entry with
///    `isCompactSummary: true` *before* finding any assistant usage, the
///    conversation has just been compacted and no post-compact assistant
///    turn exists yet. Pre-compact assistants' `input_tokens` values are
///    stale (Claude Code strips them at load time via `stripStaleUsage`),
///    so we return `None` — the context should be shown as "fresh".
///
/// 3. **Sidechain skip.** Entries with `isSidechain: true` belong to a
///    subagent conversation and their `input_tokens` are for an isolated
///    context window, not the parent session's. They must not pollute the
///    parent's context-usage number.
///
/// 4. **No `stop_reason` filter.** Claude Code includes in-progress
///    assistant turns in the context calculation; so do we. This makes
///    the displayed percentage update live while the model is streaming.
///
/// 5. **Forward pass for `session_max_input_tokens`.** The max is computed
///    over the post-compact segment only — pre-compact turns are dropped
///    because their `input_tokens` are stale (Claude Code zeroes them at
///    load time via `stripStaleUsage`). Sidechain turns are also excluded.
pub fn extract_last_context_usage(lines: &[&str]) -> Option<(u64, String, u64)> {
    // First, find the latest "compact boundary" cutoff. Anything before the
    // most recent compact summary is stale and must be ignored.
    let mut compact_cutoff: usize = 0; // inclusive lower bound for "live" entries
    for (idx, line) in lines.iter().enumerate() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if v.get("type").and_then(|t| t.as_str()) == Some("user")
                && v.get("isCompactSummary")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false)
            {
                compact_cutoff = idx + 1;
            }
        }
    }

    // Walk forward from the cutoff to (a) find session_max and (b) remember
    // the latest live assistant usage. Forward scan is fine here because we
    // already trimmed pre-compact stale data.
    let mut last: Option<(u64, String)> = None;
    let mut session_max: u64 = 0;

    for line in &lines[compact_cutoff..] {
        let Ok(v): Result<Value, _> = serde_json::from_str(line) else {
            continue;
        };

        // Skip subagent/sidechain entries — they have their own context window.
        if v.get("isSidechain")
            .and_then(|b| b.as_bool())
            .unwrap_or(false)
        {
            continue;
        }

        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(msg) = v.get("message").and_then(|m| m.as_object()) else {
            continue;
        };

        let usage = msg.get("usage");
        let input = usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cache_create = usage
            .and_then(|u| u.get("cache_creation_input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cache_read = usage
            .and_then(|u| u.get("cache_read_input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let total_input = input + cache_create + cache_read;

        if total_input == 0 {
            continue;
        }
        if total_input > session_max {
            session_max = total_input;
        }

        let model = msg
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        last = Some((total_input, model));
    }

    last.map(|(used, model)| (used, model, session_max))
}

fn extract_model(last_lines: &[Value]) -> Option<String> {
    for msg in last_lines.iter().rev() {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let model = msg
            .get("message")
            .and_then(|m| m.get("model"))
            .and_then(|m| m.as_str())
            .unwrap_or_default();
        if !model.is_empty() && model != "unknown" && model != "<synthetic>" {
            return Some(model.to_string());
        }
    }
    None
}

fn has_thinking_blocks(last_lines: &[Value]) -> bool {
    for msg in last_lines.iter() {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        if let Some(content) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        {
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("thinking") {
                    return true;
                }
            }
        }
    }
    false
}

fn extract_last_text(last_lines: &[Value]) -> Option<String> {
    for msg in last_lines.iter().rev() {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(content) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        for block in content.iter().rev() {
            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    let preview: String = text.chars().take(200).collect();
                    return Some(preview);
                }
            }
        }
    }
    None
}

fn extract_last_skill(last_lines: &[Value]) -> Option<String> {
    for msg in last_lines.iter().rev() {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(content) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        for block in content.iter().rev() {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                && block.get("name").and_then(|n| n.as_str()) == Some("Skill")
            {
                if let Some(skill) = block
                    .get("input")
                    .and_then(|i| i.get("skill"))
                    .and_then(|s| s.as_str())
                {
                    return Some(skill.to_string());
                }
            }
        }
    }
    None
}

pub fn parse_session_info(
    jsonl_path: &Path,
    session_id: String,
    workspace_path: String,
    workspace_name: String,
    ide_name: Option<String>,
    is_subagent: bool,
    parent_session_id: Option<String>,
    agent_type: Option<String>,
    agent_description: Option<String>,
    meta_model: Option<String>,
    meta_thinking_level: Option<String>,
    pid: Option<u32>,
    pid_precise: bool,
    hook_state: Option<&HookState>,
) -> Option<SessionInfo> {
    let metadata = fs::metadata(jsonl_path).ok()?;
    let last_modified = metadata.modified().ok()?;
    let last_activity_ms = last_modified
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    let created_at_ms = metadata
        .created()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(last_activity_ms);

    let age = SystemTime::now()
        .duration_since(last_modified)
        .unwrap_or(Duration::from_secs(3600));

    // Skip sessions older than 7 days
    if age > Duration::from_secs(7 * 24 * 3600) {
        return None;
    }

    let content = fs::read_to_string(jsonl_path).ok()?;
    let all_lines: Vec<&str> = content.lines().collect();

    // Last 100 lines for status
    let start = all_lines.len().saturating_sub(100);
    let last_n: Vec<Value> = all_lines[start..]
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    let file_age_secs = age.as_secs_f64();
    // content_age = time since the last real user/assistant message, NOT since
    // last file-mtime touch. `claude --resume` appends housekeeping records
    // (last-prompt, file-history-snapshot) that bump mtime without being a
    // new turn; using file mtime alone would falsely mark resumed old sessions
    // as WaitingInput. Fall back to file mtime when no real message is found.
    let content_age_secs = last_real_message_age_secs(&last_n).unwrap_or(file_age_secs);
    // Rate-limit detection has priority over everything else: if the last
    // real turn is a rate_limit API error, the session is stuck regardless
    // of mtime / streaming heuristics.
    let rate_limit = detect_rate_limit(&last_n);
    let status = if rate_limit.is_some() {
        SessionStatus::RateLimited
    } else {
        determine_status(&last_n, file_age_secs, content_age_secs, hook_state)
    };
    let stats = compute_session_stats(&all_lines);
    let context_percent = extract_last_context_usage(&all_lines)
        .and_then(|(used, model, max)| compute_context_percent(used, Some(&model), max));
    let last_message_preview = extract_last_text(&last_n);

    let slug = last_n
        .iter()
        .filter_map(|v| v.get("slug").and_then(|s| s.as_str()).map(|s| s.to_string()))
        .last();

    // ai-title appears near the start of the file; scan all lines
    let ai_title = all_lines
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("ai-title"))
        .and_then(|v| v.get("aiTitle").and_then(|t| t.as_str()).map(|s| s.to_string()));

    let model = meta_model.or_else(|| extract_model(&last_n));
    let last_skill = extract_last_skill(&last_n);
    let todos = crate::session_todos::latest_todo_summary_from_lines(&all_lines);

    // Prefer explicit thinking level from meta; fall back to detecting thinking blocks
    let thinking_level = meta_thinking_level.or_else(|| {
        if has_thinking_blocks(&last_n) {
            Some("thinking".to_string())
        } else {
            None
        }
    });

    Some(SessionInfo {
        id: session_id,
        workspace_path,
        workspace_name,
        ide_name,
        is_subagent,
        parent_session_id,
        agent_type,
        agent_description,
        slug,
        ai_title,
        status,
        token_speed: stats.token_speed,
        total_output_tokens: stats.total_output_tokens,
        total_cost_usd: stats.total_cost_usd,
        agent_total_cost_usd: stats.total_cost_usd,
        cost_speed_usd_per_min: stats.cost_speed_usd_per_min,
        last_message_preview,
        last_activity_ms,
        created_at_ms,
        jsonl_path: jsonl_path.to_string_lossy().to_string(),
        model,
        thinking_level,
        pid,
        pid_precise,
        context_percent,
        last_skill,
        agent_source: "claude-code".to_string(),
        last_outcome: None,
        rate_limit,
        todos,
        compact_count: stats.compact_count,
        compact_pre_tokens: stats.compact_pre_tokens,
        compact_post_tokens: stats.compact_post_tokens,
        compact_cost_usd: stats.compact_cost_usd,
    })
}

// ── Scan cache ───────────────────────────────────────────────────────────────

/// Caches expensive operations across rescans: process-table lookups and
/// already-parsed session files whose mtime hasn't changed.
pub struct ScanCache {
    /// Cached `scan_cli_processes()` result + timestamp.
    pub process_cache: Mutex<(Instant, Vec<CliProcess>)>,
    /// JSONL path → (mtime_ms, SessionInfo).
    pub session_cache: Mutex<HashMap<String, (u64, SessionInfo)>>,
}

impl ScanCache {
    pub fn new() -> Self {
        Self {
            process_cache: Mutex::new((Instant::now() - Duration::from_secs(999), Vec::new())),
            session_cache: Mutex::new(HashMap::new()),
        }
    }
}

/// Downgrade a cached session's status when the file hasn't been touched
/// and enough wall-clock time has elapsed.
pub fn age_out_status(info: &mut SessionInfo, age_secs: f64) {
    // Zero the speed contribution for long-tail waiting states well before
    // their status downgrades. WaitingInput/Delegating keep a 5-minute status
    // window so the UI can still distinguish them from Idle, but once the
    // file has been quiet for 30s the session isn't generating anything —
    // keeping the cached speed around until 300s inflates fleet totals.
    if matches!(
        info.status,
        SessionStatus::WaitingInput | SessionStatus::Delegating
    ) && age_secs >= 30.0
    {
        info.token_speed = 0.0;
        info.cost_speed_usd_per_min = 0.0;
    }

    // Thresholds must mirror `determine_status` so the cache-hit path (which
    // reuses a stale SessionInfo and only calls this function) agrees with
    // the cache-miss path (which re-parses the JSONL). Specifically,
    // `determine_status` keeps a session classified as Streaming when the
    // last assistant message's stop_reason is still null and file_age < 120s;
    // aging out earlier here caused live-streaming sessions to flicker into
    // Idle between JSONL flush batches.
    let idle = match info.status {
        SessionStatus::Streaming if age_secs >= 120.0 => true,
        SessionStatus::Thinking if age_secs >= 120.0 => true,
        SessionStatus::Executing if age_secs >= 60.0 => true,
        SessionStatus::Processing if age_secs >= 60.0 => true,
        SessionStatus::Active if age_secs >= 30.0 => true,
        SessionStatus::WaitingInput if age_secs >= 300.0 => true,
        SessionStatus::Delegating if age_secs >= 300.0 => true,
        _ => false,
    };
    if idle {
        info.status = SessionStatus::Idle;
        info.token_speed = 0.0;
        info.cost_speed_usd_per_min = 0.0;
    }
}

/// Check if a cached session entry is still valid (mtime matches).
/// Returns `(cached_info, age_secs)` on hit.
fn check_session_cache(
    path: &Path,
    cache: &HashMap<String, (u64, SessionInfo)>,
) -> Option<(SessionInfo, f64)> {
    let metadata = fs::metadata(path).ok()?;
    let last_modified = metadata.modified().ok()?;
    let mtime_ms = last_modified
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    let age_secs = SystemTime::now()
        .duration_since(last_modified)
        .unwrap_or(Duration::from_secs(3600))
        .as_secs_f64();

    if age_secs > 7.0 * 24.0 * 3600.0 {
        return None;
    }

    let key = path.to_string_lossy();
    let (cached_mt, cached_info) = cache.get(key.as_ref())?;
    if *cached_mt != mtime_ms {
        return None;
    }

    Some((cached_info.clone(), age_secs))
}

// ── Public entry point ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SubagentMeta {
    #[serde(rename = "agentType")]
    agent_type: Option<String>,
    description: Option<String>,
    model: Option<String>,
    #[serde(rename = "thinkingLevel")]
    thinking_level: Option<String>,
}

pub fn scan_claude_sessions(claude_dir: &Path, scan_cache: &ScanCache) -> Vec<SessionInfo> {
    let mut sessions = Vec::new();
    let ide_sessions = scan_ide_sessions(claude_dir);

    // Reuse cached process list if fresh (< 10 s).
    let cli_processes = {
        let mut guard = scan_cache.process_cache.lock().unwrap();
        if guard.0.elapsed() > Duration::from_secs(10) {
            guard.1 = scan_cli_processes();
            guard.0 = Instant::now();
        }
        guard.1.clone()
    };

    let hook_states = crate::hooks::read_hook_states();
    let session_cache_snapshot = scan_cache.session_cache.lock().unwrap().clone();

    let projects_dir = claude_dir.join("projects");
    let Ok(workspace_entries) = fs::read_dir(&projects_dir) else {
        return sessions;
    };

    for workspace_entry in workspace_entries.flatten() {
        let workspace_dir = workspace_entry.path();
        if !workspace_dir.is_dir() {
            continue;
        }

        let encoded = workspace_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();

        // Find associated IDE session by encoding the lock file paths and comparing to the
        // directory name directly.  This avoids the lossy decode round-trip: a workspace named
        // "claw-fleet" encodes to "-Users-…-claw-fleet" but decodes to "/Users/…/claw/fleet".
        let ide = ide_sessions.iter().find(|ide| {
            ide.workspace_folders
                .iter()
                .any(|f| encode_workspace_path(f) == encoded)
        });

        // Use the exact path from the lock file when available; fall back to lossy decode.
        let workspace_path = ide
            .and_then(|s| {
                s.workspace_folders
                    .iter()
                    .find(|f| encode_workspace_path(f) == encoded)
            })
            .cloned()
            .unwrap_or_else(|| decode_workspace_path(&encoded));

        let ws_name = workspace_name(&workspace_path);
        let ide_name = ide.map(|s| s.ide_name.clone());

        // Collect all CLI processes for this workspace (may be >1 when subagents are running).
        // PID: use Claude CLI process only (not the IDE PID — killing the IDE PID would
        // terminate the editor itself, not just the Claude session).
        let procs_in_cwd: Vec<CliProcess> = cli_processes
            .iter()
            .filter(|p| p.cwd == workspace_path)
            .cloned()
            .collect();

        let Ok(entries) = fs::read_dir(&workspace_dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();

            // Main session JSONL files
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                let session_id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();

                let (session_pid, pid_precise) = resolve_pid(&procs_in_cwd, &session_id);

                // Try session cache first (skip re-reading unchanged files).
                if let Some((mut info, age)) = check_session_cache(&path, &session_cache_snapshot) {
                    age_out_status(&mut info, age);
                    info.pid = session_pid;
                    info.pid_precise = pid_precise;
                    info.ide_name = ide_name.clone();
                    sessions.push(info);
                } else if let Some(info) = parse_session_info(
                    &path,
                    session_id.clone(),
                    workspace_path.clone(),
                    ws_name.clone(),
                    ide_name.clone(),
                    false,
                    None,
                    None,
                    None,
                    None,
                    None,
                    session_pid,
                    pid_precise,
                    hook_states.get(&session_id),
                ) {
                    scan_cache.session_cache.lock().unwrap()
                        .insert(path.to_string_lossy().to_string(), (info.last_activity_ms, info.clone()));
                    sessions.push(info);
                }
            }

            // Subagent directories: <session-uuid>/subagents/agent-*.jsonl
            if path.is_dir() {
                let parent_session_id = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();

                let subagents_dir = path.join("subagents");
                let Ok(agent_entries) = fs::read_dir(&subagents_dir) else {
                    continue;
                };

                for agent_entry in agent_entries.flatten() {
                    let agent_path = agent_entry.path();
                    if agent_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }

                    let agent_id = agent_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                        .to_string();

                    // Read optional meta.json
                    let meta_path = agent_path.with_extension("meta.json");
                    let meta = fs::read_to_string(&meta_path)
                        .ok()
                        .and_then(|s| serde_json::from_str::<SubagentMeta>(&s).ok());

                    let agent_type = meta.as_ref().and_then(|m| m.agent_type.clone());
                    let agent_description = meta.as_ref().and_then(|m| m.description.clone());
                    let meta_model = meta.as_ref().and_then(|m| m.model.clone());
                    let meta_thinking_level = meta.and_then(|m| m.thinking_level.clone());

                    // Subagents share the parent's PID resolution; never precise on their own
                    // since we can't kill just the subagent independently.
                    let (sub_pid, _) = resolve_pid(&procs_in_cwd, &parent_session_id);

                    // Try session cache first for subagents too.
                    if let Some((mut info, age)) = check_session_cache(&agent_path, &session_cache_snapshot) {
                        age_out_status(&mut info, age);
                        info.pid = sub_pid;
                        info.pid_precise = false;
                        info.ide_name = ide_name.clone();
                        sessions.push(info);
                    } else if let Some(info) = parse_session_info(
                        &agent_path,
                        agent_id.clone(),
                        workspace_path.clone(),
                        ws_name.clone(),
                        ide_name.clone(),
                        true,
                        Some(parent_session_id.clone()),
                        agent_type,
                        agent_description,
                        meta_model,
                        meta_thinking_level,
                        sub_pid,
                        false, // subagents are never pid_precise: stop parent instead
                        hook_states.get(&agent_id),
                    ) {
                        scan_cache.session_cache.lock().unwrap()
                            .insert(agent_path.to_string_lossy().to_string(), (info.last_activity_ms, info.clone()));
                        sessions.push(info);
                    }
                }
            }
        }
    }

    // Promote main sessions to Delegating if they have at least one actively-working subagent.
    // A subagent that is WaitingInput has finished its turn and should not cause the parent
    // to show as Delegating — otherwise the parent's own WaitingInput status gets hidden.
    let active_parent_ids: std::collections::HashSet<String> = sessions
        .iter()
        .filter(|s| {
            s.is_subagent
                && matches!(
                    s.status,
                    SessionStatus::Thinking
                        | SessionStatus::Executing
                        | SessionStatus::Streaming
                        | SessionStatus::Delegating
                        | SessionStatus::Processing
                )
        })
        .filter_map(|s| s.parent_session_id.clone())
        .collect();

    for session in &mut sessions {
        if !session.is_subagent
            && session.parent_session_id.is_none()
            && active_parent_ids.contains(&session.id)
            && matches!(
                session.status,
                SessionStatus::Active | SessionStatus::Idle | SessionStatus::Processing
            )
        {
            session.status = SessionStatus::Delegating;
        }
    }

    // Aggregate subagent cost into each main session's `agent_total_cost_usd`.
    // Main sessions already hold their own cost in that field from parse; we add
    // the sum of every subagent that points back to them.
    let mut subagent_cost_by_parent: HashMap<String, f64> = HashMap::new();
    for s in &sessions {
        if s.is_subagent {
            if let Some(pid) = &s.parent_session_id {
                *subagent_cost_by_parent.entry(pid.clone()).or_insert(0.0) += s.total_cost_usd;
            }
        }
    }
    for session in &mut sessions {
        if !session.is_subagent {
            if let Some(extra) = subagent_cost_by_parent.get(&session.id) {
                session.agent_total_cost_usd += *extra;
            }
        }
    }

    // Prune stale entries from session cache.
    {
        let live_paths: HashSet<String> = sessions.iter().map(|s| s.jsonl_path.clone()).collect();
        scan_cache.session_cache.lock().unwrap().retain(|k, _| live_paths.contains(k));
    }

    // Sort: active first, then by created_at_ms asc (oldest first = stable order)
    sessions.sort_by(|a, b| {
        let a_active = matches!(
            a.status,
            SessionStatus::Thinking
                | SessionStatus::Executing
                | SessionStatus::Streaming
                | SessionStatus::Delegating
                | SessionStatus::Processing
                | SessionStatus::WaitingInput
        );
        let b_active = matches!(
            b.status,
            SessionStatus::Thinking
                | SessionStatus::Executing
                | SessionStatus::Streaming
                | SessionStatus::Delegating
                | SessionStatus::Processing
                | SessionStatus::WaitingInput
        );
        b_active
            .cmp(&a_active)
            .then(a.created_at_ms.cmp(&b.created_at_ms))
    });

    sessions
}

/// Sort sessions: active first, then by created_at_ms asc.
pub fn sort_sessions(sessions: &mut Vec<SessionInfo>) {
    sessions.sort_by(|a, b| {
        let a_active = matches!(
            a.status,
            SessionStatus::Thinking
                | SessionStatus::Executing
                | SessionStatus::Streaming
                | SessionStatus::Delegating
                | SessionStatus::Processing
                | SessionStatus::WaitingInput
        );
        let b_active = matches!(
            b.status,
            SessionStatus::Thinking
                | SessionStatus::Executing
                | SessionStatus::Streaming
                | SessionStatus::Delegating
                | SessionStatus::Processing
                | SessionStatus::WaitingInput
        );
        b_active
            .cmp(&a_active)
            .then(a.created_at_ms.cmp(&b.created_at_ms))
    });
}

/// Scan all agent sources and merge into a single sorted list.
pub fn scan_sessions(claude_dir: &Path, scan_cache: &ScanCache) -> Vec<SessionInfo> {
    let mut sessions = scan_claude_sessions(claude_dir, scan_cache);
    sort_sessions(&mut sessions);
    sessions
}

/// Scan all registered agent sources and merge into a single sorted list.
pub fn scan_all_sources(sources: &[Box<dyn crate::agent_source::AgentSource>]) -> Vec<SessionInfo> {
    let mut sessions = Vec::new();
    for source in sources {
        if source.is_available() {
            sessions.extend(source.scan_sessions());
        }
    }
    sort_sessions(&mut sessions);
    sessions
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── Helper builders ─────────────────────────────────────────────────────

    /// Build an assistant message with given content blocks and stop_reason.
    fn assistant_msg(blocks: Vec<Value>, stop_reason: Option<&str>) -> Value {
        json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": blocks,
                "stop_reason": stop_reason,
                "model": "claude-sonnet-4-20250514",
                "usage": { "output_tokens": 100 }
            }
        })
    }

    fn assistant_msg_with_id(blocks: Vec<Value>, stop_reason: Option<&str>, id: &str, ts: &str) -> String {
        json!({
            "type": "assistant",
            "timestamp": ts,
            "message": {
                "id": id,
                "role": "assistant",
                "content": blocks,
                "stop_reason": stop_reason,
                "model": "claude-sonnet-4-20250514",
                "usage": { "output_tokens": 50 }
            }
        }).to_string()
    }

    fn user_msg() -> Value {
        json!({ "type": "user", "message": { "role": "user", "content": [{"type": "text", "text": "hello"}] } })
    }

    fn text_block(text: &str) -> Value {
        json!({"type": "text", "text": text})
    }

    fn thinking_block() -> Value {
        json!({"type": "thinking", "thinking": "hmm..."})
    }

    fn tool_use_block(name: &str) -> Value {
        json!({"type": "tool_use", "name": name, "input": {}})
    }

    fn skill_block(skill: &str) -> Value {
        json!({"type": "tool_use", "name": "Skill", "input": {"skill": skill}})
    }

    fn api_error_msg(error: &str, text: &str, timestamp: &str) -> Value {
        json!({
            "type": "assistant",
            "timestamp": timestamp,
            "isApiErrorMessage": true,
            "error": error,
            "message": {
                "role": "assistant",
                "stop_reason": "stop_sequence",
                "content": [{"type": "text", "text": text}],
            },
        })
    }

    // ── detect_rate_limit tests ────────────────────────────────────────────

    #[test]
    fn rate_limit_detect_basic() {
        let lines = vec![
            user_msg(),
            api_error_msg(
                "rate_limit",
                "You've hit your weekly limit · resets Apr 20, 10am (Asia/Shanghai)",
                "2026-04-15T10:00:00.000Z",
            ),
        ];
        let state = detect_rate_limit(&lines).expect("should detect rate_limit");
        assert!(state.parsed);
        assert_eq!(
            state.limit_type,
            crate::rate_limit_parser::RateLimitType::WeeklyLimit
        );
    }

    #[test]
    fn rate_limit_detect_unparseable_still_some() {
        // Production legacy form with no limit-type keyword.
        let lines = vec![
            user_msg(),
            api_error_msg(
                "rate_limit",
                "You've hit your limit · resets 7pm (Asia/Shanghai)",
                "2026-03-17T08:10:04.234Z",
            ),
        ];
        let state = detect_rate_limit(&lines).expect("legacy form still yields state");
        assert_eq!(
            state.limit_type,
            crate::rate_limit_parser::RateLimitType::Unknown
        );
    }

    #[test]
    fn rate_limit_ignored_when_different_error() {
        let lines = vec![
            user_msg(),
            api_error_msg(
                "authentication_failed",
                "Failed to authenticate. API Error: 403",
                "2026-04-15T10:00:00.000Z",
            ),
        ];
        assert!(detect_rate_limit(&lines).is_none());
    }

    #[test]
    fn rate_limit_stale_when_real_turn_follows() {
        // User already resumed: a real assistant message exists after the error.
        let lines = vec![
            user_msg(),
            api_error_msg(
                "rate_limit",
                "You've hit your session limit · resets 7pm (Asia/Shanghai)",
                "2026-04-15T10:00:00.000Z",
            ),
            user_msg(),
            assistant_msg(vec![text_block("back in action")], Some("end_turn")),
        ];
        assert!(
            detect_rate_limit(&lines).is_none(),
            "a real turn after the error must clear rate_limit"
        );
    }

    #[test]
    fn rate_limit_ignored_when_no_api_error_flag() {
        // A plain assistant message that happens to contain the phrase but
        // lacks isApiErrorMessage must not trigger detection.
        let lines = vec![
            user_msg(),
            assistant_msg(
                vec![text_block("You've hit your weekly limit (in testing)")],
                Some("end_turn"),
            ),
        ];
        assert!(detect_rate_limit(&lines).is_none());
    }

    fn make_session(status: SessionStatus) -> SessionInfo {
        SessionInfo {
            id: "test-session".into(),
            workspace_path: "/tmp/test".into(),
            workspace_name: "test".into(),
            ide_name: None,
            is_subagent: false,
            parent_session_id: None,
            agent_type: None,
            agent_description: None,
            slug: None,
            ai_title: None,
            status,
            token_speed: 10.0,
            total_output_tokens: 500,
            total_cost_usd: 0.0,
            agent_total_cost_usd: 0.0,
            cost_speed_usd_per_min: 0.0,
            last_message_preview: None,
            last_activity_ms: 0,
            created_at_ms: 0,
            jsonl_path: "/tmp/test.jsonl".into(),
            model: None,
            thinking_level: None,
            pid: None,
            pid_precise: false,
            last_skill: None,
            context_percent: None,
            agent_source: "claude-code".into(),
            last_outcome: None,
            rate_limit: None,
            todos: None,
            compact_count: 0,
            compact_pre_tokens: 0,
            compact_post_tokens: 0,
            compact_cost_usd: 0.0,
        }
    }

    // ── determine_status tests ──────────────────────────────────────────────

    /// Test wrapper that preserves pre-content-age semantics by passing the
    /// same value for both `file_age_secs` and `content_age_secs`. Tests that
    /// specifically care about the file-vs-content age distinction call
    /// `determine_status` directly with distinct values.
    fn ds(lines: &[Value], age: f64, hook: Option<&HookState>) -> SessionStatus {
        determine_status(lines, age, age, hook)
    }

    #[test]
    fn status_streaming_thinking_blocks() {
        let lines = vec![
            user_msg(),
            assistant_msg(vec![thinking_block()], None), // stop_reason=null → streaming
        ];
        assert_eq!(ds(&lines, 2.0, None), SessionStatus::Thinking);
    }

    #[test]
    fn status_streaming_tool_use_blocks() {
        let lines = vec![
            user_msg(),
            assistant_msg(vec![text_block("let me check"), tool_use_block("Read")], None),
        ];
        assert_eq!(ds(&lines, 1.0, None), SessionStatus::Executing);
    }

    #[test]
    fn status_streaming_text_only() {
        let lines = vec![
            user_msg(),
            assistant_msg(vec![text_block("Hello world")], None),
        ];
        assert_eq!(ds(&lines, 3.0, None), SessionStatus::Streaming);
    }

    #[test]
    fn status_end_turn_waiting_input() {
        let lines = vec![
            user_msg(),
            assistant_msg(vec![text_block("Done!")], Some("end_turn")),
        ];
        assert_eq!(ds(&lines, 10.0, None), SessionStatus::WaitingInput);
    }

    #[test]
    fn status_end_turn_too_old_becomes_idle() {
        let lines = vec![
            assistant_msg(vec![text_block("Done!")], Some("end_turn")),
        ];
        assert_eq!(ds(&lines, 500.0, None), SessionStatus::Idle);
    }

    #[test]
    fn status_tool_use_stop_reason_executing() {
        let lines = vec![
            assistant_msg(vec![tool_use_block("Bash")], Some("tool_use")),
        ];
        assert_eq!(ds(&lines, 15.0, None), SessionStatus::Executing);
    }

    #[test]
    fn status_tool_use_too_old_becomes_idle() {
        let lines = vec![
            assistant_msg(vec![tool_use_block("Bash")], Some("tool_use")),
        ];
        assert_eq!(ds(&lines, 120.0, None), SessionStatus::Idle);
    }

    #[test]
    fn status_user_message_last_thinking() {
        let lines = vec![user_msg()];
        assert_eq!(ds(&lines, 5.0, None), SessionStatus::Thinking);
    }

    #[test]
    fn status_user_message_too_old() {
        let lines = vec![user_msg()];
        assert_eq!(ds(&lines, 200.0, None), SessionStatus::Idle);
    }

    #[test]
    fn status_no_meaningful_lines_recent() {
        let lines: Vec<Value> = vec![];
        assert_eq!(ds(&lines, 10.0, None), SessionStatus::Active);
    }

    #[test]
    fn status_no_meaningful_lines_old() {
        let lines: Vec<Value> = vec![];
        assert_eq!(ds(&lines, 60.0, None), SessionStatus::Idle);
    }

    #[test]
    fn status_hook_tool_executing_overrides() {
        let lines = vec![
            assistant_msg(vec![text_block("old text")], Some("end_turn")),
        ];
        assert_eq!(
            ds(&lines, 20.0, Some(&HookState::ToolExecuting)),
            SessionStatus::Executing,
        );
    }

    #[test]
    fn status_hook_model_processing_overrides() {
        let lines = vec![
            assistant_msg(vec![text_block("old")], Some("end_turn")),
        ];
        assert_eq!(
            ds(&lines, 20.0, Some(&HookState::ModelProcessing)),
            SessionStatus::Thinking,
        );
    }

    #[test]
    fn status_hook_stopped_overrides() {
        let lines = vec![user_msg()];
        assert_eq!(
            ds(&lines, 20.0, Some(&HookState::Stopped)),
            SessionStatus::WaitingInput,
        );
    }

    #[test]
    fn status_resumed_old_session_stays_idle() {
        // Regression: `claude --resume` on a 3-day-old session appends
        // `last-prompt` + `file-history-snapshot` housekeeping records. These
        // bump the JSONL mtime but are NOT new turns. Previously the fresh
        // mtime + trailing `end_turn` stop_reason caused the session to flip
        // to WaitingInput. With content-age separated from file-age, it must
        // stay Idle.
        let lines = vec![
            user_msg(),
            assistant_msg(vec![text_block("Done!")], Some("end_turn")),
            // resume-appended housekeeping (no timestamp field)
            json!({"type": "last-prompt", "lastPrompt": "", "sessionId": "x"}),
            json!({"type": "file-history-snapshot", "messageId": "m", "snapshot": {}, "isSnapshotUpdate": true}),
            json!({"type": "file-history-snapshot", "messageId": "m", "snapshot": {}, "isSnapshotUpdate": true}),
        ];
        // file_age=5s (mtime just got touched by resume), content_age=3 days
        let content_age = 3.0 * 24.0 * 3600.0;
        assert_eq!(
            determine_status(&lines, 5.0, content_age, None),
            SessionStatus::Idle,
        );
    }

    #[test]
    fn status_resumed_old_session_ignores_stale_stopped_hook() {
        // Same regression but via the hook path: a stale `Stopped` hook plus
        // a mtime-touching resume must not produce WaitingInput when the real
        // content is old.
        let lines = vec![
            user_msg(),
            assistant_msg(vec![text_block("Done!")], Some("end_turn")),
            json!({"type": "last-prompt", "lastPrompt": "", "sessionId": "x"}),
        ];
        let content_age = 3.0 * 24.0 * 3600.0;
        assert_eq!(
            determine_status(&lines, 20.0, content_age, Some(&HookState::Stopped)),
            SessionStatus::Idle,
        );
    }

    #[test]
    fn status_hook_ignored_when_streaming() {
        let lines = vec![
            user_msg(),
            assistant_msg(vec![thinking_block()], None),
        ];
        assert_eq!(
            ds(&lines, 2.0, Some(&HookState::Stopped)),
            SessionStatus::Thinking,
        );
    }

    // ── compute_session_stats tests ─────────────────────────────────────────

    #[test]
    fn session_stats_empty_lines() {
        let lines: Vec<&str> = vec![];
        let stats = compute_session_stats(&lines);
        assert_eq!(stats.total_output_tokens, 0);
        assert_eq!(stats.token_speed, 0.0);
        assert_eq!(stats.total_cost_usd, 0.0);
        assert_eq!(stats.cost_speed_usd_per_min, 0.0);
    }

    #[test]
    fn session_stats_non_assistant_ignored() {
        let line = json!({"type": "user", "message": {"content": []}}).to_string();
        let lines: Vec<&str> = vec![&line];
        let stats = compute_session_stats(&lines);
        assert_eq!(stats.total_output_tokens, 0);
    }

    #[test]
    fn session_stats_null_stop_reason_ignored() {
        let line = json!({
            "type": "assistant",
            "message": {
                "stop_reason": null,
                "usage": {"output_tokens": 100}
            }
        }).to_string();
        let lines: Vec<&str> = vec![&line];
        let stats = compute_session_stats(&lines);
        assert_eq!(stats.total_output_tokens, 0);
    }

    #[test]
    fn session_stats_counts_finalized_tokens() {
        let line = json!({
            "type": "assistant",
            "message": {
                "id": "msg_1",
                "stop_reason": "end_turn",
                "usage": {"output_tokens": 200}
            }
        }).to_string();
        let lines: Vec<&str> = vec![&line];
        let stats = compute_session_stats(&lines);
        assert_eq!(stats.total_output_tokens, 200);
    }

    #[test]
    fn session_stats_deduplicates_by_id() {
        let line = json!({
            "type": "assistant",
            "message": {
                "id": "msg_dup",
                "stop_reason": "end_turn",
                "usage": {"output_tokens": 100}
            }
        }).to_string();
        let lines: Vec<&str> = vec![&line, &line];
        let stats = compute_session_stats(&lines);
        assert_eq!(stats.total_output_tokens, 100);
    }

    #[test]
    fn session_stats_speed_from_recent_timestamps() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let ts1 = chrono::DateTime::from_timestamp(now as i64 - 60, 0).unwrap().to_rfc3339();
        let ts2 = chrono::DateTime::from_timestamp(now as i64 - 30, 0).unwrap().to_rfc3339();

        let l1 = assistant_msg_with_id(vec![], Some("end_turn"), "m1", &ts1);
        let l2 = assistant_msg_with_id(vec![], Some("end_turn"), "m2", &ts2);
        let lines: Vec<&str> = vec![&l1, &l2];
        let stats = compute_session_stats(&lines);
        assert_eq!(stats.total_output_tokens, 100); // 50 + 50
        // 100 tokens over (now - first_ts) ≈ 60s → ~1.67 tok/s. Allow a small
        // window for clock jitter between test setup and stats computation.
        assert!(
            stats.token_speed > 1.4 && stats.token_speed < 2.0,
            "speed={}",
            stats.token_speed
        );
    }

    #[test]
    fn session_stats_cost_from_sonnet_usage() {
        // Sonnet tier = $3/$15 per Mtok. 1M input + 1M output = $18.
        let line = json!({
            "type": "assistant",
            "message": {
                "id": "msg_cost",
                "model": "claude-sonnet-4-6-20251101",
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 1_000_000,
                    "output_tokens": 1_000_000
                }
            }
        }).to_string();
        let lines: Vec<&str> = vec![&line];
        let stats = compute_session_stats(&lines);
        assert!((stats.total_cost_usd - 18.0).abs() < 1e-6, "cost={}", stats.total_cost_usd);
    }

    #[test]
    fn session_stats_cost_speed_usd_per_min() {
        // Two sonnet turns, first at now-60s, second at now-30s, each 100k
        // output tokens. Per-turn cost: 100k output * $15/M = $1.50, total
        // recent cost $3.00. Duration (= now - first_ts) ≈ 60s → cost speed
        // = $3 * 60 / 60 = $3/min.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let ts1 = chrono::DateTime::from_timestamp(now as i64 - 60, 0).unwrap().to_rfc3339();
        let ts2 = chrono::DateTime::from_timestamp(now as i64 - 30, 0).unwrap().to_rfc3339();
        let mk = |id: &str, ts: &str| json!({
            "type": "assistant",
            "timestamp": ts,
            "message": {
                "id": id,
                "model": "claude-sonnet-4-6",
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 0, "output_tokens": 100_000}
            }
        }).to_string();
        let l1 = mk("a1", &ts1);
        let l2 = mk("a2", &ts2);
        let lines: Vec<&str> = vec![&l1, &l2];
        let stats = compute_session_stats(&lines);
        assert!((stats.total_cost_usd - 3.0).abs() < 1e-6, "cost={}", stats.total_cost_usd);
        // Tolerance covers sub-second clock drift between test setup and
        // the `SystemTime::now()` read inside `compute_session_stats`.
        assert!(
            (stats.cost_speed_usd_per_min - 3.0).abs() < 0.1,
            "cost_speed={}",
            stats.cost_speed_usd_per_min
        );
    }

    #[test]
    fn session_stats_compact_count_and_estimated_cost() {
        // Sequence: one Sonnet assistant turn, then a compact_boundary,
        // then another assistant turn, then a second compact_boundary.
        // Sonnet pricing → cache_read $0.30/M, output $15/M.
        // Compact #1: pre 100k → cost $0.30 * 0.1 = $0.03, post 5k → $15 * 0.005 = $0.075.
        // Compact #2: pre 200k → $0.30 * 0.2 = $0.06,  post 8k → $15 * 0.008 = $0.12.
        // Total compact cost = 0.03 + 0.075 + 0.06 + 0.12 = 0.285 USD.
        let pre_assistant = json!({
            "type": "assistant",
            "message": {
                "id": "pre1",
                "model": "claude-sonnet-4-6-20251101",
                "stop_reason": "end_turn",
                "usage": {"output_tokens": 100}
            }
        }).to_string();
        let compact1 = json!({
            "type": "system",
            "subtype": "compact_boundary",
            "compactMetadata": {
                "trigger": "auto",
                "preTokens": 100_000,
                "postTokens": 5_000,
                "durationMs": 30_000
            }
        }).to_string();
        let mid_assistant = json!({
            "type": "assistant",
            "message": {
                "id": "mid1",
                "model": "claude-sonnet-4-6-20251101",
                "stop_reason": "end_turn",
                "usage": {"output_tokens": 100}
            }
        }).to_string();
        let compact2 = json!({
            "type": "system",
            "subtype": "compact_boundary",
            "compactMetadata": {
                "trigger": "manual",
                "preTokens": 200_000,
                "postTokens": 8_000
            }
        }).to_string();
        let lines: Vec<&str> = vec![&pre_assistant, &compact1, &mid_assistant, &compact2];
        let stats = compute_session_stats(&lines);
        assert_eq!(stats.compact_count, 2);
        assert_eq!(stats.compact_pre_tokens, 300_000);
        assert_eq!(stats.compact_post_tokens, 13_000);
        assert!(
            (stats.compact_cost_usd - 0.285).abs() < 1e-6,
            "compact_cost_usd={}",
            stats.compact_cost_usd
        );
    }

    #[test]
    fn session_stats_compact_with_missing_metadata() {
        // A compact_boundary without compactMetadata still bumps the count
        // but contributes 0 to tokens and cost — defensive against schema drift.
        let bare_compact = json!({
            "type": "system",
            "subtype": "compact_boundary"
        }).to_string();
        let lines: Vec<&str> = vec![&bare_compact];
        let stats = compute_session_stats(&lines);
        assert_eq!(stats.compact_count, 1);
        assert_eq!(stats.compact_pre_tokens, 0);
        assert_eq!(stats.compact_post_tokens, 0);
        assert_eq!(stats.compact_cost_usd, 0.0);
    }

    #[test]
    fn session_stats_speed_decays_when_idle_tail() {
        // Two turns 30s apart, but the most recent one is already 240s old
        // (i.e. the session has been waiting for input ~4 minutes).
        // Buggy formula: duration = last_ts - first_ts = 30s → speed stays high.
        // Correct formula: duration = now - first_ts ≈ 270s → speed decays.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let ts1 = chrono::DateTime::from_timestamp(now as i64 - 270, 0).unwrap().to_rfc3339();
        let ts2 = chrono::DateTime::from_timestamp(now as i64 - 240, 0).unwrap().to_rfc3339();

        let l1 = assistant_msg_with_id(vec![], Some("end_turn"), "m1", &ts1);
        let l2 = assistant_msg_with_id(vec![], Some("end_turn"), "m2", &ts2);
        let lines: Vec<&str> = vec![&l1, &l2];
        let stats = compute_session_stats(&lines);
        // 100 tokens / 270s ≈ 0.37 tok/s. Anything above 1.0 means the old
        // inter-turn-gap formula is still in play.
        assert!(
            stats.token_speed < 1.0,
            "token_speed should decay with idle tail, got {}",
            stats.token_speed
        );
    }

    // ── extract_model tests ─────────────────────────────────────────────────

    #[test]
    fn extract_model_from_assistant() {
        let lines = vec![
            assistant_msg(vec![text_block("hi")], Some("end_turn")),
        ];
        assert_eq!(extract_model(&lines), Some("claude-sonnet-4-20250514".into()));
    }

    #[test]
    fn extract_model_ignores_unknown() {
        let lines = vec![json!({
            "type": "assistant",
            "message": {"model": "unknown", "content": [], "stop_reason": "end_turn"}
        })];
        assert_eq!(extract_model(&lines), None);
    }

    #[test]
    fn extract_model_ignores_synthetic() {
        let lines = vec![json!({
            "type": "assistant",
            "message": {"model": "<synthetic>", "content": [], "stop_reason": "end_turn"}
        })];
        assert_eq!(extract_model(&lines), None);
    }

    #[test]
    fn extract_model_ignores_user_messages() {
        let lines = vec![user_msg()];
        assert_eq!(extract_model(&lines), None);
    }

    // ── has_thinking_blocks tests ───────────────────────────────────────────

    #[test]
    fn thinking_blocks_present() {
        let lines = vec![
            assistant_msg(vec![thinking_block(), text_block("result")], Some("end_turn")),
        ];
        assert!(has_thinking_blocks(&lines));
    }

    #[test]
    fn thinking_blocks_absent() {
        let lines = vec![
            assistant_msg(vec![text_block("no thinking")], Some("end_turn")),
        ];
        assert!(!has_thinking_blocks(&lines));
    }

    // ── extract_last_text tests ─────────────────────────────────────────────

    #[test]
    fn extract_text_from_last_assistant() {
        let lines = vec![
            assistant_msg(vec![text_block("first message")], Some("end_turn")),
            assistant_msg(vec![text_block("second message")], Some("end_turn")),
        ];
        assert_eq!(extract_last_text(&lines), Some("second message".into()));
    }

    #[test]
    fn extract_text_truncates_to_200_chars() {
        let long_text = "a".repeat(300);
        let lines = vec![
            assistant_msg(vec![text_block(&long_text)], Some("end_turn")),
        ];
        let result = extract_last_text(&lines).unwrap();
        assert_eq!(result.len(), 200);
    }

    #[test]
    fn extract_text_returns_none_for_no_text() {
        let lines = vec![
            assistant_msg(vec![tool_use_block("Bash")], Some("tool_use")),
        ];
        assert_eq!(extract_last_text(&lines), None);
    }

    // ── extract_last_skill tests ────────────────────────────────────────────

    #[test]
    fn extract_skill_found() {
        let lines = vec![
            assistant_msg(vec![skill_block("commit")], Some("tool_use")),
        ];
        assert_eq!(extract_last_skill(&lines), Some("commit".into()));
    }

    #[test]
    fn extract_skill_not_found() {
        let lines = vec![
            assistant_msg(vec![tool_use_block("Read")], Some("tool_use")),
        ];
        assert_eq!(extract_last_skill(&lines), None);
    }

    // ── extract_resume_id tests ─────────────────────────────────────────────

    #[test]
    fn resume_id_long_flag() {
        let cmd: Vec<std::ffi::OsString> = vec!["claude".into(), "--resume".into(), "abc123".into()];
        assert_eq!(extract_resume_id(&cmd), Some("abc123".into()));
    }

    #[test]
    fn resume_id_short_flag() {
        let cmd: Vec<std::ffi::OsString> = vec!["claude".into(), "-r".into(), "xyz".into()];
        assert_eq!(extract_resume_id(&cmd), Some("xyz".into()));
    }

    #[test]
    fn resume_id_equals_syntax() {
        let cmd: Vec<std::ffi::OsString> = vec!["claude".into(), "--resume=sess42".into()];
        assert_eq!(extract_resume_id(&cmd), Some("sess42".into()));
    }

    #[test]
    fn resume_id_absent() {
        let cmd: Vec<std::ffi::OsString> = vec!["claude".into(), "--verbose".into()];
        assert_eq!(extract_resume_id(&cmd), None);
    }

    // ── resolve_pid tests ───────────────────────────────────────────────────

    #[test]
    fn resolve_pid_empty() {
        assert_eq!(resolve_pid(&[], "sess1"), (None, false));
    }

    #[test]
    fn resolve_pid_exact_resume_match() {
        let procs = vec![
            CliProcess { pid: 100, ppid: None, cwd: "/tmp".into(), resume_session_id: Some("sess1".into()) },
            CliProcess { pid: 200, ppid: None, cwd: "/tmp".into(), resume_session_id: None },
        ];
        assert_eq!(resolve_pid(&procs, "sess1"), (Some(100), true));
    }

    #[test]
    fn resolve_pid_single_process() {
        let procs = vec![
            CliProcess { pid: 42, ppid: None, cwd: "/tmp".into(), resume_session_id: None },
        ];
        assert_eq!(resolve_pid(&procs, "other"), (Some(42), true));
    }

    #[test]
    fn resolve_pid_parent_child_filtering() {
        let procs = vec![
            CliProcess { pid: 100, ppid: Some(1), cwd: "/tmp".into(), resume_session_id: None },
            CliProcess { pid: 200, ppid: Some(100), cwd: "/tmp".into(), resume_session_id: None },
        ];
        assert_eq!(resolve_pid(&procs, "any"), (Some(100), true));
    }

    #[test]
    fn resolve_pid_multiple_roots_imprecise() {
        let procs = vec![
            CliProcess { pid: 100, ppid: Some(1), cwd: "/tmp".into(), resume_session_id: None },
            CliProcess { pid: 200, ppid: Some(2), cwd: "/tmp".into(), resume_session_id: None },
        ];
        let (pid, precise) = resolve_pid(&procs, "any");
        assert!(pid.is_some());
        assert!(!precise);
    }

    // ── workspace_name / encode / decode tests ──────────────────────────────

    #[test]
    fn workspace_name_basic() {
        assert_eq!(workspace_name("/Users/foo/my-project"), "my-project");
    }

    #[test]
    fn workspace_name_trailing_slash() {
        assert_eq!(workspace_name("/Users/foo/bar/"), "bar");
    }

    #[test]
    fn workspace_name_root() {
        assert_eq!(workspace_name("/"), "/");
    }

    #[test]
    fn encode_decode_workspace_path() {
        let original = "/Users/foo/bar";
        let encoded = encode_workspace_path(original);
        assert_eq!(encoded, "-Users-foo-bar");
        let decoded = decode_workspace_path(&encoded);
        assert_eq!(decoded, original);
    }

    // ── age_out_status tests ────────────────────────────────────────────────

    #[test]
    fn age_out_streaming() {
        // Must mirror determine_status' 120s window for a null stop_reason.
        // Aging out earlier (previously 8s) caused live-streaming sessions to
        // flicker into Idle between JSONL flush batches.
        let mut s = make_session(SessionStatus::Streaming);
        age_out_status(&mut s, 50.0);
        assert_eq!(s.status, SessionStatus::Streaming);
        age_out_status(&mut s, 119.0);
        assert_eq!(s.status, SessionStatus::Streaming);
        age_out_status(&mut s, 120.0);
        assert_eq!(s.status, SessionStatus::Idle);
        assert_eq!(s.token_speed, 0.0);
    }

    #[test]
    fn age_out_thinking() {
        let mut s = make_session(SessionStatus::Thinking);
        age_out_status(&mut s, 119.0);
        assert_eq!(s.status, SessionStatus::Thinking);
        age_out_status(&mut s, 120.0);
        assert_eq!(s.status, SessionStatus::Idle);
    }

    #[test]
    fn age_out_executing() {
        let mut s = make_session(SessionStatus::Executing);
        age_out_status(&mut s, 59.0);
        assert_eq!(s.status, SessionStatus::Executing);
        age_out_status(&mut s, 60.0);
        assert_eq!(s.status, SessionStatus::Idle);
    }

    #[test]
    fn age_out_waiting_input() {
        let mut s = make_session(SessionStatus::WaitingInput);
        age_out_status(&mut s, 299.0);
        assert_eq!(s.status, SessionStatus::WaitingInput);
        age_out_status(&mut s, 300.0);
        assert_eq!(s.status, SessionStatus::Idle);
    }

    #[test]
    fn age_out_waiting_input_zeros_speed_before_status_change() {
        // Fleet-wide token/cost speed totals should stop counting a waiting
        // session quickly, even though we keep the WaitingInput *status* for
        // 5 minutes so the user can still see the session's history context.
        let mut s = make_session(SessionStatus::WaitingInput);
        s.token_speed = 12.0;
        s.cost_speed_usd_per_min = 0.5;
        age_out_status(&mut s, 30.0);
        assert_eq!(s.token_speed, 0.0, "speed should zero at 30s idle");
        assert_eq!(s.cost_speed_usd_per_min, 0.0);
        assert_eq!(
            s.status,
            SessionStatus::WaitingInput,
            "status must still read WaitingInput until the 300s threshold"
        );
    }

    #[test]
    fn age_out_delegating_zeros_speed_before_status_change() {
        let mut s = make_session(SessionStatus::Delegating);
        s.token_speed = 20.0;
        s.cost_speed_usd_per_min = 1.0;
        age_out_status(&mut s, 30.0);
        assert_eq!(s.token_speed, 0.0);
        assert_eq!(s.cost_speed_usd_per_min, 0.0);
        assert_eq!(s.status, SessionStatus::Delegating);
    }

    #[test]
    fn age_out_idle_stays_idle() {
        let mut s = make_session(SessionStatus::Idle);
        s.token_speed = 0.0;
        age_out_status(&mut s, 9999.0);
        assert_eq!(s.status, SessionStatus::Idle);
    }

    // ── Bug fix: max_tokens should be WaitingInput ─────────────────────────

    #[test]
    fn status_max_tokens_waiting_input() {
        let lines = vec![
            user_msg(),
            assistant_msg(vec![text_block("I ran out of tokens")], Some("max_tokens")),
        ];
        assert_eq!(ds(&lines, 10.0, None), SessionStatus::WaitingInput);
    }

    // ── Bug fix: WaitingInput must not be promoted to Delegating ───────────

    #[test]
    fn delegating_does_not_override_waiting_input() {
        // Simulate: main session is WaitingInput, has an active subagent.
        // The main session should stay WaitingInput so notification fires.
        let mut sessions = vec![
            {
                let mut s = make_session(SessionStatus::WaitingInput);
                s.id = "main-session".into();
                s.is_subagent = false;
                s.parent_session_id = None;
                s
            },
            {
                let mut s = make_session(SessionStatus::Executing);
                s.id = "sub-agent-1".into();
                s.is_subagent = true;
                s.parent_session_id = Some("main-session".into());
                s
            },
        ];

        // Apply the same Delegating promotion logic from scan_claude_sessions
        let active_parent_ids: std::collections::HashSet<String> = sessions
            .iter()
            .filter(|s| {
                s.is_subagent
                    && matches!(
                        s.status,
                        SessionStatus::Thinking
                            | SessionStatus::Executing
                            | SessionStatus::Streaming
                            | SessionStatus::Delegating
                            | SessionStatus::Processing
                    )
            })
            .filter_map(|s| s.parent_session_id.clone())
            .collect();

        for session in &mut sessions {
            if !session.is_subagent
                && session.parent_session_id.is_none()
                && active_parent_ids.contains(&session.id)
                && matches!(
                    session.status,
                    SessionStatus::Active | SessionStatus::Idle | SessionStatus::Processing
                )
            {
                session.status = SessionStatus::Delegating;
            }
        }

        // WaitingInput must NOT be overridden to Delegating — notifications depend on it.
        assert_eq!(sessions[0].status, SessionStatus::WaitingInput);
    }

    // ── extract_last_context_usage ───────────────────────────────────────────

    fn asst_usage_line(input: u64, cache_create: u64, cache_read: u64, sidechain: bool) -> String {
        json!({
            "type": "assistant",
            "isSidechain": sidechain,
            "message": {
                "id": format!("msg-{input}-{cache_create}-{cache_read}"),
                "role": "assistant",
                "model": "claude-sonnet-4-20250514",
                "content": [{"type": "text", "text": "ok"}],
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": input,
                    "output_tokens": 10,
                    "cache_creation_input_tokens": cache_create,
                    "cache_read_input_tokens": cache_read
                }
            }
        }).to_string()
    }

    fn compact_summary_line() -> String {
        json!({
            "type": "user",
            "isCompactSummary": true,
            "isVisibleInTranscriptOnly": true,
            "message": {
                "role": "user",
                "content": "This session is being continued..."
            }
        }).to_string()
    }

    #[test]
    fn context_usage_picks_latest_assistant_turn() {
        // Three turns: latest is also the largest. (We test "max ≠ latest"
        // separately below.)
        let lines = vec![
            asst_usage_line(100, 0, 0, false),
            asst_usage_line(500, 100, 1000, false),
            asst_usage_line(200, 0, 5000, false),
        ];
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let (used, _model, max) = extract_last_context_usage(&refs).unwrap();
        assert_eq!(used, 200 + 5000);
        assert_eq!(max, 200 + 5000);
    }

    #[test]
    fn context_usage_max_can_exceed_latest() {
        // Latest turn can be smaller than an earlier peak (e.g. context shed
        // via tool-result cleanup or a /clear-style mid-session reset).
        let lines = vec![
            asst_usage_line(0, 0, 800_000, false), // big peak
            asst_usage_line(200, 0, 50_000, false), // smaller current
        ];
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let (used, _, max) = extract_last_context_usage(&refs).unwrap();
        assert_eq!(used, 50_200);
        assert_eq!(max, 800_000);
    }

    #[test]
    fn context_usage_skips_sidechain() {
        let lines = vec![
            asst_usage_line(500, 0, 10_000, false),
            asst_usage_line(999_999, 0, 0, true), // subagent — must be ignored
        ];
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let (used, _, max) = extract_last_context_usage(&refs).unwrap();
        assert_eq!(used, 500 + 10_000);
        assert_eq!(max, 500 + 10_000);
    }

    #[test]
    fn context_usage_resets_at_compact_boundary_when_no_post_compact_assistant() {
        // Old assistant turn, then a compact summary, then no new assistant yet.
        let lines = vec![
            asst_usage_line(1000, 0, 180_000, false),
            compact_summary_line(),
        ];
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        // Pre-compact data is stale, no live data yet → None.
        assert!(extract_last_context_usage(&refs).is_none());
    }

    #[test]
    fn context_usage_uses_post_compact_assistant() {
        let lines = vec![
            asst_usage_line(1000, 0, 180_000, false), // pre-compact, stale
            compact_summary_line(),
            asst_usage_line(200, 0, 8_000, false),    // fresh post-compact turn
        ];
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let (used, _, max) = extract_last_context_usage(&refs).unwrap();
        assert_eq!(used, 200 + 8_000);
        // Session max must NOT include the stale pre-compact turn.
        assert_eq!(max, 200 + 8_000);
    }

    #[test]
    fn context_window_explicit_1m_suffix() {
        assert_eq!(
            context_window_for_model("claude-sonnet-4-6[1m]", 0),
            Some(1_000_000)
        );
        assert_eq!(
            context_window_for_model("claude-sonnet-4-5", 0),
            Some(200_000)
        );
    }

    #[test]
    fn context_window_inferred_1m_for_opus_4_6() {
        // Observed a 530K cache_read turn → must be a 1M session.
        assert_eq!(
            context_window_for_model("claude-opus-4-6", 530_000),
            Some(1_000_000)
        );
        // Same model but only 50K observed → conservative 200K.
        assert_eq!(
            context_window_for_model("claude-opus-4-6", 50_000),
            Some(200_000)
        );
    }

    #[test]
    fn context_window_inferred_1m_for_sonnet_4_x() {
        assert_eq!(
            context_window_for_model("claude-sonnet-4-6", 250_000),
            Some(1_000_000)
        );
        assert_eq!(
            context_window_for_model("claude-sonnet-4-5", 250_000),
            Some(1_000_000)
        );
    }

    #[test]
    fn context_window_no_1m_inference_for_unsupported_families() {
        // Opus 4 / 4.1 don't support 1M — even if max>200K, stay at 200K
        // (which would clamp the percentage to 100, but at least won't lie
        // about the denominator).
        assert_eq!(
            context_window_for_model("claude-opus-4-1", 500_000),
            Some(200_000)
        );
        // Haiku 4.5 doesn't support 1M either.
        assert_eq!(
            context_window_for_model("claude-haiku-4-5", 500_000),
            Some(200_000)
        );
    }

    #[test]
    fn percent_uses_inferred_window() {
        // 250K used on Opus 4.6 with a session max of 530K → 25%, not capped.
        let pct =
            compute_context_percent(250_000, Some("claude-opus-4-6"), 530_000).unwrap();
        assert!((pct - 0.25).abs() < 1e-6);
    }
}

// ── Process kill helpers ─────────────────────────────────────────────────────

pub fn collect_process_tree(root_pid: u32) -> Vec<u32> {
    let output = match std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![root_pid],
    };
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let pid: u32 = match parts.next().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        let ppid: u32 = match parts.next().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        children.entry(ppid).or_default().push(pid);
    }

    let mut result = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(root_pid);
    while let Some(pid) = queue.pop_front() {
        result.push(pid);
        if let Some(kids) = children.get(&pid) {
            for &kid in kids {
                queue.push_back(kid);
            }
        }
    }
    result
}

/// Kill a process by PID (with process tree cleanup).
pub fn kill_pid_impl(pid: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        let pids = collect_process_tree(pid);
        crate::log_debug(&format!(
            "kill_pid: SIGTERM to {} pids (root={}): {:?}",
            pids.len(),
            pid,
            pids
        ));
        for &p in pids.iter().rev() {
            unsafe { libc::kill(p as libc::pid_t, libc::SIGTERM) };
        }

        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2000));
            for &p in pids.iter().rev() {
                if unsafe { libc::kill(p as libc::pid_t, 0) } == 0 {
                    unsafe { libc::kill(p as libc::pid_t, libc::SIGKILL) };
                }
            }
        });

        Ok(())
    }

    #[cfg(not(unix))]
    {
        std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .status()
            .map_err(|e| format!("taskkill failed: {e}"))?;
        Ok(())
    }
}

/// Kill all processes in a workspace.
pub fn kill_workspace_impl(workspace_path: &str) -> Result<(), String> {
    #[cfg(unix)]
    {
        let procs = scan_cli_processes();
        let root_pids: Vec<u32> = procs
            .iter()
            .filter(|p| p.cwd == workspace_path)
            .map(|p| p.pid)
            .collect();

        if root_pids.is_empty() {
            return Err(format!("No agent processes found in {}", workspace_path));
        }

        let mut all_pids: HashSet<u32> = HashSet::new();
        for &root in &root_pids {
            for pid in collect_process_tree(root) {
                all_pids.insert(pid);
            }
        }
        let pids: Vec<u32> = all_pids.into_iter().collect();

        crate::log_debug(&format!(
            "kill_workspace: SIGTERM to {} pids for workspace '{}': {:?}",
            pids.len(),
            workspace_path,
            pids
        ));

        for &p in pids.iter().rev() {
            unsafe { libc::kill(p as libc::pid_t, libc::SIGTERM) };
        }

        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2000));
            for &p in pids.iter().rev() {
                if unsafe { libc::kill(p as libc::pid_t, 0) } == 0 {
                    unsafe { libc::kill(p as libc::pid_t, libc::SIGKILL) };
                }
            }
        });

        Ok(())
    }

    #[cfg(not(unix))]
    {
        std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID"])
            .args(
                scan_cli_processes()
                    .iter()
                    .filter(|p| p.cwd == workspace_path)
                    .map(|p| p.pid.to_string())
                    .collect::<Vec<_>>(),
            )
            .status()
            .map_err(|e| format!("taskkill failed: {e}"))?;
        Ok(())
    }
}
