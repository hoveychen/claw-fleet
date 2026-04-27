/// remote.rs — SSH-based remote Fleet probe connection manager.
///
/// Flow:
///  1.  App calls `connect_remote(conn)`.
///  2.  We SSH to remote, check/upload the fleet-cli binary, start `fleet serve`.
///  3.  We fork a local `ssh -N -L …` tunnel process.
///  4.  We poll `localhost:<port>/health` until the probe is ready.
///  5.  We start a background thread that polls `/sessions` every second
///      and emits `sessions-updated` Tauri events — identical to the local
///      file-watcher, so the frontend needs zero changes.
///  6.  `start_watching_session` / `stop_watching_session` poll `/tail` for
///      incremental message delivery via `session-tail` events.
///  7.  `disconnect_remote` kills the tunnel, sends a kill-probe SSH command,
///      and tears down the poller/tail threads.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::session::SessionInfo;

// ── Saved-connection record ───────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConnection {
    pub id: String,
    pub label: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub identity_file: Option<String>,
    /// Optional jump/bastion host (SSH ProxyJump, e.g. "user@bastion:22").
    pub jump_host: Option<String>,
    /// If set, use this SSH config profile name instead of manual host/user/port/key.
    pub ssh_profile: Option<String>,
}

// ── ProbeClient ──────────────────────────────────────────────────────────────

/// Reusable HTTP client for communicating with a remote Fleet probe.
/// Encapsulates base URL, auth token, and a shared `reqwest` client so that
/// every remote call is a one-liner instead of ~15 lines of boilerplate.
#[derive(Clone)]
pub(crate) struct ProbeClient {
    base_url: String,
    auth_header: String,
    client: reqwest::blocking::Client,
}

impl ProbeClient {
    fn new(base_url: String, token: &str) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();
        Self {
            base_url,
            auth_header: format!("Bearer {}", token),
            client,
        }
    }

    /// GET endpoint and deserialize the JSON response body.
    fn get<T: serde::de::DeserializeOwned>(&self, endpoint: &str) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, endpoint);
        let resp = self.client
            .get(&url)
            .header("Authorization", &self.auth_header)
            .send()
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }
        resp.json::<T>().map_err(|e| e.to_string())
    }

    /// GET endpoint, only check that the status is 2xx.
    fn get_ok(&self, endpoint: &str) -> Result<(), String> {
        let url = format!("{}{}", self.base_url, endpoint);
        let resp = self.client
            .get(&url)
            .header("Authorization", &self.auth_header)
            .send()
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }
        Ok(())
    }

    /// POST endpoint.  On failure, tries to extract `{"error":"…"}` from the
    /// response body for a better error message.
    fn post_ok(&self, endpoint: &str) -> Result<(), String> {
        let url = format!("{}{}", self.base_url, endpoint);
        let resp = self.client
            .post(&url)
            .header("Authorization", &self.auth_header)
            .send()
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            let body = resp.text().unwrap_or_default();
            let err = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["error"].as_str().map(|s| s.to_string()))
                .unwrap_or(body);
            return Err(err);
        }
        Ok(())
    }

    /// POST endpoint with a JSON body, only check that the status is 2xx.
    fn post_json_ok<B: serde::Serialize>(&self, endpoint: &str, body: &B) -> Result<(), String> {
        let url = format!("{}{}", self.base_url, endpoint);
        let resp = self.client
            .post(&url)
            .header("Authorization", &self.auth_header)
            .json(body)
            .send()
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            let body_text = resp.text().unwrap_or_default();
            let err = serde_json::from_str::<serde_json::Value>(&body_text)
                .ok()
                .and_then(|v| v["error"].as_str().map(|s| s.to_string()))
                .unwrap_or(body_text);
            return Err(err);
        }
        Ok(())
    }

    /// POST endpoint with a JSON body, deserialize the JSON response.
    fn post_json<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        endpoint: &str,
        body: &B,
    ) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, endpoint);
        let resp = self.client
            .post(&url)
            .header("Authorization", &self.auth_header)
            .timeout(Duration::from_secs(90))
            .json(body)
            .send()
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            let body_text = resp.text().unwrap_or_default();
            let err = serde_json::from_str::<serde_json::Value>(&body_text)
                .ok()
                .and_then(|v| v["error"].as_str().map(|s| s.to_string()))
                .unwrap_or(body_text);
            return Err(err);
        }
        resp.json::<T>().map_err(|e| e.to_string())
    }

    /// GET endpoint and return the raw `serde_json::Value`.
    fn get_value(&self, endpoint: &str) -> Result<serde_json::Value, String> {
        self.get::<serde_json::Value>(endpoint)
    }

    /// POST raw bytes (e.g. a file upload) and deserialize the JSON response.
    fn post_bytes<T: serde::de::DeserializeOwned>(
        &self,
        endpoint: &str,
        body: Vec<u8>,
    ) -> Result<T, String> {
        let url = format!("{}{}", self.base_url, endpoint);
        let resp = self.client
            .post(&url)
            .header("Authorization", &self.auth_header)
            .header("Content-Type", "application/octet-stream")
            .timeout(Duration::from_secs(120))
            .body(body)
            .send()
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            let body_text = resp.text().unwrap_or_default();
            let err = serde_json::from_str::<serde_json::Value>(&body_text)
                .ok()
                .and_then(|v| v["error"].as_str().map(|s| s.to_string()))
                .unwrap_or(body_text);
            return Err(err);
        }
        resp.json::<T>().map_err(|e| e.to_string())
    }
}

// ── RemoteBackend ─────────────────────────────────────────────────────────────

/// Active remote connection.  Implements [`crate::backend::Backend`] so that
/// all Tauri command handlers can delegate without if/else branching.
pub struct RemoteBackend {
    // Connection metadata (needed for Drop to reach the remote probe).
    connection: RemoteConnection,
    /// HTTP client for the probe.
    probe: ProbeClient,
    /// Local `ssh -N -L …` tunnel child process.
    tunnel_child: std::process::Child,
    /// PID of `fleet serve` on the remote host.
    remote_probe_pid: Option<u32>,
    /// Set to `false` to stop the sessions-poller thread.
    poller_running: Arc<Mutex<bool>>,
    /// Set to `false` to stop the tail-poller thread.
    tail_running: Arc<Mutex<bool>>,
    // Backend state
    app: tauri::AppHandle,
    sessions: Arc<Mutex<Vec<crate::session::SessionInfo>>>,
    watch: Arc<crate::WatchState>,
    /// Active waiting-input alerts, keyed by session ID.
    waiting_alerts: Arc<Mutex<std::collections::HashMap<String, crate::backend::WaitingAlert>>>,
    /// Semantic outcome tags per session, set by background analysis.
    #[allow(dead_code)]
    session_outcomes: Arc<Mutex<std::collections::HashMap<String, Vec<String>>>>,
}

impl Drop for RemoteBackend {
    fn drop(&mut self) {
        *self.poller_running.lock().unwrap() = false;
        *self.tail_running.lock().unwrap() = false;
        let _ = self.tunnel_child.kill();
        // Best-effort: kill the remote fleet-serve process by the PID we captured
        // when we started it.  (We no longer use pkill-by-port because the port is
        // randomly OS-assigned per session; pkill-by-name would also kill other
        // users' sessions sharing this host.)
        if let Some(pid) = self.remote_probe_pid {
            let _ = ssh_exec(&self.connection, &format!("kill {} 2>/dev/null", pid));
        }
    }
}

