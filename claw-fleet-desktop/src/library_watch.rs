//! Tells the front end when the library changed on disk, so the 资料 page and
//! an open doc preview pick up a new version without restarting the app.
//!
//! Publishes and artifact adds mostly come from another process (the `fleet`
//! CLI or MCP server an agent runs), so the desktop never sees the write go by
//! — the store directories are the only shared signal. One recursive watch per
//! store root; only the per-item metadata files count, so the search index
//! (`library/search.db`, rewritten by every search) and version payloads being
//! written cannot make the page refetch itself in a loop. Bursts (a publish
//! writes the version, then `doc.json`, then the summarizer writes
//! `summary.json`) are coalesced into one `library-changed` event.
use std::path::Path;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter};

/// Quiet period after the last relevant change before the event fires.
const DEBOUNCE: Duration = Duration::from_millis(400);

/// Files whose change means the list or a doc's current version changed:
/// a wiki doc's metadata and AI summary, the stub a moved doc leaves behind,
/// an artifact's metadata (its summary lives in there too), and a project's
/// folder schema.
const META_FILES: &[&str] = &[
    claw_fleet_core::wiki::SUMMARY_FILE,
    claw_fleet_core::wiki::REDIRECT_FILE,
    "doc.json",
    "meta.json",
    "project.json",
];

/// Whether one watcher event should refresh the library views.
pub(crate) fn is_library_change(kind: &EventKind, path: &Path) -> bool {
    match kind {
        EventKind::Access(_) => false,
        // A deleted doc or artifact goes as a whole directory, and the event
        // may name only that directory.
        EventKind::Remove(_) => !is_index_file(path),
        _ => path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| META_FILES.contains(&n)),
    }
}

fn is_index_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("search.db"))
}

pub fn start(app: AppHandle) {
    let roots = match claw_fleet_core::library::Roots::home() {
        Ok(r) => r,
        Err(e) => {
            crate::log_debug(&format!("[LIBRARY-WATCH] no roots: {e}"));
            return;
        }
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
        Ok(w) => w,
        Err(e) => {
            crate::log_debug(&format!("[LIBRARY-WATCH] watcher init failed: {e}"));
            return;
        }
    };
    // `library/projects`, not `library`: the rest of that dir is the search
    // index, backfill state and ingest batches, which the page already polls.
    for dir in [roots.wiki, roots.artifacts, roots.library.join("projects")] {
        // Watching needs the dir to exist; an empty one costs nothing.
        let _ = std::fs::create_dir_all(&dir);
        if let Err(e) = watcher.watch(&dir, RecursiveMode::Recursive) {
            crate::log_debug(&format!(
                "[LIBRARY-WATCH] cannot watch {}: {e}",
                dir.display()
            ));
        }
    }
    std::thread::Builder::new()
        .name("fleet-library-watch".into())
        .spawn(move || {
            // Owned here so the watch lives as long as the thread.
            let _watcher = watcher;
            let mut dirty = false;
            loop {
                let res = if dirty {
                    rx.recv_timeout(DEBOUNCE)
                } else {
                    rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
                };
                match res {
                    Ok(Ok(event)) => {
                        if event.need_rescan()
                            || event.paths.iter().any(|p| is_library_change(&event.kind, p))
                        {
                            dirty = true;
                        }
                    }
                    // Lost events: refresh rather than risk a stale page.
                    Ok(Err(_)) => dirty = true,
                    Err(RecvTimeoutError::Timeout) => {
                        let _ = app.emit("library-changed", ());
                        dirty = false;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, DataChange, ModifyKind, RemoveKind};
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn republishing_a_doc_counts() {
        let kind = EventKind::Modify(ModifyKind::Data(DataChange::Content));
        assert!(is_library_change(&kind, &p("/h/.fleet/wiki/arch%2Fx/doc.json")));
        assert!(is_library_change(&kind, &p("/h/.fleet/wiki/arch%2Fx/summary.json")));
        assert!(is_library_change(&kind, &p("/h/.fleet/artifacts/20260101-1/meta.json")));
        assert!(is_library_change(
            &kind,
            &p("/h/.fleet/library/projects/abc/project.json")
        ));
    }

    #[test]
    fn version_payloads_and_temp_files_do_not() {
        let kind = EventKind::Create(CreateKind::File);
        assert!(!is_library_change(
            &kind,
            &p("/h/.fleet/wiki/arch%2Fx/versions/20260101/index.md")
        ));
        assert!(!is_library_change(&kind, &p("/h/.fleet/wiki/arch%2Fx/doc.json.tmp")));
        assert!(!is_library_change(&kind, &p("/h/.fleet/artifacts/1/blob")));
    }

    #[test]
    fn a_removed_item_counts_but_reads_never_do() {
        assert!(is_library_change(
            &EventKind::Remove(RemoveKind::Folder),
            &p("/h/.fleet/wiki/arch%2Fx")
        ));
        assert!(!is_library_change(
            &EventKind::Access(AccessKind::Read),
            &p("/h/.fleet/wiki/arch%2Fx/doc.json")
        ));
    }

    /// The publish path writes `doc.json.tmp` and renames it over `doc.json`
    /// (`wiki::write_doc_json`). The filter only sees what the platform watcher
    /// reports for that rename, so check it against a real watcher.
    #[test]
    fn a_real_tmp_then_rename_publish_is_seen() {
        let root = tempfile::tempdir().unwrap();
        let doc_dir = root.path().join("arch%2Fx");
        std::fs::create_dir_all(&doc_dir).unwrap();
        std::fs::write(doc_dir.join("doc.json"), "{}").unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let mut w = RecommendedWatcher::new(tx, Config::default()).unwrap();
        w.watch(root.path(), RecursiveMode::Recursive).unwrap();
        // FSEvents can report changes from just before the stream started;
        // drain those so only the publish below is judged.
        std::thread::sleep(Duration::from_millis(300));
        while rx.try_recv().is_ok() {}

        std::fs::write(doc_dir.join("doc.json.tmp"), r#"{"v":2}"#).unwrap();
        std::fs::rename(doc_dir.join("doc.json.tmp"), doc_dir.join("doc.json")).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut seen = false;
        while !seen && std::time::Instant::now() < deadline {
            if let Ok(Ok(ev)) = rx.recv_timeout(Duration::from_millis(200)) {
                seen = ev.paths.iter().any(|p| is_library_change(&ev.kind, p));
            }
        }
        assert!(seen, "no event for doc.json after tmp+rename");
    }

    #[test]
    fn the_search_index_never_counts() {
        assert!(!is_library_change(
            &EventKind::Remove(RemoveKind::File),
            &p("/h/.fleet/library/search.db-wal")
        ));
    }
}
