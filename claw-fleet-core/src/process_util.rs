//! Cross-platform `std::process::Command` hardening for desktop / GUI contexts.
//!
//! On Windows, spawning a child process from a GUI binary briefly flashes a
//! conhost window unless `CREATE_NO_WINDOW` (`0x0800_0000`) is set on the
//! creation flags. Call [`no_window`] on every `Command` before `.spawn()` /
//! `.output()` / `.status()` to suppress that flash. On non-Windows targets
//! the helper is a no-op so call sites stay portable.

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::Command;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Apply `CREATE_NO_WINDOW` on Windows, no-op elsewhere. Returns the same
/// `&mut Command` so it chains naturally before `.output()` / `.spawn()`.
pub fn no_window(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Construct a `Command` with `CREATE_NO_WINDOW` already applied on Windows.
/// Lets call sites keep the one-liner `process_util::command("foo").arg("y").output()`
/// pattern without manually plumbing a `&mut`.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    no_window(&mut cmd);
    cmd
}

/// Locate `bin` on the caller's PATH via the platform lookup command
/// (`which` on Unix, `where` on Windows). Returns the first match only —
/// `where` on Windows can list several, one per line.
pub fn which(bin: &str) -> Option<String> {
    #[cfg(unix)]
    let lookup = "which";
    #[cfg(not(unix))]
    let lookup = "where";
    let output = command(lookup).arg(bin).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout.lines().next()?.trim().to_string();
    if first.is_empty() {
        None
    } else {
        Some(first)
    }
}

/// Run a caller-supplied shell command string through the platform shell:
/// `sh -c` on Unix, `cmd /C` on Windows. Window-suppressed on Windows via
/// [`command`]. For agent-authored command strings (`fleet watch --until` /
/// `--capture`) that must be evaluated on whatever platform the watch runs
/// on — a bare `Command::new("sh")` never resolves on a stock Windows host,
/// so the condition would silently read false forever.
pub fn shell_command(script: &str) -> Command {
    #[cfg(unix)]
    let (shell, flag) = ("sh", "-c");
    #[cfg(not(unix))]
    let (shell, flag) = ("cmd", "/C");
    let mut cmd = command(shell);
    cmd.arg(flag).arg(script);
    cmd
}

/// Longest stderr excerpt kept from a gate evaluation. Enough to carry a
/// `command not found` / traceback first lines without bloating the watch
/// record, which is rewritten on every poll.
pub const GATE_STDERR_CAP: usize = 600;

/// What one evaluation of a gate command produced. `met` is the only thing the
/// timers act on; the rest exists so a gate that never fires can be *diagnosed*
/// instead of silently polled for hours — `exit_code` separates "127, the binary
/// is missing" from "1, not yet", and `stderr` carries the shell's own words.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GateOutcome {
    /// Exit status 0 — the condition is met.
    pub met: bool,
    /// Exit code, or `None` when the process was killed by a signal or the
    /// shell could not be spawned at all.
    pub exit_code: Option<i32>,
    /// Trimmed stderr, truncated to [`GATE_STDERR_CAP`]. Empty when the command
    /// said nothing (the normal "not yet" case).
    pub stderr: String,
    /// The evaluation was cut short by a caller-supplied deadline (only
    /// [`gate_probe_bounded`] sets this). The verdict is then "unknown", not
    /// "broken" — a slow gate is a normal thing to wait on.
    pub timed_out: bool,
}

impl GateOutcome {
    /// A gate whose command is *structurally* broken rather than merely unmet:
    /// the shell could not run it (`None`), could not find it (127), or found
    /// something it could not execute (126). These never become true by waiting
    /// — a watch on one of them would burn its whole deadline for nothing.
    pub fn is_structural_failure(&self) -> bool {
        !self.met && !self.timed_out && matches!(self.exit_code, None | Some(126) | Some(127))
    }

    /// One-line human summary for logs, `watch list` and resume prompts.
    pub fn summary(&self) -> String {
        if self.timed_out {
            return format!("超时未返回: {}", self.stderr);
        }
        let code = match self.exit_code {
            Some(c) => c.to_string(),
            None => "signal/spawn-failure".to_string(),
        };
        if self.stderr.is_empty() {
            format!("exit {code}")
        } else {
            format!("exit {code}: {}", self.stderr)
        }
    }
}

