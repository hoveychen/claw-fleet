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
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
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
    Stuck,        // Fleet-spawned, process alive, but wedged mid tool-use batch:
                  // a non-interactive tool_use has been missing its tool_result
                  // for minutes (STUCK_TOOL_BATCH_FLOOR_SECS). The turn is
                  // deadlocked — SIGINT (interrupt_pid) unblocks it, resumable.
}

/// Populated when `SessionStatus::RateLimited`. Carries the information needed
/// for the UI countdown and the auto-resume scheduler. `parsed` is `false` when
/// `resets_at` is an estimate derived from `error_timestamp + fallback_duration`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct RateLimitState {
    pub resets_at: chrono::DateTime<chrono::Utc>,
    pub limit_type: crate::rate_limit_parser::RateLimitType,
    pub parsed: bool,
    pub error_timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub workspace_path: String,
    pub workspace_name: String,
    pub ide_name: Option<String>,
    /// Launch identity persisted by the Claude CLI into every `user` record
    /// (from the `CLAUDE_CODE_ENTRYPOINT` env var at spawn time): "cli",
    /// "claude-vscode", `session_launch::NEW_SESSION_ENTRYPOINT`, … Taken
    /// from the FIRST user record so later `--resume` runs (which stamp their
    /// own entrypoint) don't reclassify the session. `None` for transcripts
    /// predating the field.
    #[serde(default)]
    pub entrypoint: Option<String>,
    pub is_subagent: bool,
    pub parent_session_id: Option<String>,
    pub agent_type: Option<String>,
    pub agent_description: Option<String>,
    pub slug: Option<String>,
    pub ai_title: Option<String>,
    pub status: SessionStatus,
    pub token_speed: f64,
    /// Token speed of this session + all its subagents' speeds (main sessions
    /// only). For subagents this equals `token_speed`. Lets a parent card show
    /// the speed of its hidden workflow fan-out agents so the per-card sum
    /// reconciles with the global aggregate.
    pub agent_token_speed: f64,
    pub total_output_tokens: u64,
    /// Last finalized turn's total input tokens (`input_tokens +
    /// cache_creation_input_tokens + cache_read_input_tokens`) — i.e. the current
    /// context-window size, reset by each compaction. Matches the per-session
    /// `total_input_tokens` the daily report derives (`daily_report.rs`), so
    /// "today's cumulative" can sum input on the same口径. `0` when no usage seen
    /// yet or for sources (e.g. Codex) that don't track it.
    #[serde(default)]
    pub total_input_tokens: u64,
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
    /// True when a live CLI process carries this exact session id in its argv
    /// (`--session-id` on the first turn, `--resume` on follow-ups). Unlike
    /// `pid_precise` — which also goes true for a lone root process in the cwd
    /// that never named this session — this is a definitive liveness signal for
    /// Fleet-spawned sessions, and the only way to tell apart the two meanings
    /// of `WaitingInput`: "turn ended, process gone" (resumable) vs "process
    /// alive, parked on a decision card" (resuming would race a live turn).
    #[serde(default)]
    pub proc_alive: bool,
    /// True when the most recent assistant tool_use batch has a non-interactive
    /// tool whose `tool_result` never arrived (see
    /// [`has_pending_noninteractive_tool_batch`]). Computed at parse time and
    /// carried across cache hits; `apply_pid_liveness` combines it with
    /// `proc_alive` + an age floor to promote the status to `Stuck`.
    #[serde(default)]
    pub pending_tool_batch: bool,
    pub last_skill: Option<String>,
    /// Approximate context-window utilisation (0.0 – 1.0) derived from the
    /// last finalized assistant message's usage fields.  `None` when no
    /// usage data is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_percent: Option<f64>,
    /// Source of this session: "claude-code" or "codex"
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
    /// Aggregate TASKS.md plan progress for this session's workspace (active
    /// plans only, across the main checkout + sibling worktrees). `None` when
    /// PRD Discipline isn't in use (no TASKS.md / no active plan). Drives the
    /// compact task-progress row on the session card.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub task_plan: Option<crate::prd_tasks::TaskPlanSummary>,
    /// Background tasks (shells, monitors, …) that were still running the last
    /// time this session ended a turn — i.e. what it is waiting on. Empty for
    /// the overwhelming majority of sessions.
    ///
    /// Read from the `background_tasks` array of the Stop hook payload, which
    /// Fleet already records. Stamped at scan time from the hook snapshot rather
    /// than the cached deep parse: the task list changes while the jsonl doesn't.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub background_tasks: Vec<crate::bg_guard::BackgroundTask>,
    /// Relay-chain position when this session is part of a handoff chain
    /// (`fleet handoff`). `None` = never relayed. Stamped by
    /// `handoff::enrich_sessions` at scan time, not during the cached deep
    /// parse — chain membership changes while the predecessor's jsonl doesn't.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub handoff: Option<crate::handoff::SessionHandoffInfo>,
    /// Manual review mark set by the human ("done" / "pending"). `None` = the
    /// session is unmarked, i.e. "new / needs review". Orthogonal to `status`
    /// (which is the auto-computed run state). Stamped by
    /// `session_mark::enrich_sessions` at scan time, not during the cached deep
    /// parse — the mark changes while the session's jsonl doesn't.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user_mark: Option<crate::session_mark::SessionMark>,
    /// Epoch-ms of the last time the human read this session, or `None` if never
    /// read. Orthogonal to both `status` and `user_mark`: a session is "unread"
    /// when `last_activity_ms > last_read_ms` (or this is `None`). Stamped by
    /// `session_read::enrich_sessions` at scan time, not during the cached deep
    /// parse — the read state changes while the session's jsonl doesn't.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_read_ms: Option<u64>,
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


mod paths;
mod detect;
mod stats;
mod parse;
mod scan;
mod kill;

pub use self::paths::*;
pub use self::detect::*;
pub use self::stats::*;
pub use self::parse::*;
pub use self::scan::*;
pub use self::kill::*;

#[cfg(test)]
mod test_lock {
    use std::sync::MutexGuard;

    /// Acquire the cross-module `FLEET_HOME` mutex. Delegates to the single
    /// process-wide mutex in `crate::paths` so EVERY test that mutates the
    /// global `FLEET_HOME` env serialises on ONE lock — in-crate unit tests
    /// and separate integration-test binaries alike. Poison tolerance lives
    /// in the `crate::paths` impl.
    pub fn fleet_home_lock() -> MutexGuard<'static, ()> {
        crate::paths::fleet_home_lock()
    }
}

