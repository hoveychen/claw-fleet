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
const FLEET_HOOK_COMMAND: &str = r#"sh -c 'cat >> "$HOME/.fleet/hooks.jsonl"'"#;

/// Legacy event-log path that a pre-`~/.fleet` build installed a `cat >>` hook
/// for. `is_fleet_group` never matched it (it only recognizes the current
/// `.fleet/hooks.jsonl` path), so once we migrated the write target that group
/// lingered in settings.json — appended to on every event with no truncation,
/// seen at 5.3 GB in the wild. `purge_legacy_event_hooks` deletes it on sync.
const LEGACY_EVENTS_HOOK_SUBSTR: &str = ".claude/fleet/hooks.jsonl";

// Fleet hook groups are identified structurally by
// [`group_invokes_fleet_subcommand`], which recognizes both shapes a Fleet
// hook can take: the unix `sh -c` wrapper from [`fault_tolerant_command`] and
// the Windows exec form from [`fleet_subcommand_hook`].

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
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
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
    /// Whether the PRD-context (UserPromptSubmit injection) hook is installed.
    /// Requires its SessionStart companion (`notes_hint_installed`) too, so a
    /// host upgraded from a build without the companion reads as not installed
    /// and `control_plane::heal` fills the gap on the next start.
    pub prd_context_installed: bool,
    /// Whether the notes-hint SessionStart hook (post-compaction re-injection
    /// of the session's private notes) is installed. Installed and removed with
    /// the PRD-context hook, never on its own. `default` keeps payloads from
    /// older `fleet serve` probes deserializable.
    #[serde(default)]
    pub notes_hint_installed: bool,
    /// Whether the PRD-discipline CLAUDE.md guidance is installed.
    pub prd_discipline_installed: bool,
    /// Whether the wiki-guidance CLAUDE.md block is installed. `default` keeps
    /// payloads from older `fleet serve` probes deserializable.
    #[serde(default)]
    pub wiki_guidance_installed: bool,
    /// Whether the model-guidance CLAUDE.md block is installed. `default` keeps
    /// payloads from older `fleet serve` probes deserializable.
    #[serde(default)]
    pub model_guidance_installed: bool,
    /// Whether the session-title guidance CLAUDE.md block is installed —
    /// the one that tells the agent to name its own session through
    /// `fleet__set_session_title`. `default` keeps payloads from older
    /// `fleet serve` probes deserializable.
    #[serde(default)]
    pub session_title_guidance_installed: bool,
    /// Whether the idle hooks (Stop + UserPromptSubmit → kanban Pending) are installed.
    pub idle_hooks_installed: bool,
    /// Whether the wakeup-guard hook (ScheduleWakeup / CronCreate interception)
    /// is installed. `default` keeps payloads from older `fleet serve` probes
    /// deserializable, like the two guidance flags above.
    #[serde(default)]
    pub wakeup_guard_installed: bool,
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
    /// A PreToolUse fired for a tool whose whole job is to wait for the user —
    /// a decision card, a permission prompt. The tool is "running", but nothing
    /// is being computed; the session is parked until someone answers.
    AwaitingUserInput,
    /// No recent hook events for this session.
    Unknown,
}

/// A single parsed hook event line.
#[derive(Debug, Clone)]
pub struct HookEvent {
    pub session_id: String,
    pub event_name: String,
    pub timestamp_ms: u64,
    /// The tool a `PreToolUse` / `PostToolUse` fired for; `None` on the rest.
    /// Tells a tool that runs from one the session is parked on waiting for an
    /// answer — see [`HookState::AwaitingUserInput`].
    pub tool_name: Option<String>,
    /// Only `Stop` / `SubagentStop` carry these (CLI ≥ 2.1.145); empty otherwise.
    pub background_tasks: Vec<crate::bg_guard::BackgroundTask>,
}

// ── Paths ────────────────────────────────────────────────────────────────────

fn settings_path() -> Option<PathBuf> {
    crate::session::get_claude_dir().map(|d| d.join("settings.json"))
}

pub fn hooks_events_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("hooks.jsonl"))
}

