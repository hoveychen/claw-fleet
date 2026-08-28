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
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::session::{
    deserialize_holders, prune_dead_holders as prune_holder_entries, real_home_dir, HolderEntry,
};

const LOCK_FILE_NAME: &str = "mcp-lock.json";
const CONFIG_FILE_NAME: &str = "mcp-config.json";
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
    crate::session::get_claude_config_json()
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
    /// Live Fleet process holders currently keeping the injection in place.
    #[serde(default, deserialize_with = "deserialize_holders")]
    pub holders: Vec<HolderEntry>,
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

/// True when `mcpServers.fleet` is currently registered in `~/.claude.json`
/// **and its `command` can actually be launched** — i.e. a spawned `claude`
/// child will see the `fleet` MCP server and its tools. Spawn sites use this
/// to decide whether they can safely pass `--permission-prompt-tool
/// mcp__fleet__fleet__permission_prompt`: naming a tool that doesn't resolve
/// makes the CLI abort at startup, so the flag must only be added when the
/// server is actually usable.
pub fn fleet_server_registered() -> bool {
    registered_fleet_entry().is_some()
}

/// The `mcpServers.fleet` entry as currently registered in `~/.claude.json`,
/// or `None` when the injection isn't live *or* its `command` no longer
/// resolves. Chat sessions exclude the user setting source (which is where the
/// CLI would otherwise find this server), so they need to hand the same entry
/// back through `--mcp-config` — see `crate::chat_workspace::chat_session_args`.
pub fn registered_fleet_entry() -> Option<serde_json::Value> {
    let path = claude_json_path()?;
    let content = std::fs::read_to_string(&path).ok()?;
    let v = serde_json::from_str::<serde_json::Value>(&content).ok()?;
    let entry = extract_fleet_entry(&v).0?;
    entry_command_is_live(&entry).then_some(entry)
}

/// Whether `entry`'s `command` names something a spawned `claude` can still
/// execute.
///
/// The presence of the key is not enough. A dev `fleet-cli` running out of a
/// git worktree publishes its own absolute path here, and Rule 3 deletes that
/// worktree the moment its plan merges — leaving a registration that points at
/// nothing. Every consumer of this entry (the `--permission-prompt-tool` flag,
/// the chat workspace's `--mcp-config`) then hands `claude` a server it cannot
/// start, which is a hard error per tool call rather than a missing feature.
///
/// A bare name with no path separator (the watchdog's `"fleet"` fallback) is
/// resolved from `PATH` at launch, which we can't check cheaply and which isn't
/// the failure mode this guards — accept it.
fn entry_command_is_live(entry: &serde_json::Value) -> bool {
    let Some(cmd) = entry.get("command").and_then(|c| c.as_str()) else {
        return false;
    };
    if cmd.is_empty() {
        return false;
    }
    let path = Path::new(cmd);
    let bare_name = path.parent().map(|p| p.as_os_str().is_empty()).unwrap_or(true);
    bare_name || is_executable_file(path)
}

/// `path` exists (following symlinks) and is a file with an execute bit.
/// Windows has no execute bit, so existence as a file is the whole test there.
fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
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