/// Run a gate command (`--until`) through the platform shell and report the
/// full outcome. stdin/stdout are nulled (a chatty gate must not buffer
/// megabytes every poll); **stderr is captured** — it is the only evidence of
/// *why* a gate stays false, and throwing it away is what made a mistyped
/// `--until` indistinguishable from "the job is still running".
///
/// A spawn failure (shell missing, command unrunnable) reads as not met with
/// `exit_code: None` and is logged.
///
/// Shared by `watch` (`--until`), `schedule` (`--until` gate) and `agent_loop`
/// (`--until` per-tick gate) so all three evaluate a gate identically.
pub fn gate_probe(cmd: &str) -> GateOutcome {
    match shell_command(cmd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
    {
        Ok(out) => GateOutcome {
            met: out.status.success(),
            exit_code: out.status.code(),
            stderr: truncate_chars(String::from_utf8_lossy(&out.stderr).trim(), GATE_STDERR_CAP),
            timed_out: false,
        },
        Err(e) => {
            crate::log_debug(&format!("gate poll: cannot run until-command ({e}): {cmd}"));
            GateOutcome {
                met: false,
                exit_code: None,
                stderr: format!("cannot run: {e}"),
                timed_out: false,
            }
        }
    }
}

/// [`gate_probe`] with a wall-clock deadline: the child is killed if it outlives
/// `limit`, and the outcome comes back `timed_out` rather than met/unmet.
///
/// Only the *preflight* (one synchronous run at registration, inside a tool
/// call the agent is waiting on) needs this — the polling timers run unbounded
/// on purpose, since a gate that legitimately takes minutes is normal there.
/// Deliberately not `timeout(1)`: this machine has no GNU timeout and Windows
/// has none at all.
pub fn gate_probe_bounded(cmd: &str, limit: std::time::Duration) -> GateOutcome {
    let mut child = match shell_command(cmd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            crate::log_debug(&format!("gate preflight: cannot run until-command ({e}): {cmd}"));
            return GateOutcome {
                met: false,
                exit_code: None,
                stderr: format!("cannot run: {e}"),
                timed_out: false,
            };
        }
    };
    let deadline = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stderr = child
                    .wait_with_output()
                    .map(|o| String::from_utf8_lossy(&o.stderr).trim().to_string())
                    .unwrap_or_default();
                return GateOutcome {
                    met: status.success(),
                    exit_code: status.code(),
                    stderr: truncate_chars(&stderr, GATE_STDERR_CAP),
                    timed_out: false,
                };
            }
            Ok(None) => {}
            Err(e) => {
                return GateOutcome {
                    met: false,
                    exit_code: None,
                    stderr: format!("cannot wait: {e}"),
                    timed_out: false,
                };
            }
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return GateOutcome {
                met: false,
                exit_code: None,
                stderr: format!("{}s 内未返回，已终止", limit.as_secs()),
                timed_out: true,
            };
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
}

/// Truncate on a char boundary, appending an ellipsis when anything was cut.
fn truncate_chars(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let mut out: String = s.chars().take(cap).collect();
    out.push('…');
    out
}

/// Exit-status-only view of [`gate_probe`], for the call sites that have nowhere
/// to put a diagnosis (schedule / loop per-tick gates).
pub fn gate_met(cmd: &str) -> bool {
    gate_probe(cmd).met
}

/// Put the child in its own process group (Unix), no-op on Windows.
///
/// For long-lived agent children only (claude / codex sessions). A child left
/// in the spawner's group receives every group-wide signal aimed at the Fleet
/// process — a terminal Ctrl-C at a dev build SIGINTs all in-flight agents,
/// aborting their turns (`turn_aborted reason='interrupted'` on Codex). Do NOT
/// apply this to short-lived tool invocations (`which`, `git`, …): those
/// *should* die with the caller.
///
/// Fleet's hard-stop path still walks explicit descendants. The graceful
/// interrupt path uses this dedicated group as an additional ownership signal
/// when cleaning up a tool that deliberately daemonized itself and left the
/// parent/child tree before the click.
pub fn detach_process_group(cmd: &mut Command) -> &mut Command {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
}

