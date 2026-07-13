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
    Stuck,        // Fleet-spawned, process alive, but wedged mid tool-use batch:
                  // a non-interactive tool_use has been missing its tool_result
                  // for minutes (STUCK_TOOL_BATCH_FLOOR_SECS). The turn is
                  // deadlocked — SIGINT (interrupt_pid) unblocks it, resumable.
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

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Returns the real user home directory, robust to a redirected `$HOME`.
///
/// Origin: the desktop app used to ship with the macOS App Sandbox enabled,
/// where `dirs::home_dir()` returns the container path
/// (`~/Library/Containers/<id>/Data/`). The sandbox was dropped from
/// entitlements.plist in 2026-07, but this stays the canonical home lookup:
/// `getpwuid` reads the passwd database, so `~/.claude/`, `~/.fleet/`,
/// `~/.ssh/` etc. resolve correctly even under a polluted/overridden `$HOME`.
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

/// Read the process start time (seconds since the Unix epoch) for `pid`.
///
/// Returns `None` if no live process owns this pid right now. Used together
/// with [`is_process_alive`] to defend against PID reuse: an entry in a
/// lock file becomes "stale" if either the pid is dead OR the live process
/// at that pid was spawned at a different start time than the one
/// snapshotted on `acquire`. See
/// `~/.claude/projects/.../memory/project_mcp_injector_pid_reuse.md` for
/// the diagnosis that motivated this helper.
///
/// **Why not sysinfo?** A previous version used
/// `sysinfo::refresh_processes_specifics(Some(&[pid]), …)`, but on macOS
/// that path first calls `libc::proc_listallpids` to enumerate every pid
/// on the host *before* filtering — and the macOS App Sandbox returns 0
/// from `proc_listallpids`, so the refresh becomes a no-op, the process
/// isn't recorded, and `sysinfo.process(pid)` returns `None`. Combined
/// with `unwrap_or(0)` upstream, every holder in a sandboxed Fleet build
/// silently captured `start_time_secs = 0`, neutralising the PID-reuse
/// defence. The macOS branch below therefore calls `libc::proc_pidinfo`
/// directly for the target pid, which was permitted even inside the sandbox.
/// The desktop app has since dropped the App Sandbox (2026-07), but the
/// direct call stays — it skips the full-host pid enumeration sysinfo does.
#[cfg(target_os = "macos")]
pub fn process_start_time(pid: u32) -> Option<u64> {
    use std::mem;
    if pid == 0 {
        return None;
    }
    let mut info: libc::proc_bsdinfo = unsafe { mem::zeroed() };
    let size = mem::size_of::<libc::proc_bsdinfo>() as i32;
    let result = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut libc::proc_bsdinfo as *mut std::ffi::c_void,
            size,
        )
    };
    if result != size {
        return None;
    }
    Some(info.pbi_start_tvsec as u64)
}

/// Linux equivalent: read `/proc/<pid>/stat` field 22 (`starttime` in
/// clock ticks since boot) and convert to Unix epoch via `/proc/stat`'s
/// `btime` line. We do this directly rather than via sysinfo so the
/// macOS and Linux paths share the same "no global pid enumeration"
/// shape — handy if Linux ever gains an equivalent sandbox restriction.
#[cfg(target_os = "linux")]
pub fn process_start_time(pid: u32) -> Option<u64> {
    if pid == 0 {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // /proc/<pid>/stat format:
    //   pid (comm) state ppid pgrp ... starttime ...
    // The comm field can contain spaces and parens, so we slice off
    // everything up to and including the LAST ')' before parsing the
    // remaining whitespace-separated fields. After that slice, field
    // indexes are 1-based starting at `state`; starttime is field 22
    // overall, i.e. (22 - 2) = 20 fields after the ')'. Zero-based
    // index 19.
    let after_comm = stat.rfind(')')?;
    let rest = &stat[after_comm + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let ticks: u64 = fields.get(19)?.parse().ok()?;
    let btime = read_linux_btime()?;
    let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if clk_tck <= 0 {
        return None;
    }
    Some(btime + ticks / clk_tck as u64)
}

#[cfg(target_os = "linux")]
fn read_linux_btime() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    for line in stat.lines() {
        if let Some(rest) = line.strip_prefix("btime ") {
            return rest.trim().parse().ok();
        }
    }
    None
}

/// Windows fallback: there's no app-sandbox blocking pid enumeration in
/// our Windows distribution, so sysinfo's path remains the simplest
/// option. If Windows ever gains a similar concern, swap this to
/// `NtQueryInformationProcess(ProcessTimes)` directly.
#[cfg(windows)]
pub fn process_start_time(pid: u32) -> Option<u64> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    if pid == 0 {
        return None;
    }
    let mut sys = System::new();
    let target = Pid::from_u32(pid);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[target]),
        true,
        ProcessRefreshKind::nothing(),
    );
    sys.process(target).map(|p| p.start_time())
}

/// A live-process holder of a Fleet lock file (mcp-lock / permissions-lock /
/// any future sibling). Pairs the holder's pid with its `start_time_secs`
/// so [`prune_dead_holders`] can defeat PID reuse: when the OS later
/// recycles `pid` to an unrelated process, the recycled process's
/// start_time differs from the snapshot, so the entry is correctly pruned.
///
/// `start_time_secs = 0` is reserved as the "legacy / unknown" marker — it
/// never matches a real live process (start_time is seconds-since-epoch),
/// so legacy lock files (written before this field existed) prune to
/// empty on first read. See [`deserialize_holders`] for the on-disk
/// migration path and `project_mcp_injector_pid_reuse` memory for the
/// original diagnosis.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct HolderEntry {
    pub pid: u32,
    #[serde(default)]
    pub start_time_secs: u64,
}

impl HolderEntry {
    /// Build a fresh entry for `pid`, snapshotting its current start_time.
    /// If the pid no longer resolves (process already gone), the
    /// snapshot falls back to 0 — and the next `prune_dead_holders` will
    /// drop the entry on the spot.
    pub fn capture(pid: u32) -> Self {
        Self {
            pid,
            start_time_secs: process_start_time(pid).unwrap_or(0),
        }
    }
}

/// Drop holder entries whose process is no longer alive **or** whose
/// recorded `start_time_secs` no longer matches the live process at
/// that pid. The start_time match is the PID-reuse defence.
pub fn prune_dead_holders(holders: &mut Vec<HolderEntry>) {
    holders.retain(|h| {
        is_process_alive(h.pid)
            && process_start_time(h.pid)
                .map(|t| t == h.start_time_secs)
                .unwrap_or(false)
    });
}

/// Custom serde deserializer that accepts both the legacy `[u32, ...]`
/// shape (written by older Fleet builds) and the new
/// `[{pid, start_time_secs}, ...]` shape. Legacy entries are mapped to
/// `HolderEntry { pid, start_time_secs: 0 }` so `prune_dead_holders`
/// drops them on first read after upgrade.
pub fn deserialize_holders<'de, D>(d: D) -> Result<Vec<HolderEntry>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Item {
        Legacy(u32),
        New(HolderEntry),
    }
    let items: Vec<Item> = Vec::deserialize(d)?;
    Ok(items
        .into_iter()
        .map(|i| match i {
            Item::Legacy(pid) => HolderEntry { pid, start_time_secs: 0 },
            Item::New(e) => e,
        })
        .collect())
}

