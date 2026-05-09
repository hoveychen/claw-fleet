//! Workaround for a tao/AppKit interaction: switching the window theme at
//! runtime via `WebviewWindow::set_theme` triggers an NSAppearance change,
//! which makes AppKit relayout the standard window buttons (close /
//! miniaturize / zoom) back to the system-default position. tao only
//! re-applies our `trafficLightPosition` inset from inside the content
//! view's `drawRect`, so until the view repaints, the buttons stay at the
//! wrong place — visible as the traffic lights jumping to the top-left
//! corner the moment the user toggles dark/light.
//!
//! Rather than relying on tao's draw_rect to fire after AppKit's relayout
//! (the timing is fragile), we replicate tao's `inset_traffic_lights`
//! logic ourselves and call it directly from a Tauri command.
//!
//! MUST be kept in sync with `trafficLightPosition` in tauri.conf.json.

use tauri::{Manager, Runtime};

#[cfg(target_os = "macos")]
const TRAFFIC_LIGHT_X: f64 = 20.0;
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHT_Y: f64 = 22.0;

#[cfg(target_os = "macos")]
unsafe fn reposition_traffic_lights(ns_window_ptr: *mut std::ffi::c_void) {
    use objc2::{msg_send, runtime::AnyObject};
    use objc2_foundation::NSRect;
    if ns_window_ptr.is_null() {
        return;
    }
    let window: *mut AnyObject = ns_window_ptr.cast();

    // NSWindowButton: CloseButton = 0, MiniaturizeButton = 1, ZoomButton = 2.
    let close: *mut AnyObject = msg_send![window, standardWindowButton: 0_i64];
    let mini: *mut AnyObject = msg_send![window, standardWindowButton: 1_i64];
    let zoom: *mut AnyObject = msg_send![window, standardWindowButton: 2_i64];
    if close.is_null() || mini.is_null() || zoom.is_null() {
        return;
    }

    let close_frame: NSRect = msg_send![close, frame];
    let mini_frame: NSRect = msg_send![mini, frame];
    let window_frame: NSRect = msg_send![window, frame];

    // Resize the title-bar container view so the buttons sit at y from the
    // top of the window. close.superview() is the button group; its
    // superview is the title-bar container.
    let close_super: *mut AnyObject = msg_send![close, superview];
    if close_super.is_null() {
        return;
    }
    let title_bar_container: *mut AnyObject = msg_send![close_super, superview];
    if title_bar_container.is_null() {
        return;
    }
    let mut tbc_frame: NSRect = msg_send![title_bar_container, frame];
    let title_bar_frame_height = close_frame.size.height + TRAFFIC_LIGHT_Y;
    tbc_frame.size.height = title_bar_frame_height;
    tbc_frame.origin.y = window_frame.size.height - title_bar_frame_height;
    let _: () = msg_send![title_bar_container, setFrame: tbc_frame];

    // Lay the three buttons out horizontally starting at x, preserving the
    // system-chosen vertical baseline and inter-button spacing.
    let space_between = mini_frame.origin.x - close_frame.origin.x;
    for (i, button) in [close, mini, zoom].iter().enumerate() {
        let mut frame: NSRect = msg_send![*button, frame];
        frame.origin.x = TRAFFIC_LIGHT_X + (i as f64) * space_between;
        let origin = frame.origin;
        let _: () = msg_send![*button, setFrameOrigin: origin];
    }
}

#[tauri::command]
pub fn nudge_traffic_lights<R: Runtime>(app: tauri::AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    #[cfg(target_os = "macos")]
    {
        if let Ok(ptr) = window.ns_window() {
            unsafe { reposition_traffic_lights(ptr) };
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
    }
}
