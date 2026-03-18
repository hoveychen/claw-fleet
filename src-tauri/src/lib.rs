pub mod account;
pub mod remote;
pub mod session;
mod watcher;

use std::fs;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::Manager;
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};

use account::{AccountInfo, read_keychain_credentials};
use remote::ActiveRemote;
use session::SessionInfo;
use watcher::WatcherState;

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

// ── Setup status check ───────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
pub struct DetectedTools {
    pub cli: bool,
    pub vscode: bool,
    pub jetbrains: bool,
    pub desktop: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SetupStatus {
    pub cli_installed: bool,
    pub cli_path: Option<String>,
    pub claude_dir_exists: bool,
    pub detected_tools: DetectedTools,
    pub logged_in: bool,
    pub has_sessions: bool,
    pub credentials_valid: Option<bool>,
}

#[tauri::command]
fn check_setup_status(state: tauri::State<AppState>) -> SetupStatus {
    let (cli_installed, cli_path) = check_cli_installed();

    let claude_dir_exists = dirs::home_dir()
        .map(|h| h.join(".claude").is_dir())
        .unwrap_or(false);

    let detected_tools = detect_installed_tools(&state);
    let logged_in = read_keychain_credentials().is_ok();

    // Check if any sessions exist: try in-memory cache first (fast), then
    // fall back to a quick filesystem scan to avoid a race with the background
    // watcher thread that may not have finished its first pass yet.
    let has_sessions = if !state.sessions.lock().unwrap().is_empty() {
        true
    } else {
        session::get_claude_dir().map_or(false, |claude_dir| {
            let projects_dir = claude_dir.join("projects");
            fs::read_dir(&projects_dir).map_or(false, |entries| {
                entries.filter_map(|e| e.ok()).any(|workspace_entry| {
                    let workspace_dir = workspace_entry.path();
                    workspace_dir.is_dir()
                        && fs::read_dir(&workspace_dir).map_or(false, |files| {
                            files.filter_map(|e| e.ok()).any(|f| {
                                f.path().extension().and_then(|x| x.to_str()) == Some("jsonl")
                            })
                        })
                })
            })
        })
    };

    SetupStatus {
        cli_installed,
        cli_path,
        claude_dir_exists,
        detected_tools,
        logged_in,
        has_sessions,
        credentials_valid: None,
    }
}

fn detect_installed_tools(state: &tauri::State<AppState>) -> DetectedTools {
    let home = dirs::home_dir();

    // CLI: already checked via PATH / common paths
    let (cli, _) = check_cli_installed();

    // VS Code extension: check ~/.vscode/extensions/ and ~/.vscode-insiders/extensions/
    let vscode = home.as_ref().map_or(false, |h| {
        let ext_dirs = [
            h.join(".vscode").join("extensions"),
            h.join(".vscode-insiders").join("extensions"),
            h.join(".cursor").join("extensions"),
        ];
        ext_dirs.iter().any(|dir| {
            dir.is_dir() && fs::read_dir(dir).map_or(false, |entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.file_name().to_string_lossy().starts_with("anthropic.claude-code")
                })
            })
        })
    }) || {
        // Also check live IDE sessions for VS Code-like IDE names
        let sessions = state.sessions.lock().unwrap();
        sessions.iter().any(|s| {
            s.ide_name.as_deref().map_or(false, |name| {
                let n = name.to_lowercase();
                n.contains("vscode") || n.contains("vs code") || n.contains("cursor")
            })
        })
    };

    // JetBrains: check live sessions for JetBrains IDE names
    let jetbrains = {
        let sessions = state.sessions.lock().unwrap();
        sessions.iter().any(|s| {
            s.ide_name.as_deref().map_or(false, |name| {
                let n = name.to_lowercase();
                n.contains("intellij") || n.contains("webstorm") || n.contains("pycharm")
                    || n.contains("goland") || n.contains("rustrover") || n.contains("phpstorm")
                    || n.contains("rider") || n.contains("clion") || n.contains("jetbrains")
            })
        })
    };

    // Claude Desktop app
    let desktop = {
        #[cfg(target_os = "macos")]
        { std::path::Path::new("/Applications/Claude.app").exists() }
        #[cfg(target_os = "windows")]
        {
            std::env::var("LOCALAPPDATA").map_or(false, |appdata| {
                std::path::Path::new(&appdata).join("Programs").join("Claude").join("Claude.exe").exists()
            })
        }
        #[cfg(target_os = "linux")]
        { false }
    };

    DetectedTools { cli, vscode, jetbrains, desktop }
}

