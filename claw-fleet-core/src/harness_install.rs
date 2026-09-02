//! Harness install/upgrade orchestration for the environment wizard.
//!
//! Runs each harness's **official** installer as a subprocess and streams its
//! output lines to a progress callback, so the desktop wizard can render a
//! live checklist instead of a frozen spinner. Fleet deliberately builds no
//! installer of its own: every channel here is the vendor's supported path
//! (claude.ai/install.sh|ps1, chatgpt.com/codex/install.sh|ps1, npm for dsh),
//! which also hands auto-update responsibility to the harness itself.
//!
//! Like the rca installer this is invoked from a desktop-layer Tauri command
//! (progress needs an `AppHandle` event channel); the phase-2 remote install
//! will reuse these same script builders over rca/SSH.
//!
//! All URLs verified live (HTTP 200) on 2026-09-01.

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Official installer entry points. `claude.ai/install.sh` redirects to
/// Anthropic's release bucket and `chatgpt.com/codex/install.sh` to
/// releases.openai.com — pin the stable vanity URLs, not the redirect targets.
const CLAUDE_INSTALL_SH: &str = "https://claude.ai/install.sh";
const CLAUDE_INSTALL_PS1: &str = "https://claude.ai/install.ps1";
const CODEX_INSTALL_SH: &str = "https://chatgpt.com/codex/install.sh";
const CODEX_INSTALL_PS1: &str = "https://chatgpt.com/codex/install.ps1";
const DSH_NPM_PACKAGE: &str = "@deepseek-ai/dsh";

/// Network installs legitimately run minutes (npm's dsh tree is ~300 MB); this
/// bounds a wedged installer, not a slow one.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// How many trailing output lines to keep for the error report when an
/// installer fails.
const ERROR_TAIL_LINES: usize = 12;

/// Stable, i18n-ready failure classification (mirrors the fleet-cli installer's
/// `localizeInstallError` pattern: the frontend maps `code`, `message` is the
/// raw detail for the debug log / "show details" affordance).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum InstallErrorCode {
    UnsupportedSource,
    /// dsh needs a Node.js runtime before npm can install it (wizard offers
    /// the node bootstrap or skipping dsh).
    NodeMissing,
    SpawnFailed,
    InstallFailed,
    Timeout,
    /// Installer exited 0 but the post-install probe still can't see the
    /// harness — report loudly instead of a false success.
    VerifyFailed,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InstallError {
    pub code: InstallErrorCode,
    pub message: String,
}

impl InstallError {
    fn new(code: InstallErrorCode, message: impl Into<String>) -> Self {
        InstallError { code, message: message.into() }
    }
}

/// The subprocess an install action runs: program + args + extra env.
/// Split from execution so every platform/source combination is unit-testable
/// without touching the network.
#[derive(Debug, Clone, PartialEq)]
pub struct InstallPlan {
    pub program: String,
    pub args: Vec<String>,
    pub envs: Vec<(String, String)>,
}

/// Build the official-installer invocation for `source` on this platform.
pub fn install_plan(source: &str) -> Result<InstallPlan, InstallError> {
    match source {
        "claude-code" => Ok(pipe_installer_plan(CLAUDE_INSTALL_SH, CLAUDE_INSTALL_PS1, &[])),
        "codex" => Ok(pipe_installer_plan(
            CODEX_INSTALL_SH,
            CODEX_INSTALL_PS1,
            // The flag codex's own self-update uses for unattended runs.
            &[("CODEX_NON_INTERACTIVE", "1")],
        )),
        "dsh" => dsh_install_plan(),
        other => Err(InstallError::new(
            InstallErrorCode::UnsupportedSource,
            format!("unknown harness source '{other}'"),
        )),
    }
}

