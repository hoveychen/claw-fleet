//! Manages a Fleet-owned injection into `~/.claude/settings.json`'s
//! `permissions.allow` list so that the Claude Code native permission prompt
//! defers to Fleet's own `fleet guard` PreToolUse hook instead of double-prompting
//! the user.
//!
//! Lifecycle (multi-process safe):
//!
//! - `acquire(pid)` snapshots the user's pre-injection state on the very first
//!   holder, then adds the eight Fleet rules to `permissions.allow`.  Repeat
//!   acquisitions just register additional pids without re-mutating settings.
//! - `release(pid)` removes the pid from holders.  When the last holder leaves,
//!   the snapshot is consulted to restore settings.json (or delete it if it
//!   never existed before Fleet touched it) and the lock file is removed.
//! - `prune_dead_holders` is called inside both acquire/release so a hard crash
//!   (`kill -9`) self-heals on next Fleet boot: dead pids are dropped, and if
//!   that empties the holder list the injection is restored before the new
//!   acquire re-applies it.
//!
//! The lock file lives at `~/.fleet/permissions-lock.json`.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::session::{
    deserialize_holders, get_claude_dir, get_fleet_dir,
    prune_dead_holders as prune_holder_entries, HolderEntry,
};

/// The full set of tool patterns Fleet injects into `permissions.allow`.
///
/// `Bash(*)` is the load-bearing one — it suppresses Claude Code's built-in
/// command prompt so `fleet guard` becomes the sole audit gate.  The other
/// patterns smooth out incidental prompts the user already trusts Fleet to
/// orchestrate (file IO, web fetch, skills, monitoring). The two `mcp__fleet__*`
/// rules pre-authorise Fleet's own MCP tools (`fleet__ask` / `fleet__render_a2ui`)
/// so Claude Code stops prompting on every invocation now that the desktop
/// already renders + audits them via the Decision Panel.
pub const INJECT_RULES: &[&str] = &[
    "Bash(*)",
    "Read(*)",
    "Write(*)",
    "Edit(*)",
    "WebFetch(*)",
    "WebSearch(*)",
    "Skill(*)",
    "Monitor(*)",
    "mcp__fleet__fleet__ask",
    "mcp__fleet__fleet__render_a2ui",
];

const LOCK_FILE_NAME: &str = "permissions-lock.json";
const CONFIG_FILE_NAME: &str = "permissions-config.json";
const CLAUDE_SETTINGS_FILE: &str = "settings.json";

/// User-controlled toggle for the injection feature.
///
/// Persisted at `~/.fleet/permissions-config.json`.  When `enabled = false`,
/// callers should skip the `acquire`/`release` calls; release is always safe
/// to invoke regardless (no-op when no lock exists), so the on-exit cleanup
/// stays unconditional.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PermissionsConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

fn config_path() -> Option<PathBuf> {
    get_fleet_dir().map(|d| d.join(CONFIG_FILE_NAME))
}

/// Load the toggle config from disk.  Returns the default (`enabled = true`)
/// if the file is missing, unparseable, or the fleet dir isn't resolvable.
pub fn load_config() -> PermissionsConfig {
    let Some(p) = config_path() else {
        return PermissionsConfig::default();
    };
    fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist the toggle config to disk.  Creates `~/.fleet/` as needed.
pub fn save_config(cfg: &PermissionsConfig) -> std::io::Result<()> {
    let p = config_path()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no fleet dir"))?;
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    fs::write(&p, json)
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq)]
pub struct PermissionsLock {
    /// `permissions.allow` as observed at the moment the lock was first acquired.
    #[serde(default)]
    pub original_allow: Vec<String>,
    /// Whether `permissions` existed as an object in settings.json at first acquisition.
    #[serde(default)]
    pub original_had_permissions: bool,
    /// Whether settings.json existed on disk at first acquisition.
    #[serde(default)]
    pub original_existed: bool,
    /// Live Fleet process holders currently keeping the injection in place.
    /// Each entry pairs the holder pid with its `start_time_secs` so
    /// PID reuse can't fool `prune_dead_holders`. See
    /// [`crate::session::HolderEntry`].
    #[serde(default, deserialize_with = "deserialize_holders")]
    pub holders: Vec<HolderEntry>,
}

