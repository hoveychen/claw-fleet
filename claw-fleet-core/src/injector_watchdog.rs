//! Periodic re-injection watchdog for [`crate::mcp_injector`] and
//! [`crate::permissions_injector`].
//!
//! Background: each injector holds its desired state in a lock file and
//! writes the corresponding section of `~/.claude.json` (mcp) or
//! `~/.claude/settings.json` (permissions) on first `acquire`. But that
//! one-shot write loses the race against anything that mutates those
//! files mid-run — for example, a Claude Code upgrade rewriting
//! `~/.claude.json` wholesale during a `brew upgrade`, or another tool
//! editing `settings.json`. Once the file drifts, Fleet's `release`
//! flow is the only thing that touches it again, so the user's MCP /
//! permission state can stay broken for the rest of the session.
//!
//! The watchdog tightens that loop: every 30 seconds, while at least
//! one live holder still owns each lock, the watchdog calls each
//! injector's `verify_and_reinject` to put the expected state back if
//! drift is detected. The user-visible toggle on each injector
//! (`load_config().enabled`) is honoured — a disabled injector is left
//! alone.
//!
//! Lifecycle: callers `start(fleet_path)` once during process startup
//! after their own `acquire`. The thread runs until the process exits;
//! there is no `stop` API because the watchdog has nothing to flush and
//! the OS reaps the thread cleanly at exit. Tests poke [`tick`] directly
//! instead of waiting on the timer.

use std::time::Duration;

/// Interval between drift checks. 30 seconds matches the watchdog cadence
/// chosen in the PRD; not so frequent that we burn IO during normal
/// operation, not so slow that a drift event sits unresolved for the
/// duration of an interactive Claude Code session.
pub const TICK_INTERVAL_SECS: u64 = 30;

/// Spawn the watchdog thread. `fleet_path` is the absolute path to the
/// fleet binary we expect `mcpServers.fleet.command` to point at — same
/// value the caller passed to [`crate::mcp_injector::acquire`]. The
/// thread is left running for the process's lifetime; the OS reaps it
/// at exit.
pub fn start(fleet_path: String) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(TICK_INTERVAL_SECS));
        tick(&fleet_path);
    });
}

