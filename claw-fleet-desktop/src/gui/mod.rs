// GUI-specific code, only compiled with the "gui" feature.
//
// Extracted from lib.rs to avoid pulling tauri/image/rfd/notify into the
// fleet-cli probe binary.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use std::sync::OnceLock;

use serde_json::Value;
use tauri::menu::{
    AboutMetadataBuilder, MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder,
};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager};

use super::account::AccountInfo;

use super::session::SessionInfo;
use super::*;

// ── Submodules (extracted command groups) ───────────────────────────────────
mod decision;
mod notification;
mod permissions;
mod setup;
mod tts;
// Remote harness install (remote.rs) emits the same progress event shape.
pub(crate) use setup::HarnessInstallProgress;
mod alerts;
mod artifacts;
mod audit;
mod claude_bin;
mod cli_installer;
mod elicitation;
mod explorer;
mod guard;
mod hooks;
mod llm;
mod locale;
mod mascot;
mod memory;
mod plan_approval;
mod plugins;
mod proc_runner;
mod process;
mod schedule;
mod scratchpad;
mod sessions;
mod skills;
mod source_control;
mod sources;
mod url_embed;
mod wiki;

use self::alerts::*;
use self::artifacts::*;
use self::audit::*;
use self::claude_bin::*;
use self::cli_installer::*;
use self::decision::*;
use self::elicitation::*;
use self::explorer::*;
use self::guard::*;
use self::hooks::*;
use self::llm::*;
use self::locale::*;
use self::mascot::*;
use self::memory::*;
use self::notification::*;
use self::permissions::*;
use self::plan_approval::*;
use self::plugins::*;
use self::proc_runner::*;
use self::process::*;
use self::schedule::*;
use self::scratchpad::*;
use self::sessions::*;
use self::setup::*;
use self::skills::*;
use self::source_control::*;
use self::sources::*;
use self::tts::*;
use self::url_embed::*;
use self::wiki::*;

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
        .map(|h| {
            h.join(".fleet")
                .join("claw-fleet-debug.log")
                .to_string_lossy()
                .to_string()
        })
        .unwrap_or_else(|| "unknown".to_string())
}

#[tauri::command]
fn get_platform() -> String {
    std::env::consts::OS.to_string()
}

/// Append a line to `~/.fleet/claw-fleet-debug.log` on behalf of the webview.
///
/// The webview has no file access and a release build has no devtools, so a UI
/// fault that only reproduces on the user's machine (the conversation pane
/// freezing mid-scroll) otherwise leaves no trace at all. This lets the
/// frontend's own probes land in the same log the host writes, interleaved with
/// the host-side events that explain them.
///
/// Like `reveal_path`, this is a host action rather than a data-fetching
/// capability, so it is a plain command rather than a `LocalBackend` method: it
/// records the state of *this* webview.
#[tauri::command]
fn log_frontend_debug(msg: String) {
    // Bound the line so a runaway caller can't grow the log without limit.
    let trimmed: String = msg.chars().take(2000).collect();
    claw_fleet_core::log_debug(&format!("[ui] {trimmed}"));
}

/// Reveal a path in the OS file manager (Finder / Explorer).
///
/// The `~` expansion happens here rather than in the webview: `reveal_item_in_dir`
/// does not accept `~`, and handing the home dir to the frontend just to rebuild
/// the path there would be a data round-trip for something the host already knows.
///
/// A shell action rather than a data-fetching capability, so it is a plain
/// command rather than a `LocalBackend` method; the browser build hides it
/// (`canReveal.ts`).
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
fn check_app_version(
    force: Option<bool>,
    locale: Option<String>,
) -> version_check::VersionCheckResult {
    version_check::check_app_version(force.unwrap_or(false), locale.as_deref().unwrap_or("en"))
}