fn lock_path() -> Option<PathBuf> {
    get_fleet_dir().map(|d| d.join(LOCK_FILE_NAME))
}

fn settings_path() -> Option<PathBuf> {
    get_claude_dir().map(|d| d.join(CLAUDE_SETTINGS_FILE))
}

fn read_lock() -> Option<PermissionsLock> {
    let p = lock_path()?;
    let s = fs::read_to_string(&p).ok()?;
    serde_json::from_str(&s).ok()
}

fn write_lock(lock: &PermissionsLock) -> std::io::Result<()> {
    let p = lock_path()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no fleet dir"))?;
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(lock)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    fs::write(&p, json)
}

fn delete_lock() -> std::io::Result<()> {
    let Some(p) = lock_path() else { return Ok(()) };
    if p.exists() {
        fs::remove_file(p)?;
    }
    Ok(())
}

fn read_settings() -> std::io::Result<(serde_json::Value, bool)> {
    let p = settings_path()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no claude dir"))?;
    if !p.exists() {
        return Ok((serde_json::Value::Object(Default::default()), false));
    }
    let s = fs::read_to_string(&p)?;
    let v: serde_json::Value = serde_json::from_str(&s)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok((v, true))
}

fn write_settings(v: &serde_json::Value) -> std::io::Result<()> {
    let p = settings_path()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no claude dir"))?;
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(v)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    fs::write(&p, json)
}

fn delete_settings() -> std::io::Result<()> {
    let Some(p) = settings_path() else { return Ok(()) };
    if p.exists() {
        fs::remove_file(p)?;
    }
    Ok(())
}

fn extract_allow(v: &serde_json::Value) -> (Vec<String>, bool) {
    let perms = v.get("permissions");
    let had = perms.map(|p| p.is_object()).unwrap_or(false);
    let allow = perms
        .and_then(|p| p.get("allow"))
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    (allow, had)
}

fn set_allow(v: &mut serde_json::Value, allow: Vec<String>) {
    if !v.is_object() {
        *v = serde_json::Value::Object(Default::default());
    }
    let obj = v.as_object_mut().expect("just ensured object");
    let perms = obj
        .entry("permissions".to_string())
        .or_insert_with(|| serde_json::Value::Object(Default::default()));
    if !perms.is_object() {
        *perms = serde_json::Value::Object(Default::default());
    }
    perms
        .as_object_mut()
        .expect("just ensured object")
        .insert("allow".to_string(), serde_json::json!(allow));
}

fn strip_permissions(v: &mut serde_json::Value) {
    if let Some(obj) = v.as_object_mut() {
        obj.remove("permissions");
    }
}

/// Drop holder entries whose process is no longer alive **or** whose
/// `start_time_secs` no longer matches the live process at that pid.
/// Delegates to [`crate::session::prune_dead_holders`]; the start_time
/// match is the PID-reuse defence (see memory
/// `project_mcp_injector_pid_reuse`).
pub fn prune_dead_holders(lock: &mut PermissionsLock) {
    prune_holder_entries(&mut lock.holders);
}

