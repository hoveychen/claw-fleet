//! AgentSource trait — abstraction for different AI agent backends.
//!
//! Each agent source (Claude Code, Codex) implements this trait
//! to provide session scanning, message reading, and lifecycle management.
//! `LocalBackend` owns a `Vec<Box<dyn AgentSource>>` and delegates to
//! the appropriate source based on URI prefix matching.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::backend::SourceUsageSummary;
use crate::memory::{MemoryHistoryEntry, WorkspaceMemory};
use crate::session::SessionInfo;

/// Parameters for launching a brand-new session, source-agnostic.
///
/// Not every field applies to every source: `session_id` and `entrypoint` are
/// Claude-specific (Codex mints its own thread id and has no entrypoint env);
/// Codex ignores them. `permission_mode` is Claude's `--permission-mode` today;
/// the Codex sandbox/approval mapping is a later milestone.
#[derive(Clone, Debug, Default)]
pub struct SpawnSpec {
    pub workspace_path: String,
    pub prompt: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    /// Caller-pre-assigned session id (Claude `--session-id`); Codex ignores it.
    pub session_id: Option<String>,
    /// `CLAUDE_CODE_ENTRYPOINT` value (Claude only); empty → the source's default.
    pub entrypoint: String,
}

/// Parameters for resuming an existing session, source-agnostic.
///
/// The analogue of [`SpawnSpec`] for the resume path. `session_id` is the
/// source's own id (Claude session uuid / Codex thread id). `prompt` empty →
/// the source's default follow-up ("continue"). `permission_mode` is Claude's
/// `--permission-mode`; Codex ignores it (its sandbox/approval mapping is a
/// later milestone).
#[derive(Clone, Debug, Default)]
pub struct ResumeSpec {
    pub session_id: String,
    pub workspace_path: String,
    /// Follow-up prompt for the resumed turn; empty → source default.
    pub prompt: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
}

/// How a source should be monitored for changes.
pub enum WatchStrategy {
    /// Watch filesystem paths with `notify` (Claude Code).
    Filesystem,
    /// Poll periodically at the given interval.
    Poll(Duration),
}

/// Trait implemented by each agent source (Claude Code, Codex, …).
///
/// Default implementations return "not supported" for optional capabilities,
/// so sources only need to implement what they actually support.
pub trait AgentSource: Send + Sync {
    /// Unique identifier: "claude-code", "codex".
    fn name(&self) -> &'static str;

    /// URI scheme prefix used in `SessionInfo::jsonl_path`.
    /// - Claude Code: `""` (bare file paths)
    /// - Codex: `"codex://"`
    fn uri_prefix(&self) -> &'static str;

    /// Whether this source is currently available on the system.
    fn is_available(&self) -> bool;

    /// Scan for all sessions from this source.
    fn scan_sessions(&self) -> Vec<SessionInfo>;

    /// Read messages for a session identified by its path/URI.
    fn get_messages(&self, path: &str) -> Result<Vec<Value>, String>;

    /// Read **at most** the last `n` messages of a session. Used by the
    /// SessionDetail UI to avoid materializing huge transcripts on open.
    /// Default impl falls back to `get_messages` and slices; sources backed by
    /// a single jsonl file should override with a streaming tail reader.
    fn get_messages_tail(&self, path: &str, n: usize) -> Result<Vec<Value>, String> {
        let all = self.get_messages(path)?;
        let start = all.len().saturating_sub(n);
        Ok(all[start..].to_vec())
    }

