// GUI-specific code, only compiled with the "gui" feature.
//
// Extracted from lib.rs to avoid pulling tauri/image/rfd/notify into the
// fleet-cli probe binary.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, RwLock};

use std::sync::OnceLock;

use serde_json::Value;
use tauri::{Emitter, Manager};
use tauri::menu::{AboutMetadataBuilder, MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::tray::TrayIconBuilder;

use super::account::AccountInfo;
use super::backend::Backend;
use super::session::SessionInfo;
use super::*;

// ── Submodules (extracted command groups) ───────────────────────────────────
mod tts;
mod decision;
mod permissions;
mod notification;
mod setup;
mod process;
mod proc_runner;
mod sessions;
mod audit;
mod hooks;
mod guard;
mod elicitation;
mod plan_approval;
mod cli_installer;
mod memory;
mod wiki;
mod explorer;
mod scratchpad;
mod source_control;
mod skills;
mod plugins;
mod sources;
mod claude_bin;
mod locale;
mod alerts;
mod mascot;
mod llm;

use self::tts::*;
use self::decision::*;
use self::permissions::*;
use self::notification::*;
use self::setup::*;
use self::process::*;
use self::proc_runner::*;
use self::sessions::*;
use self::audit::*;
use self::hooks::*;
use self::guard::*;
use self::elicitation::*;
use self::plan_approval::*;
use self::cli_installer::*;
use self::memory::*;
use self::wiki::*;
use self::explorer::*;
use self::scratchpad::*;
use self::source_control::*;
use self::skills::*;
use self::plugins::*;
use self::sources::*;
use self::claude_bin::*;
use self::locale::*;
use self::alerts::*;
use self::mascot::*;
use self::llm::*;

pub(crate) use self::tts::play_tts_for_notification;


fn load_png_as_tray_icon(bytes: &[u8]) -> tauri::image::Image<'static> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .expect("failed to decode tray icon PNG")
        .into_rgba8();
    let (w, h) = img.dimensions();
    tauri::image::Image::new_owned(img.into_raw(), w, h)
}

#[tauri::command]
fn get_log_path() -> String {
    session::real_home_dir()
        .map(|h| h.join(".fleet").join("claw-fleet-debug.log").to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

#[tauri::command]
fn get_platform() -> String {
    std::env::consts::OS.to_string()
}

/// Reveal a path in the OS file manager (Finder / Explorer).
///
/// The `~` expansion happens here rather than in the webview: `reveal_item_in_dir`
/// does not accept `~`, and handing the home dir to the frontend just to rebuild
/// the path there would be a data round-trip for something the host already knows.
///
/// Local-only by nature — a remote workspace's files do not exist on this disk.
/// This is a shell action rather than a data-fetching capability, so it does not
/// belong on the Backend trait; the UI hides it when the connection is remote.
#[tauri::command]
fn reveal_path(app: tauri::AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let expanded = match path.strip_prefix("~/") {
        Some(rest) => session::real_home_dir()
            .ok_or_else(|| "home directory unknown".to_string())?
            .join(rest),
        None => std::path::PathBuf::from(&path),
    };
    if !expanded.exists() {
        return Err(format!("path does not exist: {}", expanded.display()));
    }
    app.opener()
        .reveal_item_in_dir(&expanded)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn check_app_version() -> version_check::VersionCheckResult {
    version_check::check_app_version()
}

#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The git commit this desktop binary was built from (baked by build.rs, see
/// `stamp_git_commit`). The 移动端 view compares it against each connected
/// phone's `appCommit` to flag a stale mobile bundle. Like `get_app_version`,
/// this is a compile-time constant of the running app — not backend data — so
/// it stays a plain command and is identical under Local/Remote backends.
/// `"unknown"` when no commit source was available at build time.
#[tauri::command]
fn desktop_build_commit() -> String {
    option_env!("FLEET_GIT_COMMIT").unwrap_or("unknown").to_string()
}

// ── App state ────────────────────────────────────────────────────────────────

pub struct AppState {
    /// The active backend (local or remote).  Swapped on connect/disconnect.
    /// Uses RwLock so read-only operations don't block each other (all Backend
    /// trait methods take &self).  Only the connect/disconnect swap needs a
    /// write lock.
    pub backend: Arc<RwLock<Box<dyn Backend>>>,
    /// User's current UI locale (e.g. "en", "zh"), shared with backend threads.
    pub locale: Arc<Mutex<String>>,
    /// Notification mode: "all" | "user_action" | "none".
    pub notification_mode: Arc<Mutex<String>>,
    /// How the assistant addresses the user (default "老板" / "Boss").
    pub user_title: Arc<Mutex<String>>,
    /// Cached sessions for tray menu rebuilds.
    pub cached_sessions: Arc<Mutex<Vec<SessionInfo>>>,
    /// Cached per-source usage summaries for tray menu display.
    pub cached_usage: Arc<Mutex<Vec<backend::SourceUsageSummary>>>,
    /// Fingerprint of the last tray menu content — skip rebuilds when unchanged
    /// to prevent the menu from closing while the user is interacting with it.
    pub tray_fingerprint: Arc<Mutex<u64>>,
    /// Timestamp of the last tray icon click.  While the menu is presumed open
    /// (within [`TRAY_MENU_GRACE_SECS`] of a click) we defer `set_menu` calls
    /// so macOS doesn't close the menu under the user's cursor.
    /// `None` = no tray click yet, treat as "long ago" (grace period
    /// expired). Avoids `Instant::now() - 600s` at app start, which
    /// panics on Windows machines with low uptime.
    pub tray_last_click: Arc<Mutex<Option<std::time::Instant>>>,
    /// Whether a deferred tray rebuild is pending.
    pub tray_rebuild_pending: Arc<Mutex<bool>>,
    /// LLM provider config (which CLI + models to use for analysis/reports).
    pub llm_config: Arc<Mutex<llm_provider::LlmConfig>>,
    /// Cached LLM provider info — pre-fetched at startup so Settings opens instantly.
    pub cached_llm_providers: Arc<Mutex<Vec<llm_provider::LlmProviderInfo>>>,
    /// Serialized snapshot of the current decision queue, seeded by the main
    /// window before it pops the decision-float. The float window reads this
    /// on mount to hydrate its local store before live events arrive.
    pub decision_float_snapshot: Arc<Mutex<Option<serde_json::Value>>>,
}

// ── App restart ─────────────────────────────────────────────────────────────

#[tauri::command]
fn restart_app(app: tauri::AppHandle) {
    app.restart();
}

#[tauri::command]
fn show_main_window(app: tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

// Lite portrait mode — shrink main window to phone-like portrait strip.
// We intentionally keep the native decorations (titleBarStyle: Overlay on
// macOS, default chrome elsewhere) because toggling set_decorations at
// runtime drops the Overlay style and the title bar can't be restored —
// that manifested as a broken title bar after exiting lite. Trade-off:
// traffic lights stay visible in lite mode, but we gain native rounded
// corners + correct restore.
#[tauri::command]
fn set_lite_mode(app: tauri::AppHandle, enabled: bool) {
    let Some(w) = app.get_webview_window("main") else { return };
    if enabled {
        let _ = w.set_min_size(Some(tauri::Size::Logical(tauri::LogicalSize::new(
            300.0, 520.0,
        ))));
        let _ = w.set_size(tauri::Size::Logical(tauri::LogicalSize::new(340.0, 720.0)));
        if let Ok(Some(monitor)) = w.current_monitor() {
            let size = monitor.size();
            let scale = monitor.scale_factor();
            let screen_w = size.width as f64 / scale;
            let x = screen_w - 360.0;
            let y = 40.0;
            let _ = w.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
        }
    } else {
        let _ = w.set_min_size(Some(tauri::Size::Logical(tauri::LogicalSize::new(
            900.0, 600.0,
        ))));
        let _ = w.set_size(tauri::Size::Logical(tauri::LogicalSize::new(1280.0, 820.0)));
        let _ = w.center();
    }
}

#[tauri::command]
fn toggle_tray_panel(_app: tauri::AppHandle, _visible: bool) {
    // No-op: custom tray panel removed; kept for frontend compat.
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

// ── Settings window ──────────────────────────────────────────────────────────

// Theme is applied on the builder so the native title bar starts in the
// right mode. A freshly-built window otherwise inherits the system
// NSAppearance, leaving a dark title bar on top of a light app body.
fn parse_theme(s: Option<&str>) -> Option<tauri::Theme> {
    match s? {
        "light" => Some(tauri::Theme::Light),
        "dark" => Some(tauri::Theme::Dark),
        _ => None,
    }
}

#[tauri::command]
fn open_settings_window(
    app: tauri::AppHandle,
    connection: Option<String>,
    theme: Option<String>,
) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("settings") {
        if let Some(t) = parse_theme(theme.as_deref()) {
            let _ = w.set_theme(Some(t));
        }
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return Ok(());
    }

    let mut path = String::from("settings.html");
    if let Some(conn) = connection.filter(|s| !s.is_empty()) {
        use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
        path.push_str("?connection=");
        path.push_str(&utf8_percent_encode(&conn, NON_ALPHANUMERIC).to_string());
    }

    let mut builder = tauri::WebviewWindowBuilder::new(
        &app,
        "settings",
        tauri::WebviewUrl::App(path.into()),
    )
    .title("Settings")
    .inner_size(780.0, 640.0)
    .min_inner_size(560.0, 480.0)
    .center();
    if let Some(t) = parse_theme(theme.as_deref()) {
        builder = builder.theme(Some(t));
    }
    let window = builder.build().map_err(|e| e.to_string())?;

    // Hide on close instead of destroying the WKWebView: tearing down a secondary
    // webview races with delayed WebKit main-thread work items (observed crash in
    // WebPageProxy::dispatchSetObscuredContentInsets on macOS 26.3.1).
    let hide_target = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = hide_target.hide();
        }
    });

    Ok(())
}

// ── Preview subwindow (lite-mode decision preview) ──────────────────────────

#[tauri::command]
fn open_preview_window(
    app: tauri::AppHandle,
    markdown: String,
    title: Option<String>,
    theme: Option<String>,
) -> Result<(), String> {
    // If already open, just push new content via event and bring to front.
    if let Some(w) = app.get_webview_window("preview") {
        if let Some(t) = parse_theme(theme.as_deref()) {
            let _ = w.set_theme(Some(t));
        }
        let _ = w.show();
        let _ = w.unminimize();
        let payload = serde_json::json!({
            "markdown": markdown,
            "title": title,
        });
        let _ = w.emit("preview://update", payload);
        return Ok(());
    }

    let mut path = String::from("preview.html");
    {
        use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
        path.push_str("?markdown=");
        path.push_str(&utf8_percent_encode(&markdown, NON_ALPHANUMERIC).to_string());
        if let Some(t) = title.as_deref().filter(|s| !s.is_empty()) {
            path.push_str("&title=");
            path.push_str(&utf8_percent_encode(t, NON_ALPHANUMERIC).to_string());
        }
    }

    let mut builder = tauri::WebviewWindowBuilder::new(
        &app,
        "preview",
        tauri::WebviewUrl::App(path.into()),
    )
    .title(title.as_deref().unwrap_or("Preview"))
    .inner_size(420.0, 520.0)
    .min_inner_size(280.0, 240.0)
    .resizable(true)
    .decorations(true)
    .always_on_top(true)
    .skip_taskbar(true);
    if let Some(t) = parse_theme(theme.as_deref()) {
        builder = builder.theme(Some(t));
    }

    // Position beside the main window when we can; otherwise let Tauri pick.
    // Tauri's builder.position() takes logical coords, so convert physical
    // -> logical using the main window's scale factor (HiDPI correctness).
    if let Some(main) = app.get_webview_window("main") {
        let scale = main.scale_factor().unwrap_or(1.0);
        if let (Ok(pos), Ok(size)) = (main.outer_position(), main.outer_size()) {
            let x = (pos.x as f64 + size.width as f64) / scale + 8.0;
            let y = pos.y as f64 / scale;
            builder = builder.position(x, y);
        }
    }

    let window = builder.build().map_err(|e| e.to_string())?;

    // Same WKWebView teardown-race workaround as the settings window: hide
    // instead of destroying so queued WebKit work items can't dereference a
    // freed WebPageProxy.
    let hide_target = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = hide_target.hide();
        }
    });

    Ok(())
}

