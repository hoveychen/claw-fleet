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
    let out = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
        .map_err(|e| format!("security command failed: {e}"))?;

    if !out.status.success() {
        return Err("Credentials not found in keychain".to_string());
    }

    let raw = String::from_utf8(out.stdout).map_err(|e| e.to_string())?;
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
    let raw = std::fs::read_to_string(&cred_path).map_err(|e| e.to_string())?;
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

fn parse_usage(v: &Value) -> Option<UsageStats> {
    let utilization = v.get("utilization")?.as_f64()?;
    let resets_at = v.get("resets_at")?.as_str().unwrap_or("").to_string();
    Some(UsageStats { utilization, resets_at })
}

#[tauri::command]
fn get_account_info() -> Result<AccountInfo, String> {
    let (token, subscription_type) = read_keychain_credentials()?;

    let client = reqwest::blocking::Client::new();
    let auth_header = format!("Bearer {}", token);
    let beta = "oauth-2025-04-20";

    let profile_resp = client
        .get("https://api.anthropic.com/api/oauth/profile")
        .header("Authorization", &auth_header)
        .header("anthropic-beta", beta)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .map_err(|e| format!("Profile request failed: {e}"))?
        .json::<Value>()
        .map_err(|e| format!("Profile parse failed: {e}"))?;

    let usage_resp = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", &auth_header)
        .header("anthropic-beta", beta)
        .header("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .map_err(|e| format!("Usage request failed: {e}"))?
        .json::<Value>()
        .map_err(|e| format!("Usage parse failed: {e}"))?;

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

    let five_hour = usage_resp.get("five_hour").and_then(|v| parse_usage(v));
    let seven_day = usage_resp.get("seven_day").and_then(|v| parse_usage(v));
    let seven_day_sonnet = usage_resp
        .get("seven_day_sonnet")
        .and_then(|v| parse_usage(v));

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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