/// Thin re-export of [`crate::session::prune_dead_holders`] so callers
/// inside this module can keep using the historic local name. The shared
/// helper checks both `is_process_alive(pid)` and a `start_time_secs`
/// match, defeating PID reuse (see memory `project_mcp_injector_pid_reuse`).
pub fn prune_dead_holders(lock: &mut McpLock) {
    prune_holder_entries(&mut lock.holders);
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

    if !lock.holders.iter().any(|h| h.pid == pid) {
        lock.holders.push(HolderEntry::capture(pid));
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
    lock.holders.retain(|h| h.pid != pid);

    if !lock.holders.is_empty() {
        return write_lock(&lock);
    }

    restore_from_snapshot(&lock)?;
    delete_lock()?;
    Ok(())
}

/// Watchdog hook: if the lock still has at least one live holder but
/// `~/.claude.json` no longer carries our `mcpServers.fleet` entry (e.g.
/// Claude Code upgraded and rewrote the file from scratch), re-write
/// it. The snapshot already stored in the lock is preserved — we are
/// not re-acquiring, just restoring our injection on top of whatever
/// drift happened.
///
/// Returns `Ok(true)` if a re-injection actually wrote the file,
/// `Ok(false)` if nothing was off. Used by [`crate::injector_watchdog`].
pub fn verify_and_reinject(fleet_path: &str) -> std::io::Result<bool> {
    let Some(mut lock) = read_lock() else { return Ok(false) };
    prune_dead_holders(&mut lock);
    if lock.holders.is_empty() {
        // No live holders → nothing to enforce. Don't touch the file.
        return Ok(false);
    }
    let (claude_json, existed) = read_claude_json()?;
    let (current_fleet, _, _) = extract_fleet_entry(&claude_json);
    let expected = build_fleet_entry(fleet_path);
    if current_fleet.as_ref() == Some(&expected) {
        return Ok(false);
    }
    let mut new_json = if existed {
        claude_json
    } else {
        serde_json::Value::Object(Default::default())
    };
    set_fleet_entry(&mut new_json, expected);
    write_claude_json(&new_json)?;
    write_lock(&lock)?;
    Ok(true)
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
    use crate::session::process_start_time;

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
        // A current-process holder captured with the matching start_time
        // must survive; a fake pid with start_time 0 must be pruned because
        // start_time 0 never matches a real live process.
        let mut lock = McpLock {
            holders: vec![
                HolderEntry { pid: 999_999_999_u32.min(u32::MAX), start_time_secs: 0 },
                HolderEntry::capture(std::process::id()),
            ],
            ..Default::default()
        };
        prune_dead_holders(&mut lock);
        assert!(
            lock.holders.iter().any(|h| h.pid == std::process::id()),
            "current pid (captured with real start_time) should survive prune"
        );
        assert!(
            !lock.holders.iter().any(|h| h.pid == 999_999_999_u32.min(u32::MAX)),
            "obviously-dead pid should be pruned"
        );
    }

    #[test]
    fn double_acquire_same_pid_is_idempotent() {
        with_temp_home(|| {
            acquire(55, "/bin/fleet").unwrap();
            acquire(55, "/bin/fleet").unwrap();
            let lock = read_lock().unwrap();
            assert_eq!(lock.holders.iter().filter(|h| h.pid == 55).count(), 1,
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
    fn legacy_holders_array_of_bare_pids_deserialises() {
        // Older Fleet builds wrote `holders` as `[1234, 5678]` (a Vec<u32>).
        // The custom deserializer must accept that shape with start_time=0
        // so the next `prune_dead_holders` cleans them out without panic.
        let legacy = r#"{
            "original_had_fleet": false,
            "original_fleet_entry": null,
            "original_had_mcp_servers": false,
            "original_existed": false,
            "holders": [1234, 5678]
        }"#;
        let mut lock: McpLock = serde_json::from_str(legacy).unwrap();
        assert_eq!(lock.holders.len(), 2);
        assert_eq!(lock.holders[0].pid, 1234);
        assert_eq!(lock.holders[0].start_time_secs, 0);
        assert_eq!(lock.holders[1].pid, 5678);
        assert_eq!(lock.holders[1].start_time_secs, 0);
        // Pruning a legacy lock should drop everything: start_time 0
        // never matches any real live process.
        prune_dead_holders(&mut lock);
        assert!(lock.holders.is_empty(), "legacy holders must prune to empty");
    }

    #[test]
    fn prune_drops_holder_whose_pid_was_reused() {
        // Simulate PID reuse: lock claims a holder at the *current* pid
        // but with a stale start_time_secs that doesn't match. Even though
        // is_process_alive(pid) is true, the start_time mismatch must cause
        // prune_dead_holders to drop the entry.
        let my_pid = std::process::id();
        let real_start = process_start_time(my_pid).expect("self has a start_time");
        let mut lock = McpLock {
            holders: vec![HolderEntry {
                pid: my_pid,
                start_time_secs: real_start.wrapping_add(1), // intentionally wrong
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
        with_temp_home(|| {
            let my_pid = std::process::id();
            let expected_start = process_start_time(my_pid).expect("self has a start_time");
            acquire(my_pid, "/bin/fleet").unwrap();
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
        });
    }

    #[test]
    fn legacy_lock_on_disk_self_heals_via_next_acquire() {
        // End-to-end legacy-migration check: drop a legacy-shape lock on
        // disk, then call acquire. The legacy holder (start_time 0) must
        // be pruned and our acquisition must re-snapshot the original
        // ~/.claude.json — i.e. acquire goes through the "was_empty" branch.
        with_temp_home(|| {
            let claude_p = claude_json_path().unwrap();
            let pre = serde_json::json!({
                "mcpServers": {
                    "filesystem": {"command": "fs-mcp", "args": []}
                }
            });
            fs::write(&claude_p, serde_json::to_string_pretty(&pre).unwrap()).unwrap();

            // Seed a legacy lock that thinks some dead pid (no live process
            // has start_time 0) still holds the injection — but no inject
            // has actually been written to ~/.claude.json, so the snapshot
            // also lies. After a self-healing acquire, the snapshot should
            // reflect the *real* current ~/.claude.json, and Fleet's entry
            // should be present.
            let legacy_raw = r#"{
                "original_had_fleet": true,
                "original_fleet_entry": {"command": "/wrong/path", "args": ["mcp"]},
                "original_had_mcp_servers": true,
                "original_existed": true,
                "holders": [9991, 9992]
            }"#;
            let lock_p = lock_path().unwrap();
            fs::create_dir_all(lock_p.parent().unwrap()).unwrap();
            fs::write(&lock_p, legacy_raw).unwrap();

            let my_pid = std::process::id();
            acquire(my_pid, "/bin/fleet").unwrap();

            // Snapshot must have been rebuilt from the actual claude.json.
            let lock = read_lock().unwrap();
            assert!(
                lock.holders.iter().any(|h| h.pid == my_pid),
                "current pid registered"
            );
            assert!(
                !lock.holders.iter().any(|h| h.pid == 9991 || h.pid == 9992),
                "legacy holders pruned"
            );
            assert!(
                lock.original_fleet_entry.is_none(),
                "snapshot rebuilt — original claude.json had no fleet entry"
            );

            // And the file has Fleet's entry alongside the unrelated one.
            let after: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&claude_p).unwrap()).unwrap();
            assert_eq!(after["mcpServers"][FLEET_SERVER_KEY]["command"], "/bin/fleet");
            assert_eq!(after["mcpServers"]["filesystem"]["command"], "fs-mcp");

            // Last release restores fully.
            release(my_pid).unwrap();
            let restored: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&claude_p).unwrap()).unwrap();
            assert!(restored["mcpServers"].get(FLEET_SERVER_KEY).is_none());
            assert_eq!(restored["mcpServers"]["filesystem"]["command"], "fs-mcp");
        });
    }

    /// Write `~/.claude.json` with a `mcpServers.fleet` entry naming `command`.
    fn seed_registration(command: &str) {
        let p = claude_json_path().unwrap();
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        let v = serde_json::json!({
            "mcpServers": { FLEET_SERVER_KEY: build_fleet_entry(command) },
        });
        fs::write(&p, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    }

    /// Regression (2026-08-27): a debug `fleet-cli` running out of a git
    /// worktree published its own path as `mcpServers.fleet.command`; the
    /// worktree was then deleted by its own merge, so the registration pointed
    /// at a binary that no longer existed. `fleet_server_registered` only ever
    /// asked "is the key there?", so it still answered yes, and every spawn
    /// kept passing `--permission-prompt-tool
    /// mcp__fleet__fleet__permission_prompt`. The CLI could not start the
    /// server, so every tool call that needed a permission decision died with
    /// `MCP tool ... not found. Available MCP tools: none` instead of falling
    /// back to no bridge at all.
    #[test]
    fn registered_is_false_when_command_binary_is_missing() {
        with_temp_home(|| {
            seed_registration("/nonexistent/.worktrees/gone/target/debug/fleet-cli");
            assert!(
                !fleet_server_registered(),
                "a registration whose command is gone must not count as registered"
            );
        });
    }

    /// Same defect, other consumer: `chat_workspace::write_chat_mcp_config`
    /// copies this entry verbatim into `~/.fleet/chat-mcp.json` and hands it to
    /// `claude --mcp-config`, so a dead command has to be withheld here too.
    #[test]
    fn registered_entry_is_none_when_command_binary_is_missing() {
        with_temp_home(|| {
            seed_registration("/nonexistent/.worktrees/gone/target/debug/fleet-cli");
            assert!(
                registered_fleet_entry().is_none(),
                "a registration whose command is gone must not be handed to --mcp-config"
            );
        });
    }

    /// The happy path must keep working: a command that exists and is
    /// executable still registers. `current_exe` is the test binary itself —
    /// present and executable on every platform the suite runs on.
    #[test]
    fn registered_is_true_for_a_live_executable() {
        with_temp_home(|| {
            let me = std::env::current_exe().unwrap();
            seed_registration(&me.to_string_lossy());
            assert!(fleet_server_registered(), "a live executable must register");
            assert!(registered_fleet_entry().is_some());
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
