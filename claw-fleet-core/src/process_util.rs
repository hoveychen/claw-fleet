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

/// Run a gate command (`--until`) through the platform shell; exit status 0 ⇒
/// the condition is met. stdin/stdout/stderr are nulled — only the exit code
/// matters. A spawn failure (shell missing, command unrunnable) reads as "not
/// met" and is logged, so a watch/schedule/loop stuck on a broken gate is
/// diagnosable from the debug log rather than silently waiting forever.
///
/// Shared by `watch` (`--until`), `schedule` (`--until` gate) and `agent_loop`
/// (`--until` per-tick gate) so all three evaluate a gate identically.
pub fn gate_met(cmd: &str) -> bool {
    match shell_command(cmd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(s) => s.success(),
        Err(e) => {
            crate::log_debug(&format!("gate poll: cannot run until-command ({e}): {cmd}"));
            false
        }
    }
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
/// Fleet's own stop paths are unaffected: `interrupt_pid_impl` /
/// `kill_pid_tree` signal explicit pids collected by ppid walk, not the group.
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
