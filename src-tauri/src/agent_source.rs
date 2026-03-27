//! AgentSource trait — abstraction for different AI agent backends.
//!
//! Each agent source (Claude Code, Cursor, OpenClaw) implements this trait
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

/// How a source should be monitored for changes.
pub enum WatchStrategy {
    /// Watch filesystem paths with `notify` (Claude Code, OpenClaw).
    Filesystem,
    /// Poll periodically at the given interval (Cursor SQLite).
    Poll(Duration),
}

/// Trait implemented by each agent source (Claude Code, Cursor, OpenClaw, …).
///
/// Default implementations return "not supported" for optional capabilities,
/// so sources only need to implement what they actually support.
pub trait AgentSource: Send + Sync {
    /// Unique identifier: "claude-code", "cursor", "openclaw".
    fn name(&self) -> &'static str;

    /// URI scheme prefix used in `SessionInfo::jsonl_path`.
    /// - Claude Code: `""` (bare file paths)
    /// - Cursor: `"cursor://"`
    /// - OpenClaw: `"openclaw://"`
    fn uri_prefix(&self) -> &'static str;

    /// Whether this source is currently available on the system.
    fn is_available(&self) -> bool;

    /// Scan for all sessions from this source.
    fn scan_sessions(&self) -> Vec<SessionInfo>;

    /// Read messages for a session identified by its path/URI.
    fn get_messages(&self, path: &str) -> Result<Vec<Value>, String>;

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

    /// Short API name used by the frontend (e.g. "claude", "cursor", "codex", "openclaw").
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
    fn kill_pid(&self, _pid: u32) -> Result<(), String> {
        Err(format!("{}: kill_pid not supported", self.name()))
    }

    /// Kill all processes in a workspace.
    fn kill_workspace(&self, _workspace_path: &str) -> Result<(), String> {
        Err(format!("{}: kill_workspace not supported", self.name()))
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
/// Stored in `~/.claude/fleet-sources.json`.
///
/// ```json
/// {
///   "claude-code": { "enabled": true },
///   "cursor":      { "enabled": false },
///   "openclaw":    { "enabled": true }
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
    /// Read the config from `~/.claude/fleet-sources.json`.
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

    /// Write the config to `~/.claude/fleet-sources.json`.
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
    dirs::home_dir().map(|h| h.join(".claude").join("fleet-sources.json"))
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
            let home = dirs::home_dir();
            let cli_exists = {
                #[cfg(unix)]
                { std::process::Command::new("which").arg("claude").output().map_or(false, |o| o.status.success()) }
                #[cfg(not(unix))]
                { std::process::Command::new("where").arg("claude").output().map_or(false, |o| o.status.success()) }
            };
            cli_exists || home.as_ref().map_or(false, |h| h.join(".claude").is_dir())
        }),
        ("cursor", {
            dirs::home_dir().map_or(false, |h| h.join(".cursor").is_dir())
        }),
        ("openclaw", {
            let home = dirs::home_dir();
            home.as_ref().map_or(false, |h| h.join(".openclaw").is_dir())
                || {
                    #[cfg(unix)]
                    { std::process::Command::new("which").arg("openclaw").output().map_or(false, |o| o.status.success()) }
                    #[cfg(not(unix))]
                    { std::process::Command::new("where").arg("openclaw").output().map_or(false, |o| o.status.success()) }
                }
        }),
        ("codex", {
            let home = dirs::home_dir();
            home.as_ref().map_or(false, |h| h.join(".codex").is_dir())
                || {
                    #[cfg(unix)]
                    { std::process::Command::new("which").arg("codex").output().map_or(false, |o| o.status.success()) }
                    #[cfg(not(unix))]
                    { std::process::Command::new("where").arg("codex").output().map_or(false, |o| o.status.success()) }
                }
        }),
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

    // Cursor
    if config.is_enabled("cursor") {
        sources.push(Box::new(crate::cursor::CursorSource));
    }

    // OpenClaw
    if config.is_enabled("openclaw") {
        sources.push(Box::new(crate::openclaw_source::OpenClawSource::new()));
    }

    // Codex
    if config.is_enabled("codex") {
        sources.push(Box::new(crate::codex_source::CodexSource::new()));
    }

    sources
}

/// Find a source by its API name (e.g. "claude", "cursor", "codex", "openclaw").
pub fn find_source_by_api_name<'a>(
    sources: &'a [Box<dyn AgentSource>],
    api_name: &str,
) -> Option<&'a dyn AgentSource> {
    sources.iter().find(|s| s.api_name() == api_name).map(|s| s.as_ref())
}

/// Find the source that handles a given path/URI by matching URI prefix.
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
                MockAgentSource::new("cursor", "cursor", "cursor://")
                    .with_account(json!({"email": "test@example.com"}))
                    .with_usage(json!({"usage": []})),
            ),
            Box::new(
                MockAgentSource::new("codex", "codex", "codex://")
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

        let cursor = find_source_by_api_name(&sources, "cursor");
        assert!(cursor.is_some());
        assert_eq!(cursor.unwrap().name(), "cursor");

        let codex = find_source_by_api_name(&sources, "codex");
        assert!(codex.is_some());
        assert_eq!(codex.unwrap().name(), "codex");

        assert!(find_source_by_api_name(&sources, "nonexistent").is_none());
    }

    #[test]
    fn find_source_for_path_matches_uri_prefix() {
        let sources = make_sources();

        // cursor:// prefix should match cursor source
        let cursor = find_source_for_path(&sources, "cursor://abc123");
        assert!(cursor.is_some());
        assert_eq!(cursor.unwrap().name(), "cursor");

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

        let cursor = find_source_by_api_name(&sources, "cursor").unwrap();
        let result = cursor.fetch_account();
        assert!(result.is_ok());
        assert_eq!(result.unwrap()["email"], "test@example.com");

        // codex has no account endpoint
        let codex = find_source_by_api_name(&sources, "codex").unwrap();
        let result = codex.fetch_account();
        assert!(result.is_err());
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
                    }),
            ),
            Box::new(
                MockAgentSource::new("source-b", "b", "b://")
                    .unavailable()
                    .with_summary(SourceUsageSummary {
                        source: "b".into(),
                        plan: Some("free".into()),
                        bars: vec![],
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
        assert!(config.is_source_enabled("cursor"));
    }
}
