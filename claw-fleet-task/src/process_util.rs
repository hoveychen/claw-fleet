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
    fn command_constructor_runs() {
        #[cfg(unix)]
        let status = command("true").status().expect("spawn");
        #[cfg(windows)]
        let status = command("cmd").args(["/C", "exit"]).status().expect("spawn");
        assert!(status.success());
    }
}