#[tauri::command]
fn update_preview_content(
    app: tauri::AppHandle,
    markdown: String,
    title: Option<String>,
) -> Result<(), String> {
    let Some(w) = app.get_webview_window("preview") else {
        return Ok(());
    };
    let payload = serde_json::json!({
        "markdown": markdown,
        "title": title,
    });
    w.emit("preview://update", payload)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn close_preview_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("preview") {
        let _ = w.close();
    }
    Ok(())
}

// ── Decision float window (shown when main is minimized) ─────────────────────

const DECISION_FLOAT_LABEL: &str = "decision-float";
const DECISION_FLOAT_W: f64 = 480.0;
const DECISION_FLOAT_H: f64 = 380.0;
const DECISION_FLOAT_BOTTOM_MARGIN: f64 = 64.0;
const DECISION_FLOAT_MIN_W: f64 = 360.0;
const DECISION_FLOAT_MIN_H: f64 = 200.0;
const DECISION_FLOAT_MAX_H_RATIO: f64 = 0.7;
const DECISION_FLOAT_MAX_W_RATIO: f64 = 0.9;

/// macOS: visible work area (Dock + menu bar excluded) of the screen under the
/// cursor, expressed in Tauri's top-left logical coordinate space as
/// `(x_left, y_top, width, height)`. Returns `None` if AppKit yields no screens.
///
/// Tauri's `Monitor::size()` reports the full screen frame, which ignores the
/// Dock — so a bottom-anchored window computed from it slides behind the Dock.
/// `NSScreen.visibleFrame` is the only source that already subtracts the Dock
/// and menu bar, regardless of Dock edge, size, or auto-hide state.
#[cfg(target_os = "macos")]
fn macos_cursor_screen_visible_frame() -> Option<(f64, f64, f64, f64)> {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::{NSPoint, NSRect};

    unsafe {
        // Cursor in Cocoa global coords (origin = bottom-left of the main screen, y up).
        let mouse: NSPoint = msg_send![class!(NSEvent), mouseLocation];

        let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
        if screens.is_null() {
            return None;
        }
        let count: usize = msg_send![screens, count];
        if count == 0 {
            return None;
        }

        // screens[0] owns the global origin; its full height flips Cocoa's
        // y-up space into Tauri's y-down space.
        let primary: *mut AnyObject = msg_send![screens, objectAtIndex: 0usize];
        let primary_frame: NSRect = msg_send![primary, frame];
        let primary_h = primary_frame.size.height;

        // Pick the screen the cursor sits on; fall back to the primary screen.
        let mut chosen = primary;
        for i in 0..count {
            let s: *mut AnyObject = msg_send![screens, objectAtIndex: i];
            let f: NSRect = msg_send![s, frame];
            if mouse.x >= f.origin.x
                && mouse.x < f.origin.x + f.size.width
                && mouse.y >= f.origin.y
                && mouse.y < f.origin.y + f.size.height
            {
                chosen = s;
                break;
            }
        }

        let vf: NSRect = msg_send![chosen, visibleFrame];
        let x_left = vf.origin.x;
        // Cocoa top edge (y up) → Tauri top edge (y down from primary top).
        let y_top = primary_h - (vf.origin.y + vf.size.height);
        Some((x_left, y_top, vf.size.width, vf.size.height))
    }
}

