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

// ── RemoteBackend ─────────────────────────────────────────────────────────────

/// Active remote connection.  Implements [`crate::backend::Backend`] so that
/// all Tauri command handlers can delegate without if/else branching.
pub struct RemoteBackend {
    // Connection metadata (needed for Drop to reach the remote probe).
    connection: RemoteConnection,
    /// `http://127.0.0.1:<probe_port>`
    base_url: String,
    token: String,
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
    viewed_session: Arc<Mutex<Option<String>>>,
    viewed_offset: Arc<Mutex<u64>>,
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

impl crate::backend::Backend for RemoteBackend {
    fn list_sessions(&self) -> Vec<crate::session::SessionInfo> {
        self.sessions.lock().unwrap().clone()
    }

    fn get_messages(&self, path: &str) -> Result<Vec<serde_json::Value>, String> {
        remote_get_messages(&self.base_url, &self.token, path)
    }

    fn kill_pid(&self, pid: u32) -> Result<(), String> {
        remote_kill_session(&self.base_url, &self.token, pid, false)
    }

    fn kill_workspace(&self, workspace_path: String) -> Result<(), String> {
        remote_kill_workspace(&self.base_url, &self.token, &workspace_path)
    }

    fn account_info(&self) -> Result<crate::account::AccountInfo, String> {
        remote_get_account_info(&self.base_url, &self.token)
    }

    fn start_watch(&self, path: String) -> Result<u64, String> {
        let file_size = remote_file_size(&self.base_url, &self.token, &path);
        *self.tail_running.lock().unwrap() = true;
        start_remote_tail(
            self.base_url.clone(),
            self.token.clone(),
            path.clone(),
            file_size,
            self.app.clone(),
            self.tail_running.clone(),
        );
        *self.viewed_session.lock().unwrap() = Some(path);
        *self.viewed_offset.lock().unwrap() = file_size;
        Ok(file_size)
    }

    fn stop_watch(&self) {
        *self.tail_running.lock().unwrap() = false;
        *self.viewed_session.lock().unwrap() = None;
        *self.viewed_offset.lock().unwrap() = 0;
    }

    fn list_memories(&self) -> Vec<crate::memory::WorkspaceMemory> {
        remote_list_memories(&self.base_url, &self.token).unwrap_or_default()
    }

    fn get_memory_content(&self, path: &str) -> Result<String, String> {
        remote_get_memory_content(&self.base_url, &self.token, path)
    }

