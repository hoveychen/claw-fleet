//! Fleet hooks — injects Claude Code hooks into ~/.claude/settings.json for
//! accurate agent state detection, and reads the resulting hook events.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

// ── Constants ────────────────────────────────────────────────────────────────

/// The shell command our hooks use.  Used as the identity marker when merging.
const FLEET_HOOK_COMMAND: &str =
    r#"sh -c 'cat >> "$HOME/.fleet/hooks.jsonl"'"#;

// Identity markers for the fleet hook groups. These substrings must appear
// verbatim in the command string produced by `fault_tolerant_command`, which
// wraps the binary path in double quotes — so the character between `fleet`
// and the subcommand is `"`, not a space.
const FLEET_GUARD_MARKER: &str = "\" guard;";
const FLEET_ELICITATION_MARKER: &str = "\" elicitation;";
const FLEET_PLAN_APPROVAL_MARKER: &str = "\" plan-approval;";

/// Event types we need hooks for.
const FLEET_HOOK_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "Stop",
    "SubagentStop",
];

// ── Public types ─────────────────────────────────────────────────────────────

/// Describes what Fleet wants to add/change in settings.json.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HookSetupPlan {
    /// Events that need a new Fleet hook group appended (no conflict).
    pub to_add: Vec<String>,
    /// True when `disableAllHooks` is set — hooks won't run even if we add them.
    pub hooks_globally_disabled: bool,
    /// Whether Fleet hooks are already fully installed.
    pub already_installed: bool,
    /// Whether the guard (interception) hook is installed.
    pub guard_installed: bool,
    /// Whether the elicitation (AskUserQuestion interception) hook is installed.
    pub elicitation_installed: bool,
    /// Whether the plan-approval (ExitPlanMode interception) hook is installed.
    pub plan_approval_installed: bool,
    /// Whether the interaction-mode CLAUDE.md guidance is installed.
    pub interaction_mode_installed: bool,
}

/// The "cooked" state derived from the most recent hook events for a session.
#[derive(Debug, Clone, PartialEq)]
pub enum HookState {
    /// Between PreToolUse and PostToolUse — tool is definitely running.
    ToolExecuting,
    /// PostToolUse/PostToolUseFailure just fired — model is processing the result.
    ModelProcessing,
    /// Stop fired — agent finished its turn.
    Stopped,
    /// No recent hook events for this session.
    Unknown,
}

/// A single parsed hook event line.
#[derive(Debug, Clone)]
pub struct HookEvent {
    pub session_id: String,
    pub event_name: String,
    pub timestamp_ms: u64,
}

// ── Paths ────────────────────────────────────────────────────────────────────

fn settings_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".claude").join("settings.json"))
}

pub fn hooks_events_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("hooks.jsonl"))
}

fn fleet_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet"))
}

// ── Plan (dry-run) ───────────────────────────────────────────────────────────

/// Inspect settings.json and report what changes are needed.
pub fn plan_hook_setup() -> HookSetupPlan {
    let settings = read_settings().unwrap_or_else(|| json!({}));

    let hooks_disabled = settings
        .get("disableAllHooks")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let hooks_obj = settings
        .get("hooks")
        .and_then(|h| h.as_object())
        .cloned()
        .unwrap_or_default();

    let mut to_add = Vec::new();
    let mut all_present = true;

    for &event in FLEET_HOOK_EVENTS {
        if !has_fleet_hook(&hooks_obj, event) {
            to_add.push(event.to_string());
            all_present = false;
        }
    }

    let guard_installed = has_guard_hook(&hooks_obj);
    let elicitation_installed = has_elicitation_hook(&hooks_obj);
    let plan_approval_installed = has_plan_approval_hook(&hooks_obj);
    let interaction_mode_installed = crate::interaction_mode::is_interaction_mode_installed();

    HookSetupPlan {
        to_add,
        hooks_globally_disabled: hooks_disabled,
        already_installed: all_present,
        guard_installed,
        elicitation_installed,
        plan_approval_installed,
        interaction_mode_installed,
    }
}

// ── Apply ────────────────────────────────────────────────────────────────────

