//! Atomic, non-destructive JSON persistence for the `~/.fleet` on-disk stores.
//!
//! Several stores (usage-occupancy history, codex usage history, audit history,
//! user audit rules) followed the same fragile pattern: a non-atomic
//! `fs::write` truncates-then-writes the whole file, while the loader swallows
//! any read/parse failure to an empty default and the next writer overwrites the
//! file. When a concurrent reader caught a write mid-flight it parsed the torn
//! bytes as empty and clobbered the entire history — the wipe that emptied
//! `claw-fleet-usage-history.json` down to the last hour.
//!
//! This module centralizes the fix so no store re-invents (or re-breaks) it:
//! - [`write_atomic`] writes to a process- and call-unique temp file then
//!   `rename`s it over the target. `rename(2)` is atomic on POSIX, so a reader
//!   never observes a half-written file.
//! - [`load_preserving`] distinguishes *absent* (use default), *parsed*,
//!   *unreadable* (a transient I/O error — the caller must NOT overwrite), and
//!   *corrupt* (present but unparseable — the bytes are renamed aside to a
//!   `.corrupt-*` sibling for recovery, then the caller starts fresh).
//! - [`with_file_lock`] serializes a cross-process read-modify-write behind an
//!   exclusive advisory lock (`std::fs::File::lock`, stable since Rust 1.89), so
//!   two Fleet processes appending to the same store can't lost-update each
//!   other.

use std::path::Path;

/// Outcome of a preserving JSON load. `T` is the deserialized document type.
pub enum JsonLoad<T> {
    /// File does not exist — the caller should use its empty/default value.
    Missing,
    /// File read and parsed successfully.
    Loaded(T),
    /// File exists but could not be *read* (a transient I/O error, not a missing
    /// file). The bytes on disk may be perfectly intact, so a write-path caller
    /// must NOT overwrite them this round.
    Unreadable,
    /// File exists and was read but is not valid JSON — genuine corruption. The
    /// bytes have been renamed aside to a `<path>.corrupt-<ts>` sibling for
    /// recovery; the caller should continue from its empty/default value.
    Corrupt,
}

/// Read and parse `path`, preserving a corrupt file instead of silently dropping
/// it. See [`JsonLoad`] for the four outcomes.
pub fn load_preserving<T: serde::de::DeserializeOwned>(path: &Path) -> JsonLoad<T> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return JsonLoad::Missing,
        Err(_) => return JsonLoad::Unreadable,
    };
    match serde_json::from_str::<T>(&raw) {
        Ok(v) => JsonLoad::Loaded(v),
        Err(_) => {
            backup_corrupt(path);
            JsonLoad::Corrupt
        }
    }
}

/// Rename a corrupt file aside with a timestamped `.corrupt-*` suffix so its
/// bytes stay recoverable instead of being overwritten by the next write.
fn backup_corrupt(path: &Path) {
    let stamp = chrono::Utc::now().timestamp_millis();
    let mut bak = path.as_os_str().to_owned();
    bak.push(format!(".corrupt-{stamp}"));
    let _ = std::fs::rename(path, std::path::PathBuf::from(bak));
}

/// Per-process, per-call unique suffix so two concurrent writers never race on
/// the same temp path.
static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Write `bytes` to `path` atomically: to a unique temp file, then `rename` over
/// the target. A reader therefore only ever sees the complete old file or the
/// complete new file, never a truncated one.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(format!(".tmp.{}.{}", std::process::id(), seq));
    let tmp = std::path::PathBuf::from(tmp_name);
    std::fs::write(&tmp, bytes)?;
    if let Err(first) = std::fs::rename(&tmp, path) {
        // Some platforms (Windows) can't rename onto an existing file: drop the
        // old file first, then retry. Still far narrower than a full truncate.
        let _ = std::fs::remove_file(path);
        if let Err(second) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            let _ = first;
            return Err(second);
        }
    }
    Ok(())
}

