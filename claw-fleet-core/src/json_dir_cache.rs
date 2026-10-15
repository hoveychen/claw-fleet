//! In-memory cache for "one small JSON file per id" directories.
//!
//! `~/.fleet/session-mark/`, `session-title/`, `task-outcome/`, `handoffs/`
//! and `procs/` each hold thousands of files that only ever grow, and every
//! incremental session rescan (plus every `list_procs` poll) used to open,
//! read and parse all of them. Measured 2026-10-15 on a 22h-old desktop
//! process: ~37% average CPU, mostly in `open(2)` under `enrich_all`.
//!
//! The cache keeps the parsed value per file together with the `(mtime, len)`
//! it was read at, and on each load only re-reads files whose stamp changed.
//! A per-file stamp is needed rather than the directory's mtime: the writers
//! use `fs::write`, which rewrites a file in place and leaves the directory
//! mtime untouched. Writers live in other processes too (the `fleet` CLI,
//! hooks), so invalidation has to come from the filesystem, not from calls.
//!
//! Even a fully cached load still `stat`s every file, and with ~17k of them
//! across these dirs that was most of what a quiet desktop rescan cost
//! (measured 2026-10-15, every 2s). A process that watches a dir for changes
//! can [`gate_dir`] it: while no change has been reported (see [`note_changed`])
//! a load reuses the last listing without touching the disk. A full listing
//! still runs every [`GATE_FULL_EVERY`] in case an event was lost.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use serde::de::DeserializeOwned;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    mtime: Option<SystemTime>,
    len: u64,
}

struct Entry<T> {
    stamp: Stamp,
    /// `None` when the file did not parse — cached too, so a malformed file
    /// is not re-read every scan.
    value: Option<T>,
}

struct DirState<T> {
    entries: HashMap<OsString, Entry<T>>,
    /// What the last full listing returned, served as-is while the dir's
    /// [gate](gate_dir) reports no change.
    out: Vec<(String, T)>,
}

pub struct JsonDirCache<T> {
    /// `None` until first use: `HashMap::new` is not `const`, and these live
    /// in `static`s.
    dirs: Mutex<Option<HashMap<PathBuf, DirState<T>>>>,
}

impl<T: DeserializeOwned + Clone> JsonDirCache<T> {
    pub const fn new() -> Self {
        Self {
            dirs: Mutex::new(None),
        }
    }

    /// Every `*.json` file in `dir` that parses as `T`, as `(file stem, value)`
    /// in directory order. Unchanged files come from memory; changed and new
    /// ones are read; files gone from disk are dropped from the cache.
    pub fn load(&self, dir: &Path) -> Vec<(String, T)> {
        {
            let lock = self.dirs.lock().unwrap();
            if let Some(state) = lock.as_ref().and_then(|d| d.get(dir)) {
                if listing_fresh(dir) {
                    return state.out.clone();
                }
            }
        }
        begin_full_listing(dir);
        let Ok(entries) = fs::read_dir(dir) else {
            if let Some(dirs) = self.dirs.lock().unwrap().as_mut() {
                dirs.remove(dir);
            }
            return Vec::new();
        };
        let mut lock = self.dirs.lock().unwrap();
        let guard = lock.get_or_insert_with(HashMap::new);
        let mut old = guard.remove(dir).map(|s| s.entries).unwrap_or_default();
        let mut fresh: HashMap<OsString, Entry<T>> = HashMap::with_capacity(old.len());
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let path = Path::new(&name);
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(String::from) else {
                continue;
            };
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let stamp = Stamp {
                mtime: meta.modified().ok(),
                len: meta.len(),
            };
            let cached = old.remove(&name).filter(|e| e.stamp == stamp);
            let value = match cached {
                Some(e) => e.value,
                None => fs::read_to_string(entry.path())
                    .ok()
                    .and_then(|s| serde_json::from_str::<T>(&s).ok()),
            };
            if let Some(v) = &value {
                out.push((stem, v.clone()));
            }
            fresh.insert(name, Entry { stamp, value });
        }
        guard.insert(
            dir.to_path_buf(),
            DirState {
                entries: fresh,
                out: out.clone(),
            },
        );
        out
    }
}

impl<T: DeserializeOwned + Clone> Default for JsonDirCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ── Change gate ──────────────────────────────────────────────────────────────

