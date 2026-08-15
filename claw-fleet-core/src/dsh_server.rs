//! Lifecycle of the `dsh web` server Fleet talks to over [`crate::dsh_client`].
//!
//! Unlike Claude Code and Codex — whose sessions are files on disk Fleet reads
//! directly — dsh exposes its sessions only through a running server. Fleet
//! therefore owns that process: it starts one `dsh web` per workspace root,
//! learns the port the OS assigned it, health-checks it, and kills it on exit.
//!
//! The server is deliberately **not** detached. `dsh web` has no authentication
//! layer (only a Host-header loopback fence), so a stray instance surviving
//! Fleet would leave an unauthenticated port open that can read every session
//! and start new ones. Its lifetime is bound to ours through [`Drop`].

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::dsh_client::DshClient;

/// How long to wait for the server to print its listen URL. A cold profile
/// materializes its plugin tree on first launch, so this is generous.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);

/// How long to keep retrying `host.describe` after the URL appears. The port is
/// printed by the launcher shell, which can win the race against the listener.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(15);

/// Locate the `dsh` executable.
///
/// PATH first, then the standard global-npm install locations. Fleet never
/// falls back to `npx`: a cold `npx @deepseek-ai/dsh` downloads ~300 MB before
/// it serves anything, which would turn "start a session" into a multi-minute
/// stall with no way to report progress. A user who wants dsh installs it
/// (`npm i -g @deepseek-ai/dsh`), exactly like Claude Code and Codex.
pub fn discover() -> Option<PathBuf> {
    // Explicit override, same escape hatch `claude_binary` gives for a Claude
    // install Fleet cannot find. Also the only way to point at an `npx`-cached
    // copy, which is how this source is tested on a machine without a global
    // install.
    if let Some(p) = std::env::var_os("FLEET_DSH_BIN") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }

    if let Some(p) = crate::process_util::which("dsh") {
        return Some(PathBuf::from(p));
    }

    let home = dirs::home_dir();
    let candidates = [
        home.as_ref()
            .map(|h| h.join(".npm-global").join("bin").join("dsh")),
        Some(PathBuf::from("/opt/homebrew/bin/dsh")),
        Some(PathBuf::from("/usr/local/bin/dsh")),
        home.as_ref()
            .map(|h| h.join(".local").join("bin").join("dsh")),
    ];
    candidates.into_iter().flatten().find(|p| p.exists())
}

/// Is dsh installed on this machine?
pub fn is_available() -> bool {
    discover().is_some()
}