/// Register `pid` as a holder, injecting Fleet's allow rules if this is the
/// first live holder.
///
/// Idempotent: calling `acquire` repeatedly with the same pid is safe and is a
/// no-op after the first call.  Dead holders left behind by a crashed prior
/// run are pruned before this call's pid is added.
pub fn acquire(pid: u32) -> std::io::Result<()> {
    let mut lock = read_lock().unwrap_or_default();
    prune_dead_holders(&mut lock);
    let was_empty = lock.holders.is_empty();

    if was_empty {
        let (settings, existed) = read_settings()?;
        let (allow, had_permissions) = extract_allow(&settings);
        lock.original_allow = allow.clone();
        lock.original_existed = existed;
        lock.original_had_permissions = had_permissions;

        let mut merged = allow;
        for rule in INJECT_RULES {
            if !merged.iter().any(|s| s == rule) {
                merged.push((*rule).to_string());
            }
        }

        let mut new_settings = if existed {
            settings
        } else {
            serde_json::Value::Object(Default::default())
        };
        set_allow(&mut new_settings, merged);
        write_settings(&new_settings)?;
    }

    if !lock.holders.iter().any(|h| h.pid == pid) {
        lock.holders.push(HolderEntry::capture(pid));
    }
    write_lock(&lock)?;
    Ok(())
}

/// Deregister `pid` as a holder.  When the last live holder leaves, the
/// pre-injection settings state is restored from the snapshot stored in the
/// lock file, and the lock file is removed.
///
/// Safe to call when no lock exists (returns `Ok(())`).
pub fn release(pid: u32) -> std::io::Result<()> {
    let Some(mut lock) = read_lock() else { return Ok(()) };
    prune_dead_holders(&mut lock);
    lock.holders.retain(|h| h.pid != pid);

    if !lock.holders.is_empty() {
        return write_lock(&lock);
    }

    restore_from_snapshot(&lock)?;
    delete_lock()?;
    Ok(())
}