/// `fleet hook-event` — the exec-form counterpart of the unix
/// `sh -c 'cat >> "$HOME/.fleet/hooks.jsonl"'` event hook (Windows runs hook
/// strings through PowerShell when Git Bash is absent, so the sh one-liner
/// can't be relied on there). Appends the hook JSON from `reader` to
/// `~/.fleet/hooks.jsonl`, newline-terminated, creating the directory on
/// first use just like the guard/elicitation entrypoints do.
pub fn append_hook_event(reader: &mut impl std::io::Read) -> Result<(), String> {
    let mut buf = String::new();
    reader
        .read_to_string(&mut buf)
        .map_err(|e| format!("read hook event from stdin: {e}"))?;
    let line = buf.trim();
    if line.is_empty() {
        return Ok(());
    }
    let path = hooks_events_path().ok_or("cannot determine home dir")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    use std::io::Write as _;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    writeln!(f, "{line}").map_err(|e| format!("append {}: {e}", path.display()))
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
    let notes_hint_installed = has_notes_hint_hook(&hooks_obj);
    let prd_context_installed = has_prd_context_hook(&hooks_obj);
    let prd_discipline_installed = crate::prd_discipline::is_prd_discipline_installed();
    let wiki_guidance_installed = crate::wiki_guidance::is_wiki_guidance_installed();
    let model_guidance_installed = crate::model_guidance::is_model_guidance_installed();
    let session_title_guidance_installed =
        crate::session_title_guidance::is_session_title_guidance_installed();
    let idle_hooks_installed = has_idle_hooks(&hooks_obj);
    let wakeup_guard_installed = has_wakeup_guard_hook(&hooks_obj);

    HookSetupPlan {
        to_add,
        hooks_globally_disabled: hooks_disabled,
        already_installed: all_present,
        guard_installed,
        elicitation_installed,
        plan_approval_installed,
        interaction_mode_installed,
        prd_context_installed,
        notes_hint_installed,
        prd_discipline_installed,
        wiki_guidance_installed,
        model_guidance_installed,
        session_title_guidance_installed,
        idle_hooks_installed,
        wakeup_guard_installed,
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
    let obj = settings
        .as_object_mut()
        .ok_or("settings is not an object")?;

    // Ensure "hooks" key exists as an object.
    if !obj.contains_key("hooks") {
        obj.insert("hooks".into(), json!({}));
    }
    let hooks_obj = obj
        .get_mut("hooks")
        .and_then(|h| h.as_object_mut())
        .ok_or("hooks is not an object")?;

    // Self-heal: drop any legacy `~/.claude/fleet/hooks.jsonl` groups a pre-move
    // build left behind before we (re)install the current `~/.fleet` groups.
    purge_legacy_event_hooks(hooks_obj);

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

    // Uninstall should leave nothing of ours behind, including any legacy
    // event-log group from a pre-`~/.fleet` build.
    purge_legacy_event_hooks(hooks_obj);

    // Remove "hooks" key entirely if empty.
    if hooks_obj.is_empty() {
        obj.remove("hooks");
    }

    write_settings(&settings)
}

/// Remove every hook group that appends to the legacy
/// `~/.claude/fleet/hooks.jsonl` path from all event arrays, dropping any event
/// array left empty. Pure over the `hooks` object so it can be unit-tested
/// without touching settings.json.
fn purge_legacy_event_hooks(hooks_obj: &mut Map<String, Value>) {
    let events: Vec<String> = hooks_obj.keys().cloned().collect();
    for event in events {
        let Some(arr) = hooks_obj.get_mut(&event).and_then(|v| v.as_array_mut()) else {
            continue;
        };
        arr.retain(|group| !group_targets_legacy_events_file(group));
        if arr.is_empty() {
            hooks_obj.remove(&event);
        }
    }
}

/// Whether a hook group appends to the legacy `~/.claude/fleet/hooks.jsonl`
/// path (installed by an older build, no longer recognized by `is_fleet_group`).
fn group_targets_legacy_events_file(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter().any(|hook| {
                hook.get("command")
                    .and_then(|c| c.as_str())
                    .map(|c| c.contains(LEGACY_EVENTS_HOOK_SUBSTR))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

// ── Guard hook (synchronous interception) ────────────────────────────────────

/// Resolve the `fleet` binary path baked into the hook commands we write.
///
/// Delegates to [`crate::fleet_cli::resolve_fleet_binary`] — this used to carry
/// its own copy that looked only for an extension-less `fleet`, only fell back
/// to `/usr/local/bin/fleet`, and gated its PATH probe behind `#[cfg(unix)]`,
/// so on Windows it always returned `None` and every hook install failed with
/// "Cannot find fleet binary".
pub(crate) fn resolve_fleet_binary() -> Option<String> {
    crate::fleet_cli::resolve_fleet_binary().map(|p| p.to_string_lossy().to_string())
}

/// The fleet binary to bake into `settings.json`, refusing one that Rule 3's
/// worktree cleanup is about to delete.
///
/// `settings.json` outlives this process by design, so a hook naming a
/// worktree build is a hook that stops existing at merge time. The MCP injector
/// has refused that since 2026-09-06; hooks never did, which is how a machine
/// ends up with a `guard` hook — the gate for *every* shell command — pointing
/// into a directory that was deleted weeks ago.
fn resolve_publishable_fleet_binary() -> Result<String, String> {
    let bin = resolve_fleet_binary().ok_or("Cannot find fleet binary — install fleet CLI first")?;
    if !crate::fleet_cli::may_publish_self(&bin) {
        return Err(crate::fleet_cli::ephemeral_publish_refused(
            &bin,
            "settings.json hooks",
        ));
    }
    Ok(bin)
}

/// PreToolUse matcher for the guard hook. Pipe alternation fires the group for
/// **either** shell tool Claude Code can drive — `Bash` and `PowerShell` — so
/// both are audited by the single guard group. See [`apply_guard_hook`] for why
/// omitting `PowerShell` would leave Windows-without-Git-Bash sessions running
/// un-audited.
pub(crate) const GUARD_MATCHER: &str = "Bash|PowerShell";

/// Install the guard hook (synchronous PreToolUse for shell tools) into
/// settings.json.
///
/// The matcher covers **both** shell tools Claude Code can drive: `Bash`
/// (macOS/Linux, and Windows with Git Bash) and `PowerShell` (Windows without
/// Git Bash, where it is enabled automatically, plus opt-in elsewhere). Pipe
/// alternation in a PreToolUse matcher fires the group for either tool, so a
/// single group audits both — without `PowerShell`, a Windows session with no
/// Git Bash would run every shell command un-audited (the guard is the sole
/// gate now that the permissions injector suppresses Claude Code's native
/// prompt). The `PowerShell` tool carries its command under the same
/// `tool_input.command` field as `Bash`, so `fleet guard` needs no per-tool
/// parsing branch.
pub fn apply_guard_hook() -> Result<(), String> {
    crate::control_plane_prefs::note_intent(
        apply_guard_hook_inner(),
        crate::control_plane_prefs::Feature::GuardHook,
        false,
    )
}

fn apply_guard_hook_inner() -> Result<(), String> {
    let fleet_bin = resolve_publishable_fleet_binary()?;

    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let obj = settings
        .as_object_mut()
        .ok_or("settings is not an object")?;

    if !obj.contains_key("hooks") {
        obj.insert("hooks".into(), json!({}));
    }
    let hooks_obj = obj
        .get_mut("hooks")
        .and_then(|h| h.as_object_mut())
        .ok_or("hooks is not an object")?;

    let mut guard_hook = fleet_subcommand_hook(&fleet_bin, "guard");
    guard_hook["timeout"] = json!(120000);
    let guard_group = json!({
        "matcher": GUARD_MATCHER,
        "hooks": [guard_hook]
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
    crate::control_plane_prefs::note_intent(
        remove_guard_hook_inner(),
        crate::control_plane_prefs::Feature::GuardHook,
        true,
    )
}

fn remove_guard_hook_inner() -> Result<(), String> {
    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let Some(obj) = settings.as_object_mut() else {
        return Ok(());
    };
    let Some(hooks_obj) = obj.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return Ok(());
    };

    if let Some(arr) = hooks_obj
        .get_mut("PreToolUse")
        .and_then(|v| v.as_array_mut())
    {
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
    group_invokes_fleet_subcommand(group, "guard")
}

// ── Elicitation hook (AskUserQuestion interception) ─────────────────────

/// Install the elicitation hook (synchronous PreToolUse for AskUserQuestion).
pub fn apply_elicitation_hook() -> Result<(), String> {
    crate::control_plane_prefs::note_intent(
        apply_elicitation_hook_inner(),
        crate::control_plane_prefs::Feature::ElicitationHook,
        false,
    )
}

fn apply_elicitation_hook_inner() -> Result<(), String> {
    let fleet_bin = resolve_publishable_fleet_binary()?;

    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let obj = settings
        .as_object_mut()
        .ok_or("settings is not an object")?;

    if !obj.contains_key("hooks") {
        obj.insert("hooks".into(), json!({}));
    }
    let hooks_obj = obj
        .get_mut("hooks")
        .and_then(|h| h.as_object_mut())
        .ok_or("hooks is not an object")?;

    let mut elicitation_hook = fleet_subcommand_hook(&fleet_bin, "elicitation");
    elicitation_hook["timeout"] = json!(120000);
    let elicitation_group = json!({
        "matcher": "AskUserQuestion",
        "hooks": [elicitation_hook]
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
    crate::control_plane_prefs::note_intent(
        remove_elicitation_hook_inner(),
        crate::control_plane_prefs::Feature::ElicitationHook,
        true,
    )
}

fn remove_elicitation_hook_inner() -> Result<(), String> {
    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let Some(obj) = settings.as_object_mut() else {
        return Ok(());
    };
    let Some(hooks_obj) = obj.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return Ok(());
    };

    if let Some(arr) = hooks_obj
        .get_mut("PreToolUse")
        .and_then(|v| v.as_array_mut())
    {
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
    group_invokes_fleet_subcommand(group, "elicitation")
}

// ── Plan-approval hook (ExitPlanMode interception) ──────────────────────

/// Install the plan-approval hook (synchronous PreToolUse for ExitPlanMode).
pub fn apply_plan_approval_hook() -> Result<(), String> {
    crate::control_plane_prefs::note_intent(
        apply_plan_approval_hook_inner(),
        crate::control_plane_prefs::Feature::PlanApprovalHook,
        false,
    )
}

fn apply_plan_approval_hook_inner() -> Result<(), String> {
    let fleet_bin = resolve_publishable_fleet_binary()?;

    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let obj = settings
        .as_object_mut()
        .ok_or("settings is not an object")?;

    if !obj.contains_key("hooks") {
        obj.insert("hooks".into(), json!({}));
    }
    let hooks_obj = obj
        .get_mut("hooks")
        .and_then(|h| h.as_object_mut())
        .ok_or("hooks is not an object")?;

    let mut plan_approval_hook = fleet_subcommand_hook(&fleet_bin, "plan-approval");
    plan_approval_hook["timeout"] = json!(600000);
    let plan_approval_group = json!({
        "matcher": "ExitPlanMode",
        "hooks": [plan_approval_hook]
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
    crate::control_plane_prefs::note_intent(
        remove_plan_approval_hook_inner(),
        crate::control_plane_prefs::Feature::PlanApprovalHook,
        true,
    )
}

fn remove_plan_approval_hook_inner() -> Result<(), String> {
    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let Some(obj) = settings.as_object_mut() else {
        return Ok(());
    };
    let Some(hooks_obj) = obj.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return Ok(());
    };

    if let Some(arr) = hooks_obj
        .get_mut("PreToolUse")
        .and_then(|v| v.as_array_mut())
    {
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
    group_invokes_fleet_subcommand(group, "plan-approval")
}

// ── PRD-context hook (UserPromptSubmit injection of TASKS.md) ───────────

/// Install the PRD-context hook (UserPromptSubmit, no matcher) into
/// settings.json. The hook calls `fleet prd-context`, which reads the active
/// workspace's `TASKS.md` and emits it as additional context, so context
/// compression can't erase the macro plan.
pub fn apply_prd_context_hook() -> Result<(), String> {
    crate::control_plane_prefs::note_intent(
        apply_prd_context_hook_inner(),
        crate::control_plane_prefs::Feature::PrdContextHook,
        false,
    )
}

fn apply_prd_context_hook_inner() -> Result<(), String> {
    let fleet_bin = resolve_publishable_fleet_binary()?;

    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let obj = settings
        .as_object_mut()
        .ok_or("settings is not an object")?;

    if !obj.contains_key("hooks") {
        obj.insert("hooks".into(), json!({}));
    }
    let hooks_obj = obj
        .get_mut("hooks")
        .and_then(|h| h.as_object_mut())
        .ok_or("hooks is not an object")?;

    // UserPromptSubmit hooks have no matcher field — they run for every prompt.
    let mut prd_context_hook = fleet_subcommand_hook(&fleet_bin, "prd-context");
    prd_context_hook["timeout"] = json!(10000);
    let prd_context_group = json!({
        "hooks": [prd_context_hook]
    });

    if let Some(existing) = hooks_obj.get_mut("UserPromptSubmit") {
        if let Some(arr) = existing.as_array_mut() {
            arr.retain(|group| !is_prd_context_group(group));
            arr.push(prd_context_group);
        }
    } else {
        hooks_obj.insert("UserPromptSubmit".to_string(), json!([prd_context_group]));
    }

    // Companion: the notes-hint SessionStart hook. Fires when a context window
    // is (re)opened — after a compaction, on `--resume`, and on startup (a
    // handoff successor inherits its predecessor's notes) — and re-injects the
    // session's private checkpoint notes. Same feature as prd-context: both
    // exist so compaction can't erase what the agent was doing.
    let mut notes_hint_hook = fleet_subcommand_hook(&fleet_bin, "notes-hint");
    notes_hint_hook["timeout"] = json!(10000);
    let notes_hint_group = json!({
        "matcher": NOTES_HINT_MATCHER,
        "hooks": [notes_hint_hook]
    });
    if let Some(existing) = hooks_obj.get_mut("SessionStart") {
        if let Some(arr) = existing.as_array_mut() {
            arr.retain(|group| !is_notes_hint_group(group));
            arr.push(notes_hint_group);
        }
    } else {
        hooks_obj.insert("SessionStart".to_string(), json!([notes_hint_group]));
    }

    // Companion: the recent-sessions SessionStart hook. Same event, its own
    // entry — Claude Code keeps the `additionalContext` of every matching hook
    // and hands them to the model together, so this block gets its own byte
    // budget instead of eating into the notes summary's.
    //
    // Longer timeout than its neighbours because it pays for a full session
    // scan: about two seconds against a warm on-disk scan cache, but tens of
    // seconds on a machine that has never built one. Timing out costs the
    // block, not the session.
    let mut recent_sessions_hook = fleet_subcommand_hook(&fleet_bin, "recent-sessions");
    recent_sessions_hook["timeout"] = json!(20000);
    let recent_sessions_group = json!({
        "matcher": RECENT_SESSIONS_MATCHER,
        "hooks": [recent_sessions_hook]
    });
    if let Some(existing) = hooks_obj.get_mut("SessionStart") {
        if let Some(arr) = existing.as_array_mut() {
            arr.retain(|group| !is_recent_sessions_group(group));
            arr.push(recent_sessions_group);
        }
    } else {
        hooks_obj.insert("SessionStart".to_string(), json!([recent_sessions_group]));
    }

    // Companion: the context-pressure PostToolUse hook. Same feature for the
    // same reason — it exists so a session notices the window filling *before*
    // a compaction summarises its macro state away. It hangs off PostToolUse
    // rather than UserPromptSubmit because the sessions that fill a window
    // never come back for another prompt: a headless `-p` turn can run for
    // hours, and a tool call is the only event that recurs inside one.
    let mut ctx_reminder_hook = fleet_subcommand_hook(&fleet_bin, "ctx-reminder");
    ctx_reminder_hook["timeout"] = json!(10000);
    let ctx_reminder_group = json!({
        "hooks": [ctx_reminder_hook]
    });
    if let Some(existing) = hooks_obj.get_mut("PostToolUse") {
        if let Some(arr) = existing.as_array_mut() {
            arr.retain(|group| !is_ctx_reminder_group(group));
            arr.push(ctx_reminder_group);
        }
    } else {
        hooks_obj.insert("PostToolUse".to_string(), json!([ctx_reminder_group]));
    }

    write_settings(&settings)
}

/// `SessionStart` sources that open a context window whose model has not seen
/// the session's notes: `compact` (the case this exists for), `resume` (a
/// `claude --resume` — Fleet's auto-resume and relays), `startup` (a fresh
/// session; only matters for a handoff successor, which inherits notes).
/// `clear` is left out: the user asked for an empty slate. `fork` is left out
/// for the neighbouring reason — a forked session starts with its parent's
/// context, so whatever this hook would inject is already in front of it.
/// (Those five — `startup`, `resume`, `clear`, `compact`, `fork` — are the
/// whole set Claude Code emits.)
pub const NOTES_HINT_MATCHER: &str = "compact|resume|startup";

/// Sources the recent-sessions block fires on. Same set as
/// [`NOTES_HINT_MATCHER`] and for the same reasons — a context window with no
/// history of this workspace in it — kept as its own constant because the two
/// hooks answer to different features and either may need to move alone.
pub const RECENT_SESSIONS_MATCHER: &str = "compact|resume|startup";

/// Remove the PRD-context hook from settings.json.
pub fn remove_prd_context_hook() -> Result<(), String> {
    crate::control_plane_prefs::note_intent(
        remove_prd_context_hook_inner(),
        crate::control_plane_prefs::Feature::PrdContextHook,
        true,
    )
}

fn remove_prd_context_hook_inner() -> Result<(), String> {
    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let Some(obj) = settings.as_object_mut() else {
        return Ok(());
    };
    let Some(hooks_obj) = obj.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return Ok(());
    };

    if let Some(arr) = hooks_obj
        .get_mut("UserPromptSubmit")
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|group| !is_prd_context_group(group));
        if arr.is_empty() {
            hooks_obj.remove("UserPromptSubmit");
        }
    }
    if let Some(arr) = hooks_obj
        .get_mut("SessionStart")
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|group| !is_notes_hint_group(group) && !is_recent_sessions_group(group));
        if arr.is_empty() {
            hooks_obj.remove("SessionStart");
        }
    }
    if let Some(arr) = hooks_obj
        .get_mut("PostToolUse")
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|group| !is_ctx_reminder_group(group));
        if arr.is_empty() {
            hooks_obj.remove("PostToolUse");
        }
    }

    if hooks_obj.is_empty() {
        obj.remove("hooks");
    }

    write_settings(&settings)
}

/// All four parts must be present: the UserPromptSubmit injection, its two
/// SessionStart companions (notes hint, recent sessions), and the PostToolUse
/// context-pressure reminder. A
/// settings.json from a build that predates a companion therefore reads as
/// "not installed", which is what makes `control_plane::heal` add the missing
/// group instead of leaving upgraded hosts without it forever.
fn has_prd_context_hook(hooks_obj: &Map<String, Value>) -> bool {
    hooks_obj
        .get("UserPromptSubmit")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(|group| is_prd_context_group(group)))
        .unwrap_or(false)
        && has_notes_hint_hook(hooks_obj)
        && has_recent_sessions_hook(hooks_obj)
        && has_ctx_reminder_hook(hooks_obj)
}

fn has_ctx_reminder_hook(hooks_obj: &Map<String, Value>) -> bool {
    hooks_obj
        .get("PostToolUse")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(is_ctx_reminder_group))
        .unwrap_or(false)
}

fn is_ctx_reminder_group(group: &Value) -> bool {
    group_invokes_fleet_subcommand(group, "ctx-reminder")
}

fn is_prd_context_group(group: &Value) -> bool {
    group_invokes_fleet_subcommand(group, "prd-context")
}

fn has_notes_hint_hook(hooks_obj: &Map<String, Value>) -> bool {
    hooks_obj
        .get("SessionStart")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(is_notes_hint_group))
        .unwrap_or(false)
}

fn is_notes_hint_group(group: &Value) -> bool {
    group_invokes_fleet_subcommand(group, "notes-hint")
}

fn has_recent_sessions_hook(hooks_obj: &Map<String, Value>) -> bool {
    hooks_obj
        .get("SessionStart")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(is_recent_sessions_group))
        .unwrap_or(false)
}

fn is_recent_sessions_group(group: &Value) -> bool {
    group_invokes_fleet_subcommand(group, "recent-sessions")
}

// ── Wakeup guard hook (ScheduleWakeup / CronCreate interception) ────────

/// Install the wakeup guard (synchronous PreToolUse for the built-in
/// cross-turn schedulers). Installed and removed alongside the PRD-context
/// hook: it is the enforcement layer for PRD discipline's Rule 5, which tells
/// agents to relay via `fleet watch` / `fleet handoff` / `fleet loop` rather
/// than the built-ins that silently strand a Fleet turn.
pub fn apply_wakeup_guard_hook() -> Result<(), String> {
    crate::control_plane_prefs::note_intent(
        apply_wakeup_guard_hook_inner(),
        crate::control_plane_prefs::Feature::WakeupGuardHook,
        false,
    )
}

