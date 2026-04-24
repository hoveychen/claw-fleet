//! Backend trait — abstraction over local (file-based) and remote (HTTP probe)
//! data sources.  Both `LocalBackend` and `RemoteBackend` implement this trait
//! so that all Tauri command handlers can be written as simple delegations with
//! no `if remote { … } else { … }` branching.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::account::AccountInfo;
use crate::audit::{AuditRuleInfo, AuditSummary, SuggestedRule};
use crate::daily_report::{DailyReport, DailyReportStats, Lesson};
use crate::llm_provider::{LlmConfig, LlmProviderInfo};
use crate::llm_usage::FleetLlmUsageDailyBucket;
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
    /// Originating agent source id (e.g. "claude-code", "cursor", "codex").
    /// Used by the UI to suppress audible alerts for sources whose waits are
    /// already surfaced through the Decision Panel (AskUserQuestion bridge).
    pub source: String,
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
    /// Headlessly resume a rate-limited session: spawns
    /// `claude --resume <session_id> -p "continue"` in the given workspace
    /// directory so the previous task can run to completion. Detached.
    fn resume_session(&self, session_id: String, workspace_path: String) -> Result<(), String>;
    /// Read the auto-resume scheduler config.
    fn get_auto_resume_config(&self) -> crate::auto_resume::AutoResumeConfig;
    /// Persist the auto-resume scheduler config.
    fn set_auto_resume_config(
        &self,
        config: crate::auto_resume::AutoResumeConfig,
    ) -> Result<(), String>;
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

    // ── Guard (real-time interception) ───────────────────────────────────────
    fn apply_guard_hook(&self) -> Result<(), String>;
    fn remove_guard_hook(&self) -> Result<(), String>;
    fn respond_to_guard(&self, id: &str, allow: bool) -> Result<(), String>;
    fn analyze_guard_command(&self, command: &str, context: &str, lang: &str) -> Result<String, String>;

    // ── Elicitation (AskUserQuestion interception) ──────────────────────
    fn apply_elicitation_hook(&self) -> Result<(), String>;
    fn remove_elicitation_hook(&self) -> Result<(), String>;
    fn respond_to_elicitation(
        &self,
        id: &str,
        declined: bool,
        answers: std::collections::HashMap<String, String>,
    ) -> Result<(), String>;

    // ── Plan approval (ExitPlanMode interception) ──────────────────────
    fn apply_plan_approval_hook(&self) -> Result<(), String>;
    fn remove_plan_approval_hook(&self) -> Result<(), String>;
    fn list_pending_plan_approvals(&self) -> Vec<crate::plan_approval::PlanApprovalRequest>;
    fn respond_to_plan_approval(
        &self,
        id: &str,
        decision: &str,
        edited_plan: Option<String>,
        feedback: Option<String>,
    ) -> Result<(), String>;

    // ── Interaction mode (global CLAUDE.md guidance) ────────────────────────
    fn apply_interaction_mode(&self, user_title: &str, locale: &str) -> Result<(), String>;
    fn remove_interaction_mode(&self) -> Result<(), String>;

    // ── Agent sources config ─────────────────────────────────────────────────
    fn get_sources_config(&self) -> Vec<crate::agent_source::SourceInfo>;
    fn set_source_enabled(&self, name: &str, enabled: bool) -> Result<(), String>;

    // ── Full-text search ─────────────────────────────────────────────────────
    fn search_sessions(&self, query: &str, limit: usize) -> Vec<SearchHit>;

    // ── Security audit ──────────────────────────────────────────────────────
    fn get_audit_events(&self) -> AuditSummary;
    fn get_audit_rules(&self) -> Vec<AuditRuleInfo>;
    fn set_audit_rule_enabled(&self, id: &str, enabled: bool) -> Result<(), String>;
    fn save_custom_audit_rule(&self, rule: AuditRuleInfo) -> Result<(), String>;
    fn delete_custom_audit_rule(&self, id: &str) -> Result<(), String>;
    fn suggest_audit_rules(&self, concern: &str, lang: &str) -> Result<Vec<SuggestedRule>, String>;

    // ── Daily reports ────────────────────────────────────────────────────────
    fn get_daily_report(&self, date: &str) -> Result<Option<DailyReport>, String>;
    fn list_daily_report_stats(&self, from: &str, to: &str) -> Vec<DailyReportStats>;
    fn generate_daily_report(&self, date: &str) -> Result<DailyReport, String>;
    fn generate_daily_report_ai_summary(&self, date: &str) -> Result<String, String>;
    fn generate_daily_report_lessons(&self, date: &str) -> Result<Vec<Lesson>, String>;
    fn append_lesson_to_claude_md(&self, lesson: &Lesson) -> Result<(), String>;

    // ── LLM provider ────────────────────────────────────────────────────────
    fn list_llm_providers(&self) -> Vec<LlmProviderInfo>;
    fn get_llm_config(&self) -> LlmConfig;
    fn set_llm_config(&self, config: LlmConfig) -> Result<(), String>;

    // ── Fleet-self LLM usage accounting ─────────────────────────────────────
    /// Return daily usage buckets (one per date × scenario) in [from_ms, to_ms].
    fn list_fleet_llm_usage_daily(&self, from_ms: u64, to_ms: u64)
        -> Vec<FleetLlmUsageDailyBucket>;

    // ── Decision-panel attachments ──────────────────────────────────────────
    /// Make a local file available to the agent process as an absolute path.
    ///
    /// LocalBackend returns the source path unchanged (agent runs on the same
    /// machine). RemoteBackend uploads the file bytes to the probe host and
    /// returns the server-side temp path. UI layers concatenate the returned
    /// path into the textarea as `@<path>` so Claude Code picks it up.
    fn upload_attachment(&self, source_path: &std::path::Path) -> Result<String, String>;
}

/// Upper bound on a single attachment payload. Enforced by both the uploader
/// (to fail fast) and `fleet serve` (to reject oversized POSTs). Kept small
/// enough that a full upload can reasonably live in memory.
pub const MAX_ATTACHMENT_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB

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

}