/// Logical top-left position that anchors a `w × h` window at the bottom-center
/// of the monitor under the cursor, plus that monitor's logical width/height
/// so callers can clamp size against the screen. On macOS the anchor and size
/// bounds use the screen's *visible* work area (Dock + menu bar excluded);
/// elsewhere it uses the full monitor frame. Falls back to the primary monitor,
/// then to (120, 120) with screen size None.
fn decision_float_target_position_for(
    app: &tauri::AppHandle,
    w: f64,
    h: f64,
) -> (f64, f64, Option<(f64, f64)>) {
    // macOS: anchor against the visible work area so the window never slides
    // behind the Dock or under the menu bar.
    #[cfg(target_os = "macos")]
    if let Some((vx, vy, vw, vh)) = macos_cursor_screen_visible_frame() {
        let x = vx + (vw - w) / 2.0;
        let y = vy + vh - h - DECISION_FLOAT_BOTTOM_MARGIN;
        return (x, y, Some((vw, vh)));
    }

    let cursor = app.cursor_position().ok();
    let monitors = app.available_monitors().unwrap_or_default();

    let chosen = cursor.and_then(|c| {
        monitors.iter().find(|m| {
            let pos = m.position();
            let size = m.size();
            let x0 = pos.x as f64;
            let y0 = pos.y as f64;
            let x1 = x0 + size.width as f64;
            let y1 = y0 + size.height as f64;
            c.x >= x0 && c.x < x1 && c.y >= y0 && c.y < y1
        })
    }).or_else(|| app.primary_monitor().ok().flatten().and_then(|_| monitors.first()));

    if let Some(mon) = chosen {
        let scale = mon.scale_factor();
        let mon_x = mon.position().x as f64 / scale;
        let mon_y = mon.position().y as f64 / scale;
        let mon_w = mon.size().width as f64 / scale;
        let mon_h = mon.size().height as f64 / scale;
        let x = mon_x + (mon_w - w) / 2.0;
        let y = mon_y + mon_h - h - DECISION_FLOAT_BOTTOM_MARGIN;
        (x, y, Some((mon_w, mon_h)))
    } else {
        (120.0, 120.0, None)
    }
}

fn decision_float_target_position(app: &tauri::AppHandle) -> (f64, f64) {
    let (x, y, _) = decision_float_target_position_for(app, DECISION_FLOAT_W, DECISION_FLOAT_H);
    (x, y)
}

#[tauri::command]
fn show_decision_float(
    app: tauri::AppHandle,
    snapshot: serde_json::Value,
    state: tauri::State<AppState>,
) -> Result<(), String> {
    *state.decision_float_snapshot.lock().unwrap() = Some(snapshot);

    let (x, y) = decision_float_target_position(&app);

    if let Some(w) = app.get_webview_window(DECISION_FLOAT_LABEL) {
        let _ = w.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }

    tauri::WebviewWindowBuilder::new(
        &app,
        DECISION_FLOAT_LABEL,
        tauri::WebviewUrl::App("decision-float.html".into()),
    )
    .title("Fleet Decision")
    .inner_size(DECISION_FLOAT_W, DECISION_FLOAT_H)
    .min_inner_size(360.0, 280.0)
    .position(x, y)
    .resizable(true)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(true)
    .build()
    .map_err(|e| e.to_string())?;

    Ok(())
}

#[tauri::command]
fn hide_decision_float(app: tauri::AppHandle, state: tauri::State<AppState>) {
    *state.decision_float_snapshot.lock().unwrap() = None;
    if let Some(w) = app.get_webview_window(DECISION_FLOAT_LABEL) {
        let _ = w.hide();
    }
}

/// Resize the decision-float window to fit content. Either dimension may be
/// omitted (keeps current). Both are clamped against the min size and against
/// `MAX_*_RATIO` of the current monitor's logical extent. Re-anchors the
/// window to bottom-center so the float stays glued to the screen edge.
#[tauri::command]
fn resize_decision_float(
    app: tauri::AppHandle,
    width: Option<f64>,
    height: Option<f64>,
) -> Result<(), String> {
    let Some(window) = app.get_webview_window(DECISION_FLOAT_LABEL) else {
        return Ok(());
    };

    let scale = window.scale_factor().map_err(|e| e.to_string())?;
    let current = window.inner_size().map_err(|e| e.to_string())?;
    let cur_w = current.width as f64 / scale;
    let cur_h = current.height as f64 / scale;

    let mut new_w = width.unwrap_or(cur_w);
    let mut new_h = height.unwrap_or(cur_h);

    // Probe the cursor monitor for clamp bounds (also gives us the anchor pos).
    let (_, _, screen) = decision_float_target_position_for(&app, new_w, new_h);
    if let Some((mon_w, mon_h)) = screen {
        new_w = new_w.min((mon_w * DECISION_FLOAT_MAX_W_RATIO).round());
        new_h = new_h.min((mon_h * DECISION_FLOAT_MAX_H_RATIO).round());
    }
    new_w = new_w.max(DECISION_FLOAT_MIN_W);
    new_h = new_h.max(DECISION_FLOAT_MIN_H);

    let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize::new(new_w, new_h)));

    let (x, y, _) = decision_float_target_position_for(&app, new_w, new_h);
    let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));

    Ok(())
}

#[tauri::command]
fn get_decision_float_snapshot(state: tauri::State<AppState>) -> Option<serde_json::Value> {
    state.decision_float_snapshot.lock().unwrap().clone()
}

#[tauri::command]
fn is_main_window_minimized(app: tauri::AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|w| w.is_minimized().ok())
        .unwrap_or(false)
}

// ── Tray helpers ─────────────────────────────────────────────────────────────

fn status_label(s: &session::SessionStatus) -> &'static str {
    use session::SessionStatus::*;
    match s {
        Thinking => "thinking",
        Executing => "executing",
        Streaming => "streaming",
        Processing => "processing",
        WaitingInput => "waiting input",
        Active => "active",
        Delegating => "delegating",
        Idle => "idle",
        RateLimited => "rate limited",
        ServerErrored => "server error",
        Stuck => "stuck",
    }
}

fn is_session_active(s: &SessionInfo) -> bool {
    use session::SessionStatus;
    matches!(
        s.status,
        SessionStatus::Thinking | SessionStatus::Executing |
        SessionStatus::Streaming | SessionStatus::Processing |
        SessionStatus::WaitingInput | SessionStatus::Active |
        SessionStatus::Delegating
    )
}

pub fn update_tray(app: &tauri::AppHandle, sessions: &[SessionInfo]) {
    // Cache sessions for use by background usage refresh.
    let state = app.state::<AppState>();
    *state.cached_sessions.lock().unwrap() = sessions.to_vec();
    // Tray operations (set_menu, set_tooltip, set_title) touch NSStatusItem on
    // macOS and MUST run on the main thread.  This function is often called
    // from background scanner threads, so dispatch rather than calling directly.
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || rebuild_tray(&handle));
}

pub fn update_tray_usage(app: &tauri::AppHandle, summaries: Vec<backend::SourceUsageSummary>) {
    let state = app.state::<AppState>();
    *state.cached_usage.lock().unwrap() = summaries;
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || rebuild_tray(&handle));
}

/// How long after a tray click we assume the menu is still open and defer
/// rebuilds so macOS doesn't yank it away from the user.
const TRAY_MENU_GRACE_SECS: u64 = 15;

