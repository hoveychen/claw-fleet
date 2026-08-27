//! The two HTTP entry points, both thin wrappers over
//! `claw_fleet_core::hooks_server::serve`.
//!
//! They are separate subcommands rather than flags on one because they are
//! separate deployments with opposite defaults, and a flag combination is
//! something you have to remember correctly every time:
//!
//! - `fleet serve` — the token-gated API probe. What RemoteBackend, the mobile
//!   relay and the cloud container's `/v1/*` surface talk to. A token is
//!   required; the admin/scoped tiering applies. Its bind address comes from
//!   `FLEET_SERVE_HOST` (read inside `hooks_server::serve`); there is no flag.
//! - `fleet webui` — the browser build of the app: the UI bundle plus the data
//!   routes it needs. No token, because the UI needs routes outside the
//!   `/v1/*` scoped whitelist and a credential in the page would protect
//!   nothing. Loopback by default; widening it is `--host` / `FLEET_WEBUI_HOST`,
//!   and that belongs behind your own auth gateway since these routes can start
//!   agent sessions.
//!
//! The two host env vars are separate on purpose — see [`resolve_webui_host`].

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

/// Default bind address for `fleet webui` — loopback, because this port has no
/// authentication of its own.
const DEFAULT_WEBUI_HOST: &str = "127.0.0.1";
/// Default port for `fleet webui`.
const DEFAULT_WEBUI_PORT: u16 = 4571;

/// `--host` wins, then `FLEET_WEBUI_HOST`, then loopback.
///
/// Deliberately *not* `FLEET_SERVE_HOST` (which `hooks_server::serve` reads for
/// `fleet serve`): that one widens a token-gated API, this one widens a port with
/// no authentication at all. One env var flipping both to `0.0.0.0` is exactly
/// the accident worth keeping impossible.
fn resolve_webui_host(flag: Option<String>, env: Option<String>) -> String {
    flag.or_else(|| env.filter(|s| !s.is_empty()))
        .unwrap_or_else(|| DEFAULT_WEBUI_HOST.to_string())
}

/// `--port` wins, then `FLEET_WEBUI_PORT`, then [`DEFAULT_WEBUI_PORT`].
///
/// An unparseable env value is an error rather than a silent fall back to the
/// default: the operator asked for a specific port, and quietly binding another
/// one looks like success until something fails to connect.
fn resolve_webui_port(flag: Option<u16>, env: Option<String>) -> Result<u16, String> {
    if let Some(p) = flag {
        return Ok(p);
    }
    match env.filter(|s| !s.is_empty()) {
        Some(raw) => raw
            .parse::<u16>()
            .map_err(|e| format!("FLEET_WEBUI_PORT={raw} is not a valid port: {e}")),
        None => Ok(DEFAULT_WEBUI_PORT),
    }
}

/// Why a path can't serve as a bundle, or `Ok(())` if it can.
///
/// Checks for `index.html` rather than just the directory because an empty
/// directory is not a bundle: a volume mounted before it was populated would
/// pass an `is_dir` test and then 404 every page, which reads as "the UI is
/// broken" instead of "there is no UI here".
fn bundle_problem(root: &std::path::Path) -> Result<(), String> {
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    if !root.join("index.html").is_file() {
        return Err(format!(
            "{} has no index.html — is that a built bundle?",
            root.display()
        ));
    }
    Ok(())
}