/// Run `f` while holding an exclusive advisory lock, so a cross-process
/// read-modify-write on `path` is serialized. The lock is taken on a sibling
/// `<path>.lock` file (never on `path` itself, which gets renamed over by
/// [`write_atomic`]). Best-effort: if the lock file can't be opened or locked,
/// `f` still runs — degrading to the unlocked path is better than dropping the
/// write entirely.
pub fn with_file_lock<R>(path: &Path, f: impl FnOnce() -> R) -> R {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut lock_name = path.as_os_str().to_owned();
    lock_name.push(".lock");
    let lock_path = std::path::PathBuf::from(lock_name);
    let guard = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .ok();
    if let Some(ref file) = guard {
        let _ = file.lock(); // blocks until the exclusive lock is held
    }
    let out = f();
    if let Some(ref file) = guard {
        let _ = file.unlock();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn write_atomic_roundtrips_and_leaves_no_temp_residue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.json");
        write_atomic(&path, br#"{"a":1}"#).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"a":1}"#);
        // No stray .tmp.* files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "temp files must be renamed away, found: {leftovers:?}");
    }

    #[test]
    fn load_preserving_reports_missing_for_absent_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.json");
        assert!(matches!(
            load_preserving::<Vec<i32>>(&path),
            JsonLoad::Missing
        ));
    }

    #[test]
    fn load_preserving_backs_up_corrupt_bytes_and_reports_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.json");
        let garbage = "[1,2,3 NOT JSON";
        std::fs::write(&path, garbage).unwrap();

        assert!(matches!(
            load_preserving::<Vec<i32>>(&path),
            JsonLoad::Corrupt
        ));

        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".corrupt"))
            .collect();
        assert_eq!(backups.len(), 1, "corrupt bytes must be preserved, not dropped");
        assert_eq!(std::fs::read_to_string(backups[0].path()).unwrap(), garbage);
    }

    #[test]
    fn load_preserving_parses_a_good_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.json");
        write_atomic(&path, br#"[1,2,3]"#).unwrap();
        match load_preserving::<Vec<i32>>(&path) {
            JsonLoad::Loaded(v) => assert_eq!(v, vec![1, 2, 3]),
            _ => panic!("expected Loaded"),
        }
    }

    #[test]
    fn with_file_lock_serializes_concurrent_read_modify_write() {
        // Two threads each do load→+1→store 200 times against the same counter
        // file, all under with_file_lock. If the lock actually serializes the
        // RMW, no update is lost and the final value is exactly 400.
        let dir = tempfile::tempdir().unwrap();
        let path = std::sync::Arc::new(dir.path().join("counter.json"));
        write_atomic(&path, b"0").unwrap();

        // A shared in-process check that lock is held exclusively.
        let inside = std::sync::Arc::new(AtomicUsize::new(0));

        let worker = |path: std::sync::Arc<std::path::PathBuf>, inside: std::sync::Arc<AtomicUsize>| {
            for _ in 0..200 {
                with_file_lock(&path, || {
                    // Only one thread may be inside the critical section.
                    assert_eq!(inside.fetch_add(1, Ordering::SeqCst), 0, "lock not exclusive");
                    let n: i64 = match load_preserving::<i64>(&path) {
                        JsonLoad::Loaded(v) => v,
                        _ => 0,
                    };
                    write_atomic(&path, (n + 1).to_string().as_bytes()).unwrap();
                    inside.fetch_sub(1, Ordering::SeqCst);
                });
            }
        };

        let (p1, i1) = (path.clone(), inside.clone());
        let (p2, i2) = (path.clone(), inside.clone());
        let h1 = std::thread::spawn(move || worker(p1, i1));
        let h2 = std::thread::spawn(move || worker(p2, i2));
        h1.join().unwrap();
        h2.join().unwrap();

        let final_n: i64 = match load_preserving::<i64>(&path) {
            JsonLoad::Loaded(v) => v,
            _ => -1,
        };
        assert_eq!(final_n, 400, "no update should be lost under the lock");
    }
}