/// `curl | sh` on unix, `irm | iex` under PowerShell on Windows — exactly the
/// commands the vendors document, no local re-implementation of their logic.
fn pipe_installer_plan(sh_url: &str, ps1_url: &str, envs: &[(&str, &str)]) -> InstallPlan {
    let envs = envs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    #[cfg(unix)]
    {
        InstallPlan {
            program: "sh".into(),
            args: vec!["-c".into(), format!("curl -fsSL '{sh_url}' | sh")],
            envs,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = sh_url;
        InstallPlan {
            program: "powershell".into(),
            args: vec![
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-Command".into(),
                format!("irm {ps1_url} | iex"),
            ],
            envs,
        }
    }
}

/// dsh's only channel is npm (`@deepseek-ai/dsh`). Homebrew's `dsh` formula is
/// an unrelated package (Dancer's shell) — never offer it. When no npm exists
/// the error is the structured `NodeMissing` the wizard's node-bootstrap step
/// keys off.
fn dsh_install_plan() -> Result<InstallPlan, InstallError> {
    let npm = find_npm().ok_or_else(|| {
        InstallError::new(
            InstallErrorCode::NodeMissing,
            "npm not found — dsh installs via npm, which needs a Node.js runtime",
        )
    })?;
    Ok(InstallPlan {
        program: npm.to_string_lossy().into_owned(),
        args: vec!["install".into(), "-g".into(), DSH_NPM_PACKAGE.into()],
        envs: vec![],
    })
}

/// Locate npm by scanning the same augmented PATH Fleet gives spawned agents —
/// a GUI app's own PATH is only the system dirs, so a plain `which npm` misses
/// homebrew/nvm/fnm installs.
pub fn find_npm() -> Option<PathBuf> {
    #[cfg(windows)]
    let names: &[&str] = &["npm.cmd", "npm.exe", "npm"];
    #[cfg(not(windows))]
    let names: &[&str] = &["npm"];
    find_in_augmented_path(names)
}

fn find_in_augmented_path(names: &[&str]) -> Option<PathBuf> {
    let path = crate::session_launch::augmented_path_with_front(&[]);
    for dir in std::env::split_paths(&path) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Install `source` via its official channel, streaming every installer output
/// line through `progress`, then re-probe and return the fresh status.
///
/// Success is defined by the post-install probe, not the installer's exit
/// code: an installer that exits 0 without producing a runnable binary is a
/// `VerifyFailed`, never a silent "done".
pub fn install_harness(
    source: &str,
    progress: &(dyn Fn(&str) + Sync),
) -> Result<crate::harness_status::HarnessStatus, InstallError> {
    let plan = install_plan(source)?;
    progress(&format!("$ {} {}", plan.program, plan.args.join(" ")));
    run_streaming(&plan, INSTALL_TIMEOUT, progress)?;

    progress("verifying installation…");
    let status = crate::harness_status::probe_source(source).ok_or_else(|| {
        InstallError::new(InstallErrorCode::UnsupportedSource, format!("unknown source '{source}'"))
    })?;
    if !status.installed {
        return Err(InstallError::new(
            InstallErrorCode::VerifyFailed,
            format!("installer finished but no runnable {source} was found on this machine"),
        ));
    }
    progress(&format!(
        "installed: {} {}",
        source,
        status.version.as_deref().unwrap_or("(version unknown)")
    ));
    Ok(status)
}

/// Run an [`InstallPlan`], forwarding stdout+stderr lines to `progress` as
/// they arrive and keeping a tail for the failure report. Kills the child on
/// timeout (same pid-kill as the version probe / llm_provider runners).
fn run_streaming(
    plan: &InstallPlan,
    timeout: Duration,
    progress: &(dyn Fn(&str) + Sync),
) -> Result<(), InstallError> {
    let mut cmd = Command::new(&plan.program);
    crate::process_util::no_window(&mut cmd);
    cmd.args(&plan.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in &plan.envs {
        cmd.env(k, v);
    }
    // The installers themselves shell out (curl, tar, node) — give them the
    // same augmented PATH the GUI lacks.
    cmd.env("PATH", crate::session_launch::augmented_path_with_front(&[]));

    let mut child = cmd
        .spawn()
        .map_err(|e| InstallError::new(InstallErrorCode::SpawnFailed, format!("{}: {e}", plan.program)))?;

    let (tx, rx) = mpsc::channel::<String>();
    for reader in [
        child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(reader).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);

    let deadline = Instant::now() + timeout;
    let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let mut push_line = |line: String| {
        if !line.trim().is_empty() {
            progress(&line);
            if tail.len() == ERROR_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    };

    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => push_line(line),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // Both reader threads finished — the child has closed its pipes;
            // fall through to reap it.
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if Instant::now() >= deadline {
            crate::llm_provider::kill_process(child.id());
            let _ = child.wait();
            return Err(InstallError::new(
                InstallErrorCode::Timeout,
                format!("installer still running after {}s — killed", timeout.as_secs()),
            ));
        }
    }
    // Drain anything the readers sent between the last recv and disconnect.
    while let Ok(line) = rx.try_recv() {
        push_line(line);
    }

    let status = child
        .wait()
        .map_err(|e| InstallError::new(InstallErrorCode::InstallFailed, format!("wait: {e}")))?;
    if !status.success() {
        let tail: Vec<String> = tail.into_iter().collect();
        return Err(InstallError::new(
            InstallErrorCode::InstallFailed,
            format!("installer exited with {status}\n{}", tail.join("\n")),
        ));
    }
    Ok(())
}

// ── Node.js bootstrap (dsh prerequisite) ─────────────────────────────────────
//
// dsh's only install channel is npm, so a blank machine needs a Node runtime
// first. Neither the official .pkg (sudo + GUI) nor Homebrew (may not exist)
// fits an unattended wizard; the official **binary archives** from
// nodejs.org/dist do — user-space unpack into `~/.fleet/node`, no admin
// rights, identical strategy on macOS/Linux (tar.gz) and Windows (zip).
// `augmented_path_with_front` exposes that bin dir to every spawned agent, so
// dsh's `#!/usr/bin/env node` resolves at runtime too.

/// Where the bootstrapped Node lives. Unix archives have a `bin/` inside;
/// Windows zips put node.exe/npm.cmd at the archive root.
pub fn fleet_node_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("node"))
}

pub fn fleet_node_bin_dir() -> Option<PathBuf> {
    let dir = fleet_node_dir()?;
    #[cfg(windows)]
    return Some(dir);
    #[cfg(not(windows))]
    Some(dir.join("bin"))
}

/// nodejs.org release-archive platform slug for this machine.
fn node_platform_slug() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("darwin-arm64"),
        ("macos", "x86_64") => Some("darwin-x64"),
        ("linux", "x86_64") => Some("linux-x64"),
        ("linux", "aarch64") => Some("linux-arm64"),
        ("windows", "x86_64") => Some("win-x64"),
        ("windows", "aarch64") => Some("win-arm64"),
        _ => None,
    }
}

