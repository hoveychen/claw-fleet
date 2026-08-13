//! Directories the user explicitly added to the 仓库 page, persisted server-side.
//!
//! `file_explorer` gates every read on `known_workspaces`, which the desktop
//! derives purely from session transcripts: a directory is browsable only once
//! some Claude session has run in it. That is the right default, but it leaves
//! two holes the UI walked straight into:
//!
//!   • A repo cloned from the 仓库 page has zero sessions by construction, so
//!     the file tree answered "workspace is not a known session workspace" the
//!     moment the clone finished — it looked like the clone had failed.
//!   • The same for a directory added by hand ("添加路径"). The front end kept
//!     those in `mainViewState.files.extraPaths`, which is in-memory only, so
//!     the card also vanished on restart.
//!
//! The fix has to live on the backend, not the front end. `fleet serve` exposes
//! the explorer over HTTP, so a client-supplied "also allow this path" argument
//! would be no boundary at all — any caller could name any path. Instead the
//! set of user-added directories is itself server-side state: the backend
//! records it here, on disk, and `known_workspaces` unions it with the
//! session-derived paths. A client can ask the backend to *add* a path (an
//! explicit, auditable act) but cannot widen a single read.
//!
//! Storage is `~/.fleet/browse-paths.json`, a flat array of canonical absolute
//! paths, written through [`crate::atomic_json`] under a file lock — the
//! desktop app and a `fleet serve` process can both be adding paths at once.

use std::path::{Path, PathBuf};

use crate::atomic_json::{self, JsonLoad};

/// Upper bound on remembered paths. This list grows only by deliberate user
/// action, so the cap is a runaway guard rather than a real limit; the oldest
/// entries fall off the end.
const MAX_PATHS: usize = 200;

fn store_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("browse-paths.json"))
}

/// Canonicalize `path` for storage/comparison. Requires the directory to exist:
/// a path that can't be resolved is not something we want to persist as
/// "browsable", and canonicalization is also what makes the later equality
/// check against a session workspace meaningful (symlinked homes, `/tmp` →
/// `/private/tmp` on macOS).
fn canonicalize_dir(path: &str) -> Result<String, String> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(format!("browse path must be absolute: {path}"));
    }
    let canon = std::fs::canonicalize(p).map_err(|e| format!("browse path: {e}"))?;
    if !canon.is_dir() {
        return Err(format!("browse path is not a directory: {path}"));
    }
    Ok(canon.to_string_lossy().to_string())
}

/// Read the raw stored list. `None` means "the file could not be read" — the
/// caller must not overwrite it, because the bytes on disk may be intact.
fn load_raw(path: &Path) -> Option<Vec<String>> {
    match atomic_json::load_preserving::<Vec<String>>(path) {
        JsonLoad::Loaded(v) => Some(v),
        // Missing and Corrupt both mean "start from empty": Corrupt has already
        // preserved the bytes aside as a `.corrupt-*` sibling.
        JsonLoad::Missing | JsonLoad::Corrupt => Some(Vec::new()),
        JsonLoad::Unreadable => None,
    }
}

/// The directories the user has added, most-recently-added first.
///
/// Entries whose directory no longer exists are filtered out of the result but
/// left on disk — a repo sitting on an unmounted volume should come back when
/// the volume returns, rather than being silently forgotten by a read.
pub fn list() -> Vec<String> {
    let Some(path) = store_path() else {
        return Vec::new();
    };
    load_raw(&path)
        .unwrap_or_default()
        .into_iter()
        .filter(|p| Path::new(p).is_dir())
        .collect()
}

/// Register `path` as browsable. Idempotent — re-adding an existing entry just
/// moves it to the front. Returns the updated list.
pub fn add(path: &str) -> Result<Vec<String>, String> {
    let canon = canonicalize_dir(path)?;
    let store = store_path().ok_or("browse paths: home dir unknown")?;
    atomic_json::with_file_lock(&store, || {
        let mut list = load_raw(&store).ok_or("browse paths: store is unreadable")?;
        list.retain(|p| p != &canon);
        list.insert(0, canon.clone());
        list.truncate(MAX_PATHS);
        write(&store, &list)?;
        Ok(list)
    })
}

/// Drop `path` from the list. Returns the updated list. Removing something that
/// isn't there is not an error — the caller's intent ("this must not be in the
/// list") is satisfied either way.
///
/// The stored form is canonical, but a caller removing a directory that has
/// since been deleted can't canonicalize it, so we match on both the canonical
/// form (when available) and the raw string.
pub fn remove(path: &str) -> Result<Vec<String>, String> {
    let canon = canonicalize_dir(path).ok();
    let store = store_path().ok_or("browse paths: home dir unknown")?;
    atomic_json::with_file_lock(&store, || {
        let mut list = load_raw(&store).ok_or("browse paths: store is unreadable")?;
        list.retain(|p| p != path && Some(p) != canon.as_ref());
        write(&store, &list)?;
        Ok(list)
    })
}

