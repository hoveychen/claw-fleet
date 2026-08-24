//! Boots the browser front door against `dist/` without launching the desktop
//! app, so the shipped bundle can be exercised in a real browser.
//!
//! The app itself can't be started for this: it uses the single-instance
//! plugin, so a second copy would grab the running one's window. This binary
//! takes the same `web_serve::start` path the app takes and feeds it the real
//! `vite build` output, with `NullBackend` standing in for the data layer —
//! enough to prove the bundle loads, logs in, and drives its transport over
//! HTTP. Session data comes back empty by construction.
//!
//! Usage: `cargo run -p claw-fleet-desktop --example web_preview -- [port] [password]`

use std::sync::{Arc, RwLock};

use claw_fleet_core::backend::Backend;
use claw_fleet_desktop::web_serve::{self, StaticAsset, WebAccessConfig};

fn mime_for(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        _ => "application/octet-stream",
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let port: u16 = args
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(web_serve::DEFAULT_PORT);
    let password = args.next().unwrap_or_else(|| "preview".to_string());

    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("dist");
    if !dist.join("index.html").is_file() {
        eprintln!("no dist/index.html — run `pnpm run build` first");
        std::process::exit(1);
    }

    let root = dist.clone();
    let assets: web_serve::AssetSource = Arc::new(move |path: &str| {
        // Strip the leading slash and refuse traversal before touching disk.
        let rel = path.trim_start_matches('/');
        if rel.is_empty() || rel.split('/').any(|seg| seg == "..") {
            return None;
        }
        let file = root.join(rel);
        std::fs::read(&file).ok().map(|bytes| StaticAsset {
            bytes,
            mime: mime_for(rel).to_string(),
        })
    });

    let backend: Arc<RwLock<Box<dyn Backend>>> =
        Arc::new(RwLock::new(Box::new(claw_fleet_desktop::NullBackend)));

    let bound = web_serve::start_with_config(
        backend,
        port,
        WebAccessConfig {
            enabled: true,
            password: password.clone(),
            bind_lan: false,
        },
        Some(assets),
    )
    .expect("front door must bind");

    println!("serving dist/ at http://127.0.0.1:{bound}  (password: {password})");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