fn check_cli_installed() -> (bool, Option<String>) {
    // Try `which claude` (unix) or `where claude` (windows)
    #[cfg(unix)]
    let cmd = "which";
    #[cfg(not(unix))]
    let cmd = "where";

    if let Ok(output) = std::process::Command::new(cmd).arg("claude").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return (true, Some(path));
        }
    }

    // Also check common install locations
    let common_paths = [
        dirs::home_dir().map(|h| h.join(".npm-global").join("bin").join("claude")),
        dirs::home_dir().map(|h| h.join(".local").join("bin").join("claude")),
        Some(std::path::PathBuf::from("/usr/local/bin/claude")),
        Some(std::path::PathBuf::from("/opt/homebrew/bin/claude")),
    ];

    for path_opt in &common_paths {
        if let Some(path) = path_opt {
            if path.exists() {
                return (true, Some(path.to_string_lossy().to_string()));
            }
        }
    }

    (false, None)
}

#[tauri::command]
fn get_account_info() -> Result<AccountInfo, String> {
    log_debug("get_account_info: start");
    account::fetch_account_info().map_err(|e| {
        log_debug(&format!("get_account_info: error: {e}"));
        e
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
    /// Active remote connection, `None` when in local mode.
    pub remote: Arc<Mutex<Option<ActiveRemote>>>,
    /// `true` when connected to a remote — used by the local watcher to stay quiet.
    pub is_remote: Arc<Mutex<bool>>,
}

// ── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command]
fn list_sessions(state: tauri::State<AppState>) -> Vec<SessionInfo> {
    state.sessions.lock().unwrap().clone()
}

#[tauri::command]
fn get_messages(
    jsonl_path: String,
    state: tauri::State<AppState>,
) -> Result<Vec<Value>, String> {
    let remote_guard = state.remote.lock().unwrap();
    if let Some(ref active) = *remote_guard {
        // Remote mode: proxy to the probe
        return remote::remote_get_messages(&active.base_url, &active.token, &jsonl_path);
    }
    drop(remote_guard);

    // Local mode: read file directly
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
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
) -> Result<u64, String> {
    let remote_guard = state.remote.lock().unwrap();
    if let Some(ref active) = *remote_guard {
        // Remote mode: get file size from probe, then start tail poller
        let file_size = remote::remote_file_size(&active.base_url, &active.token, &jsonl_path);
        let tail_running = active.tail_running.clone();

        // Reset the flag so the new tail thread starts fresh
        *tail_running.lock().unwrap() = true;

        remote::start_remote_tail(
            active.base_url.clone(),
            active.token.clone(),
            jsonl_path.clone(),
            file_size,
            app,
            tail_running,
        );

        *state.viewed_session.lock().unwrap() = Some(jsonl_path);
        *state.viewed_offset.lock().unwrap() = file_size;
        return Ok(file_size);
    }
    drop(remote_guard);

    // Local mode
    let file_size = std::fs::metadata(&jsonl_path)
        .map(|m| m.len())
        .map_err(|e| e.to_string())?;
    *state.viewed_session.lock().unwrap() = Some(jsonl_path);
    *state.viewed_offset.lock().unwrap() = file_size;
    Ok(file_size)
}

#[tauri::command]
fn stop_watching_session(state: tauri::State<AppState>) {
    // Stop remote tail poller if running
    if let Some(ref active) = *state.remote.lock().unwrap() {
        *active.tail_running.lock().unwrap() = false;
    }
    *state.viewed_session.lock().unwrap() = None;
    *state.viewed_offset.lock().unwrap() = 0;
}

// ── CLI installer (macOS only) ───────────────────────────────────────────────

/// Create a symlink at /usr/local/bin/fleet pointing to the bundled fleet binary.
/// Requires the user to approve via osascript (admin password prompt).
#[tauri::command]
fn install_fleet_cli(app: tauri::AppHandle) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        // Tauri places externalBin sidecars next to the main executable
        let exe_dir = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .ok_or("no parent dir")?
            .to_path_buf();
        let fleet_bin = exe_dir.join("fleet");
        if !fleet_bin.exists() {
            return Err(format!("fleet binary not found at {}", fleet_bin.display()));
        }

        let target = "/usr/local/bin/fleet";
        let src = fleet_bin.to_string_lossy().to_string();

        // Use osascript to run with admin privileges
        let script = format!(
            r#"do shell script "mkdir -p /usr/local/bin && ln -sf '{}' '{}'" with administrator privileges"#,
            src, target
        );
        let status = std::process::Command::new("osascript")
            .args(["-e", &script])
            .status()
            .map_err(|e| e.to_string())?;

        if status.success() {
            Ok(target.to_string())
        } else {
            Err("Installation cancelled or failed".to_string())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("install_fleet_cli is only supported on macOS".to_string())
    }
}

// ── Skill installer ──────────────────────────────────────────────────────────

pub const FLEET_SKILL_MD: &str = include_str!("../../skills/fleet/SKILL.md");

/// Tools we know support the Agent Skills standard, keyed by their home dir name.
pub const SKILL_TARGETS: &[(&str, &str)] = &[
    ("Claude Code", ".claude"),
    ("GitHub Copilot", ".copilot"),
    ("Cursor", ".cursor"),
    ("Gemini CLI", ".gemini"),
];

#[derive(Serialize, Clone)]
struct DetectedTool {
    name: String,
    skill_path: String,
}

#[derive(Serialize)]
struct SkillInstallResult {
    installed: Vec<DetectedTool>,
    errors: Vec<String>,
}

fn home_dir() -> Result<std::path::PathBuf, String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .map_err(|_| "Cannot determine home directory".to_string())
}

