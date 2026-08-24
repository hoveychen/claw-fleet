//! The two HTTP entry points, both thin wrappers over
//! `claw_fleet_core::hooks_server::serve`.
//!
//! They are separate subcommands rather than flags on one because they are
//! separate deployments with opposite defaults, and a flag combination is
//! something you have to remember correctly every time:
//!
//! - `fleet serve` — the token-gated API probe. What RemoteBackend, the mobile
//!   relay and the cloud container's `/v1/*` surface talk to. A token is
//!   required; the admin/scoped tiering applies.
//! - `fleet webui` — the browser build of the app: the UI bundle plus the data
//!   routes it needs. No token, because the UI needs routes outside the
//!   `/v1/*` scoped whitelist and a credential in the page would protect
//!   nothing. Loopback by default; widening it is `--host`, and that belongs
//!   behind your own auth gateway since these routes can start agent sessions.

use claw_fleet_core::hooks_server::{self, ServeOptions};

/// `fleet serve` — token-gated API only. No web UI, no auth bypass.
pub(crate) fn cmd_serve(port: u16, token: String, port_file: Option<std::path::PathBuf>) {
    hooks_server::serve(ServeOptions {
        port,
        token,
        port_file,
        ..Default::default()
    });
}

/// `fleet webui` — the browser build.
pub(crate) fn cmd_webui(
    port: u16,
    web_root: Option<std::path::PathBuf>,
    host: String,
    port_file: Option<std::path::PathBuf>,
) {
    // `--web-root` wins; otherwise the env, which is how the container is
    // configured. Neither → nothing to serve, and a UI-less "web UI" is a
    // misconfiguration worth failing on rather than starting a port that
    // answers 404 for every page.
    let root = web_root.or_else(|| {
        std::env::var("FLEET_WEB_ROOT")
            .ok()
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
    });
    let Some(root) = root else {
        eprintln!("fleet webui: no bundle to serve — pass --web-root <dir> or set FLEET_WEB_ROOT");
        std::process::exit(2);
    };
    if !root.is_dir() {
        eprintln!("fleet webui: {} is not a directory", root.display());
        std::process::exit(2);
    }
    eprintln!("[fleet webui] serving web UI from {}", root.display());
    if host != "127.0.0.1" && host != "localhost" {
        eprintln!(
            "[fleet webui] binding {host} — this port has no authentication and can start agent sessions; put an auth gateway in front of it"
        );
    }

    hooks_server::serve(ServeOptions {
        port,
        // Nothing presents a token here, and an empty one matches nothing —
        // access is governed by `no_auth` plus the bind address.
        token: String::new(),
        port_file,
        no_auth: true,
        web_assets: Some(claw_fleet_core::web_assets::from_dir(root)),
        host: Some(host),
    });
}