fn rebuild_tray(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let sessions = state.cached_sessions.lock().unwrap().clone();
    let summaries = state.cached_usage.lock().unwrap().clone();

    // Show all active sessions (main + subagents), sorted: main first, then subs.
    let mut active_all: Vec<&SessionInfo> = sessions.iter()
        .filter(|s| is_session_active(s))
        .collect();
    active_all.sort_by_key(|s| s.is_subagent);
    let active_main = &active_all; // alias for build_tray_menu signature
    let sub_count = active_all.iter().filter(|s| s.is_subagent).count();
    let total = active_all.len();

    // Compute a fingerprint of the tray content so we can skip redundant
    // menu rebuilds — calling set_menu() closes the menu if it is open.
    let fingerprint = {
        let mut h = DefaultHasher::new();
        total.hash(&mut h);
        sub_count.hash(&mut h);
        for s in active_main.iter() {
            s.workspace_name.hash(&mut h);
            s.is_subagent.hash(&mut h);
            status_label(&s.status).hash(&mut h);
        }
        for su in &summaries {
            su.source.hash(&mut h);
            for b in &su.bars {
                b.label.hash(&mut h);
                ((b.utilization * 10000.0) as u64).hash(&mut h);
            }
        }
        h.finish()
    };

    let prev = {
        let mut fp = state.tray_fingerprint.lock().unwrap();
        let old = *fp;
        *fp = fingerprint;
        old
    };

    // Update tooltip & title (cheap, won't close menu)
    let tooltip = if total == 0 {
        "Claw Fleet".to_string()
    } else {
        format!(
            "Claw Fleet — {} active  (Main: {}  Sub: {})",
            total, active_main.len(), sub_count
        )
    };

    let Some(tray) = app.tray_by_id("main") else { return };
    let _ = tray.set_tooltip(Some(&tooltip));
    #[cfg(target_os = "macos")]
    {
        let title = if total > 0 { format!("{}", total) } else { String::new() };
        let _ = tray.set_title(Some(&title));
    }

    // Only rebuild the menu when content actually changed.
    if fingerprint != prev {
        // If the menu is presumed open (recent tray click), defer the rebuild
        // so we don't close it under the user's cursor.
        let within_grace = state
            .tray_last_click
            .lock()
            .unwrap()
            .map_or(false, |t| t.elapsed() < std::time::Duration::from_secs(TRAY_MENU_GRACE_SECS));
        if within_grace {
            *state.tray_rebuild_pending.lock().unwrap() = true;
            return;
        }

        if let Ok(menu) = build_tray_menu(app, active_main, sub_count, total, &summaries) {
            let _ = tray.set_menu(Some(menu));
        }
        *state.tray_rebuild_pending.lock().unwrap() = false;
    }
}

/// Flush any deferred tray rebuild.  Called from a background timer once the
/// grace period after a tray click has expired.
fn flush_pending_tray_rebuild(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let pending = *state.tray_rebuild_pending.lock().unwrap();
    if !pending { return; }

    let within_grace = state
        .tray_last_click
        .lock()
        .unwrap()
        .map_or(false, |t| t.elapsed() < std::time::Duration::from_secs(TRAY_MENU_GRACE_SECS));
    if within_grace {
        return; // still within grace period
    }

    // Force a rebuild by resetting the fingerprint so the next call rebuilds.
    *state.tray_fingerprint.lock().unwrap() = 0;
    *state.tray_rebuild_pending.lock().unwrap() = false;
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || rebuild_tray(&handle));
}

/// Render a utilization value (0.0–1.0) as a percentage string, e.g. `45%`.
fn usage_pct_str(utilization: f64) -> String {
    let pct = (utilization * 100.0).round() as u32;
    format!("{}%", pct)
}

// ── App menu bar ────────────────────────────────────────────────────────────
//
// Builds the top-of-window (macOS) / in-window (Windows/Linux) menu bar.
// Custom items carry `menu-*` ids so they never collide with the tray menu's
// own ids. Predefined items (cut/copy/paste, quit, close, about…) are handled
// by the OS / webview directly and don't need a handler.
//
// Labels are locale-gated: when the frontend calls `set_locale`, we rebuild
// the menu so macOS/Win/Linux show the user's language.

struct MenuLabels {
    app_menu_title: &'static str,
    about_item: &'static str,
    settings: &'static str,
    check_updates: &'static str,
    services: &'static str,
    hide_self: &'static str,
    hide_others: &'static str,
    show_all: &'static str,
    quit: &'static str,

    file: &'static str,
    switch_connection: &'static str,
    daily_report: &'static str,
    close_window: &'static str,

    edit: &'static str,
    undo: &'static str,
    redo: &'static str,
    cut: &'static str,
    copy: &'static str,
    paste: &'static str,
    select_all: &'static str,

    view: &'static str,
    toggle_lite: &'static str,
    theme: &'static str,
    theme_system: &'static str,
    theme_light: &'static str,
    theme_dark: &'static str,
    reload: &'static str,
    fullscreen: &'static str,

    window: &'static str,
    minimize: &'static str,
    maximize: &'static str,

    help: &'static str,
    welcome: &'static str,
    report_issue: &'static str,
}

fn menu_labels(locale: &str) -> MenuLabels {
    if locale.starts_with("zh") {
        MenuLabels {
            app_menu_title: "Claw Fleet",
            about_item: "关于 Claw Fleet",
            settings: "设置…",
            check_updates: "检查更新…",
            services: "服务",
            hide_self: "隐藏 Claw Fleet",
            hide_others: "隐藏其他",
            show_all: "全部显示",
            quit: "退出 Claw Fleet",

            file: "文件",
            switch_connection: "切换连接",
            daily_report: "每日报告",
            close_window: "关闭窗口",

            edit: "编辑",
            undo: "撤销",
            redo: "重做",
            cut: "剪切",
            copy: "复制",
            paste: "粘贴",
            select_all: "全选",

            view: "视图",
            toggle_lite: "切换轻量模式",
            theme: "主题",
            theme_system: "跟随系统",
            theme_light: "亮色",
            theme_dark: "暗色",
            reload: "重新加载",
            fullscreen: "进入全屏",

            window: "窗口",
            minimize: "最小化",
            maximize: "最大化",

            help: "帮助",
            welcome: "欢迎向导",
            report_issue: "反馈问题…",
        }
    } else {
        MenuLabels {
            app_menu_title: "Claw Fleet",
            about_item: "About Claw Fleet",
            settings: "Settings…",
            check_updates: "Check for Updates…",
            services: "Services",
            hide_self: "Hide Claw Fleet",
            hide_others: "Hide Others",
            show_all: "Show All",
            quit: "Quit Claw Fleet",

            file: "File",
            switch_connection: "Switch Connection",
            daily_report: "Daily Report",
            close_window: "Close Window",

            edit: "Edit",
            undo: "Undo",
            redo: "Redo",
            cut: "Cut",
            copy: "Copy",
            paste: "Paste",
            select_all: "Select All",

            view: "View",
            toggle_lite: "Toggle Lite Mode",
            theme: "Theme",
            theme_system: "System",
            theme_light: "Light",
            theme_dark: "Dark",
            reload: "Reload",
            fullscreen: "Enter Full Screen",

            window: "Window",
            minimize: "Minimize",
            maximize: "Maximize",

            help: "Help",
            welcome: "Welcome",
            report_issue: "Report Issue…",
        }
    }
}