/// One pass of the verify-and-reinject logic across both injectors.
/// Public so tests can invoke it deterministically without spinning up
/// the timer thread.
pub fn tick(fleet_path: &str) {
    if crate::mcp_injector::load_config().enabled {
        match crate::mcp_injector::verify_and_reinject(fleet_path) {
            Ok(true) => eprintln!(
                "[injector-watchdog] ~/.claude.json drifted — re-injected mcpServers.fleet"
            ),
            Ok(false) => {}
            Err(e) => eprintln!("[injector-watchdog] mcp_injector verify failed: {e}"),
        }
    }
    if crate::permissions_injector::load_config().enabled {
        match crate::permissions_injector::verify_and_reinject() {
            Ok(true) => eprintln!(
                "[injector-watchdog] ~/.claude/settings.json drifted — re-injected permissions.allow"
            ),
            Ok(false) => {}
            Err(e) => eprintln!("[injector-watchdog] permissions_injector verify failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::mcp_injector;
    use crate::permissions_injector;
    use crate::session::fleet_home_lock;
    use std::fs;

    fn with_temp_home<F: FnOnce()>(f: F) {
        let _guard = fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet-watchdog-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&tmp);
        // FLEET_HOME shadows the real user home for *both* injectors —
        // claude_dir (~/.claude) and fleet_dir (~/.fleet) both resolve via
        // real_home_dir(), so one env var is enough.
        let prev_fleet = std::env::var_os("FLEET_HOME");
        // SAFETY: serialised by fleet_home_lock.
        unsafe { std::env::set_var("FLEET_HOME", &tmp) };
        f();
        unsafe {
            match prev_fleet {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    fn claude_json_path() -> std::path::PathBuf {
        crate::session::real_home_dir().unwrap().join(".claude.json")
    }

    fn settings_json_path() -> std::path::PathBuf {
        crate::session::get_claude_dir().unwrap().join("settings.json")
    }

    #[test]
    fn mcp_verify_reinjects_when_claude_json_lost_fleet_entry() {
        with_temp_home(|| {
            mcp_injector::acquire(std::process::id(), "/bin/fleet").unwrap();
            // Simulate Claude Code overwriting ~/.claude.json from scratch
            // (e.g. during an upgrade): the file now has no mcpServers
            // section at all.
            fs::write(
                claude_json_path(),
                serde_json::to_string_pretty(&serde_json::json!({
                    "someOtherKey": 1
                }))
                .unwrap(),
            )
            .unwrap();

            let injected = mcp_injector::verify_and_reinject("/bin/fleet").unwrap();
            assert!(injected, "drift should be detected and repaired");

            let v: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(claude_json_path()).unwrap()).unwrap();
            assert_eq!(
                v["mcpServers"][mcp_injector::FLEET_SERVER_KEY]["command"],
                "/bin/fleet"
            );
            assert_eq!(v["someOtherKey"], 1, "unrelated keys must survive");

            let _ = mcp_injector::release(std::process::id());
        });
    }

    #[test]
    fn mcp_verify_no_op_when_entry_already_correct() {
        with_temp_home(|| {
            mcp_injector::acquire(std::process::id(), "/bin/fleet").unwrap();
            let before = fs::read_to_string(claude_json_path()).unwrap();
            let injected = mcp_injector::verify_and_reinject("/bin/fleet").unwrap();
            assert!(!injected, "no drift → no re-write");
            let after = fs::read_to_string(claude_json_path()).unwrap();
            assert_eq!(before, after, "file must be untouched when correct");
            let _ = mcp_injector::release(std::process::id());
        });
    }

    #[test]
    fn mcp_verify_no_op_when_no_live_holders() {
        with_temp_home(|| {
            // Seed a lock whose only holder is a fake dead pid — prune
            // empties holders, so verify must NOT re-inject.
            let lock_dir = crate::session::get_fleet_dir().unwrap();
            fs::create_dir_all(&lock_dir).unwrap();
            fs::write(
                lock_dir.join("mcp-lock.json"),
                r#"{"original_existed": true, "holders": [999999999]}"#,
            )
            .unwrap();
            // ~/.claude.json doesn't have our entry; if verify ran, we'd
            // see a write. Assert no write happens.
            assert!(!claude_json_path().exists());
            let injected = mcp_injector::verify_and_reinject("/bin/fleet").unwrap();
            assert!(!injected, "no live holders → must not touch file");
            assert!(!claude_json_path().exists(), "file must stay absent");
        });
    }

    #[test]
    fn permissions_verify_reinjects_when_rule_missing() {
        with_temp_home(|| {
            permissions_injector::acquire(std::process::id()).unwrap();
            // Drop the canonical rule someone external would strip.
            let mut v: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(settings_json_path()).unwrap()).unwrap();
            let allow = v
                .get_mut("permissions")
                .and_then(|p| p.get_mut("allow"))
                .and_then(|a| a.as_array_mut())
                .unwrap();
            allow.retain(|s| s.as_str() != Some("Bash(*)"));
            fs::write(
                settings_json_path(),
                serde_json::to_string_pretty(&v).unwrap(),
            )
            .unwrap();

            let injected = permissions_injector::verify_and_reinject().unwrap();
            assert!(injected, "missing rule should trigger re-inject");

            let after: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(settings_json_path()).unwrap()).unwrap();
            let restored: Vec<String> = after["permissions"]["allow"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect();
            assert!(restored.iter().any(|s| s == "Bash(*)"), "Bash(*) re-injected");
            let _ = permissions_injector::release(std::process::id());
        });
    }

    #[test]
    fn watchdog_tick_runs_both_injectors() {
        // End-to-end: drift both files at once, call `tick` directly
        // (rather than relying on the timer thread), assert both are
        // repaired in a single pass.
        with_temp_home(|| {
            mcp_injector::acquire(std::process::id(), "/bin/fleet").unwrap();
            permissions_injector::acquire(std::process::id()).unwrap();

            // Drift ~/.claude.json
            fs::write(
                claude_json_path(),
                serde_json::to_string_pretty(&serde_json::json!({})).unwrap(),
            )
            .unwrap();
            // Drift ~/.claude/settings.json
            fs::write(
                settings_json_path(),
                serde_json::to_string_pretty(&serde_json::json!({})).unwrap(),
            )
            .unwrap();

            super::tick("/bin/fleet");

            let claude_after: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(claude_json_path()).unwrap()).unwrap();
            assert_eq!(
                claude_after["mcpServers"][mcp_injector::FLEET_SERVER_KEY]["command"],
                "/bin/fleet"
            );

            let settings_after: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(settings_json_path()).unwrap()).unwrap();
            let allow: Vec<String> = settings_after["permissions"]["allow"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect();
            for rule in permissions_injector::INJECT_RULES {
                assert!(allow.iter().any(|s| s == rule), "missing {rule} after tick");
            }

            let _ = mcp_injector::release(std::process::id());
            let _ = permissions_injector::release(std::process::id());
        });
    }
}
