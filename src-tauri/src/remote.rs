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
    /// Local port that will be forwarded via the SSH tunnel.
    #[serde(default = "default_probe_port")]
    pub probe_port: u16,
}

fn default_probe_port() -> u16 {
    7007
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

    /// GET endpoint and return the raw `serde_json::Value`.
    fn get_value(&self, endpoint: &str) -> Result<serde_json::Value, String> {
        self.get::<serde_json::Value>(endpoint)
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
    watch: Arc<crate::backend::WatchState>,
    /// Active waiting-input alerts, keyed by session ID.
    waiting_alerts: Arc<Mutex<std::collections::HashMap<String, crate::backend::WaitingAlert>>>,
    /// Semantic outcome tags per session, set by background analysis.
    #[allow(dead_code)]
    session_outcomes: Arc<Mutex<std::collections::HashMap<String, Vec<String>>>>,
    /// Keys of critical audit events for which notifications have already been sent.
    #[allow(dead_code)]
    notified_audit_keys: Arc<Mutex<std::collections::HashSet<String>>>,
}

impl Drop for RemoteBackend {
    fn drop(&mut self) {
        *self.poller_running.lock().unwrap() = false;
        *self.tail_running.lock().unwrap() = false;
        let _ = self.tunnel_child.kill();
        // Best-effort: kill the remote fleet-serve process.
        if let Some(pid) = self.remote_probe_pid {
            let _ = ssh_exec(&self.connection, &format!("kill {} 2>/dev/null", pid));
        }
        let _ = ssh_exec(
            &self.connection,
            &format!(
                "pkill -f 'fleet serve --port {}' 2>/dev/null",
                self.connection.probe_port
            ),
        );
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

    fn kill_pid(&self, pid: u32) -> Result<(), String> {
        self.probe.get_ok(&format!("/stop?pid={}&force=false", pid))
    }

    fn kill_workspace(&self, workspace_path: String) -> Result<(), String> {
        let encoded = workspace_path.replace('/', "%2F");
        self.probe.get_ok(&format!("/stop_workspace?path={}", encoded))
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
        })
    }

    fn apply_hooks(&self) -> Result<(), String> {
        self.probe.post_ok("/apply_hooks")
    }

    fn remove_hooks(&self) -> Result<(), String> {
        self.probe.post_ok("/remove_hooks")
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
    dirs::home_dir().map(|h| h.join(".claude").join("fleet-connections.json"))
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

/// List SSH config profile (Host) names from ~/.ssh/config.
#[tauri::command]
pub fn list_ssh_profiles() -> Vec<String> {
    let Some(config_path) = dirs::home_dir().map(|h| h.join(".ssh").join("config")) else {
        return vec![];
    };
    let Ok(content) = std::fs::read_to_string(&config_path) else {
        return vec![];
    };
    let mut profiles = vec![];
    for line in content.lines() {
        // Strip inline comments first, then trim whitespace
        let bare = line.splitn(2, '#').next().unwrap_or("").trim();
        if bare.is_empty() {
            continue;
        }
        // Case-insensitive "Host" keyword (SSH config is case-insensitive for keywords)
        let lower = bare.to_ascii_lowercase();
        if let Some(_) = lower.strip_prefix("host ") {
            // Use original (non-lowercased) chars for the actual host names
            let offset = "host ".len();
            for host in bare[offset..].split_whitespace() {
                // Skip wildcard patterns — they are not selectable profiles
                if !host.contains('*') && !host.contains('?') {
                    profiles.push(host.to_string());
                }
            }
        }
    }
    profiles
}

/// Run a download command via SSH, streaming progress lines `"<current_bytes> <total_bytes>"`.
/// The remote script must print `DONE` on success or `FAILED` on failure as its last line.
/// Run an SSH command on the remote host and return (stdout, stderr, success).
fn ssh_exec(conn: &RemoteConnection, remote_cmd: &str) -> Result<String, String> {
    let mut args = base_ssh_args(conn);
    args.push(remote_cmd.to_string());

    let output = std::process::Command::new("ssh")
        .args(&args)
        .output()
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
    let resource_dir = app.path().resource_dir().ok()?;
    let path = resource_dir.join(format!("fleet-{suffix}"));
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
                    let mut guard = backend_arc.lock().unwrap();
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

    let needs_install = remote_ver_out.contains("NOT_FOUND")
        || !remote_ver_out.contains(current_version);

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

            let scp_out = std::process::Command::new("scp")
                .args(&scp_args)
                .output()
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
    let probe_port = conn.probe_port;

    let _ = ssh_exec(
        &conn,
        &format!(
            "pkill -f 'fleet serve --port {}' 2>/dev/null; sleep 0.3",
            probe_port
        ),
    );

    let start_cmd = format!(
        r#"( setsid {bin} serve --port {port} --token {tok} >/tmp/fleet-probe.log 2>&1 </dev/null & echo $! ) 2>/dev/null || ( nohup {bin} serve --port {port} --token {tok} >/tmp/fleet-probe.log 2>&1 </dev/null & echo $! )"#,
        bin = remote_fleet_path(),
        port = probe_port,
        tok = token,
    );
    let pid_str = ssh_exec(&conn, &start_cmd).map_err(|e| {
        emit_progress(app, "Failed to start remote probe", false, Some(&e));
        e
    })?;
    let remote_probe_pid: Option<u32> = pid_str.trim().parse().ok();

    std::thread::sleep(Duration::from_millis(500));

    // ── Step 5: start local SSH tunnel ───────────────────────────────────────
    emit_progress(app, "Creating SSH tunnel…", false, None);

    let mut tunnel_args: Vec<String> = vec![
        "-N".to_string(),
        "-L".to_string(),
        format!("{}:127.0.0.1:{}", probe_port, probe_port),
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

    let tunnel_child = std::process::Command::new("ssh")
        .args(&tunnel_args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to start SSH tunnel: {e}"))?;

    // ── Step 6: wait for probe to be ready ───────────────────────────────────
    emit_progress(app, "Waiting for probe to be ready…", false, None);
    let base_url = format!("http://127.0.0.1:{}", probe_port);
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
        let probe_log = ssh_exec(&conn, "tail -20 /tmp/fleet-probe.log 2>/dev/null")
            .unwrap_or_else(|_| "(could not read probe log)".to_string());
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
    let notified_audit_keys: Arc<Mutex<std::collections::HashSet<String>>> =
        Arc::new(Mutex::new(std::collections::HashSet::new()));

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
        let nak2 = notified_audit_keys.clone();
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
                        let wa = wa2.clone();
                        let so = so2.clone();
                        let an = analyzing.clone();
                        let app_bg = app2.clone();
                        let lang = locale.clone();
                        let title = crate::local_backend::get_user_title(&app_bg);
                        let jsonl_path = sess.jsonl_path.clone();

                        std::thread::spawn(move || {
                            let result = crate::claude_analyze::analyze_session_outcome(
                                &last_text, &lang, &session_id, &title,
                            );
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

                                // Play TTS from backend (blocks until done).
                                crate::play_tts_for_notification(&app_bg, &summary);
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

                // ── Audit critical event notifications ─────────────────────
                {
                    let mode = crate::local_backend::get_notification_mode(&app2);
                    if mode != "none" {
                        if let Ok(summary) = probe2.get::<crate::audit::AuditSummary>("/audit") {
                            let mut nk = nak2.lock().unwrap();
                            for event in &summary.events {
                                if event.risk_level != crate::audit::AuditRiskLevel::Critical {
                                    continue;
                                }
                                let key = event.dedup_key();
                                if nk.contains(&key) {
                                    continue;
                                }
                                nk.insert(key.clone());

                                let title = format!("⚠ CRITICAL: {}", event.workspace_name);
                                crate::local_backend::send_os_notification(
                                    &app2, &title, &event.command_summary,
                                );

                                let alert = crate::audit::AuditAlert {
                                    key,
                                    session_id: event.session_id.clone(),
                                    workspace_name: event.workspace_name.clone(),
                                    command_summary: event.command_summary.clone(),
                                    risk_tags: event.risk_tags.clone(),
                                    detected_at_ms: SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_millis() as u64,
                                    jsonl_path: event.jsonl_path.clone(),
                                };
                                let _ = app2.emit("audit-critical-alert", &alert);
                            }
                        }
                    }
                }
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
        watch: Arc::new(crate::backend::WatchState::new()),
        waiting_alerts,
        notified_audit_keys,
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
    let sources = crate::agent_source::build_sources();
    let new_backend = crate::local_backend::LocalBackend::new(app, locale, sources);
    // Swap: drop old backend (RemoteBackend::Drop kills tunnel + remote probe)
    // outside the lock so the SSH cleanup doesn't block other commands.
    let old = {
        let mut guard = state.backend.lock().unwrap();
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
