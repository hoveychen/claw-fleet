//! Claude Code agent source — scans ~/.claude/projects/ for JSONL session files.

use std::path::PathBuf;

use serde_json::Value;

use crate::agent_source::{AgentSource, WatchStrategy};
use crate::backend::SourceUsageSummary;
use crate::memory::{MemoryHistoryEntry, WorkspaceMemory};
use crate::session::{get_claude_dir, SessionInfo};

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

    fn watch_strategy(&self) -> WatchStrategy {
        WatchStrategy::Filesystem
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        get_claude_dir().into_iter().collect()
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
        crate::local_backend::kill_pid_impl(pid)
    }

    fn kill_workspace(&self, workspace_path: &str) -> Result<(), String> {
        crate::local_backend::kill_workspace_impl(workspace_path)
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
}
