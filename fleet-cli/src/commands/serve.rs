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

/// Marker recording that `fleet webui` already performed its first-run control
/// plane install on this host. Lives next to the injector configs in `~/.fleet`.
const WEBUI_BOOTSTRAP_MARKER: &str = "webui-bootstrap.json";

/// Install the control plane the first time `fleet webui` runs on a host, then
/// never again.
///
/// The gap this closes: `hooks_server::serve` injects the `Bash(*)` permissions
/// allow rule (suppressing Claude Code's native command prompt) and registers
/// the fleet MCP server, but it installs no hooks and no `~/.claude/CLAUDE.md`
/// guidance — those ship with the desktop onboarding UI or `fleet bootstrap`.
/// A fresh Linux host started with `fleet webui` alone, whose page nobody ever
/// opened, therefore ran agents with the native prompt suppressed and no
/// `fleet guard` audit gate, no elicitation/plan-approval bridge, no idle hooks
/// (so `fleet handoff`/`loop`/`watch` silently died) and no guidance at all.
///
/// **Why once rather than every start.** Every feature here is a tristate
/// toggle whose default is ON, but the user's explicit OFF is persisted in the
/// frontend store (`claw-fleet-desktop/app/storage.ts`), which this process
/// cannot see. Re-installing on every boot would reinstate modes the user
/// deliberately switched off, and they would only be undone again the next time
/// somebody loaded the page. Installing exactly once means "never configured"
/// gets the defaults, and every later choice — made through the UI, which does
/// reach the apply/remove routes — stands.
fn first_run_bootstrap() {
    let Some(marker) =
        claw_fleet_core::session::get_fleet_dir().map(|d| d.join(WEBUI_BOOTSTRAP_MARKER))
    else {
        return;
    };
    first_run_bootstrap_at(&marker, || {
        let settings = super::bootstrap::resolve_settings(None, None, None);
        super::bootstrap::install_control_plane(&settings)
    });
}

/// The marker gate itself, with the install injected so tests can exercise the
/// "runs once" contract without writing to a real `~/.claude`.
fn first_run_bootstrap_at(
    marker: &std::path::Path,
    install: impl FnOnce() -> Vec<super::bootstrap::Step>,
) {
    if marker.exists() {
        return;
    }

    eprintln!(
        "[fleet webui] first run on this host — installing the control plane \
         (hooks + guidance). Re-run or adjust it any time with `fleet bootstrap` \
         or the app's settings panel."
    );
    let steps = install();
    for s in &steps {
        if let Err(e) = &s.result {
            eprintln!("[fleet webui] bootstrap step {} failed: {e}", s.name);
        }
    }

    // Written even when a step failed: the marker records "the first-run install
    // was attempted", not "everything succeeded". Retrying on every boot would
    // re-run the whole install for one persistently failing step — and that is
    // exactly the every-boot reinstatement the once-only rule exists to avoid.
    // The failures are on stderr, and `fleet bootstrap` re-runs it on demand.
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let failed = steps.iter().filter(|s| s.result.is_err()).count();
    let body = serde_json::json!({
        "installed_at": chrono::Utc::now().to_rfc3339(),
        "failed_steps": failed,
    });
    if let Err(e) = std::fs::write(marker, body.to_string()) {
        eprintln!(
            "[fleet webui] could not write {} — the control plane will be \
             reinstalled on the next start: {e}",
            marker.display()
        );
    }
}

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

/// The address `--lan` binds: every interface, so the phone can reach it.
const LAN_WEBUI_HOST: &str = "0.0.0.0";

/// `--host` wins, then `--lan`, then `FLEET_WEBUI_HOST`, then loopback.
///
/// Deliberately *not* `FLEET_SERVE_HOST` (which `hooks_server::serve` reads for
/// `fleet serve`): that one widens a token-gated API, this one widens a port with
/// no authentication at all. One env var flipping both to `0.0.0.0` is exactly
/// the accident worth keeping impossible.
///
/// `--lan` outranks the env var because both flags were typed for *this* run
/// while the env var is ambient; it loses to `--host` because someone naming an
/// address means that address (a machine with several interfaces may want the
/// phone on exactly one of them, and `--lan` still prints the QR for it).
fn resolve_webui_host(flag: Option<String>, lan: bool, env: Option<String>) -> String {
    flag.or_else(|| lan.then(|| LAN_WEBUI_HOST.to_string()))
        .or_else(|| env.filter(|s| !s.is_empty()))
        .unwrap_or_else(|| DEFAULT_WEBUI_HOST.to_string())
}

