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

/// Is this a permission-denied error — i.e. the shape TCC produces when the
/// user declines access to a protected folder? Matches both the portable
/// `PermissionDenied` kind and the raw errnos macOS returns (EPERM/EACCES).
fn is_permission_error(err: &std::io::Error) -> bool {
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        return true;
    }
    matches!(err.raw_os_error(), Some(1) | Some(13)) // EPERM | EACCES
}

/// Record a filesystem denial that looks like a TCC refusal, so a deliberate
/// "Don't Allow" leaves a breadcrumb pointing at the exact code path.
///
/// Only fires when BOTH hold: the error is permission-denied AND the path is
/// inside a TCC-protected folder. Everything else (missing file, ordinary
/// EACCES on a non-protected path) is left alone — the log stays signal, not
/// noise. Appends a timestamped record + a full backtrace to
/// `~/.fleet/tcc-denials.log`. Best-effort: never panics, never propagates.
pub(crate) fn note_fs_denial(path: &Path, err: &std::io::Error) {
    let Some(log) = crate::session::real_home_dir().map(|h| h.join(".fleet").join("tcc-denials.log"))
    else {
        return;
    };
    note_fs_denial_to(path, err, &log, &|p| is_tcc_protected(p));
}

/// Injectable core of [`note_fs_denial`] — log destination and the protected
/// predicate are parameters so the gating logic is testable against a temp
/// tree without a real TCC denial (which can't be provoked in a unit test).
fn note_fs_denial_to(
    path: &Path,
    err: &std::io::Error,
    log_path: &Path,
    is_protected: &dyn Fn(&Path) -> bool,
) {
    if !is_permission_error(err) || !is_protected(path) {
        return;
    }
    use std::io::Write;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // force_capture ignores RUST_BACKTRACE so the trace is always present; in a
    // symbol-stripped release build it may be addresses only (symbolicate with
    // `atos -o <binary>`), which is still enough to locate the offending path.
    let bt = std::backtrace::Backtrace::force_capture();
    let record = format!(
        "\n=== TCC denial @ {ts} (errno {:?}) ===\npath: {}\nerror: {err}\nbacktrace:\n{bt}\n",
        err.raw_os_error(),
        path.display(),
    );
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = f.write_all(record.as_bytes());
    }
    // Also surface it on stderr so it shows up in the app's captured logs.
    eprintln!("TCC denial on protected path {} ({err})", path.display());
}

/// `std::fs::read_dir` that logs a TCC-shaped denial (see [`note_fs_denial`])
/// before returning the error unchanged. Use this at chokepoints that read
/// user-arbitrary paths (workspace/worktree roots, the directory picker), where
/// a path could legitimately resolve into a protected folder.
pub(crate) fn guarded_read_dir(path: &Path) -> std::io::Result<std::fs::ReadDir> {
    std::fs::read_dir(path).inspect_err(|e| note_fs_denial(path, e))
}

/// `std::fs::metadata` counterpart of [`guarded_read_dir`].
pub(crate) fn guarded_metadata(path: &Path) -> std::io::Result<std::fs::Metadata> {
    std::fs::metadata(path).inspect_err(|e| note_fs_denial(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn followable_dir_never_follows_symlink_into_protected() {
        use std::os::unix::fs::symlink;
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

    fn err(errno: i32) -> std::io::Error {
        std::io::Error::from_raw_os_error(errno)
    }

    #[test]
    fn logs_only_permission_denials_on_protected_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("logs").join("tcc-denials.log");
        let prot = std::path::PathBuf::from("/Users/x/Documents");
        let protected = {
            let prot = prot.clone();
            move |p: &Path| p == prot || p.starts_with(&prot)
        };

        // permission error + protected path → logged, with the path recorded.
        note_fs_denial_to(&prot, &err(13), &log, &protected); // EACCES
        let body = std::fs::read_to_string(&log).unwrap();
        assert!(body.contains("/Users/x/Documents"));
        assert!(body.contains("TCC denial"));

        // permission error but NOT protected → nothing appended.
        let before = std::fs::read_to_string(&log).unwrap();
        note_fs_denial_to(Path::new("/tmp/whatever"), &err(1), &log, &protected); // EPERM
        assert_eq!(std::fs::read_to_string(&log).unwrap(), before);

        // protected path but NOT a permission error (ENOENT) → nothing appended.
        note_fs_denial_to(&prot.join("missing"), &err(2), &log, &protected);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), before);
    }

    #[test]
    fn permission_error_matches_kind_and_raw_errnos() {
        assert!(is_permission_error(&err(1))); // EPERM
        assert!(is_permission_error(&err(13))); // EACCES
        assert!(is_permission_error(&std::io::Error::from(std::io::ErrorKind::PermissionDenied)));
        assert!(!is_permission_error(&err(2))); // ENOENT
    }
}
