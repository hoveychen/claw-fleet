//! Fleet's permission allow rules, and the way back out of the global
//! `~/.claude/settings.json` an older Fleet put them in.
//!
//! The rules ([`INJECT_RULES`]) make `fleet guard` the sole audit gate for the
//! sessions Fleet starts, so a command is not asked twice — once by Claude
//! Code's native prompt, once by Fleet. They now ride on each launch's
//! `--settings` file ([`crate::claude_launch`]); a `claude` the user opens by
//! hand keeps its native prompt.
//!
//! Earlier builds wrote them into the global `permissions.allow` instead, with
//! a lock file at `~/.fleet/permissions-lock.json`. [`deactivate`] takes them
//! back out; the one-off scope migration is its only caller.

use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use crate::session::{get_claude_dir, get_fleet_dir};

/// The full set of tool patterns Fleet injects into `permissions.allow`.
///
/// `Bash(*)` is the load-bearing one — it suppresses Claude Code's built-in
/// command prompt so `fleet guard` becomes the sole audit gate. `PowerShell(*)`
/// is its Windows sibling: Claude Code drives a separate `PowerShell` tool on
/// Windows without Git Bash (enabled automatically there), and its permission
/// rule namespace is `PowerShell(...)`, not `Bash(...)`. Without it a Windows
/// PowerShell command would hit Claude Code's native permission prompt — the
/// exact double-prompt / headless-stall this injector exists to remove. The
/// other
/// patterns smooth out incidental prompts the user already trusts Fleet to
/// orchestrate (file IO, web fetch, skills, monitoring, workflows). The two
/// `mcp__fleet__*` rules pre-authorise Fleet's own MCP tools so Claude Code
/// stops prompting on every invocation now that the desktop already renders +
/// audits them via the Decision Panel. Two families: the UI tools (`fleet__ask`
/// / `fleet__render_a2ui`) and the control tools registered for Fleet-owned
/// sessions — without the latter, an rca remote session would trade a Bash
/// `fleet` 127 for a per-call permission prompt. Plus the two image tools, which
/// are neither UI nor control and so fell through both rosters until 2026-09-09:
/// every `fleet__image_edit` call raised a permission card. The rules are kept
/// in sync with [`crate::mcp_control::CONTROL_TOOL_NAMES`] and
/// [`crate::mcp_server::ALWAYS_ON_TOOL_NAMES`] by
/// `inject_rules_preauthorise_every_advertised_mcp_tool`.
///
/// `fleet__control` is pre-authorised alongside the rest even though it is
/// destructive (it stops/interrupts other agents). The alternative is worse
/// rather than safer: a headless detached session has no prompt UI, so an
/// un-authorised call there stalls the session instead of refusing. The guard
/// against misuse lives in the tool itself — it refuses subagents, ambiguous
/// prefixes, and non-Fleet sessions for `interrupt` — not in a prompt nothing
/// can answer.
pub const INJECT_RULES: &[&str] = &[
    "Bash(*)",
    "PowerShell(*)",
    "Read(*)",
    "Write(*)",
    "Edit(*)",
    "WebFetch(*)",
    "WebSearch(*)",
    "Skill(*)",
    "Monitor(*)",
    "Workflow(*)",
    "mcp__fleet__fleet__ask",
    "mcp__fleet__fleet__render_a2ui",
    "mcp__fleet__fleet__set_session_title",
    "mcp__fleet__fleet__spawn",
    "mcp__fleet__fleet__plan",
    "mcp__fleet__fleet__handoff",
    "mcp__fleet__fleet__watch",
    "mcp__fleet__fleet__loop",
    "mcp__fleet__fleet__schedule",
    "mcp__fleet__fleet__wiki",
    "mcp__fleet__fleet__artifact",
    "mcp__fleet__fleet__inspect",
    "mcp__fleet__fleet__control",
    "mcp__fleet__fleet__notes",
    "mcp__fleet__fleet__history",
    "mcp__fleet__fleet__image",
    "mcp__fleet__fleet__image_edit",
];

const LOCK_FILE_NAME: &str = "permissions-lock.json";
const CLAUDE_SETTINGS_FILE: &str = "settings.json";

/// The part of an older Fleet's lock file [`deactivate`] still reads. The file
/// carried a snapshot and a holder list besides, which serde skips.
#[derive(Debug, Deserialize, Default)]
struct PermissionsLock {
    /// Whether settings.json existed before Fleet first wrote to it.
    #[serde(default)]
    original_existed: bool,
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
    let Some(p) = settings_path() else {
        return Ok(());
    };
    if p.exists() {
        fs::remove_file(p)?;
    }
    Ok(())
}

