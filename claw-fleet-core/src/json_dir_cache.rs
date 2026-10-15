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

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

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

pub struct JsonDirCache<T> {
    /// `None` until first use: `HashMap::new` is not `const`, and these live
    /// in `static`s.
    dirs: Mutex<Option<HashMap<PathBuf, HashMap<OsString, Entry<T>>>>>,
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
        let Ok(entries) = fs::read_dir(dir) else {
            if let Some(dirs) = self.dirs.lock().unwrap().as_mut() {
                dirs.remove(dir);
            }
            return Vec::new();
        };
        let mut lock = self.dirs.lock().unwrap();
        let guard = lock.get_or_insert_with(HashMap::new);
        let mut old = guard.remove(dir).unwrap_or_default();
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
        guard.insert(dir.to_path_buf(), fresh);
        out
    }
}

impl<T: DeserializeOwned + Clone> Default for JsonDirCache<T> {
    fn default() -> Self {
        Self::new()
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
}
