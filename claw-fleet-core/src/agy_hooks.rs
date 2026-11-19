//! Fleet's carriers into Antigravity CLI (`agy`, Beta): one named hook in
//! `~/.gemini/config/hooks.json` and one MCP server in
//! `~/.gemini/config/mcp_config.json`.
//!
//! Where the formats come from: agy ships its own docs inside the binary
//! (`antigravity` 1.5.12, read by byte search on 2026-11-19). Neither format has
//! been exercised against a real agy session — the only account on hand fails
//! agy's eligibility check before a conversation starts — so everything below
//! is written to the documented shape, not to an observed one.
//!
//! - **hooks.json** is a map of *named* hooks, each
//!   `{ "enabled"?: bool, "SessionStart": [handler], "PreToolUse": [...], ... }`;
//!   agy merges every named hook and runs them in order. The global file is
//!   `~/.gemini/config/hooks.json` (agy 1.0.8's changelog moved it there from
//!   `antigravity-cli/hooks.json`). A `SessionStart` handler may answer
//!   `{"injectSteps":[{"systemMessage":{"systemMessage":"…"}}]}`.
//! - **mcp_config.json** is `{"mcpServers": {"<name>": {"command", "args", "env"}}}`
//!   for stdio servers.
//!
//! Both files are shared with whatever the user configured by hand, so Fleet
//! owns exactly one key in each ([`HOOK_NAME`], [`MCP_SERVER_KEY`]), rewrites
//! only that key, refuses to touch a file it cannot parse, and takes its key
//! back out when agy is switched off or uninstalled — leaving the file the way
//! it found it (an emptied `mcp_config.json` goes back to 0 bytes, which is
//! how agy itself creates it; an emptied `hooks.json` is removed).
//!
//! Why guidance rides a hook instead of `~/.gemini/GEMINI.md`: that file is read
//! by every agy on the machine, so writing there would change the sessions the
//! user opens by hand — the reason codex guidance moved out of
//! `~/.codex/AGENTS.md`. The hook is global too, but its command
//! ([`session_start_output`]) answers `{}` unless the session is one Fleet
//! launched, so a hand-opened agy gets nothing injected.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

/// Fleet's key in `hooks.json`.
pub const HOOK_NAME: &str = "claw-fleet";
/// Fleet's key under `mcpServers` in `mcp_config.json` — the same key Claude and
/// codex sessions use, so tool ids keep the `fleet__*` names.
pub const MCP_SERVER_KEY: &str = crate::mcp_injector::FLEET_SERVER_KEY;
/// Env var Fleet sets on every agy process it launches: the run id of
/// `~/.fleet/agy-runs/<run_id>.json`. Hooks and the MCP server read it to tell a
/// Fleet-launched session from one the user opened by hand — assuming agy
/// passes its environment on to hook and MCP children, which is the default
/// for a Go `exec.Command` but has not been observed with a real session.
pub const RUN_ENV: &str = "FLEET_AGY_RUN";
/// The `fleet` subcommand the hook runs.
pub const HOOK_SUBCOMMAND: &str = "agy-hook";
/// Seconds agy waits for the hook (its documented default is 30).
const HOOK_TIMEOUT_SECS: u64 = 10;

fn gemini_config_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".gemini").join("config"))
}

pub fn hooks_path() -> Option<PathBuf> {
    gemini_config_dir().map(|d| d.join("hooks.json"))
}

pub fn mcp_config_path() -> Option<PathBuf> {
    gemini_config_dir().map(|d| d.join("mcp_config.json"))
}

// ── entries ──────────────────────────────────────────────────────────────────

/// Fleet's named hook. Only `SessionStart`: Fleet derives session state from its
/// own run records, and a `PreToolUse` handler would have to answer `allow` or
/// `deny` for every tool call — overriding agy's own permission prompt, which
/// phase 1 deliberately keeps (no `--dangerously-skip-permissions`).
///
/// The `sh` wrapper prints `{}` when the fleet binary is gone, so an
/// uninstalled Fleet leaves agy a well-formed no-op answer.
pub fn hook_entry(fleet_bin: &str) -> Value {
    let bin = fleet_bin.replace('\'', "'\\''");
    let command = format!(
        "if [ -x '{bin}' ]; then exec '{bin}' {HOOK_SUBCOMMAND} session-start; else echo '{{}}'; fi"
    );
    json!({
        "SessionStart": [
            { "type": "command", "command": command, "timeout": HOOK_TIMEOUT_SECS }
        ]
    })
}

