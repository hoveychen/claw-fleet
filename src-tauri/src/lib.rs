mod session;
mod watcher;

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::Manager;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};

use session::SessionInfo;
use watcher::WatcherState;

// ── Account / Usage types ─────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct UsageStats {
    pub utilization: f64,
    pub resets_at: String,
    pub prev_utilization: Option<f64>,
}

// ── Usage snapshot history ────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Default)]
struct MetricSnap {
    utilization: f64,
    resets_at: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct SnapshotEntry {
    ts: i64, // unix timestamp in ms
    five_hour: Option<MetricSnap>,
    seven_day: Option<MetricSnap>,
    seven_day_sonnet: Option<MetricSnap>,
}

fn snapshot_path() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("claude-fleet-usage-history.json"))
}

fn load_snapshots() -> Vec<SnapshotEntry> {
    let path = match snapshot_path() {
        Some(p) => p,
        None => return vec![],
    };
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_snapshots(entries: &[SnapshotEntry]) {
    if let Some(path) = snapshot_path() {
        if let Ok(json) = serde_json::to_string(entries) {
            let _ = std::fs::write(path, json);
        }
    }
}

fn period_ms(metric: &str) -> i64 {
    match metric {
        "five_hour" => 5 * 3600 * 1000,
        _ => 7 * 24 * 3600 * 1000,
    }
}

fn parse_ts_ms(rfc3339: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

fn get_metric_snap<'a>(entry: &'a SnapshotEntry, metric: &str) -> Option<&'a MetricSnap> {
    match metric {
        "five_hour" => entry.five_hour.as_ref(),
        "seven_day" => entry.seven_day.as_ref(),
        "seven_day_sonnet" => entry.seven_day_sonnet.as_ref(),
        _ => None,
    }
}