/// Take every rule Fleet injects back out of
/// `permissions.allow`, then drop the lock.
///
/// Strips by rule rather than restoring the lock's snapshot, because the
/// snapshot cannot be trusted on a long-lived host. An older build deleted the
/// lock on exit, so the next start re-snapshotted a settings.json that already
/// carried Fleet's rules — `original_allow` then lists `Bash(*)` and friends as
/// the user's own, and a restore keeps them forever. The cost is a user who had
/// written one of these exact rules by hand before installing Fleet: it goes
/// too. [`INJECT_RULES`] has held the same rule names since it was introduced,
/// so there is no older spelling to also strip.
///
/// Everything else under `permissions` (`deny`, `ask`, `additionalDirectories`,
/// the user's own allow rules) stays; the key itself goes only once empty. The
/// file is deleted only when Fleet created it and nothing else is left.
///
/// Works without a lock too — a host whose lock went missing still has the
/// rules on disk.
pub fn deactivate() -> std::io::Result<()> {
    let lock = read_lock();
    let (mut current, exists) = read_settings()?;
    if exists && strip_fleet_rules_in(&mut current) {
        let fleet_created_it = lock.as_ref().is_some_and(|l| !l.original_existed);
        let now_empty = current.as_object().is_some_and(|o| o.is_empty());
        if fleet_created_it && now_empty {
            delete_settings()?;
        } else {
            write_settings(&current)?;
        }
    }
    delete_lock()
}

