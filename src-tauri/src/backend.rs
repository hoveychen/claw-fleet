//! Backend trait — abstraction over local (file-based) and remote (HTTP probe)
//! data sources.  Both `LocalBackend` and `RemoteBackend` implement this trait
//! so that all Tauri command handlers can be written as simple delegations with
//! no `if remote { … } else { … }` branching.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::account::AccountInfo;
use crate::audit::AuditSummary;
use crate::daily_report::{DailyReport, DailyReportStats, Lesson};
use crate::memory::{MemoryHistoryEntry, WorkspaceMemory};
use crate::search_index::SearchHit;
use crate::session::SessionInfo;
use crate::skills::SkillItem;

// ── Shared types ─────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DetectedTools {
    pub cli: bool,
    pub vscode: bool,
    pub jetbrains: bool,
    pub desktop: bool,
    pub cursor: bool,
    pub openclaw: bool,
    pub codex: bool,
}

impl Default for DetectedTools {
    fn default() -> Self {
        DetectedTools {
            cli: false,
            vscode: false,
            jetbrains: false,
            desktop: false,
            cursor: false,
            openclaw: false,
            codex: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SetupStatus {
    pub cli_installed: bool,
    pub cli_path: Option<String>,
    pub claude_dir_exists: bool,
    pub detected_tools: DetectedTools,
    pub logged_in: bool,
    pub has_sessions: bool,
    pub credentials_valid: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WaitingAlert {
    pub session_id: String,
    pub workspace_name: String,
    pub summary: String,
    pub detected_at_ms: u64,
    pub jsonl_path: String,
}

// ── Unified usage summary for tray / overview ───────────────────────────────

/// A single rate-limit bar (e.g. "5h", "7d Opus", "Premium requests").
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct UsageBar {
    pub label: String,
    /// 0.0–1.0
    pub utilization: f64,
    pub resets_at: Option<String>,
}

/// Normalised usage snapshot for one agent source.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SourceUsageSummary {
    /// Source identifier: "claude", "cursor", "codex", "openclaw".
    pub source: String,
    /// Plan / tier label (e.g. "Max 5x", "pro", "Plus").
    pub plan: Option<String>,
    /// Rate-limit windows, each with a utilization bar.
    pub bars: Vec<UsageBar>,
}

impl SourceUsageSummary {
    /// Convert Claude's `AccountInfo` into a unified summary.
    pub fn from_claude(info: &AccountInfo) -> Self {
        let mut bars = Vec::new();
        if let Some(ref fh) = info.five_hour {
            bars.push(UsageBar {
                label: "5h".into(),
                utilization: fh.utilization,
                resets_at: Some(fh.resets_at.clone()),
            });
        }
        if let Some(ref sd) = info.seven_day {
            bars.push(UsageBar {
                label: "7d Opus".into(),
                utilization: sd.utilization,
                resets_at: Some(sd.resets_at.clone()),
            });
        }
        if let Some(ref ss) = info.seven_day_sonnet {
            bars.push(UsageBar {
                label: "7d Sonnet".into(),
                utilization: ss.utilization,
                resets_at: Some(ss.resets_at.clone()),
            });
        }
        SourceUsageSummary {
            source: "claude".into(),
            plan: if info.plan.is_empty() { None } else { Some(info.plan.clone()) },
            bars,
        }
    }

    /// Convert Cursor's `CursorAccountInfo` JSON value into a unified summary.
    pub fn from_cursor(val: &Value) -> Self {
        let membership = val["membershipType"].as_str().unwrap_or("").to_string();
        let mut bars = Vec::new();
        if let Some(items) = val["usage"].as_array() {
            for item in items {
                let name = item["name"].as_str().unwrap_or("unknown");
                let utilization = item["utilization"].as_f64().unwrap_or_else(|| {
                    let used = item["used"].as_u64().unwrap_or(0) as f64;
                    let limit = item["limit"].as_u64().unwrap_or(1) as f64;
                    if limit > 0.0 { used / limit } else { 0.0 }
                });
                bars.push(UsageBar {
                    label: name.to_string(),
                    utilization,
                    resets_at: item["resetsAt"].as_str().map(|s| s.to_string()),
                });
            }
        }
        SourceUsageSummary {
            source: "cursor".into(),
            plan: if membership.is_empty() { None } else { Some(membership) },
            bars,
        }
    }

    /// Convert Codex's `CodexUsageItem` JSON value into a unified summary.
    pub fn from_codex(val: &Value) -> Self {
        let plan = val["planType"].as_str().map(|s| s.to_string());
        let mut bars = Vec::new();
        if let Some(primary) = val.get("primary") {
            let pct = primary["usedPercent"].as_i64().unwrap_or(0);
            let resets = primary["resetsAt"].as_i64()
                .map(|ts| chrono::DateTime::from_timestamp(ts, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default());
            bars.push(UsageBar {
                label: "Primary".into(),
                utilization: pct as f64 / 100.0,
                resets_at: resets,
            });
        }
        if let Some(secondary) = val.get("secondary") {
            let pct = secondary["usedPercent"].as_i64().unwrap_or(0);
            let resets = secondary["resetsAt"].as_i64()
                .map(|ts| chrono::DateTime::from_timestamp(ts, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default());
            bars.push(UsageBar {
                label: "Secondary".into(),
                utilization: pct as f64 / 100.0,
                resets_at: resets,
            });
        }
        SourceUsageSummary {
            source: "codex".into(),
            plan,
            bars,
        }
    }

    /// Convert OpenClaw's `OpenClawUsageInfo` JSON value into a unified summary.
    /// Shows the highest context utilisation across active sessions.
    pub fn from_openclaw(val: &Value) -> Self {
        let mut bars = Vec::new();
        if let Some(sessions) = val["sessions"].as_array() {
            // Pick the session with the highest context usage as the representative bar.
            if let Some(top) = sessions.iter()
                .filter_map(|s| s["percentUsed"].as_f64().map(|p| (p, s)))
                .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            {
                let model = top.1["model"].as_str().unwrap_or("unknown");
                bars.push(UsageBar {
                    label: format!("ctx ({})", model),
                    utilization: top.0 / 100.0,
                    resets_at: None,
                });
            }
        }
        SourceUsageSummary {
            source: "openclaw".into(),
            plan: None,
            bars,
        }
    }
}

// ── Backend trait ─────────────────────────────────────────────────────────────

/// A boxed, Send future returning `Result<T, String>`.
pub type AccountInfoFuture = Pin<Box<dyn Future<Output = Result<AccountInfo, String>> + Send>>;
/// Generic future for per-source account/usage data (returns untyped JSON).
pub type SourceDataFuture = Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>;

pub trait Backend: Send + Sync {
    fn list_sessions(&self) -> Vec<SessionInfo>;
    fn get_messages(&self, path: &str) -> Result<Vec<Value>, String>;
    fn kill_pid(&self, pid: u32) -> Result<(), String>;
    fn kill_workspace(&self, workspace_path: String) -> Result<(), String>;
    fn account_info(&self) -> AccountInfoFuture;
    /// Fetch account/profile info for the given source (e.g. "claude", "cursor", "openclaw").
    fn source_account(&self, source: &str) -> SourceDataFuture;
    /// Fetch usage info for the given source (e.g. "claude", "cursor", "codex", "openclaw").
    fn source_usage(&self, source: &str) -> SourceDataFuture;
    /// Fetch usage summaries for all detected sources (for tray menu display).
    fn usage_summaries(&self) -> Vec<SourceUsageSummary>;
    fn check_setup(&self) -> SetupStatus;
    /// Start tailing a session file for new lines.
    /// Returns the initial byte offset (file size at call time).
    /// New lines are delivered as `session-tail` Tauri events.
    fn start_watch(&self, path: String) -> Result<u64, String>;
    fn stop_watch(&self);

    // ── Memory ───────────────────────────────────────────────────────────────
    fn list_memories(&self) -> Vec<WorkspaceMemory>;
    fn get_memory_content(&self, path: &str) -> Result<String, String>;
    fn get_memory_history(&self, path: &str) -> Vec<MemoryHistoryEntry>;

    // ── Skills ────────────────────────────────────────────────────────────────
    fn list_skills(&self) -> Vec<SkillItem>;
    fn get_skill_content(&self, path: &str) -> Result<String, String>;

    // ── Waiting alerts ────────────────────────────────────────────────────────
    fn get_waiting_alerts(&self) -> Vec<WaitingAlert>;

    // ── Hooks ─────────────────────────────────────────────────────────────────
    fn get_hooks_plan(&self) -> crate::hooks::HookSetupPlan;
    fn apply_hooks(&self) -> Result<(), String>;
    fn remove_hooks(&self) -> Result<(), String>;

    // ── Agent sources config ─────────────────────────────────────────────────
    fn get_sources_config(&self) -> Vec<crate::agent_source::SourceInfo>;
    fn set_source_enabled(&self, name: &str, enabled: bool) -> Result<(), String>;

    // ── Full-text search ─────────────────────────────────────────────────────
    fn search_sessions(&self, query: &str, limit: usize) -> Vec<SearchHit>;

    // ── Security audit ──────────────────────────────────────────────────────
    fn get_audit_events(&self) -> AuditSummary;

    // ── Daily reports ────────────────────────────────────────────────────────
    fn get_daily_report(&self, date: &str) -> Result<Option<DailyReport>, String>;
    fn list_daily_report_stats(&self, from: &str, to: &str) -> Vec<DailyReportStats>;
    fn generate_daily_report(&self, date: &str) -> Result<DailyReport, String>;
    fn generate_daily_report_ai_summary(&self, date: &str) -> Result<String, String>;
    fn generate_daily_report_lessons(&self, date: &str) -> Result<Vec<Lesson>, String>;
    fn append_lesson_to_claude_md(&self, lesson: &Lesson) -> Result<(), String>;
}

// ── Shared watch state ───────────────────────────────────────────────────────

/// Tracks which session file is currently being watched (tailed) for live
/// updates.  Used by both `LocalBackend` and `RemoteBackend`.
pub(crate) struct WatchState {
    pub session: std::sync::Mutex<Option<String>>,
    pub offset: std::sync::Mutex<u64>,
}

impl WatchState {
    pub fn new() -> Self {
        Self {
            session: std::sync::Mutex::new(None),
            offset: std::sync::Mutex::new(0),
        }
    }

    pub fn set(&self, path: String, offset: u64) {
        *self.session.lock().unwrap() = Some(path);
        *self.offset.lock().unwrap() = offset;
    }

    pub fn clear(&self) {
        *self.session.lock().unwrap() = None;
        *self.offset.lock().unwrap() = 0;
    }

    pub fn current_path(&self) -> Option<String> {
        self.session.lock().unwrap().clone()
    }
}

/// No-op placeholder used before the real backend is initialised in
/// `tauri::Builder::setup`.  Needed because `AppState` must be `manage()`d
/// before `setup()` runs, but `AppHandle` (required by `LocalBackend`) is only
/// available inside `setup()`.
pub(crate) struct NullBackend;

impl Backend for NullBackend {
    fn list_sessions(&self) -> Vec<SessionInfo> {
        vec![]
    }
    fn get_messages(&self, _: &str) -> Result<Vec<Value>, String> {
        Err("backend not ready".into())
    }
    fn kill_pid(&self, _: u32) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn kill_workspace(&self, _: String) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn account_info(&self) -> AccountInfoFuture {
        Box::pin(async { Err("backend not ready".into()) })
    }
    fn source_account(&self, _: &str) -> SourceDataFuture {
        Box::pin(async { Err("backend not ready".into()) })
    }
    fn source_usage(&self, _: &str) -> SourceDataFuture {
        Box::pin(async { Err("backend not ready".into()) })
    }
    fn usage_summaries(&self) -> Vec<SourceUsageSummary> {
        vec![]
    }
    fn check_setup(&self) -> SetupStatus {
        SetupStatus {
            cli_installed: false,
            cli_path: None,
            claude_dir_exists: false,
            detected_tools: DetectedTools::default(),
            logged_in: false,
            has_sessions: false,
            credentials_valid: None,
        }
    }
    fn start_watch(&self, _: String) -> Result<u64, String> {
        Err("backend not ready".into())
    }
    fn stop_watch(&self) {}
    fn list_memories(&self) -> Vec<WorkspaceMemory> {
        vec![]
    }
    fn get_memory_content(&self, _: &str) -> Result<String, String> {
        Err("backend not ready".into())
    }
    fn get_memory_history(&self, _: &str) -> Vec<MemoryHistoryEntry> {
        vec![]
    }
    fn list_skills(&self) -> Vec<SkillItem> {
        vec![]
    }
    fn get_skill_content(&self, _: &str) -> Result<String, String> {
        Err("backend not ready".into())
    }
    fn get_waiting_alerts(&self) -> Vec<WaitingAlert> {
        vec![]
    }
    fn get_hooks_plan(&self) -> crate::hooks::HookSetupPlan {
        crate::hooks::HookSetupPlan {
            to_add: vec![],
            hooks_globally_disabled: false,
            already_installed: true,
        }
    }
    fn apply_hooks(&self) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn remove_hooks(&self) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn get_sources_config(&self) -> Vec<crate::agent_source::SourceInfo> {
        vec![]
    }
    fn set_source_enabled(&self, _: &str, _: bool) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn search_sessions(&self, _: &str, _: usize) -> Vec<SearchHit> {
        vec![]
    }
    fn get_audit_events(&self) -> AuditSummary {
        AuditSummary { events: vec![], total_sessions_scanned: 0 }
    }
    fn get_daily_report(&self, _: &str) -> Result<Option<DailyReport>, String> {
        Err("backend not ready".into())
    }
    fn list_daily_report_stats(&self, _: &str, _: &str) -> Vec<DailyReportStats> {
        vec![]
    }
    fn generate_daily_report(&self, _: &str) -> Result<DailyReport, String> {
        Err("backend not ready".into())
    }
    fn generate_daily_report_ai_summary(&self, _: &str) -> Result<String, String> {
        Err("backend not ready".into())
    }
    fn generate_daily_report_lessons(&self, _: &str) -> Result<Vec<Lesson>, String> {
        Err("backend not ready".into())
    }
    fn append_lesson_to_claude_md(&self, _: &Lesson) -> Result<(), String> {
        Err("backend not ready".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use crate::account::{AccountInfo, UsageStats};

    // ── SourceUsageSummary::from_claude tests ───────────────────────────────

    #[test]
    fn from_claude_all_windows() {
        let info = AccountInfo {
            plan: "Max 5x".into(),
            five_hour: Some(UsageStats { utilization: 0.3, resets_at: "2026-01-01T00:00:00Z".into(), prev_utilization: None }),
            seven_day: Some(UsageStats { utilization: 0.7, resets_at: "2026-01-07T00:00:00Z".into(), prev_utilization: None }),
            seven_day_sonnet: Some(UsageStats { utilization: 0.1, resets_at: "2026-01-07T00:00:00Z".into(), prev_utilization: None }),
            ..Default::default()
        };
        let s = SourceUsageSummary::from_claude(&info);
        assert_eq!(s.source, "claude");
        assert_eq!(s.plan, Some("Max 5x".into()));
        assert_eq!(s.bars.len(), 3);
        assert_eq!(s.bars[0].label, "5h");
        assert!((s.bars[0].utilization - 0.3).abs() < f64::EPSILON);
        assert_eq!(s.bars[1].label, "7d Opus");
        assert_eq!(s.bars[2].label, "7d Sonnet");
    }

    #[test]
    fn from_claude_partial_windows() {
        let info = AccountInfo {
            plan: "".into(),
            five_hour: Some(UsageStats { utilization: 0.5, resets_at: "t".into(), prev_utilization: None }),
            ..Default::default()
        };
        let s = SourceUsageSummary::from_claude(&info);
        assert_eq!(s.plan, None); // empty plan → None
        assert_eq!(s.bars.len(), 1);
    }

    #[test]
    fn from_claude_no_windows() {
        let info = AccountInfo::default();
        let s = SourceUsageSummary::from_claude(&info);
        assert!(s.bars.is_empty());
    }

    // ── SourceUsageSummary::from_cursor tests ───────────────────────────────

    #[test]
    fn from_cursor_with_usage_items() {
        let val = json!({
            "membershipType": "pro",
            "usage": [
                {"name": "Premium", "utilization": 0.6, "resetsAt": "2026-01-01T00:00:00Z"},
                {"name": "Standard", "used": 50, "limit": 200}
            ]
        });
        let s = SourceUsageSummary::from_cursor(&val);
        assert_eq!(s.source, "cursor");
        assert_eq!(s.plan, Some("pro".into()));
        assert_eq!(s.bars.len(), 2);
        assert!((s.bars[0].utilization - 0.6).abs() < f64::EPSILON);
        // Computed from used/limit: 50/200 = 0.25
        assert!((s.bars[1].utilization - 0.25).abs() < f64::EPSILON);
        assert!(s.bars[1].resets_at.is_none());
    }

    #[test]
    fn from_cursor_empty_usage() {
        let val = json!({"membershipType": "", "usage": []});
        let s = SourceUsageSummary::from_cursor(&val);
        assert_eq!(s.plan, None); // empty → None
        assert!(s.bars.is_empty());
    }

    // ── SourceUsageSummary::from_codex tests ────────────────────────────────

    #[test]
    fn from_codex_with_primary_and_secondary() {
        let val = json!({
            "planType": "plus",
            "primary": {"usedPercent": 45, "resetsAt": 1735689600},
            "secondary": {"usedPercent": 10, "resetsAt": 1735776000}
        });
        let s = SourceUsageSummary::from_codex(&val);
        assert_eq!(s.source, "codex");
        assert_eq!(s.plan, Some("plus".into()));
        assert_eq!(s.bars.len(), 2);
        assert!((s.bars[0].utilization - 0.45).abs() < f64::EPSILON);
        assert!((s.bars[1].utilization - 0.10).abs() < f64::EPSILON);
        assert!(s.bars[0].resets_at.is_some());
    }

    #[test]
    fn from_codex_missing_plan() {
        let val = json!({});
        let s = SourceUsageSummary::from_codex(&val);
        assert_eq!(s.plan, None);
        assert!(s.bars.is_empty());
    }

    // ── SourceUsageSummary::from_openclaw tests ─────────────────────────────

    #[test]
    fn from_openclaw_picks_highest_utilization() {
        let val = json!({
            "sessions": [
                {"percentUsed": 30.0, "model": "gpt-4"},
                {"percentUsed": 80.0, "model": "claude-opus"},
                {"percentUsed": 50.0, "model": "gpt-4"}
            ]
        });
        let s = SourceUsageSummary::from_openclaw(&val);
        assert_eq!(s.source, "openclaw");
        assert_eq!(s.bars.len(), 1);
        assert!((s.bars[0].utilization - 0.8).abs() < f64::EPSILON);
        assert!(s.bars[0].label.contains("claude-opus"));
    }

    #[test]
    fn from_openclaw_empty_sessions() {
        let val = json!({"sessions": []});
        let s = SourceUsageSummary::from_openclaw(&val);
        assert!(s.bars.is_empty());
    }

    // ── WatchState tests ────────────────────────────────────────────────────

    #[test]
    fn watch_state_lifecycle() {
        let ws = WatchState::new();
        assert!(ws.current_path().is_none());

        ws.set("/tmp/test.jsonl".into(), 1024);
        assert_eq!(ws.current_path(), Some("/tmp/test.jsonl".into()));
        assert_eq!(*ws.offset.lock().unwrap(), 1024);

        ws.clear();
        assert!(ws.current_path().is_none());
        assert_eq!(*ws.offset.lock().unwrap(), 0);
    }

    // ── NullBackend tests ───────────────────────────────────────────────────

    #[test]
    fn null_backend_returns_defaults() {
        let nb = NullBackend;
        assert!(nb.list_sessions().is_empty());
        assert!(nb.get_messages("any").is_err());
        assert!(nb.kill_pid(1).is_err());
        assert!(nb.usage_summaries().is_empty());
        assert!(nb.list_memories().is_empty());
        assert!(nb.get_waiting_alerts().is_empty());
        assert!(nb.get_sources_config().is_empty());
        let setup = nb.check_setup();
        assert!(!setup.cli_installed);
        assert!(!setup.has_sessions);
    }
}