/// Find what utilization the previous period had at the same relative elapsed fraction.
fn find_prev_utilization(
    history: &[SnapshotEntry],
    metric: &str,
    current_resets_at: &str,
    now_ms: i64,
) -> Option<f64> {
    let current_reset_ms = parse_ts_ms(current_resets_at)?;
    let pms = period_ms(metric);
    let current_start_ms = current_reset_ms - pms;
    let current_frac =
        ((now_ms - current_start_ms) as f64 / pms as f64).clamp(0.0, 1.0);

    // Collect distinct resets_at values from earlier periods
    let mut prev_resets: Vec<String> = history
        .iter()
        .filter_map(|e| get_metric_snap(e, metric))
        .filter(|m| m.resets_at != current_resets_at)
        .filter(|m| {
            parse_ts_ms(&m.resets_at)
                .map(|t| t < current_reset_ms)
                .unwrap_or(false)
        })
        .map(|m| m.resets_at.clone())
        .collect();
    prev_resets.sort();
    prev_resets.dedup();

    let prev_resets_at = prev_resets.last()?;
    let prev_reset_ms = parse_ts_ms(prev_resets_at)?;
    let prev_start_ms = prev_reset_ms - pms;

    // Among snapshots in that previous period, pick the one closest in elapsed fraction
    history
        .iter()
        .filter_map(|e| {
            let snap = get_metric_snap(e, metric)?;
            if &snap.resets_at != prev_resets_at {
                return None;
            }
            let frac = ((e.ts - prev_start_ms) as f64 / pms as f64).clamp(0.0, 1.0);
            Some((frac, snap.utilization))
        })
        .min_by(|(f1, _), (f2, _)| {
            (f1 - current_frac)
                .abs()
                .partial_cmp(&(f2 - current_frac).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(_, u)| u)
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct AccountInfo {
    pub email: String,
    pub full_name: String,
    pub organization_name: String,
    pub plan: String,
    pub auth_method: String,
    pub five_hour: Option<UsageStats>,
    pub seven_day: Option<UsageStats>,
    pub seven_day_sonnet: Option<UsageStats>,
}

#[cfg(target_os = "macos")]
fn read_keychain_credentials() -> Result<(String, String), String> {
    // Try keychain first (older Claude Code versions)
    let out = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
        .map_err(|e| format!("security command failed: {e}"))?;

    let raw = if out.status.success() {
        String::from_utf8(out.stdout).map_err(|e| e.to_string())?
    } else {
        // Fallback to credentials file (newer Claude Code versions)
        let cred_path = dirs::home_dir()
            .ok_or("No home dir")?
            .join(".claude")
            .join(".credentials.json");
        std::fs::read_to_string(&cred_path)
            .map_err(|_| "Credentials not found in keychain or file".to_string())?
    };

    let json: Value = serde_json::from_str(raw.trim()).map_err(|e| e.to_string())?;

    let oauth = json.get("claudeAiOauth").ok_or("No claudeAiOauth key")?;
    let token = oauth
        .get("accessToken")
        .and_then(|v| v.as_str())
        .ok_or("No accessToken")?
        .to_string();
    let sub = oauth
        .get("subscriptionType")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    Ok((token, sub))
}

#[cfg(not(target_os = "macos"))]
fn read_keychain_credentials() -> Result<(String, String), String> {
    let cred_path = dirs::home_dir()
        .ok_or("No home dir")?
        .join(".claude")
        .join(".credentials.json");
    let raw = std::fs::read_to_string(&cred_path)
        .map_err(|e| format!("{e} (tried: {})", cred_path.display()))?;
    let json: Value = serde_json::from_str(raw.trim()).map_err(|e| e.to_string())?;
    let oauth = json.get("claudeAiOauth").ok_or("No claudeAiOauth key")?;
    let token = oauth
        .get("accessToken")
        .and_then(|v| v.as_str())
        .ok_or("No accessToken")?
        .to_string();
    let sub = oauth
        .get("subscriptionType")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    Ok((token, sub))
}

#[tauri::command]
fn get_log_path() -> String {
    dirs::home_dir()
        .map(|h| h.join(".claude").join("claude-fleet-debug.log").to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

#[tauri::command]
fn get_platform() -> String {
    std::env::consts::OS.to_string()
}

fn log_debug(msg: &str) {
    if let Some(home) = dirs::home_dir() {
        let log_path = home.join(".claude").join("claude-fleet-debug.log");
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let line = format!("[{timestamp}] {msg}\n");
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
    }
}

fn parse_usage(v: &Value) -> Option<UsageStats> {
    let utilization = v.get("utilization")?.as_f64()?;
    let resets_at = v.get("resets_at")?.as_str().unwrap_or("").to_string();
    Some(UsageStats { utilization, resets_at, prev_utilization: None })
}

#[tauri::command]
fn get_account_info() -> Result<AccountInfo, String> {
    log_debug("get_account_info: start");

    let (token, subscription_type) = read_keychain_credentials().map_err(|e| {
        log_debug(&format!("get_account_info: keychain error: {e}"));
        e
    })?;
    log_debug(&format!(
        "get_account_info: credentials ok, subscription_type={subscription_type}"
    ));

    let client = reqwest::blocking::Client::new();
    let auth_header = format!("Bearer {}", token);
    let beta = "oauth-2025-04-20";

    let profile_raw = client
        .get("https://api.anthropic.com/api/oauth/profile")
        .header("Authorization", &auth_header)
        .header("anthropic-beta", beta)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .map_err(|e| {
            log_debug(&format!("get_account_info: profile request failed: {e}"));
            format!("Profile request failed: {e}")
        })?;
    let profile_status = profile_raw.status();
    let profile_body = profile_raw
        .json::<Value>()
        .map_err(|e| {
            log_debug(&format!("get_account_info: profile parse failed: {e}"));
            format!("Profile parse failed: {e}")
        })?;
    log_debug(&format!(
        "get_account_info: profile status={profile_status}, body={}",
        serde_json::to_string(&profile_body).unwrap_or_default()
    ));
    if !profile_status.is_success() {
        let msg = format!("Profile API error {profile_status}: {profile_body}");
        log_debug(&msg);
        return Err(msg);
    }
    let profile_resp = profile_body;

    let usage_raw = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", &auth_header)
        .header("anthropic-beta", beta)
        .header("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .map_err(|e| {
            log_debug(&format!("get_account_info: usage request failed: {e}"));
            format!("Usage request failed: {e}")
        })?;
    let usage_status = usage_raw.status();
    let usage_body = usage_raw
        .json::<Value>()
        .map_err(|e| {
            log_debug(&format!("get_account_info: usage parse failed: {e}"));
            format!("Usage parse failed: {e}")
        })?;
    log_debug(&format!(
        "get_account_info: usage status={usage_status}, body={}",
        serde_json::to_string(&usage_body).unwrap_or_default()
    ));
    if !usage_status.is_success() {
        let msg = format!("Usage API error {usage_status}: {usage_body}");
        log_debug(&msg);
        return Err(msg);
    }
    let usage_resp = usage_body;

    let email = profile_resp
        .pointer("/account/email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let full_name = profile_resp
        .pointer("/account/full_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let org_name = profile_resp
        .pointer("/organization/name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let has_max = profile_resp
        .pointer("/account/has_claude_max")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let has_pro = profile_resp
        .pointer("/account/has_claude_pro")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let plan = if has_max {
        "Claude Max".to_string()
    } else if has_pro || subscription_type == "pro" {
        "Claude Pro".to_string()
    } else {
        "API / Free".to_string()
    };

    let mut five_hour = usage_resp.get("five_hour").and_then(|v| parse_usage(v));
    let mut seven_day = usage_resp.get("seven_day").and_then(|v| parse_usage(v));
    let mut seven_day_sonnet = usage_resp
        .get("seven_day_sonnet")
        .and_then(|v| parse_usage(v));

    log_debug(&format!(
        "get_account_info: ok, five_hour={}, seven_day={}, seven_day_sonnet={}",
        five_hour.is_some(),
        seven_day.is_some(),
        seven_day_sonnet.is_some()
    ));

    // ── Persist snapshot and compute previous-period utilization ─────────────
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut history = load_snapshots();
    history.push(SnapshotEntry {
        ts: now_ms,
        five_hour: five_hour.as_ref().map(|s| MetricSnap {
            utilization: s.utilization,
            resets_at: s.resets_at.clone(),
        }),
        seven_day: seven_day.as_ref().map(|s| MetricSnap {
            utilization: s.utilization,
            resets_at: s.resets_at.clone(),
        }),
        seven_day_sonnet: seven_day_sonnet.as_ref().map(|s| MetricSnap {
            utilization: s.utilization,
            resets_at: s.resets_at.clone(),
        }),
    });
    if history.len() > 200 {
        let drain = history.len() - 200;
        history.drain(0..drain);
    }
    save_snapshots(&history);

    if let Some(ref mut s) = five_hour {
        let ra = s.resets_at.clone();
        s.prev_utilization = find_prev_utilization(&history, "five_hour", &ra, now_ms);
    }
    if let Some(ref mut s) = seven_day {
        let ra = s.resets_at.clone();
        s.prev_utilization = find_prev_utilization(&history, "seven_day", &ra, now_ms);
    }
    if let Some(ref mut s) = seven_day_sonnet {
        let ra = s.resets_at.clone();
        s.prev_utilization = find_prev_utilization(&history, "seven_day_sonnet", &ra, now_ms);
    }

    Ok(AccountInfo {
        email,
        full_name,
        organization_name: org_name,
        plan,
        auth_method: "claudeai".to_string(),
        five_hour,
        seven_day,
        seven_day_sonnet,
    })
}

// ── Process tree kill ─────────────────────────────────────────────────────────

/// Collect all PIDs in the process tree rooted at `root_pid` (BFS via ps output).
#[cfg(unix)]
fn collect_process_tree(root_pid: u32) -> Vec<u32> {
    let output = match std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![root_pid],
    };
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut children: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let pid: u32 = match parts.next().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        let ppid: u32 = match parts.next().and_then(|s| s.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        children.entry(ppid).or_default().push(pid);
    }

    let mut result = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(root_pid);
    while let Some(pid) = queue.pop_front() {
        result.push(pid);
        if let Some(kids) = children.get(&pid) {
            for &kid in kids {
                queue.push_back(kid);
            }
        }
    }
    result
}

#[cfg(unix)]
#[tauri::command]
fn kill_session(pid: u32) -> Result<(), String> {
    let pids = collect_process_tree(pid);
    log_debug(&format!(
        "kill_session: SIGTERM to {} pids (root={}): {:?}",
        pids.len(), pid, pids
    ));

    // SIGTERM children-first (reverse BFS order), then root
    for &p in pids.iter().rev() {
        unsafe { libc::kill(p as libc::pid_t, libc::SIGTERM) };
    }

    // Spawn background thread to SIGKILL survivors after 2 s
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(2000));
        for &p in pids.iter().rev() {
            let alive = unsafe { libc::kill(p as libc::pid_t, 0) } == 0;
            if alive {
                unsafe { libc::kill(p as libc::pid_t, libc::SIGKILL) };
            }
        }
    });

    Ok(())
}

#[cfg(not(unix))]
#[tauri::command]
fn kill_session(pid: u32) -> Result<(), String> {
    std::process::Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .status()
        .map_err(|e| format!("taskkill failed: {e}"))?;
    Ok(())
}

// ── App state ────────────────────────────────────────────────────────────────

pub struct AppState {
    pub sessions: Arc<Mutex<Vec<SessionInfo>>>,
    pub viewed_session: Arc<Mutex<Option<String>>>,
    pub viewed_offset: Arc<Mutex<u64>>,
}

// ── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
fn list_sessions(state: tauri::State<AppState>) -> Vec<SessionInfo> {
    state.sessions.lock().unwrap().clone()
}

#[tauri::command]
fn get_messages(jsonl_path: String) -> Result<Vec<Value>, String> {
    let content = std::fs::read_to_string(&jsonl_path).map_err(|e| e.to_string())?;
    let messages: Vec<Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    Ok(messages)
}

#[tauri::command]
fn start_watching_session(
    jsonl_path: String,
    state: tauri::State<AppState>,
) -> Result<u64, String> {
    let file_size = std::fs::metadata(&jsonl_path)
        .map(|m| m.len())
        .map_err(|e| e.to_string())?;

    *state.viewed_session.lock().unwrap() = Some(jsonl_path);
    *state.viewed_offset.lock().unwrap() = file_size;
    Ok(file_size)
}

#[tauri::command]
fn stop_watching_session(state: tauri::State<AppState>) {
    *state.viewed_session.lock().unwrap() = None;
    *state.viewed_offset.lock().unwrap() = 0;
}

// ── Tray helpers ─────────────────────────────────────────────────────────────

pub fn update_tray(app: &tauri::AppHandle, sessions: &[SessionInfo]) {
    use session::SessionStatus;

    let is_active = |s: &SessionInfo| matches!(
        s.status,
        SessionStatus::Streaming | SessionStatus::Processing |
        SessionStatus::WaitingInput | SessionStatus::Active
    );

    let main_count = sessions.iter().filter(|s| !s.is_subagent && is_active(s)).count();
    let sub_count = sessions.iter().filter(|s| s.is_subagent && is_active(s)).count();
    let total = main_count + sub_count;

    let tooltip = if total == 0 {
        "Claude Fleet".to_string()
    } else {
        format!(
            "Claude Fleet — {} active  (Main: {}  Sub: {})",
            total, main_count, sub_count
        )
    };

    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(&tooltip));
        #[cfg(target_os = "macos")]
        {
            let title = if total > 0 { format!("{}", total) } else { String::new() };
            let _ = tray.set_title(Some(&title));
        }
    }
}

// ── App setup ────────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let sessions: Arc<Mutex<Vec<SessionInfo>>> = Arc::new(Mutex::new(Vec::new()));
    let viewed_session: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let viewed_offset: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));

    let watcher_state = Arc::new(WatcherState {
        sessions: sessions.clone(),
        viewed_session: viewed_session.clone(),
        viewed_offset: viewed_offset.clone(),
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            sessions,
            viewed_session,
            viewed_offset,
        })
        .setup(move |app| {
            // ── Tray icon ────────────────────────────────────────────────────
            let show_item = MenuItemBuilder::new("Show").id("show").build(app)?;
            let hide_item = MenuItemBuilder::new("Hide").id("hide").build(app)?;
            let quit_item = MenuItemBuilder::new("Quit").id("quit").build(app)?;

            let tray_menu = MenuBuilder::new(app)
                .items(&[&show_item, &hide_item, &quit_item])
                .build()?;

            let icon = app.default_window_icon().cloned().unwrap();

            TrayIconBuilder::with_id("main")
                .icon(icon)
                .menu(&tray_menu)
                .tooltip("Claude Fleet")
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "hide" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { .. } = event {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            if w.is_visible().unwrap_or(false) {
                                let _ = w.set_focus();
                            } else {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            // ── Background watcher ───────────────────────────────────────────
            let handle = app.handle().clone();
            let ws = watcher_state.clone();
            std::thread::spawn(move || {
                watcher::run(handle, ws);
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            get_messages,
            start_watching_session,
            stop_watching_session,
            get_account_info,
            get_log_path,
            get_platform,
            kill_session,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