#[cfg(windows)]
pub fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    use std::ffi::c_void;
    type Handle = *mut c_void;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    const ERROR_INVALID_PARAMETER: u32 = 87;
    extern "system" {
        fn OpenProcess(
            dw_desired_access: u32,
            b_inherit_handle: i32,
            dw_process_id: u32,
        ) -> Handle;
        fn CloseHandle(h_object: Handle) -> i32;
        fn GetExitCodeProcess(h_process: Handle, lp_exit_code: *mut u32) -> i32;
        fn GetLastError() -> u32;
    }
    // SAFETY: standard Win32 calls. Handle is closed in every success path.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            // Mirror Unix EPERM-as-alive: ERROR_INVALID_PARAMETER means
            // the pid doesn't refer to any process; any other error
            // (e.g. ERROR_ACCESS_DENIED) means the process exists but
            // we don't have rights to query it.
            return GetLastError() != ERROR_INVALID_PARAMETER;
        }
        let mut exit_code: u32 = 0;
        let got = GetExitCodeProcess(h, &mut exit_code) != 0;
        CloseHandle(h);
        got && exit_code == STILL_ACTIVE
    }
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
        // Build a map of (encoded dir name) → (real dir name) for the current
        // level. Claude collapses `/`, `.` AND `_` all into `-`, so the real
        // on-disk directory name may differ from the dash-joined candidate by a
        // `.` or `_` — matching real entries by their re-encoded form recovers
        // names like `kol_dash` that a `-`→`/`-only decode cannot.
        let level_dirs = read_level_dirs(&current);

        // Try longest remaining segment first: join parts[i..] with '-', then parts[i..len-1], etc.
        let mut matched = false;
        // Try from longest (all remaining parts) down to single part
        for end in (i + 1..=parts.len()).rev() {
            let candidate_segment = parts[i..end].join("-");
            if let Some(real) = level_dirs.get(&candidate_segment) {
                current = format!("{}/{}", current, real);
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

/// Re-encode a single directory entry name the way Claude Code encodes paths:
/// `/`, `.`, and `_` all collapse to `-`. (An entry name never contains `/`,
/// but `.` and `_` are common.)
fn encode_path_segment(name: &str) -> String {
    name.chars()
        .map(|c| if c == '.' || c == '_' { '-' } else { c })
        .collect()
}

/// List the immediate sub-directories of `parent` (empty string = filesystem
/// root) keyed by their Claude-encoded name. A directory literally named with
/// `-` wins over a `.`/`_` collision so an exact path still round-trips.
///
/// TCC-safe: never reads a protected directory, and resolves an entry's type
/// from the readdir `d_type` (no per-entry `stat`) so listing `~` doesn't fire a
/// macOS permission dialog for `~/Documents`, `~/Downloads`, etc. Symlinks are
/// only followed when their target is not TCC-protected.
fn read_level_dirs(parent: &str) -> std::collections::HashMap<String, String> {
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let dir = if parent.is_empty() { "/" } else { parent };
    let dir_path = std::path::Path::new(dir);
    if crate::tcc::is_tcc_protected(dir_path) {
        return map;
    }
    let Ok(entries) = std::fs::read_dir(dir_path) else {
        return map;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let is_dir = match entry.file_type() {
            Ok(ft) if ft.is_dir() => true,
            Ok(ft) if ft.is_symlink() => {
                let target = entry.path();
                !crate::tcc::is_tcc_protected(&target) && target.is_dir()
            }
            _ => false,
        };
        if !is_dir {
            continue;
        }
        let key = encode_path_segment(&name);
        // A literal name (no `.`/`_` rewritten) is the unambiguous decoding, so
        // let it overwrite any earlier `.`/`_` collision; otherwise keep the
        // first seen.
        if name == key || !map.contains_key(&key) {
            map.insert(key, name);
        }
    }
    map
}

pub(crate) fn encode_workspace_path(path: &str) -> String {
    // "/Users/foo/bar-baz" → "-Users-foo-bar-baz"  (inverse of decode, but lossless for matching)
    path.replace('/', "-")
}

/// Human-facing name for a workspace path. Shared with the memory module so
/// both derive worktree names identically.
pub(crate) fn workspace_name(path: &str) -> String {
    // The pure-chat workspace is a Fleet-owned directory, not a project — its
    // `chat` basename would read as a random folder in the session list.
    if crate::chat_workspace::is_chat_workspace(path) {
        return crate::chat_workspace::CHAT_WORKSPACE_NAME.to_string();
    }
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    // Fleet develops plans inside `<repo-root>/.worktrees/<task-id>` (see the
    // worktree workflow). Such a checkout belongs to the repo, so name it after
    // the segment before `.worktrees` rather than the task-id leaf.
    if let Some(idx) = segments.iter().position(|s| *s == ".worktrees") {
        if idx > 0 {
            return segments[idx - 1].to_string();
        }
    }
    segments
        .last()
        .copied()
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
fn is_headless_argv(cmd: &[std::ffi::OsString]) -> bool {
    cmd.iter()
        .any(|arg| arg == "-p" || arg == "--print")
}

fn extract_resume_id(cmd: &[std::ffi::OsString]) -> Option<String> {
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
fn strip_ide_name_from_fleet_spawns(sessions: &mut [SessionInfo]) {
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
fn has_pending_noninteractive_tool_batch(last_lines: &[Value]) -> bool {
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

fn determine_status(
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

// ── Context window helpers ────────────────────────────────────────────────────

/// Whether a Claude model belongs to the family that can be opted in to a
/// Parse the `major.minor` version that follows a Claude family token in a
/// model id, e.g. `claude-opus-4-8` + `"opus"` → `Some((4, 8))`. Returns
/// `None` for aliases without a version (`"opus"`, `"sonnet"`) or when the
/// family token isn't present. Trailing date stamps (`claude-opus-4-1-2026…`)
/// are ignored — only the first two numeric tokens after the family count.
fn claude_family_version(model_lower: &str, family: &str) -> Option<(u32, u32)> {
    let rest = model_lower.split(family).nth(1)?;
    let mut nums = rest
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<u32>().ok());
    let major = nums.next()?;
    let minor = nums.next().unwrap_or(0);
    Some((major, minor))
}

/// Whether a Claude model ships with a 1M-token context window. Verified
/// against Anthropic's model docs (2026-05): the 1M window landed at the **.6**
/// minor for BOTH Opus and Sonnet, and every later major carries it forward.
/// Parsing the family + `major.minor` (instead of hard-coding each release)
/// means future minors (Opus 4.9, 4.10) and future majors (5.x, 6.x) are
/// recognised without another edit:
///   * Opus & Sonnet — 4.6+ within major 4, and every later major (5.x …).
///   * Mythos research preview (Project Glasswing) — always 1M.
///   * Opus ≤4.5, Sonnet ≤4.5, all Haiku, Claude 3.x — 200K.
///
/// Note Sonnet 4 / 4.5 are 200K models: Sonnet 4's brief 1M was a public-beta
/// header, not the default, and Sonnet 4.5 never shipped 1M.
fn claude_model_supports_1m(model_lower: &str) -> bool {
    // Always-1M families (Fable, Mythos) are handled unconditionally by
    // `claude_model_always_1m` before this version-gated check is reached, so
    // they're intentionally not matched here.
    // Opus and Sonnet share the same 4.6+ gate; Haiku never qualifies.
    for family in ["opus", "sonnet"] {
        if let Some((major, minor)) = claude_family_version(model_lower, family) {
            return major > 4 || (major == 4 && minor >= 6);
        }
    }
    false
}

/// Claude families that ship a 1M window on **every** release, with no 200K
/// variant. Unlike the opus/sonnet version gate (whose 1M is only *inferred*
/// once a turn's observed input exceeds 200K — Claude Code never writes the
/// `[1m]` flag to JSONL), these report 1M unconditionally, from the first turn:
///   * Claude Fable 5 (`claude-fable-5`) — always 1M.
///   * Mythos / Mythos research preview (Project Glasswing) — always 1M.
fn claude_model_always_1m(model_lower: &str) -> bool {
    model_lower.contains("fable") || model_lower.contains("mythos")
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
    if m.starts_with("claude-")
        || m == "opus"
        || m == "sonnet"
        || m == "haiku"
        || claude_model_always_1m(&m)
    {
        // Explicit `[1m]` suffix — the canonical Claude Code marker.
        // Doesn't appear in JSONL today, but kept for correctness if that
        // ever changes.
        if m.contains("[1m]") {
            return Some(1_000_000);
        }
        // Always-1M families (Fable, Mythos) have no 200K variant — report 1M
        // unconditionally so a fresh session doesn't briefly read 200K.
        if claude_model_always_1m(&m) {
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

/// Running state behind [`compute_session_stats`], split out so a session's
/// stats can be advanced by only the lines appended since the last scan.
///
/// Transcripts are append-only, so every field here is either a running sum, a
/// running max, or a last-write-wins scalar — all of which fold correctly over
/// batches. The one field that does *not* is `seen_msg_ids`: a finalized
/// assistant message can be re-logged, and dedup is what keeps its tokens and
/// cost from being counted twice.
///
/// **`seen_msg_ids` must stay complete for the whole file — a bounded window is
/// not sound.** Measured across the 120 largest transcripts on disk: every one
/// of them re-logs message ids (42k duplicate instances), and while most repeats
/// sit within 10 lines of each other, 100 of them span more than 100 lines and
/// the widest is 2437. A sliding window would silently miss those and
/// double-count the turn's tokens and USD.
///
/// Ids are stored as 64-bit hashes rather than strings to keep the per-session
/// footprint at ~8 bytes/turn; a collision would drop one turn from the totals,
/// which at a few thousand ids against a 64-bit space is not a real risk.
#[derive(Clone, Debug, Default)]
pub struct StatsAcc {
    total_output: u64,
    total_cost: f64,
    /// (timestamp_secs, output_tokens, turn_cost_usd) — feeds the speed window.
    timed: Vec<(f64, u64, f64)>,
    seen_msg_ids: HashSet<u64>,
    last_model: Option<String>,
    compact_count: u32,
    compact_pre_tokens: u64,
    compact_post_tokens: u64,
    compact_cost_usd: f64,
}

fn hash_msg_id(id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

impl StatsAcc {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold a batch of newly-appended transcript lines into the running state.
    /// Safe to call repeatedly; re-feeding a line that was already folded in is
    /// a no-op for token/cost totals thanks to the msg-id dedup above.
    pub fn push_lines(&mut self, lines: &[&str]) {
        for line in lines {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                self.push_value(&v);
            }
        }
    }

    /// Fold one already-parsed record. Split out from [`push_lines`] so that
    /// [`SessionAcc`] can parse each line once and fan the same `Value` out to
    /// every extractor, instead of re-parsing the file per extractor.
    pub fn push_value(&mut self, v: &Value) {
        use crate::model_cost::{get_model_costs, turn_cost_usd, TurnUsage};

        {
            // `compact_boundary` is a system meta event Claude Code emits each time
            // it summarises the conversation. The summary LLM call itself is not
            // logged as a standalone assistant turn, so its true cost is not in
            // the transcript — we approximate from `compactMetadata`.
            if v.get("type").and_then(|t| t.as_str()) == Some("system")
                && v.get("subtype").and_then(|s| s.as_str()) == Some("compact_boundary")
            {
                self.compact_count += 1;
                let meta = v.get("compactMetadata");
                let pre = meta
                    .and_then(|m| m.get("preTokens"))
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0);
                let post = meta
                    .and_then(|m| m.get("postTokens"))
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0);
                self.compact_pre_tokens += pre;
                self.compact_post_tokens += post;

                // Price the compact call against the most recently seen model.
                // `get_model_costs("")` falls back to the default tier when no
                // assistant turn has been seen yet (defensive — compact almost
                // never precedes the first assistant turn).
                let pricing_model = self.last_model.as_deref().unwrap_or("");
                let costs = get_model_costs(pricing_model);
                self.compact_cost_usd += (pre as f64 / 1_000_000.0) * costs.cache_read
                    + (post as f64 / 1_000_000.0) * costs.output;
                return;
            }

            if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
                return;
            }
            let Some(msg) = v.get("message").and_then(|m| m.as_object()) else {
                return;
            };
            // Only count finalized messages
            if msg.get("stop_reason").map_or(true, |s| s.is_null()) {
                return;
            }
            let msg_id = msg.get("id").and_then(|i| i.as_str()).unwrap_or_default();
            if !msg_id.is_empty() && !self.seen_msg_ids.insert(hash_msg_id(msg_id)) {
                return;
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

            self.total_output += output_tokens;

            // Per-turn cost uses this turn's own model; fall back to most-recently-
            // seen model when a turn omits it (model can change mid-session).
            let turn_model = msg.get("model").and_then(|m| m.as_str());
            if let Some(m) = turn_model {
                self.last_model = Some(m.to_string());
            }
            let cost_model = turn_model.or(self.last_model.as_deref()).unwrap_or("");
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
            self.total_cost += turn_cost;

            // Timestamp for speed
            if let Some(ts_str) = v.get("timestamp").and_then(|t| t.as_str()) {
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                    self.timed
                        .push((dt.timestamp() as f64, output_tokens, turn_cost));
                }
            }
        }
    }

    /// Materialise the stats. `now_secs` is the clock the speed window is
    /// measured against — taken as a parameter so the window is testable
    /// without freezing the system clock.
    pub fn finish_at(&self, now_secs: f64) -> SessionStats {
        // Speed: tokens/s and cost/min over the last 5-minute window.
        //
        // Divide by `now - first_ts`, not `last_ts - first_ts`. The inter-turn
        // gap version makes speed a step function that holds the old rate until
        // the oldest turn slides out of the window — so a session that finished
        // a burst 4 minutes ago still reports the burst's speed, inflating
        // fleet-wide totals while nothing is actually generating. Measuring
        // against "now" lets speed decay smoothly as the idle tail grows.
        let (token_speed, cost_speed_usd_per_min) = if self.timed.len() >= 2 {
            let window_start = now_secs - 300.0;

            let recent: Vec<_> = self
                .timed
                .iter()
                .filter(|(ts, _, _)| *ts > window_start)
                .collect();

            if recent.len() >= 2 {
                let total_recent_tokens: u64 = recent.iter().map(|(_, t, _)| t).sum();
                let total_recent_cost: f64 = recent.iter().map(|(_, _, c)| c).sum();
                let first_ts = recent.first().map(|(ts, _, _)| *ts).unwrap_or(0.0);
                let duration = now_secs - first_ts;
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
            total_output_tokens: self.total_output,
            total_cost_usd: self.total_cost,
            cost_speed_usd_per_min,
            compact_count: self.compact_count,
            compact_pre_tokens: self.compact_pre_tokens,
            compact_post_tokens: self.compact_post_tokens,
            compact_cost_usd: self.compact_cost_usd,
        }
    }

    /// Same as [`finish_at`], against the wall clock.
    pub fn finish(&self) -> SessionStats {
        self.finish_at(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
        )
    }

    /// Drop speed samples that can no longer influence the 5-minute window.
    /// Called as the accumulator is carried across scans so `timed` doesn't
    /// grow without bound on a long-lived session. The margin over 300s is
    /// deliberate slack — pruning exactly at the window edge would race the
    /// next `finish` call's slightly later clock.
    pub fn prune_timed(&mut self, now_secs: f64) {
        let keep_after = now_secs - 900.0;
        self.timed.retain(|(ts, _, _)| *ts > keep_after);
    }
}

/// The batch-free original, kept as the oracle [`SessionAcc`]'s tests compare
/// against — the scan path itself now folds incrementally and no longer calls it.
#[cfg(test)]
fn compute_session_stats(lines: &[&str]) -> SessionStats {
    let mut acc = StatsAcc::new();
    acc.push_lines(lines);
    acc.finish()
}

/// A transcript's fold state carried between scans: how far we have folded, and
/// the accumulator holding the result.
#[derive(Clone, Debug, Default)]
pub struct IncrParse {
    /// Offset of the first byte not yet folded. Always sits just past a newline
    /// (see [`SessionAcc::fold_chunk`]).
    pub offset: u64,
    pub acc: SessionAcc,
}

/// Advance a transcript's fold state by reading only what was appended since
/// last time, re-reading from scratch when the file cannot have been appended to.
///
/// Rewrite detection follows the same rule `search_index` has run in production:
/// a file shorter than our offset was truncated or rewritten, so the carried
/// state is meaningless and we start over. The residual assumption — that a file
/// which is *not* shorter was only appended to — is the same one that module
/// relies on, and holds because Claude Code only ever appends to a transcript
/// (compaction adds a boundary record; it does not rewrite history).
///
/// `prev` is `None` on a cold cache, which simply means "fold the whole file".
pub fn advance_incremental(
    jsonl_path: &Path,
    prev: Option<IncrParse>,
) -> std::io::Result<IncrParse> {
    use std::io::{Read, Seek, SeekFrom};

    let file_len = fs::metadata(jsonl_path)?.len();

    let mut state = match prev {
        // Truncated or rewritten: nothing we carried can be trusted.
        Some(p) if file_len < p.offset => IncrParse::default(),
        Some(p) => p,
        None => IncrParse::default(),
    };

    if file_len == state.offset {
        return Ok(state); // nothing new
    }

    let mut f = fs::File::open(jsonl_path)?;
    if state.offset > 0 {
        f.seek(SeekFrom::Start(state.offset))?;
    }
    let mut buf = Vec::with_capacity((file_len - state.offset) as usize);
    f.read_to_end(&mut buf)?;

    // A transcript is UTF-8, but a chunk boundary can land mid-codepoint only
    // if the writer flushed a partial codepoint — treat any such tail the same
    // way as a partial line: leave it for the next tick.
    let text = match std::str::from_utf8(&buf) {
        Ok(s) => s,
        Err(e) => std::str::from_utf8(&buf[..e.valid_up_to()])
            .expect("valid_up_to marks a valid boundary"),
    };

    let consumed = state.acc.fold_chunk(text);
    state.offset += consumed as u64;
    Ok(state)
}

/// Everything `parse_session_info` used to derive by re-reading the whole
/// transcript, folded in a single forward pass that can be resumed.
///
/// Two wins over the old shape, which read the file with `read_to_string` and
/// then walked `all_lines` once per extractor:
///
/// 1. **One parse, many extractors.** Each line is turned into a `Value` once
///    and fanned out, instead of being re-parsed by `compute_session_stats`,
///    `extract_last_context_usage`, the ai-title scan, and the todo scan.
/// 2. **Resumable.** Every field folds over batches, so a session that appended
///    a few lines can be advanced with just those lines rather than re-read from
///    byte zero on every 2s scan tick.
///
/// Each field's fold rule is chosen to match the batch-free original exactly —
/// see the comments on the fields that are not simple last-write-wins.
#[derive(Clone, Debug, Default)]
pub struct SessionAcc {
    stats: StatsAcc,

    /// Latest live (non-sidechain) assistant usage: `(total_input, model)`.
    ctx_last: Option<(u64, String)>,
    /// Largest single-turn total input seen *since the last compact summary*.
    ctx_session_max: u64,

    /// First `ai-title` record wins; `None` simply means "not seen yet", so a
    /// title written later is still picked up.
    ai_title: Option<String>,

    /// The *first* `user` record decides the entrypoint — including when that
    /// record carries no `entrypoint` field at all, which settles it to `None`.
    /// Hence the explicit flag: without it, a later `user` record's entrypoint
    /// would wrongly fill in a value the original would have left empty.
    entrypoint: Option<String>,
    entrypoint_settled: bool,

    /// Latest todo block wins.
    todos: Option<crate::session_todos::TodoSummary>,
}

impl SessionAcc {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold a batch of newly-appended lines. Parses each line once.
    pub fn push_lines(&mut self, lines: &[&str]) {
        for line in lines {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            self.stats.push_value(&v);
            self.push_context(&v);

            if self.ai_title.is_none()
                && v.get("type").and_then(|t| t.as_str()) == Some("ai-title")
            {
                self.ai_title = v
                    .get("aiTitle")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string());
            }

            if !self.entrypoint_settled && v.get("type").and_then(|t| t.as_str()) == Some("user")
            {
                self.entrypoint_settled = true;
                self.entrypoint = v
                    .get("entrypoint")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
            }

            if let Some(summary) = crate::session_todos::todo_summary_from_value(&v) {
                self.todos = Some(summary);
            }
        }
    }

    /// Context-window usage, folded as a little state machine.
    ///
    /// The batch-free original first located the *last* compact-summary line and
    /// only scanned after it, because pre-compact `input_tokens` are stale
    /// (Claude Code strips them at load time). Folding forward, that same rule is
    /// just "a compact summary resets what we know" — after the final reset the
    /// surviving state is exactly the post-cutoff scan the original performed.
    fn push_context(&mut self, v: &Value) {
        if v.get("type").and_then(|t| t.as_str()) == Some("user")
            && v.get("isCompactSummary")
                .and_then(|b| b.as_bool())
                .unwrap_or(false)
        {
            self.ctx_last = None;
            self.ctx_session_max = 0;
            return;
        }

        // Subagent turns have their own context window; they must not pollute
        // the parent's number.
        if v.get("isSidechain")
            .and_then(|b| b.as_bool())
            .unwrap_or(false)
        {
            return;
        }
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            return;
        }
        let Some(msg) = v.get("message").and_then(|m| m.as_object()) else {
            return;
        };

        // Deliberately no `stop_reason` filter: Claude Code counts in-progress
        // turns toward context, so the percentage updates while streaming.
        let usage = msg.get("usage");
        let get = |k: &str| {
            usage
                .and_then(|u| u.get(k))
                .and_then(|t| t.as_u64())
                .unwrap_or(0)
        };
        let total_input =
            get("input_tokens") + get("cache_creation_input_tokens") + get("cache_read_input_tokens");
        if total_input == 0 {
            return;
        }
        if total_input > self.ctx_session_max {
            self.ctx_session_max = total_input;
        }
        let model = msg
            .get("model")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        self.ctx_last = Some((total_input, model));
    }

    pub fn stats_at(&self, now_secs: f64) -> SessionStats {
        self.stats.finish_at(now_secs)
    }

    pub fn stats(&self) -> SessionStats {
        self.stats.finish()
    }

    /// `(input_tokens_used, model, session_max_input_tokens)`, matching
    /// [`extract_last_context_usage`].
    pub fn context_usage(&self) -> Option<(u64, String, u64)> {
        self.ctx_last
            .clone()
            .map(|(used, model)| (used, model, self.ctx_session_max))
    }

    pub fn ai_title(&self) -> Option<String> {
        self.ai_title.clone()
    }

    pub fn entrypoint(&self) -> Option<String> {
        self.entrypoint.clone()
    }

    pub fn todos(&self) -> Option<crate::session_todos::TodoSummary> {
        self.todos.clone()
    }

    /// Bound the speed-window samples carried between scans.
    pub fn prune(&mut self, now_secs: f64) {
        self.stats.prune_timed(now_secs);
    }

    /// Fold the complete lines in `chunk` and report how many bytes were
    /// consumed.
    ///
    /// **A trailing fragment with no newline is left unconsumed.** Scans race
    /// the CLI's writes, so the tail of a growing transcript is routinely a
    /// half-written line. Folding it would parse-fail (silently dropping that
    /// turn's tokens and cost forever, because the offset would have moved past
    /// it); leaving it unconsumed means the next tick re-reads it once it is
    /// complete. The returned count is therefore the offset advance, not
    /// `chunk.len()`.
    pub fn fold_chunk(&mut self, chunk: &str) -> usize {
        let consumed = match chunk.rfind('\n') {
            Some(i) => i + 1,
            None => return 0, // nothing complete yet
        };
        let lines: Vec<&str> = chunk[..consumed].lines().collect();
        self.push_lines(&lines);
        consumed
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

/// Locate a session's transcript by id. Session jsonl lives at
/// `~/.claude/projects/<encoded-cwd>/<session_id>.jsonl`; the encoded dir is
/// derived from the workspace path, which the caller usually doesn't have, so
/// scan the project dirs rather than reconstructing the encoding.
pub fn find_session_jsonl(session_id: &str) -> Option<PathBuf> {
    let projects = get_claude_dir()?.join("projects");
    let name = format!("{session_id}.jsonl");
    fs::read_dir(projects)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path().join(&name))
        .find(|p| p.is_file())
}

/// Launch identity of a session, read from its transcript: the FIRST `user`
/// record's `entrypoint` field (the Claude CLI persists `CLAUDE_CODE_ENTRYPOINT`
/// there at spawn time). First record only — a later `--resume` run stamps its
/// own entrypoint on the records it appends, and that must not reclassify who
/// originally launched the session.
///
/// Feed the result to [`crate::session_launch::is_fleet_owned_entrypoint`] to
/// answer "did Fleet spawn this?" from a process that only knows a session id
/// (the `fleet mcp` child, the hook CLIs) and has no session scan to consult.
pub fn session_entrypoint(session_id: &str) -> Option<String> {
    entrypoint_from_jsonl(&find_session_jsonl(session_id)?)
}

fn entrypoint_from_jsonl(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    raw.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("user"))
        .and_then(|v| {
            v.get("entrypoint")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        })
}

/// The directory Claude Code itself was launched in for this session, read from
/// its transcript.
///
/// This is NOT the agent's shell cwd: the Bash tool's cwd persists across calls,
/// so an agent following the Rule-3 worktree workflow spends most of a session
/// `cd`-ed into `<repo>/.worktrees/<task>`. Anything that must reproduce *the
/// session's* workspace (spawning a successor, resolving its project dir) has to
/// read it from the transcript rather than trust `current_dir()`.
pub fn resolve_session_cwd(session_id: &str) -> Option<String> {
    session_cwd_from_jsonl(&find_session_jsonl(session_id)?)
}

fn session_cwd_from_jsonl(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    raw.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find_map(|v| {
            v.get("cwd")
                .and_then(|c| c.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
        })
}

/// The `model` field of `~/.claude/settings.json`, e.g. `opus[1m]`. This is the
/// CLI's default when a session is launched without `--model`.
fn configured_model_spec() -> Option<String> {
    let raw = fs::read_to_string(get_claude_dir()?.join("settings.json")).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let m = v.get("model")?.as_str()?.trim();
    (!m.is_empty()).then(|| m.to_string())
}

/// Split a model spec into its base id and its bracketed suffix:
/// `opus[1m]` → `("opus", Some("[1m]"))`, `claude-fable-5` → `(.., None)`.
fn split_model_suffix(spec: &str) -> (&str, Option<&str>) {
    match spec.find('[') {
        Some(i) if spec.ends_with(']') => (&spec[..i], Some(&spec[i..])),
        _ => (spec, None),
    }
}

/// Rebuild a `--model` spec for a session, given the model id its transcript
/// recorded and the CLI's configured default.
///
/// Transcripts record the *resolved* id (`claude-opus-4-8`) and drop any
/// bracketed opt-in suffix, so a session running the 1M-context `opus[1m]`
/// looks identical on disk to one running the 200K `opus`. Relaunching from the
/// bare id would silently halve the context window. When the configured default
/// carries a suffix and names the same model family the transcript shows, the
/// session was running that default — re-apply the suffix to the precise id
/// from the transcript (keeping its exact version) rather than to the alias.
/// A family mismatch means the session overrode the default, so its own id wins
/// verbatim.
fn reconcile_model_spec(transcript_model: &str, configured: Option<&str>) -> String {
    let Some(configured) = configured.map(str::trim).filter(|c| !c.is_empty()) else {
        return transcript_model.to_string();
    };
    let (base, suffix) = split_model_suffix(configured);
    let Some(suffix) = suffix else {
        return transcript_model.to_string();
    };
    let same_family = !base.is_empty()
        && transcript_model
            .to_lowercase()
            .contains(&base.to_lowercase());
    if same_family {
        format!("{transcript_model}{suffix}")
    } else {
        transcript_model.to_string()
    }
}

/// The `--model` spec a session is running, suitable for relaunching a
/// successor on the same model. `None` when the transcript is missing or holds
/// no assistant turn to read a model from.
///
/// Two sources, in order of authority:
///
/// 1. **What Fleet launched it with** ([`crate::launch_spec`]), when Fleet
///    launched it. This is a record, not a reconstruction — the `--model` string
///    verbatim, bracketed suffix and all.
/// 2. **Reconstruction from the transcript**, for everyone else: the resolved id
///    it recorded, with any suffix re-applied from the CLI's configured default
///    (see [`reconcile_model_spec`]). Lossy by construction — a session launched
///    with an explicit `--model opus[1m]` against a differently-defaulted
///    `settings.json` leaves no trace of the suffix anywhere.
pub fn resolve_session_model_spec(session_id: &str) -> Option<String> {
    if let Some(recorded) = crate::launch_spec::model_of(session_id) {
        return Some(recorded);
    }
    let path = find_session_jsonl(session_id)?;
    let raw = fs::read_to_string(path).ok()?;
    let lines: Vec<Value> = raw
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let model = extract_model(&lines)?;
    Some(reconcile_model_spec(&model, configured_model_spec().as_deref()))
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

/// Launch identity: the FIRST user record's `entrypoint` field (persisted by
/// the Claude CLI from `CLAUDE_CODE_ENTRYPOINT` at spawn time). First record
/// only, so later `--resume` runs — which stamp their own entrypoint on the
/// records they append — don't reclassify the session.
/// Kept as the oracle `SessionAcc`'s entrypoint tests compare against; the scan
/// path folds it forward instead of re-scanning the file.
#[cfg(test)]
fn extract_entrypoint(all_lines: &[&str]) -> Option<String> {
    all_lines
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("user"))
        .and_then(|v| v.get("entrypoint").and_then(|s| s.as_str()).map(|s| s.to_string()))
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
    // `incr`: fold state carried from the previous scan of this transcript.
    // `None` folds the file from scratch.
    incr: Option<IncrParse>,
) -> Option<(SessionInfo, IncrParse)> {
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

    // Advance the fold by only what the transcript appended since the last scan
    // (a cold `incr` folds the whole file). Nothing below re-reads the file in
    // full: the cumulative fields come off the accumulator, and the status
    // heuristics only ever needed the tail.
    let state = advance_incremental(jsonl_path, incr).ok()?;
    let acc = &state.acc;

    // Last 100 lines for status — seeks from the end rather than materialising
    // the whole transcript.
    let last_n: Vec<Value> =
        crate::jsonl_tail::read_tail_lines_as_json(jsonl_path, 100).unwrap_or_default();

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
    // Raw (age-unaware) signal for stuck detection; the age floor + proc_alive
    // gate is applied later in `apply_pid_liveness`.
    let pending_tool_batch = has_pending_noninteractive_tool_batch(&last_n);
    let stats = acc.stats();
    let context_percent = acc
        .context_usage()
        .and_then(|(used, model, max)| compute_context_percent(used, Some(&model), max));
    let last_message_preview = extract_last_text(&last_n);

    let slug = last_n
        .iter()
        .filter_map(|v| v.get("slug").and_then(|s| s.as_str()).map(|s| s.to_string()))
        .last();

    let ai_title = acc.ai_title();
    let entrypoint = acc.entrypoint();

    let model = meta_model.or_else(|| extract_model(&last_n));
    let last_skill = extract_last_skill(&last_n);
    let todos = acc.todos();
    let task_plan =
        crate::prd_tasks::summarize_workspace_tasks(Path::new(&workspace_path), Some(session_id.as_str()));

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
        entrypoint,
        is_subagent,
        parent_session_id,
        agent_type,
        agent_description,
        slug,
        ai_title,
        status,
        token_speed: stats.token_speed,
        agent_token_speed: stats.token_speed,
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
        // Stamped by `apply_pid_liveness` right after this returns — the parse
        // itself has no view of the process table.
        proc_alive: false,
        pending_tool_batch,
        context_percent,
        last_skill,
        agent_source: "claude-code".to_string(),
        last_outcome: None,
        rate_limit,
        todos,
        task_plan,
        background_tasks: Vec::new(),
        handoff: None,
        user_mark: None,
        last_read_ms: None,
        compact_count: stats.compact_count,
        compact_pre_tokens: stats.compact_pre_tokens,
        compact_post_tokens: stats.compact_post_tokens,
        compact_cost_usd: stats.compact_cost_usd,
    })
    .map(|info| (info, state))
}

// ── Scan cache ───────────────────────────────────────────────────────────────

/// Caches expensive operations across rescans: process-table lookups and
/// already-parsed session files whose mtime hasn't changed.
pub struct ScanCache {
    /// Cached `scan_cli_processes()` result + timestamp.
    /// `None` timestamp = never scanned, force refresh on first read.
    pub process_cache: Mutex<(Option<Instant>, Vec<CliProcess>)>,
    /// JSONL path → (mtime_ms, SessionInfo).
    pub session_cache: Mutex<HashMap<String, (u64, SessionInfo)>>,
    /// JSONL path → how far that transcript has been folded, plus the running
    /// accumulator. Lets a session that appended a few lines be advanced with
    /// just those lines instead of re-read from byte zero.
    ///
    /// Memory-only, deliberately: the disk cache (`scan_cache_disk`) stores the
    /// finished `SessionInfo`, not this. Persisting the accumulator would mean
    /// persisting its full `seen_msg_ids` set per session, which is exactly the
    /// unbounded thing the disk cache should not grow. A cold start therefore
    /// folds each transcript once, as it always did.
    pub incr_cache: Mutex<HashMap<String, IncrParse>>,
    /// Last time `session_cache` was flushed to disk via `scan_cache_disk::save`.
    /// `None` means never persisted in this process.
    pub last_persisted_at: Mutex<Option<Instant>>,
}

/// Minimum gap between `scan_cache_disk::save` flushes. The cache is overwritten
/// completely on each save, so a tighter interval would burn IO without buying
/// us anything — the disk copy is only consulted at process startup anyway.
pub const PERSIST_INTERVAL: Duration = Duration::from_secs(30);

/// Pure throttle predicate. Returns `true` when the cache should be persisted
/// now given the time of the previous save (`None` on the very first call).
pub fn should_persist_now(last: Option<Instant>, now: Instant, interval: Duration) -> bool {
    match last {
        None => true,
        Some(t) => now.duration_since(t) >= interval,
    }
}

impl ScanCache {
    pub fn new() -> Self {
        // `None` timestamp instead of `Instant::now() - 999s`: that
        // subtraction panics on Windows runners with low uptime
        // ("overflow when subtracting duration from instant").
        Self {
            process_cache: Mutex::new((None, Vec::new())),
            session_cache: Mutex::new(crate::scan_cache_disk::load()),
            incr_cache: Mutex::new(HashMap::new()),
            last_persisted_at: Mutex::new(None),
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
        info.agent_token_speed = 0.0;
        info.cost_speed_usd_per_min = 0.0;
    }

    // A rate-limited session is blocked by the API and generating nothing, but
    // its 5-minute speed window still holds the burst of output tokens emitted
    // just before the limit hit — so `token_speed` stays frozen at a high value
    // and keeps inflating fleet-wide totals (observed: ~21 RateLimited agents
    // contributing thousands of ghost tok/s while only a handful were actually
    // streaming). Unlike WaitingInput/Delegating there is no age at which a
    // rate-limited session resumes generating, so zero it immediately. The
    // RateLimited *status* is preserved so the UI can still show the limit card.
    if matches!(info.status, SessionStatus::RateLimited) {
        info.token_speed = 0.0;
        info.agent_token_speed = 0.0;
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
        info.agent_token_speed = 0.0;
        info.cost_speed_usd_per_min = 0.0;
    }
}

/// Hard pid-based liveness override for Fleet-spawned headless sessions.
///
/// Launchpad spawns always carry the session id in argv (`--session-id` on the
/// first turn, `--resume` on follow-ups), so for sessions whose entrypoint is
/// [`crate::session_launch::NEW_SESSION_ENTRYPOINT`] the presence/absence of an
/// exact argv match is definitive — unlike the mtime-age heuristics that govern
/// every other session:
///
/// - **Process alive but transcript quiet** (blocked on an AskUserQuestion /
///   permission decision card, or a long-running tool): the age heuristics
///   decay the status to Idle even though the agent is mid-turn. Re-promote —
///   the live process pinned to exactly this session id is stronger evidence
///   than file age. (The general "never promote on age alone" invariant is
///   about mtime guessing; this branch has a live pid as proof.)
/// - **Process dead**: any lingering in-flight status (e.g. a stale
///   ToolExecuting hook pinning Executing after a `kill -9`) is a ghost.
///   Downgrade to Idle immediately instead of waiting out the age windows.
///
/// Subagents never have their own argv-matched process; skip them.
pub fn apply_pid_liveness(
    info: &mut SessionInfo,
    exact_proc_alive: bool,
    hook_state: Option<&HookState>,
    age_secs: f64,
) {
    // Stamped for every session (subagents included, where it is always false):
    // the UI needs the raw liveness bit, not just the status it feeds into.
    info.proc_alive = exact_proc_alive;

    if info.is_subagent
        || info.entrypoint.as_deref() != Some(crate::session_launch::NEW_SESSION_ENTRYPOINT)
    {
        return;
    }
    if exact_proc_alive {
        // Deadlock guard: a live Fleet-spawned process whose transcript froze
        // mid tool-batch is wedged inside the turn — the model can't resume
        // until every tool_use_id has a tool_result, and one never came. Left
        // alone, the ToolExecuting-hook / Idle-promote logic below would keep
        // painting it as busy forever (the exact masking that hid a 1.5h hang).
        // Override to Stuck so the UI can surface a one-click interrupt.
        if info.pending_tool_batch && age_secs >= STUCK_TOOL_BATCH_FLOOR_SECS {
            info.status = SessionStatus::Stuck;
            info.token_speed = 0.0;
            info.agent_token_speed = 0.0;
            info.cost_speed_usd_per_min = 0.0;
            return;
        }
        if info.status == SessionStatus::Idle {
            info.status = match hook_state {
                Some(HookState::ToolExecuting) => SessionStatus::Executing,
                Some(HookState::ModelProcessing) => SessionStatus::Thinking,
                _ => SessionStatus::WaitingInput,
            };
        }
    } else if matches!(
        info.status,
        SessionStatus::Thinking
            | SessionStatus::Executing
            | SessionStatus::Streaming
            | SessionStatus::Processing
            | SessionStatus::Active
    ) {
        info.status = SessionStatus::Idle;
        info.token_speed = 0.0;
        info.agent_token_speed = 0.0;
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

/// How long a session whose workspace directory has been removed stays visible
/// after its last activity. Long enough to cover a session that outlives its own
/// worktree (Rule 3 has the agent delete it when the plan merges, while the
/// session keeps running and may still be holding an unanswered decision card);
/// short enough that transcripts of long-dead worktrees don't clutter the list
/// with cards whose Resume can never work.
const MISSING_WS_KEEP_MS: u64 = 24 * 60 * 60 * 1000;

/// Was `path` (a transcript file, or a subagent dir) touched inside the keep
/// window? Unreadable metadata counts as stale — the old behaviour was to hide
/// these outright.
fn touched_within_keep_window(path: &Path) -> bool {
    let Ok(modified) = fs::metadata(path).and_then(|m| m.modified()) else {
        return false;
    };
    let modified_ms = modified
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    now_ms.saturating_sub(modified_ms) <= MISSING_WS_KEEP_MS
}

pub fn scan_claude_sessions(claude_dir: &Path, scan_cache: &ScanCache) -> Vec<SessionInfo> {
    let mut sessions = Vec::new();
    let ide_sessions = scan_ide_sessions(claude_dir);

    // Reuse cached process list if fresh (< 10 s).
    let cli_processes = {
        let mut guard = scan_cache.process_cache.lock().unwrap();
        let stale = guard.0.map_or(true, |t| t.elapsed() > Duration::from_secs(10));
        if stale {
            guard.1 = scan_cli_processes();
            guard.0 = Some(Instant::now());
        }
        guard.1.clone()
    };

    // One pass over hooks.jsonl yields both the state map and the outstanding
    // background tasks — the file is huge, so the scan must not read it twice.
    let hook_snapshot = crate::hooks::read_hook_snapshot();
    let hook_states = &hook_snapshot.states;
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

        // Sessions whose workspace directory no longer exists — e.g. a git
        // worktree that was removed. Their transcripts linger under
        // ~/.claude/projects/<encoded>/ and would otherwise show as permanently
        // RateLimited cards with a Resume button that can never work (`claude
        // --resume` bails the moment it sees the cwd is gone). Skip TCC-protected
        // paths: we can't stat them without triggering a macOS permission dialog,
        // so we never hide those (decode already left them naively decoded).
        //
        // Hiding them *unconditionally* was too blunt: the Rule-3 worktree
        // workflow has an agent remove its own worktree when the plan merges, so
        // a session that is still alive — still holding an unanswered decision
        // card — can lose its cwd mid-flight. Such a session would vanish from
        // the scan, and with it the workspace label and session-history panel of
        // its pending card. So keep the recently active ones (see
        // `MISSING_WS_KEEP_MS`) and hide only the stale zombies this filter was
        // written for.
        let ws_path = Path::new(&workspace_path);
        let workspace_missing = !crate::tcc::is_tcc_protected(ws_path) && !ws_path.is_dir();

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

            // Workspace gone: keep only transcripts still being written to.
            if workspace_missing && !touched_within_keep_window(&path) {
                continue;
            }

            // Main session JSONL files
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                let session_id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();

                let (session_pid, pid_precise) = resolve_pid(&procs_in_cwd, &session_id);
                // Definitive liveness signal for Fleet-spawned sessions: a
                // live process whose argv names exactly this session id.
                let exact_proc_alive = procs_in_cwd
                    .iter()
                    .any(|p| p.resume_session_id.as_deref() == Some(session_id.as_str()));

                // Try session cache first (skip re-reading unchanged files).
                if let Some((mut info, age)) = check_session_cache(&path, &session_cache_snapshot) {
                    age_out_status(&mut info, age);
                    apply_pid_liveness(&mut info, exact_proc_alive, hook_states.get(&session_id), age);
                    info.background_tasks = hook_snapshot
                        .background_tasks
                        .get(&session_id)
                        .cloned()
                        .unwrap_or_default();
                    info.pid = session_pid;
                    info.pid_precise = pid_precise;
                    info.ide_name = ide_name.clone();
                    sessions.push(info);
                } else {
                    let key = path.to_string_lossy().to_string();
                    // Take the fold state in its own statement so the guard is
                    // dropped here. Inlining this into the `if let` scrutinee
                    // would keep the guard alive across the whole body — and the
                    // body locks `incr_cache` again, which self-deadlocks.
                    let prev_incr = scan_cache.incr_cache.lock().unwrap().get(&key).cloned();

                    if let Some((mut info, incr)) = parse_session_info(
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
                        prev_incr,
                    ) {
                        // Cache-miss = the transcript just changed, so it is not
                        // frozen; pass age 0 so a freshly-written turn is never
                        // misread as stuck. Stuck only fires on the cache-hit path
                        // above, where `age` reflects a genuinely quiet file.
                        apply_pid_liveness(&mut info, exact_proc_alive, hook_states.get(&session_id), 0.0);
                        info.background_tasks = hook_snapshot
                            .background_tasks
                            .get(&session_id)
                            .cloned()
                            .unwrap_or_default();
                        scan_cache
                            .session_cache
                            .lock()
                            .unwrap()
                            .insert(key.clone(), (info.last_activity_ms, info.clone()));
                        scan_cache.incr_cache.lock().unwrap().insert(key, incr);
                        sessions.push(info);
                    }
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

                // Collect agent transcripts: direct subagents
                // (`subagents/agent-*.jsonl`) plus Claude Code Workflow fan-out
                // agents (`subagents/workflows/wf_*/agent-*.jsonl`), so workflow
                // nodes can link to an openable session.
                let mut agent_paths: Vec<PathBuf> = Vec::new();
                for agent_entry in agent_entries.flatten() {
                    let p = agent_entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                        agent_paths.push(p);
                    }
                }
                if let Ok(wf_runs) = fs::read_dir(subagents_dir.join("workflows")) {
                    for run in wf_runs.flatten() {
                        let run_dir = run.path();
                        if !run_dir.is_dir() {
                            continue;
                        }
                        if let Ok(wf_agents) = fs::read_dir(&run_dir) {
                            for a in wf_agents.flatten() {
                                let p = a.path();
                                if p.extension().and_then(|e| e.to_str()) == Some("jsonl")
                                    && p.file_name()
                                        .and_then(|n| n.to_str())
                                        .map(|n| n.starts_with("agent-"))
                                        .unwrap_or(false)
                                {
                                    agent_paths.push(p);
                                }
                            }
                        }
                    }
                }

                for agent_path in agent_paths {

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
                    } else {
                        let key = agent_path.to_string_lossy().to_string();
                        // Guard dropped before the call — see the note at the
                        // main-session call site.
                        let prev_incr = scan_cache.incr_cache.lock().unwrap().get(&key).cloned();

                        if let Some((info, incr)) = parse_session_info(
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
                            prev_incr,
                        ) {
                            scan_cache
                                .session_cache
                                .lock()
                                .unwrap()
                                .insert(key.clone(), (info.last_activity_ms, info.clone()));
                            scan_cache.incr_cache.lock().unwrap().insert(key, incr);
                            sessions.push(info);
                        }
                    }
                }
            }
        }
    }

    strip_ide_name_from_fleet_spawns(&mut sessions);

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
    // Token speed rolls up identically: a parent's `agent_token_speed` starts at
    // its own speed (set in parse, cached pre-aggregation so no double-count on
    // cache hits) and gains every subagent's speed — including the workflow
    // fan-out agents hidden from the session list.
    let mut subagent_cost_by_parent: HashMap<String, f64> = HashMap::new();
    let mut subagent_speed_by_parent: HashMap<String, f64> = HashMap::new();
    for s in &sessions {
        if s.is_subagent {
            if let Some(pid) = &s.parent_session_id {
                *subagent_cost_by_parent.entry(pid.clone()).or_insert(0.0) += s.total_cost_usd;
                *subagent_speed_by_parent.entry(pid.clone()).or_insert(0.0) += s.token_speed;
            }
        }
    }
    for session in &mut sessions {
        if !session.is_subagent {
            if let Some(extra) = subagent_cost_by_parent.get(&session.id) {
                session.agent_total_cost_usd += *extra;
            }
            if let Some(extra) = subagent_speed_by_parent.get(&session.id) {
                session.agent_token_speed += *extra;
            }
        }
    }

    // Prune stale entries from session cache.
    {
        let live_paths: HashSet<String> = sessions.iter().map(|s| s.jsonl_path.clone()).collect();
        scan_cache.session_cache.lock().unwrap().retain(|k, _| live_paths.contains(k));

        // The fold state must be pruned on the same beat, or a session that
        // ages out of the 7-day window leaves its accumulator (including its
        // whole seen_msg_ids set) resident forever. Also bound the speed
        // samples the surviving accumulators carry.
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let mut incr = scan_cache.incr_cache.lock().unwrap();
        incr.retain(|k, _| live_paths.contains(k));
        for state in incr.values_mut() {
            state.acc.prune(now_secs);
        }
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
                | SessionStatus::RateLimited
        );
        let b_active = matches!(
            b.status,
            SessionStatus::Thinking
                | SessionStatus::Executing
                | SessionStatus::Streaming
                | SessionStatus::Delegating
                | SessionStatus::Processing
                | SessionStatus::WaitingInput
                | SessionStatus::RateLimited
        );
        b_active
            .cmp(&a_active)
            .then(a.created_at_ms.cmp(&b.created_at_ms))
    });

    // Persist the cleaned cache to disk, throttled. Subsequent process starts
    // can then seed `session_cache` and bypass the per-file re-parse on the
    // mtime-equality path inside `check_session_cache`.
    {
        let mut last_guard = scan_cache.last_persisted_at.lock().unwrap();
        let now = Instant::now();
        if should_persist_now(*last_guard, now, PERSIST_INTERVAL) {
            *last_guard = Some(now);
            drop(last_guard);
            let snapshot = scan_cache.session_cache.lock().unwrap().clone();
            std::thread::spawn(move || {
                if let Err(e) = crate::scan_cache_disk::save(&snapshot) {
                    eprintln!("[session-cache] save failed: {e}");
                }
            });
        }
    }

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
                | SessionStatus::RateLimited
        );
        let b_active = matches!(
            b.status,
            SessionStatus::Thinking
                | SessionStatus::Executing
                | SessionStatus::Streaming
                | SessionStatus::Delegating
                | SessionStatus::Processing
                | SessionStatus::WaitingInput
                | SessionStatus::RateLimited
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

/// Minimal `SessionInfo` for tests. Lives here (rather than being re-spelled as
/// a 40-field literal in each module's test mod) so the enricher suites can
/// build one cheaply.
#[cfg(test)]
pub(crate) fn test_session(id: &str) -> SessionInfo {
    SessionInfo {
        id: id.into(),
        workspace_path: "/ws".into(),
        workspace_name: "ws".into(),
        ide_name: None,
        entrypoint: None,
        is_subagent: false,
        parent_session_id: None,
        agent_type: None,
        agent_description: None,
        slug: None,
        ai_title: None,
        status: SessionStatus::Idle,
        token_speed: 0.0,
        agent_token_speed: 0.0,
        total_output_tokens: 0,
        total_cost_usd: 0.0,
        agent_total_cost_usd: 0.0,
        cost_speed_usd_per_min: 0.0,
        last_message_preview: None,
        last_activity_ms: 0,
        created_at_ms: 0,
        jsonl_path: format!("/tmp/{id}.jsonl"),
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
        task_plan: None,
        handoff: None,
        user_mark: None,
        last_read_ms: None,
        compact_count: 0,
        compact_pre_tokens: 0,
        compact_post_tokens: 0,
        compact_cost_usd: 0.0,
    }
}

/// Stamp the scan-time state that lives outside the jsonl — relay position,
/// manual mark, read state. Every path that hands sessions to the frontend must
/// go through this, including the incremental rescan: a source's sessions come
/// back off disk with these fields unset, so skipping an enricher silently
/// clears whatever the human had set.
pub fn enrich_all(sessions: &mut [SessionInfo]) {
    crate::handoff::enrich_sessions(sessions);
    crate::session_mark::enrich_sessions(sessions);
    crate::session_read::enrich_sessions(sessions);
}

/// Scan all registered agent sources and merge into a single sorted list.
pub fn scan_all_sources(sources: &[Box<dyn crate::agent_source::AgentSource>]) -> Vec<SessionInfo> {
    let mut sessions = Vec::new();
    for source in sources {
        if source.is_available() {
            sessions.extend(source.scan_sessions());
        }
    }
    enrich_all(&mut sessions);
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

/// Grace period before a SIGINT that nobody handled escalates to a tree kill.
const INTERRUPT_ESCALATION: Duration = Duration::from_millis(5000);

/// Gracefully interrupt the agent at `pid`: deliver SIGINT to the **root pid
/// only** and let the CLI decide how to unwind. Signalling the whole tree (as
/// [`kill_pid_impl`] does) would kill the tool child behind the CLI's back and
/// lose the transcript marker.
///
/// What SIGINT actually does depends on how the CLI was started — both verified
/// against `claude` 2.1.204 with a blocking foreground Bash call in flight:
///
/// * **headless `-p`** (what the launchpad spawns): aborts the tool call, kills
///   its own tool child, appends `[Request interrupted by user for tool use]`
///   and exits 0. `claude --resume <session-id>` then picks the conversation
///   back up. This is the case worth calling "interrupt".
/// * **interactive, attached to a pty** (what the user runs in a terminal): the
///   TUI reads Ctrl-C as a keystroke in raw mode, so a real SIGINT means "quit".
///   It exits 0 and **abandons its tool child**, reparented to init.
/// * **still booting**, before a handler is installed: killed outright.
///
/// Hence: sweep whatever the captured tree left behind once the root is gone,
/// and escalate to [`kill_pid_impl`] if the root ignored the signal entirely.
pub fn interrupt_pid_impl(pid: u32) -> Result<(), String> {
    interrupt_pid_with_grace(pid, INTERRUPT_ESCALATION)
}

/// [`interrupt_pid_impl`] with an explicit escalation delay. Split out so tests
/// don't have to wait five seconds to observe the fallback.
pub fn interrupt_pid_with_grace(pid: u32, grace: Duration) -> Result<(), String> {
    #[cfg(unix)]
    {
        // Capture the tree BEFORE signalling: once the root exits, its children
        // are reparented to init and walking down from `pid` finds nothing.
        let tree = collect_process_tree(pid);
        crate::log_debug(&format!(
            "interrupt_pid: SIGINT to root {pid} (captured tree of {})",
            tree.len()
        ));
        if unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) } != 0 {
            return Err(format!("no such process: {pid}"));
        }

        std::thread::spawn(move || {
            std::thread::sleep(grace);

            if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
                crate::log_debug(&format!(
                    "interrupt_pid: {pid} still alive {grace:?} after SIGINT; escalating to tree kill"
                ));
                let _ = kill_pid_impl(pid);
                return;
            }

            // The root is gone. A headless CLI reaped its own children; an
            // interactive one abandoned them. Sweep the survivors.
            //
            // Every pid here was alive moments ago, so reuse inside this window
            // is unlikely — the same bet kill_pid_tree's delayed SIGKILL makes.
            let orphans: Vec<u32> = tree
                .iter()
                .copied()
                .filter(|&p| p != pid && unsafe { libc::kill(p as libc::pid_t, 0) } == 0)
                .collect();
            if orphans.is_empty() {
                return;
            }
            crate::log_debug(&format!(
                "interrupt_pid: root {pid} exited but orphaned {orphans:?}; sweeping"
            ));
            for &p in orphans.iter().rev() {
                unsafe { libc::kill(p as libc::pid_t, libc::SIGTERM) };
            }
            std::thread::sleep(Duration::from_millis(2000));
            for &p in orphans.iter().rev() {
                if unsafe { libc::kill(p as libc::pid_t, 0) } == 0 {
                    unsafe { libc::kill(p as libc::pid_t, libc::SIGKILL) };
                }
            }
        });

        Ok(())
    }

    // Windows has no way to deliver SIGINT to an unrelated process (the console
    // control events only reach the sender's own console group), so there is no
    // graceful tier — fall through to the hard kill.
    #[cfg(not(unix))]
    {
        let _ = grace;
        kill_pid_impl(pid)
    }
}

