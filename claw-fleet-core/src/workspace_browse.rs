//! Directory picker for clients that have no native file dialog.
//!
//! The desktop picks a session's workspace with Tauri's native directory
//! dialog. The phone has no such thing, so its composer used to offer a bare
//! text box — "type the absolute path of a directory on a machine you cannot
//! see". This module is the data source that replaces it: list the directories
//! under a path, one level at a time, so the phone can walk down to a workspace.
//!
//! Security model — deliberately *not* `file_explorer`'s.
//!
//! `file_explorer` gates every call on `known_workspaces`: you may only browse
//! inside a workspace that already has a session. That is exactly wrong here —
//! the whole point is to reach a directory that has *never* had a session. So
//! this module defines its own boundary instead:
//!   • The browsable roots are the home directory, plus any known workspace
//!     that lives outside it (a repo on another volume stays reachable).
//!   • Every path — the one the client asks for and the parent link we hand
//!     back — is canonicalized and required to sit under one of those roots, so
//!     `..` chains and symlinks pointing out of home are both refused.
//!   • Directories only, and never their contents. This surface cannot read a
//!     file; it cannot even tell you a file exists.
//! The relay is reachable over the public internet (behind the pairing secret),
//! so "list the directory names under $HOME" is the entire blast radius, and
//! that is intentional — it is what a directory picker inherently is.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Directories past this many in a single listing are dropped and the response
/// is flagged `truncated`. A picker is for walking a tree by hand; a directory
/// with thousands of children is not something anyone scrolls, and the whole
/// listing has to fit in one relay frame.
const MAX_ENTRIES: usize = 500;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrowseEntry {
    pub name: String,
    pub path: String,
    /// Has a `.git` — i.e. this is very likely the thing the user wants to pick,
    /// so the phone can badge it rather than making them recognise repo names.
    pub is_git_repo: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BrowseDirResponse {
    /// The canonical directory this listing is of.
    pub path: String,
    /// Where "up" goes, or `None` at a root — the client renders the ".." row
    /// from this, so the boundary is enforced here rather than trusted there.
    pub parent: Option<String>,
    pub entries: Vec<BrowseEntry>,
    /// `entries` was cut at [`MAX_ENTRIES`].
    pub truncated: bool,
}

/// List the directories one level under `path` (`None`/empty = the home dir).
pub fn browse_dir(path: Option<&str>, known_workspaces: &[String]) -> Result<BrowseDirResponse, String> {
    let home = crate::session::real_home_dir().ok_or("home directory unknown")?;
    browse_dir_in(
        path,
        &home,
        &browse_roots(&home, known_workspaces),
        &|p| crate::tcc::is_tcc_protected(p),
    )
}

/// The roots a client may browse: home, plus any known workspace that lives
/// outside it (an external-volume repo would otherwise be unreachable).
fn browse_roots(home: &Path, known_workspaces: &[String]) -> Vec<PathBuf> {
    let home = fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let mut roots = vec![home.clone()];
    for ws in known_workspaces {
        let Ok(c) = fs::canonicalize(ws) else { continue };
        if !c.starts_with(&home) && !roots.contains(&c) {
            roots.push(c);
        }
    }
    roots
}

/// Root-injectable core of [`browse_dir`], so the boundary rules are testable
/// against a temp tree instead of the real home directory.
fn browse_dir_in(
    path: Option<&str>,
    home: &Path,
    roots: &[PathBuf],
    is_protected: &dyn Fn(&Path) -> bool,
) -> Result<BrowseDirResponse, String> {
    let requested = match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => expand(p, home),
        None => home.to_path_buf(),
    };
    // Canonicalize before the boundary check, never after: `..` components and
    // symlinks must be resolved while we still get to reject the result.
    let dir = fs::canonicalize(&requested)
        .map_err(|e| format!("{}: {e}", requested.display()))?;
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let root = enclosing_root(&dir, roots)
        .ok_or_else(|| format!("{} is outside the browsable roots", dir.display()))?;

    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let child = entry.path();
        // Resolve dir-ness from the readdir `d_type`, NOT `child.is_dir()`: the
        // latter `stat`s every entry, which fires a macOS TCC dialog the moment
        // we list a home dir containing ~/Documents, ~/Desktop, ~/Downloads. The
        // helper follows symlinks only when their target isn't TCC-protected, so
        // it can't stat into a protected folder either. Shared verbatim with
        // `session::paths::read_level_dirs`.
        if !crate::tcc::readdir_is_followable_dir(entry.file_type(), &child, is_protected) {
            continue;
        }
        let Some(name) = child.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };
        // Dotfiles are noise in a workspace picker (`.git`, `.cache`, `.npm`…).
        // The one hidden directory a user might want — the chat workspace — has
        // its own entry in the composer and never needs to be browsed to.
        if name.starts_with('.') {
            continue;
        }
        if entries.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }
        entries.push(BrowseEntry {
            // Never probe `.git` inside a TCC-protected dir: `child.join(".git")`
            // starts_with the protected path, so `.exists()` would `stat` into it
            // and fire a dialog. A protected dir is still offered as a pickable
            // entry (a repo may live there) — just never badged.
            is_git_repo: !is_protected(&child) && child.join(".git").exists(),
            name,
            path: child.to_string_lossy().into_owned(),
        });
    }
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    // At a root, "up" would leave the boundary — report no parent so the client
    // doesn't render a dead ".." row.
    let parent = if dir == *root {
        None
    } else {
        dir.parent()
            .filter(|p| enclosing_root(p, roots).is_some())
            .map(|p| p.to_string_lossy().into_owned())
    };

    Ok(BrowseDirResponse {
        path: dir.to_string_lossy().into_owned(),
        parent,
        entries,
        truncated,
    })
}