fn restore_from_snapshot(lock: &PermissionsLock) -> std::io::Result<()> {
    if !lock.original_existed {
        // Settings.json didn't exist before Fleet touched it.  Strip our entries;
        // if nothing else is left, delete the file entirely.
        let (mut current, exists) = read_settings()?;
        if !exists {
            return Ok(());
        }
        let (current_allow, _) = extract_allow(&current);
        let stripped: Vec<String> = current_allow
            .into_iter()
            .filter(|s| !INJECT_RULES.contains(&s.as_str()))
            .collect();
        // If permissions.allow is now empty and permissions only had allow, drop permissions.
        let perms_obj_keys_only_allow = current
            .get("permissions")
            .and_then(|p| p.as_object())
            .map(|o| o.len() == 1 && o.contains_key("allow"))
            .unwrap_or(false);
        if stripped.is_empty() && perms_obj_keys_only_allow {
            strip_permissions(&mut current);
        } else {
            set_allow(&mut current, stripped);
        }
        // If the resulting file is an empty object, delete it.
        let is_empty_obj = current
            .as_object()
            .map(|o| o.is_empty())
            .unwrap_or(false);
        if is_empty_obj {
            delete_settings()?;
        } else {
            write_settings(&current)?;
        }
        return Ok(());
    }

    // Settings.json existed originally.  Restore the original allow exactly.
    let (mut current, exists) = read_settings()?;
    if !exists {
        // User (or something else) deleted settings.json while Fleet was running.
        // Recreate it with the original snapshot rather than silently swallowing
        // the deletion: that matches the explicit "Fleet restores on shutdown"
        // contract better than no-op-ing here.
        let mut fresh = serde_json::Value::Object(Default::default());
        if lock.original_had_permissions {
            set_allow(&mut fresh, lock.original_allow.clone());
        }
        let is_empty_obj = fresh.as_object().map(|o| o.is_empty()).unwrap_or(true);
        if !is_empty_obj {
            write_settings(&fresh)?;
        }
        return Ok(());
    }

    if !lock.original_had_permissions {
        strip_permissions(&mut current);
        write_settings(&current)?;
        return Ok(());
    }

    let originals: std::collections::HashSet<&str> =
        lock.original_allow.iter().map(|s| s.as_str()).collect();
    let (current_allow, _) = extract_allow(&current);
    let restored: Vec<String> = current_allow
        .into_iter()
        .filter(|s| {
            if originals.contains(s.as_str()) {
                return true;
            }
            !INJECT_RULES.contains(&s.as_str())
        })
        .collect();
    set_allow(&mut current, restored);
    write_settings(&current)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fleet_home_lock;
    use std::sync::MutexGuard;
    use tempfile::TempDir;

    struct TestEnv {
        _tmp: TempDir,
        _guard: MutexGuard<'static, ()>,
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            std::env::remove_var("FLEET_HOME");
        }
    }

    fn setup() -> TestEnv {
        let guard = fleet_home_lock();
        let tmp = TempDir::new().unwrap();
        std::env::set_var("FLEET_HOME", tmp.path());
        TestEnv { _tmp: tmp, _guard: guard }
    }

    fn read_settings_for_test() -> Option<serde_json::Value> {
        let p = settings_path().unwrap();
        if !p.exists() {
            return None;
        }
        let s = fs::read_to_string(p).ok()?;
        serde_json::from_str(&s).ok()
    }

    fn write_settings_for_test(v: &serde_json::Value) {
        write_settings(v).unwrap();
    }

    fn allow_of(v: &serde_json::Value) -> Vec<String> {
        extract_allow(v).0
    }

    fn dead_pid() -> u32 {
        // Spawn /bin/true (or `cmd /C exit` on Windows), reap it, return its pid.
        #[cfg(unix)]
        let mut child = std::process::Command::new("true").spawn().unwrap();
        #[cfg(windows)]
        let mut child = std::process::Command::new("cmd").args(["/C", "exit"]).spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    /// Spawn a long-running child so its pid is guaranteed live for the test's duration.
    /// Caller must `.kill()` + `.wait()` it before the test returns.
    fn live_child() -> std::process::Child {
        #[cfg(unix)]
        {
            std::process::Command::new("sleep").arg("60").spawn().unwrap()
        }
        #[cfg(windows)]
        {
            std::process::Command::new("ping")
                .args(["-n", "60", "127.0.0.1"])
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        }
    }

    #[test]
    fn acquire_cold_start_creates_settings() {
        let _env = setup();
        assert!(read_settings_for_test().is_none());
        acquire(1234).unwrap();
        let v = read_settings_for_test().expect("settings.json created");
        let allow = allow_of(&v);
        assert_eq!(allow.len(), INJECT_RULES.len());
        for rule in INJECT_RULES {
            assert!(allow.iter().any(|s| s == rule), "missing rule {rule}");
        }
        let lock = read_lock().expect("lock written");
        assert!(lock.holders.iter().any(|h| h.pid == 1234));
        assert!(!lock.original_existed);
        assert!(!lock.original_had_permissions);
        assert!(lock.original_allow.is_empty());
    }

    #[test]
    fn acquire_preserves_existing_allow_entries() {
        let _env = setup();
        write_settings_for_test(&serde_json::json!({
            "permissions": { "allow": ["Bash(npm run:*)", "Read(/etc/*)"] },
            "theme": "dark",
        }));
        acquire(1234).unwrap();
        let v = read_settings_for_test().unwrap();
        let allow = allow_of(&v);
        assert!(allow.contains(&"Bash(npm run:*)".to_string()));
        assert!(allow.contains(&"Read(/etc/*)".to_string()));
        for rule in INJECT_RULES {
            assert!(allow.iter().any(|s| s == rule));
        }
        assert_eq!(v.get("theme").and_then(|t| t.as_str()), Some("dark"));
        let lock = read_lock().unwrap();
        assert_eq!(lock.original_allow, vec!["Bash(npm run:*)", "Read(/etc/*)"]);
        assert!(lock.original_had_permissions);
        assert!(lock.original_existed);
    }

    #[test]
    fn acquire_dedups_when_rule_already_present() {
        let _env = setup();
        write_settings_for_test(&serde_json::json!({
            "permissions": { "allow": ["Bash(*)", "WebFetch(*)"] },
        }));
        acquire(1234).unwrap();
        let allow = allow_of(&read_settings_for_test().unwrap());
        // Each rule still appears exactly once.
        for rule in INJECT_RULES {
            let count = allow.iter().filter(|s| s.as_str() == *rule).count();
            assert_eq!(count, 1, "rule {rule} duplicated");
        }
    }

    #[test]
    fn acquire_idempotent_for_same_pid() {
        let _env = setup();
        acquire(1234).unwrap();
        let snap = read_settings_for_test().unwrap();
        acquire(1234).unwrap();
        assert_eq!(read_settings_for_test().unwrap(), snap);
        let lock = read_lock().unwrap();
        let count = lock.holders.iter().filter(|h| h.pid == 1234).count();
        assert_eq!(count, 1);
    }

    #[test]
    fn release_last_holder_restores_original() {
        let _env = setup();
        write_settings_for_test(&serde_json::json!({
            "permissions": { "allow": ["Bash(npm run:*)"] },
            "theme": "dark",
        }));
        acquire(1234).unwrap();
        release(1234).unwrap();
        let v = read_settings_for_test().expect("file preserved");
        assert_eq!(allow_of(&v), vec!["Bash(npm run:*)"]);
        assert_eq!(v.get("theme").and_then(|t| t.as_str()), Some("dark"));
        assert!(read_lock().is_none(), "lock file removed");
    }

    #[test]
    fn release_last_holder_with_no_original_file_deletes_settings() {
        let _env = setup();
        acquire(1234).unwrap();
        release(1234).unwrap();
        assert!(read_settings_for_test().is_none());
        assert!(read_lock().is_none());
    }

    #[test]
    fn release_last_holder_with_original_no_permissions_strips_block() {
        let _env = setup();
        write_settings_for_test(&serde_json::json!({ "theme": "dark" }));
        acquire(1234).unwrap();
        release(1234).unwrap();
        let v = read_settings_for_test().expect("file preserved");
        assert!(v.get("permissions").is_none());
        assert_eq!(v.get("theme").and_then(|t| t.as_str()), Some("dark"));
    }

    #[test]
    fn release_with_other_holders_keeps_injection() {
        let _env = setup();
        let mut alive = live_child();
        let other = alive.id();
        let me = std::process::id();
        acquire(me).unwrap();
        acquire(other).unwrap();
        release(me).unwrap();
        let allow = allow_of(&read_settings_for_test().unwrap());
        assert!(allow.iter().any(|s| s == "Bash(*)"));
        let lock = read_lock().unwrap();
        assert_eq!(lock.holders.len(), 1);
        assert_eq!(lock.holders[0].pid, other);
        alive.kill().ok();
        alive.wait().ok();
    }

    #[test]
    fn release_preserves_user_entries_added_mid_run() {
        let _env = setup();
        write_settings_for_test(&serde_json::json!({
            "permissions": { "allow": ["Bash(npm run:*)"] },
        }));
        acquire(1234).unwrap();

        // User adds an entry while Fleet is running.
        let mut v = read_settings_for_test().unwrap();
        let mut allow = allow_of(&v);
        allow.push("Read(./secrets/*)".to_string());
        set_allow(&mut v, allow);
        write_settings_for_test(&v);

        release(1234).unwrap();
        let allow = allow_of(&read_settings_for_test().unwrap());
        assert!(allow.contains(&"Bash(npm run:*)".to_string()), "original kept");
        assert!(allow.contains(&"Read(./secrets/*)".to_string()), "user addition kept");
        // Fleet's injected rules should be gone.
        assert!(!allow.contains(&"WebFetch(*)".to_string()));
    }

    #[test]
    fn acquire_prunes_dead_holders_before_adding() {
        let _env = setup();
        // Seed a lock containing a dead pid (start_time 0 doubles as "stale
        // marker" — prune drops it either way).
        let dead = dead_pid();
        write_lock(&PermissionsLock {
            original_allow: vec![],
            original_had_permissions: false,
            original_existed: false,
            holders: vec![HolderEntry { pid: dead, start_time_secs: 0 }],
        })
        .unwrap();

        acquire(1234).unwrap();
        let lock = read_lock().unwrap();
        assert!(!lock.holders.iter().any(|h| h.pid == dead), "dead pid pruned");
        assert!(lock.holders.iter().any(|h| h.pid == 1234));
        // Settings should have been injected (the dead holder didn't prevent cold-start).
        let allow = allow_of(&read_settings_for_test().unwrap());
        assert!(allow.iter().any(|s| s == "Bash(*)"));
    }

    #[test]
    fn config_defaults_to_enabled_when_missing() {
        let _env = setup();
        let cfg = load_config();
        assert!(cfg.enabled);
    }

    #[test]
    fn config_round_trip() {
        let _env = setup();
        save_config(&PermissionsConfig { enabled: false }).unwrap();
        assert!(!load_config().enabled);
        save_config(&PermissionsConfig { enabled: true }).unwrap();
        assert!(load_config().enabled);
    }

    #[test]
    fn config_load_tolerates_unparseable_file() {
        let _env = setup();
        let p = config_path().unwrap();
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "{ not json").unwrap();
        // Should fall back to default rather than panic.
        assert!(load_config().enabled);
    }

    #[test]
    fn release_no_lock_is_noop() {
        let _env = setup();
        // Should not panic.
        release(1234).unwrap();
    }

    #[test]
    fn release_unknown_pid_does_not_strip_when_holders_remain() {
        let _env = setup();
        let me = std::process::id();
        acquire(me).unwrap();
        // Releasing a pid that isn't in holders should be safe and leave injection in place.
        release(dead_pid()).unwrap();
        let allow = allow_of(&read_settings_for_test().unwrap());
        assert!(allow.iter().any(|s| s == "Bash(*)"));
        let lock = read_lock().expect("lock still present");
        assert_eq!(lock.holders.len(), 1);
        assert_eq!(lock.holders[0].pid, me);
    }

    #[test]
    fn legacy_holders_array_of_bare_pids_deserialises() {
        // Older Fleet builds wrote `holders` as `[1234, 5678]`. The
        // custom deserializer must accept that shape, then prune drops
        // them because start_time 0 never matches a real process.
        let legacy = r#"{
            "original_allow": [],
            "original_had_permissions": false,
            "original_existed": false,
            "holders": [1234, 5678]
        }"#;
        let mut lock: PermissionsLock = serde_json::from_str(legacy).unwrap();
        assert_eq!(lock.holders.len(), 2);
        assert_eq!(lock.holders[0].pid, 1234);
        assert_eq!(lock.holders[0].start_time_secs, 0);
        prune_dead_holders(&mut lock);
        assert!(
            lock.holders.is_empty(),
            "legacy holders must prune to empty under start_time check"
        );
    }

    #[test]
    fn prune_drops_holder_whose_pid_was_reused() {
        // Even with pid alive, a mismatched start_time must trigger prune
        // — defeating the OS-recycled-PID-into-a-Fleet-holder scenario.
        let my_pid = std::process::id();
        let real_start = crate::session::process_start_time(my_pid)
            .expect("self has a start_time");
        let mut lock = PermissionsLock {
            holders: vec![HolderEntry {
                pid: my_pid,
                start_time_secs: real_start.wrapping_add(1),
            }],
            ..Default::default()
        };
        prune_dead_holders(&mut lock);
        assert!(
            lock.holders.is_empty(),
            "holder with mismatched start_time must be pruned even though pid is alive"
        );
    }

    #[test]
    fn acquire_captures_start_time_in_holder_entry() {
        let _env = setup();
        let my_pid = std::process::id();
        let expected_start =
            crate::session::process_start_time(my_pid).expect("self has a start_time");
        acquire(my_pid).unwrap();
        let lock = read_lock().unwrap();
        let entry = lock
            .holders
            .iter()
            .find(|h| h.pid == my_pid)
            .expect("own pid must be in holders");
        assert_eq!(
            entry.start_time_secs, expected_start,
            "acquire must snapshot the live process start_time, not 0"
        );
    }
}