    /// Incrementally follow a session's transcript for the live detail view,
    /// returning `(messages, new_offset)` for the growth since byte `offset`.
    ///
    /// The default is the byte-offset raw tail every jsonl source used before
    /// this method existed: seek to `offset`, parse the complete lines appended
    /// since, advance the offset past them. It is correct for sources whose
    /// on-disk records are already self-contained, renderable messages one per
    /// line (Claude Code) — the client appends the returned lines verbatim.
    ///
    /// Sources whose rollout format is *folded* (a message reconstructed from
    /// several lines with whole-file context — Codex) MUST override this: a raw
    /// byte slice of such a rollout is not a message, so appending it renders
    /// nothing. The override returns freshly *normalized* messages carrying the
    /// stable per-message `uuid` the clients dedup on.
    fn tail_incremental(&self, path: &str, offset: u64) -> Result<(Vec<Value>, u64), String> {
        use std::io::{Read as _, Seek as _};
        let real = self
            .resolve_file_path(path)
            .ok_or_else(|| format!("cannot resolve path: {path}"))?;
        let size = std::fs::metadata(&real).map_err(|e| e.to_string())?.len();
        // Truncated/rotated file (offset past EOF) or no growth: resync, no lines.
        if offset >= size {
            return Ok((Vec::new(), size));
        }
        let mut file = std::fs::File::open(&real).map_err(|e| e.to_string())?;
        file.seek(std::io::SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
        let mut buf = String::new();
        file.read_to_string(&mut buf).map_err(|e| e.to_string())?;
        // Advance only past complete (newline-terminated) lines; a half-written
        // trailing record stays unconsumed so the next poll re-reads its bytes.
        let (lines, consumed) = crate::jsonl_tail::parse_incremental_tail(&buf);
        Ok((lines, offset + consumed as u64))
    }

    /// How this source should be watched for changes.
    fn watch_strategy(&self) -> WatchStrategy;

    /// Directories to watch with `notify` (for `WatchStrategy::Filesystem`).
    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![]
    }