fn apply_wakeup_guard_hook_inner() -> Result<(), String> {
    let fleet_bin = resolve_publishable_fleet_binary()?;

    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let obj = settings
        .as_object_mut()
        .ok_or("settings is not an object")?;

    if !obj.contains_key("hooks") {
        obj.insert("hooks".into(), json!({}));
    }
    let hooks_obj = obj
        .get_mut("hooks")
        .and_then(|h| h.as_object_mut())
        .ok_or("hooks is not an object")?;

    // Short timeout: the decision is a local file check, not a user prompt.
    let mut wakeup_hook = fleet_subcommand_hook(&fleet_bin, "wakeup-guard");
    wakeup_hook["timeout"] = json!(5000);
    let wakeup_group = json!({
        "matcher": crate::wakeup_guard::WAKEUP_GUARD_MATCHER,
        "hooks": [wakeup_hook]
    });

    // Idempotent: strip any pre-existing fleet wakeup-guard groups (possibly
    // pointing at stale binary paths) before appending a fresh one.
    if let Some(existing) = hooks_obj.get_mut("PreToolUse") {
        if let Some(arr) = existing.as_array_mut() {
            arr.retain(|group| !is_wakeup_guard_group(group));
            arr.push(wakeup_group);
        }
    } else {
        hooks_obj.insert("PreToolUse".to_string(), json!([wakeup_group]));
    }

    write_settings(&settings)
}

/// Remove the wakeup guard from settings.json.
pub fn remove_wakeup_guard_hook() -> Result<(), String> {
    crate::control_plane_prefs::note_intent(
        remove_wakeup_guard_hook_inner(),
        crate::control_plane_prefs::Feature::WakeupGuardHook,
        true,
    )
}

fn remove_wakeup_guard_hook_inner() -> Result<(), String> {
    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let Some(obj) = settings.as_object_mut() else {
        return Ok(());
    };
    let Some(hooks_obj) = obj.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return Ok(());
    };

    if let Some(arr) = hooks_obj
        .get_mut("PreToolUse")
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|group| !is_wakeup_guard_group(group));
        if arr.is_empty() {
            hooks_obj.remove("PreToolUse");
        }
    }

    if hooks_obj.is_empty() {
        obj.remove("hooks");
    }

    write_settings(&settings)
}

fn is_wakeup_guard_group(group: &Value) -> bool {
    group_invokes_fleet_subcommand(group, "wakeup-guard")
}

fn has_wakeup_guard_hook(hooks_obj: &Map<String, Value>) -> bool {
    hooks_obj
        .get("PreToolUse")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(is_wakeup_guard_group))
        .unwrap_or(false)
}

// ── Idle hooks (Stop + UserPromptSubmit → kanban Pending sentinel) ──────

/// Install both idle hooks: `Stop` calls `fleet session idle` (marks the
/// kanban card Pending), `UserPromptSubmit` calls `fleet session resume`
/// (clears it back to Running on next prompt).
///
/// Coexists with the prd-context hook on UserPromptSubmit — markers are
/// distinct, retain-then-push only filters our own group.
pub fn apply_idle_hooks() -> Result<(), String> {
    crate::control_plane_prefs::note_intent(
        apply_idle_hooks_inner(),
        crate::control_plane_prefs::Feature::IdleHooks,
        false,
    )
}

fn apply_idle_hooks_inner() -> Result<(), String> {
    let fleet_bin = resolve_publishable_fleet_binary()?;

    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let obj = settings
        .as_object_mut()
        .ok_or("settings is not an object")?;

    if !obj.contains_key("hooks") {
        obj.insert("hooks".into(), json!({}));
    }
    let hooks_obj = obj
        .get_mut("hooks")
        .and_then(|h| h.as_object_mut())
        .ok_or("hooks is not an object")?;

    let mut stop_hook = fleet_subcommand_hook(&fleet_bin, "session idle");
    stop_hook["timeout"] = json!(5000);
    let stop_group = json!({
        "hooks": [stop_hook]
    });
    let mut resume_hook = fleet_subcommand_hook(&fleet_bin, "session resume");
    resume_hook["timeout"] = json!(5000);
    let resume_group = json!({
        "hooks": [resume_hook]
    });

    if let Some(existing) = hooks_obj.get_mut("Stop") {
        if let Some(arr) = existing.as_array_mut() {
            arr.retain(|group| !is_idle_stop_group(group));
            arr.push(stop_group);
        }
    } else {
        hooks_obj.insert("Stop".to_string(), json!([stop_group]));
    }

    if let Some(existing) = hooks_obj.get_mut("UserPromptSubmit") {
        if let Some(arr) = existing.as_array_mut() {
            arr.retain(|group| !is_idle_resume_group(group));
            arr.push(resume_group);
        }
    } else {
        hooks_obj.insert("UserPromptSubmit".to_string(), json!([resume_group]));
    }

    write_settings(&settings)
}

fn has_idle_hooks(hooks_obj: &Map<String, Value>) -> bool {
    let stop = hooks_obj
        .get("Stop")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(|group| is_idle_stop_group(group)))
        .unwrap_or(false);
    let resume = hooks_obj
        .get("UserPromptSubmit")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(|group| is_idle_resume_group(group)))
        .unwrap_or(false);
    stop && resume
}

fn is_idle_stop_group(group: &Value) -> bool {
    group_invokes_fleet_subcommand(group, "session idle")
}

fn is_idle_resume_group(group: &Value) -> bool {
    group_invokes_fleet_subcommand(group, "session resume")
}

// ── Default model (settings.json `model`) ───────────────────────────────────

/// Pin Claude Code's default model in `~/.claude/settings.json`.
///
/// A headless host has no interactive `/model` picker, and Fleet only passes
/// `--model` when the caller named one (`push_session_override_args`) — so a
/// spawn with no explicit model lands on whatever Claude Code itself defaults
/// to. `settings.json`'s `model` key is the only lever that moves that default,
/// and on the Fleet Cloud container `~/.claude` is on the ephemeral layer, so
/// the write has to happen on every start (`fleet bootstrap`, which the
/// entrypoint runs before serving).
///
/// Accepts either an alias (`opus`, `sonnet`) or a full id (`claude-opus-5`) —
/// the value is handed to Claude Code verbatim. A blank value is a no-op, which
/// is what leaves a host on the CLI's own default.
pub fn apply_default_model(model: &str) -> Result<(), String> {
    let model = model.trim();
    if model.is_empty() {
        return Ok(());
    }
    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let obj = settings
        .as_object_mut()
        .ok_or("settings is not an object")?;
    obj.insert("model".to_string(), json!(model));
    write_settings(&settings)
}

// ── Commit attribution (settings.json `attribution`) ────────────────────────

/// Turn off Claude Code's commit/PR bylines in `~/.claude/settings.json`.
///
/// Claude Code's own system prompt ends every commit message with
/// `Co-Authored-By: Claude … <noreply@anthropic.com>` (and a
/// `🤖 Generated with Claude Code` line in PR bodies), and web / Remote Control
/// sessions additionally paste a claude.ai session URL. That default is *not*
/// reachable from guidance text — `attribution` in settings.json is the only
/// lever, so a Fleet-governed host has to set it here alongside the hooks.
///
/// Inside `attribution`, two keys, both booleans:
/// - `commitTrailers` — the `Co-Authored-By` / `Generated with` trailers.
/// - `sessionUrl` — the claude.ai session link in web/Remote Control commits.
///
/// Plus the top-level `includeCoAuthoredBy`, the older spelling of
/// `commitTrailers`. Writing the new one alone is **not** enough: measured
/// against Claude Code 2.1.263 on 2026-09-10, `attribution.commitTrailers:
/// false` does not reach the system-prompt assembly, and a fresh session still
/// gets `End git commit messages with: Co-Authored-By: …`. The probe was a new
/// `claude -p` session asked to quote that line verbatim — with only
/// `attribution.commitTrailers` it quoted the trailer, with
/// `includeCoAuthoredBy: false` it answered `NONE`, and with both it answered
/// `NONE` (they do not conflict). So write both spellings until upstream wires
/// the new key up; dropping the old one silently re-enables the byline.
///
/// Merges into an existing `attribution` object rather than replacing it, so a
/// future key someone set by hand survives. Claude Code reads settings.json at
/// startup, so this only affects sessions spawned after the write.
pub fn apply_no_commit_attribution() -> Result<(), String> {
    let mut settings = read_settings().unwrap_or_else(|| json!({}));
    let obj = settings
        .as_object_mut()
        .ok_or("settings is not an object")?;
    obj.insert("includeCoAuthoredBy".to_string(), json!(false));
    let attribution = obj
        .entry("attribution".to_string())
        .or_insert_with(|| json!({}));
    if !attribution.is_object() {
        *attribution = json!({});
    }
    let attribution = attribution
        .as_object_mut()
        .ok_or("attribution is not an object")?;
    attribution.insert("commitTrailers".to_string(), json!(false));
    attribution.insert("sessionUrl".to_string(), json!(false));
    write_settings(&settings)
}

/// Whether [`apply_no_commit_attribution`] has already been applied.
///
/// Exists so `control_plane::heal` can stay silent on a host that is already
/// whole — it prints the steps it ran, and an unconditional write would put a
/// line on every `fleet webui` start. Deliberately *not* a
/// [`HookSetupPlan`] field: that struct is the settings panel's toggle list, and
/// this is a value with no on/off UI, like the pinned default model.
pub fn no_commit_attribution_applied() -> bool {
    let Some(settings) = read_settings() else {
        return false;
    };
    let Some(attribution) = settings.get("attribution") else {
        return false;
    };
    attribution.get("commitTrailers").and_then(|v| v.as_bool()) == Some(false)
        && attribution.get("sessionUrl").and_then(|v| v.as_bool()) == Some(false)
        // The old spelling is the one Claude Code actually honours, so a host
        // that only has the new key is *not* whole — heal must rewrite it.
        && settings
            .get("includeCoAuthoredBy")
            .and_then(|v| v.as_bool())
            == Some(false)
}

// ── Read hook events ─────────────────────────────────────────────────────────

/// Everything the session scan derives from one pass over the hook events.
///
/// Bundled because `read_recent_events` reads the whole `hooks.jsonl` to keep
/// its tail, and that file runs to tens of megabytes on a busy machine — the
/// scan must not pay for it twice per tick.
#[derive(Debug, Default, Clone)]
pub struct HookSnapshot {
    /// session_id → derived agent state.
    pub states: HashMap<String, HookState>,
    /// session_id → the background tasks that were still running the last time
    /// the session ended a turn. Empty for sessions with nothing outstanding.
    pub background_tasks: HashMap<String, Vec<crate::bg_guard::BackgroundTask>>,
}

/// Read the hook events file and compute per-session HookState.
/// Returns a map from session_id to the derived state.
pub fn read_hook_states() -> HashMap<String, HookState> {
    read_hook_snapshot().states
}

/// How long a session's last hook event still describes what it is doing.
const HOOK_STATE_MAX_AGE_MS: u64 = 300_000;

/// Lines the first read of a process seeds itself from. Later reads only
/// consume the bytes appended since, so this is a one-off cost, not a window.
const HOOK_SEED_LINES: usize = 500;

/// What one session's last hook event said, and when this process saw it.
struct SessionHookState {
    state: HookState,
    /// Ingest time, not a field of the record: Claude Code's hook payloads carry
    /// no timestamp at all (see `read_recent_events`). Stamping on the way in is
    /// what finally gives the freshness gate a real clock — the old code dated
    /// every record by the file's mtime, which on a busy machine is always
    /// "now", so nothing ever aged out and *position in the file* was the only
    /// thing bounding staleness.
    seen_ms: u64,
    /// Background tasks still running as of that event. Only `Stop` carries any.
    background_tasks: Vec<crate::bg_guard::BackgroundTask>,
}

/// Incremental follow state for `hooks.jsonl`, kept for the life of the process.
///
/// Reading a fixed tail window on every scan made a session's hook state a
/// function of *machine load*: `hooks.jsonl` is machine-wide, so a session quiet
/// inside a long tool slid out of the last 500 lines as busier siblings appended
/// (measured 2026-09-17: ~56 events/min across 11 sessions, i.e. the window held
/// about 9 minutes), and its phase was simply forgotten. Following the file
/// forward instead means a session's last event is remembered until a newer one
/// replaces it or it ages out — the same answer no matter what the neighbours
/// are doing.
struct HookTail {
    /// Byte offset of the first unconsumed byte, or `None` before the first read.
    offset: Option<u64>,
    /// The file `offset` belongs to. A test (or anything else) that repoints
    /// `HOME` mid-process must not have its offset applied to a different file.
    path: Option<PathBuf>,
    states: HashMap<String, SessionHookState>,
}

static HOOK_TAIL: std::sync::LazyLock<std::sync::Mutex<HookTail>> =
    std::sync::LazyLock::new(|| {
        std::sync::Mutex::new(HookTail {
            offset: None,
            path: None,
            states: HashMap::new(),
        })
    });

