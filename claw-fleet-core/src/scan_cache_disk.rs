//! Persistent on-disk mirror of `ScanCache.session_cache`.
//!
//! `session_cache` is `HashMap<jsonl_path, (mtime_ms, SessionInfo)>` and lives
//! only in process memory. Cold-starting the desktop app on a machine with
//! thousands of `.claude/projects/*/*.jsonl` files therefore re-runs
//! `parse_session_info` for every file before the UI's session list appears.
//!
//! This module mirrors the same map to `~/.fleet/session-cache.json` so the
//! next process can seed `session_cache` and walk straight into the fast
//! mtime-equality path inside `scan_one_source`. Entries whose mtime no longer
//! matches the file on disk are re-parsed as before — correctness still rides
//! on the same invariant, just with a warm cache instead of a cold one.

use crate::session::{get_fleet_dir, SessionInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;

const CACHE_FILE_NAME: &str = "session-cache.json";
const CACHE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct DiskCache {
    version: u32,
    entries: HashMap<String, DiskEntry>,
}

#[derive(Serialize, Deserialize)]
struct DiskEntry {
    #[serde(rename = "mtimeMs")]
    mtime_ms: u64,
    info: SessionInfo,
}

fn cache_path() -> Option<PathBuf> {
    get_fleet_dir().map(|d| d.join(CACHE_FILE_NAME))
}

/// Read the persisted cache. Returns an empty map for any non-fatal condition
/// (file missing, parse error, version mismatch). Never panics; a corrupt
/// cache must not block startup.
pub fn load() -> HashMap<String, (u64, SessionInfo)> {
    let Some(p) = cache_path() else {
        return HashMap::new();
    };
    let Ok(text) = fs::read_to_string(&p) else {
        return HashMap::new();
    };
    let Ok(disk) = serde_json::from_str::<DiskCache>(&text) else {
        return HashMap::new();
    };
    if disk.version != CACHE_VERSION {
        return HashMap::new();
    }
    disk.entries
        .into_iter()
        .map(|(k, v)| (k, (v.mtime_ms, v.info)))
        .collect()
}

/// Atomically persist the cache. Writes to `<path>.tmp` then renames so a
/// crashed process can't leave a half-written file behind.
pub fn save(cache: &HashMap<String, (u64, SessionInfo)>) -> io::Result<()> {
    let p = cache_path()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no fleet dir"))?;
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let entries: HashMap<String, DiskEntry> = cache
        .iter()
        .map(|(k, (mtime, info))| {
            (
                k.clone(),
                DiskEntry {
                    mtime_ms: *mtime,
                    info: info.clone(),
                },
            )
        })
        .collect();
    let disk = DiskCache {
        version: CACHE_VERSION,
        entries,
    };
    let json = serde_json::to_string(&disk)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    let tmp = p.with_extension("json.tmp");
    fs::write(&tmp, json.as_bytes())?;
    fs::rename(&tmp, &p)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{fleet_home_lock, SessionStatus};

    fn sample_info(id: &str) -> SessionInfo {
        SessionInfo {
            id: id.into(),
            workspace_path: "/tmp/test".into(),
            workspace_name: "test".into(),
            ide_name: None,
            entrypoint: None,
            is_subagent: false,
            parent_session_id: None,
            agent_type: None,
            agent_description: None,
            slug: None,
            ai_title: None,
            status: SessionStatus::Idle,
            token_speed: 0.0,
            agent_token_speed: 0.0,
            total_output_tokens: 0,
            total_input_tokens: 0,
            total_cost_usd: 0.0,
            agent_total_cost_usd: 0.0,
            cost_speed_usd_per_min: 0.0,
            last_message_preview: None,
            last_activity_ms: 0,
            created_at_ms: 0,
            jsonl_path: format!("/tmp/{id}.jsonl"),
            model: None,
            thinking_level: None,
            pid: None,
            pid_precise: false,
            proc_alive: false,
            pending_tool_batch: false,
            last_skill: None,
            context_percent: None,
            agent_source: "claude-code".into(),
            last_outcome: None,
            rate_limit: None,
            todos: None,
            background_tasks: Vec::new(),
            task_plan: None, handoff: None, user_mark: None, last_read_ms: None,            compact_count: 0,
            compact_pre_tokens: 0,
            compact_post_tokens: 0,
            compact_cost_usd: 0.0,
        }
    }

    struct EnvGuard;
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("FLEET_HOME");
        }
    }
    // Return order matters: callers destructure as
    // `let (_lock, _env, _tmp) = with_temp_home();` and Rust drops locals in
    // reverse declaration order, so `_tmp` is dropped first, then `_env`
    // clears `FLEET_HOME`, then `_lock` releases the mutex. The previous
    // `(tmp, env, lock)` shape released the lock first, leaving a race
    // window where the next test could acquire the lock and `set_var` its
    // own tempdir before this test's `EnvGuard` cleared `FLEET_HOME`.
    fn with_temp_home() -> (std::sync::MutexGuard<'static, ()>, EnvGuard, tempfile::TempDir) {
        let lock = fleet_home_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        std::env::set_var("FLEET_HOME", tmp.path());
        (lock, EnvGuard, tmp)
    }

    #[test]
    fn roundtrip_save_load_yields_equal_map() {
        let (_lock, _env, _tmp) = with_temp_home();
        let mut cache: HashMap<String, (u64, SessionInfo)> = HashMap::new();
        cache.insert("/a/s1.jsonl".into(), (12_345, sample_info("s1")));
        cache.insert("/b/s2.jsonl".into(), (67_890, sample_info("s2")));

        save(&cache).expect("save ok");
        let loaded = load();

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get("/a/s1.jsonl").map(|(m, _)| *m), Some(12_345));
        assert_eq!(loaded.get("/a/s1.jsonl").map(|(_, i)| i.id.clone()), Some("s1".into()));
        assert_eq!(loaded.get("/b/s2.jsonl").map(|(m, _)| *m), Some(67_890));
    }

    #[test]
    fn missing_file_returns_empty() {
        let (_lock, _env, _tmp) = with_temp_home();
        let loaded = load();
        assert!(loaded.is_empty());
    }

    #[test]
    fn corrupt_file_returns_empty() {
        let (_lock, _env, tmp) = with_temp_home();
        let p = tmp.path().join(CACHE_FILE_NAME);
        fs::write(&p, b"not json").unwrap();
        let loaded = load();
        assert!(loaded.is_empty());
    }

    #[test]
    fn version_mismatch_returns_empty() {
        let (_lock, _env, tmp) = with_temp_home();
        let p = tmp.path().join(CACHE_FILE_NAME);
        fs::write(&p, br#"{"version":999,"entries":{}}"#).unwrap();
        let loaded = load();
        assert!(loaded.is_empty());
    }

    #[test]
    fn should_persist_now_returns_true_when_never_persisted() {
        use crate::session::should_persist_now;
        use std::time::{Duration, Instant};
        assert!(should_persist_now(None, Instant::now(), Duration::from_secs(30)));
    }

    #[test]
    fn should_persist_now_returns_false_inside_throttle_window() {
        use crate::session::should_persist_now;
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let recent = now - Duration::from_secs(5);
        assert!(!should_persist_now(Some(recent), now, Duration::from_secs(30)));
    }

    #[test]
    fn should_persist_now_returns_true_past_throttle_window() {
        use crate::session::should_persist_now;
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let long_ago = now - Duration::from_secs(60);
        assert!(should_persist_now(Some(long_ago), now, Duration::from_secs(30)));
    }

    #[test]
    fn scan_cache_new_seeds_session_cache_from_disk() {
        let (_lock, _env, _tmp) = with_temp_home();
        let mut cache: HashMap<String, (u64, SessionInfo)> = HashMap::new();
        cache.insert("/seed/path.jsonl".into(), (42, sample_info("seed")));
        save(&cache).expect("save ok");

        let sc = crate::session::ScanCache::new();
        let guard = sc.session_cache.lock().unwrap();
        assert_eq!(guard.len(), 1);
        assert_eq!(guard.get("/seed/path.jsonl").map(|(m, _)| *m), Some(42));
        assert_eq!(
            guard.get("/seed/path.jsonl").map(|(_, i)| i.id.clone()),
            Some("seed".into())
        );
    }
}