    /// File extensions that should trigger a rescan for this source.
    fn trigger_extensions(&self) -> Vec<&'static str> {
        vec![]
    }

    /// Additional filesystem directories to watch for memory file changes
    /// (`.md` files under `*/memory/` subdirectories).
    ///
    /// Return paths that contain memory files but are **not** already covered
    /// by `watch_paths()`.  For sources whose session dirs and memory dirs
    /// overlap (e.g. Claude Code stores everything under `~/.claude/`), this
    /// can return an empty vec — the session watcher already receives the
    /// events and `LocalBackend` filters them by path.
    ///
    /// Sources that keep memory in a separate location (e.g. a different tool
    /// storing memory outside its session dir) should return those dirs here
    /// so they are added to the notify watcher.
    fn memory_watch_paths(&self) -> Vec<PathBuf> {
        vec![]
    }

    /// Whether a given file path belongs to this source's watch domain.
    fn owns_path(&self, path: &str) -> bool {
        let prefix = self.uri_prefix();
        if prefix.is_empty() {
            // Claude Code: owns paths under ~/.claude/
            // Check done externally via watch_paths
            false
        } else {
            path.starts_with(prefix)
        }
    }

    /// Resolve a session URI/path to a real filesystem path (for JSONL-based sources).
    /// Used by the probe's `/tail` and `/file_size` endpoints.
    /// Default implementation returns the path as-is (bare file paths, e.g. Claude Code).
    fn resolve_file_path(&self, path: &str) -> Option<PathBuf> {
        Some(PathBuf::from(path))
    }

    /// Short API name used by the frontend (e.g. "claude", "codex").
    /// Defaults to `self.name()`, override for sources where the config name differs
    /// (e.g. "claude-code" → "claude").
    fn api_name(&self) -> &'static str {
        self.name()
    }

    // ── Account & usage (per-source) ────────────────────────────────────────

    /// Fetch account/profile info for this source. Returns JSON.
    fn fetch_account(&self) -> Result<Value, String> {
        Err(format!("{}: fetch_account not supported", self.name()))
    }

    /// Fetch usage info for this source. Returns JSON.
    fn fetch_usage(&self) -> Result<Value, String> {
        Err(format!("{}: fetch_usage not supported", self.name()))
    }

    /// Build a normalised usage summary for the tray/overview bar.
    fn usage_summary(&self) -> Option<SourceUsageSummary> {
        None
    }

    // ── Optional capabilities ───────────────────────────────────────────────

    /// Kill a process by PID.
    /// Stop the turn `session_id` is running, without touching any other
    /// session.
    ///
    /// Fleet's stop button is otherwise pid-based, which assumes one process per
    /// session. That assumption holds for Claude Code and Codex and fails for
    /// dsh: every dsh session's turn runs inside one shared `dsh web`, so the
    /// pid on a dsh `SessionInfo` is that server's — signalling it would stop
    /// every dsh session at once and take Fleet's own server down with them.
    ///
    /// A source that overrides this is saying "route the stop button here
    /// instead of at my pid". The default refusal is what keeps the pid path in
    /// charge for the two sources where it is correct.
    fn interrupt_session(&self, _session_id: &str) -> Result<(), String> {
        Err(format!("{}: interrupt_session not supported", self.name()))
    }

    fn kill_pid(&self, _pid: u32) -> Result<(), String> {
        Err(format!("{}: kill_pid not supported", self.name()))
    }

    /// Kill all processes in a workspace.
    fn kill_workspace(&self, _workspace_path: &str) -> Result<(), String> {
        Err(format!("{}: kill_workspace not supported", self.name()))
    }

    /// Launch a brand-new headless session for this source. Default is "not
    /// supported"; sources that can be Fleet-launched (Claude, Codex) override.
    fn spawn(
        &self,
        _spec: &SpawnSpec,
    ) -> Result<crate::session_launch::SpawnSessionResponse, String> {
        Err(format!("{}: spawn not supported", self.name()))
    }

    /// Headlessly resume an existing session for this source. Default is "not
    /// supported"; sources that can be Fleet-resumed (Claude, Codex) override.
    ///
    /// `on_exit(success)` is invoked from the reaper thread when the spawned
    /// resume process exits — the auto-resume scheduler uses it to free its
    /// concurrency slot and record backoff. Manual (untracked) resume passes a
    /// no-op. A boxed `FnOnce` (rather than a generic) keeps the trait
    /// object-safe for `dyn AgentSource`.
    fn resume(
        &self,
        _spec: &ResumeSpec,
        _on_exit: Box<dyn FnOnce(bool) + Send>,
    ) -> Result<(), String> {
        Err(format!("{}: resume not supported", self.name()))
    }

    /// List memory files from this source.
    fn list_memories(&self) -> Vec<WorkspaceMemory> {
        vec![]
    }

    /// Read a memory file.
    fn get_memory_content(&self, _path: &str) -> Result<String, String> {
        Err(format!("{}: get_memory_content not supported", self.name()))
    }

    /// Get memory edit history.
    fn get_memory_history(&self, _path: &str) -> Vec<MemoryHistoryEntry> {
        vec![]
    }
}

// ── Source registry & configuration ─────────────────────────────────────────

/// Per-source enable/disable config.
/// Stored in `~/.fleet/fleet-sources.json`.
///
/// ```json
/// {
///   "claude-code": { "enabled": true },
///   "codex":       { "enabled": true }
/// }
/// ```
///
/// Missing entries default to `enabled: true` — newly installed sources are
/// automatically picked up.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SourcesConfig {
    #[serde(flatten)]
    pub sources: HashMap<String, SourceEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SourceEntry {
    pub enabled: bool,
}

impl SourcesConfig {
    /// Read the config from `~/.fleet/fleet-sources.json`.
    /// Returns default (all enabled) if the file is missing or unparseable.
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Write the config to `~/.fleet/fleet-sources.json`.
    pub fn save(&self) -> Result<(), String> {
        let path = config_path().ok_or("Cannot determine config path")?;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| e.to_string())
    }

    /// Check if a source is enabled.  Missing entries → enabled by default.
    pub fn is_enabled(&self, name: &str) -> bool {
        self.sources
            .get(name)
            .map(|e| e.enabled)
            .unwrap_or(true)
    }

    /// Check if a source is explicitly enabled in config. Returns false if missing.
    pub fn is_explicitly_enabled(&self, name: &str) -> bool {
        self.sources
            .get(name)
            .map(|e| e.enabled)
            .unwrap_or(false)
    }

    /// Check if a source is enabled, accepting both config names ("claude-code")
    /// and API/short names ("claude").
    pub fn is_source_enabled(&self, name: &str) -> bool {
        let config_name = match name {
            "claude" => "claude-code",
            other => other,
        };
        self.is_enabled(config_name)
    }
}

