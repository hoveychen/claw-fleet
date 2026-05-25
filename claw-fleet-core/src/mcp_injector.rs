//! Manages a Fleet-owned `mcpServers.fleet` entry in `~/.claude.json` so
//! Claude Code's agent sees the `fleet__ask` tool as soon as Fleet is
//! running, and the entry is removed cleanly the last time Fleet exits.
//!
//! Mirror of [`crate::permissions_injector`] (same multi-holder refcount
//! semantics, same `prune_dead_holders` self-heal on `kill -9`). The two
//! injectors are deliberately independent — Fleet's permission allowlist
//! lives in `~/.claude/settings.json`, while MCP server registration lives
//! in `~/.claude.json` (Claude Code's main user config), so they can't
//! share one lock file even though the lifecycle pattern is identical.
//!
//! Lifecycle:
//!
//! - `acquire(pid, fleet_path)` snapshots the user's pre-injection state
//!   on the very first holder, then writes
//!   `mcpServers.fleet = {"command": fleet_path, "args": ["mcp"]}` into
//!   `~/.claude.json`. Repeat acquisitions register additional pids without
//!   re-mutating the config.
//! - `release(pid)` removes the pid; when the last live holder leaves, the
//!   snapshot is consulted to restore `~/.claude.json` to its pre-Fleet
//!   shape (delete the `fleet` key entirely, or restore the user's prior
//!   value verbatim if one existed).
//! - `prune_dead_holders` runs inside both acquire/release, so a hard crash
//!   self-heals on the next Fleet boot.
//!
//! Lock file: `~/.fleet/mcp-lock.json`. Toggle config:
//! `~/.fleet/mcp-config.json` (mirror of `permissions-config.json`).

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::session::{is_process_alive, real_home_dir};

const LOCK_FILE_NAME: &str = "mcp-lock.json";
const CONFIG_FILE_NAME: &str = "mcp-config.json";
const CLAUDE_JSON_FILE: &str = ".claude.json";
/// Key under `mcpServers` that Fleet owns. Picking a stable name (rather
/// than e.g. `fleet-${pid}`) lets the restore step recognise our entry even
/// after a crash leaves a stale `mcp-lock.json` pointing at a dead holder.
pub const FLEET_SERVER_KEY: &str = "fleet";
/// Subcommand fleet binaries expose for the MCP stdio server.
pub const FLEET_MCP_ARG: &str = "mcp";

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct McpInjectorConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl Default for McpInjectorConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

fn fleet_dir() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".fleet"))
}

fn config_path() -> Option<PathBuf> {
    fleet_dir().map(|d| d.join(CONFIG_FILE_NAME))
}

fn lock_path() -> Option<PathBuf> {
    fleet_dir().map(|d| d.join(LOCK_FILE_NAME))
}

fn claude_json_path() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(CLAUDE_JSON_FILE))
}