/// Kill a process by PID (with process tree cleanup).
pub fn kill_pid_impl(pid: u32) -> Result<(), String> {
    kill_pid_tree(pid, false)
}

/// Kill `pid` **and every descendant**. Signalling the root alone leaves the
/// agent's tool children — a build, a test run, a dev server — reparented to
/// init and still burning CPU after the agent itself is gone.
///
/// `force` sends SIGKILL straight away; otherwise SIGTERM now, and SIGKILL to
/// whatever is still standing 2s later.
pub fn kill_pid_tree(pid: u32, force: bool) -> Result<(), String> {
    #[cfg(unix)]
    {
        let pids = collect_process_tree(pid);
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        crate::log_debug(&format!(
            "kill_pid: {} to {} pids (root={}): {:?}",
            if force { "SIGKILL" } else { "SIGTERM" },
            pids.len(),
            pid,
            pids
        ));
        for &p in pids.iter().rev() {
            unsafe { libc::kill(p as libc::pid_t, signal) };
        }

        if !force {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(2000));
                for &p in pids.iter().rev() {
                    if unsafe { libc::kill(p as libc::pid_t, 0) } == 0 {
                        unsafe { libc::kill(p as libc::pid_t, libc::SIGKILL) };
                    }
                }
            });
        }

        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = force;
        crate::process_util::command("taskkill")
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
        crate::process_util::command("taskkill")
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