/// Encode a path for use in a query parameter.
fn encode_path(path: &str) -> String {
    utf8_percent_encode(path, NON_ALPHANUMERIC).to_string()
}

impl crate::backend::Backend for RemoteBackend {
    fn list_sessions(&self) -> Vec<crate::session::SessionInfo> {
        self.sessions.lock().unwrap().clone()
    }

    fn get_messages(&self, path: &str) -> Result<Vec<serde_json::Value>, String> {
        self.probe.get(&format!("/messages?path={}", encode_path(path)))
    }

    fn get_messages_tail(&self, path: &str, n: usize) -> Result<Vec<serde_json::Value>, String> {
        self.probe.get(&format!(
            "/messages?path={}&tail={}",
            encode_path(path),
            n
        ))
    }

    fn kill_pid(&self, pid: u32) -> Result<(), String> {
        self.probe.get_ok(&format!("/stop?pid={}&force=false", pid))
    }

    fn kill_workspace(&self, workspace_path: String) -> Result<(), String> {
        let encoded = workspace_path.replace('/', "%2F");
        self.probe.get_ok(&format!("/stop_workspace?path={}", encoded))
    }

    fn resume_session(&self, session_id: String, workspace_path: String) -> Result<(), String> {
        let sid = session_id.replace('/', "%2F");
        let wp = workspace_path.replace('/', "%2F");
        self.probe.get_ok(&format!(
            "/resume_session?session_id={}&workspace_path={}",
            sid, wp
        ))
    }

    fn get_auto_resume_config(&self) -> claw_fleet_core::auto_resume::AutoResumeConfig {
        self.probe
            .get::<claw_fleet_core::auto_resume::AutoResumeConfig>("/auto_resume_config")
            .unwrap_or_default()
    }

    fn set_auto_resume_config(
        &self,
        config: claw_fleet_core::auto_resume::AutoResumeConfig,
    ) -> Result<(), String> {
        self.probe.post_json_ok("/auto_resume_config", &config)
    }

    fn account_info(&self) -> crate::backend::AccountInfoFuture {
        let probe = self.probe.clone();
        Box::pin(async move {
            probe.get("/sources/claude/account")
        })
    }

    fn source_account(&self, source: &str) -> crate::backend::SourceDataFuture {
        let config = crate::agent_source::SourcesConfig::load();
        if !config.is_source_enabled(source) {
            let msg = format!("Source '{}' is disabled", source);
            return Box::pin(async move { Err(msg) });
        }
        let probe = self.probe.clone();
        let endpoint = format!("/sources/{}/account", source);
        Box::pin(async move {
            probe.get(&endpoint)
        })
    }

    fn source_usage(&self, source: &str) -> crate::backend::SourceDataFuture {
        let config = crate::agent_source::SourcesConfig::load();
        if !config.is_source_enabled(source) {
            let msg = format!("Source '{}' is disabled", source);
            return Box::pin(async move { Err(msg) });
        }
        let probe = self.probe.clone();
        let endpoint = format!("/sources/{}/usage", source);
        Box::pin(async move {
            probe.get(&endpoint)
        })
    }

    fn check_setup(&self) -> crate::backend::SetupStatus {
        self.probe.get::<crate::backend::SetupStatus>("/setup-status")
            .unwrap_or_else(|e| {
                crate::log_debug(&format!("remote check_setup failed: {e}"));
                crate::backend::SetupStatus {
                    cli_installed: false,
                    cli_path: None,
                    claude_dir_exists: false,
                    detected_tools: crate::backend::DetectedTools::default(),
                    logged_in: false,
                    has_sessions: !self.sessions.lock().unwrap().is_empty(),
                    credentials_valid: None,
                }
            })
    }

    fn usage_summaries(&self) -> Vec<crate::backend::SourceUsageSummary> {
        self.probe.get("/usage_summaries").unwrap_or_default()
    }

    fn start_watch(&self, path: String) -> Result<u64, String> {
        let file_size = self.probe.get_value(&format!("/file_size?path={}", encode_path(&path)))
            .ok()
            .and_then(|v| v["size"].as_u64())
            .unwrap_or(0);
        *self.tail_running.lock().unwrap() = true;
        start_remote_tail(
            self.probe.clone(),
            path.clone(),
            file_size,
            self.app.clone(),
            self.tail_running.clone(),
        );
        self.watch.set(path, file_size);
        Ok(file_size)
    }

    fn stop_watch(&self) {
        *self.tail_running.lock().unwrap() = false;
        self.watch.clear();
    }

    fn list_memories(&self) -> Vec<crate::memory::WorkspaceMemory> {
        self.probe.get("/memories").unwrap_or_default()
    }

    fn get_memory_content(&self, path: &str) -> Result<String, String> {
        self.probe.get(&format!("/memory_content?path={}", encode_path(path)))
    }

    fn get_memory_history(&self, path: &str) -> Vec<crate::memory::MemoryHistoryEntry> {
        self.probe.get(&format!("/memory_history?path={}", encode_path(path))).unwrap_or_default()
    }

    fn list_skills(&self) -> Vec<crate::skills::SkillItem> {
        self.probe.get("/skills").unwrap_or_default()
    }

    fn get_skill_content(&self, path: &str) -> Result<String, String> {
        self.probe.get(&format!("/skill_content?path={}", encode_path(path)))
    }

    fn get_waiting_alerts(&self) -> Vec<crate::backend::WaitingAlert> {
        self.waiting_alerts.lock().unwrap().values().cloned().collect()
    }

    fn get_hooks_plan(&self) -> crate::hooks::HookSetupPlan {
        self.probe.get("/hooks_plan").unwrap_or(crate::hooks::HookSetupPlan {
            to_add: vec![],
            hooks_globally_disabled: false,
            already_installed: true,
            guard_installed: false,
            elicitation_installed: false,
            interaction_mode_installed: false,
            plan_approval_installed: false,
        })
    }

    fn apply_hooks(&self) -> Result<(), String> {
        self.probe.post_ok("/apply_hooks")
    }

    fn remove_hooks(&self) -> Result<(), String> {
        self.probe.post_ok("/remove_hooks")
    }

    fn apply_guard_hook(&self) -> Result<(), String> {
        self.probe.post_ok("/apply_guard_hook")
    }

    fn remove_guard_hook(&self) -> Result<(), String> {
        self.probe.post_ok("/remove_guard_hook")
    }

    fn respond_to_guard(&self, id: &str, allow: bool) -> Result<(), String> {
        use claw_fleet_core::guard::{GuardDecision, GuardResponse};
        let resp = GuardResponse {
            id: id.to_string(),
            decision: if allow { GuardDecision::Allow } else { GuardDecision::Block },
        };
        self.probe.post_json_ok("/guard/respond", &resp)
    }

