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
    r#"sh -c 'cat >> "$HOME/.claude/fleet/hooks.jsonl"'"#;

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
    dirs::home_dir().map(|h| h.join(".claude").join("settings.json"))
}

pub fn hooks_events_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("fleet").join("hooks.jsonl"))
}

fn fleet_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("fleet"))
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

    HookSetupPlan {
        to_add,
        hooks_globally_disabled: hooks_disabled,
        already_installed: all_present,
    }
}

// ── Apply ────────────────────────────────────────────────────────────────────

/// Merge Fleet hooks into settings.json.  Only touches the `hooks` key;
/// all other settings are preserved byte-for-byte.
pub fn apply_hook_setup() -> Result<(), String> {
    // Ensure ~/.claude/fleet/ directory exists.
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
                    .map(|c| c.contains("fleet/hooks.jsonl"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
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
