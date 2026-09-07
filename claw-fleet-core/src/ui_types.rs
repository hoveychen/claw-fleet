//! Shared view types — the payload shapes every Fleet client renders:
//! the desktop (via `LocalBackend`), `fleet serve` / `fleet webui` routes and
//! the mobile relay all build these from the same core functions, so the
//! desktop card, the browser tab and the phone show the same thing.
//!
//! This used to also hold the `Backend` trait the desktop dispatched through
//! (with a local and an SSH-tunnelled remote implementation). The remote
//! backend is gone and the desktop calls `LocalBackend` directly, so only the
//! types stayed.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::account::AccountInfo;
use crate::session::SessionInfo;

// ── Shared types ─────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DetectedTools {
    pub cli: bool,
    pub vscode: bool,
    pub jetbrains: bool,
    pub desktop: bool,
    pub codex: bool,
}

impl Default for DetectedTools {
    fn default() -> Self {
        DetectedTools {
            cli: false,
            vscode: false,
            jetbrains: false,
            desktop: false,
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
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct WaitingAlert {
    pub session_id: String,
    pub workspace_name: String,
    pub summary: String,
    pub detected_at_ms: u64,
    pub jsonl_path: String,
    /// Originating agent source id (e.g. "claude-code", "codex").
    /// Used by the UI to suppress audible alerts for sources whose waits are
    /// already surfaced through the Decision Panel (AskUserQuestion bridge).
    pub source: String,
}

// ── Pending decisions snapshot (mount catch-up) ──────────────────────────────

/// Snapshot of every decision-panel request currently awaiting a response,
/// across all five file-IPC channels. Returned by `LocalBackend::list_pending_decisions`
/// so the frontend can pull outstanding decisions on mount instead of relying
/// solely on the one-shot watcher emit (which is lost if no Tauri listener is
/// attached at emit time — e.g. on a cold app restart while a `fleet
/// elicitation` / `fleet mcp` child process is still blocking on its poll).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PendingDecisions {
    pub guard: Vec<crate::guard::GuardRequest>,
    pub elicitation: Vec<crate::elicitation::ElicitationRequest>,
    pub fleet_ask: Vec<crate::mcp_ipc::FleetAskRequest>,
    pub a2ui_render: Vec<crate::mcp_a2ui_ipc::A2uiRenderRequest>,
    pub plan_approval: Vec<crate::plan_approval::PlanApprovalRequest>,
    /// Native permission prompts from headless sessions, routed through the
    /// `fleet__permission_prompt` MCP tool (`--permission-prompt-tool`).
    /// `#[serde(default)]` keeps older `fleet serve` probes (whose
    /// `/pending_decisions` payload predates this field) deserializable.
    #[serde(default)]
    pub permission_prompt: Vec<crate::permission_prompt_ipc::PermissionPromptRequest>,
}

/// Fill in each pending request's `workspace_name` / `ai_title` from the
/// session cache, mirroring `local_backend::resolve_session_display` so
/// mount-catch-up cards show the same workspace/title labels the live watcher
/// would have stamped. Only fills empty `workspace_name` and `None` `ai_title`
/// so a request that already carries display info is left untouched.
pub fn resolve_pending_display(pending: &mut PendingDecisions, sessions: &[SessionInfo]) {
    let lookup = |session_id: &str| -> Option<(String, Option<String>)> {
        sessions
            .iter()
            .find(|s| s.id == session_id)
            // Prefer the human/agent title override — for Codex sessions it's the
            // only real title (ai_title is the raw first prompt).
            .map(|s| {
                (
                    s.workspace_name.clone(),
                    s.title_override.clone().or_else(|| s.ai_title.clone()),
                )
            })
    };
    macro_rules! resolve_vec {
        ($v:expr) => {
            for req in $v.iter_mut() {
                if let Some((ws, ai)) = lookup(&req.session_id) {
                    if req.workspace_name.is_empty() {
                        req.workspace_name = ws;
                    }
                    if req.ai_title.is_none() {
                        req.ai_title = ai;
                    }
                }
            }
        };
    }
    resolve_vec!(pending.guard);
    resolve_vec!(pending.elicitation);
    resolve_vec!(pending.fleet_ask);
    resolve_vec!(pending.a2ui_render);
    resolve_vec!(pending.plan_approval);
    resolve_vec!(pending.permission_prompt);
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
    /// Source identifier: "claude", "codex".
    pub source: String,
    /// Plan / tier label (e.g. "Max 5x", "pro", "Plus").
    pub plan: Option<String>,
    /// Rate-limit windows, each with a utilization bar.
    pub bars: Vec<UsageBar>,
    /// Where the numbers came from — `"foxy-switcher"` when read from the local
    /// foxy daemon, else the provider's own path (`"anthropic"` /
    /// `"codex-app-server"`). `None` when the source reported nothing.
    /// `#[serde(default)]` keeps older serialized payloads deserializable.
    #[serde(default)]
    pub usage_source: Option<String>,
    /// Which account these numbers belong to, so a foxy-managed machine can
    /// name the account in use on every source, not just Claude. `None` when
    /// the source cannot tell (e.g. Codex on API-key auth, which writes no
    /// `id_token`). `#[serde(default)]` for older serialized payloads.
    #[serde(default)]
    pub email: Option<String>,
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
        for sc in &info.seven_day_scoped {
            bars.push(UsageBar {
                label: format!("7d {}", sc.model_label),
                utilization: sc.utilization,
                resets_at: Some(sc.resets_at.clone()),
            });
        }
        SourceUsageSummary {
            source: "claude".into(),
            plan: if info.plan.is_empty() { None } else { Some(info.plan.clone()) },
            bars,
            usage_source: if info.usage_source.is_empty() {
                None
            } else {
                Some(info.usage_source.clone())
            },
            email: if info.email.is_empty() { None } else { Some(info.email.clone()) },
        }
    }

    /// Convert Codex's `CodexUsageItem` JSON value into a unified summary.
    pub fn from_codex(val: &Value) -> Self {
        let plan = val["planType"].as_str().map(|s| s.to_string());
        let mut bars = Vec::new();
        let buckets = val.get("rateLimitBuckets").and_then(Value::as_array);
        if let Some(buckets) = buckets.filter(|items| !items.is_empty()) {
            for bucket in buckets {
                for slot in ["primary", "secondary"] {
                    if let Some(window) = bucket.get(slot).filter(|window| !window.is_null()) {
                        bars.push(codex_usage_bar(bucket, window, slot));
                    }
                }
            }
        } else {
            for (slot, label) in [("primary", "Primary"), ("secondary", "Secondary")] {
                if let Some(window) = val.get(slot).filter(|window| !window.is_null()) {
                    let mut bar = codex_usage_bar(val, window, slot);
                    bar.label = label.to_string();
                    bars.push(bar);
                }
            }
        }
        SourceUsageSummary {
            source: "codex".into(),
            plan,
            bars,
            usage_source: val["usageSource"].as_str().map(|s| s.to_string()),
            email: val["email"].as_str().map(|s| s.to_string()),
        }
    }
}

fn codex_usage_bar(bucket: &Value, window: &Value, slot: &str) -> UsageBar {
    let base = bucket
        .get("limitName")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .or_else(|| bucket.get("normalModelSlug").and_then(Value::as_str).filter(|value| !value.is_empty()))
        .or_else(|| bucket.get("limitId").and_then(Value::as_str).filter(|value| !value.is_empty()))
        .unwrap_or("Codex");
    let duration = window
        .get("windowDurationMins")
        .and_then(Value::as_i64)
        .filter(|mins| *mins > 0)
        .map(|mins| {
            if mins % (24 * 60) == 0 {
                format!("{}d", mins / (24 * 60))
            } else if mins % 60 == 0 {
                format!("{}h", mins / 60)
            } else {
                format!("{mins}m")
            }
        })
        .unwrap_or_else(|| if slot == "primary" { "Primary".into() } else { "Secondary".into() });
    let resets_at = window["resetsAt"].as_i64().map(|ts| {
        chrono::DateTime::from_timestamp(ts, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default()
    });
    UsageBar {
        label: format!("{base} · {duration}"),
        utilization: window["usedPercent"].as_i64().unwrap_or(0) as f64 / 100.0,
        resets_at,
    }
}

/// Upper bound on a single attachment payload. Enforced by both the uploader
/// (to fail fast) and `fleet serve` (to reject oversized POSTs). Kept small
/// enough that a full upload can reasonably live in memory.
pub const MAX_ATTACHMENT_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use crate::account::{AccountInfo, ScopedUsage, UsageStats};

    // ── SourceUsageSummary::from_claude tests ───────────────────────────────

    // ── resolve_pending_display ─────────────────────────────────────────────

    fn mk_session(id: &str, workspace_name: &str, ai_title: Option<&str>) -> SessionInfo {
        use crate::session::SessionStatus;
        SessionInfo {
            id: id.into(),
            workspace_path: "/tmp/test".into(),
            workspace_name: workspace_name.into(),
            ide_name: None,
            entrypoint: None,
            is_subagent: false,
            fleet_spawned: false,
            parent_session_id: None,
            agent_type: None,
            agent_description: None,
            slug: None,
            ai_title: ai_title.map(|s| s.to_string()),
            status: SessionStatus::Idle,
            token_speed: 0.0,
            agent_token_speed: 0.0,
            total_output_tokens: 0,
            reasoning_output_tokens: 0,
            total_input_tokens: 0,
            total_cost_usd: 0.0,
            agent_total_cost_usd: 0.0,
            cost_speed_usd_per_min: 0.0,
            last_message_preview: None,
            last_activity_ms: 0,
            agent_last_activity_ms: 0,
            running_subagent_count: 0,
            created_at_ms: 0,
            jsonl_path: format!("/tmp/{id}.jsonl"),
            model: None,
            thinking_level: None,
            effort: None,
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
            task_plan: None, handoff: None, user_mark: None, title_override: None, last_read_ms: None,            compact_count: 0,
            compact_pre_tokens: 0,
            compact_post_tokens: 0,
            compact_cost_usd: 0.0,
            pending_messages: Vec::new(),
            watches: Vec::new(),
            remote_disconnect: None,
            mirror_write: None,
        }
    }

    #[test]
    fn resolve_pending_display_fills_empty_workspace_and_title() {
        use crate::elicitation::{ElicitationQuestion, ElicitationRequest};
        let mut pending = PendingDecisions::default();
        pending.elicitation.push(ElicitationRequest {
            parked: false,
            id: "e1".into(),
            session_id: "s1".into(),
            workspace_name: String::new(),
            ai_title: None,
            questions: vec![ElicitationQuestion {
                question: "q?".into(),
                header: "h".into(),
                options: vec![],
                multi_select: false,
            }],
            timestamp: "t".into(),
        });
        let sessions = vec![mk_session("s1", "my-workspace", Some("Fix the bug"))];
        resolve_pending_display(&mut pending, &sessions);
        assert_eq!(pending.elicitation[0].workspace_name, "my-workspace");
        assert_eq!(pending.elicitation[0].ai_title.as_deref(), Some("Fix the bug"));
    }

    #[test]
    fn resolve_pending_display_prefers_title_override_over_ai_title() {
        use crate::elicitation::ElicitationRequest;
        let mut pending = PendingDecisions::default();
        pending.elicitation.push(ElicitationRequest {
            parked: false,
            id: "e1".into(),
            session_id: "s1".into(),
            workspace_name: String::new(),
            ai_title: None,
            questions: vec![],
            timestamp: "t".into(),
        });
        // Codex-like session: `ai_title` is the raw first prompt; the real,
        // human/agent-set title lives in the override. The decision card must
        // show the override, not the raw prompt.
        let mut s = mk_session("s1", "ws", Some("raw first prompt"));
        s.title_override = Some("Renamed nicely".into());
        resolve_pending_display(&mut pending, &[s]);
        assert_eq!(pending.elicitation[0].ai_title.as_deref(), Some("Renamed nicely"));
    }

    #[test]
    fn resolve_pending_display_preserves_existing_and_handles_unknown_session() {
        use crate::elicitation::ElicitationRequest;
        let mut pending = PendingDecisions::default();
        // Already-populated workspace must be preserved.
        pending.elicitation.push(ElicitationRequest {
            parked: false,
            id: "e1".into(),
            session_id: "s1".into(),
            workspace_name: "preset-ws".into(),
            ai_title: Some("preset-title".into()),
            questions: vec![],
            timestamp: "t".into(),
        });
        // Unknown session → left as-is (empty), no panic.
        pending.elicitation.push(ElicitationRequest {
            parked: false,
            id: "e2".into(),
            session_id: "missing".into(),
            workspace_name: String::new(),
            ai_title: None,
            questions: vec![],
            timestamp: "t".into(),
        });
        let sessions = vec![mk_session("s1", "lookup-ws", Some("lookup-title"))];
        resolve_pending_display(&mut pending, &sessions);
        assert_eq!(pending.elicitation[0].workspace_name, "preset-ws");
        assert_eq!(pending.elicitation[0].ai_title.as_deref(), Some("preset-title"));
        assert_eq!(pending.elicitation[1].workspace_name, "");
        assert_eq!(pending.elicitation[1].ai_title, None);
    }

    #[test]
    fn from_claude_all_windows() {
        let info = AccountInfo {
            plan: "Max 5x".into(),
            five_hour: Some(UsageStats { utilization: 0.3, resets_at: "2026-01-01T00:00:00Z".into(), prev_utilization: None }),
            seven_day: Some(UsageStats { utilization: 0.7, resets_at: "2026-01-07T00:00:00Z".into(), prev_utilization: None }),
            seven_day_scoped: vec![ScopedUsage {
                model_label: "Fable".into(),
                utilization: 0.1,
                resets_at: "2026-01-07T00:00:00Z".into(),
                prev_utilization: None,
            }],
            ..Default::default()
        };
        let s = SourceUsageSummary::from_claude(&info);
        assert_eq!(s.source, "claude");
        assert_eq!(s.plan, Some("Max 5x".into()));
        assert_eq!(s.bars.len(), 3);
        assert_eq!(s.bars[0].label, "5h");
        assert!((s.bars[0].utilization - 0.3).abs() < f64::EPSILON);
        assert_eq!(s.bars[1].label, "7d Opus");
        assert_eq!(s.bars[2].label, "7d Fable");
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
    fn from_codex_flattens_named_dynamic_buckets() {
        let val = json!({
            "planType": "plus",
            "rateLimitBuckets": [
                {"limitId": "codex", "primary": {"usedPercent": 12, "windowDurationMins": 300}},
                {"limitId": "base_model_inference", "limitName": "Luna Reserve",
                 "primary": {"usedPercent": 48, "windowDurationMins": 10080}}
            ]
        });
        let summary = SourceUsageSummary::from_codex(&val);
        assert_eq!(summary.bars.len(), 2);
        assert_eq!(summary.bars[0].label, "codex · 5h");
        assert_eq!(summary.bars[1].label, "Luna Reserve · 7d");
        assert!((summary.bars[1].utilization - 0.48).abs() < f64::EPSILON);
    }

    #[test]
    fn from_codex_carries_the_usage_source_through() {
        // The normalised summary feeds the tray menu and the mobile usage view;
        // dropping the label there would leave those two surfaces unable to say
        // whether the numbers came from foxy.
        let val = json!({
            "planType": "team",
            "primary": {"usedPercent": 1, "resetsAt": 1_787_622_736_i64},
            "usageSource": "foxy-switcher"
        });
        let s = SourceUsageSummary::from_codex(&val);
        assert_eq!(s.usage_source.as_deref(), Some("foxy-switcher"));
    }

    #[test]
    fn from_codex_without_a_usage_source_reports_none() {
        // An older backend (or a payload predating the field) must not surface
        // an empty-string source the UI would try to translate.
        let s = SourceUsageSummary::from_codex(&json!({ "planType": "team" }));
        assert_eq!(s.usage_source, None);
    }

    #[test]
    fn from_claude_carries_the_usage_source_through() {
        let info = AccountInfo {
            usage_source: "foxy-switcher".into(),
            ..AccountInfo::default()
        };
        let s = SourceUsageSummary::from_claude(&info);
        assert_eq!(s.usage_source.as_deref(), Some("foxy-switcher"));
    }

    #[test]
    fn from_claude_empty_usage_source_reports_none() {
        let s = SourceUsageSummary::from_claude(&AccountInfo::default());
        assert_eq!(s.usage_source, None);
    }

    #[test]
    fn from_codex_missing_plan() {
        let val = json!({});
        let s = SourceUsageSummary::from_codex(&val);
        assert_eq!(s.plan, None);
        assert!(s.bars.is_empty());
    }

}