    fn analyze_guard_command(&self, command: &str, context: &str, lang: &str) -> Result<String, String> {
        #[derive(serde::Serialize)]
        struct Req<'a> { command: &'a str, context: &'a str, lang: &'a str }
        #[derive(serde::Deserialize)]
        struct Resp { analysis: String }
        let resp: Resp = self.probe.post_json("/guard/analyze", &Req { command, context, lang })?;
        Ok(resp.analysis)
    }

    fn apply_elicitation_hook(&self) -> Result<(), String> {
        self.probe.post_ok("/apply_elicitation_hook")
    }

    fn remove_elicitation_hook(&self) -> Result<(), String> {
        self.probe.post_ok("/remove_elicitation_hook")
    }

    fn respond_to_elicitation(
        &self,
        id: &str,
        declined: bool,
        answers: std::collections::HashMap<String, String>,
    ) -> Result<(), String> {
        let resp = claw_fleet_core::elicitation::ElicitationResponse {
            id: id.to_string(),
            declined,
            answers,
        };
        self.probe.post_json_ok("/elicitation/respond", &resp)
    }

    fn apply_plan_approval_hook(&self) -> Result<(), String> {
        self.probe.post_ok("/apply_plan_approval_hook")
    }

    fn remove_plan_approval_hook(&self) -> Result<(), String> {
        self.probe.post_ok("/remove_plan_approval_hook")
    }

    fn list_pending_plan_approvals(&self) -> Vec<claw_fleet_core::plan_approval::PlanApprovalRequest> {
        self.probe.get("/plan-approval/pending").unwrap_or_default()
    }

    fn respond_to_plan_approval(
        &self,
        id: &str,
        decision: &str,
        edited_plan: Option<String>,
        feedback: Option<String>,
    ) -> Result<(), String> {
        let resp = claw_fleet_core::plan_approval::PlanApprovalResponse {
            id: id.to_string(),
            decision: decision.to_string(),
            edited_plan,
            feedback,
        };
        self.probe.post_json_ok("/plan-approval/respond", &resp)
    }

    fn apply_interaction_mode(&self, user_title: &str, locale: &str) -> Result<(), String> {
        #[derive(serde::Serialize)]
        struct Req<'a> { user_title: &'a str, locale: &'a str }
        self.probe.post_json_ok("/apply_interaction_mode", &Req { user_title, locale })
    }

    fn remove_interaction_mode(&self) -> Result<(), String> {
        self.probe.post_ok("/remove_interaction_mode")
    }

    fn get_sources_config(&self) -> Vec<crate::agent_source::SourceInfo> {
        self.probe.get("/sources_config").unwrap_or_default()
    }

    fn set_source_enabled(&self, name: &str, enabled: bool) -> Result<(), String> {
        self.probe.post_ok(&format!(
            "/set_source_enabled?name={}&enabled={}",
            name, enabled
        ))
    }

    fn search_sessions(&self, query: &str, limit: usize) -> Vec<crate::search_index::SearchHit> {
        let encoded_q = percent_encoding::utf8_percent_encode(query, percent_encoding::NON_ALPHANUMERIC).to_string();
        self.probe
            .get(&format!("/search?q={}&limit={}", encoded_q, limit))
            .unwrap_or_default()
    }

    fn get_audit_events(&self) -> crate::audit::AuditSummary {
        self.probe
            .get("/audit")
            .unwrap_or_else(|_| crate::audit::AuditSummary {
                events: vec![],
                total_sessions_scanned: 0,
            })
    }

    fn get_audit_rules(&self) -> Vec<crate::audit::AuditRuleInfo> {
        self.probe.get("/audit/rules").unwrap_or_default()
    }

    fn set_audit_rule_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        #[derive(serde::Serialize)]
        struct Body { id: String, enabled: bool }
        self.probe.post_json_ok("/audit/rules/toggle", &Body { id: id.to_string(), enabled })
    }

    fn save_custom_audit_rule(&self, rule: crate::audit::AuditRuleInfo) -> Result<(), String> {
        self.probe.post_json_ok("/audit/rules/save", &rule)
    }

    fn delete_custom_audit_rule(&self, id: &str) -> Result<(), String> {
        #[derive(serde::Serialize)]
        struct Body { id: String }
        self.probe.post_json_ok("/audit/rules/delete", &Body { id: id.to_string() })
    }

    fn suggest_audit_rules(&self, concern: &str, lang: &str) -> Result<Vec<crate::audit::SuggestedRule>, String> {
        #[derive(serde::Serialize)]
        struct Body { concern: String, lang: String }
        self.probe.post_json("/audit/rules/suggest", &Body { concern: concern.to_string(), lang: lang.to_string() })
    }

    fn get_daily_report(&self, date: &str) -> Result<Option<crate::daily_report::DailyReport>, String> {
        let encoded = encode_path(date);
        self.probe.get(&format!("/daily_report?date={}", encoded))
    }

    fn list_daily_report_stats(&self, from: &str, to: &str) -> Vec<crate::daily_report::DailyReportStats> {
        let encoded_from = encode_path(from);
        let encoded_to = encode_path(to);
        self.probe
            .get(&format!("/daily_report_stats?from={}&to={}", encoded_from, encoded_to))
            .unwrap_or_default()
    }

    fn generate_daily_report(&self, date: &str) -> Result<crate::daily_report::DailyReport, String> {
        self.probe.get(&format!("/daily_report/generate?date={}", encode_path(date)))
    }

    fn generate_daily_report_ai_summary(&self, date: &str) -> Result<String, String> {
        self.probe.get(&format!("/daily_report/ai_summary?date={}", encode_path(date)))
    }

    fn generate_daily_report_lessons(&self, date: &str) -> Result<Vec<crate::daily_report::Lesson>, String> {
        self.probe.get(&format!("/daily_report/lessons?date={}", encode_path(date)))
    }

    fn append_lesson_to_claude_md(&self, lesson: &crate::daily_report::Lesson) -> Result<(), String> {
        self.probe.post_json_ok("/daily_report/append_lesson", lesson)
    }

    fn list_llm_providers(&self) -> Vec<crate::llm_provider::LlmProviderInfo> {
        self.probe.get("/llm/providers").unwrap_or_default()
    }

    fn get_llm_config(&self) -> crate::llm_provider::LlmConfig {
        self.probe.get("/llm/config").unwrap_or_default()
    }

    fn set_llm_config(&self, config: crate::llm_provider::LlmConfig) -> Result<(), String> {
        self.probe.post_json_ok("/llm/config", &config)
    }

    fn list_fleet_llm_usage_daily(
        &self,
        from_ms: u64,
        to_ms: u64,
    ) -> Vec<crate::llm_usage::FleetLlmUsageDailyBucket> {
        self.probe
            .get(&format!(
                "/fleet_llm_usage/daily?from_ms={from_ms}&to_ms={to_ms}"
            ))
            .unwrap_or_default()
    }

    fn upload_attachment(&self, source_path: &std::path::Path) -> Result<String, String> {
        let meta = std::fs::metadata(source_path).map_err(|e| e.to_string())?;
        if meta.len() > claw_fleet_core::backend::MAX_ATTACHMENT_BYTES {
            return Err(format!(
                "attachment too large: {} bytes (max {})",
                meta.len(),
                claw_fleet_core::backend::MAX_ATTACHMENT_BYTES
            ));
        }
        let bytes = std::fs::read(source_path).map_err(|e| e.to_string())?;
        let name = source_path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment.bin".to_string());
        let encoded = utf8_percent_encode(&name, NON_ALPHANUMERIC).to_string();
        #[derive(serde::Deserialize)]
        struct Resp { path: String }
        let resp: Resp = self.probe.post_bytes(
            &format!("/elicitation/upload?name={encoded}"),
            bytes,
        )?;
        Ok(resp.path)
    }

    fn start_feishu_oauth(&self) -> Result<claw_fleet_core::feishu::OauthHandle, String> {
        self.probe.get("/feishu/start_oauth")
    }
    fn poll_feishu_oauth(
        &self,
        state: &str,
    ) -> Result<claw_fleet_core::feishu::OauthStatus, String> {
        self.probe.get(&format!("/feishu/poll_oauth?state={}", encode_path(state)))
    }
    fn feishu_status(&self) -> Result<claw_fleet_core::feishu::FeishuConnection, String> {
        self.probe.get("/feishu/status")
    }
    fn disconnect_feishu(&self) -> Result<(), String> {
        self.probe.post_ok("/feishu/disconnect")
    }
    fn get_feishu_creds(&self) -> Result<claw_fleet_core::feishu::StoredCreds, String> {
        self.probe.get("/feishu/creds")
    }
    fn set_feishu_creds(
        &self,
        creds: claw_fleet_core::feishu::StoredCreds,
    ) -> Result<(), String> {
        self.probe.post_json_ok("/feishu/creds", &creds)
    }
}