/// One pass over the hook events → both the state map and the outstanding
/// background tasks per session.
pub fn read_hook_snapshot() -> HookSnapshot {
    let Some(path) = hooks_events_path() else {
        return HookSnapshot::default();
    };

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut tail = match HOOK_TAIL.lock() {
        Ok(t) => t,
        // A panic in another reader must not take the scan down with it.
        Err(poisoned) => poisoned.into_inner(),
    };
    tail.follow(&path, now_ms);
    tail.snapshot(now_ms)
}

impl HookTail {
    /// Consume whatever was appended since the last call and fold it in.
    fn follow(&mut self, path: &Path, now_ms: u64) {
        let len = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if self.path.as_deref() != Some(path) {
            self.offset = None;
            self.path = Some(path.to_path_buf());
        }
        match self.offset {
            // First read of this process, or the file shrank under us
            // (`maybe_truncate_events_file` rewrites it to its last 2000 lines):
            // seed from the tail and follow forward from there.
            None => self.seed(path, len, now_ms),
            Some(prev) if len < prev => self.seed(path, len, now_ms),
            Some(prev) => {
                let (events, consumed) = read_events_from(path, prev, len);
                for ev in events {
                    self.ingest(ev, now_ms);
                }
                self.offset = Some(consumed);
            }
        }
    }

    fn seed(&mut self, path: &Path, len: u64, now_ms: u64) {
        for ev in read_recent_events(path, HOOK_SEED_LINES) {
            self.ingest(ev, now_ms);
        }
        // Resume from the last complete record, not from `len`: a hook caught
        // mid-append would otherwise have its first bytes consumed here and its
        // tail parsed as a line of its own, losing the event entirely.
        //
        // `len` is sampled *before* the tail read, so anything appended in
        // between is re-read next time rather than skipped. Re-ingesting an
        // event is harmless: it just re-asserts the state it already set.
        self.offset = Some(end_of_last_record(path, len));
    }

    /// Drop states too old to describe the present, then publish what is left.
    fn snapshot(&mut self, now_ms: u64) -> HookSnapshot {
        self.states
            .retain(|_, s| now_ms.saturating_sub(s.seen_ms) <= HOOK_STATE_MAX_AGE_MS);

        let mut snapshot = HookSnapshot::default();
        for (sid, s) in self.states.iter() {
            if !s.background_tasks.is_empty() {
                snapshot
                    .background_tasks
                    .insert(sid.clone(), s.background_tasks.clone());
            }
            snapshot.states.insert(sid.clone(), s.state.clone());
        }
        snapshot
    }

    /// Fold one event into the per-session state, overwriting whatever the
    /// session's previous event said — including its background tasks, since
    /// only the latest event describes the session's present.
    fn ingest(&mut self, ev: HookEvent, now_ms: u64) {
        let state = match ev.event_name.as_str() {
            // A PreToolUse for a tool that exists to *ask the user something*
            // is not work in flight — the session is parked on a decision card
            // or a permission prompt until someone answers. Reported as its own
            // state so the status machine can say "waiting for input" instead of
            // "running tools". (Under the old tail window this mostly sorted
            // itself out by accident: the event was evicted before anyone
            // looked. Following the file forward removes that accident, so the
            // distinction has to be made explicitly.)
            "PreToolUse" => {
                if crate::session::detect::is_interactive_wait_tool(
                    ev.tool_name.as_deref().unwrap_or(""),
                ) {
                    HookState::AwaitingUserInput
                } else {
                    HookState::ToolExecuting
                }
            }
            "PostToolUse" | "PostToolUseFailure" => HookState::ModelProcessing,
            "Stop" | "SubagentStop" => HookState::Stopped,
            _ => HookState::Unknown,
        };
        let background_tasks = ev
            .background_tasks
            .iter()
            .filter(|t| t.is_running())
            .cloned()
            .collect();
        self.states.insert(
            ev.session_id,
            SessionHookState {
                state,
                seen_ms: now_ms,
                background_tasks,
            },
        );
    }
}

/// Offset just past the last newline at or before `len`, i.e. the start of the
/// record currently being appended (or `len` itself when the file ends cleanly).
fn end_of_last_record(path: &Path, len: u64) -> u64 {
    use std::io::{Read, Seek, SeekFrom};

    const LOOKBACK: u64 = 64 * 1024;
    if len == 0 {
        return 0;
    }
    let Ok(mut f) = fs::File::open(path) else {
        return len;
    };
    let from = len.saturating_sub(LOOKBACK);
    if f.seek(SeekFrom::Start(from)).is_err() {
        return len;
    }
    let mut buf = vec![0u8; (len - from) as usize];
    if f.read_exact(&mut buf).is_err() {
        return len;
    }
    match buf.iter().rposition(|&b| b == b'\n') {
        Some(i) => from + i as u64 + 1,
        // No newline within the lookback: either a single enormous partial
        // record or a file with no line breaks at all. Re-reading it is
        // cheaper than losing it.
        None => from,
    }
}

/// Parse the whole lines in `[from, to)` of the events file.
///
/// Returns the events plus the offset just past the last newline consumed — a
/// record still being appended stays unconsumed and is picked up next read
/// rather than parsed in half.
fn read_events_from(path: &Path, from: u64, to: u64) -> (Vec<HookEvent>, u64) {
    use std::io::{Read, Seek, SeekFrom};

    if to <= from {
        return (Vec::new(), from);
    }
    let Ok(mut f) = fs::File::open(path) else {
        return (Vec::new(), from);
    };
    if f.seek(SeekFrom::Start(from)).is_err() {
        return (Vec::new(), from);
    }
    let mut buf = vec![0u8; (to - from) as usize];
    if f.read_exact(&mut buf).is_err() {
        return (Vec::new(), from);
    }
    let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') else {
        return (Vec::new(), from);
    };
    let text = String::from_utf8_lossy(&buf[..=last_nl]);
    let events = text.lines().filter_map(parse_event_line).collect();
    (events, from + last_nl as u64 + 1)
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
    // Create ~/.claude if it's absent — a fresh host (e.g. the Fleet Cloud
    // container's ephemeral layer on first boot) has no ~/.claude yet, and
    // fs::write does not create parent dirs. Without this the hook installers
    // (guard / elicitation / plan-approval) fail with ENOENT.
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create settings dir: {e}"))?;
    }
    let content =
        serde_json::to_string_pretty(value).map_err(|e| format!("serialize settings: {e}"))?;
    fs::write(&path, content + "\n").map_err(|e| format!("write settings: {e}"))
}

/// Build the Fleet hook group object.
///
/// Unix appends the raw hook JSON via the `sh -c 'cat >> …'` one-liner. On
/// Windows that string only runs when Git Bash is installed (Claude Code falls
/// back to PowerShell otherwise), so emit the shell-free exec form invoking
/// `fleet hook-event` — the CLI equivalent that appends stdin to
/// `~/.fleet/hooks.jsonl`. When the fleet binary can't be resolved on Windows
/// the sh form is still written (same best-effort behavior as before).
fn fleet_hook_group() -> Value {
    #[cfg(windows)]
    if let Some(bin) = resolve_fleet_binary() {
        return json!({
            "hooks": [{
                "type": "command",
                "command": bin,
                "args": ["hook-event"],
                "async": true
            }]
        });
    }
    json!({
        "hooks": [{
            "type": "command",
            "command": FLEET_HOOK_COMMAND,
            "async": true
        }]
    })
}

// ── Binary-path drift ────────────────────────────────────────────────────────

/// The `(fleet binary, subcommand)` a hook entry names, for either shape
/// [`fleet_subcommand_hook_with`] writes. `None` for anything that is not a
/// `fleet <subcommand>` hook — notably the `cat >> ~/.fleet/hooks.jsonl`
/// one-liner, which names no binary and therefore cannot drift.
fn hook_fleet_invocation(hook: &Value) -> Option<(String, String)> {
    let cmd = hook.get("command").and_then(|c| c.as_str())?;

    // Unix sh-wrapper: `sh -c 'if [ -x "{bin}" ]; then exec "{bin}" {sub}; else exit 0; fi'`
    if let Some(rest) = cmd.split_once("then exec \"").map(|(_, r)| r) {
        let (bin, rest) = rest.split_once('"')?;
        let sub = rest.split_once(';')?.0.trim();
        if bin.is_empty() || sub.is_empty() {
            return None;
        }
        return Some((bin.to_string(), sub.to_string()));
    }

    // Windows exec form: `command` is the binary, `args` are the subcommand
    // tokens. Split the basename by hand — `Path::file_stem` only treats `\`
    // as a separator on Windows, and a Windows-written settings.json has to be
    // recognized when this runs on any host.
    let base = cmd
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(cmd)
        .to_ascii_lowercase();
    if base != "fleet" && base != "fleet.exe" {
        return None;
    }
    let args: Vec<&str> = hook
        .get("args")?
        .as_array()?
        .iter()
        .map(|v| v.as_str().unwrap_or(""))
        .collect();
    if args.is_empty() || args.iter().any(|a| a.is_empty()) {
        return None;
    }
    Some((cmd.to_string(), args.join(" ")))
}

/// Rewrite every Fleet hook in `hooks_obj` to name `fleet_bin`, returning how
/// many entries actually changed. Pure, so the drift logic is testable without
/// touching a real `settings.json`.
fn repoint_fleet_hooks_in(hooks_obj: &mut Map<String, Value>, fleet_bin: &str) -> usize {
    let mut changed = 0;
    for (_event, groups) in hooks_obj.iter_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        for group in groups.iter_mut() {
            let Some(entries) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) else {
                continue;
            };
            for entry in entries.iter_mut() {
                let Some((bin, sub)) = hook_fleet_invocation(entry) else {
                    continue;
                };
                if bin == fleet_bin {
                    continue;
                }
                // Keep the shape already on disk: a Windows settings.json
                // carries exec form, a unix one the sh wrapper, and a machine
                // must not be handed the other platform's shape just because
                // the path drifted.
                let was_exec_form = entry.get("args").is_some();
                let mut fresh = fleet_subcommand_hook_with(was_exec_form, fleet_bin, &sub);
                // Preserve per-entry settings the appliers add (`timeout`,
                // `async`) — this rewrites the path, nothing else.
                if let (Some(fresh_obj), Some(old_obj)) = (fresh.as_object_mut(), entry.as_object())
                {
                    for (k, v) in old_obj {
                        if k != "command" && k != "args" && k != "type" {
                            fresh_obj.insert(k.clone(), v.clone());
                        }
                    }
                }
                *entry = fresh;
                changed += 1;
            }
        }
    }
    changed
}

/// Point every Fleet hook in `settings.json` at the fleet binary this machine
/// resolves *now*, and report how many entries moved.
///
/// Hook commands bake an absolute path, and nothing ever rewrote it: the
/// appliers replace a hook wholesale, but they only run when a feature is
/// installed or toggled, and `control_plane::heal` skips anything already
/// present — a check that reads the *subcommand*, never the path. So a hook
/// installed by a `./target/debug/fleet` keeps naming that build forever,
/// and a machine ends up with its hooks split across several binaries of
/// different ages. Measured on the author's Mac on 2026-09-14: eight Fleet
/// hooks across three different binaries, one of which no longer knew the
/// subcommand it was pointed at.
///
/// Path-only: which features are installed is not this function's business, so
/// it adds and removes nothing. Safe and cheap to run on every startup — it
/// writes only when something actually changed.
pub fn repoint_fleet_hooks() -> Result<usize, String> {
    // Nothing publishable to point at — including a worktree build, which would
    // move every hook onto a path that disappears at merge. Leaving the
    // existing paths alone is strictly better than rewriting them to a guess.
    let Ok(fleet_bin) = resolve_publishable_fleet_binary() else {
        return Ok(0);
    };
    let Some(mut settings) = read_settings() else {
        return Ok(0);
    };
    let Some(hooks_obj) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return Ok(0);
    };
    let changed = repoint_fleet_hooks_in(hooks_obj, &fleet_bin);
    if changed == 0 {
        return Ok(0);
    }
    write_settings(&settings)?;
    Ok(changed)
}

/// Check whether a given event already has a Fleet hook group.
fn has_fleet_hook(hooks_obj: &Map<String, Value>, event: &str) -> bool {
    hooks_obj
        .get(event)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(|group| is_fleet_group(group)))
        .unwrap_or(false)
}

/// Check whether a hook group object is ours — either the `cat >>` shell
/// one-liner (identified by its `.fleet/hooks.jsonl` target) or the Windows
/// exec form invoking `fleet hook-event`.
fn is_fleet_group(group: &Value) -> bool {
    let cat_shape = group
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
        .unwrap_or(false);
    cat_shape || group_invokes_fleet_subcommand(group, "hook-event")
}

