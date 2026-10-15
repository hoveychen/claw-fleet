//! Watches the per-id `~/.fleet` dirs every rescan re-reads (launch notes,
//! marks, titles, task outcomes) and reports changes to
//! `claw_fleet_core::json_dir_cache`, so a rescan with nothing new skips the
//! ~17k `stat`s those dirs cost per pass. See `json_dir_cache::gate_dir`.
//!
//! Desktop only: a headless `fleet serve` never gates, and lists the dirs on
//! every load exactly as before.

use claw_fleet_core::json_dir_cache;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

/// Start the watcher thread. A dir is gated only once its watch is in place,
/// so no change can fall between the two.
pub fn start() {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
        Ok(w) => w,
        Err(e) => {
            crate::log_debug(&format!("[DIR-GATE] watcher init failed: {e}"));
            return;
        }
    };
    for dir in json_dir_cache::gateable_dirs() {
        // Watching needs the dir to exist; an empty one costs nothing.
        let _ = std::fs::create_dir_all(&dir);
        match watcher.watch(&dir, RecursiveMode::NonRecursive) {
            Ok(()) => json_dir_cache::gate_dir(&dir),
            Err(e) => crate::log_debug(&format!(
                "[DIR-GATE] cannot watch {}: {e}",
                dir.display()
            )),
        }
    }
    std::thread::Builder::new()
        .name("fleet-dir-gate".into())
        .spawn(move || {
            // Owned here so the watch lives as long as the thread.
            let _watcher = watcher;
            for res in rx {
                match res {
                    Ok(event) if event.need_rescan() || event.paths.is_empty() => {
                        json_dir_cache::note_all_changed()
                    }
                    Ok(event) => {
                        for p in &event.paths {
                            json_dir_cache::note_changed(p);
                        }
                    }
                    Err(_) => json_dir_cache::note_all_changed(),
                }
            }
        })
        .ok();
}