/// How long a gated dir's listing may be reused before a full one runs anyway.
/// Bounds the damage of a lost filesystem event to this much staleness.
pub const GATE_FULL_EVERY: Duration = Duration::from_secs(30);

struct Gate {
    dir: PathBuf,
    /// Other spellings of `dir` an event may arrive under — the canonical
    /// path, when `~/.fleet` sits behind a symlink.
    aliases: Vec<PathBuf>,
    dirty: bool,
    last_full: Option<Instant>,
}

impl Gate {
    fn covers(&self, path: &Path) -> bool {
        std::iter::once(&self.dir)
            .chain(&self.aliases)
            .any(|d| path.starts_with(d) || d.starts_with(path))
    }
}

static GATES: Mutex<Vec<Gate>> = Mutex::new(Vec::new());

/// The per-id dirs a scan re-reads every tick, and so the ones worth gating:
/// launch notes, manual marks, title overrides and task outcomes.
pub fn gateable_dirs() -> Vec<PathBuf> {
    [
        crate::launch_spec::spec_dir(),
        crate::session_mark::mark_dir(),
        crate::session_title::title_dir(),
        crate::task_outcome::outcome_dir(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Promise that this process reports every change under `dir` through
/// [`note_changed`] — call it only once a watcher on `dir` is running. From
/// then on, loads of `dir` (by [`JsonDirCache`] and the launch-note registry)
/// skip the disk while nothing changed. Starts dirty, so the first load after
/// it is a full one.
pub fn gate_dir(dir: &Path) {
    let mut gates = GATES.lock().unwrap();
    if gates.iter().any(|g| g.dir == dir) {
        return;
    }
    let aliases = fs::canonicalize(dir)
        .ok()
        .filter(|c| c != dir)
        .into_iter()
        .collect();
    gates.push(Gate {
        dir: dir.to_path_buf(),
        aliases,
        dirty: true,
        last_full: None,
    });
}

/// Something under (or above) `path` changed: the next load of every gated
/// dir it touches lists the disk again. Writers in this process call it too,
/// so a load right after their own write does not wait on the watcher.
pub fn note_changed(path: &Path) {
    for g in GATES.lock().unwrap().iter_mut() {
        if g.covers(path) {
            g.dirty = true;
        }
    }
}

/// The watcher itself failed or lost track: distrust every gated dir.
pub fn note_all_changed() {
    for g in GATES.lock().unwrap().iter_mut() {
        g.dirty = true;
    }
}

/// True when `dir` is gated, nothing changed since its last full listing, and
/// that listing is younger than [`GATE_FULL_EVERY`]. An ungated dir is never
/// fresh — every load lists it, as before gates existed.
pub fn listing_fresh(dir: &Path) -> bool {
    listing_fresh_at(dir, Instant::now())
}

fn listing_fresh_at(dir: &Path, now: Instant) -> bool {
    GATES.lock().unwrap().iter().any(|g| {
        g.dir == dir
            && !g.dirty
            && g.last_full
                .is_some_and(|t| now.duration_since(t) < GATE_FULL_EVERY)
    })
}

/// Record that a full listing of `dir` starts now. Clears the dirty bit
/// *before* the disk is read, so a change landing mid-listing is not lost: it
/// sets the bit again and the next load lists once more.
pub fn begin_full_listing(dir: &Path) {
    begin_full_listing_at(dir, Instant::now())
}

fn begin_full_listing_at(dir: &Path, now: Instant) {
    for g in GATES.lock().unwrap().iter_mut() {
        if g.dir == dir {
            g.dirty = false;
            g.last_full = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize, Clone, Debug, PartialEq)]
    struct Rec {
        v: u32,
    }

    fn sorted(mut v: Vec<(String, Rec)>) -> Vec<(String, u32)> {
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v.into_iter().map(|(k, r)| (k, r.v)).collect()
    }

    #[test]
    fn loads_parses_and_skips_non_json_and_malformed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.json"), r#"{"v":1}"#).unwrap();
        fs::write(dir.path().join("b.json"), "not json").unwrap();
        fs::write(dir.path().join("c.txt"), r#"{"v":3}"#).unwrap();
        let cache = JsonDirCache::<Rec>::new();
        assert_eq!(sorted(cache.load(dir.path())), vec![("a".into(), 1)]);
    }

    #[test]
    fn picks_up_in_place_rewrites_additions_and_removals() {
        let dir = tempfile::tempdir().unwrap();
        let cache = JsonDirCache::<Rec>::new();
        fs::write(dir.path().join("a.json"), r#"{"v":1}"#).unwrap();
        fs::write(dir.path().join("b.json"), r#"{"v":2}"#).unwrap();
        assert_eq!(
            sorted(cache.load(dir.path())),
            vec![("a".into(), 1), ("b".into(), 2)]
        );

        // In-place rewrite with a different length, an addition, a removal.
        fs::write(dir.path().join("a.json"), r#"{"v":100}"#).unwrap();
        fs::write(dir.path().join("c.json"), r#"{"v":3}"#).unwrap();
        fs::remove_file(dir.path().join("b.json")).unwrap();
        assert_eq!(
            sorted(cache.load(dir.path())),
            vec![("a".into(), 100), ("c".into(), 3)]
        );
    }

    #[test]
    fn same_length_rewrite_is_seen_through_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let cache = JsonDirCache::<Rec>::new();
        let p = dir.path().join("a.json");
        fs::write(&p, r#"{"v":1}"#).unwrap();
        assert_eq!(sorted(cache.load(dir.path())), vec![("a".into(), 1)]);
        fs::write(&p, r#"{"v":2}"#).unwrap();
        // Force a distinct mtime so the test does not depend on timestamp
        // granularity of the filesystem it runs on.
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert_eq!(sorted(cache.load(dir.path())), vec![("a".into(), 2)]);
    }

    #[test]
    fn unchanged_files_are_served_from_memory() {
        let dir = tempfile::tempdir().unwrap();
        let cache = JsonDirCache::<Rec>::new();
        let p = dir.path().join("a.json");
        fs::write(&p, r#"{"v":1}"#).unwrap();
        let stamp_before = fs::metadata(&p).unwrap().modified().unwrap();
        cache.load(dir.path());
        // Swap the content behind the cache's back while keeping the stamp:
        // a cache hit must return the remembered value, proving no re-read.
        fs::write(&p, r#"{"v":9}"#).unwrap();
        fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(stamp_before)
            .unwrap();
        assert_eq!(sorted(cache.load(dir.path())), vec![("a".into(), 1)]);
    }

    #[test]
    fn missing_dir_is_empty() {
        let cache = JsonDirCache::<Rec>::new();
        assert!(cache.load(Path::new("/nonexistent/fleet-json-dir-cache")).is_empty());
    }

    #[test]
    fn gated_dir_reuses_listing_until_a_change_is_noted() {
        let dir = tempfile::tempdir().unwrap();
        let cache = JsonDirCache::<Rec>::new();
        fs::write(dir.path().join("a.json"), r#"{"v":1}"#).unwrap();
        gate_dir(dir.path());
        assert_eq!(sorted(cache.load(dir.path())), vec![("a".into(), 1)]);

        // A write nobody reported is not seen: the listing is reused.
        fs::write(dir.path().join("b.json"), r#"{"v":2}"#).unwrap();
        assert_eq!(sorted(cache.load(dir.path())), vec![("a".into(), 1)]);

        // Once reported, the next load lists the disk again.
        note_changed(&dir.path().join("b.json"));
        assert_eq!(
            sorted(cache.load(dir.path())),
            vec![("a".into(), 1), ("b".into(), 2)]
        );
    }

    #[test]
    fn gate_forces_a_full_listing_after_the_fallback_window() {
        let dir = tempfile::tempdir().unwrap();
        gate_dir(dir.path());
        assert!(!listing_fresh(dir.path()), "a new gate starts dirty");
        let t0 = Instant::now();
        begin_full_listing_at(dir.path(), t0);
        assert!(listing_fresh_at(dir.path(), t0 + Duration::from_secs(1)));
        assert!(!listing_fresh_at(dir.path(), t0 + GATE_FULL_EVERY));
    }

    #[test]
    fn ungated_dir_is_never_fresh_and_unrelated_changes_do_not_dirty() {
        let gated = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        assert!(!listing_fresh(other.path()));
        gate_dir(gated.path());
        begin_full_listing(gated.path());
        note_changed(&other.path().join("x.json"));
        assert!(listing_fresh(gated.path()));
        note_changed(gated.path());
        assert!(!listing_fresh(gated.path()));
    }
}
