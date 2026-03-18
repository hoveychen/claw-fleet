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
use tauri::{AppHandle, Emitter};

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
    /// Local port that will be forwarded via the SSH tunnel.
    #[serde(default = "default_probe_port")]
    pub probe_port: u16,
}

fn default_probe_port() -> u16 {
    7007
}

// ── Runtime state of an active remote connection ──────────────────────────────

pub struct ActiveRemote {
    pub connection: RemoteConnection,
    /// `http://127.0.0.1:<probe_port>`
    pub base_url: String,
    pub token: String,
    /// The local `ssh -N -L …` tunnel child process.
    pub tunnel_child: std::process::Child,
    /// PID of the `fleet serve` process on the remote host.
    pub remote_probe_pid: Option<u32>,
    /// Set to `false` to stop the sessions-poller thread.
    pub poller_running: Arc<Mutex<bool>>,
    /// Set to `false` to stop the tail-poller thread.
    pub tail_running: Arc<Mutex<bool>>,
}

impl Drop for ActiveRemote {
    fn drop(&mut self) {
        *self.poller_running.lock().unwrap() = false;
        *self.tail_running.lock().unwrap() = false;
        let _ = self.tunnel_child.kill();
    }
}

// ── Progress event emitted to the frontend during connect ────────────────────

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConnectProgress {
    pub step: String,
    pub done: bool,
    pub error: Option<String>,
}

fn emit_progress(app: &AppHandle, step: &str, done: bool, error: Option<&str>) {
    let _ = app.emit(
        "remote-connect-progress",
        ConnectProgress {
            step: step.to_string(),
            done,
            error: error.map(|s| s.to_string()),
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
        "-p".to_string(),
        conn.port.to_string(),
    ];
    if let Some(ref key) = conn.identity_file {
        args.push("-i".to_string());
        args.push(key.clone());
    }
    args.push(format!("{}@{}", conn.username, conn.host));
    args
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

#[tauri::command]
pub fn connect_remote(
    conn: RemoteConnection,
    app: AppHandle,
    state: tauri::State<crate::AppState>,
) -> Result<(), String> {
    // ── Step 1: verify SSH connectivity ──────────────────────────────────────
    emit_progress(&app, "Connecting via SSH…", false, None);
    ssh_exec(&conn, "echo ok").map_err(|e| {
        emit_progress(&app, "SSH connection failed", false, Some(&e));
        e
    })?;

    // ── Step 2: check remote version ─────────────────────────────────────────
    emit_progress(&app, "Checking remote fleet version…", false, None);
    let current_version = env!("CARGO_PKG_VERSION");
    let remote_ver_out = ssh_exec(
        &conn,
        &format!("{} --version 2>/dev/null || echo NOT_FOUND", remote_fleet_path()),
    )
    .unwrap_or_else(|_| "NOT_FOUND".to_string());

    let needs_upload = remote_ver_out.contains("NOT_FOUND")
        || !remote_ver_out.contains(current_version);

    // ── Step 3: upload binary if needed ──────────────────────────────────────
    if needs_upload {
        emit_progress(&app, "Uploading fleet binary to remote…", false, None);
        let local_bin = find_local_fleet_binary()
            .ok_or_else(|| "Cannot find local fleet binary to upload".to_string())?;

        // Ensure remote directory exists
        ssh_exec(&conn, "mkdir -p ~/.fleet-probe").map_err(|e| {
            emit_progress(&app, "Failed to create remote directory", false, Some(&e));
            e
        })?;

        // SCP upload
        let mut scp_args: Vec<String> = vec![
            "-o".to_string(),
            "StrictHostKeyChecking=accept-new".to_string(),
            "-o".to_string(),
            "ConnectTimeout=30".to_string(),
            "-P".to_string(),
            conn.port.to_string(),
        ];
        if let Some(ref key) = conn.identity_file {
            scp_args.push("-i".to_string());
            scp_args.push(key.clone());
        }
        scp_args.push(local_bin.to_string_lossy().to_string());
        scp_args.push(format!(
            "{}@{}:{}",
            conn.username,
            conn.host,
            remote_fleet_path()
        ));

        let scp_out = std::process::Command::new("scp")
            .args(&scp_args)
            .output()
            .map_err(|e| format!("scp failed: {e}"))?;

        if !scp_out.status.success() {
            let err = String::from_utf8_lossy(&scp_out.stderr).to_string();
            emit_progress(&app, "Binary upload failed", false, Some(&err));
            return Err(err);
        }

        // Make executable
        ssh_exec(&conn, &format!("chmod +x {}", remote_fleet_path())).map_err(|e| {
            emit_progress(&app, "chmod failed", false, Some(&e));
            e
        })?;
    } else {
        emit_progress(&app, "Remote fleet binary up to date, skipping upload.", false, None);
    }

    // ── Step 4: start remote probe ───────────────────────────────────────────
    emit_progress(&app, "Starting remote fleet probe…", false, None);
    let token = generate_token();
    let probe_port = conn.probe_port;

    // Kill any stale probe on the same port before starting
    let _ = ssh_exec(
        &conn,
        &format!(
            "pkill -f 'fleet serve --port {}' 2>/dev/null; sleep 0.3",
            probe_port
        ),
    );

    let start_cmd = format!(
        "nohup {} serve --port {} --token {} >/tmp/fleet-probe.log 2>&1 & echo $!",
        remote_fleet_path(),
        probe_port,
        token
    );
    let pid_str = ssh_exec(&conn, &start_cmd).map_err(|e| {
        emit_progress(&app, "Failed to start remote probe", false, Some(&e));
        e
    })?;
    let remote_probe_pid: Option<u32> = pid_str.trim().parse().ok();

    // ── Step 5: start local SSH tunnel ───────────────────────────────────────
    emit_progress(&app, "Creating SSH tunnel…", false, None);

    let mut tunnel_args: Vec<String> = vec![
        "-N".to_string(),
        "-L".to_string(),
        format!("{}:127.0.0.1:{}", probe_port, probe_port),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        "-o".to_string(),
        "ConnectTimeout=15".to_string(),
        "-o".to_string(),
        "ServerAliveInterval=30".to_string(),
        "-o".to_string(),
        "ExitOnForwardFailure=yes".to_string(),
        "-p".to_string(),
        conn.port.to_string(),
    ];
    if let Some(ref key) = conn.identity_file {
        tunnel_args.push("-i".to_string());
        tunnel_args.push(key.clone());
    }
    tunnel_args.push(format!("{}@{}", conn.username, conn.host));

    let tunnel_child = std::process::Command::new("ssh")
        .args(&tunnel_args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to start SSH tunnel: {e}"))?;

    // ── Step 6: wait for probe to be ready ───────────────────────────────────
    emit_progress(&app, "Waiting for probe to be ready…", false, None);
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
        // Clean up tunnel
        let mut tc = tunnel_child;
        let _ = tc.kill();
        let err = "Probe did not become ready within 10 seconds";
        emit_progress(&app, err, false, Some(err));
        return Err(err.to_string());
    }

    // ── Step 7: save connection & start background poller ────────────────────
    // Persist the connection (upsert by id)
    let mut saved = load_saved_connections();
    if let Some(existing) = saved.iter_mut().find(|c| c.id == conn.id) {
        *existing = conn.clone();
    } else {
        saved.push(conn.clone());
    }
    let _ = save_connections(&saved);

    let poller_running = Arc::new(Mutex::new(true));
    let tail_running = Arc::new(Mutex::new(true));

    // Set remote mode in AppState
    {
        let active = ActiveRemote {
            connection: conn.clone(),
            base_url: base_url.clone(),
            token: token.clone(),
            tunnel_child,
            remote_probe_pid,
            poller_running: poller_running.clone(),
            tail_running: tail_running.clone(),
        };
        *state.remote.lock().unwrap() = Some(active);
        *state.is_remote.lock().unwrap() = true;
    }

    // Spawn sessions-poller thread
    {
        let sessions_arc = state.sessions.clone();
        let app2 = app.clone();
        let pr = poller_running.clone();
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
                            if let Ok(sessions) =
                                serde_json::from_str::<Vec<SessionInfo>>(&body)
                            {
                                *sessions_arc.lock().unwrap() = sessions.clone();
                                let _ = app2.emit("sessions-updated", &sessions);
                                crate::update_tray(&app2, &sessions);
                            }
                        }
                    }
                }
            }
        });
    }

    emit_progress(
        &app,
        &format!("Connected to {}@{}", conn.username, conn.host),
        true,
        None,
    );
    Ok(())
}