/// `fleet webui` — the browser build.
pub(crate) fn cmd_webui(
    port: Option<u16>,
    web_root: Option<std::path::PathBuf>,
    host: Option<String>,
    port_file: Option<std::path::PathBuf>,
) {
    let host = resolve_webui_host(host, std::env::var("FLEET_WEBUI_HOST").ok());
    let port = match resolve_webui_port(port, std::env::var("FLEET_WEBUI_PORT").ok()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("fleet webui: {e}");
            std::process::exit(2);
        }
    };
    // Where the bundle comes from: `--web-root`, then `FLEET_WEB_ROOT`, then
    // whatever this build compiled in. A directory beats the embedded copy on
    // purpose: that is how you iterate on the frontend without rebuilding the
    // binary, and how a deployment pins its own bundle.
    //
    // The two disk sources are NOT interchangeable when the path holds no
    // bundle, and conflating them crash-looped the cloud container:
    //
    // - `--web-root` is a demand someone typed for this one invocation. An
    //   unusable path is a typo worth failing on.
    // - `FLEET_WEB_ROOT` is ambient deployment config — the cloud image presets
    //   it as the UI/API mode switch, and since the UI moved into the binary
    //   nothing is written to that path any more. Exiting on it meant the
    //   container refused to start even though it was carrying a perfectly good
    //   UI: `fleet webui: /usr/share/fleet-web is not a directory`, forever.
    //   So an unusable env value warns and falls through to the built-in copy.
    //
    // Nothing usable anywhere → a UI-less "web UI" is a misconfiguration worth
    // failing on rather than a port that answers 404 for every page.
    let from_disk = match web_root {
        Some(flag) => {
            if let Err(why) = bundle_problem(&flag) {
                eprintln!("fleet webui: {}", why);
                std::process::exit(2);
            }
            Some(flag)
        }
        None => match std::env::var("FLEET_WEB_ROOT")
            .ok()
            .filter(|s| !s.is_empty())
            .map(std::path::PathBuf::from)
        {
            Some(env_root) => match bundle_problem(&env_root) {
                Ok(()) => Some(env_root),
                Err(why) => {
                    eprintln!("[fleet webui] ignoring FLEET_WEB_ROOT: {why}");
                    None
                }
            },
            None => None,
        },
    };

    let assets = match from_disk {
        Some(root) => {
            eprintln!("[fleet webui] serving web UI from {}", root.display());
            claw_fleet_core::web_assets::from_dir(root)
        }
        None => match crate::webui_embed::asset_source() {
            Some(assets) => {
                eprintln!(
                    "[fleet webui] serving the web UI built into this binary ({} files)",
                    crate::webui_embed::embedded_file_count()
                );
                assets
            }
            None => {
                eprintln!(
                    "fleet webui: no bundle to serve — this binary was built without the web UI, \
                     so pass --web-root <dir> or set FLEET_WEB_ROOT"
                );
                std::process::exit(2);
            }
        },
    };
    if host != DEFAULT_WEBUI_HOST && host != "localhost" {
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
        web_assets: Some(assets),
        host: Some(host),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_flag_beats_env_beats_default() {
        assert_eq!(
            resolve_webui_host(Some("10.0.0.5".into()), Some("0.0.0.0".into())),
            "10.0.0.5"
        );
        assert_eq!(resolve_webui_host(None, Some("0.0.0.0".into())), "0.0.0.0");
        assert_eq!(resolve_webui_host(None, None), DEFAULT_WEBUI_HOST);
    }

    #[test]
    fn empty_host_env_is_not_a_value() {
        // Docker passes `-e FLEET_WEBUI_HOST` with no value as an empty string;
        // binding "" would fail, so it has to read as "unset".
        assert_eq!(resolve_webui_host(None, Some(String::new())), DEFAULT_WEBUI_HOST);
    }

    #[test]
    fn port_flag_beats_env_beats_default() {
        assert_eq!(resolve_webui_port(Some(9000), Some("8080".into())), Ok(9000));
        assert_eq!(resolve_webui_port(None, Some("8080".into())), Ok(8080));
        assert_eq!(resolve_webui_port(None, None), Ok(DEFAULT_WEBUI_PORT));
        assert_eq!(resolve_webui_port(None, Some(String::new())), Ok(DEFAULT_WEBUI_PORT));
    }

    #[test]
    fn port_zero_stays_zero() {
        // 0 means "let the OS pick" — it must survive both paths rather than
        // being mistaken for "unset" and replaced by the default.
        assert_eq!(resolve_webui_port(Some(0), None), Ok(0));
        assert_eq!(resolve_webui_port(None, Some("0".into())), Ok(0));
    }

    #[test]
    fn bundle_problem_names_what_is_wrong() {
        let dir = tempfile::TempDir::new().unwrap();

        let missing = dir.path().join("nope");
        assert!(bundle_problem(&missing).unwrap_err().contains("not a directory"));

        // A directory is not enough — an empty one would 404 every page.
        let empty = dir.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(bundle_problem(&empty).unwrap_err().contains("no index.html"));

        std::fs::write(empty.join("index.html"), b"<html></html>").unwrap();
        assert_eq!(bundle_problem(&empty), Ok(()));
    }

    #[test]
    fn unparseable_port_env_errors_instead_of_defaulting() {
        for raw in ["not-a-port", "70000", "-1", "4571 "] {
            let err = match resolve_webui_port(None, Some(raw.into())) {
                Err(e) => e,
                Ok(p) => panic!("{raw:?} must be rejected, was parsed as {p}"),
            };
            assert!(err.contains(raw), "message should quote the bad value: {err}");
        }
    }
}