fn config_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("fleet-sources.json"))
}

/// Info about a single agent source exposed to the frontend.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub name: String,
    pub enabled: bool,
    pub available: bool,
}

/// Build the `SourceInfo` list by merging config with availability detection.
pub fn get_sources_config_local() -> Vec<SourceInfo> {
    let config = SourcesConfig::load();

    let all_sources: Vec<(&str, bool)> = vec![
        ("claude-code", {
            let cli_exists = {
                #[cfg(unix)]
                { crate::process_util::command("which").arg("claude").output().map_or(false, |o| o.status.success()) }
                #[cfg(not(unix))]
                { crate::process_util::command("where").arg("claude").output().map_or(false, |o| o.status.success()) }
            };
            cli_exists || crate::session::get_claude_dir().map_or(false, |d| d.is_dir())
        }),
        ("codex", {
            let home = crate::session::real_home_dir();
            home.as_ref().map_or(false, |h| h.join(".codex").is_dir())
                || {
                    #[cfg(unix)]
                    { crate::process_util::command("which").arg("codex").output().map_or(false, |o| o.status.success()) }
                    #[cfg(not(unix))]
                    { crate::process_util::command("where").arg("codex").output().map_or(false, |o| o.status.success()) }
                }
        }),
        // dsh. Availability is the *binary*, not a home directory: `~/.dsh`
        // exists as soon as anything writes a setting there, but this source can
        // only reach its sessions through a `dsh web` it starts itself — which is
        // also the gate [`build_sources`] applies. Reusing `is_available` keeps
        // the row's "not detected" label and the actual scan decision from ever
        // disagreeing.
        ("dsh", crate::dsh_server::is_available()),
    ];

    all_sources
        .into_iter()
        .map(|(name, available)| {
            let enabled = config.is_enabled(name);
            SourceInfo {
                name: name.to_string(),
                enabled,
                available,
            }
        })
        .collect()
}

/// Toggle a source on/off and persist to disk.
pub fn set_source_enabled_local(name: &str, enabled: bool) -> Result<(), String> {
    let mut config = SourcesConfig::load();
    config.sources.insert(name.to_string(), SourceEntry { enabled });
    config.save()
}

/// Build the list of enabled agent sources based on config.
///
/// This is the single entry point used by both `LocalBackend` and `fleet serve`
/// to construct the source registry.
pub fn build_sources() -> Vec<Box<dyn AgentSource>> {
    let config = SourcesConfig::load();
    let mut sources: Vec<Box<dyn AgentSource>> = Vec::new();

    // Claude Code — always registered first (bare file path prefix = fallback).
    if config.is_enabled("claude-code") {
        sources.push(Box::new(crate::claude_source::ClaudeCodeSource::new()));
    }

    // Codex
    if config.is_enabled("codex") {
        sources.push(Box::new(crate::codex_source::CodexSource::new()));
    }

    // dsh (DeepSeek Harness). Gated on the binary existing, not just on the
    // config flag: this source reaches its sessions through a server it has to
    // start, so on a machine without dsh installed every poll would fail and
    // log. The other two sources read files and degrade to an empty list.
    if config.is_enabled("dsh") && crate::dsh_server::is_available() {
        sources.push(Box::new(crate::dsh_source::DshSource::new()));
    }

    sources
}

