//! The three host-settings pairs the Settings panel reads and writes.
//!
//! Each is a GET (current value) + POST (save, answer with the stored value).
//! They exist for the browser build: a tab served by `fleet webui` has no host
//! of its own, so without these the panel would render a toggle that reads as
//! the host's state and saves nowhere. See `routes::AUTO_RESUME_CONFIG` for why
//! `RemoteBackend` still reads these locally instead of calling them.
//!
//! Each POST mirrors what the desktop's Tauri command does — *including its
//! side effects*, which is the part that is easy to drop: saving the
//! permissions toggle without acquiring/deactivating the injector would leave
//! settings.json untouched while the UI reports the flip as done, and saving
//! the decision-panel config without clamping would persist a value the hooks
//! then silently ignore.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

/// GET → serialize `load()`. POST → parse the body, run `save`, answer with
/// whatever `save` says was stored (which is not always what came in: the
/// decision-panel config is clamped on the way to disk).
///
/// A body that doesn't deserialize is a 400, not a silent default — a config
/// POST that half-parsed and saved the defaults would quietly reset the user's
/// settings and report success.
fn config_pair<T: serde::de::DeserializeOwned + serde::Serialize>(
    mut request: tiny_http::Request,
    json_header: tiny_http::Header,
    load: impl FnOnce() -> T,
    save: impl FnOnce(T) -> Result<T, String>,
) {
    if request.method() == &tiny_http::Method::Post {
        let mut buf = String::new();
        let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
        let (status, body) = match serde_json::from_str::<T>(&buf) {
            Err(e) => (400u16, serde_json::json!({"error": e.to_string()}).to_string()),
            Ok(cfg) => match save(cfg) {
                Ok(stored) => (200u16, serde_json::to_string(&stored).unwrap_or_default()),
                Err(e) => (500u16, serde_json::json!({"error": e}).to_string()),
            },
        };
        let _ = request.respond(
            tiny_http::Response::from_string(body)
                .with_status_code(status)
                .with_header(json_header),
        );
        return;
    }
    let body = serde_json::to_string(&load()).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

pub(crate) fn route_auto_resume_config(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    config_pair(
        request,
        json_header,
        crate::auto_resume::AutoResumeConfig::load,
        |cfg| cfg.save().map(|()| cfg),
    );
}

pub(crate) fn route_permissions_config(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    config_pair(
        request,
        json_header,
        crate::permissions_injector::load_config,
        |cfg| {
            // Save *then* apply, same order as `gui::set_permissions_config`:
            // every other Fleet process's watchdog reads the file, so it has to
            // be the new truth before the lock changes under them.
            crate::permissions_injector::save_config(&cfg).map_err(|e| e.to_string())?;
            if cfg.enabled {
                crate::permissions_injector::acquire(std::process::id())
                    .map_err(|e| e.to_string())?;
            } else {
                // The toggle is the only un-injection path, and it is global:
                // deactivate even while a peer process holds the lock, since
                // every watchdog then reads `enabled == false` and stops
                // re-injecting.
                crate::permissions_injector::deactivate().map_err(|e| e.to_string())?;
            }
            Ok(cfg)
        },
    );
}

pub(crate) fn route_decision_panel_config(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    config_pair(
        request,
        json_header,
        crate::decision_panel_config::load,
        |cfg| {
            let mut cfg = cfg;
            crate::decision_panel_config::save(&mut cfg).map_err(|e| e.to_string())?;
            Ok(cfg)
        },
    );
}
