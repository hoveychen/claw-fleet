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
    /// Every browsable root, canonical. A root has no parent, so without this
    /// the client standing in one has no way back to the others — and on a
    /// cloud container the listing *starts* in a root that is not home.
    #[serde(default)]
    pub roots: Vec<String>,
}

/// List the directories one level under `path`. `None`/empty means the default
/// start: the designated workspace root when the host declares one, else home
/// (see [`default_start`]).
pub fn browse_dir(path: Option<&str>, known_workspaces: &[String]) -> Result<BrowseDirResponse, String> {
    let home = crate::session::real_home_dir().ok_or("home directory unknown")?;
    let roots = browse_roots(&home, known_workspaces);
    browse_dir_in(
        path,
        &home,
        &default_start(&home),
        &roots,
        &|p| crate::tcc::is_tcc_protected(p),
    )
}

/// The directory a client with no path of its own should land in.
///
/// On a Fleet Cloud container `$HOME` is the image's **ephemeral writable
/// layer**: the one persistent volume is mounted elsewhere (muvee mounts
/// `/workspace`, and the entrypoint symlinks `~/.fleet` and the transcript dirs
/// into it). A workspace created under `$HOME` therefore disappears on the next
/// deploy, along with whatever the agent built in it. `FLEET_PUBLIC_WORKSPACE`
/// is the host's own declaration of where customer work belongs — the same path
/// the `/v1` API is confined to — so when it is set, that is where the picker
/// opens. Unset (every desktop, every laptop) → home, exactly as before.
fn default_start(home: &Path) -> PathBuf {
    workspace_root().unwrap_or_else(|| home.to_path_buf())
}

/// The host's declared workspace directory, canonicalized, if it is set and
/// really is a directory. A value naming something that does not exist is
/// ignored rather than fatal: the picker still has to work.
fn workspace_root() -> Option<PathBuf> {
    let raw = std::env::var("FLEET_PUBLIC_WORKSPACE").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let canonical = fs::canonicalize(trimmed).ok()?;
    canonical.is_dir().then_some(canonical)
}

/// Create one new child directory under `parent` (`None`/empty = the home dir)
/// and return the listing *of the new directory*, so the caller lands inside it
/// and can pick it immediately.
///
/// A picker that can only walk an existing tree is unusable on a host where the
/// tree is empty — a fresh Fleet Cloud container has nothing under `/home/fleet`,
/// so "选择工作目录" offered no directory to select and no way to make one.
///
/// The boundary is [`browse_dir`]'s, unchanged: the parent must canonicalize to
/// somewhere under the browsable roots. On top of that, `name` must be a single
/// plain child name — this creates exactly one directory, one level down, never
/// a path.
pub fn create_dir(
    parent: Option<&str>,
    name: &str,
    known_workspaces: &[String],
) -> Result<BrowseDirResponse, String> {
    let home = crate::session::real_home_dir().ok_or("home directory unknown")?;
    let roots = browse_roots(&home, known_workspaces);
    create_dir_in(
        parent,
        name,
        &home,
        &default_start(&home),
        &roots,
        &|p| crate::tcc::is_tcc_protected(p),
    )
}

/// Root-injectable core of [`create_dir`], mirroring [`browse_dir_in`].
fn create_dir_in(
    parent: Option<&str>,
    name: &str,
    home: &Path,
    start: &Path,
    roots: &[PathBuf],
    is_protected: &dyn Fn(&Path) -> bool,
) -> Result<BrowseDirResponse, String> {
    let name = validate_child_name(name)?;
    let requested = match parent.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => expand(p, home),
        None => start.to_path_buf(),
    };
    // Same order as browse_dir_in: canonicalize first, judge the result. A
    // symlinked or `..`-laden parent must be resolved while we can still refuse.
    let dir = fs::canonicalize(&requested).map_err(|e| format!("{}: {e}", requested.display()))?;
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    if enclosing_root(&dir, roots).is_none() {
        return Err(format!("{} is outside the browsable roots", dir.display()));
    }

    let child = dir.join(name);
    // `create_dir`, never `create_dir_all`: one level only, and an existing name
    // must be an error rather than a silent no-op that looks like a creation.
    fs::create_dir(&child).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("{name} already exists"),
        _ => format!("{}: {e}", child.display()),
    })?;

    // Answer with the new directory's own listing (empty, with a parent link),
    // so the client is standing in it without a second round trip.
    browse_dir_in(Some(&child.to_string_lossy()), home, start, roots, is_protected)
}