/// Fleet's stdio MCP server entry.
pub fn mcp_entry(fleet_bin: &str) -> Value {
    crate::mcp_injector::build_fleet_entry(fleet_bin)
}

// ── pure merge ───────────────────────────────────────────────────────────────

/// Set (`Some`) or remove (`None`) `key` inside `existing`'s top-level object, or
/// inside its `container` object when one is named. Returns the new file text,
/// or `None` when nothing changes.
///
/// - Empty / whitespace-only text reads as `{}` (agy creates `mcp_config.json`
///   as a 0-byte file).
/// - Unparseable text, or a non-object where an object belongs, is an error:
///   Fleet never overwrites a file it cannot read back.
/// - When removal leaves nothing but empty objects, the result is `""` — the
///   caller decides whether that means "truncate" or "remove".
pub fn merge_entry(
    existing: &str,
    container: Option<&str>,
    key: &str,
    entry: Option<Value>,
) -> Result<Option<String>, String> {
    let mut root: Map<String, Value> = if existing.trim().is_empty() {
        Map::new()
    } else {
        match serde_json::from_str::<Value>(existing) {
            Ok(Value::Object(m)) => m,
            Ok(_) => return Err("top level is not a JSON object".to_string()),
            Err(e) => return Err(format!("not valid JSON: {e}")),
        }
    };
    let before = Value::Object(root.clone());
    {
        let target: &mut Map<String, Value> = match container {
            None => &mut root,
            Some(c) => {
                if entry.is_none() && !root.contains_key(c) {
                    return Ok(None);
                }
                let slot = root
                    .entry(c.to_string())
                    .or_insert_with(|| Value::Object(Map::new()));
                match slot {
                    Value::Object(m) => m,
                    _ => return Err(format!("`{c}` is not a JSON object")),
                }
            }
        };
        match entry {
            Some(v) => {
                target.insert(key.to_string(), v);
            }
            None => {
                target.remove(key);
            }
        }
    }
    if let Some(c) = container {
        if root.get(c).is_some_and(|v| v.as_object().is_some_and(Map::is_empty))
            && !before.get(c).is_some_and(|v| v.as_object().is_some_and(|m| m.is_empty()))
        {
            root.remove(c);
        }
    }
    let after = Value::Object(root);
    if after == before {
        return Ok(None);
    }
    if after.as_object().is_some_and(Map::is_empty) {
        return Ok(Some(String::new()));
    }
    serde_json::to_string_pretty(&after)
        .map(|s| Some(s + "\n"))
        .map_err(|e| e.to_string())
}

/// What to leave behind when Fleet's removal empties a file.
#[derive(Clone, Copy, Debug, PartialEq)]
enum WhenEmpty {
    /// Keep a 0-byte file (agy's own initial state for `mcp_config.json`).
    Truncate,
    /// Delete the file (absent is agy's default for `hooks.json`).
    Remove,
}