/// Newest LTS version tag ("v24.x.y") from nodejs.org's release index.
/// No hardcoded fallback: a guessed version that stops existing would turn
/// into a mystery 404, while "cannot reach nodejs.org" is self-explanatory.
///
/// Blocking HTTP — callers must be on a plain/blocking thread (the Tauri
/// command wraps everything in `spawn_blocking`), never inside an async task.
fn latest_node_lts() -> Result<String, InstallError> {
    let index: serde_json::Value = reqwest::blocking::Client::new()
        .get("https://nodejs.org/dist/index.json")
        .timeout(Duration::from_secs(30))
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.json())
        .map_err(|e| {
            InstallError::new(
                InstallErrorCode::InstallFailed,
                format!("cannot fetch the Node.js release index: {e}"),
            )
        })?;
    index
        .as_array()
        .into_iter()
        .flatten()
        // Entries are newest-first; LTS entries carry a codename, non-LTS `false`.
        .find(|e| e.get("lts").map(|l| l.as_str().is_some()).unwrap_or(false))
        .and_then(|e| e.get("version").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .ok_or_else(|| {
            InstallError::new(
                InstallErrorCode::InstallFailed,
                "Node.js release index has no LTS entry",
            )
        })
}

/// The unpack script per platform, pure for testing. Downloads into
/// `<dest>.new` and swaps at the end, so a failed download never leaves a
/// half-written `~/.fleet/node` that later probes mistake for an install.
fn node_install_plan_for(version: &str, slug: &str, dest: &std::path::Path) -> InstallPlan {
    let dest = dest.to_string_lossy();
    #[cfg(unix)]
    {
        let url = format!("https://nodejs.org/dist/{version}/node-{version}-{slug}.tar.gz");
        InstallPlan {
            program: "sh".into(),
            args: vec![
                "-c".into(),
                format!(
                    "set -e; rm -rf '{dest}.new'; mkdir -p '{dest}.new'; \
                     curl -fsSL '{url}' | tar xz -C '{dest}.new' --strip-components=1; \
                     rm -rf '{dest}'; mv '{dest}.new' '{dest}'"
                ),
            ],
            envs: vec![],
        }
    }
    #[cfg(not(unix))]
    {
        let url = format!("https://nodejs.org/dist/{version}/node-{version}-{slug}.zip");
        InstallPlan {
            program: "powershell".into(),
            args: vec![
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-Command".into(),
                format!(
                    "$ErrorActionPreference='Stop'; \
                     Invoke-WebRequest -Uri '{url}' -OutFile '{dest}.zip'; \
                     if (Test-Path '{dest}.new') {{ Remove-Item -Recurse -Force '{dest}.new' }}; \
                     Expand-Archive -Path '{dest}.zip' -DestinationPath '{dest}.new'; \
                     Remove-Item '{dest}.zip'; \
                     if (Test-Path '{dest}') {{ Remove-Item -Recurse -Force '{dest}' }}; \
                     Move-Item (Join-Path '{dest}.new' 'node-{version}-{slug}') '{dest}'; \
                     Remove-Item -Recurse -Force '{dest}.new'"
                ),
            ],
            envs: vec![],
        }
    }
}