/// What a `fleet` build should exit with when handed a subcommand it has never
/// heard of. `true` means the caller is a human at a terminal.
///
/// [`fault_tolerant_command`] guards against the fleet binary being *missing*.
/// It cannot guard against the binary being merely *older* than the
/// `settings.json` that names it — that binary exists, runs, and dies on
/// clap's usage error (exit 2). And exit 2 from a hook is not cosmetic.
/// Measured on Claude Code 2.1.263 (temp dir + a `settings.local.json` hook
/// that just `exit 2`s, driven by `claude -p --output-format stream-json`):
///
/// - **PreToolUse** — the tool call is *denied* and lands in the result's
///   `permission_denials`. Fleet's `guard` matches `Bash|PowerShell`, so this
///   refuses every shell command on the machine.
/// - **UserPromptSubmit** — the prompt never reaches the model at all (zero
///   turns), yet the run still reports `subtype: "success", is_error: false`.
///   Two Fleet hooks live here (`prd-context`, `session resume`), and a
///   headless spawn — a handoff successor, a `fleet loop` tick — looks like it
///   succeeded while having done nothing.
/// - **Stop** — the session can never end: the failure is fed back to the
///   model as `Stop hook feedback` forever, bounded only by `--max-turns`.
/// - PostToolUse / SessionStart — harmless.
///
/// So an unknown subcommand must fail *open* when it arrived from a hook.
/// Piped stdin is the discriminator: Claude Code always feeds hook JSON on
/// stdin and a person at a terminal never does. Known subcommands never reach
/// here, so a deliberate `echo … | fleet guard` is untouched — and the human
/// typo (`fleet agnts`) still gets clap's error.
///
/// This cannot be solved in the wrapper string instead: `guard`,
/// `elicitation`, `plan-approval` and `wakeup-guard` all use exit 2 as their
/// *intended* "block this" signal, so a wrapper that swallows exit 2 would
/// disarm them. Only the binary itself knows which of the two it meant.
pub fn unknown_subcommand_exit_code(stdin_is_terminal: bool) -> i32 {
    if stdin_is_terminal {
        2
    } else {
        0
    }
}

/// Build a fault-tolerant shell command that silently exits 0 when the fleet
/// binary is missing (e.g. after uninstall), so Claude Code is not blocked.
/// When the binary exists, it `exec`s into it — propagating its exit code and
/// stdout/stderr as normal.
///
/// Note this only covers a *missing* binary; a stale one that no longer knows
/// the subcommand is handled inside the binary, by
/// [`unknown_subcommand_exit_code`].
fn fault_tolerant_command(fleet_bin: &str, subcommand: &str) -> String {
    // Use `test -x` so it works even if the binary was removed from PATH but
    // the absolute path is stale.  `exec` avoids an extra shell process.
    format!(
        r#"sh -c 'if [ -x "{bin}" ]; then exec "{bin}" {sub}; else exit 0; fi'"#,
        bin = fleet_bin,
        sub = subcommand,
    )
}

/// Build the hook entry that runs `fleet <subcommand…>`.
///
/// Unix keeps the [`fault_tolerant_command`] `sh -c` wrapper (silently exits 0
/// when the binary is gone, so an uninstalled Fleet never blocks Claude Code).
/// Windows cannot rely on that string: Claude Code runs hook command strings
/// through Git Bash only when it is installed and falls back to PowerShell
/// otherwise, where `sh -c '…'` is a parse error and every Fleet hook dies
/// silently. The exec form (`command` + `args`, hooks.md "Command Hook
/// Fields") bypasses the shell entirely — the exe is spawned directly with the
/// hook JSON on stdin, identical to shell form — so it works regardless of
/// which shell Claude Code would have picked. `fleet.exe` satisfies exec
/// form's real-executable requirement. The trade-off is no missing-binary
/// fault tolerance on Windows; the published `~/.fleet/bin/fleet.exe` copy is
/// what the hooks point at, and hook spawn failures are non-blocking.
fn fleet_subcommand_hook(fleet_bin: &str, subcommand: &str) -> Value {
    crate::log_debug(&format!(
        "hooks: emitting `fleet {subcommand}` hook as {} (bin={fleet_bin})",
        if cfg!(windows) {
            "exec form"
        } else {
            "sh wrapper"
        },
    ));
    fleet_subcommand_hook_with(cfg!(windows), fleet_bin, subcommand)
}

/// Pure core of [`fleet_subcommand_hook`] — the `windows` flag stands in for
/// `cfg!(windows)` so both shapes are unit-testable on any host.
fn fleet_subcommand_hook_with(windows: bool, fleet_bin: &str, subcommand: &str) -> Value {
    if windows {
        json!({
            "type": "command",
            "command": fleet_bin,
            "args": subcommand.split_whitespace().collect::<Vec<_>>(),
        })
    } else {
        json!({
            "type": "command",
            "command": fault_tolerant_command(fleet_bin, subcommand),
        })
    }
}

/// Does this hook entry invoke `fleet <subcommand…>`? Recognizes both shapes
/// from [`fleet_subcommand_hook_with`]; both are checked on every platform so
/// a settings.json carrying the other platform's shape (or a pre-upgrade one)
/// is still recognized and replaced idempotently rather than duplicated.
fn hook_invokes_fleet_subcommand(hook: &Value, subcommand: &str) -> bool {
    let Some(cmd) = hook.get("command").and_then(|c| c.as_str()) else {
        return false;
    };
    // Unix sh-wrapper shape: `… exec "{bin}" {sub}; else …` — the char before
    // the subcommand is the closing quote around the binary path.
    if cmd.contains(&format!("\" {subcommand};")) {
        return true;
    }
    // Windows exec shape: bare fleet binary path + exact args tokens. Split
    // the basename by hand — `Path::file_stem` treats `\` as a separator only
    // on Windows, and this matcher must recognize a Windows-written
    // settings.json on every platform.
    let base = cmd
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(cmd)
        .to_ascii_lowercase();
    let is_fleet_bin = base == "fleet" || base == "fleet.exe";
    is_fleet_bin
        && hook
            .get("args")
            .and_then(|a| a.as_array())
            .is_some_and(|args| {
                args.iter()
                    .map(|v| v.as_str().unwrap_or(""))
                    .eq(subcommand.split_whitespace())
            })
}

/// Group-level wrapper over [`hook_invokes_fleet_subcommand`].
fn group_invokes_fleet_subcommand(group: &Value, subcommand: &str) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter()
                .any(|hook| hook_invokes_fleet_subcommand(hook, subcommand))
        })
        .unwrap_or(false)
}

/// Read the last `max_lines` lines of a file without loading all of it.
///
/// `hooks.jsonl` is append-only and reaches tens of megabytes on a busy machine
/// (72 MB when this was written), while the session scan wants only its tail —
/// and re-reads it on every tick. Slurping the whole file to keep the last 500
/// lines was throwing away >99% of the bytes it read.
///
/// Walks backwards in chunks until it has counted more than `max_lines`
/// newlines. That overshoot is what makes the result safe: a chunk boundary can
/// land mid-line (and mid-UTF-8-sequence), but only ever at the *front* of the
/// buffer, and the extra newline guarantees that partial first line is dropped
/// by the `saturating_sub` below rather than parsed. When the walk reaches
/// offset 0 the whole file is in hand and every line is intact by construction.
fn read_tail_lines(path: &Path, max_lines: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};

    const CHUNK: u64 = 64 * 1024;

    let Ok(mut f) = fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(len) = f.seek(SeekFrom::End(0)) else {
        return Vec::new();
    };

    let mut buf: Vec<u8> = Vec::new();
    let mut pos = len;
    let mut newlines = 0usize;

    while pos > 0 && newlines <= max_lines {
        let read_size = CHUNK.min(pos);
        pos -= read_size;

        let mut chunk = vec![0u8; read_size as usize];
        if f.seek(SeekFrom::Start(pos)).is_err() || f.read_exact(&mut chunk).is_err() {
            break;
        }
        newlines += chunk.iter().filter(|&&b| b == b'\n').count();

        chunk.extend_from_slice(&buf);
        buf = chunk;
    }

    // Lossy only where a chunk boundary split a multi-byte char — always in the
    // partial first line, which the tail slice discards.
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].iter().map(|s| (*s).to_string()).collect()
}

/// Read the last `max_lines` from the events file and parse them.
fn read_recent_events(path: &Path, max_lines: usize) -> Vec<HookEvent> {
    let lines = read_tail_lines(path, max_lines);
    if lines.is_empty() {
        return Vec::new();
    }

    // Claude Code's hook payloads do NOT carry a "timestamp" field, so the
    // per-record lookup below almost always misses. Use the file's mtime as
    // the upper-bound fallback: it equals the time of the most recent
    // append, which is a safe over-estimate for every record in the file.
    // The 5-minute freshness gate in `read_hook_states` still expires an
    // untouched hooks.jsonl correctly.
    let file_mtime_ms: u64 = fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    lines
        .iter()
        .filter_map(|line| {
            let mut ev = parse_event_line(line)?;
            if ev.timestamp_ms == 0 {
                ev.timestamp_ms = file_mtime_ms;
            }
            Some(ev)
        })
        .collect()
}

/// Parse one `hooks.jsonl` line. `timestamp_ms` is 0 when the record carries no
/// usable timestamp, which is the normal case — callers supply their own clock.
fn parse_event_line(line: &str) -> Option<HookEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    let session_id = v.get("session_id")?.as_str()?.to_string();
    let event_name = v.get("hook_event_name")?.as_str()?.to_string();

    let timestamp_ms = v
        .get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.timestamp_millis() as u64)
        })
        .unwrap_or(0);

    let tool_name = v
        .get("tool_name")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string());

    // Present on Stop payloads only; a CLI older than 2.1.145 omits it.
    let background_tasks = v
        .get("background_tasks")
        .and_then(|t| serde_json::from_value(t.clone()).ok())
        .unwrap_or_default();

    Some(HookEvent {
        session_id,
        event_name,
        timestamp_ms,
        tool_name,
        background_tasks,
    })
}

#[cfg(test)]
mod fleet_subcommand_hook_tests {
    use super::*;

    #[test]
    fn unix_shape_is_the_fault_tolerant_sh_wrapper() {
        let hook = fleet_subcommand_hook_with(false, "/usr/local/bin/fleet", "guard");
        let cmd = hook["command"].as_str().unwrap();
        assert!(
            cmd.starts_with("sh -c"),
            "unix shape must stay sh-wrapped: {cmd}"
        );
        assert!(
            cmd.contains("\" guard;"),
            "sh wrapper must carry the subcommand: {cmd}"
        );
        assert!(
            hook.get("args").is_none(),
            "unix shape must not carry exec-form args"
        );
    }

    #[test]
    fn windows_shape_is_exec_form_with_split_args() {
        let hook =
            fleet_subcommand_hook_with(true, r"C:\Users\foo\.fleet\bin\fleet.exe", "session idle");
        assert_eq!(hook["command"], r"C:\Users\foo\.fleet\bin\fleet.exe");
        assert_eq!(hook["args"], json!(["session", "idle"]));
    }

    #[test]
    fn matcher_recognizes_both_shapes_and_distinguishes_subcommands() {
        for windows in [false, true] {
            let bin = if windows {
                r"C:\x\fleet.exe"
            } else {
                "/usr/local/bin/fleet"
            };
            let hook = fleet_subcommand_hook_with(windows, bin, "session idle");
            assert!(
                hook_invokes_fleet_subcommand(&hook, "session idle"),
                "windows={windows}: own subcommand must match"
            );
            assert!(
                !hook_invokes_fleet_subcommand(&hook, "session resume"),
                "windows={windows}: sibling subcommand must not match"
            );
            assert!(
                !hook_invokes_fleet_subcommand(&hook, "guard"),
                "windows={windows}: unrelated subcommand must not match"
            );
        }
    }

    #[test]
    fn exec_shape_requires_the_fleet_binary() {
        // Same args under a different exe is someone else's hook.
        let foreign = json!({"type": "command", "command": r"C:\x\other.exe", "args": ["guard"]});
        assert!(!hook_invokes_fleet_subcommand(&foreign, "guard"));
    }

    #[test]
    fn is_guard_group_accepts_the_windows_exec_shape() {
        let group = json!({
            "matcher": "Bash",
            "hooks": [fleet_subcommand_hook_with(true, r"C:\x\fleet.exe", "guard")]
        });
        assert!(is_guard_group(&group));
    }