fn build_app_menu(
    app: &tauri::AppHandle,
    l: &MenuLabels,
) -> Result<tauri::menu::Menu<tauri::Wry>, tauri::Error> {
    // ── App submenu (macOS shows as "Claw Fleet"; ignored on Win/Linux) ─
    let about_meta = AboutMetadataBuilder::new()
        .name(Some("Claw Fleet"))
        .version(Some(env!("CARGO_PKG_VERSION")))
        .website(Some("https://github.com/hoveychen/claw-fleet"))
        .website_label(Some("GitHub"))
        .build();
    let about = PredefinedMenuItem::about(app, Some(l.about_item), Some(about_meta))?;

    let app_submenu = SubmenuBuilder::new(app, l.app_menu_title)
        .item(&about)
        .separator()
        .item(
            &MenuItemBuilder::new(l.settings)
                .id("menu-settings")
                .accelerator("CmdOrCtrl+,")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::new(l.check_updates)
                .id("menu-check-updates")
                .build(app)?,
        )
        .separator()
        .services_with_text(l.services)
        .separator()
        .hide_with_text(l.hide_self)
        .hide_others_with_text(l.hide_others)
        .show_all_with_text(l.show_all)
        .separator()
        .quit_with_text(l.quit)
        .build()?;

    // ── File ────────────────────────────────────────────────────────────
    let file_submenu = SubmenuBuilder::new(app, l.file)
        .item(
            &MenuItemBuilder::new(l.switch_connection)
                .id("menu-switch-connection")
                .accelerator("CmdOrCtrl+Shift+C")
                .build(app)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::new(l.daily_report)
                .id("menu-daily-report")
                .build(app)?,
        )
        .separator()
        .close_window_with_text(l.close_window)
        .build()?;

    // ── Edit (required for text inputs on macOS) ────────────────────────
    let edit_submenu = SubmenuBuilder::new(app, l.edit)
        .undo_with_text(l.undo)
        .redo_with_text(l.redo)
        .separator()
        .cut_with_text(l.cut)
        .copy_with_text(l.copy)
        .paste_with_text(l.paste)
        .separator()
        .select_all_with_text(l.select_all)
        .build()?;

    // ── View ────────────────────────────────────────────────────────────
    let theme_submenu = SubmenuBuilder::new(app, l.theme)
        .item(
            &MenuItemBuilder::new(l.theme_system)
                .id("menu-theme-system")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::new(l.theme_light)
                .id("menu-theme-light")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::new(l.theme_dark)
                .id("menu-theme-dark")
                .build(app)?,
        )
        .build()?;

    let view_submenu = SubmenuBuilder::new(app, l.view)
        .item(
            &MenuItemBuilder::new(l.toggle_lite)
                .id("menu-toggle-lite")
                .accelerator("CmdOrCtrl+Shift+L")
                .build(app)?,
        )
        .item(&theme_submenu)
        .separator()
        .item(
            &MenuItemBuilder::new(l.reload)
                .id("menu-reload")
                .accelerator("CmdOrCtrl+R")
                .build(app)?,
        )
        .fullscreen_with_text(l.fullscreen)
        .build()?;

    // ── Window ──────────────────────────────────────────────────────────
    let window_submenu = SubmenuBuilder::new(app, l.window)
        .minimize_with_text(l.minimize)
        .maximize_with_text(l.maximize)
        .build()?;

    // ── Help ────────────────────────────────────────────────────────────
    let help_submenu = SubmenuBuilder::new(app, l.help)
        .item(
            &MenuItemBuilder::new(l.welcome)
                .id("menu-welcome")
                .build(app)?,
        )
        .separator()
        .item(
            &MenuItemBuilder::new(l.report_issue)
                .id("menu-report-issue")
                .build(app)?,
        )
        .item(
            &MenuItemBuilder::new(l.check_updates)
                .id("menu-check-updates-help")
                .build(app)?,
        )
        .build()?;

    MenuBuilder::new(app)
        .item(&app_submenu)
        .item(&file_submenu)
        .item(&edit_submenu)
        .item(&view_submenu)
        .item(&window_submenu)
        .item(&help_submenu)
        .build()
}

/// Build and install the app menu using the current locale stored in
/// AppState. Called from `setup` (initial build) and `set_locale` (rebuild).
fn install_app_menu(app: &tauri::AppHandle) {
    let locale = {
        let state = app.state::<AppState>();
        let guard = state.locale.lock().unwrap();
        guard.clone()
    };
    let labels = menu_labels(&locale);
    match build_app_menu(app, &labels) {
        Ok(menu) => {
            let _ = app.set_menu(menu);
        }
        Err(e) => {
            eprintln!("failed to build app menu: {e}");
        }
    }
}

/// Handle an event fired by the app menu (distinct from the tray menu).
/// Returns `true` if the id was recognised and handled.
fn handle_app_menu_event(app: &tauri::AppHandle, id: &str) -> bool {
    match id {
        "menu-settings" => {
            // App menu has no theme context; mirror the main window's current
            // NSAppearance so the Settings titlebar matches.
            let theme = app
                .get_webview_window("main")
                .and_then(|w| w.theme().ok())
                .map(|t| match t {
                    tauri::Theme::Dark => "dark".to_string(),
                    _ => "light".to_string(),
                });
            let _ = open_settings_window(app.clone(), None, theme);
        }
        "menu-check-updates" | "menu-check-updates-help" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            let _ = app.emit("menu-check-updates", ());
        }
        "menu-switch-connection" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            let _ = app.emit("switch-connection", ());
        }
        "menu-daily-report" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            let _ = app.emit("menu-daily-report", ());
        }
        "menu-toggle-lite" => {
            let _ = app.emit("menu-toggle-lite", ());
        }
        "menu-theme-system" => {
            let _ = app.emit("menu-theme", "system");
        }
        "menu-theme-light" => {
            let _ = app.emit("menu-theme", "light");
        }
        "menu-theme-dark" => {
            let _ = app.emit("menu-theme", "dark");
        }
        "menu-reload" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.eval("window.location.reload()");
            }
        }
        "menu-welcome" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            let _ = app.emit("menu-welcome", ());
        }
        "menu-report-issue" => {
            use tauri_plugin_opener::OpenerExt;
            let _ = app
                .opener()
                .open_url("https://github.com/hoveychen/claw-fleet/issues", None::<&str>);
        }
        _ => return false,
    }
    true
}

fn build_tray_menu(
    app: &tauri::AppHandle,
    active_main: &[&SessionInfo],
    _sub_count: usize,
    total: usize,
    summaries: &[backend::SourceUsageSummary],
) -> Result<tauri::menu::Menu<tauri::Wry>, tauri::Error> {
    let mut builder = MenuBuilder::new(app);

    // ── Active agents section ────────────────────────────────────────────
    let header_text = if total > 0 {
        format!("{} Active Agent{}", total, if total == 1 { "" } else { "s" })
    } else {
        "No Active Agents".to_string()
    };
    builder = builder.item(
        &MenuItemBuilder::new(header_text).id("info-header").enabled(false).build(app)?
    );

    // List all active sessions (main + subagents), clickable to open detail.
    for (i, s) in active_main.iter().enumerate() {
        let prefix = if s.is_subagent { "  ↳ " } else { "" };
        let label = format!("{}{} — {}", prefix, s.workspace_name, status_label(&s.status));
        builder = builder.item(
            &MenuItemBuilder::new(label).id(format!("open-session-{}", i)).build(app)?
        );
    }

    builder = builder.item(&PredefinedMenuItem::separator(app)?);

    // ── Usage section (all sources) ─────────────────────────────────────
    if !summaries.is_empty() {
        for (idx, summary) in summaries.iter().enumerate() {
            if summary.bars.is_empty() {
                continue;
            }
            let parts: Vec<String> = summary.bars.iter()
                .map(|b| format!("{}\t{}", b.label, usage_pct_str(b.utilization)))
                .collect();
            let source_label = match summary.source.as_str() {
                "claude" => "Claude",
                "codex" => "Codex",
                other => other,
            };
            let line = format!("{}\t{}", source_label, parts.join("\t"));
            builder = builder.item(
                &MenuItemBuilder::new(line)
                    .id(format!("info-usage-{}", idx))
                    .enabled(true)
                    .build(app)?
            );
        }
        builder = builder.item(&PredefinedMenuItem::separator(app)?);
    }

    // ── Actions ──────────────────────────────────────────────────────────
    builder = builder.item(
        &MenuItemBuilder::new("Switch Connection").id("switch-connection").build(app)?
    );
    builder = builder.item(&PredefinedMenuItem::separator(app)?);
    builder = builder.item(
        &MenuItemBuilder::new("Quit").id("quit").build(app)?
    );

    builder.build()
}