/// Merge Fleet hooks into settings.json.  Only touches the `hooks` key;
/// all other settings are preserved byte-for-byte.
pub fn apply_hook_setup() -> Result<(), String> {
    // Ensure ~/.fleet/ directory exists.
    if let Some(dir) = fleet_dir() {
        fs::create_dir_all(&dir).map_err(|e| format!("create fleet dir: {e}"))?;
    }

    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let obj = settings.as_object_mut().ok_or("settings is not an object")?;

    // Ensure "hooks" key exists as an object.
    if !obj.contains_key("hooks") {
        obj.insert("hooks".into(), json!({}));
    }
    let hooks_obj = obj
        .get_mut("hooks")
        .and_then(|h| h.as_object_mut())
        .ok_or("hooks is not an object")?;

    for &event in FLEET_HOOK_EVENTS {
        if has_fleet_hook(hooks_obj, event) {
            continue;
        }

        let fleet_group = fleet_hook_group();

        if let Some(existing) = hooks_obj.get_mut(event) {
            // Append our group to the existing array.
            if let Some(arr) = existing.as_array_mut() {
                arr.push(fleet_group);
            }
        } else {
            // Create new array with just our group.
            hooks_obj.insert(event.to_string(), json!([fleet_group]));
        }
    }

    write_settings(&settings)
}

/// Remove all Fleet hooks from settings.json.
pub fn remove_fleet_hooks() -> Result<(), String> {
    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let Some(obj) = settings.as_object_mut() else {
        return Ok(());
    };
    let Some(hooks_obj) = obj.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return Ok(());
    };

    for &event in FLEET_HOOK_EVENTS {
        if let Some(arr) = hooks_obj.get_mut(event).and_then(|v| v.as_array_mut()) {
            arr.retain(|group| !is_fleet_group(group));
            if arr.is_empty() {
                hooks_obj.remove(event);
            }
        }
    }

    // Remove "hooks" key entirely if empty.
    if hooks_obj.is_empty() {
        obj.remove("hooks");
    }

    write_settings(&settings)
}

// ── Guard hook (synchronous interception) ────────────────────────────────────

/// Resolve the `fleet` binary path for use in the guard hook command.
fn resolve_fleet_binary() -> Option<String> {
    // 1. Check if this process IS the fleet binary (desktop app has sidecar).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let fleet_bin = dir.join("fleet");
            if fleet_bin.exists() {
                return Some(fleet_bin.to_string_lossy().to_string());
            }
        }
    }

    // 2. Check common install locations.
    let candidates = [
        "/usr/local/bin/fleet",
    ];
    for c in candidates {
        if std::path::Path::new(c).exists() {
            return Some(c.to_string());
        }
    }

    // 3. Try PATH.
    #[cfg(unix)]
    {
        if let Ok(output) = std::process::Command::new("which").arg("fleet").output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Some(path);
                }
            }
        }
    }

    None
}

/// Install the guard hook (synchronous PreToolUse for Bash) into settings.json.
pub fn apply_guard_hook() -> Result<(), String> {
    let fleet_bin = resolve_fleet_binary()
        .ok_or("Cannot find fleet binary — install fleet CLI first")?;

    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let obj = settings.as_object_mut().ok_or("settings is not an object")?;

    if !obj.contains_key("hooks") {
        obj.insert("hooks".into(), json!({}));
    }
    let hooks_obj = obj
        .get_mut("hooks")
        .and_then(|h| h.as_object_mut())
        .ok_or("hooks is not an object")?;

    let guard_group = json!({
        "matcher": "Bash",
        "hooks": [{
            "type": "command",
            "command": fault_tolerant_command(&fleet_bin, "guard"),
            "timeout": 120000
        }]
    });

    // Idempotent: strip any pre-existing fleet guard groups (possibly pointing
    // at stale binary paths) before appending a fresh one.
    if let Some(existing) = hooks_obj.get_mut("PreToolUse") {
        if let Some(arr) = existing.as_array_mut() {
            arr.retain(|group| !is_guard_group(group));
            arr.push(guard_group);
        }
    } else {
        hooks_obj.insert("PreToolUse".to_string(), json!([guard_group]));
    }

    write_settings(&settings)
}

/// Remove the guard hook from settings.json.
pub fn remove_guard_hook() -> Result<(), String> {
    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let Some(obj) = settings.as_object_mut() else {
        return Ok(());
    };
    let Some(hooks_obj) = obj.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return Ok(());
    };

    if let Some(arr) = hooks_obj.get_mut("PreToolUse").and_then(|v| v.as_array_mut()) {
        arr.retain(|group| !is_guard_group(group));
        if arr.is_empty() {
            hooks_obj.remove("PreToolUse");
        }
    }

    if hooks_obj.is_empty() {
        obj.remove("hooks");
    }

    write_settings(&settings)
}

/// Check whether PreToolUse already has a guard hook group.
fn has_guard_hook(hooks_obj: &Map<String, Value>) -> bool {
    hooks_obj
        .get("PreToolUse")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(|group| is_guard_group(group)))
        .unwrap_or(false)
}