    #[test]
    fn repoint_moves_every_shape_onto_the_current_binary_and_spares_the_rest() {
        // A settings.json in the state this Mac was actually found in on
        // 2026-09-14: Fleet hooks spread over three binaries of different ages,
        // in both shapes, next to a user's own hook and the `cat >>` event
        // logger (which names no binary and must not be touched).
        let mut hooks_obj = json!({
            "PreToolUse": [
                {"matcher": "Bash|PowerShell", "hooks": [{
                    "type": "command",
                    "command": fault_tolerant_command("/old/path/fleet", "guard"),
                    "timeout": 120000
                }]},
                {"matcher": "ScheduleWakeup", "hooks": [{
                    "type": "command",
                    "command": "C:\\Users\\x\\.fleet\\bin\\fleet.exe",
                    "args": ["wakeup-guard"]
                }]},
                {"matcher": "Bash", "hooks": [{
                    "type": "command",
                    "command": "my-own-linter --check"
                }]}
            ],
            "Stop": [
                {"hooks": [{"type": "command", "command": FLEET_HOOK_COMMAND, "async": true}]},
                {"hooks": [{
                    "type": "command",
                    "command": fault_tolerant_command("/new/fleet", "session idle")
                }]}
            ]
        })
        .as_object()
        .unwrap()
        .clone();

        let moved = repoint_fleet_hooks_in(&mut hooks_obj, "/new/fleet");
        // guard + wakeup-guard moved; `session idle` was already current, the
        // user's linter and the `cat >>` logger are not ours.
        assert_eq!(moved, 2, "moved the wrong number of hooks");

        let pre = &hooks_obj["PreToolUse"];
        let guard = &pre[0]["hooks"][0];
        assert_eq!(
            guard["command"].as_str().unwrap(),
            fault_tolerant_command("/new/fleet", "guard"),
            "the unix wrapper should be rewritten in place"
        );
        assert_eq!(
            guard["timeout"], 120000,
            "rewriting the path must not drop the entry's timeout"
        );

        let wakeup = &pre[1]["hooks"][0];
        assert_eq!(
            wakeup["command"], "/new/fleet",
            "a Windows exec-form entry must stay exec form, just repointed"
        );
        assert_eq!(wakeup["args"], json!(["wakeup-guard"]));

        assert_eq!(
            pre[2]["hooks"][0]["command"], "my-own-linter --check",
            "a hook that is not Fleet's must be left alone"
        );
        assert_eq!(
            hooks_obj["Stop"][0]["hooks"][0]["command"], FLEET_HOOK_COMMAND,
            "the `cat >>` event logger names no binary and cannot drift"
        );

        // Idempotent: a second pass has nothing left to do.
        assert_eq!(repoint_fleet_hooks_in(&mut hooks_obj, "/new/fleet"), 0);
    }

    #[test]
    fn hook_fleet_invocation_reads_back_what_the_appliers_write() {
        // Round-trip guard: if the emitted shape ever changes, the drift
        // parser must change with it or repointing silently stops working.
        for sub in ["guard", "prd-context", "session idle", "hook-event"] {
            for windows in [false, true] {
                let hook = fleet_subcommand_hook_with(windows, "/some/fleet", sub);
                assert_eq!(
                    hook_fleet_invocation(&hook),
                    Some(("/some/fleet".to_string(), sub.to_string())),
                    "could not read back {sub} (windows={windows})"
                );
            }
        }
    }

    #[test]
    fn unknown_subcommand_fails_open_for_hooks_and_keeps_erroring_for_humans() {
        // A hook: Claude Code pipes the event JSON in, so stdin is not a tty.
        // Exit 0 or PreToolUse denies the tool / UserPromptSubmit eats the
        // prompt / Stop never lets the session end.
        assert_eq!(unknown_subcommand_exit_code(false), 0);
        // A person who typo'd a subcommand still gets clap's usage error —
        // failing open there would hide real mistakes.
        assert_eq!(unknown_subcommand_exit_code(true), 2);
    }