#[cfg(all(test, unix))]
mod interrupt_tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};

    /// `sleep` dies on SIGINT's default disposition — the graceful tier, no
    /// escalation. Spawned directly rather than via `sh -c 'sleep 30'`: dash
    /// (Linux `/bin/sh`) does NOT exec a single-command `-c` string, it forks
    /// `sleep` as a child, so a SIGINT aimed at the shell's own pid neither
    /// kills the shell nor reaches sleep — the interrupt would wrongly escalate
    /// to SIGTERM. (bash-as-/bin/sh on macOS execs, which is why it passed
    /// locally but failed on CI.) Spawning `sleep` directly makes the signalled
    /// pid the sleep process itself, with its default SIGINT disposition.
    #[test]
    fn interrupt_delivers_sigint_and_does_not_escalate() {
        let mut child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        std::thread::sleep(Duration::from_millis(200));

        interrupt_pid_with_grace(child.id(), Duration::from_millis(300)).expect("interrupt");

        let status = child.wait().expect("wait");
        assert_eq!(
            status.signal(),
            Some(libc::SIGINT),
            "process must die from SIGINT, not from the escalation path"
        );
    }

    /// A process that ignores SIGINT must still go down, via the tree kill.
    #[test]
    fn interrupt_escalates_when_sigint_is_ignored() {
        let mut child = Command::new("sh")
            .args(["-c", "trap '' INT; sleep 30"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        std::thread::sleep(Duration::from_millis(200));

        let started = std::time::Instant::now();
        interrupt_pid_with_grace(child.id(), Duration::from_millis(300)).expect("interrupt");

        // Still alive right after the SIGINT: it was ignored.
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            child.try_wait().expect("try_wait").is_none(),
            "shell traps INT, so it must survive the graceful tier"
        );

        let status = child.wait().expect("wait");
        assert_eq!(
            status.signal(),
            Some(libc::SIGTERM),
            "escalation must SIGTERM the tree"
        );
        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "escalation must wait out the full grace period"
        );
    }

    #[test]
    fn interrupt_reports_missing_process() {
        // Reap a child, then signal its (now free) pid.
        let mut child = Command::new("sh").args(["-c", "exit 0"]).spawn().expect("spawn");
        let pid = child.id();
        child.wait().expect("wait");
        std::thread::sleep(Duration::from_millis(100));
        assert!(interrupt_pid_with_grace(pid, Duration::from_millis(50)).is_err());
    }
}

