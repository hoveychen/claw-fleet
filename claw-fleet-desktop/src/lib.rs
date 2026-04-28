// Re-export everything from core so that `crate::session`, `crate::backend`,
// `crate::pattern_update`, etc. keep working in desktop-only modules that
// were written with `use crate::…` / `use super::…` paths.
pub use claw_fleet_core::*;

// ── Desktop-only modules ────────────────────────────────────────────────────
// These are always compiled — this crate IS the GUI app, so no #[cfg] gates.
pub mod app_nap;
pub mod embedded_server;
mod gui;
pub mod local_backend;
pub mod region;
pub mod remote;
pub mod tunnel;
pub mod version_check;

pub use gui::*;

// ── Desktop-only backend helpers ────────────────────────────────────────────
// WatchState and NullBackend were previously behind #[cfg(feature = "gui")]
// in the monolithic lib.  They live here now because they are only needed by
// the desktop app (LocalBackend, RemoteBackend, and the Tauri setup code).

use std::sync::Mutex;

use audit::AuditSummary;
use backend::{
    AccountInfoFuture, Backend, DetectedTools, SetupStatus, SourceDataFuture,
    SourceUsageSummary, WaitingAlert,
};
use daily_report::{DailyReport, DailyReportStats, Lesson};
use llm_provider::{LlmConfig, LlmProviderInfo};
use memory::{MemoryHistoryEntry, WorkspaceMemory};
use search_index::SearchHit;
use skills::{SkillFileEntry, SkillItem};
use serde_json::Value;

/// Tracks which session file is currently being watched (tailed) for live
/// updates.  Used by both `LocalBackend` and `RemoteBackend`.
pub struct WatchState {
    pub session: Mutex<Option<String>>,
    pub offset: Mutex<u64>,
}