/// Launch a new session with the given `tool` ("claude" / "codex"), routing to
/// that source's [`AgentSource::spawn`]. Empty/unknown-blank `tool` defaults to
/// "claude" (older callers omitted it). Errors if the tool's source is not
/// registered (e.g. Codex disabled in `fleet-sources.json`) or doesn't support
/// spawning.
pub fn spawn_session(
    tool: &str,
    spec: &SpawnSpec,
) -> Result<crate::session_launch::SpawnSessionResponse, String> {
    let tool = normalize_tool(tool);
    let sources = build_sources();
    let source = find_source_by_api_name(&sources, tool)
        .ok_or_else(|| format!("agent tool '{tool}' is not available or is disabled"))?;
    source.spawn(spec)
}

/// Resume an existing session with the given `tool`, routing to that source's
/// [`AgentSource::resume`]. Accepts both api names ("claude"/"codex") and
/// config/`agent_source` names ("claude-code"); blank defaults to "claude"
/// (older callers omitted it). `on_exit(success)` fires when the resume process
/// exits (no-op box for untracked manual resume). Errors if the tool's source
/// is not registered or doesn't support resuming.
pub fn resume_session(
    tool: &str,
    spec: &ResumeSpec,
    on_exit: Box<dyn FnOnce(bool) + Send>,
) -> Result<(), String> {
    let tool = normalize_tool(tool);
    let sources = build_sources();
    let source = find_source_by_api_name(&sources, tool)
        .ok_or_else(|| format!("agent tool '{tool}' is not available or is disabled"))?;
    source.resume(spec, on_exit)
}

/// Normalise a caller-supplied tool string to a source api name. Blank →
/// "claude"; the config name "claude-code" → its api name "claude" (the resume
/// path receives `SessionInfo::agent_source`, which uses config names). Other
/// values (e.g. "codex") pass through unchanged.
fn normalize_tool(tool: &str) -> &str {
    match tool.trim() {
        "" | "claude-code" => "claude",
        t => t,
    }
}

/// Find a source by its API name (e.g. "claude", "codex").
pub fn find_source_by_api_name<'a>(
    sources: &'a [Box<dyn AgentSource>],
    api_name: &str,
) -> Option<&'a dyn AgentSource> {
    sources.iter().find(|s| s.api_name() == api_name).map(|s| s.as_ref())
}

/// Find the source that handles a given path/URI by matching URI prefix.
/// Stop the turn running in the session at `path`, through whichever source
/// owns it.
///
/// The counterpart to the pid-based stop for sources that have no per-session
/// process. Errors when no source claims the path, or when the owning source
/// has no per-session interrupt — in which case the caller should be signalling
/// the pid instead, and the error says so rather than silently doing nothing.
pub fn interrupt_session_at(path: &str) -> Result<(), String> {
    let sources = build_sources();
    let source = find_source_for_path(&sources, path)
        .ok_or_else(|| format!("no agent source owns {path}"))?;
    source.interrupt_session(path)
}

pub fn find_source_for_path<'a>(
    sources: &'a [Box<dyn AgentSource>],
    path: &str,
) -> Option<&'a dyn AgentSource> {
    // Try non-empty prefixes first (most specific match).
    for source in sources {
        let prefix = source.uri_prefix();
        if !prefix.is_empty() && path.starts_with(prefix) {
            return Some(source.as_ref());
        }
    }
    // Fall back to the source with empty prefix (Claude Code = bare file paths).
    for source in sources {
        if source.uri_prefix().is_empty() {
            return Some(source.as_ref());
        }
    }
    None
}

// ── Shared helpers ──────────────────────────────────────────────────────────