/// Remove every [`INJECT_RULES`] entry from `permissions.allow`, dropping
/// `allow` and then `permissions` once they are empty. Returns whether
/// anything changed.
fn strip_fleet_rules_in(v: &mut serde_json::Value) -> bool {
    let Some(obj) = v.as_object_mut() else {
        return false;
    };
    let Some(perms) = obj.get_mut("permissions").and_then(|p| p.as_object_mut()) else {
        return false;
    };
    let Some(allow) = perms.get_mut("allow").and_then(|a| a.as_array_mut()) else {
        return false;
    };
    let before = allow.len();
    allow.retain(|r| !r.as_str().is_some_and(|s| INJECT_RULES.contains(&s)));
    if allow.len() == before {
        return false;
    }
    if allow.is_empty() {
        perms.remove("allow");
    }
    if perms.is_empty() {
        obj.remove("permissions");
    }
    true
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
        TestEnv {
            _tmp: tmp,
            _guard: guard,
        }
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
        v.pointer("/permissions/allow")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// What an older Fleet's first `acquire` left behind: its rules appended to
    /// `permissions.allow` and a lock recording whether settings.json existed.
    fn legacy_inject() {
        let (mut v, existed) = read_settings().unwrap();
        let mut allow = allow_of(&v);
        for rule in INJECT_RULES {
            if !allow.iter().any(|s| s == rule) {
                allow.push((*rule).to_string());
            }
        }
        v["permissions"]["allow"] = serde_json::json!(allow);
        write_settings(&v).unwrap();
        let lock = lock_path().unwrap();
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        fs::write(
            lock,
            serde_json::json!({ "original_existed": existed, "holders": [] }).to_string(),
        )
        .unwrap();
    }

    /// Regression: every MCP control tool `mcp_control` registers for
    /// Fleet-owned sessions must be pre-authorised in INJECT_RULES. Otherwise
    /// Claude Code prompts on every `mcp__fleet__fleet__plan` call — the exact
    /// permission-card popup a Fleet session hit after the control tools shipped
    /// but before they were added to the allow-list. Driven off
    /// `CONTROL_TOOL_NAMES` so adding a future control tool without an allow rule
    /// fails here rather than in production.
    #[test]
    fn inject_rules_preauthorise_every_advertised_mcp_tool() {
        // `fleet__permission_prompt` is the one tool the model never issues: the
        // harness invokes it via `--permission-prompt-tool` when some OTHER tool
        // lacks an allow rule. Pre-authorising it would be a no-op.
        const NOT_MODEL_INVOKED: [&str; 1] = ["fleet__permission_prompt"];

        let advertised = crate::mcp_control::CONTROL_TOOL_NAMES
            .iter()
            .chain(crate::mcp_server::ALWAYS_ON_TOOL_NAMES.iter());
        for name in advertised {
            if NOT_MODEL_INVOKED.contains(name) {
                continue;
            }
            let rule = format!("mcp__fleet__{name}");
            assert!(
                INJECT_RULES.contains(&rule.as_str()),
                "MCP tool {name} not pre-authorised (expected rule {rule} in INJECT_RULES) — \
                 every call would raise a permission card"
            );
        }
    }

    /// Same contract for the always-on tool the session-title guidance tells
    /// agents to call: guidance that names a tool without an allow rule turns
    /// every self-titling session into a permission card the user has to clear.
    /// Driven off the rendered guidance rather than a literal so deleting the
    /// rule fails here.
    #[test]
    fn inject_rules_preauthorise_the_tool_the_session_title_guidance_names() {
        let guidance = crate::session_title_guidance::render_guidance("Boss", "en");
        assert!(
            guidance.contains("fleet__set_session_title"),
            "guidance no longer names the tool — update this test with it"
        );
        assert!(
            INJECT_RULES.contains(&"mcp__fleet__fleet__set_session_title"),
            "session-title guidance asks agents to call a tool that is not pre-authorised"
        );
    }

    /// Regression: the Windows `PowerShell` tool has its own permission-rule
    /// namespace (`PowerShell(...)`), so `Bash(*)` alone does not pre-authorise
    /// it. Dropping `PowerShell(*)` would make a Windows-without-Git-Bash
    /// session hit Claude Code's native prompt on every command — or stall a
    /// detached headless session that has no prompt UI to answer it.
    #[test]
    fn inject_rules_preauthorise_powershell_tool() {
        assert!(
            INJECT_RULES.contains(&"PowerShell(*)"),
            "PowerShell(*) must be injected so the Windows PowerShell tool is pre-authorised"
        );
    }

    #[test]
    fn deactivate_restores_original() {
        let _env = setup();
        write_settings_for_test(&serde_json::json!({
            "permissions": { "allow": ["Bash(npm run:*)"] },
            "theme": "dark",
        }));
        legacy_inject();
        deactivate().unwrap();
        let v = read_settings_for_test().expect("file preserved");
        assert_eq!(allow_of(&v), vec!["Bash(npm run:*)"]);
        assert_eq!(v.get("theme").and_then(|t| t.as_str()), Some("dark"));
        assert!(read_lock().is_none(), "lock file removed");
    }

    #[test]
    fn deactivate_with_no_original_file_deletes_settings() {
        let _env = setup();
        legacy_inject();
        deactivate().unwrap();
        assert!(read_settings_for_test().is_none());
        assert!(read_lock().is_none());
    }

    #[test]
    fn deactivate_with_original_no_permissions_strips_block() {
        let _env = setup();
        write_settings_for_test(&serde_json::json!({ "theme": "dark" }));
        legacy_inject();
        deactivate().unwrap();
        let v = read_settings_for_test().expect("file preserved");
        assert!(v.get("permissions").is_none());
        assert_eq!(v.get("theme").and_then(|t| t.as_str()), Some("dark"));
    }

    #[test]
    fn deactivate_no_lock_leaves_a_clean_file_alone() {
        let _env = setup();
        write_settings_for_test(&serde_json::json!({ "theme": "dark" }));
        deactivate().unwrap();
        let v = read_settings_for_test().expect("untouched");
        assert_eq!(v, serde_json::json!({ "theme": "dark" }));
    }

    /// A lost lock used to make deactivate a no-op, stranding the rules.
    #[test]
    fn deactivate_without_a_lock_still_strips_fleet_rules() {
        let _env = setup();
        write_settings_for_test(&serde_json::json!({
            "permissions": { "allow": ["Bash(*)", "Bash(ls)", "mcp__fleet__fleet__ask"] },
        }));
        deactivate().unwrap();
        assert_eq!(allow_of(&read_settings_for_test().unwrap()), vec!["Bash(ls)"]);
    }

    /// With no `permissions` before Fleet, restoring used to drop the whole
    /// object — taking `deny` and `additionalDirectories` the user added since.
    #[test]
    fn deactivate_keeps_other_permission_keys_added_since() {
        let _env = setup();
        write_settings_for_test(&serde_json::json!({ "theme": "dark" }));
        legacy_inject();
        let mut v = read_settings_for_test().unwrap();
        v["permissions"]["deny"] = serde_json::json!(["Bash(rm -rf:*)"]);
        v["permissions"]["additionalDirectories"] = serde_json::json!(["/tmp"]);
        write_settings_for_test(&v);

        deactivate().unwrap();
        assert_eq!(
            read_settings_for_test().unwrap(),
            serde_json::json!({
                "theme": "dark",
                "permissions": {
                    "deny": ["Bash(rm -rf:*)"],
                    "additionalDirectories": ["/tmp"],
                },
            })
        );
    }

    /// The snapshot on a long-lived host lists Fleet's own rules as the
    /// user's (taken after an older build's injection); they must go anyway.
    #[test]
    fn deactivate_strips_fleet_rules_the_snapshot_claims_as_original() {
        let _env = setup();
        write_settings_for_test(&serde_json::json!({
            "permissions": { "allow": ["Bash(*)", "Read(*)", "Bash(ls)", "mcp__fleet__fleet__ask"] },
        }));
        legacy_inject();

        deactivate().unwrap();
        assert_eq!(allow_of(&read_settings_for_test().unwrap()), vec!["Bash(ls)"]);
        assert!(read_lock().is_none());
    }

    #[test]
    fn deactivate_preserves_user_entries_added_mid_run() {
        let _env = setup();
        write_settings_for_test(&serde_json::json!({
            "permissions": { "allow": ["Bash(npm run:*)"] },
        }));
        legacy_inject();

        // User adds an entry while Fleet is running.
        let mut v = read_settings_for_test().unwrap();
        let mut allow = allow_of(&v);
        allow.push("Read(./secrets/*)".to_string());
        v["permissions"]["allow"] = serde_json::json!(allow);
        write_settings_for_test(&v);

        deactivate().unwrap();
        let allow = allow_of(&read_settings_for_test().unwrap());
        assert!(
            allow.contains(&"Bash(npm run:*)".to_string()),
            "original kept"
        );
        assert!(
            allow.contains(&"Read(./secrets/*)".to_string()),
            "user addition kept"
        );
        // Fleet's injected rules should be gone.
        assert!(!allow.contains(&"WebFetch(*)".to_string()));
    }
}