/// Detect which AI tools are installed (by checking their home directories).
#[tauri::command]
fn detect_ai_tools() -> Result<Vec<DetectedTool>, String> {
    let home = home_dir()?;
    let detected = SKILL_TARGETS
        .iter()
        .filter(|(_, dir)| home.join(dir).exists())
        .map(|(name, dir)| DetectedTool {
            name: name.to_string(),
            skill_path: home
                .join(dir)
                .join("skills")
                .join("fleet")
                .join("SKILL.md")
                .to_string_lossy()
                .to_string(),
        })
        .collect();
    Ok(detected)
}

/// Open a native file-open dialog and return the chosen path.
#[tauri::command]
async fn pick_file(title: String) -> Option<String> {
    rfd::AsyncFileDialog::new()
        .set_title(&title)
        .pick_file()
        .await
        .map(|f| f.path().to_string_lossy().to_string())
}

/// Open a native save dialog and write SKILL.md to the chosen path.
#[tauri::command]
async fn save_skill_file() -> Result<String, String> {
    let handle = rfd::AsyncFileDialog::new()
        .set_file_name("SKILL.md")
        .set_title("Save Fleet Skill")
        .save_file()
        .await;

    match handle {
        Some(file) => {
            file.write(FLEET_SKILL_MD.as_bytes())
                .await
                .map_err(|e| e.to_string())?;
            Ok(file.path().to_string_lossy().to_string())
        }
        None => Err("cancelled".to_string()),
    }
}

/// Install the fleet skill to all detected AI tool directories.
#[tauri::command]
fn install_fleet_skill() -> Result<SkillInstallResult, String> {
    let home = home_dir()?;
    let mut installed = vec![];
    let mut errors = vec![];

    for (name, dir) in SKILL_TARGETS {
        let tool_home = home.join(dir);
        if !tool_home.exists() {
            continue;
        }
        let skill_dir = tool_home.join("skills").join("fleet");
        let skill_path = skill_dir.join("SKILL.md");
        match std::fs::create_dir_all(&skill_dir)
            .and_then(|_| std::fs::write(&skill_path, FLEET_SKILL_MD))
        {
            Ok(_) => installed.push(DetectedTool {
                name: name.to_string(),
                skill_path: skill_path.to_string_lossy().to_string(),
            }),
            Err(e) => errors.push(format!("{}: {}", name, e)),
        }
    }

    if installed.is_empty() && errors.is_empty() {
        errors.push(
            "No supported AI tools detected. Install Claude Code, Cursor, GitHub Copilot, or Gemini CLI first.".to_string(),
        );
    }

    Ok(SkillInstallResult { installed, errors })
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
    let is_remote: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));

    let watcher_state = Arc::new(WatcherState {
        sessions: sessions.clone(),
        viewed_session: viewed_session.clone(),
        viewed_offset: viewed_offset.clone(),
        is_remote: is_remote.clone(),
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            sessions,
            viewed_session,
            viewed_offset,
            remote: Arc::new(Mutex::new(None)),
            is_remote,
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
            check_setup_status,
            install_fleet_cli,
            detect_ai_tools,
            install_fleet_skill,
            save_skill_file,
            remote::list_saved_connections,
            remote::delete_connection,
            remote::connect_remote,
            remote::disconnect_remote,
            pick_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