/// Apply [`merge_entry`] to the file at `path`. A missing file is only created
/// to add an entry, never to record a removal.
fn apply(
    path: &Path,
    container: Option<&str>,
    key: &str,
    entry: Option<Value>,
    when_empty: WhenEmpty,
) -> Result<(), String> {
    let exists = path.exists();
    if !exists && entry.is_none() {
        return Ok(());
    }
    let existing = if exists {
        fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?
    } else {
        String::new()
    };
    let Some(text) = merge_entry(&existing, container, key, entry)
        .map_err(|e| format!("{}: {e}; left untouched", path.display()))?
    else {
        return Ok(());
    };
    if text.is_empty() && when_empty == WhenEmpty::Remove {
        return fs::remove_file(path).map_err(|e| format!("remove {}: {e}", path.display()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension("json.fleet-tmp");
    fs::write(&tmp, &text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", path.display()))
}

/// Write (`Some(bin)`) or remove (`None`) Fleet's two entries in `config_dir`.
/// `guidance` gates the hook alone: with every guidance concept switched off
/// the hook has nothing to inject, while the MCP server still carries the
/// decision-card tools.
pub fn reconcile_in(config_dir: &Path, fleet_bin: Option<&str>, guidance: bool) -> Result<(), String> {
    let hooks = apply(
        &config_dir.join("hooks.json"),
        None,
        HOOK_NAME,
        fleet_bin.filter(|_| guidance).map(hook_entry),
        WhenEmpty::Remove,
    );
    let mcp = apply(
        &config_dir.join("mcp_config.json"),
        Some("mcpServers"),
        MCP_SERVER_KEY,
        fleet_bin.map(mcp_entry),
        WhenEmpty::Truncate,
    );
    hooks.and(mcp)
}

/// Mirror Fleet's state onto agy's config: entries present while agy is
/// installed and its source is switched on, removed otherwise. Called with the
/// codex/dsh reconcilers (startup and every concept toggle) and when the agy
/// source is toggled.
///
/// A machine that has never had agy is left alone entirely — `~/.gemini` is not
/// conjured for a user who does not use it.
pub fn reconcile() -> Result<(), String> {
    let Some(dir) = gemini_config_dir() else {
        return Ok(());
    };
    let wanted = crate::agent_source::SourcesConfig::load()
        .is_enabled(crate::agy_source::SOURCE_NAME)
        && crate::agy_source::find_agy_binary().is_some();
    if !wanted && !dir.exists() {
        return Ok(());
    }
    let bin = wanted
        .then(crate::fleet_cli::resolve_fleet_binary)
        .flatten()
        .map(|p| p.to_string_lossy().to_string());
    reconcile_in(&dir, bin.as_deref(), guidance_enabled())
}

/// Whether any guidance concept is switched on.
fn guidance_enabled() -> bool {
    use crate::control_plane_prefs::{is_enabled, Feature};
    [
        Feature::InteractionMode,
        Feature::PrdDiscipline,
        Feature::WikiGuidance,
        Feature::ModelGuidance,
        Feature::SessionTitleGuidance,
        Feature::Lessons,
    ]
    .into_iter()
    .any(is_enabled)
}

/// Whether Fleet's entries are currently in agy's config.
pub fn is_installed() -> bool {
    let read = |p: Option<PathBuf>| {
        p.and_then(|p| fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    };
    let hook = read(hooks_path()).is_some_and(|v| v.get(HOOK_NAME).is_some());
    let mcp = read(mcp_config_path())
        .is_some_and(|v| v.pointer(&format!("/mcpServers/{MCP_SERVER_KEY}")).is_some());
    hook || mcp
}

// ── hook command ─────────────────────────────────────────────────────────────

/// Whether the agy session a hook fired in is one Fleet launched: the run-id env
/// var is set, or (if agy does not pass its env through) the payload's
/// `conversationId` belongs to a Fleet run that has not ended yet.
pub fn is_fleet_launched(
    run_env: Option<&str>,
    conversation_id: Option<&str>,
    runs: &[crate::agy_source::RunRecord],
) -> bool {
    if run_env.is_some_and(|r| !r.trim().is_empty()) {
        return true;
    }
    let Some(conv) = conversation_id.filter(|c| !c.is_empty()) else {
        return false;
    };
    runs.iter()
        .any(|r| r.meta.ended_ms.is_none() && r.conversation_id() == Some(conv))
}

/// The JSON a `SessionStart` hook prints: the guidance as an injected system
/// message for a Fleet-launched session, `{}` otherwise.
pub fn session_start_output(fleet_launched: bool, guidance: Option<&str>) -> Value {
    match guidance.map(str::trim).filter(|g| !g.is_empty()) {
        Some(text) if fleet_launched => json!({
            "injectSteps": [ { "systemMessage": { "systemMessage": text } } ]
        }),
        _ => json!({}),
    }
}

/// Answer one hook invocation. `payload` is agy's stdin JSON. Unknown events
/// answer `{}` — the documented no-op for every event Fleet registers.
pub fn handle_hook(event: &str, payload: &Value) -> Value {
    if event != "session-start" {
        return json!({});
    }
    let run_env = std::env::var(RUN_ENV).ok();
    let conv = payload.get("conversationId").and_then(Value::as_str);
    let launched = is_fleet_launched(
        run_env.as_deref(),
        conv,
        &crate::agy_source::runs().unwrap_or_default(),
    );
    if !launched {
        return json!({});
    }
    let guidance = crate::claude_launch::render_harness_guidance();
    session_start_output(true, guidance.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn merge_into_zero_byte_file_creates_the_container() {
        let out = merge_entry("", Some("mcpServers"), "fleet", Some(json!({"command": "/f"})))
            .unwrap()
            .unwrap();
        assert_eq!(obj(&out), json!({"mcpServers": {"fleet": {"command": "/f"}}}));
    }

    #[test]
    fn merge_keeps_other_entries_and_is_idempotent() {
        let existing = r#"{"mcpServers":{"mine":{"url":"https://x"}},"other":1}"#;
        let entry = json!({"command": "/f", "args": ["mcp"]});
        let once = merge_entry(existing, Some("mcpServers"), "fleet", Some(entry.clone()))
            .unwrap()
            .unwrap();
        let v = obj(&once);
        assert_eq!(v["mcpServers"]["mine"], json!({"url": "https://x"}));
        assert_eq!(v["other"], json!(1));
        assert_eq!(v["mcpServers"]["fleet"], entry);
        assert_eq!(
            merge_entry(&once, Some("mcpServers"), "fleet", Some(entry)).unwrap(),
            None,
            "a second write must be a no-op"
        );
    }

    #[test]
    fn removal_restores_the_users_file() {
        let user = r#"{"mine":{"Stop":[{"type":"command","command":"x"}]}}"#;
        let added = merge_entry(user, None, HOOK_NAME, Some(hook_entry("/f")))
            .unwrap()
            .unwrap();
        let removed = merge_entry(&added, None, HOOK_NAME, None).unwrap().unwrap();
        assert_eq!(obj(&removed), obj(user));
        assert_eq!(merge_entry(&removed, None, HOOK_NAME, None).unwrap(), None);
    }

    #[test]
    fn removal_that_empties_the_file_returns_empty_text() {
        let added = merge_entry("", Some("mcpServers"), "fleet", Some(json!({"command": "/f"})))
            .unwrap()
            .unwrap();
        assert_eq!(
            merge_entry(&added, Some("mcpServers"), "fleet", None).unwrap(),
            Some(String::new())
        );
    }

    #[test]
    fn removal_keeps_a_container_the_user_left_empty() {
        let user = r#"{"mcpServers":{}}"#;
        assert_eq!(merge_entry(user, Some("mcpServers"), "fleet", None).unwrap(), None);
    }

    #[test]
    fn unparseable_or_misshapen_files_are_refused() {
        assert!(merge_entry("{oops", None, HOOK_NAME, Some(json!({}))).is_err());
        assert!(merge_entry("[]", None, HOOK_NAME, Some(json!({}))).is_err());
        assert!(merge_entry(r#"{"mcpServers":[]}"#, Some("mcpServers"), "fleet", None).is_err());
    }

    #[test]
    fn hook_entry_matches_the_documented_shape() {
        let e = hook_entry("/Apps/it's/fleet");
        let h = &e["SessionStart"][0];
        assert_eq!(h["type"], "command");
        assert_eq!(h["timeout"], json!(HOOK_TIMEOUT_SECS));
        let cmd = h["command"].as_str().unwrap();
        assert!(cmd.contains("agy-hook session-start"), "{cmd}");
        assert!(cmd.contains(r"'/Apps/it'\''s/fleet'"), "quote escaped: {cmd}");
        assert!(cmd.ends_with("else echo '{}'; fi"), "{cmd}");
        assert!(e.get("PreToolUse").is_none(), "must not override agy's permission prompt");
    }

    #[test]
    fn reconcile_in_round_trips_against_real_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("mcp_config.json"), "").unwrap();
        fs::write(
            dir.join("hooks.json"),
            r#"{"mine":{"SessionStart":[{"type":"command","command":"echo hi"}]}}"#,
        )
        .unwrap();

        reconcile_in(dir, Some("/bin/fleet"), true).unwrap();
        let hooks = obj(&fs::read_to_string(dir.join("hooks.json")).unwrap());
        assert!(hooks.get("mine").is_some() && hooks.get(HOOK_NAME).is_some());
        let mcp = obj(&fs::read_to_string(dir.join("mcp_config.json")).unwrap());
        assert_eq!(mcp["mcpServers"]["fleet"]["command"], "/bin/fleet");

        // Guidance off: hook out, MCP stays.
        reconcile_in(dir, Some("/bin/fleet"), false).unwrap();
        let hooks = obj(&fs::read_to_string(dir.join("hooks.json")).unwrap());
        assert!(hooks.get(HOOK_NAME).is_none() && hooks.get("mine").is_some());

        // agy off: everything Fleet wrote is gone, the 0-byte file is back.
        reconcile_in(dir, None, true).unwrap();
        assert_eq!(fs::read_to_string(dir.join("mcp_config.json")).unwrap(), "");
        let hooks = obj(&fs::read_to_string(dir.join("hooks.json")).unwrap());
        assert_eq!(hooks, json!({"mine":{"SessionStart":[{"type":"command","command":"echo hi"}]}}));
    }

    #[test]
    fn reconcile_in_removes_a_hooks_file_it_created() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        reconcile_in(dir, Some("/bin/fleet"), true).unwrap();
        assert!(dir.join("hooks.json").exists());
        reconcile_in(dir, None, true).unwrap();
        assert!(!dir.join("hooks.json").exists());
        // Removal on a machine where nothing exists creates nothing.
        let empty = tempfile::tempdir().unwrap();
        reconcile_in(empty.path(), None, true).unwrap();
        assert!(fs::read_dir(empty.path()).unwrap().next().is_none());
    }

    #[test]
    fn reconcile_in_leaves_an_unparseable_file_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("hooks.json"), "{ not json").unwrap();
        assert!(reconcile_in(dir, Some("/bin/fleet"), true).is_err());
        assert_eq!(fs::read_to_string(dir.join("hooks.json")).unwrap(), "{ not json");
        // The other file is still reconciled.
        assert!(dir.join("mcp_config.json").exists());
    }

    fn run(conv: Option<&str>, ended: bool) -> crate::agy_source::RunRecord {
        let mut r = crate::agy_source::RunRecord::default();
        r.meta.conversation_id = conv.map(str::to_string);
        r.meta.ended_ms = ended.then_some(1);
        r
    }

    #[test]
    fn fleet_launch_is_recognised_by_env_or_live_run() {
        assert!(is_fleet_launched(Some("run-1"), None, &[]));
        assert!(!is_fleet_launched(Some(" "), None, &[]));
        let runs = [run(Some("c1"), false), run(Some("c2"), true)];
        assert!(is_fleet_launched(None, Some("c1"), &runs));
        assert!(!is_fleet_launched(None, Some("c2"), &runs), "ended run");
        assert!(!is_fleet_launched(None, Some("c3"), &runs), "hand-opened");
        assert!(!is_fleet_launched(None, None, &runs));
    }

    #[test]
    fn session_start_injects_only_into_fleet_sessions() {
        assert_eq!(
            session_start_output(true, Some("rules")),
            json!({"injectSteps":[{"systemMessage":{"systemMessage":"rules"}}]})
        );
        assert_eq!(session_start_output(false, Some("rules")), json!({}));
        assert_eq!(session_start_output(true, Some("  ")), json!({}));
        assert_eq!(session_start_output(true, None), json!({}));
    }
}