#[cfg(all(test, unix))]
mod interrupt_orphan_tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn alive(pattern: &str) -> bool {
        Command::new("pgrep")
            .args(["-f", pattern])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// An interactive `claude` exits on SIGINT (rc 0) and leaves its tool child
    /// reparented to init — verified against claude 2.1.204 on a pty. Only the
    /// headless `-p` sessions reap their own children. So interrupt must sweep
    /// the tree it captured up front, not just escalate when the root survives.
    #[test]
    fn interrupt_reaps_orphans_when_the_root_exits() {
        let marker = "sleep 5051";
        // Dies on SIGINT and abandons its background child — the interactive shape.
        let mut child = Command::new("sh")
            .args(["-c", "sleep 5051 & trap 'exit 0' INT; wait"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        std::thread::sleep(Duration::from_millis(300));
        assert!(alive(marker), "precondition: the tool child must be running");

        interrupt_pid_with_grace(child.id(), Duration::from_millis(300)).expect("interrupt");
        let status = child.wait().expect("wait");
        assert!(status.success(), "root should exit cleanly on SIGINT");

        // Grace window + the sweep's own SIGTERM->SIGKILL delay.
        std::thread::sleep(Duration::from_millis(1200));
        let leaked = alive(marker);
        Command::new("pkill").args(["-9", "-f", marker]).output().ok();

        assert!(!leaked, "interrupt orphaned the tool child after the root exited");
    }
}
