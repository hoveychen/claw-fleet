/// Switch the current process's console to UTF-8 (CP 65001) on Windows.
///
/// Windows zh-CN systems default to CP 936 (GBK). Fleet binaries print UTF-8
/// bytes from CC transcripts and TUI rendering; under CP 936 those bytes are
/// decoded as GBK and every subsequent line — including ASCII after a broken
/// multibyte sequence — comes out garbled. Calling this once at process entry
/// (before any output and before clap parsing) keeps stdout/stderr legible.
///
/// On non-Windows targets this is a no-op. On Windows targets without an
/// attached console (e.g. release builds of GUI apps with
/// `#![windows_subsystem = "windows"]`), the Win32 calls fail and the returned
/// `Err` is discarded — safe to call unconditionally from any binary's `main`.
#[cfg(windows)]
pub fn init_utf8() {
    // SAFETY: Win32 Console API only reads/writes this process's console
    // handles. No memory-safety invariants involved.
    use windows::Win32::System::Console::{SetConsoleCP, SetConsoleOutputCP};
    unsafe {
        let _ = SetConsoleOutputCP(65001);
        let _ = SetConsoleCP(65001);
    }
}

#[cfg(not(windows))]
pub fn init_utf8() {}
