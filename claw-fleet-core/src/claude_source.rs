//! Claude Code agent source — scans ~/.claude/projects/ for JSONL session files.

use std::path::PathBuf;

use serde_json::Value;

use crate::agent_source::{AgentSource, ResumeSpec, WatchStrategy};
use crate::backend::SourceUsageSummary;
use crate::memory::{MemoryHistoryEntry, WorkspaceMemory};
use crate::session::{get_claude_dir, CliProcess, SessionInfo, SessionStatus};

/// The `agent_source` id stamped on every Claude Code session (see
/// `parse_session_info`). Mirror of `codex_launch::FLEET_AGENT_SOURCE_CODEX`.
pub const FLEET_AGENT_SOURCE_CLAUDE: &str = "claude-code";

pub struct ClaudeCodeSource {
    pub scan_cache: crate::session::ScanCache,
}

impl ClaudeCodeSource {
    pub fn new() -> Self {
        Self {
            scan_cache: crate::session::ScanCache::new(),
        }
    }
}

impl AgentSource for ClaudeCodeSource {
    fn name(&self) -> &'static str {
        "claude-code"
    }

    fn uri_prefix(&self) -> &'static str {
        "" // bare file paths
    }

    fn api_name(&self) -> &'static str {
        "claude"
    }

    fn is_available(&self) -> bool {
        get_claude_dir().map(|d| d.is_dir()).unwrap_or(false)
    }

    fn scan_sessions(&self) -> Vec<SessionInfo> {
        match get_claude_dir() {
            Some(dir) => crate::session::scan_claude_sessions(&dir, &self.scan_cache),
            None => vec![],
        }
    }

    fn get_messages(&self, path: &str) -> Result<Vec<Value>, String> {
        let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Ok(content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect())
    }

    fn get_messages_tail(&self, path: &str, n: usize) -> Result<Vec<Value>, String> {
        crate::jsonl_tail::read_tail_lines_as_json(std::path::Path::new(path), n)
            .map_err(|e| e.to_string())
    }

    fn watch_strategy(&self) -> WatchStrategy {
        WatchStrategy::Filesystem
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        // Only watch the subdirectories we actually care about, instead of the
        // entire ~/.claude tree (which contains caches, configs, etc. that
        // generate noise and trigger unnecessary rescans).
        let Some(dir) = get_claude_dir() else {
            return vec![];
        };
        let mut paths = Vec::new();
        let projects = dir.join("projects");
        if projects.is_dir() {
            paths.push(projects);
        }
        let ide = dir.join("ide");
        if ide.is_dir() {
            paths.push(ide);
        }
        paths
    }

    fn trigger_extensions(&self) -> Vec<&'static str> {
        vec!["jsonl", "lock"]
    }

    fn fetch_account(&self) -> Result<Value, String> {
        let info = crate::account::fetch_account_info_blocking()?;
        serde_json::to_value(&info).map_err(|e| e.to_string())
    }

    fn fetch_usage(&self) -> Result<Value, String> {
        // Claude's account info includes usage data.
        self.fetch_account()
    }

    fn usage_summary(&self) -> Option<SourceUsageSummary> {
        let info = crate::account::fetch_account_info_blocking().ok()?;
        Some(SourceUsageSummary::from_claude(&info))
    }

    fn kill_pid(&self, pid: u32) -> Result<(), String> {
        crate::session::kill_pid_impl(pid)
    }

    fn kill_workspace(&self, workspace_path: &str) -> Result<(), String> {
        crate::session::kill_workspace_impl(workspace_path)
    }

    fn list_memories(&self) -> Vec<WorkspaceMemory> {
        crate::memory::scan_all_memories()
    }

    fn get_memory_content(&self, path: &str) -> Result<String, String> {
        crate::memory::read_memory_file(path)
    }

    fn get_memory_history(&self, path: &str) -> Vec<MemoryHistoryEntry> {
        crate::memory::trace_memory_history(path)
    }

    fn spawn(
        &self,
        spec: &crate::agent_source::SpawnSpec,
    ) -> Result<crate::session_launch::SpawnSessionResponse, String> {
        // Empty entrypoint → the "新会话" default, so a bare spawn is still
        // classified as Fleet-owned by the scanner.
        let entrypoint = if spec.entrypoint.is_empty() {
            crate::session_launch::NEW_SESSION_ENTRYPOINT
        } else {
            spec.entrypoint.as_str()
        };
        crate::session_launch::spawn_new_session_impl(
            &spec.workspace_path,
            &spec.prompt,
            spec.model.as_deref(),
            spec.effort.as_deref(),
            spec.permission_mode.as_deref(),
            spec.session_id.as_deref(),
            entrypoint,
        )
    }

    fn resume(
        &self,
        spec: &ResumeSpec,
        on_exit: Box<dyn FnOnce(bool) + Send>,
    ) -> Result<(), String> {
        crate::auto_resume::spawn_resume_tracked_prompt(
            &spec.session_id,
            &spec.workspace_path,
            &spec.prompt,
            spec.model.as_deref(),
            spec.effort.as_deref(),
            spec.permission_mode.as_deref(),
            on_exit,
        )
    }
}