/// A new directory's name: one plain path component, nothing that could reach
/// out of the parent or hide from the listing.
fn validate_child_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("directory name is empty".into());
    }
    if name.len() > 255 {
        return Err("directory name is too long".into());
    }
    if name == "." || name == ".." {
        return Err(format!("{name:?} is not a directory name"));
    }
    // Separators are the escape: `a/b` would create two levels, `../x` would
    // create one outside the directory the user is looking at. Both are refused
    // by name rather than being canonicalized away, so the error names the cause.
    if name.contains('/') || name.contains('\\') {
        return Err("directory name must not contain a path separator".into());
    }
    if name.contains('\0') {
        return Err("directory name must not contain a NUL".into());
    }
    // The listing hides dotfiles (`.git`, `.cache`, …), so a `.foo` created here
    // would vanish the moment it is created — refuse rather than confuse.
    if name.starts_with('.') {
        return Err("directory name must not start with a dot".into());
    }
    Ok(name)
}

/// The roots a client may browse: home, the host's declared workspace directory
/// (see [`default_start`] — on a cloud container that is the persistent volume,
/// and home is not), plus any known workspace that lives outside home (an
/// external-volume repo would otherwise be unreachable).
fn browse_roots(home: &Path, known_workspaces: &[String]) -> Vec<PathBuf> {
    let home = fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let mut roots = vec![home.clone()];
    // Its parent stays unbrowsable: on the cloud volume that parent also holds
    // `.fleet-state` and the cred-store dir, and a root never exposes its own
    // parent. Under home it needs no root of its own — home already reaches it.
    if let Some(ws) = workspace_root() {
        if !ws.starts_with(&home) {
            roots.push(ws);
        }
    }
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
    start: &Path,
    roots: &[PathBuf],
    is_protected: &dyn Fn(&Path) -> bool,
) -> Result<BrowseDirResponse, String> {
    let requested = match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => expand(p, home),
        None => start.to_path_buf(),
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
    // `guarded_read_dir` logs a backtrace if this read is TCC-denied — the user
    // may navigate the picker into ~/Documents et al., and a deliberate "Don't
    // Allow" should leave a breadcrumb rather than a silent empty listing.
    for entry in crate::tcc::guarded_read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
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
        roots: roots.iter().map(|r| r.to_string_lossy().into_owned()).collect(),
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
        let r = browse_dir_in(None, &home, &home, &roots, &none_protected).unwrap();
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
        assert_eq!(browse_dir_in(None, &home, &home, &roots, &none_protected).unwrap().parent, None);
        let proj = browse_dir_in(Some("proj"), &home, &home, &roots, &none_protected).unwrap();
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
            assert_eq!(browse_dir_in(Some(input), &home, &home, &roots, &none_protected).unwrap().path, want, "{input}");
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
            let r = browse_dir_in(Some(&escape), &home, &home, &roots, &none_protected);
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
        // `browse_roots` reads FLEET_PUBLIC_WORKSPACE, so pin it off — otherwise
        // a concurrently-running env test decides how many roots there are.
        let roots = with_workspace_env(None, || {
            browse_roots(&home, &[ext.to_string_lossy().into_owned()])
        });
        assert_eq!(roots.len(), 2);

        let r = browse_dir_in(Some(&ext.to_string_lossy()), &home, &home, &roots, &none_protected).unwrap();
        assert_eq!(r.path, ext.to_string_lossy());
        assert_eq!(r.parent, None, "a root must not expose its parent");

        // ...but its parent `outside/` is still not browsable.
        assert!(browse_dir_in(Some(&outside.to_string_lossy()), &home, &home, &roots, &none_protected).is_err());
    }

    /// A known workspace already under home must not become a second root —
    /// that would let the client walk up from it to the same places home
    /// already allows, and worse, dedup failures would multiply the list.
    #[test]
    fn known_workspace_inside_home_adds_no_root() {
        let (_tmp, home, _) = fixture();
        let inside = home.join("proj").to_string_lossy().into_owned();
        with_workspace_env(None, || assert_eq!(browse_roots(&home, &[inside]), vec![home.clone()]));
    }

    #[test]
    fn missing_directory_errors_rather_than_silently_listing_home() {
        let (_tmp, home, _) = fixture();
        let roots = vec![home.clone()];
        assert!(browse_dir_in(Some("~/nope"), &home, &home, &roots, &none_protected).is_err());
        // A file is not a directory: the picker must not "descend" into it.
        assert!(browse_dir_in(Some("file.txt"), &home, &home, &roots, &none_protected).is_err());
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

        let r = browse_dir_in(None, &home, &home, &roots, &is_protected).unwrap();
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

        let r = browse_dir_in(None, &home, &home, &roots, &is_protected).unwrap();
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

    /// The whole point: a directory that has no children can still get one, and
    /// the caller comes back standing *inside* it (empty listing, parent link
    /// pointing at where it was created) so it can be picked in one more tap.
    #[test]
    fn creates_a_child_and_answers_with_its_listing() {
        let (_tmp, home, _) = fixture();
        let roots = vec![home.clone()];
        let r = create_dir_in(Some("notes"), "sub", &home, &home, &roots, &none_protected).unwrap();
        assert_eq!(r.path, home.join("notes/sub").to_string_lossy());
        assert_eq!(r.parent.as_deref(), Some(&*home.join("notes").to_string_lossy()));
        assert!(r.entries.is_empty());
        assert!(home.join("notes/sub").is_dir());
        // ...and it shows up in the parent's listing afterwards.
        let parent = browse_dir_in(Some("notes"), &home, &home, &roots, &none_protected).unwrap();
        assert_eq!(names(&parent), vec!["sub"]);
    }

    /// `None` parent means home, same as browsing.
    #[test]
    fn creates_under_home_by_default() {
        let (_tmp, home, _) = fixture();
        let roots = vec![home.clone()];
        let r = create_dir_in(None, "  fresh  ", &home, &home, &roots, &none_protected).unwrap();
        assert_eq!(r.path, home.join("fresh").to_string_lossy());
        assert!(home.join("fresh").is_dir());
    }

    /// A name is a name, never a path: separators and `..` must not let the
    /// client create anything outside the directory it is looking at, and a
    /// leading dot must not create something the listing then hides.
    #[test]
    fn refuses_names_that_are_not_a_single_visible_component() {
        let (_tmp, home, _) = fixture();
        let roots = vec![home.clone()];
        for bad in ["", "   ", ".", "..", "a/b", "../escape", "a\\b", ".hidden"] {
            let r = create_dir_in(None, bad, &home, &home, &roots, &none_protected);
            assert!(r.is_err(), "{bad:?} must be refused, got {r:?}");
        }
        assert!(!home.parent().unwrap().join("escape").exists());
    }

    /// The parent carries the same boundary as browsing — otherwise `create_dir`
    /// would be a hole straight through `browse_dir`'s security model.
    #[test]
    fn refuses_to_create_outside_the_roots() {
        let (_tmp, home, outside) = fixture();
        let roots = vec![home.clone()];
        for escape in [
            outside.to_string_lossy().into_owned(),
            "../outside".to_string(),
            "proj/../../outside".to_string(),
        ] {
            let r = create_dir_in(Some(&escape), "x", &home, &home, &roots, &none_protected);
            assert!(r.is_err(), "{escape:?} must be refused, got {r:?}");
        }
        assert!(!outside.join("x").exists());
    }

    /// An existing name is an error, not a silent success: "created" must mean
    /// created, or the user has no way to tell a fresh directory from someone
    /// else's.
    #[test]
    fn refuses_an_existing_name() {
        let (_tmp, home, _) = fixture();
        let roots = vec![home.clone()];
        assert!(create_dir_in(None, "notes", &home, &home, &roots, &none_protected).is_err());
        // A file with that name collides too — and is not clobbered.
        assert!(create_dir_in(None, "file.txt", &home, &home, &roots, &none_protected).is_err());
        assert_eq!(fs::read_to_string(home.join("file.txt")).unwrap(), "x");
    }

    #[test]
    fn missing_parent_errors_rather_than_creating_it() {
        let (_tmp, home, _) = fixture();
        let roots = vec![home.clone()];
        assert!(create_dir_in(Some("~/nope"), "x", &home, &home, &roots, &none_protected).is_err());
        assert!(!home.join("nope").exists());
    }

    /// Serializes the tests that mutate `FLEET_PUBLIC_WORKSPACE`; cargo runs
    /// test fns concurrently inside one process, and the env is process-global.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Runs `f` with `FLEET_PUBLIC_WORKSPACE` set to `value` (or unset for
    /// `None`), restoring whatever was there before.
    fn with_workspace_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("FLEET_PUBLIC_WORKSPACE").ok();
        match value {
            Some(v) => std::env::set_var("FLEET_PUBLIC_WORKSPACE", v),
            None => std::env::remove_var("FLEET_PUBLIC_WORKSPACE"),
        }
        let out = f();
        match prev {
            Some(v) => std::env::set_var("FLEET_PUBLIC_WORKSPACE", v),
            None => std::env::remove_var("FLEET_PUBLIC_WORKSPACE"),
        }
        out
    }

    /// The cloud shape: `$HOME` is the container's ephemeral layer and the
    /// persistent volume is mounted elsewhere, so a workspace created under home
    /// dies on the next deploy. When the host declares its workspace directory,
    /// that is where the picker opens and where a nameless `create_dir` lands.
    #[test]
    fn declared_workspace_is_the_default_start_and_a_root() {
        let (_tmp, home, outside) = fixture();
        let volume = outside.join("repo");
        fs::create_dir_all(&volume).unwrap();
        with_workspace_env(Some(&volume.to_string_lossy()), || {
            let roots = browse_roots(&home, &[]);
            assert!(roots.contains(&volume), "declared workspace must be a root: {roots:?}");
            assert!(roots.contains(&home), "home must stay browsable");

            let start = default_start(&home);
            assert_eq!(start, volume);

            let r = browse_dir_in(None, &home, &start, &roots, &none_protected).unwrap();
            assert_eq!(r.path, volume.to_string_lossy());
            assert_eq!(r.parent, None, "a root must not expose its parent");
            // The client needs the root list to get back to home from here —
            // there is no ".." to climb.
            assert!(r.roots.contains(&home.to_string_lossy().into_owned()));

            // A name the fixture's home does not already have, so "not in home"
            // means the call put it elsewhere rather than the fixture being noisy.
            create_dir_in(None, "fresh-proj", &home, &start, &roots, &none_protected).unwrap();
            assert!(volume.join("fresh-proj").is_dir(), "must land on the persistent volume");
            assert!(!home.join("fresh-proj").exists(), "must NOT land in the ephemeral home");
        });
    }

    /// Its parent stays out of bounds: on the real volume that parent also holds
    /// the Fleet state dir and the cred-store dir.
    #[test]
    fn declared_workspace_does_not_open_up_its_parent() {
        let (_tmp, home, outside) = fixture();
        let volume = outside.join("repo");
        fs::create_dir_all(&volume).unwrap();
        with_workspace_env(Some(&volume.to_string_lossy()), || {
            let roots = browse_roots(&home, &[]);
            let up = outside.to_string_lossy().into_owned();
            assert!(browse_dir_in(Some(&up), &home, &volume, &roots, &none_protected).is_err());
            assert!(create_dir_in(Some(&up), "x", &home, &volume, &roots, &none_protected).is_err());
        });
    }

    /// Every desktop: nothing declared, so home is still the start and the only
    /// root that isn't a known workspace. A value naming a missing directory is
    /// ignored the same way — the picker must still work.
    #[test]
    fn without_the_env_home_stays_the_start() {
        let (_tmp, home, _) = fixture();
        for value in [None, Some("/definitely/not/here")] {
            with_workspace_env(value, || {
                assert_eq!(default_start(&home), home);
                assert_eq!(browse_roots(&home, &[]), vec![home.clone()]);
            });
        }
    }

    #[test]
    fn caps_huge_listings() {
        let (_tmp, home, _) = fixture();
        let many = home.join("many");
        for i in 0..(MAX_ENTRIES + 10) {
            fs::create_dir_all(many.join(format!("d{i:04}"))).unwrap();
        }
        let r = browse_dir_in(Some("many"), &home, &home, &vec![home.clone()], &none_protected).unwrap();
        assert!(r.truncated);
        assert_eq!(r.entries.len(), MAX_ENTRIES);
    }
}