/// Restore `SIG_DFL` for any of SIGINT / SIGTERM / SIGHUP this process inherited
/// as `SIG_IGN`, and report which ones were cleared.
///
/// A Fleet process that installs a termination handler has to call this first.
/// `ctrlc::try_set_handler` refuses to install when any of those three has a
/// non-`SIG_DFL` disposition (`platform::unix::init_os_handler` returns `EEXIST`,
/// surfaced as `MultipleHandlers`), and an inherited `SIG_IGN` is exactly that.
///
/// It is reachable in ordinary use, not a corner case: a non-interactive shell
/// sets SIGINT to `SIG_IGN` for background jobs and `nohup` does the same for
/// SIGHUP, so `fleet serve &` from a script — how Fleet's own harnesses and
/// launchers start it — lands here. Measured: `fleet serve` started that way
/// logged "ctrlc handler install failed" and left its `dsh web` reparented to
/// init on SIGTERM; started with default dispositions, the same binary and the
/// same signal reaped the child.
///
/// Only `SIG_IGN` can arrive this way — `execve` resets handlers to `SIG_DFL` and
/// keeps only ignores — so nothing else is touched, and a deliberate in-process
/// handler installed later is unaffected.
#[cfg(unix)]
pub fn clear_inherited_signal_ignores() -> Vec<&'static str> {
    let mut cleared = Vec::new();
    for (sig, name) in [
        (libc::SIGINT, "SIGINT"),
        (libc::SIGTERM, "SIGTERM"),
        (libc::SIGHUP, "SIGHUP"),
    ] {
        // SAFETY: `sigaction` with a null `act` only queries; the write that
        // follows installs `SIG_DFL`, the disposition the process would have had
        // if nothing had ignored the signal for us.
        unsafe {
            let mut current: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(sig, std::ptr::null(), &mut current) != 0 {
                continue;
            }
            if current.sa_sigaction != libc::SIG_IGN {
                continue;
            }
            let mut default: libc::sigaction = std::mem::zeroed();
            default.sa_sigaction = libc::SIG_DFL;
            if libc::sigaction(sig, &default, std::ptr::null_mut()) == 0 {
                cleared.push(name);
            }
        }
    }
    cleared
}

/// No-op on Windows: there is no `SIG_IGN` inheritance, and ctrlc uses the
/// console control handler rather than `sigaction`.
#[cfg(not(unix))]
pub fn clear_inherited_signal_ignores() -> Vec<&'static str> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_probe_reports_exit_code_and_stderr() {
        let out = gate_probe("echo boom 1>&2; exit 3");
        assert!(!out.met);
        assert_eq!(out.exit_code, Some(3));
        assert_eq!(out.stderr, "boom");
        assert!(!out.is_structural_failure(), "exit 3 is a plain unmet gate");
        assert_eq!(out.summary(), "exit 3: boom");
    }

    #[test]
    fn gate_probe_marks_missing_command_structural() {
        // 127 is what every POSIX shell returns for "command not found"; the
        // whole point of the diagnosis is that this never becomes true by waiting.
        let out = gate_probe("fleet-no-such-binary-xyz --version");
        assert!(!out.met);
        assert_eq!(out.exit_code, Some(127));
        assert!(out.is_structural_failure());
        assert!(!out.stderr.is_empty(), "the shell explains itself on stderr");
    }

    #[test]
    fn gate_probe_met_on_zero_and_gate_met_agrees() {
        let out = gate_probe("exit 0");
        assert!(out.met);
        assert!(!out.is_structural_failure());
        assert!(gate_met("exit 0"));
        assert!(!gate_met("exit 1"));
    }

    #[test]
    fn gate_probe_bounded_kills_a_hanging_gate_and_reports_unknown() {
        let out = gate_probe_bounded("sleep 30", std::time::Duration::from_millis(300));
        assert!(out.timed_out);
        assert!(!out.met);
        assert!(
            !out.is_structural_failure(),
            "a slow gate is unknown, not broken — registering it must still be allowed"
        );
    }

    #[test]
    fn gate_probe_bounded_returns_a_fast_verdict_intact() {
        let out = gate_probe_bounded("echo nope 1>&2; exit 4", std::time::Duration::from_secs(5));
        assert!(!out.timed_out);
        assert_eq!(out.exit_code, Some(4));
        assert_eq!(out.stderr, "nope");
    }

    #[test]
    fn gate_probe_truncates_long_stderr_on_char_boundary() {
        // Multi-byte chars: a naive byte slice would panic here.
        let out = gate_probe("python3 -c \"import sys;sys.stderr.write('中'*5000)\"; exit 1");
        assert!(out.stderr.chars().count() <= GATE_STDERR_CAP + 1, "capped + ellipsis");
        assert!(out.stderr.ends_with('…'));
    }

    #[test]
    fn no_window_returns_same_command_and_still_runs() {
        #[cfg(unix)]
        let mut cmd = Command::new("true");
        #[cfg(windows)]
        let mut cmd = {
            let mut c = Command::new("cmd");
            c.args(["/C", "exit"]);
            c
        };

        let status = no_window(&mut cmd).status().expect("spawn");
        assert!(status.success());
    }

    #[test]
    fn shell_command_evaluates_the_platform_shell_string() {
        // `exit 0` / `exit 1` parse identically under sh and cmd, so this
        // exercises the real platform shell on whichever host runs the tests.
        assert!(shell_command("exit 0").status().expect("spawn").success());
        assert!(!shell_command("exit 1").status().expect("spawn").success());
    }

    #[test]
    fn command_constructor_runs() {
        #[cfg(unix)]
        let status = command("true").status().expect("spawn");
        #[cfg(windows)]
        let status = command("cmd").args(["/C", "exit"]).status().expect("spawn");
        assert!(status.success());
    }
}