/// The host another device types to reach a server bound to `bind_host`.
///
/// `0.0.0.0` is not an address anything can connect to, so it has to be
/// resolved to this machine's LAN IP. Anything else was named explicitly and is
/// already the answer. `None` means there is nothing worth printing: bound to
/// every interface, but no non-loopback address exists (offline machine).
fn advertised_host(bind_host: &str) -> Option<String> {
    if bind_host == LAN_WEBUI_HOST || bind_host == "::" {
        return claw_fleet_core::lan_access::lan_ipv4().map(|ip| ip.to_string());
    }
    Some(bind_host.to_string())
}

/// The `--lan` banner: both URLs and a QR code for the one you scan.
///
/// The QR points at the mobile UI (`/m/`) rather than the root when the bundle
/// carries one. The root would also work — its index.html redirects coarse
/// pointers to `/m/` — but "scan this with your phone" should not depend on a
/// pointer-media heuristic being right about the device that scanned it.
fn lan_banner(bind_host: &str, port: u16, has_mobile: bool) -> String {
    let Some(host) = advertised_host(bind_host) else {
        return "[fleet webui] --lan: no non-loopback address on this machine — is it offline?"
            .to_string();
    };
    let root = format!("http://{host}:{port}/");
    let scan = if has_mobile { format!("{root}m/") } else { root.clone() };

    let mut out = String::from("\n[fleet webui] 同一局域网内可访问：\n");
    out.push_str(&format!("  桌面 UI  {root}\n"));
    if has_mobile {
        out.push_str(&format!("  手机 UI  {scan}\n"));
    } else {
        out.push_str("  (这个 bundle 不含 /m/ 移动端产物，手机打开的也是桌面 UI)\n");
    }
    match claw_fleet_core::lan_access::qr_terminal(&scan) {
        Ok(qr) => {
            out.push_str(&format!("\n  扫这个二维码：{scan}\n\n"));
            out.push_str(&qr);
            out.push('\n');
        }
        // A URL this short cannot overflow a QR, so this is unreachable in
        // practice — but printing the link without the code beats swallowing it.
        Err(e) => out.push_str(&format!("  (二维码渲染失败：{e})\n")),
    }
    out
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
    lan: bool,
    port_file: Option<std::path::PathBuf>,
) {
    let host = resolve_webui_host(host, lan, std::env::var("FLEET_WEBUI_HOST").ok());
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
    // Before `serve` takes the thread: a host whose UI nobody has opened yet has
    // no hooks and no guidance, and `serve` is about to inject the permissions
    // allow-list on top of that. See [`first_run_bootstrap`].
    first_run_bootstrap();

    if host != DEFAULT_WEBUI_HOST && host != "localhost" {
        eprintln!(
            "[fleet webui] binding {host} — this port has no authentication and can start agent sessions; put an auth gateway in front of it"
        );
    }

    // Probed before the bundle is handed over, while it is still a local value:
    // whether this build actually carries the mobile UI decides which URL the
    // QR encodes. One lookup of one small file.
    let has_mobile = lan && assets("/m/index.html").is_some();

    hooks_server::serve(ServeOptions {
        port,
        // Nothing presents a token here, and an empty one matches nothing —
        // access is governed by `no_auth` plus the bind address.
        token: String::new(),
        port_file,
        no_auth: true,
        web_assets: Some(assets),
        host: Some(host),
        // Printed from the callback rather than here because `--port 0` means
        // the port in the URL is not known until the listener is up.
        on_listen: lan.then_some(Box::new(move |bound_host: &str, bound_port: u16| {
            eprintln!("{}", lan_banner(bound_host, bound_port, has_mobile));
        }) as Box<dyn FnOnce(&str, u16) + Send>),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp dir that cleans itself up, so the marker tests never touch a real
    /// `~/.fleet`.
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "fleet-webui-bootstrap-{tag}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn first_run_installs_and_records_the_marker() {
        let dir = temp_dir("first");
        let marker = dir.join(WEBUI_BOOTSTRAP_MARKER);
        let mut ran = 0;

        first_run_bootstrap_at(&marker, || {
            ran += 1;
            vec![super::super::bootstrap::Step { name: "fake", result: Ok(()) }]
        });

        assert_eq!(ran, 1, "a host with no marker must get the control plane");
        assert!(marker.is_file(), "the install must be recorded");
        let body: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&marker).unwrap()).unwrap();
        assert_eq!(body["failed_steps"], 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_start_does_not_reinstall() {
        // The whole point of the marker: a user who switched a mode OFF in the
        // UI (state the backend cannot see) must not have it reinstated by the
        // next `fleet webui` boot.
        let dir = temp_dir("second");
        let marker = dir.join(WEBUI_BOOTSTRAP_MARKER);
        std::fs::write(&marker, "{}").unwrap();
        let mut ran = 0;

        first_run_bootstrap_at(&marker, || {
            ran += 1;
            vec![]
        });

        assert_eq!(ran, 0, "an existing marker must suppress the install");
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "{}", "and not be rewritten");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failing_step_still_marks_the_host_but_counts_the_failure() {
        // Retrying a persistently failing step on every boot would re-run the
        // whole install each time — exactly the reinstatement the gate prevents.
        let dir = temp_dir("failing");
        let marker = dir.join(WEBUI_BOOTSTRAP_MARKER);

        first_run_bootstrap_at(&marker, || {
            vec![
                super::super::bootstrap::Step { name: "ok", result: Ok(()) },
                super::super::bootstrap::Step { name: "bad", result: Err("boom".into()) },
            ]
        });

        let body: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&marker).unwrap()).unwrap();
        assert_eq!(body["failed_steps"], 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_marker_lands_in_the_fleet_dir() {
        // Guards the wiring the injected-install tests deliberately skip: the
        // real entry point must resolve its marker under ~/.fleet, alongside the
        // permissions/MCP injector configs.
        let dir = temp_dir("home");
        let _guard = claw_fleet_core::paths::fleet_home_lock();
        let prev = std::env::var_os("FLEET_HOME");
        unsafe { std::env::set_var("FLEET_HOME", &dir) };

        let resolved = claw_fleet_core::session::get_fleet_dir().map(|d| d.join(WEBUI_BOOTSTRAP_MARKER));
        assert_eq!(resolved, Some(dir.join(".fleet").join(WEBUI_BOOTSTRAP_MARKER)));

        match prev {
            Some(v) => unsafe { std::env::set_var("FLEET_HOME", v) },
            None => unsafe { std::env::remove_var("FLEET_HOME") },
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_flag_beats_env_beats_default() {
        assert_eq!(
            resolve_webui_host(Some("10.0.0.5".into()), false, Some("0.0.0.0".into())),
            "10.0.0.5"
        );
        assert_eq!(resolve_webui_host(None, false, Some("0.0.0.0".into())), "0.0.0.0");
        assert_eq!(resolve_webui_host(None, false, None), DEFAULT_WEBUI_HOST);
    }

    #[test]
    fn empty_host_env_is_not_a_value() {
        // Docker passes `-e FLEET_WEBUI_HOST` with no value as an empty string;
        // binding "" would fail, so it has to read as "unset".
        assert_eq!(
            resolve_webui_host(None, false, Some(String::new())),
            DEFAULT_WEBUI_HOST
        );
    }

    #[test]
    fn lan_widens_the_bind_but_loses_to_an_explicit_host() {
        assert_eq!(resolve_webui_host(None, true, None), LAN_WEBUI_HOST);
        // Typed this run, so it outranks ambient deployment config.
        assert_eq!(
            resolve_webui_host(None, true, Some("127.0.0.1".into())),
            LAN_WEBUI_HOST
        );
        // ...but naming an interface means that interface.
        assert_eq!(
            resolve_webui_host(Some("192.168.1.5".into()), true, None),
            "192.168.1.5"
        );
    }

    #[test]
    fn advertised_host_resolves_wildcards_only() {
        // A named address is already what the phone types.
        assert_eq!(advertised_host("192.168.1.5").as_deref(), Some("192.168.1.5"));
        // The wildcards are not connectable, so they resolve to the LAN IP —
        // or to None on a machine that has none (CI containers, offline).
        for wildcard in ["0.0.0.0", "::"] {
            match advertised_host(wildcard) {
                Some(ip) => assert_ne!(ip, wildcard),
                None => {}
            }
        }
    }

    #[test]
    fn lan_banner_carries_both_urls_and_a_qr() {
        let out = lan_banner("192.168.1.5", 4571, true);
        assert!(out.contains("http://192.168.1.5:4571/"));
        assert!(out.contains("http://192.168.1.5:4571/m/"));
        assert!(out.contains('█') || out.contains('▀') || out.contains('▄'));
    }

    #[test]
    fn lan_banner_without_a_mobile_bundle_says_so() {
        let out = lan_banner("192.168.1.5", 4571, false);
        assert!(out.contains("http://192.168.1.5:4571/"));
        // No /m/ URL advertised, and none in the QR either (the explanatory
        // line mentions the path, hence matching on the URL rather than "/m/").
        assert!(!out.contains(":4571/m/"));
        assert!(out.contains("不含 /m/"));
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