// ── Disconnect ────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn disconnect_remote(
    state: tauri::State<crate::AppState>,
    app: AppHandle,
) -> Result<(), String> {
    let mut remote_guard = state.remote.lock().unwrap();
    if let Some(active) = remote_guard.take() {
        // Stop background threads
        *active.poller_running.lock().unwrap() = false;
        *active.tail_running.lock().unwrap() = false;

        // Kill the SSH tunnel (Drop would also do this, but be explicit)
        let conn = active.connection.clone();
        let probe_pid = active.remote_probe_pid;
        drop(active); // kills tunnel child via Drop

        // Kill the remote probe process
        if let Some(pid) = probe_pid {
            let _ = ssh_exec(&conn, &format!("kill {} 2>/dev/null", pid));
        }
        // Also try pkill by name as a fallback
        let _ = ssh_exec(
            &conn,
            &format!(
                "pkill -f 'fleet serve --port {}' 2>/dev/null",
                conn.probe_port
            ),
        );
    }

    *state.is_remote.lock().unwrap() = false;

    // Trigger a local rescan so the UI reflects local sessions again
    if let Some(claude_dir) = crate::session::get_claude_dir() {
        let sessions = crate::session::scan_sessions(&claude_dir);
        *state.sessions.lock().unwrap() = sessions.clone();
        let _ = app.emit("sessions-updated", &sessions);
        crate::update_tray(&app, &sessions);
    }

    Ok(())
}

// ── HTTP proxy helpers (used by lib.rs mode-aware commands) ──────────────────

/// Encode a path for use in a query parameter.
pub fn encode_path(path: &str) -> String {
    utf8_percent_encode(path, NON_ALPHANUMERIC).to_string()
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