/// Extract the listening port from one launcher stdout line.
///
/// `dsh web` prints exactly `dsh web: http://127.0.0.1:<port>` once the server
/// is up. With `--port 0` that port is OS-assigned, so parsing this line is the
/// only way to learn it — which is also why Fleet uses `--port 0`: it never has
/// to guess a free port or collide with another instance.
fn parse_port_line(line: &str) -> Option<u16> {
    let rest = line.split("http://127.0.0.1:").nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// A running `dsh web` instance owned by this process.
pub struct DshServer {
    child: Child,
    port: u16,
    binary: PathBuf,
    workspace: PathBuf,
}

impl DshServer {
    /// Start a server rooted at `workspace` and wait until it answers RPC.
    ///
    /// The invoking directory is dsh's default workspace root, so `workspace`
    /// decides which project new sessions belong to.
    pub fn start(binary: &Path, workspace: &Path) -> Result<Self, String> {
        if !workspace.is_dir() {
            return Err(format!("workspace does not exist: {}", workspace.display()));
        }

        let mut cmd = crate::process_util::command(binary);
        cmd.arg("web")
            .arg("--port")
            .arg("0")
            .current_dir(workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", binary.display()))?;

        let port = match read_port(&mut child) {
            Ok(port) => port,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };

        let mut server = Self {
            child,
            port,
            binary: binary.to_path_buf(),
            workspace: workspace.to_path_buf(),
        };

        if let Err(e) = server.wait_healthy() {
            server.stop();
            return Err(e);
        }

        Ok(server)
    }

    /// The OS-assigned port this instance listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The workspace root this instance was started in.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// An RPC client pointed at this instance.
    pub fn client(&self) -> Result<DshClient, String> {
        DshClient::new(self.port).map_err(Into::into)
    }

    /// Has the process exited? Reaps it when it has, so a crashed server does
    /// not linger as a zombie until Fleet quits.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Restart after a crash, replacing the child and the port.
    ///
    /// The port changes: the old one was OS-assigned and the new listener gets
    /// its own, so every cached [`DshClient`] must be rebuilt from [`client`].
    ///
    /// [`client`]: Self::client
    pub fn restart(&mut self) -> Result<(), String> {
        self.stop();
        let mut fresh = Self::start(&self.binary, &self.workspace)?;
        // Swap the handles rather than moving out of `fresh` (this type has a
        // Drop impl, so it cannot be destructured). After the swap `fresh` owns
        // the already-reaped dead child, and its Drop is a no-op.
        std::mem::swap(&mut self.child, &mut fresh.child);
        self.port = fresh.port;
        Ok(())
    }

    /// Restart only if the process is gone. Cheap enough to call per poll.
    pub fn ensure_alive(&mut self) -> Result<(), String> {
        if self.is_alive() {
            return Ok(());
        }
        self.restart()
    }

    /// Terminate the server and reap it. Idempotent.
    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Poll `host.describe` until it answers or the health budget runs out.
    fn wait_healthy(&mut self) -> Result<(), String> {
        let client = self.client()?;
        let deadline = Instant::now() + HEALTH_TIMEOUT;
        let mut last = String::new();
        while Instant::now() < deadline {
            if !self.is_alive() {
                return Err("dsh web exited during startup".into());
            }
            match client.call("host.describe", serde_json::json!({})) {
                Ok(_) => return Ok(()),
                Err(e) => last = e.to_string(),
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err(format!("dsh web never answered host.describe: {last}"))
    }
}

impl Drop for DshServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Read the launcher's stdout on a helper thread until it prints its URL.
///
/// The read must not happen on this thread: a server that fails to start (bad
/// profile, port refused, missing artifacts) prints nothing and never closes
/// stdout, so a direct `read_line` would block forever instead of timing out.
fn read_port(child: &mut Child) -> Result<u16, String> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "dsh web produced no stdout pipe".to_string())?;

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(port) = parse_port_line(&line) {
                let _ = tx.send(port);
                return;
            }
        }
        // stdout closed without a URL — let the receiver fail on disconnect
        // rather than wait out the whole timeout.
    });

    match rx.recv_timeout(STARTUP_TIMEOUT) {
        Ok(port) => Ok(port),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "dsh web did not report a port within {STARTUP_TIMEOUT:?}"
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("dsh web exited before reporting a port".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_launcher_url_line() {
        // Verbatim from a live `dsh web --port 0` run.
        assert_eq!(
            parse_port_line("dsh web: http://127.0.0.1:63234"),
            Some(63234)
        );
    }

    #[test]
    fn parses_a_fixed_port_line() {
        assert_eq!(
            parse_port_line("dsh web: http://127.0.0.1:3080"),
            Some(3080)
        );
    }

    #[test]
    fn tolerates_trailing_text_after_the_port() {
        assert_eq!(
            parse_port_line("dsh web: http://127.0.0.1:3080/ (press ctrl-c to stop)"),
            Some(3080)
        );
    }

    #[test]
    fn ignores_unrelated_lines() {
        assert_eq!(parse_port_line("npm warn deprecated foo@1.0.0"), None);
        assert_eq!(parse_port_line(""), None);
        // A non-loopback URL is not ours to talk to: the /api fence would 403 it.
        assert_eq!(parse_port_line("serving http://0.0.0.0:3080"), None);
    }

    #[test]
    fn rejects_a_port_that_is_not_a_number() {
        assert_eq!(parse_port_line("dsh web: http://127.0.0.1:abc"), None);
    }

    #[test]
    fn start_rejects_a_missing_workspace() {
        // `unwrap_err` would need DshServer: Debug; a running server is not a
        // thing this type should be able to print.
        let err = match DshServer::start(
            Path::new("/nonexistent/dsh"),
            Path::new("/nonexistent/workspace-dir"),
        ) {
            Err(e) => e,
            Ok(_) => panic!("a missing workspace must not start a server"),
        };
        assert!(err.contains("workspace does not exist"), "{err}");
    }
}