#[cfg(test)]
pub(crate) use test_lock::fleet_home_lock;

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

    fn interrupt_user_msg(for_tool_use: bool) -> Value {
        let text = if for_tool_use {
            "[Request interrupted by user for tool use]"
        } else {
            "[Request interrupted by user]"
        };
        json!({ "type": "user", "message": { "role": "user", "content": [{"type": "text", "text": text}] } })
    }

    fn user_tool_result_msg() -> Value {
        json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "tool_result", "tool_use_id": "abc", "content": "ok"}]
            }
        })
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

    // ── extract_entrypoint tests ───────────────────────────────────────────

    #[test]
    fn entrypoint_taken_from_first_user_record_only() {
        // A resumed session appends user records with the RESUMING process's
        // entrypoint ("cli" here) — classification must stick to the launch
        // identity from the first record.
        let l1 = json!({"type": "ai-title", "aiTitle": "t"}).to_string();
        let l2 = json!({"type": "user", "entrypoint": "claw-fleet-newsession",
                        "message": {"role": "user", "content": "hi"}}).to_string();
        let l3 = json!({"type": "user", "entrypoint": "cli",
                        "message": {"role": "user", "content": "continue"}}).to_string();
        let lines: Vec<&str> = vec![&l1, &l2, &l3];
        assert_eq!(
            extract_entrypoint(&lines).as_deref(),
            Some("claw-fleet-newsession")
        );
    }

    #[test]
    fn entrypoint_none_for_legacy_transcripts() {
        // Transcripts written by CLIs predating the field must classify as None.
        let l1 = json!({"type": "user", "message": {"role": "user", "content": "hi"}}).to_string();
        let lines: Vec<&str> = vec![&l1];
        assert_eq!(extract_entrypoint(&lines), None);
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

    #[test]
    fn ide_badge_stays_off_fleet_spawned_sessions() {
        // A VS Code lock in the workspace must not decorate (or auto-resume-
        // exclude) launchpad/handoff headless sessions that merely share the
        // cwd; genuinely interactive sessions keep the badge.
        let mut vscode = make_session(SessionStatus::Idle);
        vscode.entrypoint = Some("claude-vscode".into());
        vscode.ide_name = Some("Visual Studio Code".into());
        let mut launchpad = make_session(SessionStatus::Idle);
        launchpad.entrypoint = Some(crate::session_launch::NEW_SESSION_ENTRYPOINT.into());
        launchpad.ide_name = Some("Visual Studio Code".into());
        let mut handoff = make_session(SessionStatus::Idle);
        handoff.entrypoint = Some(crate::handoff::HANDOFF_ENTRYPOINT.into());
        handoff.ide_name = Some("Visual Studio Code".into());

        let mut sessions = vec![vscode, launchpad, handoff];
        strip_ide_name_from_fleet_spawns(&mut sessions);

        assert_eq!(
            sessions[0].ide_name.as_deref(),
            Some("Visual Studio Code"),
            "interactive IDE session must keep its badge"
        );
        assert_eq!(
            sessions[1].ide_name, None,
            "launchpad-spawned session must not inherit the workspace IDE badge"
        );
        assert_eq!(
            sessions[2].ide_name, None,
            "handoff-spawned session must not inherit the workspace IDE badge"
        );
    }

    fn make_session(status: SessionStatus) -> SessionInfo {
        SessionInfo {
            id: "test-session".into(),
            workspace_path: "/tmp/test".into(),
            workspace_name: "test".into(),
            ide_name: None,
            entrypoint: None,
            is_subagent: false,
            parent_session_id: None,
            agent_type: None,
            agent_description: None,
            slug: None,
            ai_title: None,
            status,
            token_speed: 10.0,
            agent_token_speed: 10.0,
            total_output_tokens: 500,
            total_input_tokens: 0,
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
            proc_alive: false,
            pending_tool_batch: false,
            last_skill: None,
            context_percent: None,
            agent_source: "claude-code".into(),
            last_outcome: None,
            rate_limit: None,
            todos: None,
            background_tasks: Vec::new(),
            task_plan: None, handoff: None, user_mark: None, last_read_ms: None,
            compact_count: 0,
            compact_pre_tokens: 0,
            compact_post_tokens: 0,
            compact_cost_usd: 0.0,
        }
    }

    #[test]
    fn sort_treats_rate_limited_as_active() {
        // A rate-limited session is unfinished work waiting to resume, so it
        // must sort alongside the active sessions (ahead of Idle), not get
        // buried in the idle tail.
        let mut active = make_session(SessionStatus::Thinking);
        active.created_at_ms = 100;
        let mut rate_limited = make_session(SessionStatus::RateLimited);
        rate_limited.created_at_ms = 200;
        let mut idle = make_session(SessionStatus::Idle);
        idle.created_at_ms = 50;

        let mut sessions = vec![idle, rate_limited, active];
        sort_sessions(&mut sessions);

        // Active group first (Thinking, RateLimited) ordered by created_at asc,
        // then the Idle session.
        assert_eq!(sessions[0].status, SessionStatus::Thinking);
        assert_eq!(sessions[1].status, SessionStatus::RateLimited);
        assert_eq!(sessions[2].status, SessionStatus::Idle);
    }

    // ── determine_status tests ──────────────────────────────────────────────

    /// Test wrapper that preserves pre-content-age semantics by passing the
    /// same value for both `file_age_secs` and `content_age_secs`. Tests that
    /// specifically care about the file-vs-content age distinction call
    /// `determine_status` directly with distinct values.
    fn ds(lines: &[Value], age: f64, hook: Option<&HookState>) -> SessionStatus {
        determine_status(lines, age, age, hook)
    }

    // ── stuck tool-batch detection ───────────────────────────────────────────

    fn tool_use_block_id(name: &str, id: &str) -> Value {
        json!({"type": "tool_use", "name": name, "id": id, "input": {}})
    }

    fn tool_result_msg(id: &str) -> Value {
        json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "tool_result", "tool_use_id": id, "content": "ok"}]
            }
        })
    }

    #[test]
    fn stuck_batch_all_resolved_is_false() {
        let lines = vec![
            user_msg(),
            assistant_msg(
                vec![tool_use_block_id("WebFetch", "t1"), tool_use_block_id("WebFetch", "t2")],
                None,
            ),
            tool_result_msg("t1"),
            tool_result_msg("t2"),
        ];
        assert!(!has_pending_noninteractive_tool_batch(&lines));
    }

    #[test]
    fn stuck_batch_missing_noninteractive_result_is_true() {
        // Mirrors the real incident: Agent + 2 WebFetch issued, one WebFetch
        // never returns its tool_result.
        let lines = vec![
            user_msg(),
            assistant_msg(
                vec![
                    tool_use_block_id("Agent", "agent1"),
                    tool_use_block_id("WebFetch", "wf_hung"),
                    tool_use_block_id("WebFetch", "wf_ok"),
                ],
                None,
            ),
            tool_result_msg("wf_ok"),
            tool_result_msg("agent1"),
        ];
        assert!(has_pending_noninteractive_tool_batch(&lines));
    }

    #[test]
    fn stuck_batch_only_askuserquestion_pending_is_false() {
        // A parked decision card is a legitimate user-wait, not a deadlock.
        let lines = vec![
            user_msg(),
            assistant_msg(vec![tool_use_block_id("AskUserQuestion", "ask1")], None),
        ];
        assert!(!has_pending_noninteractive_tool_batch(&lines));
    }

    #[test]
    fn stuck_batch_only_permission_prompt_pending_is_false() {
        let lines = vec![
            user_msg(),
            assistant_msg(
                vec![tool_use_block_id("mcp__fleet__fleet__permission_prompt", "perm1")],
                None,
            ),
        ];
        assert!(!has_pending_noninteractive_tool_batch(&lines));
    }

    #[test]
    fn stuck_batch_no_tool_use_is_false() {
        let lines = vec![
            user_msg(),
            assistant_msg(vec![text_block("just talking")], Some("end_turn")),
        ];
        assert!(!has_pending_noninteractive_tool_batch(&lines));
    }

    #[test]
    fn stuck_batch_uses_latest_batch_only() {
        // An earlier fully-resolved batch must not mask a later stuck one, and a
        // later fully-resolved batch must clear an earlier stuck-looking one.
        let earlier_stuck_later_ok = vec![
            assistant_msg(vec![tool_use_block_id("WebFetch", "old_hung")], None),
            // no result for old_hung, but a NEW batch supersedes it, fully resolved
            assistant_msg(vec![tool_use_block_id("Bash", "new1")], None),
            tool_result_msg("new1"),
        ];
        assert!(!has_pending_noninteractive_tool_batch(&earlier_stuck_later_ok));
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

    // Interrupt regression: user pressed Esc; JSONL ends with a synthetic
    // user message containing "[Request interrupted by user]". Must NOT be
    // treated as "model thinking about the user's prompt".
    #[test]
    fn status_interrupt_initial_thinking_not_thinking() {
        let lines = vec![
            user_msg(),
            json!({"type": "file-history-snapshot", "messageId": "m", "snapshot": {}}),
            interrupt_user_msg(false),
        ];
        let s = ds(&lines, 5.0, None);
        assert_ne!(s, SessionStatus::Thinking, "got {:?}", s);
    }

    #[test]
    fn status_interrupt_during_tool_use_not_thinking() {
        let lines = vec![
            user_msg(),
            assistant_msg(vec![thinking_block()], Some("tool_use")),
            assistant_msg(vec![tool_use_block("Bash")], Some("tool_use")),
            user_tool_result_msg(),
            interrupt_user_msg(true),
        ];
        let s = ds(&lines, 5.0, None);
        assert_ne!(s, SessionStatus::Thinking, "got {:?}", s);
        assert_ne!(s, SessionStatus::Executing, "got {:?}", s);
    }

    #[test]
    fn status_interrupt_overrides_hook_model_processing() {
        // hook_state is stale; the JSONL has a fresh interrupt marker that
        // must take precedence over Phase-0 hook overrides.
        let lines = vec![
            user_msg(),
            interrupt_user_msg(false),
        ];
        let s = ds(&lines, 20.0, Some(&HookState::ModelProcessing));
        assert_ne!(s, SessionStatus::Thinking, "got {:?}", s);
    }

    #[test]
    fn status_interrupt_for_tool_use_variant_not_executing() {
        let lines = vec![
            user_msg(),
            assistant_msg(vec![tool_use_block("Bash")], Some("tool_use")),
            interrupt_user_msg(true),
        ];
        let s = ds(&lines, 20.0, Some(&HookState::ToolExecuting));
        assert_ne!(s, SessionStatus::Executing, "got {:?}", s);
        assert_ne!(s, SessionStatus::Thinking, "got {:?}", s);
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

    // ── StatsAcc: incremental folding ───────────────────────────────────────
    //
    // The scan carries a StatsAcc across ticks and feeds it only the lines a
    // session appended since last time. These pin the property that makes that
    // sound: folding in batches must equal folding the whole file at once.

    /// A finalized assistant turn with an explicit id, so tests can control
    /// exactly which turns are duplicates of which.
    fn turn(id: &str, output: u64) -> String {
        json!({
            "type": "assistant",
            "timestamp": "2025-01-01T00:00:00Z",
            "message": {
                "id": id,
                "role": "assistant",
                "model": "claude-sonnet-4-20250514",
                "content": [{"type": "text", "text": "ok"}],
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 100,
                    "output_tokens": output,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": 0
                }
            }
        })
        .to_string()
    }

    #[test]
    fn stats_acc_batched_equals_whole_file() {
        let owned: Vec<String> = (0..30).map(|i| turn(&format!("m{i}"), 10)).collect();
        let lines: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();

        let whole = {
            let mut a = StatsAcc::new();
            a.push_lines(&lines);
            a.finish_at(0.0)
        };

        // Same lines, but handed over in three appends like the scanner would.
        let batched = {
            let mut a = StatsAcc::new();
            a.push_lines(&lines[..7]);
            a.push_lines(&lines[7..19]);
            a.push_lines(&lines[19..]);
            a.finish_at(0.0)
        };

        assert_eq!(batched.total_output_tokens, whole.total_output_tokens);
        assert_eq!(batched.total_cost_usd, whole.total_cost_usd);
        assert_eq!(whole.total_output_tokens, 300);
    }

    /// The reason `seen_msg_ids` is a full set and not a sliding window.
    ///
    /// Real transcripts re-log a finalized assistant message far from its first
    /// occurrence: across the 120 largest transcripts on disk, 100 duplicate
    /// pairs sit >100 lines apart and the widest gap measured was 2437. Here the
    /// duplicate lands in a *later batch* than the original — exactly the case a
    /// bounded window would miss, double-counting the turn's tokens and cost.
    #[test]
    fn stats_acc_dedups_a_duplicate_that_arrives_in_a_later_batch() {
        let first = turn("msg-repeated", 50);
        let filler: Vec<String> = (0..2500).map(|i| turn(&format!("f{i}"), 1)).collect();
        let dup = turn("msg-repeated", 50); // re-logged ~2500 lines later

        let mut acc = StatsAcc::new();
        acc.push_lines(&[first.as_str()]);
        let filler_refs: Vec<&str> = filler.iter().map(|s| s.as_str()).collect();
        acc.push_lines(&filler_refs);
        let before = acc.finish_at(0.0).total_output_tokens;

        acc.push_lines(&[dup.as_str()]); // must be recognised as already counted
        let after = acc.finish_at(0.0);

        assert_eq!(before, 50 + 2500);
        assert_eq!(
            after.total_output_tokens, before,
            "a re-logged msg id must not be counted twice, however many lines \
             separate it from the original"
        );
    }

    #[test]
    fn stats_acc_folds_compact_events_across_batches() {
        let c = |pre: u64, post: u64| {
            json!({
                "type": "system",
                "subtype": "compact_boundary",
                "compactMetadata": {"preTokens": pre, "postTokens": post}
            })
            .to_string()
        };
        let (a1, a2) = (c(1000, 100), c(2000, 200));

        let mut acc = StatsAcc::new();
        acc.push_lines(&[a1.as_str()]);
        acc.push_lines(&[a2.as_str()]);
        let s = acc.finish_at(0.0);

        assert_eq!(s.compact_count, 2);
        assert_eq!(s.compact_pre_tokens, 3000);
        assert_eq!(s.compact_post_tokens, 300);
        assert!(s.compact_cost_usd > 0.0);
    }

    #[test]
    fn prune_timed_keeps_the_speed_window_intact() {
        // Two turns inside the 5-minute window, one long past it.
        let now = 1_000_000.0;
        let mk = |ts: &str, id: &str| {
            json!({
                "type": "assistant",
                "timestamp": ts,
                "message": {
                    "id": id, "role": "assistant", "model": "claude-sonnet-4-20250514",
                    "content": [], "stop_reason": "end_turn",
                    "usage": {"input_tokens": 0, "output_tokens": 100,
                              "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}
                }
            })
            .to_string()
        };
        // 1_000_000s ≈ 1970-01-12T13:46:40Z; place two turns 60s and 120s back.
        let recent_a = mk("1970-01-12T13:44:40Z", "a"); // now - 120
        let recent_b = mk("1970-01-12T13:45:40Z", "b"); // now - 60

        let mut acc = StatsAcc::new();
        acc.push_lines(&[recent_a.as_str(), recent_b.as_str()]);
        let before = acc.finish_at(now);

        acc.prune_timed(now); // must not evict samples the window still needs
        let after = acc.finish_at(now);

        assert!(before.token_speed > 0.0, "sanity: window should be non-empty");
        assert_eq!(after.token_speed, before.token_speed);
        assert_eq!(after.cost_speed_usd_per_min, before.cost_speed_usd_per_min);
    }

    // ── SessionAcc: equivalence with the whole-file extractors ──────────────
    //
    // SessionAcc replaces four separate full-file passes. These tests pin it
    // against the originals rather than against hand-computed expectations, so
    // the incremental path cannot quietly drift from the batch-free behaviour.

    /// Fold a transcript in three uneven batches, the way the scanner would as
    /// the file grows.
    fn fold_in_batches(lines: &[&str]) -> SessionAcc {
        let mut acc = SessionAcc::new();
        let a = lines.len() / 3;
        let b = lines.len() * 2 / 3;
        acc.push_lines(&lines[..a]);
        acc.push_lines(&lines[a..b]);
        acc.push_lines(&lines[b..]);
        acc
    }

    fn user_line(entrypoint: Option<&str>) -> String {
        match entrypoint {
            Some(e) => json!({"type": "user", "entrypoint": e}).to_string(),
            None => json!({"type": "user"}).to_string(),
        }
    }

    #[test]
    fn session_acc_context_matches_whole_file_extractor() {
        let owned = vec![
            asst_usage_line(1000, 0, 0, false),
            asst_usage_line(5000, 0, 0, false),  // the session max
            asst_usage_line(200, 0, 0, true),    // sidechain — must be ignored
            asst_usage_line(3000, 0, 0, false),  // the latest live turn
        ];
        let lines: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();

        let expected = extract_last_context_usage(&lines);
        assert_eq!(fold_in_batches(&lines).context_usage(), expected);
        assert_eq!(expected.map(|(used, _, max)| (used, max)), Some((3000, 5000)));
    }

    /// A compact summary invalidates everything before it — including the
    /// running max. If the batch boundary falls before the compact line, a naive
    /// fold would carry the stale pre-compact max forward.
    #[test]
    fn session_acc_context_resets_at_a_compact_summary_across_batches() {
        let big = asst_usage_line(90_000, 0, 0, false); // huge, pre-compact
        let compact = json!({"type": "user", "isCompactSummary": true}).to_string();
        let after = asst_usage_line(4000, 0, 0, false);
        let owned = vec![big, compact, after];
        let lines: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();

        let expected = extract_last_context_usage(&lines);

        // Batch boundary deliberately placed so the pre-compact turn lands in an
        // earlier batch than the compact line.
        let mut acc = SessionAcc::new();
        acc.push_lines(&lines[..1]);
        acc.push_lines(&lines[1..]);

        assert_eq!(acc.context_usage(), expected);
        assert_eq!(
            expected.map(|(used, _, max)| (used, max)),
            Some((4000, 4000)),
            "the 90k pre-compact turn must not survive as the session max"
        );
    }

    /// The first `user` record settles the entrypoint even when it has none —
    /// a later record's entrypoint must not backfill it.
    #[test]
    fn session_acc_entrypoint_settles_on_the_first_user_record() {
        let owned = vec![user_line(None), user_line(Some("vscode"))];
        let lines: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();

        assert_eq!(extract_entrypoint(&lines), None, "sanity: original settles to None");
        assert_eq!(fold_in_batches(&lines).entrypoint(), None);
    }

    #[test]
    fn session_acc_entrypoint_matches_whole_file_extractor() {
        let owned = vec![user_line(Some("cli")), user_line(Some("vscode"))];
        let lines: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();

        let expected = extract_entrypoint(&lines);
        assert_eq!(expected.as_deref(), Some("cli"));
        assert_eq!(fold_in_batches(&lines).entrypoint(), expected);
    }

    #[test]
    fn session_acc_ai_title_takes_the_first_and_survives_batching() {
        let owned = vec![
            json!({"type": "user"}).to_string(),
            json!({"type": "ai-title", "aiTitle": "first"}).to_string(),
            json!({"type": "ai-title", "aiTitle": "second"}).to_string(),
        ];
        let lines: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
        assert_eq!(fold_in_batches(&lines).ai_title().as_deref(), Some("first"));
    }

    #[test]
    fn session_acc_todos_match_the_reverse_scan() {
        let todo = |content: &str, status: &str| {
            json!({
                "type": "assistant",
                "message": {"content": [{
                    "type": "tool_use", "name": "TodoWrite",
                    "input": {"todos": [{"content": content, "status": status,
                                         "activeForm": content}]}
                }]}
            })
            .to_string()
        };
        let owned = vec![todo("old", "completed"), todo("new", "in_progress")];
        let lines: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();

        let expected = crate::session_todos::latest_todo_summary_from_lines(&lines);
        assert!(expected.is_some(), "sanity: the fixture must carry a todo block");
        assert_eq!(fold_in_batches(&lines).todos(), expected);
    }

    #[test]
    fn session_acc_stats_match_compute_session_stats() {
        let owned: Vec<String> = (0..12).map(|i| turn(&format!("m{i}"), 7)).collect();
        let lines: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();

        let expected = compute_session_stats(&lines);
        let got = fold_in_batches(&lines).stats_at(0.0);
        assert_eq!(got.total_output_tokens, expected.total_output_tokens);
        assert_eq!(got.total_cost_usd, expected.total_cost_usd);
        assert_eq!(got.total_output_tokens, 84);
    }

    // ── Incremental read: offsets, partial lines, rewrites ──────────────────

    fn tmp_jsonl(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "incr_{}_{}_{}.jsonl",
            name,
            std::process::id(),
            // Distinct per test even within a process.
            name.len()
        ));
        let _ = fs::remove_file(&p);
        p
    }

    fn append(path: &Path, s: &str) {
        use std::io::Write;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        f.write_all(s.as_bytes()).unwrap();
    }

    /// The scan races the CLI's writes, so a half-written trailing line is
    /// normal. It must be folded once — when it is complete — and never lost.
    ///
    /// This is the case a naive `lines()`-to-EOF reader gets wrong: it would
    /// parse-fail on the fragment and still advance the offset past it, dropping
    /// that turn's tokens from the total permanently.
    #[test]
    fn incremental_defers_a_half_written_line_until_it_is_complete() {
        let p = tmp_jsonl("partial");
        let full = turn("m1", 100);

        // The CLI has flushed only the first half of the record.
        let split = full.len() / 2;
        append(&p, &full[..split]);

        let s1 = advance_incremental(&p, None).unwrap();
        assert_eq!(s1.offset, 0, "no complete line yet — offset must not move");
        assert_eq!(s1.acc.stats_at(0.0).total_output_tokens, 0);

        // The rest of the record lands, terminated.
        append(&p, &format!("{}\n", &full[split..]));

        let s2 = advance_incremental(&p, Some(s1)).unwrap();
        assert_eq!(
            s2.acc.stats_at(0.0).total_output_tokens,
            100,
            "the once-partial line must be folded exactly once, not lost"
        );
        assert_eq!(s2.offset, fs::metadata(&p).unwrap().len());

        let _ = fs::remove_file(&p);
    }

    #[test]
    fn incremental_folds_only_appended_bytes_and_matches_a_full_reparse() {
        let p = tmp_jsonl("append");
        append(&p, &format!("{}\n", turn("m1", 10)));
        append(&p, &format!("{}\n", turn("m2", 20)));

        let s1 = advance_incremental(&p, None).unwrap();
        assert_eq!(s1.acc.stats_at(0.0).total_output_tokens, 30);
        let after_first = s1.offset;

        append(&p, &format!("{}\n", turn("m3", 5)));
        let s2 = advance_incremental(&p, Some(s1)).unwrap();

        assert!(s2.offset > after_first);
        assert_eq!(s2.acc.stats_at(0.0).total_output_tokens, 35);

        // Equivalence with reading the whole file from scratch.
        let content = fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(
            s2.acc.stats_at(0.0).total_output_tokens,
            compute_session_stats(&lines).total_output_tokens
        );

        let _ = fs::remove_file(&p);
    }

    #[test]
    fn incremental_is_a_noop_when_nothing_was_appended() {
        let p = tmp_jsonl("noop");
        append(&p, &format!("{}\n", turn("m1", 10)));

        let s1 = advance_incremental(&p, None).unwrap();
        let s2 = advance_incremental(&p, Some(s1.clone())).unwrap();

        assert_eq!(s2.offset, s1.offset);
        assert_eq!(s2.acc.stats_at(0.0).total_output_tokens, 10);

        let _ = fs::remove_file(&p);
    }

    /// A file shorter than our offset cannot have been appended to — the carried
    /// accumulator is meaningless and must be rebuilt, not extended.
    #[test]
    fn incremental_reparses_from_scratch_when_the_file_shrinks() {
        let p = tmp_jsonl("shrink");
        append(&p, &format!("{}\n", turn("m1", 10)));
        append(&p, &format!("{}\n", turn("m2", 20)));
        let s1 = advance_incremental(&p, None).unwrap();
        assert_eq!(s1.acc.stats_at(0.0).total_output_tokens, 30);

        // Rewritten shorter, with different content.
        fs::write(&p, format!("{}\n", turn("z1", 7))).unwrap();

        let s2 = advance_incremental(&p, Some(s1)).unwrap();
        assert_eq!(
            s2.acc.stats_at(0.0).total_output_tokens,
            7,
            "stale pre-rewrite totals must not survive"
        );
        assert_eq!(s2.offset, fs::metadata(&p).unwrap().len());

        let _ = fs::remove_file(&p);
    }

    // ── parse_session_info: the incremental path must equal a cold parse ─────

    fn parse_for_test(p: &Path, incr: Option<IncrParse>) -> Option<(SessionInfo, IncrParse)> {
        parse_session_info(
            p,
            "sid".to_string(),
            "/tmp/ws".to_string(),
            "ws".to_string(),
            None,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            None,
            incr,
        )
    }

    /// The whole point of the change: a session advanced tick-by-tick as it grows
    /// must land on exactly the same numbers as one parsed cold from a full file.
    /// Anything else means the launchpad silently shows different tokens/cost
    /// depending on whether Fleet happened to be running while you worked.
    #[test]
    fn parse_session_info_incremental_equals_a_cold_parse() {
        let p = tmp_jsonl("parse_eq");
        append(&p, &format!("{}\n", json!({"type": "user", "entrypoint": "cli"})));
        append(&p, &format!("{}\n", json!({"type": "ai-title", "aiTitle": "T"})));
        append(&p, &format!("{}\n", turn("m1", 40)));

        // Scanned once while the session was mid-flight...
        let (_, incr) = parse_for_test(&p, None).unwrap();

        // ...then it keeps working, including a compact and a duplicate re-log.
        append(&p, &format!("{}\n", turn("m2", 60)));
        append(
            &p,
            &format!(
                "{}\n",
                json!({"type": "system", "subtype": "compact_boundary",
                       "compactMetadata": {"preTokens": 500, "postTokens": 50}})
            ),
        );
        append(&p, &format!("{}\n", turn("m2", 60))); // re-logged: must not double-count
        append(&p, &format!("{}\n", turn("m3", 5)));

        let (incremental, _) = parse_for_test(&p, Some(incr)).unwrap();
        let (cold, _) = parse_for_test(&p, None).unwrap();

        assert_eq!(incremental.total_output_tokens, cold.total_output_tokens);
        assert_eq!(incremental.total_cost_usd, cold.total_cost_usd);
        assert_eq!(incremental.compact_count, cold.compact_count);
        assert_eq!(incremental.compact_pre_tokens, cold.compact_pre_tokens);
        assert_eq!(incremental.context_percent, cold.context_percent);
        assert_eq!(incremental.ai_title, cold.ai_title);
        assert_eq!(incremental.entrypoint, cold.entrypoint);

        // And the dedup actually held across the scan boundary.
        assert_eq!(
            cold.total_output_tokens, 105,
            "m2 was logged twice and must be counted once: 40 + 60 + 5"
        );

        let _ = fs::remove_file(&p);
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

    // ── reconcile_model_spec tests ─────────────────────────────────────────
    //
    // Verified against the real CLI (2026-07-10): `claude --model` accepts all
    // of `opus[1m]`, `claude-opus-4-8[1m]`, `claude-opus-4-8` and
    // `claude-fable-5`, so re-attaching a suffix to a resolved id is legal.

    /// The transcript never records the `[1m]` opt-in, so a session running the
    /// configured `opus[1m]` default must not be relaunched on bare opus — that
    /// would silently drop it from a 1M window to 200K.
    #[test]
    fn reconcile_reapplies_the_configured_suffix_to_the_resolved_id() {
        assert_eq!(
            reconcile_model_spec("claude-opus-4-8", Some("opus[1m]")),
            "claude-opus-4-8[1m]"
        );
    }

    /// The suffix rides on the transcript's exact version, not on the alias, so
    /// a pinned older opus stays pinned.
    #[test]
    fn reconcile_keeps_the_transcript_version_not_the_alias() {
        assert_eq!(
            reconcile_model_spec("claude-opus-4-6", Some("opus[1m]")),
            "claude-opus-4-6[1m]"
        );
    }

    /// A session that overrode the default runs a different family; the default's
    /// suffix must not leak onto it.
    #[test]
    fn reconcile_ignores_suffix_across_family_mismatch() {
        assert_eq!(
            reconcile_model_spec("claude-fable-5", Some("opus[1m]")),
            "claude-fable-5"
        );
    }

    /// No suffix to preserve, or no configured default at all — the transcript id
    /// is already complete.
    #[test]
    fn reconcile_passes_through_when_nothing_to_reapply() {
        assert_eq!(
            reconcile_model_spec("claude-sonnet-5", Some("opus")),
            "claude-sonnet-5"
        );
        assert_eq!(reconcile_model_spec("claude-sonnet-5", None), "claude-sonnet-5");
        assert_eq!(reconcile_model_spec("claude-sonnet-5", Some("   ")), "claude-sonnet-5");
    }

    /// A full-name default with a suffix matches its own resolved id.
    #[test]
    fn reconcile_matches_full_name_configured_default() {
        assert_eq!(
            reconcile_model_spec("claude-opus-4-8", Some("claude-opus-4-8[1m]")),
            "claude-opus-4-8[1m]"
        );
    }

    /// The case `reconcile_model_spec` cannot solve, and doesn't have to: a
    /// session Fleet launched with an explicit `--model claude-opus-4-8[1m]`
    /// while `settings.json` defaults to a *different family*. The transcript
    /// records the bare `claude-opus-4-8`, and there is no suffix anywhere to
    /// reconcile against — reconstruction alone would silently relaunch this
    /// session on the 200K model and lose 800K of context window.
    ///
    /// Fleet doesn't have to reconstruct it: it chose the flag, so it wrote the
    /// flag down. The recorded launch spec outranks the transcript.
    #[test]
    fn explicit_launch_model_beats_the_transcript_reconstruction() {
        let _lock = fleet_home_lock();
        let home = std::env::temp_dir().join(format!(
            "fleet-launch-model-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let projects = home.join(".claude").join("projects").join("p");
        fs::create_dir_all(&projects).unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialized on the process-wide FLEET_HOME lock.
        unsafe { std::env::set_var("FLEET_HOME", &home) };

        // Settings default to another family, so there is no suffix to re-apply.
        fs::write(
            home.join(".claude").join("settings.json"),
            r#"{"model":"claude-fable-5"}"#,
        )
        .unwrap();
        // The transcript, as Claude Code writes it: resolved id, no suffix.
        fs::write(
            projects.join("sess-1m.jsonl"),
            "{\"type\":\"user\",\"cwd\":\"/ws\"}\n\
             {\"type\":\"assistant\",\"message\":{\"model\":\"claude-opus-4-8\",\"content\":[]}}\n",
        )
        .unwrap();

        // Without the launch note, all we can recover is the lossy bare id.
        assert_eq!(
            resolve_session_model_spec("sess-1m").as_deref(),
            Some("claude-opus-4-8"),
            "sanity: reconstruction alone cannot see the suffix"
        );

        // With it, the session comes back exactly as it was launched.
        crate::launch_spec::record("sess-1m", Some("claude-opus-4-8[1m]"), Some("high"));
        assert_eq!(
            resolve_session_model_spec("sess-1m").as_deref(),
            Some("claude-opus-4-8[1m]"),
            "a session Fleet launched must be relaunched on the model Fleet gave it"
        );

        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn split_model_suffix_cases() {
        assert_eq!(split_model_suffix("opus[1m]"), ("opus", Some("[1m]")));
        assert_eq!(split_model_suffix("claude-fable-5"), ("claude-fable-5", None));
        // malformed: an unterminated bracket is not a suffix
        assert_eq!(split_model_suffix("opus[1m"), ("opus[1m", None));
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

    #[test]
    fn resume_id_session_id_flag() {
        // Launchpad spawns pass `--session-id <uuid>` on the first turn.
        let cmd: Vec<std::ffi::OsString> = vec![
            "claude".into(),
            "-p".into(),
            "hi".into(),
            "--session-id".into(),
            "sess-new".into(),
        ];
        assert_eq!(extract_resume_id(&cmd), Some("sess-new".into()));
    }

    #[test]
    fn resume_id_session_id_equals_syntax() {
        let cmd: Vec<std::ffi::OsString> =
            vec!["claude".into(), "--session-id=sess-new".into()];
        assert_eq!(extract_resume_id(&cmd), Some("sess-new".into()));
    }

    // ── apply_pid_liveness tests ────────────────────────────────────────────

    fn make_launchpad_session(status: SessionStatus) -> SessionInfo {
        let mut s = make_session(status);
        s.entrypoint = Some(crate::session_launch::NEW_SESSION_ENTRYPOINT.into());
        s
    }

    #[test]
    fn pid_liveness_dead_process_downgrades_working_status() {
        for status in [
            SessionStatus::Thinking,
            SessionStatus::Executing,
            SessionStatus::Streaming,
            SessionStatus::Processing,
            SessionStatus::Active,
        ] {
            let mut s = make_launchpad_session(status.clone());
            apply_pid_liveness(&mut s, false, None, 0.0);
            assert_eq!(s.status, SessionStatus::Idle, "from {status:?}");
            assert_eq!(s.token_speed, 0.0);
        }
    }

    #[test]
    fn pid_liveness_dead_process_keeps_waiting_input() {
        // Normal end-of-turn: the -p process exits, the session waits for a
        // follow-up. That's WaitingInput, not a ghost — leave it alone.
        let mut s = make_launchpad_session(SessionStatus::WaitingInput);
        apply_pid_liveness(&mut s, false, None, 0.0);
        assert_eq!(s.status, SessionStatus::WaitingInput);
    }

    #[test]
    fn pid_liveness_stamps_proc_alive_even_when_status_is_untouched() {
        // The UI can't read `WaitingInput` as "resumable" on its own — it means
        // both "turn ended, process gone" and "alive, parked on a decision
        // card". `proc_alive` is what separates them, so it must be stamped on
        // every session, including ones this function returns early on.
        let mut dead = make_launchpad_session(SessionStatus::WaitingInput);
        apply_pid_liveness(&mut dead, false, None, 0.0);
        assert!(!dead.proc_alive);

        let mut alive = make_launchpad_session(SessionStatus::WaitingInput);
        apply_pid_liveness(&mut alive, true, None, 0.0);
        assert!(alive.proc_alive);

        let mut sub = make_launchpad_session(SessionStatus::WaitingInput);
        sub.is_subagent = true;
        apply_pid_liveness(&mut sub, false, None, 0.0);
        assert!(!sub.proc_alive);
    }

    #[test]
    fn pid_liveness_alive_process_promotes_idle() {
        // Blocked on a decision card for 20 minutes: age heuristics decayed
        // the status to Idle, but the process is provably alive and mid-turn.
        let mut s = make_launchpad_session(SessionStatus::Idle);
        apply_pid_liveness(&mut s, true, None, 0.0);
        assert_eq!(s.status, SessionStatus::WaitingInput);

        let mut s = make_launchpad_session(SessionStatus::Idle);
        apply_pid_liveness(&mut s, true, Some(&HookState::ToolExecuting), 0.0);
        assert_eq!(s.status, SessionStatus::Executing);

        let mut s = make_launchpad_session(SessionStatus::Idle);
        apply_pid_liveness(&mut s, true, Some(&HookState::ModelProcessing), 0.0);
        assert_eq!(s.status, SessionStatus::Thinking);
    }

    #[test]
    fn pid_liveness_stuck_batch_over_floor_becomes_stuck() {
        // Live Fleet process, transcript frozen mid-batch past the floor: the
        // deadlock this whole feature exists to surface. Must win even over the
        // ToolExecuting hook that would otherwise pin the card to Executing.
        let mut s = make_launchpad_session(SessionStatus::Executing);
        s.pending_tool_batch = true;
        apply_pid_liveness(
            &mut s,
            true,
            Some(&HookState::ToolExecuting),
            STUCK_TOOL_BATCH_FLOOR_SECS + 1.0,
        );
        assert_eq!(s.status, SessionStatus::Stuck);
        assert_eq!(s.token_speed, 0.0);
    }

    #[test]
    fn pid_liveness_stuck_batch_below_floor_stays_normal() {
        // Same incomplete batch but only briefly quiet — a merely-slow tool, not
        // a deadlock. Leave the normal promote/keep behaviour intact.
        let mut s = make_launchpad_session(SessionStatus::Executing);
        s.pending_tool_batch = true;
        apply_pid_liveness(
            &mut s,
            true,
            Some(&HookState::ToolExecuting),
            STUCK_TOOL_BATCH_FLOOR_SECS - 60.0,
        );
        assert_ne!(s.status, SessionStatus::Stuck);
        assert_eq!(s.status, SessionStatus::Executing);
    }

    #[test]
    fn pid_liveness_stuck_needs_live_process() {
        // A dead process is not "stuck" — it's just gone; the working-status
        // downgrade owns it. Stuck implies a live process worth interrupting.
        let mut s = make_launchpad_session(SessionStatus::Executing);
        s.pending_tool_batch = true;
        apply_pid_liveness(&mut s, false, None, STUCK_TOOL_BATCH_FLOOR_SECS + 1.0);
        assert_eq!(s.status, SessionStatus::Idle);
    }

    #[test]
    fn pid_liveness_stuck_only_for_fleet_spawned() {
        // Non-launchpad sessions (plain CLI/VS Code) return early — Fleet can't
        // safely interrupt a process it didn't spawn, so never flag them Stuck.
        let mut s = make_session(SessionStatus::Executing); // no NEW_SESSION entrypoint
        s.pending_tool_batch = true;
        apply_pid_liveness(
            &mut s,
            true,
            Some(&HookState::ToolExecuting),
            STUCK_TOOL_BATCH_FLOOR_SECS + 1.0,
        );
        assert_ne!(s.status, SessionStatus::Stuck);
    }

    #[test]
    fn pid_liveness_alive_process_keeps_fresh_status() {
        // A fine-grained status derived from a fresh transcript wins over the
        // coarse pid signal — only decayed-to-Idle gets re-promoted.
        let mut s = make_launchpad_session(SessionStatus::Streaming);
        apply_pid_liveness(&mut s, true, None, 0.0);
        assert_eq!(s.status, SessionStatus::Streaming);
    }

    #[test]
    fn pid_liveness_ignores_non_launchpad_sessions() {
        // IDE / terminal sessions never carry their id in argv, so process
        // absence proves nothing — the age heuristics stay authoritative.
        let mut s = make_session(SessionStatus::Thinking);
        apply_pid_liveness(&mut s, false, None, 0.0);
        assert_eq!(s.status, SessionStatus::Thinking);

        let mut s = make_launchpad_session(SessionStatus::Thinking);
        s.is_subagent = true;
        apply_pid_liveness(&mut s, false, None, 0.0);
        assert_eq!(s.status, SessionStatus::Thinking);
    }

    // ── resolve_pid tests ───────────────────────────────────────────────────

    #[test]
    fn resolve_pid_empty() {
        assert_eq!(resolve_pid(&[], "sess1"), (None, false));
    }

    fn argv(args: &[&str]) -> Vec<std::ffi::OsString> {
        args.iter().map(std::ffi::OsString::from).collect()
    }

    #[test]
    fn headless_argv_matches_fleets_own_spawn_shape() {
        // Verbatim from `ps` for a session Fleet spawned (prompt elided).
        assert!(is_headless_argv(&argv(&[
            "claude", "-p", "帮我查一下这个 handoff",
            "--session-id", "f74954c1-5deb-4098-889a-721e1d83ff1e",
            "--output-format", "stream-json", "--permission-mode", "acceptEdits",
        ])));
    }

    #[test]
    fn interactive_ide_argv_is_not_headless() {
        // Verbatim from `ps` for the VS Code extension's CLI: no `-p`, and it
        // keeps background shells alive across turns — must never be blocked.
        assert!(!is_headless_argv(&argv(&[
            "claude", "--output-format", "stream-json", "--verbose",
            "--input-format", "stream-json", "--permission-prompt-tool", "stdio",
            "--resume", "822b7957-4d5d-4d77-b84d-56f76a3acff3",
            "--permission-mode", "acceptEdits", "--include-partial-messages",
        ])));
    }

    #[test]
    fn a_prompt_mentioning_dash_p_is_not_headless() {
        // The prompt is one argv element, so its text can't be mistaken for the
        // flag — guard against a future switch to substring matching.
        assert!(!is_headless_argv(&argv(&[
            "claude", "--resume", "s1", "run it with -p and --print",
        ])));
    }

    #[test]
    fn long_form_print_flag_is_headless() {
        assert!(is_headless_argv(&argv(&["claude", "--print", "hi"])));
    }

    #[test]
    fn is_headless_session_needs_an_exact_session_match() {
        let procs = vec![
            CliProcess { pid: 1, ppid: None, cwd: "/w".into(), resume_session_id: Some("headless-one".into()), headless: true },
            CliProcess { pid: 2, ppid: None, cwd: "/w".into(), resume_session_id: Some("ide-one".into()), headless: false },
        ];
        assert!(is_headless_session_in(&procs, "headless-one"));
        assert!(!is_headless_session_in(&procs, "ide-one"));
        // A session no live process names — e.g. it already exited. Unknown must
        // not be treated as headless, or we'd block turns we know nothing about.
        assert!(!is_headless_session_in(&procs, "who-dis"));
        assert!(!is_headless_session_in(&[], "headless-one"));
    }

    #[test]
    fn resolve_pid_exact_resume_match() {
        let procs = vec![
            CliProcess { pid: 100, ppid: None, cwd: "/tmp".into(), resume_session_id: Some("sess1".into()), headless: false },
            CliProcess { pid: 200, ppid: None, cwd: "/tmp".into(), resume_session_id: None, headless: false },
        ];
        assert_eq!(resolve_pid(&procs, "sess1"), (Some(100), true));
    }

    #[test]
    fn resolve_pid_single_process() {
        let procs = vec![
            CliProcess { pid: 42, ppid: None, cwd: "/tmp".into(), resume_session_id: None, headless: false },
        ];
        assert_eq!(resolve_pid(&procs, "other"), (Some(42), true));
    }

    #[test]
    fn resolve_pid_parent_child_filtering() {
        let procs = vec![
            CliProcess { pid: 100, ppid: Some(1), cwd: "/tmp".into(), resume_session_id: None, headless: false },
            CliProcess { pid: 200, ppid: Some(100), cwd: "/tmp".into(), resume_session_id: None, headless: false },
        ];
        assert_eq!(resolve_pid(&procs, "any"), (Some(100), true));
    }

    #[test]
    fn resolve_pid_multiple_roots_imprecise() {
        let procs = vec![
            CliProcess { pid: 100, ppid: Some(1), cwd: "/tmp".into(), resume_session_id: None, headless: false },
            CliProcess { pid: 200, ppid: Some(2), cwd: "/tmp".into(), resume_session_id: None, headless: false },
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
    fn workspace_name_chat_workspace_is_renamed() {
        let tmp = tempfile::tempdir().unwrap();
        let _g = crate::session::fleet_home_lock();
        let prev = std::env::var_os("FLEET_HOME");
        unsafe { std::env::set_var("FLEET_HOME", tmp.path()) };
        let chat = tmp.path().join(".fleet/chat");
        assert_eq!(workspace_name(&chat.to_string_lossy()), "Chat");
        // A sibling directory keeps its basename.
        assert_eq!(workspace_name(&tmp.path().join(".fleet/wiki").to_string_lossy()), "wiki");
        match prev {
            Some(v) => unsafe { std::env::set_var("FLEET_HOME", v) },
            None => unsafe { std::env::remove_var("FLEET_HOME") },
        }
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
    fn workspace_name_worktree_uses_repo_name() {
        // Fleet develops each plan inside `<repo-root>/.worktrees/<task-id>`.
        // The workspace name must be the repo (`maliang`), not the task id.
        assert_eq!(
            workspace_name("/Users/hoveychen/workspace/maliang/.worktrees/script-runtime"),
            "maliang"
        );
    }

    #[test]
    fn workspace_name_worktree_trailing_slash() {
        assert_eq!(
            workspace_name("/Users/foo/my-repo/.worktrees/fix-bug/"),
            "my-repo"
        );
    }

    #[test]
    fn encode_decode_workspace_path() {
        let original = "/Users/foo/bar";
        let encoded = encode_workspace_path(original);
        assert_eq!(encoded, "-Users-foo-bar");
        let decoded = decode_workspace_path(&encoded);
        assert_eq!(decoded, original);
    }

    #[test]
    fn encode_workspace_path_matches_claude_sanitize_all_non_alnum() {
        // Claude Code's sanitizePath (sessionStoragePortable.ts) maps EVERY
        // non-alphanumeric char to '-', not just '/'. Fleet uses this encode
        // both to match the `~/.claude/projects/<dir>` name AND to locate CC's
        // scratchpad slug (`getProjectTempDir = join(tmp, sanitizePath(cwd))`).
        // Replacing only '/' left '.'/'_'/spaces intact, so those workspaces
        // never matched — a cross-platform bug, not Windows-specific.
        assert_eq!(
            encode_workspace_path("/Users/foo/my_app.v2"),
            "-Users-foo-my-app-v2"
        );
        assert_eq!(encode_workspace_path("/a b/c"), "-a-b-c");
        // A Windows drive path: ':' and '\' both collapse to '-'.
        assert_eq!(
            encode_workspace_path("C:\\Users\\foo\\proj"),
            "C--Users-foo-proj"
        );
    }

    #[test]
    fn encode_path_segment_collapses_all_non_alnum() {
        // Mirrors sanitizePath at the single-segment level so read_level_dirs
        // re-encodes on-disk names the way CC encoded them.
        assert_eq!(encode_path_segment("my_app.v2"), "my-app-v2");
        assert_eq!(encode_path_segment("a b"), "a-b");
        assert_eq!(encode_path_segment("plain"), "plain");
    }

    #[test]
    fn windows_drive_split_recognizes_drive_letter_prefix() {
        // "C:\Users\foo" → sanitizePath → "C--Users-foo" → split → these parts.
        // The single-letter head + empty second part is the `--` from `:`+`\`.
        assert_eq!(
            super::windows_drive_split(&["C", "", "Users", "foo"]),
            Some(("C", &["Users", "foo"][..]))
        );
        // Empties inside the path are preserved for the fs walk to disambiguate.
        assert_eq!(
            super::windows_drive_split(&["D", "", "a", "", "b"]),
            Some(("D", &["a", "", "b"][..]))
        );
        // A unix-style leading-dash encoding ("-Users-foo" → ["Users","foo"])
        // has no empty second part → not a drive.
        assert_eq!(super::windows_drive_split(&["Users", "foo"]), None);
        // A two-char head is not a drive letter.
        assert_eq!(super::windows_drive_split(&["ab", "", "x"]), None);
        // Single letter but no empty second part (a real 1-char dir) → not a drive.
        assert_eq!(super::windows_drive_split(&["a", "b"]), None);
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
    fn age_out_rate_limited_zeros_speed() {
        // A rate-limited agent is blocked by the API and generating nothing,
        // but its 5-minute window still holds the pre-ratelimit burst tokens,
        // so `token_speed` stays frozen at a high value. Without zeroing it,
        // every rate-limited session keeps inflating the fleet-wide totals
        // (observed live: 21 RateLimited agents contributing ~2730 ghost tok/s
        // against only ~230 tok/s of real generation). Zero immediately —
        // there is no age at which a rate-limited session is generating.
        let mut s = make_session(SessionStatus::RateLimited);
        s.token_speed = 200.0;
        s.cost_speed_usd_per_min = 4.0;
        age_out_status(&mut s, 0.0);
        assert_eq!(s.token_speed, 0.0, "rate-limited session must not report speed");
        assert_eq!(s.cost_speed_usd_per_min, 0.0);
        assert_eq!(
            s.status,
            SessionStatus::RateLimited,
            "status stays RateLimited so the UI can still show the limit card"
        );
    }

    #[test]
    fn age_out_zeros_agent_token_speed_rollup() {
        // The per-card rollup (agent_token_speed = own speed + every subagent's
        // speed) must also drop when a session ages out. age_out_status used to
        // zero only token_speed, leaving agent_token_speed frozen at the value
        // it held when the session was last active — so an idle card with zero
        // live subagents still displayed a stale "0.0 · 64.0" second number
        // (verified live: 9 idle ashare-mon mains showed nonzero rollups while
        // pointing at 0 subagents). The post-age-out subagent rollup re-adds any
        // genuinely-active subagent afterwards, so zeroing the base here is safe.
        for status in [
            SessionStatus::WaitingInput,
            SessionStatus::Delegating,
            SessionStatus::RateLimited,
            SessionStatus::Streaming, // ages to Idle at >=120s
        ] {
            let mut s = make_session(status.clone());
            s.token_speed = 50.0;
            s.agent_token_speed = 600.0;
            s.cost_speed_usd_per_min = 3.0;
            age_out_status(&mut s, 300.0);
            assert_eq!(s.token_speed, 0.0, "own speed must zero for {status:?}");
            assert_eq!(
                s.agent_token_speed, 0.0,
                "rollup (agent_token_speed) must zero for {status:?}"
            );
        }
    }

    #[test]
    fn age_out_idle_stays_idle() {
        let mut s = make_session(SessionStatus::Idle);
        s.token_speed = 0.0;
        age_out_status(&mut s, 9999.0);
        assert_eq!(s.status, SessionStatus::Idle);
    }

    // Regression: card stuck on Thinking forever; restarting Fleet fixes it.
    // Root cause: when the filesystem watcher drops the jsonl event for the
    // model's final end_turn message, the session's source stays "clean".
    // incremental_rescan_and_emit then keeps the cached SessionInfo and only
    // runs age_out_status on it — and age_out_status is a one-way ladder that
    // can only downgrade an active status to Idle. It must NEVER promote a
    // stale Thinking into WaitingInput from age alone, because without
    // re-reading the JSONL we cannot know whether the turn actually ended.
    // The fix has to happen elsewhere (re-trigger rescan when hooks.jsonl
    // grows, or add a periodic full-scan heartbeat). This test locks the
    // current invariant in place so any future patch makes the upgrade path
    // explicit rather than smuggling it in here.
    #[test]
    fn age_out_thinking_never_upgrades_to_waiting_input() {
        for age in [0.0_f64, 1.0, 30.0, 60.0, 119.0, 120.0, 200.0, 5000.0] {
            let mut s = make_session(SessionStatus::Thinking);
            age_out_status(&mut s, age);
            assert_ne!(
                s.status,
                SessionStatus::WaitingInput,
                "age_out_status must never produce WaitingInput from Thinking (age={age})"
            );
        }
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
    fn context_window_inferred_1m_for_opus_4_7() {
        // Opus 4.7 also supports 1M; without this, on-disk transcripts that
        // don't carry the `[1m]` flag would default the denominator to 200K.
        assert_eq!(
            context_window_for_model("claude-opus-4-7", 530_000),
            Some(1_000_000)
        );
        assert_eq!(
            context_window_for_model("claude-opus-4-7", 50_000),
            Some(200_000)
        );
    }

    #[test]
    fn context_window_sonnet_1m_only_from_4_6() {
        // Per Anthropic's model docs (2026-05): Sonnet gained 1M at 4.6.
        // Sonnet 4.6 → 1M.
        assert_eq!(
            context_window_for_model("claude-sonnet-4-6", 250_000),
            Some(1_000_000)
        );
        // Sonnet 4.5 is a 200K model — even with a >195K observed turn it must
        // NOT be promoted to 1M (its real ceiling is ~200K, so 100% there is a
        // truthful "full", not a fake one).
        assert_eq!(
            context_window_for_model("claude-sonnet-4-5", 250_000),
            Some(200_000)
        );
    }

    #[test]
    fn context_window_no_1m_inference_for_unsupported_families() {
        // Opus 4 / 4.1 / 4.5 don't support 1M — even if max>200K, stay at 200K
        // (which would clamp the percentage to 100, but at least won't lie
        // about the denominator). 1M landed at Opus 4.6.
        assert_eq!(
            context_window_for_model("claude-opus-4-1", 500_000),
            Some(200_000)
        );
        assert_eq!(
            context_window_for_model("claude-opus-4-5", 500_000),
            Some(200_000)
        );
        // Haiku 4.5 doesn't support 1M either.
        assert_eq!(
            context_window_for_model("claude-haiku-4-5", 500_000),
            Some(200_000)
        );
    }

    #[test]
    fn context_window_mythos_is_1m() {
        // Claude Mythos Preview (Project Glasswing) ships with a 1M window.
        // Its dated id is invitation-only / unpublished, so we match the family
        // substring; assert on a representative id shape.
        assert_eq!(
            context_window_for_model("claude-mythos-preview", 530_000),
            Some(1_000_000)
        );
    }

    #[test]
    fn context_window_fable_is_1m() {
        // Claude Fable 5 (claude-fable-5) ships with a 1M window. It's a new
        // family token, so the opus/sonnet version gate doesn't match it —
        // it must be whitelisted like mythos. Without this, the denominator
        // defaults to 200K and the UI shows a fake "ctx 100%".
        assert_eq!(
            context_window_for_model("claude-fable-5", 530_000),
            Some(1_000_000)
        );
    }

    #[test]
    fn context_window_fable_mythos_always_1m_even_with_low_observed() {
        // Fable 5 and Mythos 5 have no 200K variant — they are always 1M.
        // Unlike opus/sonnet (whose 1M is inferred only once a turn exceeds
        // 200K), these must report 1M from the very first turn, when observed
        // input is still tiny. Otherwise a fresh Fable session briefly shows a
        // fake 200K window / inflated ctx-%.
        for model in ["claude-fable-5", "fable", "claude-mythos-5", "claude-mythos-preview"] {
            assert_eq!(
                context_window_for_model(model, 0),
                Some(1_000_000),
                "{model} should report 1M regardless of observed tokens"
            );
        }
    }

    #[test]
    fn context_window_inferred_1m_for_opus_4_8_and_future_versions() {
        // Opus 4.8 (the model that shipped after the hard-coded 4-6/4-7
        // whitelist) must be recognised as 1M-capable — otherwise its
        // denominator defaults to 200K and the UI shows a fake "ctx 100%".
        assert_eq!(
            context_window_for_model("claude-opus-4-8", 530_000),
            Some(1_000_000)
        );
        // Future Opus minors (4.9, 4.10) and future majors (5.x, 6.x) should
        // be auto-recognised without another whitelist edit.
        assert_eq!(
            context_window_for_model("claude-opus-4-10", 530_000),
            Some(1_000_000)
        );
        assert_eq!(
            context_window_for_model("claude-opus-5-0", 530_000),
            Some(1_000_000)
        );
        assert_eq!(
            context_window_for_model("claude-opus-6-2", 530_000),
            Some(1_000_000)
        );
        // Future Sonnet majors must also be recognised — today's code only
        // matched the literal "sonnet-4".
        assert_eq!(
            context_window_for_model("claude-sonnet-5-0", 530_000),
            Some(1_000_000)
        );
        // Below-threshold observed input still yields the conservative 200K.
        assert_eq!(
            context_window_for_model("claude-opus-4-8", 50_000),
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

    #[test]
    fn process_start_time_returns_value_for_self_and_is_stable() {
        let pid = std::process::id();
        let first = super::process_start_time(pid).expect("self should have a start time");
        // start_time is seconds since epoch — a real process can never report 0
        // (that would be 1970-01-01) so this also doubles as a "didn't return
        // a zero-valued sentinel" check.
        assert!(first > 0, "self start_time should be > 0, got {first}");
        let second = super::process_start_time(pid).expect("self still alive");
        assert_eq!(first, second, "start_time must be stable across calls");
    }

    #[test]
    fn process_start_time_returns_none_for_clearly_dead_pid() {
        // PID 0 is reserved on Unix; treat as the "definitely not a real
        // process" sentinel. macOS sysinfo also reports it as missing.
        assert!(
            super::process_start_time(0).is_none(),
            "pid 0 should never have a start_time"
        );
    }

    #[test]
    fn process_start_time_for_fresh_child_is_near_now() {
        // Spawn a long-lived helper, then assert its captured start_time
        // sits within a small window around `SystemTime::now()`. Catches
        // the sandboxed-sysinfo regression mode (returning 0 / None) and
        // any future drift where we accidentally read a wrong field.
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock not before epoch")
            .as_secs();
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep child");
        let captured = super::process_start_time(child.id())
            .expect("freshly-spawned child must have a start_time");
        let _ = child.kill();
        let _ = child.wait();

        assert!(captured > 0, "start_time must be a real unix-epoch value, got {captured}");
        let delta = if captured > now_secs {
            captured - now_secs
        } else {
            now_secs - captured
        };
        assert!(
            delta <= 5,
            "child captured at {captured} should be within 5s of now {now_secs} (delta = {delta})"
        );
    }

    #[test]
    fn deserialize_holders_accepts_legacy_and_new_shapes() {
        // Mixed array: bare pid (legacy) + full object (new). Both must
        // deserialise without error; the legacy entry gets start_time_secs 0.
        let mixed = r#"[123, {"pid": 456, "start_time_secs": 99}]"#;
        let mut d = serde_json::Deserializer::from_str(mixed);
        let out = super::deserialize_holders(&mut d).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].pid, 123);
        assert_eq!(out[0].start_time_secs, 0);
        assert_eq!(out[1].pid, 456);
        assert_eq!(out[1].start_time_secs, 99);
    }

    #[test]
    fn holder_entry_capture_self_records_real_start_time() {
        let my_pid = std::process::id();
        let real = super::process_start_time(my_pid).expect("self alive");
        let entry = super::HolderEntry::capture(my_pid);
        assert_eq!(entry.pid, my_pid);
        assert_eq!(
            entry.start_time_secs, real,
            "capture must record the live start_time, not 0"
        );
    }

    #[test]
    fn shared_prune_drops_pid_reused_holder() {
        // Direct test of the helper on a raw Vec<HolderEntry>, independent
        // of either injector's lock wrapper struct.
        let my_pid = std::process::id();
        let real = super::process_start_time(my_pid).expect("self alive");
        let mut holders = vec![
            super::HolderEntry::capture(my_pid), // matches → kept
            super::HolderEntry {                  // pid alive but start_time wrong → pruned
                pid: my_pid,
                start_time_secs: real.wrapping_add(1),
            },
        ];
        super::prune_dead_holders(&mut holders);
        assert_eq!(holders.len(), 1, "exactly the matching entry survives");
        assert_eq!(holders[0].start_time_secs, real);
    }

    /// Regression: a session whose workspace directory no longer exists (e.g. a
    /// deleted git worktree) and which has been idle past the keep window must
    /// be filtered out of the scan — its Resume can never work. Sessions whose
    /// workspace still exists stay visible regardless of age.
    ///
    /// A *recently active* session whose workspace vanished is the other half of
    /// this contract and stays visible; see
    /// `scan_keeps_recently_active_sessions_whose_workspace_was_removed`.
    #[test]
    fn scan_hides_sessions_whose_workspace_was_deleted() {
        use std::path::Path;
        use std::time::{Duration, SystemTime};
        let tmp = tempfile::tempdir().unwrap();

        let claude_dir = tmp.path().join("claude_home");
        let projects = claude_dir.join("projects");
        fs::create_dir_all(&projects).unwrap();

        // A minimal but parseable session JSONL written `now` (well within the
        // 7-day freshness window parse_session_info requires).
        let write_session = |proj_dir: &Path, id: &str, modified: Option<SystemTime>| {
            fs::create_dir_all(proj_dir).unwrap();
            let line = json!({
                "type": "user",
                "message": {"role": "user", "content": "hi"},
                "timestamp": "2026-06-14T00:00:00.000Z"
            });
            let path = proj_dir.join(format!("{id}.jsonl"));
            fs::write(&path, format!("{line}\n")).unwrap();
            if let Some(t) = modified {
                fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_modified(t)
                    .unwrap();
            }
        };

        // Workspace that still exists on disk → its session must survive.
        let live_ws = tmp.path().join("live-workspace");
        fs::create_dir_all(&live_ws).unwrap();
        let live_encoded = live_ws.to_string_lossy().replace('/', "-");
        let live_id = "11111111-1111-1111-1111-111111111111";
        write_session(&projects.join(&live_encoded), live_id, None);

        // Workspace deleted (never created) and long idle → session must be hidden.
        let gone_ws = tmp.path().join("deleted-worktree");
        let gone_encoded = gone_ws.to_string_lossy().replace('/', "-");
        let gone_id = "22222222-2222-2222-2222-222222222222";
        write_session(
            &projects.join(&gone_encoded),
            gone_id,
            Some(SystemTime::now() - Duration::from_secs(30 * 24 * 3600)),
        );

        let cache = ScanCache::new();
        let sessions = scan_claude_sessions(&claude_dir, &cache);
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();

        assert!(
            ids.contains(&live_id),
            "session in an existing workspace must remain visible: {ids:?}"
        );
        assert!(
            !ids.contains(&gone_id),
            "long-idle session whose workspace was deleted must be filtered out: {ids:?}"
        );
    }

    /// Regression: Claude Code encodes `/`, `.` AND `_` all to `-` in the
    /// projects dir name, so a live workspace whose name contains `_` (or `.`)
    /// must still resolve back to its real on-disk path and stay visible. The
    /// lossy `-`→`/`-only decode hid every such session (e.g. `kol_dash`).
    #[test]
    fn scan_keeps_sessions_whose_workspace_name_has_underscore() {
        use std::path::Path;
        let tmp = tempfile::tempdir().unwrap();

        let claude_dir = tmp.path().join("claude_home");
        let projects = claude_dir.join("projects");
        fs::create_dir_all(&projects).unwrap();

        let write_session = |proj_dir: &Path, id: &str| {
            fs::create_dir_all(proj_dir).unwrap();
            let line = json!({
                "type": "user",
                "message": {"role": "user", "content": "hi"},
                "timestamp": "2026-06-14T00:00:00.000Z"
            });
            fs::write(proj_dir.join(format!("{id}.jsonl")), format!("{line}\n")).unwrap();
        };

        // Mimic Claude Code's path encoding: every `/`, `.`, and `_` becomes `-`.
        let claude_encode = |p: &Path| -> String {
            p.to_string_lossy()
                .chars()
                .map(|c| if c == '/' || c == '.' || c == '_' { '-' } else { c })
                .collect()
        };

        // A live workspace whose directory name contains an underscore.
        let us_ws = tmp.path().join("kol_dash");
        fs::create_dir_all(&us_ws).unwrap();
        let us_encoded = claude_encode(&us_ws);
        let us_id = "33333333-3333-3333-3333-333333333333";
        write_session(&projects.join(&us_encoded), us_id);

        let cache = ScanCache::new();
        let sessions = scan_claude_sessions(&claude_dir, &cache);
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();

        assert!(
            ids.contains(&us_id),
            "session in an existing workspace with an underscore in its name \
             must remain visible: {ids:?}"
        );
    }

    /// Regression: sessions whose workspace directory is gone used to be hidden
    /// unconditionally. But Rule 3 has the agent `git worktree remove` its own
    /// checkout when the plan merges, and the handoff relay used to spawn
    /// successors inside that worktree — so a *live* session, still holding an
    /// unanswered decision card, would lose its cwd and disappear from the scan.
    /// Its card then lost the workspace label and the session-history panel
    /// (both resolve the session by id against this list).
    ///
    /// Keep the recently active ones; keep hiding the stale zombies the filter
    /// was written for (their Resume can never work).
    #[test]
    fn scan_keeps_recently_active_sessions_whose_workspace_was_removed() {
        use std::path::Path;
        use std::time::{Duration, SystemTime};
        let tmp = tempfile::tempdir().unwrap();

        let claude_dir = tmp.path().join("claude_home");
        let projects = claude_dir.join("projects");
        fs::create_dir_all(&projects).unwrap();

        let claude_encode = |p: &Path| -> String {
            p.to_string_lossy()
                .chars()
                .map(|c| if c == '/' || c == '.' || c == '_' { '-' } else { c })
                .collect()
        };

        // A workspace that no longer exists — a merged-and-removed worktree.
        let gone_ws = tmp.path().join("goneworktree");
        let gone_dir = projects.join(claude_encode(&gone_ws));
        fs::create_dir_all(&gone_dir).unwrap();

        let write_session = |dir: &Path, id: &str, modified: SystemTime| {
            let line = json!({
                "type": "user",
                "message": {"role": "user", "content": "hi"},
                "timestamp": "2026-07-11T00:00:00.000Z"
            });
            let path = dir.join(format!("{id}.jsonl"));
            fs::write(&path, format!("{line}\n")).unwrap();
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(modified)
                .unwrap();
        };

        // Still being written to — the session outlived its worktree.
        let live_id = "44444444-4444-4444-4444-444444444444";
        write_session(&gone_dir, live_id, SystemTime::now());

        // Long dead: a transcript of a worktree removed weeks ago.
        let zombie_id = "55555555-5555-5555-5555-555555555555";
        write_session(
            &gone_dir,
            zombie_id,
            SystemTime::now() - Duration::from_secs(30 * 24 * 3600),
        );

        let cache = ScanCache::new();
        let sessions = scan_claude_sessions(&claude_dir, &cache);
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();

        assert!(
            ids.contains(&live_id),
            "a session still active after its worktree was removed must stay \
             visible — its pending decision card resolves workspace + history \
             through this list: {ids:?}"
        );
        assert!(
            !ids.contains(&zombie_id),
            "a long-dead transcript of a removed worktree must stay hidden: {ids:?}"
        );
    }
}