#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The git commit this desktop binary was built from (baked by build.rs, see
/// `stamp_git_commit`). The 移动端 view compares it against each connected
/// phone's `appCommit` to flag a stale mobile bundle. Like `get_app_version`,
/// this is a compile-time constant of the running app — not backend data — so
/// it stays a plain command.
/// `"unknown"` when no commit source was available at build time.
#[tauri::command]
fn desktop_build_commit() -> String {
    option_env!("FLEET_GIT_COMMIT")
        .unwrap_or("unknown")
        .to_string()
}

// ── App state ────────────────────────────────────────────────────────────────

pub struct AppState {
    /// The data-plane facade every Tauri command delegates to. Built in
    /// `setup()` (it needs the `AppHandle`) and never replaced afterwards;
    /// all of its methods take `&self`, so no lock sits in front of it.
    pub backend: Arc<local_backend::LocalBackend>,
    /// User's current UI locale (e.g. "en", "zh"), shared with backend threads.
    pub locale: Arc<Mutex<String>>,
    /// Notification mode: "all" | "user_action" | "none".
    pub notification_mode: Arc<Mutex<String>>,
    /// How the assistant addresses the user (default "老板" / "Boss").
    pub user_title: Arc<Mutex<String>>,
    /// Cached sessions for tray menu rebuilds.
    pub cached_sessions: Arc<Mutex<Vec<SessionInfo>>>,
    /// Cached per-source usage summaries for tray menu display.
    pub cached_usage: Arc<Mutex<Vec<ui_types::SourceUsageSummary>>>,
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

/// Size and place the main window inside the monitor's work area (taskbar /
/// menu-bar / Dock excluded) at launch.
///
/// tauri.conf.json's fixed 1280×820 + `center: true` is unsafe on smaller or
/// high-DPI displays: `center: true` centers on the *full monitor* (taskbar
/// strip included), so when the window is nearly as tall as the monitor its top
/// edge — and on Windows the custom caption buttons, since decorations are off
/// there — get pushed off the top of the screen. We therefore clamp the size to
/// the work area and center *within the work area*, clamping the offset to ≥ 0
/// so the top-left corner is always on-screen.
///
/// The target size is taken from the configured constants rather than
/// `inner_size()` because at `setup()` time the reported size is not yet
/// reliable across platforms.
fn fit_main_window_to_work_area(w: &tauri::WebviewWindow) {
    // Keep in sync with tauri.conf.json `app.windows[0]`.
    const CONF_W: f64 = 1280.0;
    const CONF_H: f64 = 820.0;
    const MIN_W: f64 = 900.0;
    const MIN_H: f64 = 600.0;
    // Small inset so the window isn't flush against the work-area edges.
    const MARGIN: f64 = 24.0;

    let Ok(Some(monitor)) = w.current_monitor() else {
        return;
    };
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let area_w = area.size.width as f64 / scale;
    let area_h = area.size.height as f64 / scale;
    let area_x = area.position.x as f64 / scale;
    let area_y = area.position.y as f64 / scale;

    // Fit the configured size into the work area, but never below the min size:
    // on a tiny work area a slight overflow beats breaking the layout.
    let new_w = CONF_W.min((area_w - MARGIN).max(0.0)).max(MIN_W);
    let new_h = CONF_H.min((area_h - MARGIN).max(0.0)).max(MIN_H);

    // Center within the work area; clamp the offset to ≥ 0 so the top-left stays
    // on-screen even when the window is taller/wider than the work area.
    let x = area_x + ((area_w - new_w) / 2.0).max(0.0);
    let y = area_y + ((area_h - new_h) / 2.0).max(0.0);

    let _ = w.set_size(tauri::Size::Logical(tauri::LogicalSize::new(new_w, new_h)));
    let _ = w.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

/// Page margin AppKit gets for the reader's print job, in points (72pt = 1in).
/// 28pt ≈ 10mm. Only a floor: the print panel still lets the user change it.
#[cfg(target_os = "macos")]
const PRINT_MARGIN_PT: f64 = 28.0;

/// Run the print operation ourselves so the sheet gets real page margins.
///
/// `WebviewWindow::print()` forwards to wry's `print_with_options(&default())`,
/// and `PrintMargin` derives `Default` over four `f32`s — so wry explicitly
/// stamps **zero** margins onto `NSPrintInfo` before creating the operation.
/// That is why the first version printed with prose running into both paper
/// edges. Everything else here mirrors wry's implementation (wry 0.55.1,
/// `src/wkwebview/mod.rs::print_with_options`).
///
/// Unlike wry we copy `sharedPrintInfo` instead of mutating it: it is an
/// app-wide object, and editing it in place would leave our margins on every
/// later print job the app runs.
#[cfg(target_os = "macos")]
unsafe fn print_with_margins(
    wk_webview: *mut std::ffi::c_void,
    ns_window: *mut std::ffi::c_void,
) -> Result<(), String> {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, sel};

    if wk_webview.is_null() || ns_window.is_null() {
        return Err("no native webview/window handle".into());
    }
    let webview: *mut AnyObject = wk_webview.cast();

    // printOperationWithPrintInfo: is macOS 11+; without it there is nothing to
    // fall back to on this path, so let the caller use the plain wry route.
    let can_print: bool =
        msg_send![webview, respondsToSelector: sel!(printOperationWithPrintInfo:)];
    if !can_print {
        return Err("this WKWebView cannot print".into());
    }

    let shared: *mut AnyObject = msg_send![class!(NSPrintInfo), sharedPrintInfo];
    let info: *mut AnyObject = msg_send![shared, copy];
    let _: () = msg_send![info, setTopMargin: PRINT_MARGIN_PT];
    let _: () = msg_send![info, setRightMargin: PRINT_MARGIN_PT];
    let _: () = msg_send![info, setBottomMargin: PRINT_MARGIN_PT];
    let _: () = msg_send![info, setLeftMargin: PRINT_MARGIN_PT];

    let op: *mut AnyObject = msg_send![webview, printOperationWithPrintInfo: info];
    if op.is_null() {
        return Err("could not create the print operation".into());
    }
    // Lets the modal detach from this thread so the panel is not blocking.
    let _: () = msg_send![op, setCanSpawnSeparateThread: true];

    let win: *mut AnyObject = ns_window.cast();
    let nil: *mut AnyObject = std::ptr::null_mut();
    let _: () = msg_send![
        op,
        runOperationModalForWindow: win,
        delegate: nil,
        didRunSelector: std::ptr::null_mut::<std::ffi::c_void>(),
        contextInfo: std::ptr::null_mut::<std::ffi::c_void>(),
    ];
    Ok(())
}

/// Hand the calling webview to the OS print pipeline — how the reader's
/// "导出 PDF" works: macOS opens the system print panel (whose `PDF ▾ → Save as
/// PDF` is the actual export), Windows opens the WebView2 print dialog.
///
/// There is deliberately no JS route. `window.print()` is a no-op inside
/// WKWebView, so a frontend-only implementation would silently do nothing on
/// macOS; wry only reaches `NSPrintOperation` from the Rust side. Sync on
/// purpose — Tauri runs sync commands on the main thread, which is where
/// AppKit's print operation must be created. (Measured with an isolated probe:
/// called off the main thread, print() returns Ok and no panel ever appears.)
///
/// On macOS the operation is built by hand to get non-zero page margins — see
/// [`print_with_margins`]. Everywhere else `WebviewWindow::print()` is right:
/// on Windows it injects `window.print()`, and the WebView2 dialog brings its
/// own margin control.
///
/// What lands on the page is whatever the print stylesheet leaves visible, so
/// the reader's `@media print` rules decide the artifact, not this command.
#[tauri::command]
fn print_webview(window: tauri::WebviewWindow) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // Carried as an address because `with_webview` wants a Send closure and
        // a raw pointer is not: it is re-formed inside, on the main thread.
        let ns_window = window.ns_window().map_err(|e| e.to_string())? as usize;
        // with_webview hands over the raw WKWebView; the closure runs on the
        // main thread, which is also where NSPrintOperation must be created.
        let outcome = std::sync::Arc::new(std::sync::Mutex::new(None::<Result<(), String>>));
        let sink = outcome.clone();
        window
            .with_webview(move |webview| {
                let r = unsafe {
                    print_with_margins(webview.inner(), ns_window as *mut std::ffi::c_void)
                };
                *sink.lock().unwrap() = Some(r);
            })
            .map_err(|e| e.to_string())?;
        // `with_webview` dispatches to the main thread; when the command is
        // already on it (the normal case for a sync command) the closure has run
        // by now. If it hasn't, fall through to the plain route rather than
        // blocking the main thread waiting on ourselves.
        let done = outcome.lock().unwrap().take();
        if let Some(r) = done {
            return r;
        }
    }
    window.print().map_err(|e| e.to_string())
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
        RemoteDisconnected => "remote disconnected",
        Stuck => "stuck",
    }
}

