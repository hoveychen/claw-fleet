use base64::Engine;

fn decode_image(base64: &str) -> Result<Vec<u8>, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 50 * 1024 * 1024 {
        return Err("image exceeds 50 MiB".into());
    }
    Ok(bytes)
}

#[tauri::command(async)]
pub(crate) fn save_preview_image(dest: String, base64: String) -> Result<(), String> {
    let bytes = decode_image(&base64)?;
    std::fs::write(dest, bytes).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub(crate) fn share_preview_image(
    app: tauri::AppHandle,
    base64: String,
) -> Result<(), String> {
    let bytes = decode_image(&base64)?;
    #[cfg(target_os = "macos")]
    {
        use tauri::Manager;
        let window = app.get_webview_window("main").ok_or("main window unavailable")?;
        let ns_window = window.ns_window().map_err(|e| e.to_string())? as usize;
        window.run_on_main_thread(move || unsafe {
            use objc2::{class, msg_send, runtime::AnyObject};
            use objc2_foundation::NSRect;
            let data: *mut AnyObject = msg_send![class!(NSData), dataWithBytes: bytes.as_ptr(), length: bytes.len()];
            let image: *mut AnyObject = msg_send![class!(NSImage), alloc];
            let image: *mut AnyObject = msg_send![image, initWithData: data];
            if image.is_null() { return; }
            let items: *mut AnyObject = msg_send![class!(NSArray), arrayWithObject: image];
            let picker: *mut AnyObject = msg_send![class!(NSSharingServicePicker), alloc];
            let picker: *mut AnyObject = msg_send![picker, initWithItems: items];
            let view: *mut AnyObject = msg_send![ns_window as *mut AnyObject, contentView];
            if !view.is_null() && !picker.is_null() {
                let frame: NSRect = msg_send![view, bounds];
                let _: () = msg_send![picker, showRelativeToRect: frame, ofView: view, preferredEdge: 1_u64];
            }
        }).map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, bytes);
        Err("system image sharing is unavailable on this platform".into())
    }
}
