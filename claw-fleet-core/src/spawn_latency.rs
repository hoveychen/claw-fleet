//! Spawn-to-visible latency for freshly launched sessions.
//!
//! "A new session takes ages to start" was patched several times without a way
//! to tell which leg was slow. The desktop log already has the spawn time
//! (`new_session: spawned …`) and the transcript has the CLI's own timestamps,
//! but nothing recorded when the session list a client renders first contained
//! the new id — the moment the "starting…" spinner can go away. `note_spawned`
//! remembers the spawn instant; a publisher calls `observe` with every list it
//! is about to hand to a UI and the first sighting logs one
//! `[SPAWN-VISIBLE] <id> +<ms>ms` line (read by `scripts/startup-latency.sh`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Spawns not seen within this window are dropped silently — the process died
/// before writing a transcript, or no publisher is running in this process.
const FORGET_AFTER: Duration = Duration::from_secs(600);

static PENDING: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

/// Record that `session_id` was just spawned by this process.
pub fn note_spawned(session_id: &str) {
    let mut guard = PENDING.lock().unwrap();
    guard
        .get_or_insert_with(HashMap::new)
        .insert(session_id.to_string(), Instant::now());
}

/// Check a list about to be published. Logs and forgets every pending spawn
/// whose id is in `ids`; returns those `(id, elapsed)` pairs.
pub fn observe<'a>(ids: impl IntoIterator<Item = &'a str>) -> Vec<(String, Duration)> {
    let mut guard = PENDING.lock().unwrap();
    let Some(pending) = guard.as_mut() else {
        return Vec::new();
    };
    if pending.is_empty() {
        return Vec::new();
    }
    pending.retain(|_, at| at.elapsed() < FORGET_AFTER);
    let mut seen = Vec::new();
    for id in ids {
        if let Some(at) = pending.remove(id) {
            seen.push((id.to_string(), at.elapsed()));
        }
    }
    drop(guard);
    for (id, took) in &seen {
        crate::log_debug(&format!("[SPAWN-VISIBLE] {} +{}ms", id, took.as_millis()));
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_sighting_is_reported_once() {
        note_spawned("spawn-latency-test-a");
        assert!(observe(["unrelated"]).is_empty());
        let seen = observe(["x", "spawn-latency-test-a"]);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "spawn-latency-test-a");
        assert!(observe(["spawn-latency-test-a"]).is_empty());
    }
}