// ── App setup ────────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Keep the consumer-heartbeat thread running when Fleet is backgrounded
    // on macOS (no-op on other platforms). See app_nap.rs for rationale.
    crate::app_nap::disable_app_nap();

    // Re-acquire the keep-awake assertion when the user left the toggle on
    // last run (no-op when disabled or unsupported). See keep_awake.rs.
    crate::keep_awake::restore_at_startup();

    // Workaround for WebKit2GTK DMA-BUF renderer hanging the GPU/compositor
    // under rapid input on Linux. Falls back to shared-memory rendering.
    // Must run before any WebView is initialized.
    #[cfg(target_os = "linux")]
    {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        // Serves wiki content into the webview: fleet-wiki://localhost/
        // <slug>/<version>/<relpath…> (http://fleet-wiki.localhost/… on
        // Windows). Routed through the Backend trait so a remote connection
        // transparently proxies bytes over the probe API. Asynchronous +
        // worker thread because RemoteBackend does blocking HTTP.
        .register_asynchronous_uri_scheme_protocol("fleet-wiki", move |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            std::thread::spawn(move || {
                let dec = |s: &str| {
                    percent_encoding::percent_decode_str(s)
                        .decode_utf8_lossy()
                        .to_string()
                };
                let path = request.uri().path().trim_start_matches('/').to_string();
                let mut segs = path.splitn(3, '/');
                let slug = dec(segs.next().unwrap_or(""));
                let version = dec(segs.next().unwrap_or(""));
                let rel = dec(segs.next().unwrap_or(""));
                // Scope the backend read lock so it's released before respond.
                let result = {
                    let state = app.state::<AppState>();
                    let backend = state.backend.read().unwrap();
                    backend.get_wiki_file(&slug, &version, &rel)
                };
                let response = match result {
                    Ok(f) => tauri::http::Response::builder()
                        .status(200)
                        .header("Content-Type", f.mime)
                        .header("Access-Control-Allow-Origin", "*")
                        .body(f.bytes)
                        .unwrap(),
                    Err(e) => tauri::http::Response::builder()
                        .status(404)
                        .header("Content-Type", "text/plain")
                        .body(e.into_bytes())
                        .unwrap(),
                };
                responder.respond(response);
            });
        })
        // Serves fleet__ask decision-card assets into the webview:
        // fleet-decision://localhost/<id>/q<idx>/<relpath…>
        // (http://fleet-decision.localhost/… on Windows). Same Backend-routed,
        // worker-thread shape as fleet-wiki:// so remote sessions proxy the
        // bytes over the probe API. Lets image-bearing cards load their
        // index.html + images without base64-inlining into the tool call.
        .register_asynchronous_uri_scheme_protocol("fleet-decision", move |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            std::thread::spawn(move || {
                let dec = |s: &str| {
                    percent_encoding::percent_decode_str(s)
                        .decode_utf8_lossy()
                        .to_string()
                };
                let path = request.uri().path().trim_start_matches('/').to_string();
                let mut segs = path.splitn(3, '/');
                let id = dec(segs.next().unwrap_or(""));
                let qidx = dec(segs.next().unwrap_or(""));
                let rel = dec(segs.next().unwrap_or(""));
                let result = {
                    let state = app.state::<AppState>();
                    let backend = state.backend.read().unwrap();
                    backend.get_decision_asset(&id, &qidx, &rel)
                };
                let response = match result {
                    Ok(f) => tauri::http::Response::builder()
                        .status(200)
                        .header("Content-Type", f.mime)
                        .header("Access-Control-Allow-Origin", "*")
                        .body(f.bytes)
                        .unwrap(),
                    Err(e) => tauri::http::Response::builder()
                        .status(404)
                        .header("Content-Type", "text/plain")
                        .body(e.into_bytes())
                        .unwrap(),
                };
                responder.respond(response);
            });
        })
        // Serves user-direction attachments (composer pastes, decision-panel
        // picks) into the webview so history can render them as thumbnails:
        // fleet-attachment://localhost/<key>/<name>
        // Same Backend-routed shape as fleet-decision:// above, so a remote
        // session proxies the bytes off the probe host — which is where the
        // agent, and therefore the stored attachment, actually lives.
        .register_asynchronous_uri_scheme_protocol("fleet-attachment", move |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            std::thread::spawn(move || {
                let dec = |s: &str| {
                    percent_encoding::percent_decode_str(s)
                        .decode_utf8_lossy()
                        .to_string()
                };
                let path = request.uri().path().trim_start_matches('/').to_string();
                let mut segs = path.splitn(2, '/');
                let key = dec(segs.next().unwrap_or(""));
                let name = dec(segs.next().unwrap_or(""));
                let result = {
                    let state = app.state::<AppState>();
                    let backend = state.backend.read().unwrap();
                    backend.get_user_attachment(&key, &name)
                };
                let response = match result {
                    Ok(f) => tauri::http::Response::builder()
                        .status(200)
                        .header("Content-Type", f.mime)
                        .header("Access-Control-Allow-Origin", "*")
                        .body(f.bytes)
                        .unwrap(),
                    Err(e) => tauri::http::Response::builder()
                        .status(404)
                        .header("Content-Type", "text/plain")
                        .body(e.into_bytes())
                        .unwrap(),
                };
                responder.respond(response);
            });
        });

    builder.manage(AppState {
            // NullBackend is a placeholder; replaced with LocalBackend in setup().
            backend: Arc::new(RwLock::new(Box::new(NullBackend) as Box<dyn Backend>)),
            locale: Arc::new(Mutex::new("en".to_string())),
            notification_mode: Arc::new(Mutex::new("user_action".to_string())),
            user_title: Arc::new(Mutex::new(String::new())),
            cached_sessions: Arc::new(Mutex::new(Vec::new())),
            cached_usage: Arc::new(Mutex::new(Vec::new())),
            tray_fingerprint: Arc::new(Mutex::new(0)),
            tray_last_click: Arc::new(Mutex::new(None)),
            tray_rebuild_pending: Arc::new(Mutex::new(false)),
            llm_config: Arc::new(Mutex::new(llm_provider::LlmConfig::load())),
            cached_llm_providers: Arc::new(Mutex::new(Vec::new())),
            decision_float_snapshot: Arc::new(Mutex::new(None)),
        })
        .setup(move |app| {
            // Windows: strip native chrome so the frontend's drag bar +
            // caption-button overlay can replace the OS title bar / system
            // menu. macOS keeps titleBarStyle: Overlay from tauri.conf.json.
            // Done at setup() (not in conf) because the option is
            // platform-conditional and Tauri's per-window `decorations`
            // toggle is the cleanest way to express that.
            #[cfg(target_os = "windows")]
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_decorations(false);
            }

            // Drop live-thinking sidecars left behind by finished turns. A
            // `claude --print` turn's sidecar goes stale as soon as it exits;
            // 6h is a generous ceiling that keeps ~/.fleet/live-thinking small
            // without racing any still-streaming session.
            claw_fleet_core::live_thinking::prune_old(6 * 60 * 60);

            // Replace NullBackend with the real LocalBackend now that AppHandle
            // is available.
            {
                let state = app.state::<AppState>();
                let locale = state.locale.clone();
                let llm_cfg = state.llm_config.clone();
                llm_provider::set_shared_config(llm_cfg.lock().unwrap().clone());

                // Build the agent source registry from config (~/.fleet/fleet-sources.json).
                let sources = agent_source::build_sources();

                let local = local_backend::LocalBackend::new(
                    app.handle().clone(),
                    locale,
                    llm_cfg,
                    sources,
                );
                *state.backend.write().unwrap() = Box::new(local);

                // Pre-fetch LLM provider info in background so Settings opens instantly.
                let cached = state.cached_llm_providers.clone();
                std::thread::spawn(move || {
                    let infos = llm_provider::all_provider_infos();
                    *cached.lock().unwrap() = infos;
                });
            }

            // Publish the bundled fleet CLI into ~/.fleet/bin, which
            // session_launch already prepends to every spawned agent's PATH.
            // Without this the directory stays empty and the agent's
            // `fleet plan …` calls — which the PRD-discipline guidance tells it
            // to make — only resolve on macOS, via the /usr/local/bin symlink
            // that the macOS-only installer command creates. Non-fatal: a
            // failure just means those calls won't resolve.
            if let Err(e) = claw_fleet_core::fleet_cli::ensure_fleet_cli_link() {
                claw_fleet_core::log_debug(&format!("ensure_fleet_cli_link failed: {e}"));
            }

            // Inject Fleet's permissions allowlist into ~/.claude/settings.json
            // so fleet guard becomes the sole audit gate. prune_dead_holders
            // inside acquire self-heals when a prior Fleet process died
            // without releasing.
            if claw_fleet_core::permissions_injector::load_config().enabled {
                if let Err(e) =
                    claw_fleet_core::permissions_injector::acquire(std::process::id())
                {
                    claw_fleet_core::log_debug(&format!(
                        "permissions_injector::acquire failed: {e}"
                    ));
                }
            }

            // Inject `mcpServers.fleet` into ~/.claude.json so Claude Code's
            // agent sees the `fleet__ask` MCP tool as soon as Fleet is up.
            // Same refcount / restore-on-last-release contract as the
            // permissions injector. Skipped when the fleet sibling binary
            // can't be located (dev runs without a built fleet-cli) — the
            // agent then falls back to native AskUserQuestion only.
            //
            // Debug-only: v2 fleet__ask has known UX gaps vs v1 (no preview,
            // form-style submit flow, every call prompts Claude Code for
            // permission, frequently lands as a deferred tool so agents
            // default to AskUserQuestion anyway). Release builds skip the
            // injection AND release() any pre-existing entry so users
            // upgrading from a dev build don't keep a stale mcpServers.fleet.
            if cfg!(debug_assertions)
                && claw_fleet_core::mcp_injector::load_config().enabled
            {
                match crate::fleet_binary::resolve_fleet_binary() {
                    Some(p) => {
                        let path_str = p.to_string_lossy().to_string();
                        if let Err(e) = claw_fleet_core::mcp_injector::acquire(
                            std::process::id(),
                            &path_str,
                        ) {
                            claw_fleet_core::log_debug(&format!(
                                "mcp_injector::acquire failed: {e}"
                            ));
                        }
                    }
                    None => {
                        claw_fleet_core::log_debug(
                            "[mcp_injector] fleet sibling binary not found; skipping injection",
                        );
                    }
                }
            } else if !cfg!(debug_assertions) {
                let _ = claw_fleet_core::mcp_injector::release(std::process::id());
            }

            // ── Injector drift watchdog ──────────────────────────────────
            // Every 30s, verify both injections are still present on disk
            // and re-write them if they've drifted (e.g. a Claude Code
            // upgrade rewrote ~/.claude.json from scratch). The watchdog
            // self-disables when there are no live holders, so it's safe
            // to start unconditionally even when one or both injectors
            // are toggled off — verify_and_reinject sees an empty holder
            // list and no-ops. Thread runs until process exit; no handle
            // to keep.
            {
                let fleet_path = crate::fleet_binary::resolve_fleet_binary()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "fleet".to_string());
                claw_fleet_core::injector_watchdog::start(fleet_path);
            }

            // ── Usage occupancy sampler ──────────────────────────────────
            // Sample the Claude usage API every 10 minutes while the desktop
            // app is running, regardless of which tab is open or whether the
            // usage panel's auto-refresh toggle is on. This is the always-on
            // host for local-only users who never run `fleet serve`, giving the
            // 24h occupancy chart continuous coverage. Idempotent per process;
            // errors are swallowed and retried on the next tick.
            claw_fleet_core::account::start_background_sampler(
                std::time::Duration::from_secs(600),
            );
            // Codex parallel: same 10-minute cadence, but each tick self-gates
            // on codex being installed (no wasted `codex app-server` spawns for
            // Claude-only users). Feeds the codex 占用率历史 chart.
            claw_fleet_core::codex_source::start_codex_background_sampler(
                std::time::Duration::from_secs(600),
            );


            // Truncate the hook events file if it has grown too large.
            crate::hooks::maybe_truncate_events_file();

            // ── Audit pattern updates ───────────────────────────────────────
            // Seed local patterns from bundled resource (first run or app
            // upgrade), then start the daily background updater.
            desktop_pattern_update::bootstrap_patterns(app.handle());
            pattern_update::start_background_updater();

            // Background usage refresh removed — the frontend's periodic
            // `get_source_usage` / `get_account_info` calls now update the
            // cached tray summaries as a side-effect, avoiding duplicate
            // network requests that could hit rate limits.

            // ── App menu bar ─────────────────────────────────────────────────
            // Register the main app menu (File / Edit / View / Window / Help …).
            // The global menu-event handler below dispatches custom items with
            // `menu-*` ids; tray items keep their own (tray-scoped) handler.
            // Labels come from the current locale (AppState::locale), which is
            // synced from the frontend on mount via `set_locale`; the menu is
            // rebuilt there whenever the user switches language.
            install_app_menu(app.handle());
            app.handle().on_menu_event(|app, event| {
                let id = event.id().as_ref().to_string();
                if id.starts_with("menu-") {
                    handle_app_menu_event(app, &id);
                }
            });

            // ── Tray icon ────────────────────────────────────────────────────
            // Build an initial menu; it will be rebuilt dynamically by rebuild_tray().
            let tray_menu = MenuBuilder::new(app)
                .item(&MenuItemBuilder::new("No Active Agents").id("info-header").enabled(false).build(app)?)
                .item(&PredefinedMenuItem::separator(app)?)
                .item(&MenuItemBuilder::new("Switch Connection").id("switch-connection").build(app)?)
                .item(&PredefinedMenuItem::separator(app)?)
                .item(&MenuItemBuilder::new("Quit").id("quit").build(app)?)
                .build()?;

            #[cfg(target_os = "macos")]
            let tray_builder = {
                let icon = load_png_as_tray_icon(include_bytes!("../../icons/tray-macos.png"));
                TrayIconBuilder::with_id("main")
                    .icon(icon)
                    .icon_as_template(true)
            };

            #[cfg(target_os = "windows")]
            let tray_builder = {
                let icon = load_png_as_tray_icon(include_bytes!("../../icons/tray-windows.png"));
                TrayIconBuilder::with_id("main")
                    .icon(icon)
            };

            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let tray_builder = {
                let icon = app.default_window_icon().cloned().unwrap();
                TrayIconBuilder::with_id("main")
                    .icon(icon)
            };

            tray_builder
                .menu(&tray_menu)
                .tooltip("Claw Fleet")
                .on_tray_icon_event(|tray, event| {
                    // Record click timestamp so we can defer tray menu rebuilds
                    // while the menu is open.
                    if let tauri::tray::TrayIconEvent::Click { button, button_state, .. } = &event {
                        if matches!(button_state, tauri::tray::MouseButtonState::Up) {
                            let app = tray.app_handle();
                            let state = app.state::<AppState>();
                            *state.tray_last_click.lock().unwrap() = Some(std::time::Instant::now());

                            // Left-click: show main window
                            if matches!(button, tauri::tray::MouseButton::Left) {
                                if let Some(w) = app.get_webview_window("main") {
                                    let _ = w.show();
                                    let _ = w.set_focus();
                                }
                            }
                        }
                    }
                })
                .on_menu_event(|app, event| {
                    let id = event.id();
                    let id_str = id.as_ref();
                    if id_str == "switch-connection" {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                        let _ = app.emit("switch-connection", ());
                    } else if id_str == "quit" {
                        app.exit(0);
                    } else if let Some(idx_str) = id_str.strip_prefix("open-session-") {
                        if let Ok(idx) = idx_str.parse::<usize>() {
                            let state = app.state::<AppState>();
                            let sessions = state.cached_sessions.lock().unwrap().clone();
                            let mut active: Vec<&SessionInfo> = sessions.iter()
                                .filter(|s| is_session_active(s))
                                .collect();
                            active.sort_by_key(|s| s.is_subagent);
                            if let Some(s) = active.get(idx) {
                                // Show the main window and emit the session to open.
                                if let Some(w) = app.get_webview_window("main") {
                                    let _ = w.show();
                                    let _ = w.set_focus();
                                }
                                let _ = app.emit("open-session", s.jsonl_path.clone());
                            }
                        }
                    }
                })
                .build(app)?;

            // Background thread to flush deferred tray rebuilds once the
            // grace period after a tray click has elapsed.
            {
                let app_handle = app.handle().clone();
                std::thread::spawn(move || {
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(TRAY_MENU_GRACE_SECS));
                        flush_pending_tray_rebuild(&app_handle);
                    }
                });
            }

            // ── Main window minimize watcher ─────────────────────────────────
            // Emit a frontend event whenever the main window's minimized state
            // may have changed, so the decision-float window can be shown /
            // hidden accordingly. Tauri has no dedicated "minimized" event, so
            // we re-check on Resized and Focused.
            if let Some(main_win) = app.get_webview_window("main") {
                let handle = app.handle().clone();
                main_win.on_window_event(move |event| {
                    use tauri::WindowEvent;
                    match event {
                        WindowEvent::Resized(_) | WindowEvent::Focused(_) => {
                            if let Some(w) = handle.get_webview_window("main") {
                                let minimized = w.is_minimized().unwrap_or(false);
                                let _ = handle.emit(
                                    "main-window-minimize-state-changed",
                                    minimized,
                                );
                            }
                        }
                        _ => {}
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            today_usage,
            today_usage_breakdown,
            search_sessions,
            get_messages,
            get_messages_tail,
            get_tool_result_full,
            get_skill_history,
            get_workflow_trees,
            get_task_token_breakdown,
            get_codex_token_breakdown,
            get_session_todos,
            get_audit_events,
            get_audit_rules,
            set_audit_rule_enabled,
            save_custom_audit_rule,
            delete_custom_audit_rule,
            suggest_audit_rules,
            check_pattern_update,
            get_pattern_info,
            start_watching_session,
            stop_watching_session,
            get_account_info,
            get_log_path,
            get_platform,
            reveal_path,
            check_app_version,
            get_app_version,
            desktop_build_commit,
            interrupt_session,
            kill_session,
            kill_workspace_sessions,
            resume_rate_limited_session,
            enqueue_session_message,
            spawn_new_claude_session,
            chat_workspace,
            browse_dir,
            get_auto_resume_config,
            set_auto_resume_config,
            set_session_mark,
            set_session_title,
            mark_sessions_read,
            list_workspace_procs,
            run_workspace_proc,
            kill_workspace_proc,
            read_workspace_proc_output,
            write_workspace_proc_input,
            resize_workspace_proc,
            clear_workspace_procs,
            keep_awake_supported,
            get_keep_awake,
            set_keep_awake,
            check_setup_status,
            install_fleet_cli,
            detect_ai_tools,
            install_fleet_skill,
            save_skill_file,
            remote::list_saved_connections,
            remote::list_ssh_profiles,
            remote::delete_connection,
            remote::connect_remote,
            remote::disconnect_remote,
            pick_file,
            get_source_account,
            get_source_usage,
            list_memories,
            get_memory_content,
            read_live_thinking,
            get_task_plans,
            get_memory_history,
            get_claude_md_content,
            promote_memory,
            list_wiki_docs,
            get_wiki_doc,
            get_handoff_chain,
            get_wiki_file_text,
            delete_wiki_doc,
            delete_wiki_version,
            move_wiki_doc,
            move_wiki_folder,
            delete_wiki_folder,
            search_wiki_docs,
            export_wiki_doc,
            list_explorer_roots,
            git_status,
            git_push,
            git_pull,
            list_explorer_dir,
            read_explorer_file,
            list_scratchpad_dir,
            read_scratchpad_file,
            list_skills,
            skill_sync_inventory,
            skill_sync_apply,
            skill_sync_adopt,
            skill_sync_unlink,
            get_skill_autosync,
            set_skill_autosync,
            get_skill_content,
            list_skill_files,
            delete_skill,
            list_plugins,
            set_plugin_enabled,
            install_plugin,
            uninstall_plugin,
            list_marketplaces,
            add_marketplace,
            remove_marketplace,
            get_waiting_alerts,
            set_locale,
            get_hooks_setup_plan,
            apply_hooks_setup,
            remove_hooks,
            apply_guard_hook,
            remove_guard_hook,
            respond_to_guard,
            list_guard_allow_rules,
            remove_guard_allow_rule,
            analyze_guard_command,
            get_guard_context,
            apply_elicitation_hook,
            remove_elicitation_hook,
            apply_interaction_mode,
            remove_interaction_mode,
            apply_wiki_guidance,
            remove_wiki_guidance,
            apply_model_guidance,
            remove_model_guidance,
            get_interaction_diagnostics,
            test_decision_frontend_only,
            test_decision_end_to_end,
            test_decision_via_claude_cli,
            test_fleet_ask_end_to_end,
            test_fleet_ask_via_claude_cli,
            apply_prd_mode,
            remove_prd_mode,
            reconcile_codex_guidance,
            respond_to_elicitation,
            respond_to_fleet_ask,
            respond_to_a2ui_render,
            respond_to_permission_prompt,
            apply_mcp_injector,
            upload_elicitation_attachment,
            stage_pasted_attachment,
            read_local_file_bytes,
            apply_plan_approval_hook,
            remove_plan_approval_hook,
            list_pending_plan_approvals,
            respond_to_plan_approval,
            list_session_decisions,
            list_pending_decisions,
            get_mobile_relay_config,
            set_mobile_relay_config,
            rotate_mobile_relay_secret,
            mobile_relay_status,
            mobile_relay_qr_svg,
            generate_mascot_quips,
            list_llm_providers,
            get_llm_config,
            set_llm_config,
            list_fleet_llm_usage_daily,
            get_usage_history,
            get_codex_usage_history,
            get_sources_config,
            set_source_enabled,
            list_claude_binaries,
            get_claude_binary_override,
            set_claude_binary_override,
            restart_app,
            get_notification_mode,
            set_notification_mode,
            get_decision_panel_config,
            set_decision_panel_config,
            get_permissions_config,
            set_permissions_config,
            get_user_title,
            set_user_title,
            open_notification_settings,
            show_main_window,
            set_lite_mode,
            crate::traffic_lights::nudge_traffic_lights,
            toggle_tray_panel,
            quit_app,
            open_settings_window,
            open_preview_window,
            update_preview_content,
            close_preview_window,
            show_decision_float,
            hide_decision_float,
            resize_decision_float,
            get_decision_float_snapshot,
            is_main_window_minimized,
            get_tts_voices,
            speak_text,
            speak_text_say,
            get_daily_report,
            list_daily_report_stats,
            generate_daily_report,
            generate_daily_report_ai_summary,
            generate_daily_report_lessons,
            append_lesson_to_claude_md,
            list_managed_lessons,
            remove_managed_lesson,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            // Deregister this pid from both injector locks on every exit path.
            // Unconditional — each is a no-op when no lock exists, so they
            // self-heal if the toggle was flipped off mid-run.
            //
            // Note the asymmetry with setup()'s acquires: permissions_injector
            // deliberately leaves ~/.claude/settings.json injected, because the
            // claude sessions we spawned are detached and keep running after we
            // quit — pulling the allow rules would strand them on permission
            // prompts nothing is left to answer. Only the settings-panel toggle
            // un-injects, via permissions_injector::deactivate().
            if matches!(event, tauri::RunEvent::Exit) {
                let _ = claw_fleet_core::permissions_injector::release(
                    std::process::id(),
                );
                let _ = claw_fleet_core::mcp_injector::release(
                    std::process::id(),
                );
            }
        });
}