// ── Progress event emitted to the frontend during connect ────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConnectProgress {
    pub step: String,
    pub done: bool,
    pub error: Option<String>,
    /// When true, the frontend should replace the last progress entry instead of appending.
    pub update_last: bool,
}

fn emit_progress(app: &AppHandle, step: &str, done: bool, error: Option<&str>) {
    let _ = app.emit(
        "remote-connect-progress",
        ConnectProgress {
            step: step.to_string(),
            done,
            error: error.map(|s| s.to_string()),
            update_last: false,
        },
    );
}

// ── Saved connections persistence ────────────────────────────────────────────

fn connections_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("fleet-connections.json"))
}

pub fn load_saved_connections() -> Vec<RemoteConnection> {
    let path = match connections_path() {
        Some(p) => p,
        None => return vec![],
    };
    let Ok(data) = std::fs::read_to_string(&path) else {
        return vec![];
    };
    serde_json::from_str(&data).unwrap_or_default()
}

fn save_connections(conns: &[RemoteConnection]) -> Result<(), String> {
    let path = connections_path().ok_or("cannot determine home dir")?;
    let data = serde_json::to_string_pretty(conns).map_err(|e| e.to_string())?;
    std::fs::write(&path, data).map_err(|e| e.to_string())
}

// ── Tauri commands — saved connections ───────────────────────────────────────

#[tauri::command]
pub fn list_saved_connections() -> Vec<RemoteConnection> {
    load_saved_connections()
}

#[tauri::command]
pub fn delete_connection(id: String) -> Result<(), String> {
    let mut conns = load_saved_connections();
    conns.retain(|c| c.id != id);
    save_connections(&conns)
}

// ── SSH helpers ───────────────────────────────────────────────────────────────

/// Apply the real HOME environment to an SSH/SCP `Command` so that the child
/// process finds `~/.ssh/config` at the true user home, not inside the macOS
/// sandbox container (`~/Library/Containers/<id>/Data/`).
fn apply_real_home(cmd: &mut std::process::Command) {
    if let Some(home) = crate::session::real_home_dir() {
        cmd.env("HOME", home);
    }
}

/// Build common SSH CLI arguments (no command, no target host yet).
fn base_ssh_args(conn: &RemoteConnection) -> Vec<String> {
    let mut args = vec![
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        "-o".to_string(),
        "ConnectTimeout=15".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
    ];
    if let Some(ref profile) = conn.ssh_profile {
        // Use SSH config profile directly — the profile resolves host/user/port/key
        args.push(profile.clone());
    } else {
        args.push("-p".to_string());
        args.push(conn.port.to_string());
        if let Some(ref key) = conn.identity_file {
            args.push("-i".to_string());
            args.push(key.clone());
        }
        if let Some(ref jump) = conn.jump_host {
            args.push("-J".to_string());
            args.push(jump.clone());
        }
        args.push(format!("{}@{}", conn.username, conn.host));
    }
    args
}

/// Build SCP arguments targeting the remote host.
fn base_scp_args(conn: &RemoteConnection) -> Vec<String> {
    let mut args = vec![
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        "-o".to_string(),
        "ConnectTimeout=30".to_string(),
    ];
    if conn.ssh_profile.is_none() {
        args.push("-P".to_string());
        args.push(conn.port.to_string());
        if let Some(ref key) = conn.identity_file {
            args.push("-i".to_string());
            args.push(key.clone());
        }
        if let Some(ref jump) = conn.jump_host {
            args.push("-J".to_string());
            args.push(jump.clone());
        }
    }
    args
}

/// Returns the SCP target prefix "user@host" or "profile".
fn scp_target(conn: &RemoteConnection) -> String {
    if let Some(ref profile) = conn.ssh_profile {
        profile.clone()
    } else {
        format!("{}@{}", conn.username, conn.host)
    }
}

/// List SSH config profile (Host) names from ~/.ssh/config, following Include directives.
#[tauri::command]
pub fn list_ssh_profiles() -> Vec<String> {
    let Some(home) = crate::session::real_home_dir() else {
        return vec![];
    };
    let config_path = home.join(".ssh").join("config");
    let mut profiles = vec![];
    let mut visited = std::collections::HashSet::new();
    collect_ssh_hosts(&config_path, &home, &mut profiles, &mut visited);
    profiles
}

/// Recursively collect Host names from an SSH config file, resolving Include directives.
fn collect_ssh_hosts(
    path: &std::path::Path,
    home: &std::path::Path,
    profiles: &mut Vec<String>,
    visited: &mut std::collections::HashSet<PathBuf>,
) {
    let canonical = match std::fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) => return,
    };
    if !visited.insert(canonical) {
        return; // avoid cycles
    }

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let ssh_dir = home.join(".ssh");

    for line in content.lines() {
        let bare = line.splitn(2, '#').next().unwrap_or("").trim();
        if bare.is_empty() {
            continue;
        }
        let lower = bare.to_ascii_lowercase();

        if let Some(_) = lower.strip_prefix("host ") {
            let offset = "host ".len();
            for host in bare[offset..].split_whitespace() {
                if !host.contains('*') && !host.contains('?') {
                    profiles.push(host.to_string());
                }
            }
        } else if let Some(_) = lower.strip_prefix("include ") {
            let offset = "include ".len();
            let pattern = bare[offset..].trim();
            // Resolve ~ and relative paths per OpenSSH rules:
            //   ~/ → user home;  relative (no /) → relative to ~/.ssh/
            let resolved = if let Some(rest) = pattern.strip_prefix("~/") {
                home.join(rest).to_string_lossy().into_owned()
            } else if !pattern.starts_with('/') {
                ssh_dir.join(pattern).to_string_lossy().into_owned()
            } else {
                pattern.to_string()
            };

            // Expand globs (e.g. "config.d/*").  We handle the common case
            // where the glob is in the filename component only.
            let resolved_path = std::path::Path::new(&resolved);
            if let Some(fname) = resolved_path.file_name().and_then(|f| f.to_str()) {
                if fname.contains('*') || fname.contains('?') {
                    // Read directory and match entries against the pattern
                    if let Some(parent) = resolved_path.parent() {
                        if let Ok(entries) = std::fs::read_dir(parent) {
                            for entry in entries.flatten() {
                                let name = entry.file_name();
                                let name_str = name.to_string_lossy();
                                if glob_match(fname, &name_str) {
                                    collect_ssh_hosts(&entry.path(), home, profiles, visited);
                                }
                            }
                        }
                    }
                } else {
                    // No glob characters — literal path
                    collect_ssh_hosts(resolved_path, home, profiles, visited);
                }
            }
        }
    }
}