/// Fetch usage summaries from all available sources via trait dispatch.
/// All network I/O happens here, outside any Mutex guard.
pub fn fetch_usage_summaries_from_sources(sources: &[Box<dyn AgentSource>]) -> Vec<SourceUsageSummary> {
    sources
        .iter()
        .filter(|s| s.is_available())
        .filter_map(|s| s.usage_summary())
        .collect()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A configurable mock for unit-testing code that accepts `dyn AgentSource`.
    pub struct MockAgentSource {
        pub name: &'static str,
        pub api_name: &'static str,
        pub uri_prefix: &'static str,
        pub available: bool,
        pub sessions: Vec<SessionInfo>,
        pub messages: Result<Vec<Value>, String>,
        pub account: Result<Value, String>,
        pub usage: Result<Value, String>,
        pub summary: Option<SourceUsageSummary>,
    }

    impl MockAgentSource {
        /// Create a minimal mock with sensible defaults.
        pub fn new(name: &'static str, api_name: &'static str, uri_prefix: &'static str) -> Self {
            Self {
                name,
                api_name,
                uri_prefix,
                available: true,
                sessions: vec![],
                messages: Ok(vec![]),
                account: Err("not configured".into()),
                usage: Err("not configured".into()),
                summary: None,
            }
        }

        pub fn with_account(mut self, val: Value) -> Self {
            self.account = Ok(val);
            self
        }

        pub fn with_usage(mut self, val: Value) -> Self {
            self.usage = Ok(val);
            self
        }

        pub fn with_summary(mut self, s: SourceUsageSummary) -> Self {
            self.summary = Some(s);
            self
        }

        pub fn unavailable(mut self) -> Self {
            self.available = false;
            self
        }
    }

    impl AgentSource for MockAgentSource {
        fn name(&self) -> &'static str { self.name }
        fn api_name(&self) -> &'static str { self.api_name }
        fn uri_prefix(&self) -> &'static str { self.uri_prefix }
        fn is_available(&self) -> bool { self.available }
        fn scan_sessions(&self) -> Vec<SessionInfo> { self.sessions.clone() }
        fn get_messages(&self, _path: &str) -> Result<Vec<Value>, String> { self.messages.clone() }
        fn watch_strategy(&self) -> WatchStrategy { WatchStrategy::Poll(Duration::from_secs(5)) }
        fn fetch_account(&self) -> Result<Value, String> { self.account.clone() }
        fn fetch_usage(&self) -> Result<Value, String> { self.usage.clone() }
        fn usage_summary(&self) -> Option<SourceUsageSummary> { self.summary.clone() }
    }

    fn make_sources() -> Vec<Box<dyn AgentSource>> {
        vec![
            Box::new(
                MockAgentSource::new("claude-code", "claude", "")
                    .with_account(json!({"plan": "max"}))
                    .with_usage(json!({"plan": "max", "utilization": 0.5})),
            ),
            Box::new(
                MockAgentSource::new("codex", "codex", "codex://")
                    .with_account(json!({"email": "test@example.com"}))
                    .with_usage(json!({"planType": "plus"})),
            ),
        ]
    }

    #[test]
    fn find_source_by_api_name_returns_correct_source() {
        let sources = make_sources();

        let claude = find_source_by_api_name(&sources, "claude");
        assert!(claude.is_some());
        assert_eq!(claude.unwrap().name(), "claude-code");

        let codex = find_source_by_api_name(&sources, "codex");
        assert!(codex.is_some());
        assert_eq!(codex.unwrap().name(), "codex");

        assert!(find_source_by_api_name(&sources, "nonexistent").is_none());
    }

    #[test]
    fn find_source_for_path_matches_uri_prefix() {
        let sources = make_sources();

        // codex:// prefix should match codex source
        let codex = find_source_for_path(&sources, "codex://session1");
        assert!(codex.is_some());
        assert_eq!(codex.unwrap().name(), "codex");

        // bare path falls back to claude-code (empty prefix)
        let claude = find_source_for_path(&sources, "/home/user/.claude/projects/foo.jsonl");
        assert!(claude.is_some());
        assert_eq!(claude.unwrap().name(), "claude-code");
    }

    #[test]
    fn fetch_account_dispatches_via_trait() {
        let sources = make_sources();

        let claude = find_source_by_api_name(&sources, "claude").unwrap();
        let result = claude.fetch_account();
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["plan"], "max");

        let codex = find_source_by_api_name(&sources, "codex").unwrap();
        let result = codex.fetch_account();
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["email"], "test@example.com");
    }

    #[test]
    fn fetch_usage_dispatches_via_trait() {
        let sources = make_sources();

        let claude = find_source_by_api_name(&sources, "claude").unwrap();
        let result = claude.fetch_usage();
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["utilization"], 0.5);

        let codex = find_source_by_api_name(&sources, "codex").unwrap();
        let result = codex.fetch_usage();
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["planType"], "plus");
    }

    #[test]
    fn usage_summary_filters_unavailable_sources() {
        let sources: Vec<Box<dyn AgentSource>> = vec![
            Box::new(
                MockAgentSource::new("source-a", "a", "a://")
                    .with_summary(SourceUsageSummary {
                        source: "a".into(),
                        plan: Some("pro".into()),
                        bars: vec![],
                        usage_source: None,
                    }),
            ),
            Box::new(
                MockAgentSource::new("source-b", "b", "b://")
                    .unavailable()
                    .with_summary(SourceUsageSummary {
                        source: "b".into(),
                        plan: Some("free".into()),
                        bars: vec![],
                        usage_source: None,
                    }),
            ),
            Box::new(
                MockAgentSource::new("source-c", "c", "c://")
                    // available but no summary
            ),
        ];

        let summaries: Vec<SourceUsageSummary> = sources
            .iter()
            .filter(|s| s.is_available())
            .filter_map(|s| s.usage_summary())
            .collect();

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].source, "a");
    }

    #[test]
    fn api_name_defaults_to_name() {
        let mock = MockAgentSource {
            name: "test-source",
            api_name: "test-source", // same as name
            uri_prefix: "test://",
            available: true,
            sessions: vec![],
            messages: Ok(vec![]),
            account: Err("n/a".into()),
            usage: Err("n/a".into()),
            summary: None,
        };
        assert_eq!(mock.api_name(), mock.name());
    }

    #[test]
    fn api_name_can_differ_from_name() {
        let sources = make_sources();
        let claude = find_source_by_api_name(&sources, "claude").unwrap();
        assert_eq!(claude.name(), "claude-code");
        assert_eq!(claude.api_name(), "claude");
    }

    #[test]
    fn sources_config_missing_entry_defaults_enabled() {
        let config = SourcesConfig::default();
        assert!(config.is_enabled("claude-code"));
        assert!(config.is_enabled("nonexistent"));
    }

    #[test]
    fn sources_config_is_source_enabled_maps_claude() {
        let mut config = SourcesConfig::default();
        config.sources.insert("claude-code".into(), SourceEntry { enabled: false });

        // "claude" should map to "claude-code"
        assert!(!config.is_source_enabled("claude"));
        assert!(!config.is_source_enabled("claude-code"));

        // other names pass through
        assert!(config.is_source_enabled("codex"));
    }

    /// An unknown tool name must fail loudly in the dispatcher rather than
    /// silently launching Claude — the routing decision has to be explicit.
    /// (Uses a bogus tool so no real process is ever spawned.)
    #[test]
    fn spawn_session_rejects_unknown_tool() {
        let err = super::spawn_session("definitely-not-a-tool", &SpawnSpec::default())
            .unwrap_err();
        assert!(
            err.contains("not available") || err.contains("disabled"),
            "unexpected error: {err}"
        );
    }

    /// The resume dispatcher must reject an unknown tool loudly rather than
    /// silently falling back to Claude — same contract as `spawn_session`.
    /// (Bogus tool → no real process is ever spawned.)
    #[test]
    fn resume_session_rejects_unknown_tool() {
        let err = super::resume_session(
            "definitely-not-a-tool",
            &ResumeSpec::default(),
            Box::new(|_| {}),
        )
        .unwrap_err();
        assert!(
            err.contains("not available") || err.contains("disabled"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn normalize_tool_maps_blank_and_claude_code_to_claude() {
        assert_eq!(super::normalize_tool(""), "claude");
        assert_eq!(super::normalize_tool("  "), "claude");
        assert_eq!(super::normalize_tool("claude-code"), "claude");
        assert_eq!(super::normalize_tool("claude"), "claude");
        assert_eq!(super::normalize_tool("codex"), "codex");
    }
}