    fn get_memory_history(&self, path: &str) -> Vec<crate::memory::MemoryHistoryEntry> {
        remote_get_memory_history(&self.base_url, &self.token, path).unwrap_or_default()
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

/// Like `emit_progress` but replaces the last progress entry in the frontend (for live updates).
fn emit_progress_update(app: &AppHandle, step: &str) {
    let _ = app.emit(
        "remote-connect-progress",
        ConnectProgress {
            step: step.to_string(),
            done: false,
            error: None,
            update_last: true,
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
fn ssh_download_with_progress<F>(
    conn: &RemoteConnection,
    remote_cmd: &str,
    mut on_progress: F,
) -> Result<(), String>
where
    F: FnMut(u64, u64),
{
    use std::io::BufRead;

    let mut args = base_ssh_args(conn);
    args.push(remote_cmd.to_string());

    let mut child = std::process::Command::new("ssh")
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("ssh spawn failed: {e}"))?;

    let stdout = child.stdout.take().ok_or("no stdout from ssh")?;
    let reader = std::io::BufReader::new(stdout);

    let mut success = false;
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim();
        if line == "DONE" {
            success = true;
            break;
        } else if line == "FAILED" {
            break;
        } else {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let cur: u64 = parts[0].parse().unwrap_or(0);
                let total: u64 = parts[1].parse().unwrap_or(0);
                on_progress(cur, total);
            }
        }
    }

    child.wait().ok();

    if success {
        Ok(())
    } else {
        Err("Remote download failed".to_string())
    }
}

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

    // Map to GitHub release artifact suffix, e.g. "linux-x64"
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
        // Priority 3: download from latest GitHub release on the remote (needs internet)
        // Priority 4: error
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
        } else if let Some(suffix) = release_suffix {
            // Priority 3: download directly on the remote
            let dl_url = format!(
                "https://github.com/hoveychen/claude-fleet/releases/latest/download/fleet-{suffix}"
            );
            emit_progress(
                app,
                &format!("Downloading fleet binary for {remote_uname}…"),
                false,
                None,
            );
            // Bash script: fetches Content-Length first, then downloads in background
            // while printing "<current_bytes> <total_bytes>" lines for progress.
            // Prints DONE or FAILED as the final line.
            let bin = remote_fleet_path();
            let dl_cmd = format!(
                r#"TOTAL=$(curl -sI "{url}" 2>/dev/null | grep -i 'content-length' | tail -1 | tr -d '\r' | awk '{{print $2}}'); [ -z "$TOTAL" ] && TOTAL=0; curl -fL "{url}" -o {bin}.tmp 2>/dev/null & CURL_PID=$!; while kill -0 $CURL_PID 2>/dev/null; do CUR=$(stat -c '%s' {bin}.tmp 2>/dev/null || echo 0); echo "$CUR $TOTAL"; sleep 0.5; done; wait $CURL_PID; if [ $? -eq 0 ]; then mv {bin}.tmp {bin} && chmod +x {bin} && echo DONE; else rm -f {bin}.tmp; echo FAILED; fi"#,
                url = dl_url,
                bin = bin,
            );
            let app_ref = app;
            let result = ssh_download_with_progress(&conn, &dl_cmd, |cur, total| {
                let step = if total > 0 {
                    let pct = (cur as f64 / total as f64 * 100.0) as u32;
                    let cur_mb = cur as f64 / 1_048_576.0;
                    let total_mb = total as f64 / 1_048_576.0;
                    format!("Downloading fleet binary… {pct}% ({cur_mb:.1}/{total_mb:.1} MB)")
                } else {
                    let cur_mb = cur as f64 / 1_048_576.0;
                    format!("Downloading fleet binary… {cur_mb:.1} MB")
                };
                emit_progress_update(app_ref, &step);
            });
            if result.is_ok() {
                emit_progress_update(app, "Downloading fleet binary… complete.");
                // Binary is already in place — skip SCP, go straight to probe startup
                return connect_remote_start_probe(conn, app, remote_uname);
            }
            let err = format!(
                "No bundled binary for {remote_uname} and GitHub download failed.\n\
                 Run build-local.sh to include the bundled Linux probe binary."
            );
            emit_progress(app, &err, false, Some(&err));
            return Err(err);
        } else {
            let err = format!(
                "Unsupported remote platform: {remote_uname}.\n\
                 No matching binary available (local, bundled, or downloadable)."
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
    let health_url = format!("{}/health", base_url);
    let auth_header = format!("Bearer {}", token);

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| e.to_string())?;

    let ready = (0..20).any(|i| {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(500));
        }
        client
            .get(&health_url)
            .header("Authorization", &auth_header)
            .send()
            .map(|r| r.status().is_success())
            .unwrap_or(false)
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

    // Do an initial synchronous fetch so list_sessions() is populated immediately.
    {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        if let Ok(resp) = client
            .get(&format!("{}/sessions", base_url))
            .header("Authorization", &auth_header)
            .send()
        {
            if let Ok(body) = resp.text() {
                if let Ok(s) = serde_json::from_str::<Vec<SessionInfo>>(&body) {
                    *sessions.lock().unwrap() = s.clone();
                    let _ = app.emit("sessions-updated", &s);
                    crate::update_tray(app, &s);
                }
            }
        }
    }

    // Start background poller for continuous session updates.
    {
        let app2 = app.clone();
        let pr = poller_running.clone();
        let sess2 = sessions.clone();
        let poll_url = format!("{}/sessions", base_url);
        let poll_auth = format!("Bearer {}", token);

        std::thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap();
            let mut last_json = String::new();
            while *pr.lock().unwrap() {
                std::thread::sleep(Duration::from_secs(1));
                if let Ok(resp) = client
                    .get(&poll_url)
                    .header("Authorization", &poll_auth)
                    .send()
                {
                    if let Ok(body) = resp.text() {
                        if body != last_json {
                            last_json = body.clone();
                            if let Ok(s) = serde_json::from_str::<Vec<SessionInfo>>(&body) {
                                *sess2.lock().unwrap() = s.clone();
                                let _ = app2.emit("sessions-updated", &s);
                                crate::update_tray(&app2, &s);
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
        base_url,
        token,
        tunnel_child,
        remote_probe_pid,
        poller_running,
        tail_running,
        app: app.clone(),
        sessions,
        viewed_session: Arc::new(Mutex::new(None)),
        viewed_offset: Arc::new(Mutex::new(0)),
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
    let new_backend = crate::local_backend::LocalBackend::new(app);
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

// ── HTTP proxy helpers (used by lib.rs mode-aware commands) ──────────────────

/// Encode a path for use in a query parameter.
pub fn encode_path(path: &str) -> String {
    utf8_percent_encode(path, NON_ALPHANUMERIC).to_string()
}

/// GET `{base_url}/stop_workspace?path=<encoded>` to kill all claude processes in a workspace.
pub fn remote_kill_workspace(base_url: &str, token: &str, workspace_path: &str) -> Result<(), String> {
    let encoded = workspace_path.replace('/', "%2F");
    let url = format!("{}/stop_workspace?path={}", base_url, encoded);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Remote stop_workspace error: HTTP {}", resp.status()));
    }
    Ok(())
}

/// POST `{base_url}/stop?pid=<pid>&force=<bool>` to kill a process on the remote host.
pub fn remote_kill_session(base_url: &str, token: &str, pid: u32, force: bool) -> Result<(), String> {
    let url = format!("{}/stop?pid={}&force={}", base_url, pid, force);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Remote stop error: HTTP {}", resp.status()));
    }
    Ok(())
}

/// GET `{base_url}/account` and return the remote `AccountInfo`.
pub fn remote_get_account_info(
    base_url: &str,
    token: &str,
) -> Result<crate::account::AccountInfo, String> {
    let url = format!("{}/account", base_url);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Remote account error: HTTP {}", resp.status()));
    }
    resp.json::<crate::account::AccountInfo>().map_err(|e| e.to_string())
}

/// GET `{base_url}/messages?path=<encoded>` and return raw JSON values.
pub fn remote_get_messages(
    base_url: &str,
    token: &str,
    jsonl_path: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let url = format!("{}/messages?path={}", base_url, encode_path(jsonl_path));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| e.to_string())?;
    let messages: Vec<serde_json::Value> = resp.json().map_err(|e| e.to_string())?;
    Ok(messages)
}

/// GET `{base_url}/file_size?path=<encoded>` and return the file size.
pub fn remote_file_size(base_url: &str, token: &str, jsonl_path: &str) -> u64 {
    let url = format!("{}/file_size?path={}", base_url, encode_path(jsonl_path));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .ok()
        .and_then(|r| r.json::<serde_json::Value>().ok())
        .and_then(|v| v["size"].as_u64())
        .unwrap_or(0)
}

// ── Remote memory helpers ─────────────────────────────────────────────────────

fn remote_list_memories(
    base_url: &str,
    token: &str,
) -> Result<Vec<crate::memory::WorkspaceMemory>, String> {
    let url = format!("{}/memories", base_url);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.json().map_err(|e| e.to_string())
}

fn remote_get_memory_content(
    base_url: &str,
    token: &str,
    path: &str,
) -> Result<String, String> {
    let url = format!("{}/memory_content?path={}", base_url, encode_path(path));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.json::<String>().map_err(|e| e.to_string())
}

fn remote_get_memory_history(
    base_url: &str,
    token: &str,
    path: &str,
) -> Result<Vec<crate::memory::MemoryHistoryEntry>, String> {
    let url = format!("{}/memory_history?path={}", base_url, encode_path(path));
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.json().map_err(|e| e.to_string())
}

/// Spawn a background thread that polls `/tail` and emits `session-tail` events.
/// Returns an `Arc<Mutex<bool>>` stop flag. Set it to `false` to stop the thread.
pub fn start_remote_tail(
    base_url: String,
    token: String,
    jsonl_path: String,
    initial_offset: u64,
    app: AppHandle,
    tail_running: Arc<Mutex<bool>>,
) {
    std::thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let auth = format!("Bearer {}", token);
        let mut offset = initial_offset;

        while *tail_running.lock().unwrap() {
            std::thread::sleep(Duration::from_millis(500));
            let url = format!(
                "{}/tail?path={}&offset={}",
                base_url,
                encode_path(&jsonl_path),
                offset
            );
            if let Ok(resp) = client.get(&url).header("Authorization", &auth).send() {
                if let Ok(val) = resp.json::<serde_json::Value>() {
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
        }
    });
}
