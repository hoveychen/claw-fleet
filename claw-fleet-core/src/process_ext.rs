//! Suppress the cmd.exe / conhost.exe window that pops up whenever a Tauri GUI
//! process spawns a Windows console subsystem child. Without
//! `CREATE_NO_WINDOW`, every `where`, `taskkill`, `ssh`, `claude --resume`,
//! etc. flashes a console window and serializes through the USER subsystem,
//! which can make the GUI appear to hang during the startup source-detection
//! scan.
//!
//! Usage: `Command::new("where").no_window().arg("claude").output()`.
//!
//! On non-Windows platforms `no_window()` is a no-op so callers can use the
//! same chain unconditionally.

pub trait NoWindowExt {
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindowExt for std::process::Command {
    #[cfg(windows)]
    fn no_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        self.creation_flags(CREATE_NO_WINDOW)
    }

    #[cfg(not(windows))]
    fn no_window(&mut self) -> &mut Self {
        self
    }
}