fn write(store: &Path, list: &[String]) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?;
    atomic_json::write_atomic(store, &bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Point `~/.fleet` at a private temp dir for the duration of a test. Holds
    /// `fleet_home_lock` so concurrent tests can't observe each other's HOME.
    struct HomeSandbox {
        dir: PathBuf,
        prev: Option<std::ffi::OsString>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl HomeSandbox {
        fn new(tag: &str) -> Self {
            let guard = crate::session::fleet_home_lock();
            let dir = std::env::temp_dir().join(format!(
                "fleet-browse-paths-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let prev = std::env::var_os("FLEET_HOME");
            unsafe { std::env::set_var("FLEET_HOME", &dir) };
            Self { dir, prev, _guard: guard }
        }
    }

    impl Drop for HomeSandbox {
        fn drop(&mut self) {
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var("FLEET_HOME", v),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn make_dir(under: &Path, name: &str) -> String {
        let p = under.join(name);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::canonicalize(&p).unwrap().to_string_lossy().to_string()
    }

    #[test]
    fn add_then_list_roundtrips_and_survives_a_fresh_read() {
        let home = HomeSandbox::new("roundtrip");
        let repo = make_dir(&home.dir, "cloned-repo");

        assert!(list().is_empty(), "a fresh store starts empty");
        add(&repo).unwrap();
        // `list()` re-reads from disk, so this is the restart case: the entry
        // must come back from the file, not from process memory.
        assert_eq!(list(), vec![repo]);
    }

    #[test]
    fn add_is_idempotent_and_moves_the_entry_to_the_front() {
        let home = HomeSandbox::new("idempotent");
        let a = make_dir(&home.dir, "a");
        let b = make_dir(&home.dir, "b");

        add(&a).unwrap();
        add(&b).unwrap();
        assert_eq!(list(), vec![b.clone(), a.clone()]);

        add(&a).unwrap();
        assert_eq!(list(), vec![a, b], "re-adding must not duplicate");
    }

    #[test]
    fn remove_drops_the_entry_and_is_ok_when_absent() {
        let home = HomeSandbox::new("remove");
        let a = make_dir(&home.dir, "a");
        let b = make_dir(&home.dir, "b");

        add(&a).unwrap();
        add(&b).unwrap();
        remove(&a).unwrap();
        assert_eq!(list(), vec![b]);

        remove(&a).expect("removing an absent entry is not an error");
    }

    #[test]
    fn rejects_relative_paths_and_non_directories() {
        let home = HomeSandbox::new("reject");
        assert!(add("relative/path").is_err());

        let file = home.dir.join("a-file");
        std::fs::write(&file, b"x").unwrap();
        assert!(add(file.to_str().unwrap()).is_err(), "a file is not browsable");

        assert!(
            add(home.dir.join("does-not-exist").to_str().unwrap()).is_err(),
            "a missing directory must not be persisted"
        );
    }

    #[test]
    fn stored_paths_are_canonical_so_they_can_match_session_workspaces() {
        let home = HomeSandbox::new("canon");
        let repo = make_dir(&home.dir, "repo");
        // Ask with a non-canonical spelling of the same directory.
        let noisy = format!("{}/./", repo);
        add(&noisy).unwrap();
        assert_eq!(list(), vec![repo]);
    }

    #[test]
    fn a_deleted_directory_is_hidden_from_list_but_not_erased_from_disk() {
        let home = HomeSandbox::new("vanished");
        let repo = make_dir(&home.dir, "gone");
        add(&repo).unwrap();
        std::fs::remove_dir_all(&repo).unwrap();

        assert!(list().is_empty(), "a vanished directory is not offered");

        std::fs::create_dir_all(&repo).unwrap();
        assert_eq!(
            list(),
            vec![repo],
            "the entry must still be on disk so it returns with the directory"
        );
    }

    #[test]
    fn corrupt_store_starts_empty_instead_of_failing_every_read() {
        let home = HomeSandbox::new("corrupt");
        let store = home.dir.join(".fleet").join("browse-paths.json");
        std::fs::create_dir_all(store.parent().unwrap()).unwrap();
        std::fs::write(&store, b"NOT JSON [").unwrap();

        assert!(list().is_empty());
        // …and the bytes were preserved aside rather than dropped.
        let backups: Vec<_> = std::fs::read_dir(store.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".corrupt"))
            .collect();
        assert_eq!(backups.len(), 1);
    }
}