/// `~`-expanding, home-anchoring path resolution — same shape the spawn path
/// applies (`session_launch::normalize_workspace_path`), so a path typed into
/// the composer and a path clicked in the picker resolve identically.
fn expand(input: &str, home: &Path) -> PathBuf {
    if input == "~" {
        home.to_path_buf()
    } else if let Some(rest) = input.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(input).is_absolute() {
        PathBuf::from(input)
    } else {
        home.join(input)
    }
}

fn enclosing_root<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {
    roots.iter().find(|r| path.starts_with(r))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default predicate for tests that don't exercise TCC: nothing is protected,
    /// so the guard is a no-op and behaviour matches the pre-guard listing.
    fn none_protected(_: &Path) -> bool {
        false
    }

    /// home/
    ///   proj/          (a git repo)
    ///     src/
    ///   notes/
    ///   .hidden/
    ///   file.txt
    /// outside/         (sibling of home — not browsable unless a known ws)
    ///   external-repo/
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join("proj/src")).unwrap();
        fs::create_dir_all(home.join("proj/.git")).unwrap();
        fs::create_dir_all(home.join("notes")).unwrap();
        fs::create_dir_all(home.join(".hidden")).unwrap();
        fs::write(home.join("file.txt"), "x").unwrap();
        let outside = tmp.path().join("outside");
        fs::create_dir_all(outside.join("external-repo")).unwrap();
        // canonicalize: on macOS the temp dir lives under a /var → /private/var
        // symlink, and the responses carry canonical paths.
        let home = fs::canonicalize(home).unwrap();
        let outside = fs::canonicalize(outside).unwrap();
        (tmp, home, outside)
    }

    fn names(r: &BrowseDirResponse) -> Vec<&str> {
        r.entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn lists_only_visible_directories_and_flags_repos() {
        let (_tmp, home, _) = fixture();
        let roots = vec![home.clone()];
        let r = browse_dir_in(None, &home, &roots, &none_protected).unwrap();
        // Files and dotfiles are gone; the repo is badged; order is stable.
        assert_eq!(names(&r), vec!["notes", "proj"]);
        assert!(r.entries.iter().find(|e| e.name == "proj").unwrap().is_git_repo);
        assert!(!r.entries.iter().find(|e| e.name == "notes").unwrap().is_git_repo);
        assert!(!r.truncated);
    }

    #[test]
    fn home_is_a_root_so_it_has_no_parent_but_a_child_does() {
        let (_tmp, home, _) = fixture();
        let roots = vec![home.clone()];
        assert_eq!(browse_dir_in(None, &home, &roots, &none_protected).unwrap().parent, None);
        let proj = browse_dir_in(Some("proj"), &home, &roots, &none_protected).unwrap();
        assert_eq!(proj.path, home.join("proj").to_string_lossy());
        assert_eq!(proj.parent.as_deref(), Some(&*home.to_string_lossy()));
        assert_eq!(names(&proj), vec!["src"]);
    }

    #[test]
    fn accepts_tilde_and_bare_relative_the_same_way_the_spawn_path_does() {
        let (_tmp, home, _) = fixture();
        let roots = vec![home.clone()];
        let want = home.join("proj").to_string_lossy().into_owned();
        for input in ["~/proj", "proj", "  ~/proj  "] {
            assert_eq!(browse_dir_in(Some(input), &home, &roots, &none_protected).unwrap().path, want, "{input}");
        }
    }

    /// The boundary is the whole security model: the relay is public, so a
    /// crafted `..` chain (or a path that simply names somewhere else) must not
    /// enumerate directories outside the roots.
    #[test]
    fn refuses_to_escape_the_roots() {
        let (_tmp, home, outside) = fixture();
        let roots = vec![home.clone()];
        for escape in [
            outside.to_string_lossy().into_owned(),
            "../outside".to_string(),
            "proj/../../outside".to_string(),
            "/etc".to_string(),
        ] {
            let r = browse_dir_in(Some(&escape), &home, &roots, &none_protected);
            assert!(r.is_err(), "{escape:?} must be refused, got {r:?}");
        }
    }

    /// A workspace on another volume has a session but is not under home; it
    /// stays reachable as its own root — and browsing it stops there rather
    /// than walking up into its parent.
    #[test]
    fn known_workspace_outside_home_becomes_its_own_root() {
        let (_tmp, home, outside) = fixture();
        let ext = outside.join("external-repo");
        let roots = browse_roots(&home, &[ext.to_string_lossy().into_owned()]);
        assert_eq!(roots.len(), 2);

        let r = browse_dir_in(Some(&ext.to_string_lossy()), &home, &roots, &none_protected).unwrap();
        assert_eq!(r.path, ext.to_string_lossy());
        assert_eq!(r.parent, None, "a root must not expose its parent");

        // ...but its parent `outside/` is still not browsable.
        assert!(browse_dir_in(Some(&outside.to_string_lossy()), &home, &roots, &none_protected).is_err());
    }

    /// A known workspace already under home must not become a second root —
    /// that would let the client walk up from it to the same places home
    /// already allows, and worse, dedup failures would multiply the list.
    #[test]
    fn known_workspace_inside_home_adds_no_root() {
        let (_tmp, home, _) = fixture();
        let inside = home.join("proj").to_string_lossy().into_owned();
        assert_eq!(browse_roots(&home, &[inside]), vec![home]);
    }

    #[test]
    fn missing_directory_errors_rather_than_silently_listing_home() {
        let (_tmp, home, _) = fixture();
        let roots = vec![home.clone()];
        assert!(browse_dir_in(Some("~/nope"), &home, &roots, &none_protected).is_err());
        // A file is not a directory: the picker must not "descend" into it.
        assert!(browse_dir_in(Some("file.txt"), &home, &roots, &none_protected).is_err());
    }

    /// TCC guard: a protected top-level dir (e.g. `~/Documents`) is still listed
    /// so a repo living there stays reachable, but it must NOT be probed for
    /// `.git` — that probe `stat`s *inside* the protected dir and fires a macOS
    /// TCC dialog. So a protected dir is always reported `is_git_repo: false`,
    /// even when it really does contain a `.git`, because we never look.
    #[test]
    fn protected_dir_is_listed_but_never_git_probed() {
        let (_tmp, home, _) = fixture();
        // A protected-by-name dir that genuinely contains a repo marker.
        fs::create_dir_all(home.join("Documents/.git")).unwrap();
        let roots = vec![home.clone()];
        let is_protected = |p: &Path| p.file_name().map_or(false, |n| n == "Documents");

        let r = browse_dir_in(None, &home, &roots, &is_protected).unwrap();
        // Still offered as a pickable directory...
        let doc = r.entries.iter().find(|e| e.name == "Documents").expect("Documents must be listed");
        // ...but never git-probed, so no stat lands inside ~/Documents.
        assert!(!doc.is_git_repo, "protected dir must not be git-probed (would fire TCC)");
        // The ordinary repo is still badged, proving the guard is scoped.
        assert!(r.entries.iter().find(|e| e.name == "proj").unwrap().is_git_repo);
    }

    /// TCC guard, symlink case: a home-root symlink pointing INTO a protected
    /// dir must not be followed. Following it (`child.is_dir()`) stats the
    /// protected target and fires a macOS TCC dialog — so the entry is dropped.
    /// A symlink to an ordinary dir is still followed (a symlinked repo is a
    /// legitimate pick).
    #[cfg(unix)]
    #[test]
    fn symlink_into_protected_dir_is_not_followed() {
        use std::os::unix::fs::symlink;
        let (_tmp, home, _) = fixture();
        let protected = home.join("Protected");
        fs::create_dir_all(&protected).unwrap();
        symlink(&protected, home.join("plink")).unwrap();
        symlink(home.join("notes"), home.join("nlink")).unwrap();
        let roots = vec![home.clone()];
        let prot = protected.clone();
        let is_protected = move |p: &Path| p == prot;

        let r = browse_dir_in(None, &home, &roots, &is_protected).unwrap();
        let names: Vec<&str> = r.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.contains(&"plink"),
            "symlink into a protected dir must not be followed/listed, got {names:?}"
        );
        assert!(
            names.contains(&"nlink"),
            "symlink to an ordinary dir must still be followed, got {names:?}"
        );
    }

    #[test]
    fn caps_huge_listings() {
        let (_tmp, home, _) = fixture();
        let many = home.join("many");
        for i in 0..(MAX_ENTRIES + 10) {
            fs::create_dir_all(many.join(format!("d{i:04}"))).unwrap();
        }
        let r = browse_dir_in(Some("many"), &home, &vec![home.clone()], &none_protected).unwrap();
        assert!(r.truncated);
        assert_eq!(r.entries.len(), MAX_ENTRIES);
    }
}