    #[test]
    fn is_fleet_group_accepts_the_hook_event_exec_shape() {
        let group = json!({
            "hooks": [{
                "type": "command",
                "command": r"C:\Users\foo\.fleet\bin\fleet.exe",
                "args": ["hook-event"],
                "async": true
            }]
        });
        assert!(is_fleet_group(&group));
        // The unix cat-append one-liner keeps matching too.
        assert!(is_fleet_group(&fleet_hook_group()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Builds the same group JSON as `apply_guard_hook` would emit.
    fn guard_group_for(bin: &str) -> Value {
        json!({
            "matcher": GUARD_MATCHER,
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

    fn prd_context_group_for(bin: &str) -> Value {
        json!({
            "hooks": [{
                "type": "command",
                "command": fault_tolerant_command(bin, "prd-context"),
                "timeout": 10000
            }]
        })
    }

    fn notes_hint_group_for(bin: &str) -> Value {
        json!({
            "matcher": NOTES_HINT_MATCHER,
            "hooks": [{
                "type": "command",
                "command": fault_tolerant_command(bin, "notes-hint"),
                "timeout": 10000
            }]
        })
    }

    /// The notes-hint group lives under SessionStart next to whatever the user
    /// put there; its marker must catch only itself, and `has_prd_context_hook`
    /// must demand both halves so an upgraded host gets healed.
    #[test]
    fn notes_hint_marker_and_prd_context_pairing() {
        let bin = "/x/fleet";
        let hint = notes_hint_group_for(bin);
        let prd = prd_context_group_for(bin);
        let user_start =
            json!({ "matcher": "startup", "hooks": [{"type": "command", "command": "echo hi"}] });

        assert!(is_notes_hint_group(&hint));
        assert!(!is_notes_hint_group(&prd));
        assert!(!is_notes_hint_group(&user_start));
        assert!(!is_prd_context_group(&hint));
        assert!(!is_idle_resume_group(&hint));
        assert!(!is_wakeup_guard_group(&hint));

        // Old-build shape: prd-context present, no SessionStart companion.
        let mut hooks = Map::new();
        hooks.insert("UserPromptSubmit".into(), json!([prd.clone()]));
        assert!(
            !has_prd_context_hook(&hooks),
            "must read as not installed until the companion exists"
        );
        assert!(!has_notes_hint_hook(&hooks));

        // Current shape: both halves; a neighbouring user group is untouched by
        // the idempotent retain.
        let mut start_arr = vec![
            user_start.clone(),
            notes_hint_group_for("/old/fleet"),
            hint.clone(),
        ];
        start_arr.retain(|g| !is_notes_hint_group(g));
        assert_eq!(start_arr, vec![user_start.clone()]);
        hooks.insert(
            "SessionStart".into(),
            json!([user_start.clone(), hint.clone()]),
        );
        assert!(has_notes_hint_hook(&hooks));
        assert!(
            !has_prd_context_hook(&hooks),
            "still not installed until the PostToolUse companion exists"
        );

        // Current shape: all three. The PostToolUse array already carries
        // Fleet's own logging group; the retain must spare it.
        let ctx = ctx_reminder_group_for(bin);
        let logging = json!({"hooks": [{"type": "command", "command": "cat >> log"}]});
        let mut post_arr = vec![
            logging.clone(),
            ctx_reminder_group_for("/old/fleet"),
            ctx.clone(),
        ];
        post_arr.retain(|g| !is_ctx_reminder_group(g));
        assert_eq!(post_arr, vec![logging.clone()]);
        assert!(!is_ctx_reminder_group(&hint));
        assert!(!is_ctx_reminder_group(&prd));
        hooks.insert("PostToolUse".into(), json!([logging, ctx]));
        assert!(has_ctx_reminder_hook(&hooks));
        assert!(
            !has_prd_context_hook(&hooks),
            "still not installed until the recent-sessions companion exists"
        );

        // Current shape: all four. The two SessionStart companions are separate
        // entries; each marker must catch only its own, or installing one would
        // evict the other on every apply.
        let recent = recent_sessions_group_for(bin);
        assert!(is_recent_sessions_group(&recent));
        assert!(!is_recent_sessions_group(&hint));
        assert!(!is_notes_hint_group(&recent));
        let mut start_arr = vec![
            user_start.clone(),
            recent_sessions_group_for("/old/fleet"),
            recent.clone(),
            hint.clone(),
        ];
        start_arr.retain(|g| !is_recent_sessions_group(g));
        assert_eq!(start_arr, vec![user_start.clone(), hint.clone()]);
        hooks.insert("SessionStart".into(), json!([user_start, hint, recent]));
        assert!(has_recent_sessions_hook(&hooks));
        assert!(has_notes_hint_hook(&hooks));
        assert!(has_prd_context_hook(&hooks));
    }

    fn recent_sessions_group_for(bin: &str) -> Value {
        json!({
            "matcher": RECENT_SESSIONS_MATCHER,
            "hooks": [{
                "type": "command",
                "command": fault_tolerant_command(bin, "recent-sessions"),
                "timeout": 20000
            }]
        })
    }

    fn ctx_reminder_group_for(bin: &str) -> Value {
        json!({
            "hooks": [{
                "type": "command",
                "command": fault_tolerant_command(bin, "ctx-reminder"),
                "timeout": 10000
            }]
        })
    }

    fn idle_stop_group_for(bin: &str) -> Value {
        json!({
            "hooks": [{
                "type": "command",
                "command": fault_tolerant_command(bin, "session idle"),
                "timeout": 5000
            }]
        })
    }

    fn idle_resume_group_for(bin: &str) -> Value {
        json!({
            "hooks": [{
                "type": "command",
                "command": fault_tolerant_command(bin, "session resume"),
                "timeout": 5000
            }]
        })
    }

    fn wakeup_guard_group_for(bin: &str) -> Value {
        json!({
            "matcher": crate::wakeup_guard::WAKEUP_GUARD_MATCHER,
            "hooks": [{
                "type": "command",
                "command": fault_tolerant_command(bin, "wakeup-guard"),
                "timeout": 5000
            }]
        })
    }

    #[test]
    fn wakeup_guard_group_is_told_apart_from_its_pretooluse_neighbours() {
        // All four live under PreToolUse. If `is_wakeup_guard_group` were loose
        // enough to match a sibling, `remove_wakeup_guard_hook` would silently
        // uninstall the command audit gate (guard) or the decision-card bridge
        // (elicitation) instead.
        let bin = "/tmp/fleet";
        assert!(is_wakeup_guard_group(&wakeup_guard_group_for(bin)));
        for neighbour in [
            guard_group_for(bin),
            elicitation_group_for(bin),
            plan_approval_group_for(bin),
        ] {
            assert!(
                !is_wakeup_guard_group(&neighbour),
                "must not claim a sibling PreToolUse group: {neighbour}"
            );
        }
        // ...and the siblings must not claim the wakeup group either.
        let wakeup = wakeup_guard_group_for(bin);
        assert!(!is_guard_group(&wakeup));
        assert!(!is_elicitation_group(&wakeup));
        assert!(!is_plan_approval_group(&wakeup));
    }

    #[test]
    fn has_wakeup_guard_hook_sees_it_only_when_present() {
        // The probe self-healing reads: a false negative reinstalls a hook that
        // is already there (harmless), but a false *positive* leaves the wakeup
        // guard missing forever, since heal only installs what reads as absent.
        let bin = "/tmp/fleet";
        let mut hooks = Map::new();
        assert!(
            !has_wakeup_guard_hook(&hooks),
            "empty settings have no hooks"
        );

        // Siblings under the same event must not read as the wakeup guard.
        hooks.insert(
            "PreToolUse".to_string(),
            json!([guard_group_for(bin), elicitation_group_for(bin)]),
        );
        assert!(!has_wakeup_guard_hook(&hooks));

        hooks.insert(
            "PreToolUse".to_string(),
            json!([guard_group_for(bin), wakeup_guard_group_for(bin)]),
        );
        assert!(has_wakeup_guard_hook(&hooks));
    }

    #[test]
    fn apply_wakeup_guard_is_idempotent_and_spares_sibling_hooks() {
        let _guard = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet-hooks-wakeupguard-{}-{}",
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

        let outcome = (|| -> Result<(), String> {
            // Seed a settings.json that already has a foreign PreToolUse group,
            // so we can prove apply/remove leave it untouched.
            let foreign = json!({
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": "/usr/local/bin/somebody-else"}]
            });
            write_settings(&json!({ "hooks": { "PreToolUse": [foreign.clone()] } }))?;

            // Apply twice — the second must not duplicate the group.
            apply_wakeup_guard_hook()?;
            apply_wakeup_guard_hook()?;

            let settings = read_settings().ok_or("settings vanished after apply")?;
            let arr = settings["hooks"]["PreToolUse"]
                .as_array()
                .ok_or("PreToolUse is not an array")?
                .clone();
            let ours: Vec<&Value> = arr.iter().filter(|g| is_wakeup_guard_group(g)).collect();
            if ours.len() != 1 {
                return Err(format!(
                    "expected exactly 1 wakeup group, got {}",
                    ours.len()
                ));
            }
            let matcher = ours[0]["matcher"].as_str().unwrap_or_default();
            if matcher != crate::wakeup_guard::WAKEUP_GUARD_MATCHER {
                return Err(format!("wrong matcher: {matcher}"));
            }
            if !arr.contains(&foreign) {
                return Err("apply clobbered the foreign PreToolUse group".into());
            }

            remove_wakeup_guard_hook()?;
            let after = read_settings().ok_or("settings vanished after remove")?;
            let arr_after = after["hooks"]["PreToolUse"]
                .as_array()
                .ok_or("remove dropped PreToolUse entirely, taking the foreign group with it")?;
            if arr_after.iter().any(is_wakeup_guard_group) {
                return Err("remove left our group behind".into());
            }
            if !arr_after.contains(&foreign) {
                return Err("remove clobbered the foreign PreToolUse group".into());
            }
            Ok(())
        })();

        // Restore env before asserting so a failure can't leak FLEET_HOME.
        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = fs::remove_dir_all(&tmp);

        // resolve_fleet_binary() needs an installed fleet CLI; without one the
        // apply path can't be exercised, so treat that as a skip rather than a
        // failure (mirrors how the hook itself fails open).
        if let Err(e) = &outcome {
            if e.contains("Cannot find fleet binary") {
                eprintln!("skipped: no fleet binary on this host");
                return;
            }
        }
        outcome.expect("wakeup guard apply/remove must be idempotent and sibling-safe");
    }

    #[test]
    fn removing_a_hook_records_the_users_intent_and_reapplying_clears_it() {
        // The contract self-healing rests on: after the user switches the guard
        // off, `~/.fleet/control-plane-prefs.json` says so, and heal must skip
        // it. Without this record both "never installed" and "deliberately off"
        // look identical in settings.json, and heal would override the choice.
        use crate::control_plane_prefs::{is_disabled, Feature};

        let _guard = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet-hooks-intent-{}-{}",
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

        let outcome = (|| -> Result<(), String> {
            if is_disabled(Feature::GuardHook) {
                return Err("a fresh host must not read as disabled".into());
            }

            remove_guard_hook()?;
            if !is_disabled(Feature::GuardHook) {
                return Err("remove_guard_hook must record the disablement".into());
            }
            // Only that one feature — an over-broad record would suppress heal
            // for the whole control plane.
            if is_disabled(Feature::ElicitationHook) {
                return Err("remove_guard_hook must not touch its neighbours".into());
            }

            apply_guard_hook()?;
            if is_disabled(Feature::GuardHook) {
                return Err("apply_guard_hook must clear the disablement".into());
            }
            Ok(())
        })();

        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = fs::remove_dir_all(&tmp);

        if let Err(e) = &outcome {
            if e.contains("Cannot find fleet binary") {
                eprintln!("skipped: no fleet binary on this host");
                return;
            }
        }
        outcome.expect("remove/apply must record and clear the user's intent");
    }

    #[test]
    fn write_settings_creates_claude_dir_when_absent() {
        // Regression: a fresh host — e.g. the Fleet Cloud container on first
        // boot, where ~/.claude is on the ephemeral layer and does not exist —
        // must still get settings.json written. Before the fix write_settings
        // called fs::write without creating the parent ~/.claude, so
        // apply_guard_hook (the command audit gate) failed with ENOENT, leaving
        // the injected Bash(*) allow rule with nothing to intercept commands.
        let _guard = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet-hooks-writesettings-{}-{}",
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

        let claude_dir = tmp.join(".claude");
        let setup_ok = !claude_dir.exists();
        let res = write_settings(&json!({ "hooks": {} }));
        let wrote = res.is_ok() && claude_dir.join("settings.json").is_file();

        // Restore env before asserting so a failure can't leak FLEET_HOME.
        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = fs::remove_dir_all(&tmp);

        assert!(setup_ok, "test setup: ~/.claude must start absent");
        assert!(
            wrote,
            "write_settings must create ~/.claude and write settings.json on a fresh host: {res:?}"
        );
    }

    #[test]
    fn apply_default_model_sets_model_and_leaves_siblings_alone() {
        // The Fleet Cloud container re-runs `fleet bootstrap` on every start
        // (~/.claude is ephemeral), so this write has to be idempotent, must
        // not disturb the hook groups the other bootstrap steps just wrote, and
        // must treat a blank model as "keep the CLI's own default".
        let _guard = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet-hooks-defaultmodel-{}-{}",
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

        let outcome = (|| -> Result<(), String> {
            write_settings(&json!({ "hooks": { "PreToolUse": [] } }))?;

            apply_default_model("opus")?;
            let after = read_settings().ok_or("settings unreadable after apply")?;
            if after.get("model").and_then(|m| m.as_str()) != Some("opus") {
                return Err(format!("model not written: {after}"));
            }
            if after.get("hooks").is_none() {
                return Err("apply_default_model clobbered the hooks key".into());
            }

            // Blank = no-op, not a wipe of the previously pinned model.
            apply_default_model("  ")?;
            let after = read_settings().ok_or("settings unreadable after blank apply")?;
            if after.get("model").and_then(|m| m.as_str()) != Some("opus") {
                return Err(format!("blank model must not change settings: {after}"));
            }
            Ok(())
        })();

        // Restore env before asserting so a failure can't leak FLEET_HOME.
        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = fs::remove_dir_all(&tmp);

        outcome.expect("apply_default_model must pin the model without touching siblings");
    }

    #[test]
    fn apply_no_commit_attribution_disables_trailers_and_merges() {
        // Guards the reason this exists: Claude Code's Co-Authored-By trailer is
        // a system-prompt default that guidance text cannot override, so the
        // control plane has to write `attribution` — without clobbering the
        // hooks the other bootstrap steps wrote, without dropping an unrelated
        // key someone put inside `attribution` by hand, and reporting itself as
        // applied afterwards so heal stays quiet.
        let _guard = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet-hooks-attribution-{}-{}",
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

        let outcome = (|| -> Result<(), String> {
            write_settings(&json!({
                "hooks": { "PreToolUse": [] },
                "attribution": { "somethingElse": "keep me" }
            }))?;
            if no_commit_attribution_applied() {
                return Err("must not read as applied before the write".into());
            }

            apply_no_commit_attribution()?;
            let after = read_settings().ok_or("settings unreadable after apply")?;
            let attr = after.get("attribution").ok_or("attribution key missing")?;
            if attr.get("commitTrailers").and_then(|v| v.as_bool()) != Some(false) {
                return Err(format!("commitTrailers not disabled: {after}"));
            }
            if attr.get("sessionUrl").and_then(|v| v.as_bool()) != Some(false) {
                return Err(format!("sessionUrl not disabled: {after}"));
            }
            // Both spellings, because as of Claude Code 2.1.263 only the old
            // one suppresses the trailer in the system prompt — see
            // `apply_no_commit_attribution`'s doc comment for the probe.
            if after.get("includeCoAuthoredBy").and_then(|v| v.as_bool()) != Some(false) {
                return Err(format!("includeCoAuthoredBy not disabled: {after}"));
            }
            if attr.get("somethingElse").and_then(|v| v.as_str()) != Some("keep me") {
                return Err(format!("apply replaced the attribution object: {after}"));
            }
            if after.get("hooks").is_none() {
                return Err("apply clobbered the hooks key".into());
            }
            if !no_commit_attribution_applied() {
                return Err("probe must see its own write, or heal reruns forever".into());
            }

            // Idempotent — bootstrap re-runs on every Fleet Cloud start.
            apply_no_commit_attribution()?;
            if !no_commit_attribution_applied() {
                return Err("second apply must leave it applied".into());
            }

            // A host carrying only the new spelling — every host Fleet healed
            // before 2026-09-10 — still gets the byline, so it must read as
            // *not* applied and be rewritten.
            write_settings(&json!({
                "attribution": { "commitTrailers": false, "sessionUrl": false }
            }))?;
            if no_commit_attribution_applied() {
                return Err("attribution-only host must not read as applied".into());
            }
            apply_no_commit_attribution()?;
            if !no_commit_attribution_applied() {
                return Err("heal must add the old spelling to such a host".into());
            }
            Ok(())
        })();

        // Restore env before asserting so a failure can't leak FLEET_HOME.
        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = fs::remove_dir_all(&tmp);

        outcome.expect("apply_no_commit_attribution must disable both trailers and merge");
    }

    #[test]
    fn read_recent_events_falls_back_to_file_mtime_when_record_lacks_timestamp() {
        // Regression: Claude Code's hook payloads (PreToolUse / PostToolUse /
        // Stop / SubagentStop) do NOT contain a "timestamp" field — only
        // session_id, hook_event_name, cwd, tool_*, etc. Verified live by
        // inspecting ~/.fleet/hooks.jsonl: 51/51 recent records had no
        // "timestamp" key. The old code returned timestamp_ms=0, which made
        // read_hook_states treat every hook event as ">5 min stale" and
        // discard them, killing the Phase-0 hook override in
        // determine_status.
        //
        // Fix: fall back to the file's mtime when the record lacks a usable
        // timestamp. File mtime is a strict upper bound on every record in
        // the file, which is correct for the 5-minute freshness gate (a
        // long-untouched hooks.jsonl still expires; a recently-written one
        // keeps its events alive).
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"session_id":"sess-1","hook_event_name":"Stop","cwd":"/x"}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"session_id":"sess-2","hook_event_name":"PreToolUse","tool_name":"Bash","cwd":"/y"}}"#
        )
        .unwrap();
        drop(f);

        let events = read_recent_events(&path, 500);
        assert_eq!(events.len(), 2);
        for ev in &events {
            assert!(
                ev.timestamp_ms > 0,
                "event must inherit non-zero timestamp from file mtime when \
                 record itself lacks a timestamp field — got {} for session_id={}",
                ev.timestamp_ms,
                ev.session_id
            );
        }
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

    /// Regression: the guard matcher must fire for the Windows `PowerShell`
    /// tool as well as `Bash`. Reverting to a `Bash`-only matcher would leave a
    /// Windows-without-Git-Bash session running every shell command past the
    /// audit gate, because Claude Code names that tool `PowerShell`, not `Bash`.
    #[test]
    fn guard_matcher_covers_bash_and_powershell() {
        let alts: Vec<&str> = GUARD_MATCHER.split('|').collect();
        assert!(
            alts.contains(&"Bash"),
            "guard matcher must cover the Bash tool"
        );
        assert!(
            alts.contains(&"PowerShell"),
            "guard matcher must cover the Windows PowerShell tool"
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
        let group = plan_approval_group_for("/Applications/Claw Fleet.app/Contents/MacOS/fleet");
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
        // All four markers must be mutually exclusive.
        let g = guard_group_for("/x/fleet");
        let e = elicitation_group_for("/x/fleet");
        let p = plan_approval_group_for("/x/fleet");
        let c = prd_context_group_for("/x/fleet");
        assert!(!is_elicitation_group(&g));
        assert!(!is_plan_approval_group(&g));
        assert!(!is_prd_context_group(&g));
        assert!(!is_guard_group(&e));
        assert!(!is_plan_approval_group(&e));
        assert!(!is_prd_context_group(&e));
        assert!(!is_guard_group(&p));
        assert!(!is_elicitation_group(&p));
        assert!(!is_prd_context_group(&p));
        assert!(!is_guard_group(&c));
        assert!(!is_elicitation_group(&c));
        assert!(!is_plan_approval_group(&c));
    }

    fn legacy_events_group() -> Value {
        json!({
            "hooks": [{
                "type": "command",
                "command": r#"sh -c 'cat >> "$HOME/.claude/fleet/hooks.jsonl"'"#,
                "async": true
            }]
        })
    }

    #[test]
    fn legacy_group_is_not_recognized_as_current_fleet_group() {
        // The whole reason it lingered: is_fleet_group only matches the
        // `.fleet/hooks.jsonl` path, and `.claude/fleet/hooks.jsonl` does not
        // contain that substring (the `f` is preceded by `/`, not `.`).
        let legacy = legacy_events_group();
        assert!(!is_fleet_group(&legacy));
        assert!(group_targets_legacy_events_file(&legacy));
        assert!(!group_targets_legacy_events_file(&fleet_hook_group()));
    }

    #[test]
    fn purge_drops_legacy_groups_but_keeps_others() {
        let mut hooks: Map<String, Value> = Map::new();
        // PostToolUse hosts the legacy group alongside the current one and an
        // unrelated third-party group — only the legacy one must go.
        let unrelated = json!({
            "hooks": [{ "type": "command", "command": "echo hi" }]
        });
        hooks.insert(
            "PostToolUse".into(),
            json!([legacy_events_group(), fleet_hook_group(), unrelated.clone()]),
        );
        // Stop hosts ONLY the legacy group — the emptied array must be removed.
        hooks.insert("Stop".into(), json!([legacy_events_group()]));

        purge_legacy_event_hooks(&mut hooks);

        let post = hooks.get("PostToolUse").unwrap().as_array().unwrap();
        assert_eq!(post.len(), 2, "legacy dropped, current + unrelated kept");
        assert!(post.iter().any(is_fleet_group));
        assert!(post
            .iter()
            .any(|g| !is_fleet_group(g) && !group_targets_legacy_events_file(g)));
        assert!(!post.iter().any(group_targets_legacy_events_file));
        assert!(!hooks.contains_key("Stop"), "emptied event array removed");
    }

    #[test]
    fn prd_context_marker_detects_actual_generated_command() {
        let group = prd_context_group_for("/Applications/Claw Fleet.app/Contents/MacOS/fleet");
        assert!(
            is_prd_context_group(&group),
            "is_prd_context_group must recognise the command actually produced by \
             fault_tolerant_command"
        );
    }

    #[test]
    fn idle_markers_detect_actual_generated_commands() {
        let stop = idle_stop_group_for("/Applications/Claw Fleet.app/Contents/MacOS/fleet");
        let resume = idle_resume_group_for("/Applications/Claw Fleet.app/Contents/MacOS/fleet");
        assert!(
            is_idle_stop_group(&stop),
            "is_idle_stop_group must recognise generated cmd"
        );
        assert!(
            is_idle_resume_group(&resume),
            "is_idle_resume_group must recognise generated cmd"
        );
    }

    #[test]
    fn idle_markers_do_not_cross_match_prd_context_or_each_other() {
        // UserPromptSubmit hosts BOTH prd-context and idle-resume; their markers
        // must not collide. Stop hosts idle-stop; it must not match resume.
        let bin = "/x/fleet";
        let stop = idle_stop_group_for(bin);
        let resume = idle_resume_group_for(bin);
        let prd = prd_context_group_for(bin);

        assert!(!is_idle_resume_group(&stop));
        assert!(!is_idle_stop_group(&resume));
        assert!(!is_idle_resume_group(&prd));
        assert!(!is_idle_stop_group(&prd));
        assert!(!is_prd_context_group(&resume));
        assert!(!is_prd_context_group(&stop));

        // And the original four markers must not catch the new groups.
        assert!(!is_guard_group(&stop));
        assert!(!is_elicitation_group(&stop));
        assert!(!is_plan_approval_group(&stop));
        assert!(!is_guard_group(&resume));
        assert!(!is_elicitation_group(&resume));
        assert!(!is_plan_approval_group(&resume));
    }

    #[test]
    fn idle_idempotent_retain_removes_stale_groups() {
        // Mirrors apply_idle_hooks: existing fleet idle groups must be filtered
        // out across binary path changes, leaving unrelated entries intact.
        let mut user_prompt_arr = vec![
            json!({ "hooks": [{"type": "command", "command": "user-other"}] }),
            prd_context_group_for("/Applications/Claw Fleet.app/Contents/MacOS/fleet"),
            idle_resume_group_for("/Applications/Claw Fleet.app/Contents/MacOS/fleet"),
            idle_resume_group_for("/Users/x/workspace/claude-fleet/target/debug/fleet"),
        ];
        user_prompt_arr.retain(|g| !is_idle_resume_group(g));
        // PRD-context must survive; only the unrelated entry + prd-context remain.
        assert_eq!(user_prompt_arr.len(), 2, "prd-context must not be filtered");
        assert!(user_prompt_arr.iter().any(|g| is_prd_context_group(g)));
    }

    // ── read_tail_lines ─────────────────────────────────────────────────────
    //
    // The scan reads hooks.jsonl on every tick and the file reaches tens of MB,
    // so it now walks backwards from the end instead of slurping the whole file.
    // These pin the behaviour that made the old (correct but wasteful) version
    // safe: the tail must come back byte-identical, including across the chunk
    // boundaries the new reader introduces.

    fn write_tmp(name: &str, content: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "fleet_tail_{}_{}_{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&p, content).unwrap();
        p
    }

    /// The property the whole optimisation rests on: same answer as reading the
    /// entire file and taking the last N lines.
    fn slurp_tail(content: &str, n: usize) -> Vec<String> {
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(n);
        lines[start..].iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn tail_matches_a_full_read_on_a_multi_chunk_file() {
        // > 64 KiB, so the reader must walk several chunks backwards.
        let content: String = (0..4000)
            .map(|i| {
                format!(
                    "{{\"session_id\":\"s{i}\",\"hook_event_name\":\"Stop\",\"pad\":\"{}\"}}\n",
                    "x".repeat(40)
                )
            })
            .collect();
        assert!(content.len() > 64 * 1024, "test needs a multi-chunk file");
        let p = write_tmp("multi.jsonl", &content);

        assert_eq!(read_tail_lines(&p, 500), slurp_tail(&content, 500));
        // And the last line really is the last line.
        assert!(read_tail_lines(&p, 1)[0].contains("\"s3999\""));

        let _ = fs::remove_file(&p);
    }

    #[test]
    fn tail_does_not_corrupt_multibyte_chars_at_a_chunk_boundary() {
        // Chinese descriptions are the norm in this codebase's payloads. A chunk
        // boundary lands mid-character constantly; the affected line is the
        // partial leading one and must be dropped, never mangled into the result.
        let content: String = (0..3000)
            .map(|i| {
                format!(
                    "{{\"session_id\":\"s{i}\",\"description\":\"等待生产部署完成并上传矩阵\"}}\n"
                )
            })
            .collect();
        assert!(content.len() > 64 * 1024);
        let p = write_tmp("utf8.jsonl", &content);

        let tail = read_tail_lines(&p, 500);
        assert_eq!(tail, slurp_tail(&content, 500));
        // Every returned line must still be valid JSON with the text intact —
        // a split multi-byte char would have produced U+FFFD here.
        for line in &tail {
            let v: Value = serde_json::from_str(line).expect("tail line must be valid JSON");
            assert_eq!(v["description"], "等待生产部署完成并上传矩阵");
            assert!(
                !line.contains('\u{FFFD}'),
                "no replacement chars in the tail"
            );
        }

        let _ = fs::remove_file(&p);
    }

    #[test]
    fn tail_returns_everything_when_the_file_is_shorter_than_max_lines() {
        let content = "a\nb\nc\n";
        let p = write_tmp("short.jsonl", content);
        assert_eq!(read_tail_lines(&p, 500), vec!["a", "b", "c"]);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn tail_handles_a_missing_trailing_newline() {
        let content = "a\nb\nlast-line-no-newline";
        let p = write_tmp("nonl.jsonl", content);
        assert_eq!(read_tail_lines(&p, 2), vec!["b", "last-line-no-newline"]);
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn tail_of_empty_or_missing_file_is_empty() {
        let p = write_tmp("empty.jsonl", "");
        assert!(read_tail_lines(&p, 500).is_empty());
        let _ = fs::remove_file(&p);

        assert!(read_tail_lines(Path::new("/nonexistent/hooks.jsonl"), 500).is_empty());
    }

    #[test]
    fn read_recent_events_parses_background_tasks_from_a_stop_payload() {
        // End-to-end through the tail reader: the Stop payload's background_tasks
        // must survive into the HookEvent the scan consumes.
        let content = concat!(
            r#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash"}"#,
            "\n",
            r#"{"session_id":"s1","hook_event_name":"Stop","background_tasks":[{"id":"b1","type":"shell","status":"running","description":"等部署"}]}"#,
            "\n",
        );
        let p = write_tmp("events.jsonl", content);

        let evs = read_recent_events(&p, 500);
        assert_eq!(evs.len(), 2);
        let stop = evs.iter().find(|e| e.event_name == "Stop").unwrap();
        assert_eq!(stop.background_tasks.len(), 1);
        assert_eq!(stop.background_tasks[0].id, "b1");
        assert!(stop.background_tasks[0].is_running());
        assert_eq!(stop.background_tasks[0].description, "等部署");
        // Non-Stop events carry none.
        let pre = evs.iter().find(|e| e.event_name == "PreToolUse").unwrap();
        assert!(pre.background_tasks.is_empty());

        let _ = fs::remove_file(&p);
    }

    // ── Incremental follow ────────────────────────────────────────────────

    fn empty_tail() -> HookTail {
        HookTail {
            offset: None,
            path: None,
            states: HashMap::new(),
        }
    }

    fn pre_tool(sid: &str, tool: &str) -> String {
        format!(r#"{{"session_id":"{sid}","hook_event_name":"PreToolUse","tool_name":"{tool}"}}"#)
            + "\n"
    }

    fn append(path: &Path, line: &str) {
        use std::io::Write;
        let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(line.as_bytes()).unwrap();
    }

    #[test]
    fn hook_tail_remembers_a_session_pushed_out_of_the_seed_window() {
        // The flicker's other half: `hooks.jsonl` is machine-wide, so a session
        // quiet inside a long tool used to be forgotten as soon as busier
        // siblings appended past the fixed tail window, and its status fell out
        // of the working set until its next hook event landed. Following the
        // file forward keeps the answer independent of the neighbours.
        let p = write_tmp("follow.jsonl", &pre_tool("quiet", "Bash"));
        let mut tail = empty_tail();
        tail.follow(&p, 1_000);
        assert_eq!(
            tail.snapshot(1_000).states.get("quiet"),
            Some(&HookState::ToolExecuting)
        );

        for i in 0..2_000 {
            append(&p, &pre_tool(&format!("busy-{i}"), "Read"));
        }
        tail.follow(&p, 2_000);

        let snap = tail.snapshot(2_000);
        assert_eq!(
            snap.states.get("quiet"),
            Some(&HookState::ToolExecuting),
            "a session 2000 events deep must still be remembered"
        );
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn hook_tail_expires_a_state_once_it_stops_describing_the_present() {
        // Ingest time is the clock — the records themselves carry no timestamp,
        // and dating them by the file's mtime (the old behaviour) meant nothing
        // ever expired on a machine whose hooks.jsonl is always being written.
        let p = write_tmp("expire.jsonl", &pre_tool("s1", "Bash"));
        let mut tail = empty_tail();
        tail.follow(&p, 1_000);

        assert!(tail
            .snapshot(1_000 + HOOK_STATE_MAX_AGE_MS)
            .states
            .contains_key("s1"));
        assert!(
            !tail
                .snapshot(1_000 + HOOK_STATE_MAX_AGE_MS + 1)
                .states
                .contains_key("s1"),
            "a state older than the freshness window must not be reported"
        );
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn hook_tail_waits_for_a_record_to_be_fully_written() {
        // The scan reads while hooks are appending; half a line must not be
        // parsed, nor silently skipped once its newline lands.
        let p = write_tmp("partial.jsonl", "");
        let mut tail = empty_tail();
        let line = pre_tool("s1", "Bash");
        let (head, rest) = line.split_at(20);

        append(&p, head);
        tail.follow(&p, 1_000);
        assert!(tail.snapshot(1_000).states.is_empty());

        append(&p, rest);
        tail.follow(&p, 1_000);
        assert_eq!(
            tail.snapshot(1_000).states.get("s1"),
            Some(&HookState::ToolExecuting)
        );
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn hook_tail_reseeds_when_the_file_is_truncated() {
        // `maybe_truncate_events_file` rewrites the file to its last 2000 lines,
        // which moves every offset. Re-seed instead of reading garbage.
        let mut before = String::new();
        for i in 0..50 {
            before.push_str(&pre_tool(&format!("old-{i}"), "Bash"));
        }
        let p = write_tmp("truncate.jsonl", &before);
        let mut tail = empty_tail();
        tail.follow(&p, 1_000);

        fs::write(&p, pre_tool("fresh", "Bash")).unwrap();
        tail.follow(&p, 1_000);

        let snap = tail.snapshot(1_000);
        assert_eq!(snap.states.get("fresh"), Some(&HookState::ToolExecuting));
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn hook_tail_calls_an_interactive_tool_a_wait_not_work() {
        // A `fleet__ask` / permission prompt PreToolUse is a session parked on a
        // card, not work in flight. Under the old fixed window this sorted
        // itself out by accident — the event was evicted before anyone looked —
        // so following the file forward has to draw the line on purpose.
        let p = write_tmp("interactive.jsonl", "");
        let mut tail = empty_tail();
        append(&p, &pre_tool("asking", "mcp__fleet__fleet__ask"));
        append(&p, &pre_tool("working", "Bash"));
        tail.follow(&p, 1_000);

        let snap = tail.snapshot(1_000);
        assert_eq!(
            snap.states.get("asking"),
            Some(&HookState::AwaitingUserInput)
        );
        assert_eq!(snap.states.get("working"), Some(&HookState::ToolExecuting));
        let _ = fs::remove_file(&p);
    }

    #[test]
    fn hook_tail_clears_background_tasks_when_the_session_moves_on() {
        // Only the latest event describes the session's present: a Stop's
        // outstanding tasks must not outlive the next tool call.
        let stop = concat!(
            r#"{"session_id":"s1","hook_event_name":"Stop","background_tasks":[{"id":"b1","type":"shell","status":"running","description":"deploy"}]}"#,
            "\n"
        );
        let p = write_tmp("bgtasks.jsonl", stop);
        let mut tail = empty_tail();
        tail.follow(&p, 1_000);
        assert_eq!(
            tail.snapshot(1_000)
                .background_tasks
                .get("s1")
                .map(Vec::len),
            Some(1)
        );

        append(&p, &pre_tool("s1", "Bash"));
        tail.follow(&p, 1_000);
        assert!(tail.snapshot(1_000).background_tasks.is_empty());
        let _ = fs::remove_file(&p);
    }
}