pub fn load_config() -> McpInjectorConfig {
    let Some(p) = config_path() else {
        return McpInjectorConfig::default();
    };
    fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_config(cfg: &McpInjectorConfig) -> std::io::Result<()> {
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
pub struct McpLock {
    /// True when `mcpServers.fleet` existed in `~/.claude.json` before
    /// Fleet first injected its entry. If so, `original_fleet_entry` carries
    /// the original value so we can put it back on release.
    #[serde(default)]
    pub original_had_fleet: bool,
    /// Snapshot of the user's pre-existing `mcpServers.fleet` value, if any.
    /// `None` means the key didn't exist — restore deletes it.
    #[serde(default)]
    pub original_fleet_entry: Option<serde_json::Value>,
    /// Whether `mcpServers` existed as an object in `~/.claude.json` at
    /// first acquisition.
    #[serde(default)]
    pub original_had_mcp_servers: bool,
    /// Whether `~/.claude.json` existed on disk at first acquisition.
    #[serde(default)]
    pub original_existed: bool,
    /// Live Fleet process ids currently holding the injection.
    #[serde(default)]
    pub holders: Vec<u32>,
}

fn read_lock() -> Option<McpLock> {
    let p = lock_path()?;
    let s = fs::read_to_string(&p).ok()?;
    serde_json::from_str(&s).ok()
}

fn write_lock(lock: &McpLock) -> std::io::Result<()> {
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

fn read_claude_json() -> std::io::Result<(serde_json::Value, bool)> {
    let p = claude_json_path()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no home dir"))?;
    if !p.exists() {
        return Ok((serde_json::Value::Object(Default::default()), false));
    }
    let s = fs::read_to_string(&p)?;
    let v: serde_json::Value = serde_json::from_str(&s)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok((v, true))
}

fn write_claude_json(v: &serde_json::Value) -> std::io::Result<()> {
    let p = claude_json_path()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no home dir"))?;
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(v)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    fs::write(&p, json)
}

/// Build the Fleet MCP entry that we inject. Public so callers (e.g. tests
/// or a Settings UI) can reference the exact value Fleet writes.
pub fn build_fleet_entry(fleet_path: &str) -> serde_json::Value {
    serde_json::json!({
        "command": fleet_path,
        "args": [FLEET_MCP_ARG],
    })
}

fn extract_fleet_entry(v: &serde_json::Value) -> (Option<serde_json::Value>, bool, bool) {
    // Returns (fleet_entry, had_mcp_servers_object, json_existed_dummy).
    // Last bool is unused here — kept symmetric with the read_settings path
    // in permissions_injector for readability.
    let mcp = v.get("mcpServers");
    let had_mcp = mcp.map(|m| m.is_object()).unwrap_or(false);
    let fleet = mcp
        .and_then(|m| m.get(FLEET_SERVER_KEY))
        .cloned();
    (fleet, had_mcp, false)
}

fn set_fleet_entry(v: &mut serde_json::Value, entry: serde_json::Value) {
    if !v.is_object() {
        *v = serde_json::Value::Object(Default::default());
    }
    let obj = v.as_object_mut().expect("just ensured object");
    let mcp = obj
        .entry("mcpServers".to_string())
        .or_insert_with(|| serde_json::Value::Object(Default::default()));
    if !mcp.is_object() {
        *mcp = serde_json::Value::Object(Default::default());
    }
    mcp.as_object_mut()
        .expect("just ensured object")
        .insert(FLEET_SERVER_KEY.to_string(), entry);
}

fn remove_fleet_entry(v: &mut serde_json::Value) {
    let Some(obj) = v.as_object_mut() else { return };
    let Some(mcp) = obj.get_mut("mcpServers") else { return };
    let Some(mcp_obj) = mcp.as_object_mut() else { return };
    mcp_obj.remove(FLEET_SERVER_KEY);
    // If mcpServers is now an empty object AND it didn't exist before Fleet,
    // we strip it entirely. The caller (restore_from_snapshot) decides
    // whether to strip based on `original_had_mcp_servers`.
}

fn strip_mcp_servers(v: &mut serde_json::Value) {
    if let Some(obj) = v.as_object_mut() {
        obj.remove("mcpServers");
    }
}

/// Drop holder pids that no longer correspond to live processes.
pub fn prune_dead_holders(lock: &mut McpLock) {
    lock.holders.retain(|pid| is_process_alive(*pid));
}

/// Register `pid` as a holder, injecting `mcpServers.fleet` if this is the
/// first live holder. `fleet_path` is the absolute path to the fleet
/// binary (the one that exposes `fleet mcp`).
///
/// Idempotent: calling `acquire` repeatedly with the same pid is safe and
/// is a no-op after the first call (the existing entry is overwritten with
/// the same value, which Claude Code re-reads on next launch).
pub fn acquire(pid: u32, fleet_path: &str) -> std::io::Result<()> {
    let mut lock = read_lock().unwrap_or_default();
    prune_dead_holders(&mut lock);
    let was_empty = lock.holders.is_empty();

    if was_empty {
        let (claude_json, existed) = read_claude_json()?;
        let (orig_fleet, had_mcp, _) = extract_fleet_entry(&claude_json);
        lock.original_existed = existed;
        lock.original_had_mcp_servers = had_mcp;
        lock.original_had_fleet = orig_fleet.is_some();
        lock.original_fleet_entry = orig_fleet;

        let mut new_json = if existed {
            claude_json
        } else {
            serde_json::Value::Object(Default::default())
        };
        set_fleet_entry(&mut new_json, build_fleet_entry(fleet_path));
        write_claude_json(&new_json)?;
    }

    if !lock.holders.contains(&pid) {
        lock.holders.push(pid);
    }
    write_lock(&lock)?;
    Ok(())
}

/// Deregister `pid`. When the last live holder leaves, the pre-injection
/// state is restored from the snapshot.
///
/// Safe to call when no lock exists (returns `Ok(())`).
pub fn release(pid: u32) -> std::io::Result<()> {
    let Some(mut lock) = read_lock() else { return Ok(()) };
    prune_dead_holders(&mut lock);
    lock.holders.retain(|p| *p != pid);

    if !lock.holders.is_empty() {
        return write_lock(&lock);
    }

    restore_from_snapshot(&lock)?;
    delete_lock()?;
    Ok(())
}

fn restore_from_snapshot(lock: &McpLock) -> std::io::Result<()> {
    if !lock.original_existed {
        // ~/.claude.json didn't exist before Fleet touched it. Strip our
        // entry; if mcpServers is empty after, drop it too; if the resulting
        // file is an empty object, delete it.
        let (mut current, exists) = read_claude_json()?;
        if !exists {
            return Ok(());
        }
        remove_fleet_entry(&mut current);
        let mcp_empty = current
            .get("mcpServers")
            .and_then(|m| m.as_object())
            .map(|o| o.is_empty())
            .unwrap_or(false);
        if mcp_empty {
            strip_mcp_servers(&mut current);
        }
        let is_empty_obj = current.as_object().map(|o| o.is_empty()).unwrap_or(false);
        if is_empty_obj {
            // We created the file ourselves; remove it cleanly.
            if let Some(p) = claude_json_path() {
                if p.exists() {
                    fs::remove_file(p)?;
                }
            }
        } else {
            write_claude_json(&current)?;
        }
        return Ok(());
    }

    // ~/.claude.json existed originally. Restore the original `fleet` entry
    // (or remove it if there was none) without touching other mcpServers
    // entries or other top-level keys.
    let (mut current, exists) = read_claude_json()?;
    if !exists {
        // File was deleted while Fleet was running. Recreate it with just
        // the original snapshot's data — matches the "Fleet restores on
        // shutdown" contract better than silently swallowing the deletion.
        let mut fresh = serde_json::Value::Object(Default::default());
        if lock.original_had_mcp_servers {
            if let Some(entry) = &lock.original_fleet_entry {
                set_fleet_entry(&mut fresh, entry.clone());
            }
        }
        let is_empty_obj = fresh.as_object().map(|o| o.is_empty()).unwrap_or(true);
        if !is_empty_obj {
            write_claude_json(&fresh)?;
        }
        return Ok(());
    }

    if !lock.original_had_mcp_servers {
        // mcpServers didn't exist originally — strip the whole object after
        // removing our entry.
        remove_fleet_entry(&mut current);
        strip_mcp_servers(&mut current);
        write_claude_json(&current)?;
        return Ok(());
    }

    if let Some(entry) = &lock.original_fleet_entry {
        set_fleet_entry(&mut current, entry.clone());
    } else {
        remove_fleet_entry(&mut current);
    }
    write_claude_json(&current)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_home<F: FnOnce()>(f: F) {
        let _guard = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet-mcp-injector-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&tmp);
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialised by the fleet_home_lock.
        unsafe { std::env::set_var("FLEET_HOME", &tmp) };
        f();
        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn acquire_creates_claude_json_when_absent() {
        with_temp_home(|| {
            let p = claude_json_path().unwrap();
            assert!(!p.exists(), "test setup: claude.json must start absent");
            acquire(12345, "/usr/local/bin/fleet").unwrap();
            assert!(p.exists(), "acquire should create ~/.claude.json");
            let v: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
            assert_eq!(v["mcpServers"][FLEET_SERVER_KEY]["command"], "/usr/local/bin/fleet");
            assert_eq!(v["mcpServers"][FLEET_SERVER_KEY]["args"][0], FLEET_MCP_ARG);
        });
    }

    #[test]
    fn acquire_preserves_existing_unrelated_mcp_servers() {
        with_temp_home(|| {
            let p = claude_json_path().unwrap();
            let pre = serde_json::json!({
                "mcpServers": {
                    "filesystem": {"command": "fs-mcp", "args": []},
                    "github": {"command": "gh-mcp", "args": []}
                },
                "someOtherKey": 42
            });
            fs::write(&p, serde_json::to_string_pretty(&pre).unwrap()).unwrap();
            acquire(12345, "/bin/fleet").unwrap();
            let after: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
            assert_eq!(after["mcpServers"]["filesystem"]["command"], "fs-mcp");
            assert_eq!(after["mcpServers"]["github"]["command"], "gh-mcp");
            assert_eq!(after["mcpServers"][FLEET_SERVER_KEY]["command"], "/bin/fleet");
            assert_eq!(after["someOtherKey"], 42);
        });
    }

    #[test]
    fn release_restores_unrelated_entries_and_removes_fleet() {
        with_temp_home(|| {
            let p = claude_json_path().unwrap();
            let pre = serde_json::json!({
                "mcpServers": {
                    "filesystem": {"command": "fs-mcp", "args": []},
                }
            });
            fs::write(&p, serde_json::to_string_pretty(&pre).unwrap()).unwrap();
            acquire(42, "/bin/fleet").unwrap();
            release(42).unwrap();
            let after: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
            assert!(after["mcpServers"].get(FLEET_SERVER_KEY).is_none(),
                "fleet entry should be removed on last release");
            assert_eq!(after["mcpServers"]["filesystem"]["command"], "fs-mcp",
                "unrelated mcp servers must survive");
        });
    }

    #[test]
    fn release_deletes_claude_json_when_fleet_created_it() {
        with_temp_home(|| {
            let p = claude_json_path().unwrap();
            assert!(!p.exists());
            acquire(7, "/bin/fleet").unwrap();
            assert!(p.exists());
            release(7).unwrap();
            assert!(!p.exists(),
                "if Fleet created claude.json from scratch, release should delete it");
        });
    }

    #[test]
    fn release_restores_user_owned_fleet_entry_verbatim() {
        // Regression: if the user already had `mcpServers.fleet` (pointing at
        // a *different* binary, e.g. a custom build), Fleet must put exactly
        // that value back on release, not just remove it.
        with_temp_home(|| {
            let p = claude_json_path().unwrap();
            let pre = serde_json::json!({
                "mcpServers": {
                    "fleet": {"command": "/opt/custom/fleet", "args": ["mcp", "--debug"]}
                }
            });
            fs::write(&p, serde_json::to_string_pretty(&pre).unwrap()).unwrap();
            acquire(11, "/bin/fleet").unwrap();
            release(11).unwrap();
            let after: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
            assert_eq!(after["mcpServers"][FLEET_SERVER_KEY]["command"], "/opt/custom/fleet");
            assert_eq!(after["mcpServers"][FLEET_SERVER_KEY]["args"][1], "--debug");
        });
    }

    #[test]
    fn refcount_keeps_injection_until_last_holder_leaves() {
        // Use the current pid + a real subprocess pid so prune_dead_holders
        // (which calls is_process_alive) doesn't silently drop the "other
        // holder" mid-test and collapse the refcount.
        with_temp_home(|| {
            // Spawn a long-lived helper that we own for the duration of the test.
            let mut child = std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("spawn sleep child");
            let helper_pid = child.id();
            let my_pid = std::process::id();
            assert_ne!(helper_pid, my_pid, "subprocess pid must differ");

            let p = claude_json_path().unwrap();
            acquire(my_pid, "/bin/fleet").unwrap();
            acquire(helper_pid, "/bin/fleet").unwrap();

            // First release shouldn't strip — helper still alive.
            release(my_pid).unwrap();
            let mid: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
            assert_eq!(
                mid["mcpServers"][FLEET_SERVER_KEY]["command"], "/bin/fleet",
                "fleet entry must persist while another holder lives"
            );

            // Last release strips and deletes claude.json (which we created).
            release(helper_pid).unwrap();
            assert!(
                !p.exists(),
                "after last release, Fleet-created claude.json should be gone"
            );

            // Reap the helper.
            let _ = child.kill();
            let _ = child.wait();
        });
    }

    #[test]
    fn prune_dead_holders_removes_unknown_pids() {
        let mut lock = McpLock {
            holders: vec![1, 999_999_999_u32.min(u32::MAX), std::process::id()],
            ..Default::default()
        };
        prune_dead_holders(&mut lock);
        // The current process id must survive; the obviously-fake one must not.
        assert!(lock.holders.contains(&std::process::id()),
            "current pid should survive prune");
        assert!(!lock.holders.iter().any(|p| *p == 999_999_999_u32.min(u32::MAX)),
            "obviously-dead pid should be pruned");
    }

    #[test]
    fn double_acquire_same_pid_is_idempotent() {
        with_temp_home(|| {
            acquire(55, "/bin/fleet").unwrap();
            acquire(55, "/bin/fleet").unwrap();
            let lock = read_lock().unwrap();
            assert_eq!(lock.holders.iter().filter(|p| **p == 55).count(), 1,
                "same pid must not double-register");
        });
    }

    #[test]
    fn release_unknown_pid_when_no_lock_is_ok() {
        with_temp_home(|| {
            release(987654).unwrap();
        });
    }

    #[test]
    fn build_fleet_entry_shape() {
        let v = build_fleet_entry("/path/to/fleet");
        assert_eq!(v["command"], "/path/to/fleet");
        assert_eq!(v["args"][0], FLEET_MCP_ARG);
        // No extra keys — keeps the wire shape minimal so Claude Code's mcp
        // validator (whatever it ends up being) has the smallest surface.
        assert_eq!(v.as_object().unwrap().len(), 2);
    }
}