/// Bootstrap Node.js into `~/.fleet/node` and return the npm path, streaming
/// progress like [`install_harness`]. Success is defined by `npm --version`
/// running from the fresh install.
pub fn install_node(progress: &(dyn Fn(&str) + Sync)) -> Result<PathBuf, InstallError> {
    let slug = node_platform_slug().ok_or_else(|| {
        InstallError::new(
            InstallErrorCode::InstallFailed,
            format!(
                "nodejs.org publishes no binary archive for this platform ({} {})",
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
        )
    })?;
    let dest = fleet_node_dir().ok_or_else(|| {
        InstallError::new(InstallErrorCode::InstallFailed, "cannot resolve home directory")
    })?;

    progress("resolving the current Node.js LTS release…");
    let version = latest_node_lts()?;
    progress(&format!("installing Node.js {version} ({slug}) into {}", dest.display()));
    let plan = node_install_plan_for(&version, slug, &dest);
    run_streaming(&plan, INSTALL_TIMEOUT, progress)?;

    progress("verifying npm…");
    let npm = fleet_node_bin_dir()
        .map(|d| {
            #[cfg(windows)]
            return d.join("npm.cmd");
            #[cfg(not(windows))]
            d.join("npm")
        })
        .filter(|p| p.is_file())
        .ok_or_else(|| {
            InstallError::new(
                InstallErrorCode::VerifyFailed,
                "Node unpacked but npm is missing from the archive",
            )
        })?;
    let out = crate::process_util::command(&npm)
        .arg("--version")
        .env(
            "PATH",
            crate::session_launch::augmented_path_with_front(&[]),
        )
        .output()
        .map_err(|e| InstallError::new(InstallErrorCode::VerifyFailed, format!("npm: {e}")))?;
    if !out.status.success() {
        return Err(InstallError::new(
            InstallErrorCode::VerifyFailed,
            format!("npm --version failed: {}", String::from_utf8_lossy(&out.stderr).trim()),
        ));
    }
    progress(&format!(
        "npm {} ready",
        String::from_utf8_lossy(&out.stdout).trim()
    ));
    Ok(npm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn collect() -> (Box<dyn Fn(&str) + Sync>, std::sync::Arc<Mutex<Vec<String>>>) {
        let lines = std::sync::Arc::new(Mutex::new(Vec::new()));
        let sink = lines.clone();
        (
            Box::new(move |l: &str| sink.lock().unwrap().push(l.to_string())),
            lines,
        )
    }

    #[test]
    #[cfg(unix)]
    fn install_plan_uses_official_vendor_scripts() {
        let claude = install_plan("claude-code").unwrap();
        assert_eq!(claude.program, "sh");
        assert!(claude.args[1].contains("https://claude.ai/install.sh"));
        assert!(claude.envs.is_empty());

        let codex = install_plan("codex").unwrap();
        assert!(codex.args[1].contains("https://chatgpt.com/codex/install.sh"));
        assert_eq!(
            codex.envs,
            vec![("CODEX_NON_INTERACTIVE".to_string(), "1".to_string())]
        );
    }

    #[test]
    fn install_plan_rejects_unknown_source() {
        let err = install_plan("copilot").unwrap_err();
        assert_eq!(err.code, InstallErrorCode::UnsupportedSource);
    }

    #[test]
    fn dsh_plan_is_npm_or_structured_node_missing() {
        // Whichever machine runs this: with npm present the plan must target
        // the real @deepseek-ai/dsh package (never a brew formula — the brew
        // `dsh` is the unrelated Dancer's shell); without npm the error must
        // be the structured NodeMissing the wizard branches on.
        match install_plan("dsh") {
            Ok(plan) => {
                assert!(plan.program.contains("npm"));
                assert_eq!(plan.args, vec!["install", "-g", "@deepseek-ai/dsh"]);
            }
            Err(e) => assert_eq!(e.code, InstallErrorCode::NodeMissing),
        }
    }

    #[test]
    #[cfg(unix)]
    fn run_streaming_forwards_both_streams_and_succeeds() {
        let (progress, lines) = collect();
        let plan = InstallPlan {
            program: "sh".into(),
            args: vec!["-c".into(), "echo out1; echo err1 >&2; echo out2".into()],
            envs: vec![],
        };
        run_streaming(&plan, Duration::from_secs(10), &*progress).unwrap();
        let got = lines.lock().unwrap().clone();
        assert!(got.contains(&"out1".to_string()));
        assert!(got.contains(&"err1".to_string()));
        assert!(got.contains(&"out2".to_string()));
    }

    #[test]
    #[cfg(unix)]
    fn run_streaming_reports_failure_with_output_tail() {
        let (progress, _lines) = collect();
        let plan = InstallPlan {
            program: "sh".into(),
            args: vec!["-c".into(), "echo boom reason >&2; exit 3".into()],
            envs: vec![],
        };
        let err = run_streaming(&plan, Duration::from_secs(10), &*progress).unwrap_err();
        assert_eq!(err.code, InstallErrorCode::InstallFailed);
        assert!(err.message.contains("boom reason"), "tail missing: {}", err.message);
    }

    #[test]
    #[cfg(unix)]
    fn run_streaming_kills_on_timeout() {
        let (progress, _lines) = collect();
        let plan = InstallPlan {
            program: "sh".into(),
            args: vec!["-c".into(), "sleep 30".into()],
            envs: vec![],
        };
        let start = Instant::now();
        let err = run_streaming(&plan, Duration::from_secs(1), &*progress).unwrap_err();
        assert_eq!(err.code, InstallErrorCode::Timeout);
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    #[cfg(unix)]
    fn node_install_plan_downloads_official_archive_and_swaps_atomically() {
        let dest = std::path::Path::new("/Users/u/.fleet/node");
        let plan = node_install_plan_for("v24.7.0", "darwin-arm64", dest);
        let script = &plan.args[1];
        assert!(script.contains("https://nodejs.org/dist/v24.7.0/node-v24.7.0-darwin-arm64.tar.gz"));
        // Unpack into .new first, swap last — no half-written ~/.fleet/node.
        assert!(script.contains("/Users/u/.fleet/node.new"));
        assert!(script.contains("mv '/Users/u/.fleet/node.new' '/Users/u/.fleet/node'"));
        assert!(script.contains("--strip-components=1"));
    }

    #[test]
    fn node_platform_slug_known_on_this_machine() {
        // Every platform we build the desktop app for has an official archive.
        assert!(node_platform_slug().is_some());
    }

    /// Manual smoke: real download of the current LTS into ~/.fleet/node.
    /// `cargo test -p claw-fleet-core --lib node_bootstrap_smoke -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn node_bootstrap_smoke_real_download() {
        let (progress, _lines) = collect();
        let npm = install_node(&*progress).unwrap();
        println!("npm at {}", npm.display());
        assert!(npm.is_file());
    }

    #[test]
    fn run_streaming_spawn_failure_is_structured() {
        let (progress, _lines) = collect();
        let plan = InstallPlan {
            program: "/does/not/exist/installer".into(),
            args: vec![],
            envs: vec![],
        };
        let err = run_streaming(&plan, Duration::from_secs(1), &*progress).unwrap_err();
        assert_eq!(err.code, InstallErrorCode::SpawnFailed);
    }
}
