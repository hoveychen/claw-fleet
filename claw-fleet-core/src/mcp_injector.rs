//! The `fleet` MCP server entry: its shape, and the way back out of the
//! global `~/.claude.json` an older Fleet registered it in.
//!
//! Fleet sessions get the server from their launch's `--mcp-config` file
//! ([`crate::claude_launch`]), under the same `fleet` key so tool ids stay
//! `mcp__fleet__fleet__*`. Earlier builds wrote it into `~/.claude.json`, with a
//! lock at `~/.fleet/mcp-lock.json`; [`strip_fleet_server`] takes both out, and
//! [`registered_fleet_entry`] reports one that is still there.

use std::fs;
use std::path::{Path, PathBuf};

use crate::session::real_home_dir;

const LOCK_FILE_NAME: &str = "mcp-lock.json";
/// Key under `mcpServers` that Fleet owns.
pub const FLEET_SERVER_KEY: &str = "fleet";
/// Subcommand fleet binaries expose for the MCP stdio server.
pub const FLEET_MCP_ARG: &str = "mcp";

fn fleet_dir() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".fleet"))
}

fn lock_path() -> Option<PathBuf> {
    fleet_dir().map(|d| d.join(LOCK_FILE_NAME))
}

fn claude_json_path() -> Option<PathBuf> {
    crate::session::get_claude_config_json()
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

/// The `fleet` server entry a launch's `--mcp-config` carries.
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
/// resolves.
pub fn registered_fleet_entry() -> Option<serde_json::Value> {
    let path = claude_json_path()?;
    let content = std::fs::read_to_string(&path).ok()?;
    let v = serde_json::from_str::<serde_json::Value>(&content).ok()?;
    let entry = extract_fleet_entry(&v)?;
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
    let bare_name = path
        .parent()
        .map(|p| p.as_os_str().is_empty())
        .unwrap_or(true);
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

fn extract_fleet_entry(v: &serde_json::Value) -> Option<serde_json::Value> {
    v.get("mcpServers")
        .and_then(|m| m.get(FLEET_SERVER_KEY))
        .cloned()
}

fn remove_fleet_entry(v: &mut serde_json::Value) {
    let Some(obj) = v.as_object_mut() else { return };
    let Some(mcp) = obj.get_mut("mcpServers") else {
        return;
    };
    let Some(mcp_obj) = mcp.as_object_mut() else {
        return;
    };
    mcp_obj.remove(FLEET_SERVER_KEY);
}

fn strip_mcp_servers(v: &mut serde_json::Value) {
    if let Some(obj) = v.as_object_mut() {
        obj.remove("mcpServers");
    }
}

/// Whether `entry` is the server Fleet registers: a `fleet` binary run as
/// `fleet mcp`. A same-named server pointing anywhere else is the user's.
fn is_fleets_own_entry(entry: &serde_json::Value) -> bool {
    let base = entry
        .get("command")
        .and_then(|c| c.as_str())
        .map(|c| c.rsplit(['/', '\\']).next().unwrap_or(c).to_ascii_lowercase());
    let args_are_mcp = entry
        .get("args")
        .and_then(|a| a.as_array())
        .is_some_and(|a| a.len() == 1 && a[0] == FLEET_MCP_ARG);
    matches!(base.as_deref(), Some("fleet" | "fleet.exe")) && args_are_mcp
}

/// Take Fleet's `mcpServers.fleet` out of `~/.claude.json` for good and drop
/// the injector lock — Fleet sessions get the server from `--mcp-config` now.
///
/// Deliberately ignores the lock's snapshot: on hosts where a lock was ever
/// recreated after an earlier injection, its `original_fleet_entry` *is*
/// Fleet's own entry, and restoring it would put the server straight back.
/// Returns whether the file changed.
pub(crate) fn strip_fleet_server() -> std::io::Result<bool> {
    let (mut current, exists) = read_claude_json()?;
    let ours = exists
        && extract_fleet_entry(&current)
            .as_ref()
            .is_some_and(is_fleets_own_entry);
    if ours {
        remove_fleet_entry(&mut current);
        let mcp_empty = current
            .get("mcpServers")
            .and_then(|m| m.as_object())
            .is_some_and(|o| o.is_empty());
        if mcp_empty {
            strip_mcp_servers(&mut current);
        }
        write_claude_json(&current)?;
    }
    delete_lock()?;
    Ok(ours)
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
