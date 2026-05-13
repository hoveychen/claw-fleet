//! macOS TCC (Transparency, Consent, and Control) path guards.
//!
//! On macOS, accessing certain directories (~/Music, ~/Pictures, ~/Documents,
//! ~/Desktop, ~/Downloads, ~/Movies) triggers system permission dialogs.
//! This module provides utilities to avoid stat'ing these paths.

use std::path::Path;

/// TCC-protected directories under the user's home.
#[cfg(target_os = "macos")]
const TCC_PROTECTED_DIRS: &[&str] = &[
    "Desktop", "Documents", "Downloads", "Music", "Pictures", "Movies",
];

/// Check if a path is inside a macOS TCC-protected directory.
#[cfg(target_os = "macos")]
fn check_tcc_path(path: &Path) -> bool {
    let Some(home) = crate::session::real_home_dir() else { return false };
    for dir_name in TCC_PROTECTED_DIRS {
        let protected = home.join(dir_name);
        if path == protected || path.starts_with(&protected) {
            return true;
        }
    }
    false
}

#[cfg(not(target_os = "macos"))]
fn check_tcc_path(_path: &Path) -> bool {
    false
}

/// Like `Path::exists()`, but returns `false` for TCC-protected paths instead of
/// calling `stat()`.  This prevents the greedy decode algorithm from locking onto
/// a TCC-protected directory (e.g. `~/Music`, `~/Downloads`).
pub fn safe_exists(path: &Path) -> bool {
    if check_tcc_path(path) {
        return false;
    }
    path.exists()
}

/// Check whether the given path is inside a TCC-protected directory.
/// Use this to skip filesystem operations on decoded workspace paths
/// that might resolve into protected folders.
pub fn is_tcc_protected(path: &Path) -> bool {
    check_tcc_path(path)
}