fn is_session_active(s: &SessionInfo) -> bool {
    use session::SessionStatus;
    matches!(
        s.status,
        SessionStatus::Thinking
            | SessionStatus::Executing
            | SessionStatus::Streaming
            | SessionStatus::Processing
            | SessionStatus::WaitingInput
            | SessionStatus::Active
            | SessionStatus::Delegating
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

pub fn update_tray_usage(app: &tauri::AppHandle, summaries: Vec<ui_types::SourceUsageSummary>) {
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
    let mut active_all: Vec<&SessionInfo> =
        sessions.iter().filter(|s| is_session_active(s)).collect();
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
            total,
            active_main.len(),
            sub_count
        )
    };

    let Some(tray) = app.tray_by_id("main") else {
        return;
    };
    let _ = tray.set_tooltip(Some(&tooltip));
    #[cfg(target_os = "macos")]
    {
        let title = if total > 0 {
            format!("{}", total)
        } else {
            String::new()
        };
        let _ = tray.set_title(Some(&title));
    }

    // Only rebuild the menu when content actually changed.
    if fingerprint != prev {
        // If the menu is presumed open (recent tray click), defer the rebuild
        // so we don't close it under the user's cursor.
        let within_grace = state.tray_last_click.lock().unwrap().map_or(false, |t| {
            t.elapsed() < std::time::Duration::from_secs(TRAY_MENU_GRACE_SECS)
        });
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
    if !pending {
        return;
    }

    let within_grace = state.tray_last_click.lock().unwrap().map_or(false, |t| {
        t.elapsed() < std::time::Duration::from_secs(TRAY_MENU_GRACE_SECS)
    });
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
            // Settings is an overlay inside the main window, so surface that
            // window first and let the frontend open the panel.
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
            let _ = app.emit("menu-settings", ());
        }
        "menu-check-updates" | "menu-check-updates-help" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            let _ = app.emit("menu-check-updates", ());
        }
        "menu-daily-report" => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
            let _ = app.emit("menu-daily-report", ());
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
            let _ = app.opener().open_url(
                "https://github.com/hoveychen/claw-fleet/issues",
                None::<&str>,
            );
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
    summaries: &[ui_types::SourceUsageSummary],
) -> Result<tauri::menu::Menu<tauri::Wry>, tauri::Error> {
    let mut builder = MenuBuilder::new(app);

    // ── Active agents section ────────────────────────────────────────────
    let header_text = if total > 0 {
        format!(
            "{} Active Agent{}",
            total,
            if total == 1 { "" } else { "s" }
        )
    } else {
        "No Active Agents".to_string()
    };
    builder = builder.item(
        &MenuItemBuilder::new(header_text)
            .id("info-header")
            .enabled(false)
            .build(app)?,
    );

    // List all active sessions (main + subagents), clickable to open detail.
    for (i, s) in active_main.iter().enumerate() {
        let prefix = if s.is_subagent { "  ↳ " } else { "" };
        let label = format!(
            "{}{} — {}",
            prefix,
            s.workspace_name,
            status_label(&s.status)
        );
        builder = builder.item(
            &MenuItemBuilder::new(label)
                .id(format!("open-session-{}", i))
                .build(app)?,
        );
    }

    builder = builder.item(&PredefinedMenuItem::separator(app)?);

    // ── Usage section (all sources) ─────────────────────────────────────
    if !summaries.is_empty() {
        for (idx, summary) in summaries.iter().enumerate() {
            if summary.bars.is_empty() {
                continue;
            }
            let parts: Vec<String> = summary
                .bars
                .iter()
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
                    .build(app)?,
            );
        }
        builder = builder.item(&PredefinedMenuItem::separator(app)?);
    }

    // ── Actions ──────────────────────────────────────────────────────────
    builder = builder.item(&MenuItemBuilder::new("Quit").id("quit").build(app)?);

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
        // Windows). Answered from LocalBackend on a worker thread so a large
        // asset read never blocks the webview's IPC thread.
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
                    let backend = &state.backend;
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
        // Serves artifact blobs into the webview: fleet-artifact://localhost/
        // <id>/<name> (http://fleet-artifact.localhost/… on Windows). The
        // trailing <name> is cosmetic — the id alone identifies the blob — but
        // it gives the viewer a real filename and the right extension, which is
        // what makes a <video> pick a decoder and a PDF frame render.
        //
        // Unlike the three protocols below it, this one honours `Range` and can
        // answer 206. That is the entire reason the artifact store is not just
        // more wiki: a deliverable can be a 400 MB render, and a <video> seeks
        // by asking for byte ranges. Answer those with the whole file every
        // time and the viewer must buffer it all before it can jump.
        //
        // Verified in P0 that WKWebView asks for ranges here even for embedded
        // PDFs (`Range: bytes=0-16383`), so this is not a video-only path.
        .register_asynchronous_uri_scheme_protocol(
            "fleet-artifact",
            move |ctx, request, responder| {
                let app = ctx.app_handle().clone();
                std::thread::spawn(move || {
                    let dec = |s: &str| {
                        percent_encoding::percent_decode_str(s)
                            .decode_utf8_lossy()
                            .to_string()
                    };
                    let path = request.uri().path().trim_start_matches('/').to_string();
                    let id = dec(path.split('/').next().unwrap_or(""));
                    let range = request
                        .headers()
                        .get("Range")
                        .and_then(|v| v.to_str().ok())
                        .and_then(claw_fleet_core::artifacts::parse_range_header);

                    let result = {
                        let state = app.state::<AppState>();
                        let backend = &state.backend;
                        backend.read_artifact_bytes(&id, range)
                    };
                    let response = artifact_response(result, range.is_some());
                    responder.respond(response);
                });
            },
        )
        // Serves fleet__ask decision-card assets into the webview:
        // fleet-decision://localhost/<id>/q<idx>/<relpath…>
        // (http://fleet-decision.localhost/… on Windows). Same worker-thread
        // shape as fleet-wiki://. Lets image-bearing cards load their
        // index.html + images without base64-inlining into the tool call.
        .register_asynchronous_uri_scheme_protocol(
            "fleet-decision",
            move |ctx, request, responder| {
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
                        let backend = &state.backend;
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
            },
        )
        // Serves images a Codex session generated into the webview:
        // fleet-genimage://localhost/<session id>/<name>
        // Same shape as fleet-decision:// above. The files sit in $CODEX_HOME,
        // outside every workspace.
        .register_asynchronous_uri_scheme_protocol(
            "fleet-genimage",
            move |ctx, request, responder| {
                let app = ctx.app_handle().clone();
                std::thread::spawn(move || {
                    let dec = |s: &str| {
                        percent_encoding::percent_decode_str(s)
                            .decode_utf8_lossy()
                            .to_string()
                    };
                    let path = request.uri().path().trim_start_matches('/').to_string();
                    let mut segs = path.splitn(2, '/');
                    let session = dec(segs.next().unwrap_or(""));
                    let name = dec(segs.next().unwrap_or(""));
                    let result = {
                        let state = app.state::<AppState>();
                        let backend = &state.backend;
                        backend.get_session_image(&session, &name)
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
            },
        )
        // Serves user-direction attachments (composer pastes, decision-panel
        // picks) into the webview so history can render them as thumbnails:
        // fleet-attachment://localhost/<key>/<name>
        // Same shape as fleet-decision:// above.
        .register_asynchronous_uri_scheme_protocol(
            "fleet-attachment",
            move |ctx, request, responder| {
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
                        let backend = &state.backend;
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
            },
        );

    builder
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

            // Shrink the initial window to fit the screen's usable area on
            // small / high-DPI displays. No-op when the configured size already
            // fits, so nothing changes on roomy monitors.
            if let Some(w) = app.get_webview_window("main") {
                fit_main_window_to_work_area(&w);
            }

            // Drop live-thinking sidecars left behind by finished turns. A
            // `claude --print` turn's sidecar goes stale as soon as it exits;
            // 6h is a generous ceiling that keeps ~/.fleet/live-thinking small
            // without racing any still-streaming session.
            claw_fleet_core::live_thinking::prune_old(6 * 60 * 60);

            // Build the LocalBackend now that the AppHandle exists, and register
            // AppState around it. Tauri runs `setup` before the event loop
            // starts, so no command can be dispatched before this point — there
            // is no window in which a placeholder backend would be observed.
            {
                let locale = Arc::new(Mutex::new("en".to_string()));
                let llm_cfg = Arc::new(Mutex::new(llm_provider::LlmConfig::load()));
                llm_provider::set_shared_config(llm_cfg.lock().unwrap().clone());

                // Build the agent source registry from config (~/.fleet/fleet-sources.json).
                let sources = agent_source::build_sources();

                let local = local_backend::LocalBackend::new(
                    app.handle().clone(),
                    locale.clone(),
                    llm_cfg.clone(),
                    sources,
                );
                let cached_llm_providers = Arc::new(Mutex::new(Vec::new()));
                app.manage(AppState {
                    backend: Arc::new(local),
                    locale,
                    notification_mode: Arc::new(Mutex::new("user_action".to_string())),
                    user_title: Arc::new(Mutex::new(String::new())),
                    cached_sessions: Arc::new(Mutex::new(Vec::new())),
                    cached_usage: Arc::new(Mutex::new(Vec::new())),
                    tray_fingerprint: Arc::new(Mutex::new(0)),
                    tray_last_click: Arc::new(Mutex::new(None)),
                    tray_rebuild_pending: Arc::new(Mutex::new(false)),
                    llm_config: llm_cfg,
                    cached_llm_providers: cached_llm_providers.clone(),
                });

                // Pre-fetch LLM provider info in background so Settings opens instantly.
                std::thread::spawn(move || {
                    let infos = llm_provider::all_provider_infos();
                    *cached_llm_providers.lock().unwrap() = infos;
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

            // Install the idle hooks (Stop → `fleet session idle`,
            // UserPromptSubmit → `fleet session resume`). The Stop hook is the
            // trigger for handoff relays (`handoff::consume_and_spawn`) and the
            // loop/watch reconcile — without it, a registered `fleet handoff`
            // never spawns its successor and stranded `fleet loop`/`fleet watch`
            // timers never re-arm. This ran at every launch inside the old
            // `daemon_autostart::ensure_supervisor_daemon` alongside
            // `ensure_fleet_cli_link` above; when the kanban daemon was removed
            // (192b35b) the CLI-link half was kept here but this call was
            // dropped, silently orphaning the handoff trigger on every fresh
            // install. Restored here as its twin. Idempotent (retain-then-push).
            if let Err(e) = claw_fleet_core::hooks::apply_idle_hooks() {
                claw_fleet_core::log_debug(&format!("apply_idle_hooks failed: {e}"));
            }

            // One-time migration: port/token/bin and the defunct event log all
            // moved to ~/.fleet, so the old ~/.claude/fleet directory is now
            // pure legacy — remove it. Best-effort; a failure is not fatal.
            if let Err(e) = claw_fleet_core::launchd::remove_legacy_fleet_dir() {
                claw_fleet_core::log_debug(&format!("remove_legacy_fleet_dir failed: {e}"));
            }

            // One-time cleanup: an old build installed a `com.claudefleet.serve`
            // LaunchAgent that keeps a `fleet serve` process alive at login.
            // On a desktop machine that stray serve becomes a *second*
            // mobile-relay provider, so every phone submit spawns two claude
            // processes (duplicate prompts / decision cards). Current code
            // installs no LaunchAgent — remove the legacy plist. Best-effort.
            if let Err(e) = claw_fleet_core::launchd::remove_legacy_serve_launchagent() {
                claw_fleet_core::log_debug(&format!("remove_legacy_serve_launchagent failed: {e}"));
            }

            // Reclaim legacy token-less `dsh web` instances. Current 0.1.2
            // records carry an owner-only launch token and survive here for
            // DshSource to adopt without interrupting an active turn.
            let reaped = claw_fleet_core::dsh_server::reap_orphans();
            if reaped > 0 {
                claw_fleet_core::log_debug(&format!(
                    "reaped {reaped} orphaned dsh web process(es)"
                ));
            }

            // Inject Fleet's permissions allowlist into ~/.claude/settings.json
            // so fleet guard becomes the sole audit gate. prune_dead_holders
            // inside acquire self-heals when a prior Fleet process died
            // without releasing.
            if claw_fleet_core::permissions_injector::load_config().enabled {
                if let Err(e) = claw_fleet_core::permissions_injector::acquire(std::process::id()) {
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
            // Previously debug-only: v2 fleet__ask had UX gaps vs v1 (no
            // preview, per-call permission prompt, guidance defaulted to v1).
            // Those are now closed — the fleet-ask card renders option previews,
            // the `mcp__fleet__*` permissions allow-list suppresses the prompt,
            // and the interaction-mode guidance no longer steers to v1 — so the
            // tool ships in every build, gated only by the user toggle. When the
            // toggle is off we release() any stale entry so an upgrade from an
            // earlier build doesn't keep an orphaned mcpServers.fleet.
            if claw_fleet_core::mcp_injector::load_config().enabled {
                match crate::fleet_binary::resolve_fleet_binary() {
                    Some(p) => {
                        let path_str = p.to_string_lossy().to_string();
                        if let Err(e) =
                            claw_fleet_core::mcp_injector::acquire(std::process::id(), &path_str)
                        {
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
            } else {
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
            claw_fleet_core::account::start_background_sampler(std::time::Duration::from_secs(600));
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
                .item(
                    &MenuItemBuilder::new("No Active Agents")
                        .id("info-header")
                        .enabled(false)
                        .build(app)?,
                )
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
                TrayIconBuilder::with_id("main").icon(icon)
            };

            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let tray_builder = {
                let icon = app.default_window_icon().cloned().unwrap();
                TrayIconBuilder::with_id("main").icon(icon)
            };

            tray_builder
                .menu(&tray_menu)
                .tooltip("Claw Fleet")
                .on_tray_icon_event(|tray, event| {
                    // Record click timestamp so we can defer tray menu rebuilds
                    // while the menu is open.
                    if let tauri::tray::TrayIconEvent::Click {
                        button,
                        button_state,
                        ..
                    } = &event
                    {
                        if matches!(button_state, tauri::tray::MouseButtonState::Up) {
                            let app = tray.app_handle();
                            let state = app.state::<AppState>();
                            *state.tray_last_click.lock().unwrap() =
                                Some(std::time::Instant::now());

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
                    if id_str == "quit" {
                        app.exit(0);
                    } else if let Some(idx_str) = id_str.strip_prefix("open-session-") {
                        if let Ok(idx) = idx_str.parse::<usize>() {
                            let state = app.state::<AppState>();
                            let sessions = state.cached_sessions.lock().unwrap().clone();
                            let mut active: Vec<&SessionInfo> =
                                sessions.iter().filter(|s| is_session_active(s)).collect();
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
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(TRAY_MENU_GRACE_SECS));
                    flush_pending_tray_rebuild(&app_handle);
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            today_usage,
            today_usage_breakdown,
            usage_range_breakdown,
            search_sessions,
            get_messages,
            get_messages_tail,
            get_messages_since,
            get_tool_result_full,
            get_skill_history,
            get_workflow_trees,
            get_task_token_breakdown,
            get_codex_token_breakdown,
            get_dsh_token_breakdown,
            get_dsh_session_cost,
            dsh_models,
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
            log_frontend_debug,
            reveal_path,
            probe_url_embeddable,
            check_app_version,
            get_app_version,
            desktop_build_commit,
            interrupt_session,
            interrupt_agent_session,
            kill_session,
            kill_workspace_sessions,
            resume_rate_limited_session,
            enqueue_session_message,
            cancel_session_pending_message,
            spawn_new_claude_session,
            chat_workspace,
            browse_dir,
            create_dir,
            remote_browse_dir,
            remote_create_dir,
            remote_host_health,
            list_ssh_hosts,
            upsert_ssh_host,
            remove_ssh_host,
            list_remote_workspaces,
            upsert_remote_workspace,
            remove_remote_workspace,
            get_auto_resume_config,
            set_auto_resume_config,
            set_session_mark,
            set_session_title,
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
            harness_statuses,
            install_harness,
            install_node_runtime,
            update_harness,
            harness_login_context,
            claude_login_start,
            claude_login_poll,
            claude_login_submit_code,
            claude_login_cancel,
            codex_login_start,
            codex_login_poll,
            codex_login_cancel,
            dsh_credential_refs,
            dsh_credentials_describe,
            dsh_credentials_set,
            install_fleet_cli,
            detect_ai_tools,
            install_fleet_skill,
            save_skill_file,
            rca_provision::list_ssh_profiles,
            rca_provision::install_rca_remote,
            rca_provision::install_rca_on_host,
            rca_provision::update_rca_remote,
            rca_provision::remote_workspace_harness_statuses,
            rca_provision::install_harness_remote,
            remote_codex_login_start,
            remote_codex_login_poll,
            pick_file,
            get_source_account,
            get_source_usage,
            list_memories,
            list_loops,
            list_schedules,
            cancel_loop,
            cancel_schedule,
            update_schedule,
            get_memory_content,
            read_live_thinking,
            get_task_plans,
            get_plan_forest,
            get_memory_history,
            get_claude_md_content,
            promote_memory,
            list_artifacts,
            list_session_images,
            get_artifact,
            add_artifact,
            update_artifact,
            delete_artifact,
            artifact_usage,
            export_artifact,
            artifact_local_path,
            open_artifact_external,
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
            publish_wiki_text,
            print_webview,
            list_browse_paths,
            add_browse_path,
            remove_browse_path,
            list_explorer_roots,
            git_status,
            git_push,
            git_pull,
            git_clone,
            start_git_clone,
            list_explorer_dir,
            read_explorer_file,
            find_explorer_path,
            read_external_file,
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
            apply_session_title_guidance,
            remove_session_title_guidance,
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
            mobile_relay_pairing_url,
            generate_mascot_quips,
            list_llm_providers,
            get_llm_config,
            set_llm_config,
            list_fleet_llm_usage_daily,
            get_usage_history,
            get_codex_usage_history,
            get_sources_config,
            list_codex_profiles,
            set_source_enabled,
            list_claude_binaries,
            get_claude_binary_override,
            set_claude_binary_override,
            restart_app,
            get_notification_mode,
            set_notification_mode,
            get_decision_panel_config,
            set_decision_panel_config,
            read_review_doc,
            get_permissions_config,
            set_permissions_config,
            get_user_title,
            set_user_title,
            open_notification_settings,
            show_main_window,
            crate::traffic_lights::nudge_traffic_lights,
            quit_app,
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
            list_task_reviews,
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
                let _ = claw_fleet_core::permissions_injector::release(std::process::id());
                let _ = claw_fleet_core::mcp_injector::release(std::process::id());
                // dsh 0.1.2 is an authenticated machine service. It deliberately
                // survives this GUI process so an app update/relaunch cannot
                // interrupt every active dsh turn; the next Fleet adopts it from
                // the owner-only registry.
            }
        });
}