impl WatchState {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(None),
            offset: Mutex::new(0),
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
pub struct NullBackend;

impl Backend for NullBackend {
    fn list_sessions(&self) -> Vec<session::SessionInfo> {
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
    fn resume_session(&self, _: String, _: String) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn get_auto_resume_config(&self) -> claw_fleet_core::auto_resume::AutoResumeConfig {
        claw_fleet_core::auto_resume::AutoResumeConfig::default()
    }
    fn set_auto_resume_config(
        &self,
        _: claw_fleet_core::auto_resume::AutoResumeConfig,
    ) -> Result<(), String> {
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
    fn list_skill_files(&self, _: &str) -> Result<Vec<SkillFileEntry>, String> {
        Err("backend not ready".into())
    }
    fn get_skill_history(
        &self,
        _: &str,
    ) -> Result<Vec<claw_fleet_core::skill_history::SkillInvocation>, String> {
        Err("backend not ready".into())
    }
    fn get_waiting_alerts(&self) -> Vec<WaitingAlert> {
        vec![]
    }
    fn get_hooks_plan(&self) -> hooks::HookSetupPlan {
        hooks::HookSetupPlan {
            to_add: vec![],
            hooks_globally_disabled: false,
            already_installed: true,
            guard_installed: false,
            elicitation_installed: false,
            interaction_mode_installed: false,
            plan_approval_installed: false,
        }
    }
    fn apply_hooks(&self) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn remove_hooks(&self) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn apply_guard_hook(&self) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn remove_guard_hook(&self) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn respond_to_guard(&self, _: &str, _: bool) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn analyze_guard_command(&self, _: &str, _: &str, _: &str) -> Result<String, String> {
        Err("backend not ready".into())
    }
    fn apply_elicitation_hook(&self) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn remove_elicitation_hook(&self) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn respond_to_elicitation(
        &self,
        _: &str,
        _: bool,
        _: std::collections::HashMap<String, String>,
    ) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn apply_plan_approval_hook(&self) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn remove_plan_approval_hook(&self) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn list_pending_plan_approvals(
        &self,
    ) -> Vec<claw_fleet_core::plan_approval::PlanApprovalRequest> {
        vec![]
    }
    fn respond_to_plan_approval(
        &self,
        _: &str,
        _: &str,
        _: Option<String>,
        _: Option<String>,
    ) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn list_session_decisions(
        &self,
        _: &str,
        _: Option<&str>,
    ) -> Vec<claw_fleet_core::decision_history::DecisionHistoryRecord> {
        vec![]
    }
    fn apply_interaction_mode(&self, _: &str, _: &str) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn remove_interaction_mode(&self) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn get_sources_config(&self) -> Vec<agent_source::SourceInfo> {
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
    fn get_audit_rules(&self) -> Vec<audit::AuditRuleInfo> {
        vec![]
    }
    fn set_audit_rule_enabled(&self, _: &str, _: bool) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn save_custom_audit_rule(&self, _: audit::AuditRuleInfo) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn delete_custom_audit_rule(&self, _: &str) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn suggest_audit_rules(&self, _: &str, _: &str) -> Result<Vec<audit::SuggestedRule>, String> {
        Err("backend not ready".into())
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
    fn list_llm_providers(&self) -> Vec<LlmProviderInfo> {
        vec![]
    }
    fn get_llm_config(&self) -> LlmConfig {
        LlmConfig::default()
    }
    fn set_llm_config(&self, _: LlmConfig) -> Result<(), String> {
        Err("backend not ready".into())
    }
    fn list_fleet_llm_usage_daily(
        &self,
        _: u64,
        _: u64,
    ) -> Vec<llm_usage::FleetLlmUsageDailyBucket> {
        vec![]
    }
    fn upload_attachment(&self, _: &std::path::Path) -> Result<String, String> {
        Err("backend not ready".into())
    }
}

// ── Desktop-only pattern update helpers ─────────────────────────────────────
// These use `tauri::AppHandle` to resolve bundled resources, so they cannot
// live in the platform-agnostic core crate.

pub mod desktop_pattern_update {
    use crate::audit::{self, ExternalPatternsFile};

    /// Read the version from the bundled resource file.  Returns 0 if absent.
    fn bundled_version(app_handle: &tauri::AppHandle) -> u32 {
        use tauri::Manager;
        app_handle
            .path()
            .resolve(
                "resources/audit-patterns.json",
                tauri::path::BaseDirectory::Resource,
            )
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<ExternalPatternsFile>(&s).ok())
            .map(|f| f.version)
            .unwrap_or(0)
    }

    /// On first run (no local file), copy the bundled resource to
    /// `~/.fleet/fleet-audit-patterns.json` so the audit module has something
    /// to load without waiting for the first remote check.
    ///
    /// Also upgrades the local file if the bundled version is newer (happens
    /// after an app upgrade ships new built-in patterns).
    pub fn bootstrap_patterns(app_handle: &tauri::AppHandle) {
        use tauri::Manager;
        let Some(local) = crate::session::real_home_dir()
            .map(|h| h.join(".fleet").join("fleet-audit-patterns.json"))
        else {
            return;
        };
        if local.exists() {
            // Already have a local file.  Check if the bundled version is newer.
            let lv = local_version(&local);
            let bv = bundled_version(app_handle);
            if bv > lv {
                if let Ok(bundled_path) = app_handle
                    .path()
                    .resolve(
                        "resources/audit-patterns.json",
                        tauri::path::BaseDirectory::Resource,
                    )
                {
                    if let Ok(content) = std::fs::read_to_string(&bundled_path) {
                        let _ = atomic_write(&local, &content);
                        audit::reload_patterns();
                        crate::log_debug(&format!(
                            "pattern_update: upgraded local patterns v{lv} → v{bv} from bundled resource"
                        ));
                    }
                }
            }
            return;
        }
        // No local file — seed from bundled resource.
        if let Ok(bundled_path) = app_handle
            .path()
            .resolve(
                "resources/audit-patterns.json",
                tauri::path::BaseDirectory::Resource,
            )
        {
            if let Ok(content) = std::fs::read_to_string(&bundled_path) {
                let _ = atomic_write(&local, &content);
                crate::log_debug(
                    "pattern_update: seeded local patterns from bundled resource",
                );
            }
        }
    }

    /// Read the version from a local JSON file.  Returns 0 if absent / unparseable.
    fn local_version(path: &std::path::Path) -> u32 {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<ExternalPatternsFile>(&s).ok())
            .map(|f| f.version)
            .unwrap_or(0)
    }

    /// Atomic file write: write to temp, then rename.
    fn atomic_write(target: &std::path::Path, content: &str) -> Result<(), String> {
        let dir = target.parent().ok_or("no parent dir")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir: {e}"))?;
        let tmp = dir.join(format!(
            ".fleet-audit-patterns-{}.tmp",
            std::process::id()
        ));
        std::fs::write(&tmp, content).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("write tmp: {e}")
        })?;
        std::fs::rename(&tmp, target).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("rename: {e}")
        })?;
        Ok(())
    }
}