/// Check whether a hook group is a guard hook (by matching the command).
fn is_guard_group(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter().any(|hook| {
                hook.get("command")
                    .and_then(|c| c.as_str())
                    .map(|c| c.contains(FLEET_GUARD_MARKER))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

// ── Elicitation hook (AskUserQuestion interception) ─────────────────────

/// Install the elicitation hook (synchronous PreToolUse for AskUserQuestion).
pub fn apply_elicitation_hook() -> Result<(), String> {
    let fleet_bin = resolve_fleet_binary()
        .ok_or("Cannot find fleet binary — install fleet CLI first")?;

    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let obj = settings.as_object_mut().ok_or("settings is not an object")?;

    if !obj.contains_key("hooks") {
        obj.insert("hooks".into(), json!({}));
    }
    let hooks_obj = obj
        .get_mut("hooks")
        .and_then(|h| h.as_object_mut())
        .ok_or("hooks is not an object")?;

    let elicitation_group = json!({
        "matcher": "AskUserQuestion",
        "hooks": [{
            "type": "command",
            "command": fault_tolerant_command(&fleet_bin, "elicitation"),
            "timeout": 120000
        }]
    });

    // Idempotent: strip any pre-existing fleet elicitation groups (possibly
    // pointing at stale binary paths) before appending a fresh one.
    if let Some(existing) = hooks_obj.get_mut("PreToolUse") {
        if let Some(arr) = existing.as_array_mut() {
            arr.retain(|group| !is_elicitation_group(group));
            arr.push(elicitation_group);
        }
    } else {
        hooks_obj.insert("PreToolUse".to_string(), json!([elicitation_group]));
    }

    write_settings(&settings)
}

/// Remove the elicitation hook from settings.json.
pub fn remove_elicitation_hook() -> Result<(), String> {
    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let Some(obj) = settings.as_object_mut() else {
        return Ok(());
    };
    let Some(hooks_obj) = obj.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return Ok(());
    };

    if let Some(arr) = hooks_obj.get_mut("PreToolUse").and_then(|v| v.as_array_mut()) {
        arr.retain(|group| !is_elicitation_group(group));
        if arr.is_empty() {
            hooks_obj.remove("PreToolUse");
        }
    }

    if hooks_obj.is_empty() {
        obj.remove("hooks");
    }

    write_settings(&settings)
}

fn has_elicitation_hook(hooks_obj: &Map<String, Value>) -> bool {
    hooks_obj
        .get("PreToolUse")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(|group| is_elicitation_group(group)))
        .unwrap_or(false)
}

fn is_elicitation_group(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter().any(|hook| {
                hook.get("command")
                    .and_then(|c| c.as_str())
                    .map(|c| c.contains(FLEET_ELICITATION_MARKER))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

// ── Plan-approval hook (ExitPlanMode interception) ──────────────────────

/// Install the plan-approval hook (synchronous PreToolUse for ExitPlanMode).
pub fn apply_plan_approval_hook() -> Result<(), String> {
    let fleet_bin = resolve_fleet_binary()
        .ok_or("Cannot find fleet binary — install fleet CLI first")?;

    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let obj = settings.as_object_mut().ok_or("settings is not an object")?;

    if !obj.contains_key("hooks") {
        obj.insert("hooks".into(), json!({}));
    }
    let hooks_obj = obj
        .get_mut("hooks")
        .and_then(|h| h.as_object_mut())
        .ok_or("hooks is not an object")?;

    let plan_approval_group = json!({
        "matcher": "ExitPlanMode",
        "hooks": [{
            "type": "command",
            "command": fault_tolerant_command(&fleet_bin, "plan-approval"),
            "timeout": 600000
        }]
    });

    // Idempotent: strip any pre-existing fleet plan-approval groups before
    // appending a fresh one.
    if let Some(existing) = hooks_obj.get_mut("PreToolUse") {
        if let Some(arr) = existing.as_array_mut() {
            arr.retain(|group| !is_plan_approval_group(group));
            arr.push(plan_approval_group);
        }
    } else {
        hooks_obj.insert("PreToolUse".to_string(), json!([plan_approval_group]));
    }

    write_settings(&settings)
}

/// Remove the plan-approval hook from settings.json.
pub fn remove_plan_approval_hook() -> Result<(), String> {
    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let Some(obj) = settings.as_object_mut() else {
        return Ok(());
    };
    let Some(hooks_obj) = obj.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return Ok(());
    };

    if let Some(arr) = hooks_obj.get_mut("PreToolUse").and_then(|v| v.as_array_mut()) {
        arr.retain(|group| !is_plan_approval_group(group));
        if arr.is_empty() {
            hooks_obj.remove("PreToolUse");
        }
    }

    if hooks_obj.is_empty() {
        obj.remove("hooks");
    }

    write_settings(&settings)
}

fn has_plan_approval_hook(hooks_obj: &Map<String, Value>) -> bool {
    hooks_obj
        .get("PreToolUse")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(|group| is_plan_approval_group(group)))
        .unwrap_or(false)
}

fn is_plan_approval_group(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter().any(|hook| {
                hook.get("command")
                    .and_then(|c| c.as_str())
                    .map(|c| c.contains(FLEET_PLAN_APPROVAL_MARKER))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

// ── Read hook events ─────────────────────────────────────────────────────────

/// Read the hook events file and compute per-session HookState.
/// Returns a map from session_id to the derived state.
pub fn read_hook_states() -> HashMap<String, HookState> {
    let Some(path) = hooks_events_path() else {
        return HashMap::new();
    };

    let events = read_recent_events(&path, 500);
    let mut result: HashMap<String, HookState> = HashMap::new();

    // Group by session_id, keep only the latest event per session.
    let mut latest: HashMap<String, HookEvent> = HashMap::new();
    for ev in events {
        let entry = latest.entry(ev.session_id.clone()).or_insert_with(|| ev.clone());
        if ev.timestamp_ms >= entry.timestamp_ms {
            *entry = ev;
        }
    }

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    for (sid, ev) in latest {
        let age_ms = now_ms.saturating_sub(ev.timestamp_ms);

        // Ignore hook events older than 5 minutes — too stale to be useful.
        if age_ms > 300_000 {
            continue;
        }

        let state = match ev.event_name.as_str() {
            "PreToolUse" => HookState::ToolExecuting,
            "PostToolUse" | "PostToolUseFailure" => HookState::ModelProcessing,
            "Stop" | "SubagentStop" => HookState::Stopped,
            _ => HookState::Unknown,
        };
        result.insert(sid, state);
    }

    result
}

/// Truncate the hooks events file if it exceeds a threshold (e.g. 10000 lines).
/// Keeps the last 2000 lines.
pub fn maybe_truncate_events_file() {
    let Some(path) = hooks_events_path() else {
        return;
    };
    let Ok(content) = fs::read_to_string(&path) else {
        return;
    };
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() > 10_000 {
        let keep = &lines[lines.len() - 2000..];
        let _ = fs::write(&path, keep.join("\n") + "\n");
    }
}

// ── Private helpers ──────────────────────────────────────────────────────────

fn read_settings() -> Option<Value> {
    let path = settings_path()?;
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

fn write_settings(value: &Value) -> Result<(), String> {
    let path = settings_path().ok_or("cannot determine home dir")?;
    let content =
        serde_json::to_string_pretty(value).map_err(|e| format!("serialize settings: {e}"))?;
    fs::write(&path, content + "\n").map_err(|e| format!("write settings: {e}"))
}

/// Build the Fleet hook group object.
fn fleet_hook_group() -> Value {
    json!({
        "hooks": [{
            "type": "command",
            "command": FLEET_HOOK_COMMAND,
            "async": true
        }]
    })
}

/// Check whether a given event already has a Fleet hook group.
fn has_fleet_hook(hooks_obj: &Map<String, Value>, event: &str) -> bool {
    hooks_obj
        .get(event)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(|group| is_fleet_group(group)))
        .unwrap_or(false)
}

/// Check whether a hook group object is ours (by matching the command string).
fn is_fleet_group(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter().any(|hook| {
                hook.get("command")
                    .and_then(|c| c.as_str())
                    .map(|c| c.contains(".fleet/hooks.jsonl"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Build a fault-tolerant shell command that silently exits 0 when the fleet
/// binary is missing (e.g. after uninstall), so Claude Code is not blocked.
/// When the binary exists, it `exec`s into it — propagating its exit code and
/// stdout/stderr as normal.
fn fault_tolerant_command(fleet_bin: &str, subcommand: &str) -> String {
    // Use `test -x` so it works even if the binary was removed from PATH but
    // the absolute path is stale.  `exec` avoids an extra shell process.
    format!(
        r#"sh -c 'if [ -x "{bin}" ]; then exec "{bin}" {sub}; else exit 0; fi'"#,
        bin = fleet_bin,
        sub = subcommand,
    )
}

/// Read the last `max_lines` from the events file and parse them.
fn read_recent_events(path: &Path, max_lines: usize) -> Vec<HookEvent> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };

    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(max_lines);

    lines[start..]
        .iter()
        .filter_map(|line| {
            let v: Value = serde_json::from_str(line).ok()?;
            let session_id = v.get("session_id")?.as_str()?.to_string();
            let event_name = v.get("hook_event_name")?.as_str()?.to_string();

            // Try to get timestamp from the event; fall back to 0.
            let timestamp_ms = v
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(|s| {
                    chrono::DateTime::parse_from_rfc3339(s)
                        .ok()
                        .map(|dt| dt.timestamp_millis() as u64)
                })
                .unwrap_or(0);

            Some(HookEvent {
                session_id,
                event_name,
                timestamp_ms,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Builds the same group JSON as `apply_guard_hook` would emit.
    fn guard_group_for(bin: &str) -> Value {
        json!({
            "matcher": "Bash",
            "hooks": [{
                "type": "command",
                "command": fault_tolerant_command(bin, "guard"),
                "timeout": 120000
            }]
        })
    }

    fn elicitation_group_for(bin: &str) -> Value {
        json!({
            "matcher": "AskUserQuestion",
            "hooks": [{
                "type": "command",
                "command": fault_tolerant_command(bin, "elicitation"),
                "timeout": 120000
            }]
        })
    }

    fn plan_approval_group_for(bin: &str) -> Value {
        json!({
            "matcher": "ExitPlanMode",
            "hooks": [{
                "type": "command",
                "command": fault_tolerant_command(bin, "plan-approval"),
                "timeout": 600000
            }]
        })
    }

    #[test]
    fn guard_marker_detects_actual_generated_command() {
        // Reproduces the dedup bug: the marker `"fleet guard"` (with a space)
        // never matches the real command string, which is
        // `... "/path/to/fleet" guard; ...` — i.e. a quote separates `fleet`
        // from `guard`, not a space.
        let group = guard_group_for("/Applications/Claw Fleet.app/Contents/MacOS/fleet");
        assert!(
            is_guard_group(&group),
            "is_guard_group must recognise the command actually produced by \
             fault_tolerant_command"
        );
    }

    #[test]
    fn elicitation_marker_detects_actual_generated_command() {
        let group = elicitation_group_for("/Applications/Claw Fleet.app/Contents/MacOS/fleet");
        assert!(
            is_elicitation_group(&group),
            "is_elicitation_group must recognise the command actually produced \
             by fault_tolerant_command"
        );
    }

    #[test]
    fn plan_approval_marker_detects_actual_generated_command() {
        let group =
            plan_approval_group_for("/Applications/Claw Fleet.app/Contents/MacOS/fleet");
        assert!(
            is_plan_approval_group(&group),
            "is_plan_approval_group must recognise the command actually produced \
             by fault_tolerant_command"
        );
    }

    #[test]
    fn idempotent_retain_removes_stale_groups() {
        // Simulates what apply_*_hook's retain-then-push loop does across
        // multiple binary paths: existing fleet groups must be filtered out
        // regardless of which binary path they point to.
        let mut arr = vec![
            json!({ "matcher": "Bash", "hooks": [{"type": "command", "command": "unrelated"}] }),
            guard_group_for("/Applications/Claw Fleet.app/Contents/MacOS/fleet"),
            guard_group_for("/Users/x/workspace/claude-fleet/target/debug/fleet"),
            elicitation_group_for("/Applications/Claw Fleet.app/Contents/MacOS/fleet"),
            elicitation_group_for("/Users/x/workspace/claude-fleet/target/debug/fleet"),
            plan_approval_group_for("/Applications/Claw Fleet.app/Contents/MacOS/fleet"),
            plan_approval_group_for("/Users/x/workspace/claude-fleet/target/debug/fleet"),
        ];
        arr.retain(|g| {
            !is_guard_group(g) && !is_elicitation_group(g) && !is_plan_approval_group(g)
        });
        assert_eq!(arr.len(), 1, "only the unrelated entry should survive");
    }

    #[test]
    fn markers_do_not_cross_match() {
        // All three markers must be mutually exclusive.
        let g = guard_group_for("/x/fleet");
        let e = elicitation_group_for("/x/fleet");
        let p = plan_approval_group_for("/x/fleet");
        assert!(!is_elicitation_group(&g));
        assert!(!is_plan_approval_group(&g));
        assert!(!is_guard_group(&e));
        assert!(!is_plan_approval_group(&e));
        assert!(!is_guard_group(&p));
        assert!(!is_elicitation_group(&p));
    }
}
