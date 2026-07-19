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

/// Decide whether a `read_dir` entry counts as a followable directory WITHOUT
/// stat'ing into a TCC-protected target.
///
/// - A real directory: yes.
/// - A symlink: followed (a symlinked repo is a legitimate pick) only when its
///   target is not TCC-protected. The target is read with `fs::read_link` — a
///   single hop, resolved against the link's parent for relative targets —
///   NOT `canonicalize`, because canonicalize itself `stat`s all the way
///   through and would fire the very TCC dialog we're avoiding. When the link
///   is safe, `entry_path.is_dir()` does the final follow.
/// - Anything else (file, error): no.
///
/// `is_protected` is injected so the boundary is testable against a temp tree;
/// production callers pass [`is_tcc_protected`].
pub(crate) fn readdir_is_followable_dir(
    file_type: std::io::Result<std::fs::FileType>,
    entry_path: &Path,
    is_protected: &dyn Fn(&Path) -> bool,
) -> bool {
    match file_type {
        Ok(ft) if ft.is_dir() => true,
        Ok(ft) if ft.is_symlink() => {
            let Ok(raw) = std::fs::read_link(entry_path) else {
                return false;
            };
            let target = if raw.is_absolute() {
                raw
            } else {
                entry_path.parent().map(|p| p.join(&raw)).unwrap_or(raw)
            };
            if is_protected(&target) {
                return false;
            }
            entry_path.is_dir()
        }
        _ => false,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn followable_dir_never_follows_symlink_into_protected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let protected = root.join("Protected");
        std::fs::create_dir_all(&protected).unwrap();
        let plain = root.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(root.join("file.txt"), "x").unwrap();
        symlink(&protected, root.join("plink")).unwrap();
        symlink(&plain, root.join("nlink")).unwrap();

        let prot = protected.clone();
        let is_protected = move |p: &std::path::Path| p == prot;
        let ft = |p: &std::path::Path| std::fs::symlink_metadata(p).map(|m| m.file_type());

        // real dir: followable
        assert!(readdir_is_followable_dir(ft(&protected), &protected, &is_protected));
        // symlink → protected: NOT followed (would stat into a TCC dir)
        let plink = root.join("plink");
        assert!(!readdir_is_followable_dir(ft(&plink), &plink, &is_protected));
        // symlink → ordinary dir: still followed
        let nlink = root.join("nlink");
        assert!(readdir_is_followable_dir(ft(&nlink), &nlink, &is_protected));
        // plain file: not a dir
        let file = root.join("file.txt");
        assert!(!readdir_is_followable_dir(ft(&file), &file, &is_protected));
    }
}