/// Simple glob matching supporting `*` (any chars) and `?` (single char).
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_inner(&p, &t)
}

fn glob_match_inner(pattern: &[char], text: &[char]) -> bool {
    let (mut pi, mut ti) = (0, 0);
    let (mut star_pi, mut star_ti) = (usize::MAX, 0);
    while ti < text.len() {
        if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == text[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star_pi = pi;
            star_ti = ti;
            pi += 1;
        } else if star_pi != usize::MAX {
            pi = star_pi + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }
    pi == pattern.len()
}

/// Run a download command via SSH, streaming progress lines `"<current_bytes> <total_bytes>"`.
/// The remote script must print `DONE` on success or `FAILED` on failure as its last line.
/// Run an SSH command on the remote host and return (stdout, stderr, success).
fn ssh_exec(conn: &RemoteConnection, remote_cmd: &str) -> Result<String, String> {
    let mut args = base_ssh_args(conn);
    args.push(remote_cmd.to_string());

    let mut cmd = std::process::Command::new("ssh");
    cmd.args(&args);
    apply_real_home(&mut cmd);
    let output = cmd.output()
        .map_err(|e| format!("ssh exec failed: {e}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(stderr)
    }
}

/// Find a platform-specific binary bundled as a Tauri resource, e.g. "fleet-linux-x64".
fn find_bundled_fleet_binary(app: &AppHandle, suffix: &str) -> Option<PathBuf> {
    let path = app
        .path()
        .resolve(
            format!("resources/fleet-{suffix}"),
            tauri::path::BaseDirectory::Resource,
        )
        .ok()?;
    if path.exists() { Some(path) } else { None }
}

/// Find the local fleet binary: sidecar next to app exe, then PATH.
fn find_local_fleet_binary() -> Option<PathBuf> {
    // Tauri bundles the sidecar next to the main executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // macOS app bundle: Contents/MacOS/fleet
            let candidate = dir.join("fleet");
            if candidate.exists() {
                return Some(candidate);
            }
            // The binary might be named fleet-cli (dev builds)
            let candidate2 = dir.join("fleet-cli");
            if candidate2.exists() {
                return Some(candidate2);
            }
        }
    }

    // Fallback: search PATH
    for path_dir in std::env::var("PATH").unwrap_or_default().split(':') {
        let p = PathBuf::from(path_dir).join("fleet");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Remote path where the probe binary lives.
fn remote_fleet_path() -> &'static str {
    "~/.fleet-probe/fleet"
}

/// Generate a simple random-ish auth token (good-enough for local SSH-tunnelled use).
fn generate_token() -> String {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let pid = std::process::id();
    format!("{:x}{:x}{:x}", t.as_secs(), t.subsec_nanos(), pid)
}

// ── Core connect logic ────────────────────────────────────────────────────────

/// Tauri command — returns immediately so the UI stays responsive.
/// All progress (including errors) is reported via `remote-connect-progress` events.
#[tauri::command]
pub fn connect_remote(
    conn: RemoteConnection,
    app: AppHandle,
    state: tauri::State<crate::AppState>,
) -> Result<(), String> {
    let backend_arc = state.backend.clone();

    std::thread::spawn(move || {
        match connect_remote_impl(conn, &app) {
            Ok(remote_backend) => {
                // Swap backend outside the lock so Drop (which may do SSH) doesn't
                // block other commands.
                let old = {
                    let mut guard = backend_arc.write().unwrap();
                    std::mem::replace(
                        &mut *guard,
                        Box::new(remote_backend) as Box<dyn crate::backend::Backend>,
                    )
                };
                drop(old);
            }
            Err(e) => {
                emit_progress(&app, &e, false, Some(&e));
            }
        }
    });

    Ok(()) // ← returns before SSH even starts; progress events drive the UI
}

fn connect_remote_impl(
    conn: RemoteConnection,
    app: &AppHandle,
) -> Result<RemoteBackend, String> {
    // ── Step 1: verify SSH connectivity + detect remote platform ─────────────
    emit_progress(app, "Connecting via SSH…", false, None);
    ssh_exec(&conn, "echo ok").map_err(|e| {
        emit_progress(app, "SSH connection failed", false, Some(&e));
        e
    })?;

    // Detect remote OS/arch so we upload the correct binary
    let remote_uname = ssh_exec(&conn, "uname -sm")
        .unwrap_or_default()
        .trim()
        .to_string();

    // Map to bundled binary suffix, e.g. "linux-x64"
    let release_suffix: Option<&str> = if remote_uname.contains("Linux") {
        if remote_uname.contains("x86_64") {
            Some("linux-x64")
        } else if remote_uname.contains("aarch64") || remote_uname.contains("arm64") {
            Some("linux-arm64")
        } else {
            None
        }
    } else {
        None
    };

    // ── Step 2: check remote version ─────────────────────────────────────────
    emit_progress(app, "Checking remote fleet version…", false, None);
    let current_version = env!("CARGO_PKG_VERSION");
    let remote_ver_out = ssh_exec(
        &conn,
        &format!("{} --version 2>/dev/null || echo NOT_FOUND", remote_fleet_path()),
    )
    .unwrap_or_else(|_| "NOT_FOUND".to_string());

    // Capability probe: the remote binary must support `--port-file` (added when
    // we switched to OS-assigned probe ports).  Without this check, same-version
    // dev builds would silently keep a stale old binary that can't report its port.
    let supports_port_file = ssh_exec(
        &conn,
        &format!(
            "{} serve --help 2>/dev/null | grep -q -- '--port-file' && echo OK || echo NO",
            remote_fleet_path()
        ),
    )
    .map(|s| s.trim() == "OK")
    .unwrap_or(false);

    let needs_install = remote_ver_out.contains("NOT_FOUND")
        || !remote_ver_out.contains(current_version)
        || !supports_port_file;

    // ── Step 3: install probe binary on remote ────────────────────────────────
    if needs_install {
        ssh_exec(&conn, "mkdir -p ~/.fleet-probe").map_err(|e| {
            emit_progress(app, "Failed to create remote directory", false, Some(&e));
            e
        })?;

        // Returns true if the locally-running app's native binary matches the remote platform.
        fn local_matches_remote(remote_uname: &str) -> bool {
            let os = std::env::consts::OS;
            let arch = std::env::consts::ARCH;
            match (os, arch) {
                ("macos",   "aarch64") => remote_uname.contains("Darwin") && remote_uname.contains("arm64"),
                ("macos",   "x86_64")  => remote_uname.contains("Darwin") && remote_uname.contains("x86_64"),
                ("linux",   "x86_64")  => remote_uname.contains("Linux")  && remote_uname.contains("x86_64"),
                ("linux",   "aarch64") => remote_uname.contains("Linux")  && (remote_uname.contains("aarch64") || remote_uname.contains("arm64")),
                ("windows", "x86_64")  => remote_uname.contains("Windows") && remote_uname.contains("x86_64"),
                ("windows", "aarch64") => remote_uname.contains("Windows") && remote_uname.contains("aarch64"),
                _ => false,
            }
        }

        // Priority 1: local native CLI (same OS+arch — e.g. macOS→macOS, Linux→Linux)
        // Priority 2: bundled cross-platform CLI (e.g. fleet-linux-x64 inside the app)
        let upload_bin: Option<PathBuf> = if local_matches_remote(&remote_uname) {
            find_local_fleet_binary()
        } else {
            release_suffix.and_then(|s| find_bundled_fleet_binary(app, s))
        };

        if let Some(bin) = upload_bin {
            let file_size = std::fs::metadata(&bin).map(|m| m.len()).unwrap_or(0);
            let size_str = if file_size > 1_048_576 {
                format!("{:.1} MB", file_size as f64 / 1_048_576.0)
            } else {
                format!("{} KB", file_size / 1024)
            };
            emit_progress(
                app,
                &format!("Uploading fleet binary for {remote_uname} ({size_str})…"),
                false,
                None,
            );

            let mut scp_args = base_scp_args(&conn);
            scp_args.push(bin.to_string_lossy().to_string());
            scp_args.push(format!("{}:{}", scp_target(&conn), remote_fleet_path()));

            let mut scp_cmd = std::process::Command::new("scp");
            scp_cmd.args(&scp_args);
            apply_real_home(&mut scp_cmd);
            let scp_out = scp_cmd.output()
                .map_err(|e| format!("scp failed: {e}"))?;

            if !scp_out.status.success() {
                let err = String::from_utf8_lossy(&scp_out.stderr).to_string();
                emit_progress(app, "Binary upload failed", false, Some(&err));
                return Err(err);
            }

            ssh_exec(&conn, &format!("chmod +x {}", remote_fleet_path())).map_err(|e| {
                emit_progress(app, "chmod failed", false, Some(&e));
                e
            })?;

            emit_progress(app, "Fleet binary ready.", false, None);
        } else {
            let err = format!(
                "No matching fleet binary available for {remote_uname}.\n\
                 Run build-local.sh to include the bundled probe binary."
            );
            emit_progress(app, &err, false, Some(&err));
            return Err(err);
        };
    } else {
        emit_progress(app, "Remote fleet binary up to date.", false, None);
    }

    connect_remote_start_probe(conn, app, remote_uname)
}

/// Steps 4–7: start probe, tunnel, health-check, poller.  Returns the fully
/// connected `RemoteBackend` on success.
fn connect_remote_start_probe(
    conn: RemoteConnection,
    app: &AppHandle,
    remote_uname: String,
) -> Result<RemoteBackend, String> {
    // ── Step 4: start remote probe ───────────────────────────────────────────
    emit_progress(app, "Starting remote fleet probe…", false, None);
    let token = generate_token();
    let remote_port_file = format!("/tmp/fleet-probe-{}.port", token);
    let remote_log_file = format!("/tmp/fleet-probe-{}.log", token);

    // Launch `fleet serve --port 0` on the remote: the OS picks an ephemeral
    // port, which the probe writes to `--port-file` so we can read it back.
    // `rm -f` before launching guarantees we don't read a stale file.
    let start_cmd = format!(
        r#"rm -f {pf}; ( setsid {bin} serve --port 0 --token {tok} --port-file {pf} >{log} 2>&1 </dev/null & echo $! ) 2>/dev/null || ( nohup {bin} serve --port 0 --token {tok} --port-file {pf} >{log} 2>&1 </dev/null & echo $! )"#,
        bin = remote_fleet_path(),
        tok = token,
        pf = remote_port_file,
        log = remote_log_file,
    );
    let pid_str = ssh_exec(&conn, &start_cmd).map_err(|e| {
        emit_progress(app, "Failed to start remote probe", false, Some(&e));
        e
    })?;
    let remote_probe_pid: Option<u32> = pid_str.trim().parse().ok();

    // Poll the remote port-file until the probe has bound and recorded its port.
    let remote_port: u16 = {
        let mut port_opt: Option<u16> = None;
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(200));
            if let Ok(out) = ssh_exec(&conn, &format!("cat {} 2>/dev/null", remote_port_file)) {
                if let Ok(p) = out.trim().parse::<u16>() {
                    if p != 0 {
                        port_opt = Some(p);
                        break;
                    }
                }
            }
        }
        match port_opt {
            Some(p) => p,
            None => {
                let probe_log = ssh_exec(&conn, &format!("tail -20 {} 2>/dev/null", remote_log_file))
                    .unwrap_or_else(|_| "(could not read probe log)".to_string());
                if let Some(pid) = remote_probe_pid {
                    let _ = ssh_exec(&conn, &format!("kill {} 2>/dev/null", pid));
                }
                let err = format!(
                    "Remote probe never reported its port.\nProbe log:\n{probe_log}"
                );
                emit_progress(app, &err, false, Some(&err));
                return Err(err);
            }
        }
    };

    // Pick a free local TCP port for the tunnel.  There is a tiny race window
    // between dropping the listener and ssh binding it, but on loopback with a
    // desktop app it is effectively never triggered; ExitOnForwardFailure=yes
    // will surface the conflict quickly if it ever does happen.
    let local_port: u16 = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("Failed to find a free local port: {e}"))?;
        let p = listener.local_addr()
            .map_err(|e| format!("Failed to read local addr: {e}"))?
            .port();
        drop(listener);
        p
    };

    // ── Step 5: start local SSH tunnel ───────────────────────────────────────
    emit_progress(app, "Creating SSH tunnel…", false, None);

    let mut tunnel_args: Vec<String> = vec![
        "-N".to_string(),
        "-L".to_string(),
        format!("{}:127.0.0.1:{}", local_port, remote_port),
        "-o".to_string(), "StrictHostKeyChecking=accept-new".to_string(),
        "-o".to_string(), "ConnectTimeout=15".to_string(),
        "-o".to_string(), "ServerAliveInterval=30".to_string(),
        "-o".to_string(), "ExitOnForwardFailure=yes".to_string(),
    ];
    if let Some(ref profile) = conn.ssh_profile {
        tunnel_args.push(profile.clone());
    } else {
        tunnel_args.push("-p".to_string());
        tunnel_args.push(conn.port.to_string());
        if let Some(ref key) = conn.identity_file {
            tunnel_args.push("-i".to_string());
            tunnel_args.push(key.clone());
        }
        if let Some(ref jump) = conn.jump_host {
            tunnel_args.push("-J".to_string());
            tunnel_args.push(jump.clone());
        }
        tunnel_args.push(format!("{}@{}", conn.username, conn.host));
    }

    let mut tunnel_cmd = std::process::Command::new("ssh");
    tunnel_cmd
        .args(&tunnel_args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    apply_real_home(&mut tunnel_cmd);
    let tunnel_child = tunnel_cmd.spawn()
        .map_err(|e| format!("Failed to start SSH tunnel: {e}"))?;

    // ── Step 6: wait for probe to be ready ───────────────────────────────────
    emit_progress(app, "Waiting for probe to be ready…", false, None);
    let base_url = format!("http://127.0.0.1:{}", local_port);
    let probe = ProbeClient::new(base_url, &token);

    let ready = (0..20).any(|i| {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(500));
        }
        probe.get_ok("/health").is_ok()
    });

    if !ready {
        let mut tc = tunnel_child;
        let _ = tc.kill();
        let probe_log = ssh_exec(&conn, &format!("tail -20 {} 2>/dev/null", remote_log_file))
            .unwrap_or_else(|_| "(could not read probe log)".to_string());
        if let Some(pid) = remote_probe_pid {
            let _ = ssh_exec(&conn, &format!("kill {} 2>/dev/null", pid));
        }
        let err = format!(
            "Probe did not become ready within 10 seconds.\nProbe log:\n{probe_log}"
        );
        emit_progress(app, &err, false, Some(&err));
        return Err(err);
    }

    // ── Step 7: save connection & start background poller ────────────────────
    let mut saved = load_saved_connections();
    if let Some(existing) = saved.iter_mut().find(|c| c.id == conn.id) {
        *existing = conn.clone();
    } else {
        saved.push(conn.clone());
    }
    let _ = save_connections(&saved);

    // Sessions cache owned by this RemoteBackend instance.
    let sessions: Arc<Mutex<Vec<SessionInfo>>> = Arc::new(Mutex::new(Vec::new()));
    let poller_running = Arc::new(Mutex::new(true));
    let tail_running = Arc::new(Mutex::new(true));
    let waiting_alerts: Arc<Mutex<std::collections::HashMap<String, crate::backend::WaitingAlert>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));
    let session_outcomes: Arc<Mutex<std::collections::HashMap<String, Vec<String>>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));
    // Do an initial synchronous fetch so list_sessions() is populated immediately.
    if let Ok(s) = probe.get::<Vec<SessionInfo>>("/sessions") {
        *sessions.lock().unwrap() = s.clone();
        let _ = app.emit("sessions-updated", &s);
        let _ = app.emit("scan-ready", true);
        crate::update_tray(app, &s);
    }

    // Start background poller for continuous session updates + waiting alerts.
    {
        let app2 = app.clone();
        let pr = poller_running.clone();
        let sess2 = sessions.clone();
        let wa2 = waiting_alerts.clone();
        let so2 = session_outcomes.clone();
        let probe2 = probe.clone();
        let locale = app
            .try_state::<crate::AppState>()
            .map(|s| s.locale.lock().unwrap().clone())
            .unwrap_or_else(|| "en".to_string());
        std::thread::spawn(move || {
            use std::collections::{HashMap, HashSet};
            use crate::session::SessionStatus;

            let mut prev_statuses: HashMap<String, SessionStatus> = HashMap::new();
            let analyzing: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
            let busy_statuses = [
                SessionStatus::Thinking,
                SessionStatus::Executing,
                SessionStatus::Streaming,
                SessionStatus::Processing,
                SessionStatus::Active,
            ];

            loop {
                std::thread::sleep(Duration::from_secs(1));
                if !*pr.lock().unwrap() {
                    break;
                }
                let Ok(mut s) = probe2.get::<Vec<SessionInfo>>("/sessions") else {
                    continue;
                };

                // Inject cached outcome tags.
                {
                    let oc = so2.lock().unwrap();
                    for sess in &mut s {
                        if let Some(tags) = oc.get(&sess.id) {
                            sess.last_outcome = Some(tags.clone());
                        }
                    }
                }

                *sess2.lock().unwrap() = s.clone();
                let _ = app2.emit("sessions-updated", &s);
                crate::update_tray(&app2, &s);

                // ── Waiting-input detection & outcome analysis ───────────────
                let mut alerts_changed = false;
                for sess in &s {
                    if sess.is_subagent {
                        continue;
                    }
                    let prev = prev_statuses.get(&sess.id);
                    let is_waiting = sess.status == SessionStatus::WaitingInput;
                    let was_waiting = prev == Some(&SessionStatus::WaitingInput);
                    let was_busy = prev.map_or(false, |p| busy_statuses.contains(p));

                    if is_waiting && !was_waiting {
                        let mut guard = analyzing.lock().unwrap();
                        if guard.contains(&sess.id) {
                            continue;
                        }
                        guard.insert(sess.id.clone());
                        drop(guard);

                        let session_id = sess.id.clone();
                        let display_name = sess.ai_title.clone()
                            .unwrap_or_else(|| sess.workspace_name.clone());
                        let last_text = sess.last_message_preview.clone().unwrap_or_default();
                        let agent_source = sess.agent_source.clone();
                        let wa = wa2.clone();
                        let so = so2.clone();
                        let an = analyzing.clone();
                        let app_bg = app2.clone();
                        let lang = locale.clone();
                        let title = crate::local_backend::get_user_title(&app_bg);
                        let jsonl_path = sess.jsonl_path.clone();

                        let probe_bg = probe2.clone();

                        std::thread::spawn(move || {
                            // Delegate LLM analysis to the remote probe which
                            // has access to CLI tools (Claude Code, etc.).
                            let req = crate::claude_analyze::AnalyzeRequest {
                                session_id: session_id.clone(),
                                last_text,
                                locale: lang,
                                user_title: title,
                            };
                            let result: Option<crate::claude_analyze::AnalysisResult> =
                                probe_bg.post_json("/analyze", &req).ok();
                            an.lock().unwrap().remove(&session_id);

                            if let Some(ref result) = result {
                                so.lock().unwrap().insert(session_id.clone(), result.tags.clone());
                            }

                            let has_needs_input = result.as_ref()
                                .map_or(false, |r| r.tags.contains(&"needs_input".to_string()));
                            let mode = crate::local_backend::get_notification_mode(&app_bg);

                            let should_alert = mode == "all" || has_needs_input;
                            let should_os_notify = mode != "none" && (mode == "all" || has_needs_input);

                            if should_alert {
                                let summary = result.as_ref().and_then(|r| r.summary.clone())
                                    .unwrap_or_else(|| crate::local_backend::fallback_summary_for_tags(
                                        result.as_ref().map(|r| r.tags.as_slice()).unwrap_or(&[])
                                    ));
                                let alert = crate::backend::WaitingAlert {
                                    session_id: session_id.clone(),
                                    workspace_name: display_name.clone(),
                                    summary: summary.clone(),
                                    detected_at_ms: SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_millis() as u64,
                                    jsonl_path,
                                    source: agent_source.clone(),
                                };
                                wa.lock().unwrap().insert(session_id, alert);
                                let alerts: Vec<crate::backend::WaitingAlert> =
                                    wa.lock().unwrap().values().cloned().collect();
                                let _ = app_bg.emit("waiting-alerts-updated", &alerts);
                                if should_os_notify {
                                    crate::local_backend::send_os_notification(
                                        &app_bg, &display_name, &summary,
                                    );
                                }

                                // Suppress waitalert TTS for Claude Code — its
                                // waits are already spoken by the DecisionPanel
                                // (AskUserQuestion bridge) to avoid double play.
                                if agent_source != "claude-code" {
                                    crate::play_tts_for_notification(&app_bg, &summary);
                                }
                            }
                        });
                    } else if !is_waiting && was_waiting {
                        if wa2.lock().unwrap().remove(&sess.id).is_some() {
                            alerts_changed = true;
                        }
                    }

                    // Clear stale outcome when session becomes busy again.
                    if busy_statuses.contains(&sess.status) && !was_busy {
                        so2.lock().unwrap().remove(&sess.id);
                    }
                }

                // Clean up alerts for sessions that no longer exist.
                {
                    let current_ids: HashSet<String> =
                        s.iter().map(|sess| sess.id.clone()).collect();
                    let mut wa = wa2.lock().unwrap();
                    let before = wa.len();
                    wa.retain(|id, _| current_ids.contains(id));
                    if wa.len() != before {
                        alerts_changed = true;
                    }
                }

                if alerts_changed {
                    let alerts: Vec<crate::backend::WaitingAlert> =
                        wa2.lock().unwrap().values().cloned().collect();
                    let _ = app2.emit("waiting-alerts-updated", &alerts);
                }

                prev_statuses.clear();
                for sess in &s {
                    if !sess.is_subagent {
                        prev_statuses.insert(sess.id.clone(), sess.status.clone());
                    }
                }

            }
        });
    }

    // Guard polling thread — polls remote probe for pending guard requests.
    {
        let app_guard = app.clone();
        let pr_guard = poller_running.clone();
        let probe_guard = probe.clone();
        std::thread::spawn(move || {
            let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();
            loop {
                std::thread::sleep(Duration::from_millis(500));
                if !*pr_guard.lock().unwrap() {
                    break;
                }
                let Ok(pending) = probe_guard.get::<Vec<claw_fleet_core::guard::GuardRequest>>("/guard/pending") else {
                    continue;
                };
                let pending_ids: std::collections::HashSet<String> =
                    pending.iter().map(|r| r.id.clone()).collect();
                for req in &pending {
                    if known.insert(req.id.clone()) {
                        claw_fleet_core::log_debug(&format!(
                            "[remote-guard] new request: {} cmd={}",
                            req.id, req.command_summary
                        ));
                        let _ = app_guard.emit("guard-request", req);
                    }
                }
                for id in known.iter().filter(|id| !pending_ids.contains(*id)) {
                    let _ = app_guard.emit("guard-dismissed", id.clone());
                }
                known.retain(|id| pending_ids.contains(id));
            }
        });
    }

    // Elicitation polling thread — polls remote probe for pending elicitation requests.
    {
        let app_elicit = app.clone();
        let pr_elicit = poller_running.clone();
        let probe_elicit = probe.clone();
        std::thread::spawn(move || {
            let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();
            loop {
                std::thread::sleep(Duration::from_millis(500));
                if !*pr_elicit.lock().unwrap() {
                    break;
                }
                let Ok(pending) = probe_elicit.get::<Vec<claw_fleet_core::elicitation::ElicitationRequest>>("/elicitation/pending") else {
                    continue;
                };
                let pending_ids: std::collections::HashSet<String> =
                    pending.iter().map(|r| r.id.clone()).collect();
                for req in &pending {
                    if known.insert(req.id.clone()) {
                        claw_fleet_core::log_debug(&format!(
                            "[remote-elicitation] new request: {} questions={}",
                            req.id, req.questions.len()
                        ));
                        let _ = app_elicit.emit("elicitation-request", req);
                    }
                }
                for id in known.iter().filter(|id| !pending_ids.contains(*id)) {
                    let _ = app_elicit.emit("elicitation-dismissed", id.clone());
                }
                known.retain(|id| pending_ids.contains(id));
            }
        });
    }

    // Plan-approval polling thread — polls remote probe for pending plan-approval requests.
    {
        let app_plan = app.clone();
        let pr_plan = poller_running.clone();
        let probe_plan = probe.clone();
        std::thread::spawn(move || {
            let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();
            loop {
                std::thread::sleep(Duration::from_millis(500));
                if !*pr_plan.lock().unwrap() {
                    break;
                }
                let Ok(pending) = probe_plan.get::<Vec<claw_fleet_core::plan_approval::PlanApprovalRequest>>("/plan-approval/pending") else {
                    continue;
                };
                let pending_ids: std::collections::HashSet<String> =
                    pending.iter().map(|r| r.id.clone()).collect();
                for req in &pending {
                    if known.insert(req.id.clone()) {
                        claw_fleet_core::log_debug(&format!(
                            "[remote-plan-approval] new request: {} plan_len={}",
                            req.id,
                            req.plan_content.len()
                        ));
                        let _ = app_plan.emit("plan-approval-request", req);
                    }
                }
                for id in known.iter().filter(|id| !pending_ids.contains(*id)) {
                    let _ = app_plan.emit("plan-approval-dismissed", id.clone());
                }
                known.retain(|id| pending_ids.contains(id));
            }
        });
    }

    let label = if let Some(ref p) = conn.ssh_profile {
        p.clone()
    } else {
        format!("{}@{}", conn.username, conn.host)
    };
    emit_progress(app, &format!("Connected to {label} ({remote_uname})"), true, None);

    Ok(RemoteBackend {
        connection: conn,
        probe,
        tunnel_child,
        remote_probe_pid,
        poller_running,
        tail_running,
        app: app.clone(),
        sessions,
        watch: Arc::new(crate::WatchState::new()),
        waiting_alerts,
        session_outcomes,
    })
}