// ── Dead-process liveness reconciliation ─────────────────────────────────────

/// Reconcile a stale `proc_alive == true` on Claude Code sessions against a live
/// CLI process set. Split from [`refresh_dead_claude_liveness`] so tests can
/// inject a synthetic process slice instead of scanning the real table. Returns
/// true iff any session was clamped.
fn reconcile_claude_liveness(sessions: &mut [SessionInfo], processes: &[CliProcess]) -> bool {
    let mut changed = false;
    for s in sessions.iter_mut() {
        if s.agent_source != FLEET_AGENT_SOURCE_CLAUDE || !s.proc_alive {
            continue;
        }
        // Still alive? A live CLI process whose argv names exactly this session
        // id — the same signal `apply_pid_liveness` stamps `proc_alive` from. A
        // session id is globally unique, so this needs no cwd filter.
        if processes
            .iter()
            .any(|p| p.resume_session_id.as_deref() == Some(s.id.as_str()))
        {
            continue;
        }
        s.proc_alive = false;
        // Mirror `apply_pid_liveness`'s dead-process branch: an in-flight status
        // left frozen by the last transcript write is a ghost once the process
        // is gone — downgrade it so the "会话运行中" UI, the pending-message
        // drain, and auto-resume all unstick. RateLimited / ServerErrored /
        // WaitingInput / Idle / Stuck are terminal display states and MUST
        // survive untouched — auto-resume keys on RateLimited, so clobbering it
        // here would silently disable the very recovery this reconciliation
        // exists to unblock.
        if matches!(
            s.status,
            SessionStatus::Thinking
                | SessionStatus::Executing
                | SessionStatus::Streaming
                | SessionStatus::Processing
                | SessionStatus::Active
                | SessionStatus::Delegating
        ) {
            s.status = SessionStatus::Idle;
            s.token_speed = 0.0;
            s.agent_token_speed = 0.0;
            s.cost_speed_usd_per_min = 0.0;
        }
        changed = true;
    }
    changed
}