// ── Disconnect ────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn disconnect_remote(
    state: tauri::State<crate::AppState>,
    app: AppHandle,
) -> Result<(), String> {
    // Construct the new LocalBackend first (triggers initial local scan and
    // emits sessions-updated) before dropping the RemoteBackend.
    let locale = state.locale.clone();
    let llm_config = state.llm_config.clone();
    let sources = crate::agent_source::build_sources();
    let new_backend = crate::local_backend::LocalBackend::new(app, locale, llm_config, sources);
    // Swap: drop old backend (RemoteBackend::Drop kills tunnel + remote probe)
    // outside the lock so the SSH cleanup doesn't block other commands.
    let old = {
        let mut guard = state.backend.write().unwrap();
        std::mem::replace(
            &mut *guard,
            Box::new(new_backend) as Box<dyn crate::backend::Backend>,
        )
    };
    drop(old);
    Ok(())
}

// ── Tail remote helper ──────────────────────────────────────────────────────

fn start_remote_tail(
    probe: ProbeClient,
    jsonl_path: String,
    initial_offset: u64,
    app: AppHandle,
    tail_running: Arc<Mutex<bool>>,
) {
    std::thread::spawn(move || {
        let mut offset = initial_offset;

        while *tail_running.lock().unwrap() {
            std::thread::sleep(Duration::from_millis(500));
            let endpoint = format!(
                "/tail?path={}&offset={}",
                encode_path(&jsonl_path),
                offset
            );
            if let Ok(val) = probe.get_value(&endpoint) {
                if let Some(lines) = val["lines"].as_array() {
                    if !lines.is_empty() {
                        let _ = app.emit("session-tail", lines);
                    }
                }
                if let Some(new_offset) = val["newOffset"].as_u64() {
                    offset = new_offset;
                }
            }
        }
    });
}