/// Heal Claude Code sessions whose cached snapshot still reads `proc_alive ==
/// true` but whose CLI process has actually exited.
///
/// A headless `claude -p` turn that ends by hitting the usage/session limit
/// writes its `error: "rate_limit"` synthetic message and then **exits** — and
/// that exit produces no further transcript write. The desktop rebuilds a Claude
/// session's `proc_alive` only when its fs-watcher sees a write under
/// `~/.claude/projects/`, so the process death produces no event and `proc_alive`
/// stays frozen `true`. That jams three time-gated recovery paths that all key on
/// `!proc_alive`: the "会话运行中" enqueue UI (`canEnqueueSession`), the
/// pending-message drain, and auto-resume (`select_resume_candidates` filters
/// `!proc_alive`). Since a rate limit clears at a wall-clock `resets_at` — never a
/// filesystem event — the session sits stuck until some *unrelated* write happens
/// to wake the watcher.
///
/// On macOS the watcher fires often enough (noisy FSEvents from other active
/// sessions, `hooks.jsonl` appends, …) that a full rescan heals `proc_alive`
/// within seconds, masking the bug. On Windows `ReadDirectoryChangesW` is precise:
/// a quiet machine delivers no events, so the freeze persists for as long as the
/// machine stays idle (observed: 38 minutes past a reset).
///
/// Called off the periodic ticker — which fires regardless of file writes — this
/// recomputes liveness against the live process table and clamps the dead ones so
/// all three gates unstick. Mirror of
/// [`crate::codex_source::refresh_dead_codex_liveness`]. Returns true iff any
/// session changed (the caller should re-emit the snapshot).
pub fn refresh_dead_claude_liveness(sessions: &mut [SessionInfo]) -> bool {
    // Cheap early-out: scan the process table only when there is a Claude session
    // actually flagged live to reconcile.
    if !sessions
        .iter()
        .any(|s| s.agent_source == FLEET_AGENT_SOURCE_CLAUDE && s.proc_alive)
    {
        return false;
    }
    let processes = crate::session::scan_cli_processes();
    reconcile_claude_liveness(sessions, &processes)
}

#[cfg(test)]
mod liveness_tests {
    use super::*;

    fn mk(id: &str, source: &str, status: SessionStatus, proc_alive: bool) -> SessionInfo {
        let mut s = SessionInfo::default();
        s.id = id.to_string();
        s.agent_source = source.to_string();
        s.status = status;
        s.proc_alive = proc_alive;
        s
    }

    fn proc(session_id: &str) -> CliProcess {
        CliProcess {
            pid: 1,
            ppid: None,
            cwd: "/w".into(),
            resume_session_id: Some(session_id.to_string()),
            headless: true,
        }
    }

    #[test]
    fn clamps_dead_claude_session_and_preserves_rate_limited() {
        // A rate-limited Claude session whose process has exited: proc_alive must
        // flip to false so auto-resume/drain can fire, but the RateLimited status
        // must survive (auto-resume keys on it).
        let mut sessions = vec![mk(
            "dead-rl",
            FLEET_AGENT_SOURCE_CLAUDE,
            SessionStatus::RateLimited,
            true,
        )];
        // No process names this session → it is gone.
        assert!(reconcile_claude_liveness(&mut sessions, &[]));
        assert!(!sessions[0].proc_alive, "dead session proc_alive cleared");
        assert_eq!(
            sessions[0].status,
            SessionStatus::RateLimited,
            "RateLimited must survive so auto-resume still fires"
        );
    }

    #[test]
    fn downgrades_frozen_in_flight_status() {
        let mut sessions = vec![mk(
            "dead-thinking",
            FLEET_AGENT_SOURCE_CLAUDE,
            SessionStatus::Thinking,
            true,
        )];
        assert!(reconcile_claude_liveness(&mut sessions, &[]));
        assert!(!sessions[0].proc_alive);
        assert_eq!(sessions[0].status, SessionStatus::Idle);
    }

    #[test]
    fn leaves_live_claude_session_untouched() {
        let mut sessions = vec![mk(
            "alive",
            FLEET_AGENT_SOURCE_CLAUDE,
            SessionStatus::RateLimited,
            true,
        )];
        // A live process names this session → still running.
        assert!(!reconcile_claude_liveness(&mut sessions, &[proc("alive")]));
        assert!(sessions[0].proc_alive);
    }

    #[test]
    fn ignores_non_claude_sources() {
        // A codex session with a stale flag must be left to the codex reconciler.
        let mut sessions = vec![mk("codex-dead", "codex", SessionStatus::Thinking, true)];
        assert!(!reconcile_claude_liveness(&mut sessions, &[]));
        assert!(sessions[0].proc_alive, "codex session left for its own path");
    }

    #[test]
    fn early_out_when_nothing_flagged_live() {
        // Nothing flagged proc_alive → refresh must not even scan (returns false).
        let mut sessions = vec![mk(
            "dead",
            FLEET_AGENT_SOURCE_CLAUDE,
            SessionStatus::Idle,
            false,
        )];
        assert!(!refresh_dead_claude_liveness(&mut sessions));
    }
}
