//! Everything Fleet adds to a Claude Code session it starts, handed over as
//! launch arguments instead of written into the user's global config.
//!
//! Fleet used to govern Claude Code by editing `~/.claude/settings.json`
//! (hooks, `permissions.allow`, `model`, attribution), `~/.claude.json`
//! (`mcpServers.fleet`) and `~/.claude/CLAUDE.md` (guidance `@import`s). That
//! changed every `claude` on the machine, including the ones the user opens by
//! hand. This module builds the same control plane per launch:
//!
//! - `--settings <file>` — Fleet's hooks, the permission allow rules, and
//!   commit attribution off. Claude Code merges these with the user's own
//!   settings (hooks from both fire; deny beats allow across layers).
//! - `--mcp-config <file>` — the `fleet` MCP server, under the same server
//!   name so tool ids stay `mcp__fleet__fleet__*`.
//! - `--permission-prompt-tool` — only alongside the MCP config it names.
//! - `--append-system-prompt-file <file>` — the enabled guidance and lessons.
//! - `--model <m>` — Fleet's default model, unless the caller named one.
//!
//! Measured on Claude Code 2.1.284: hooks, permissions and MCP servers are read
//! per process, so every spawn *and* every resume has to pass them. The system
//! prompt is the opposite — it is frozen into the transcript when the session
//! is created, survives a `--resume` that omits the flag, and is ignored when
//! only a resume supplies it. So guidance reaches the sessions Fleet creates,
//! for their whole life, and a guidance edit reaches only sessions created
//! after it.
//!
//! Which features are on comes from [`crate::control_plane_prefs`].

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::control_plane_prefs::{is_enabled, Feature};

const LAUNCH_DIR: &str = "claude-launch";
const SETTINGS_FILE: &str = "settings.json";
const MCP_FILE: &str = "mcp.json";
const SYSTEM_PROMPT_FILE: &str = "system-prompt.md";
/// The concept guidance rendered by [`reconcile_guidance`]. Kept apart from
/// [`SYSTEM_PROMPT_FILE`] because it needs the user's title and locale, which
/// only the reconcile callers know; lessons are appended at launch time.
const GUIDANCE_FILE: &str = "claude-guidance.md";
const CONFIG_FILE: &str = "claude-launch.json";

fn fleet_dir() -> Option<PathBuf> {
    crate::session::get_fleet_dir()
}

fn launch_dir() -> Option<PathBuf> {
    fleet_dir().map(|d| d.join(LAUNCH_DIR))
}

fn guidance_path() -> Option<PathBuf> {
    fleet_dir().map(|d| d.join(GUIDANCE_FILE))
}

fn config_path() -> Option<PathBuf> {
    fleet_dir().map(|d| d.join(CONFIG_FILE))
}

// ── Default model ────────────────────────────────────────────────────────────

/// `~/.fleet/claude-launch.json`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeLaunchConfig {
    /// Model a Fleet session runs on when the caller names none. Replaces the
    /// `model` key Fleet used to write into the global settings.json.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
}

pub fn load_config() -> ClaudeLaunchConfig {
    config_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(cfg: &ClaudeLaunchConfig) -> Result<(), String> {
    let path = config_path().ok_or("cannot determine fleet dir")?;
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    write_if_changed(&path, &json)
}

/// Fleet's default Claude model: the recorded one, else `$FLEET_CLAUDE_MODEL`
/// (what `fleet bootstrap` and the cloud container are configured with).
pub fn default_model() -> Option<String> {
    load_config()
        .default_model
        .or_else(|| std::env::var("FLEET_CLAUDE_MODEL").ok())
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
}

/// Record Fleet's default Claude model. A blank value is a no-op, which leaves
/// sessions on the CLI's own default.
pub fn set_default_model(model: &str) -> Result<(), String> {
    let model = model.trim();
    if model.is_empty() {
        return Ok(());
    }
    let mut cfg = load_config();
    cfg.default_model = Some(model.to_string());
    save_config(&cfg)
}

// ── Guidance ─────────────────────────────────────────────────────────────────

/// Render the enabled guidance concepts, in the order the old CLAUDE.md
/// imports used. Deterministic for a given (prefs, title, locale), which is
/// what keeps a fork's system prompt identical to its parent's.
pub fn render_guidance(user_title: &str, locale: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if is_enabled(Feature::InteractionMode) {
        parts.push(crate::interaction_mode::render_guidance(user_title, locale));
    }
    if is_enabled(Feature::PrdDiscipline) {
        parts.push(crate::prd_discipline::render_guidance(user_title, locale));
    }
    if is_enabled(Feature::WikiGuidance) {
        parts.push(crate::wiki_guidance::render_guidance(locale));
    }
    if is_enabled(Feature::ModelGuidance) {
        parts.push(crate::model_guidance::render_guidance(locale));
    }
    if is_enabled(Feature::SessionTitleGuidance) {
        parts.push(crate::session_title_guidance::render_guidance(
            user_title, locale,
        ));
    }
    join_sections(parts)
}

fn join_sections(parts: Vec<String>) -> String {
    parts
        .iter()
        .map(|p| p.trim_end())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Re-render the guidance file from the current prefs. Called wherever the
/// codex/dsh carriers are reconciled — after a concept toggle and on startup —
/// since those are the callers that know the user's title and locale.
pub fn reconcile_guidance(user_title: &str, locale: &str) -> Result<(), String> {
    let path = guidance_path().ok_or("cannot determine fleet dir")?;
    write_if_changed(&path, &render_guidance(user_title, locale))
}

/// Whether [`reconcile_guidance`] has ever written the guidance file here.
pub fn guidance_rendered() -> bool {
    guidance_path().is_some_and(|p| p.exists())
}

/// The text handed to `--append-system-prompt-file`: lessons, then the
/// concept guidance. `None` when there is nothing to add.
fn system_prompt_text() -> Option<String> {
    let lessons = crate::lessons_store::managed_file_content().unwrap_or_default();
    // No reconcile has run on this host yet (a `fleet serve` nobody configured).
    // Render with the defaults rather than start the session with no guidance.
    let guidance = guidance_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .unwrap_or_else(|| render_guidance("", "en"));
    let text = join_sections(vec![lessons, guidance]);
    (!text.is_empty()).then_some(text)
}

// ── Settings / MCP files ─────────────────────────────────────────────────────

/// The `--settings` document. `fleet_bin` is `None` when no fleet binary
/// resolves, in which case only the settings that need no binary are kept.
fn settings_value(fleet_bin: Option<&str>) -> Value {
    let mut v = json!({
        "permissions": { "allow": crate::permissions_injector::INJECT_RULES },
        // Both spellings: only the old `includeCoAuthoredBy` actually reaches
        // the system prompt (see `hooks::apply_no_commit_attribution`).
        "includeCoAuthoredBy": false,
        "attribution": { "commitTrailers": false, "sessionUrl": false },
    });
    if let Some(bin) = fleet_bin {
        v["hooks"] = crate::hooks::launch_hooks(bin, is_enabled);
    }
    v
}

fn mcp_value(fleet_bin: &str) -> Value {
    json!({
        "mcpServers": {
            crate::mcp_injector::FLEET_SERVER_KEY: crate::mcp_injector::build_fleet_entry(fleet_bin),
        }
    })
}

/// Write `content` to `path` unless it already holds exactly that, via a
/// temp file and a rename so a concurrently starting `claude` never reads a
/// half-written file.
fn write_if_changed(path: &Path, content: &str) -> Result<(), String> {
    if fs::read_to_string(path).is_ok_and(|old| old == content) {
        return Ok(());
    }
    let parent = path.parent().ok_or("path has no parent")?;
    fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::write(&tmp, content).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename into {}: {e}", path.display())
    })
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter()
        .any(|a| a == flag || a.starts_with(&format!("{flag}=")))
}

/// The arguments that make a `claude` launch a Fleet session. `existing` is
/// the argv the caller already built; flags it sets itself (`--model`,
/// `--permission-prompt-tool`) are not added twice.
///
/// Best-effort: a file that cannot be written drops that one flag and is
/// logged, rather than failing the spawn — a session without Fleet's hooks is
/// degraded, a session that never starts is lost.
pub fn fleet_launch_args(existing: &[String]) -> Vec<String> {
    let fleet_bin = crate::hooks::resolve_fleet_binary();
    launch_args_with(existing, fleet_bin.as_deref())
}

fn launch_args_with(existing: &[String], fleet_bin: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Some(dir) = launch_dir() else {
        return out;
    };
    let mut emit = |flag: &str, file: &str, value: String| {
        let path = dir.join(file);
        match write_if_changed(&path, &value) {
            Ok(()) => {
                out.push(flag.to_string());
                out.push(path.to_string_lossy().into_owned());
                true
            }
            Err(e) => {
                crate::log_debug(&format!("claude_launch: dropping {flag}: {e}"));
                false
            }
        }
    };

    let settings = serde_json::to_string_pretty(&settings_value(fleet_bin)).unwrap_or_default();
    emit("--settings", SETTINGS_FILE, settings);

    let mcp_ok = fleet_bin.is_some_and(|bin| {
        let mcp = serde_json::to_string_pretty(&mcp_value(bin)).unwrap_or_default();
        emit("--mcp-config", MCP_FILE, mcp)
    });

    if let Some(text) = system_prompt_text() {
        emit("--append-system-prompt-file", SYSTEM_PROMPT_FILE, text);
    }

    // Naming a tool the CLI cannot resolve aborts it at startup, so this rides
    // on the MCP config actually having been handed over.
    if mcp_ok && !has_flag(existing, "--permission-prompt-tool") {
        out.push("--permission-prompt-tool".to_string());
        out.push(crate::session_launch::PERMISSION_PROMPT_TOOL.to_string());
    }

    if !has_flag(existing, "--model") {
        if let Some(model) = default_model() {
            out.push("--model".to_string());
            out.push(model);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Home {
        dir: PathBuf,
        _fleet: crate::paths::FleetHomeGuard,
    }

    impl Home {
        fn new(tag: &str) -> Self {
            let fleet = crate::paths::fleet_home_guard_with(|| {
                let dir = std::env::temp_dir().join(format!(
                    "fleet-claude-launch-{tag}-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                fs::create_dir_all(&dir).unwrap();
                dir
            });
            Self {
                dir: fleet.home().to_path_buf(),
                _fleet: fleet,
            }
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    const BIN: &str = "/opt/fleet/bin/fleet";

    fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.windows(2)
            .find(|w| w[0] == flag)
            .map(|w| w[1].as_str())
    }

    fn read_json(path: &str) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn subcommands(settings: &Value) -> String {
        settings["hooks"].to_string()
    }

    #[test]
    fn a_fresh_host_gets_every_hook_permission_and_mcp_server() {
        let _h = Home::new("fresh");
        let args = launch_args_with(&[], Some(BIN));

        let settings = read_json(value_after(&args, "--settings").expect("--settings"));
        let hooks = subcommands(&settings);
        for sub in [
            "guard",
            "elicitation",
            "plan-approval",
            "prd-context",
            "notes-hint",
            "recent-sessions",
            "ctx-reminder",
            "wakeup-guard",
            "session idle",
            "session resume",
            ".fleet/hooks.jsonl",
        ] {
            assert!(hooks.contains(sub), "missing `{sub}` hook in {hooks}");
        }
        let allow = settings["permissions"]["allow"].as_array().unwrap();
        assert!(allow.iter().any(|r| r == "Bash(*)"));
        assert!(allow.iter().any(|r| r == "mcp__fleet__fleet__ask"));
        assert_eq!(settings["includeCoAuthoredBy"], json!(false));
        assert_eq!(settings["attribution"]["commitTrailers"], json!(false));

        let mcp = read_json(value_after(&args, "--mcp-config").expect("--mcp-config"));
        assert_eq!(mcp["mcpServers"]["fleet"]["command"], json!(BIN));
        assert_eq!(
            value_after(&args, "--permission-prompt-tool"),
            Some(crate::session_launch::PERMISSION_PROMPT_TOOL)
        );
    }

    #[test]
    fn a_disabled_feature_leaves_its_hooks_out() {
        let _h = Home::new("disabled");
        crate::control_plane_prefs::mark_disabled(Feature::GuardHook).unwrap();
        crate::control_plane_prefs::mark_disabled(Feature::IdleHooks).unwrap();

        let args = launch_args_with(&[], Some(BIN));
        let hooks = subcommands(&read_json(value_after(&args, "--settings").unwrap()));
        assert!(!hooks.contains("\" guard;"), "guard must be off: {hooks}");
        assert!(!hooks.contains("session idle"), "idle must be off: {hooks}");
        assert!(hooks.contains("elicitation"), "others stay on: {hooks}");
    }

    #[test]
    fn without_a_fleet_binary_no_mcp_or_prompt_tool_is_named() {
        // Naming an unresolvable permission-prompt tool aborts the CLI.
        let _h = Home::new("nobin");
        let args = launch_args_with(&[], None);
        assert!(value_after(&args, "--mcp-config").is_none());
        assert!(value_after(&args, "--permission-prompt-tool").is_none());
        let settings = read_json(value_after(&args, "--settings").unwrap());
        assert!(settings.get("hooks").is_none());
        assert!(settings["permissions"]["allow"].is_array());
    }

    #[test]
    fn the_default_model_is_added_only_when_the_caller_named_none() {
        let _h = Home::new("model");
        assert!(value_after(&launch_args_with(&[], Some(BIN)), "--model").is_none());

        set_default_model("opus[1m]").unwrap();
        let args = launch_args_with(&[], Some(BIN));
        assert_eq!(value_after(&args, "--model"), Some("opus[1m]"));

        let explicit = vec!["--model".to_string(), "sonnet".to_string()];
        assert!(value_after(&launch_args_with(&explicit, Some(BIN)), "--model").is_none());
    }

    #[test]
    fn a_caller_supplied_prompt_tool_is_not_duplicated() {
        let _h = Home::new("ppt");
        let existing = vec![
            "--permission-prompt-tool".to_string(),
            crate::session_launch::PERMISSION_PROMPT_TOOL.to_string(),
        ];
        let args = launch_args_with(&existing, Some(BIN));
        assert!(!args.iter().any(|a| a == "--permission-prompt-tool"));
    }

    #[test]
    fn the_system_prompt_carries_enabled_guidance_only() {
        let _h = Home::new("guidance");
        crate::control_plane_prefs::mark_disabled(Feature::WikiGuidance).unwrap();
        reconcile_guidance("老板", "zh").unwrap();

        let args = launch_args_with(&[], Some(BIN));
        let text =
            fs::read_to_string(value_after(&args, "--append-system-prompt-file").unwrap()).unwrap();
        let interaction = crate::interaction_mode::render_guidance("老板", "zh");
        let prd = crate::prd_discipline::render_guidance("老板", "zh");
        assert!(text.contains(interaction.trim_end()));
        assert!(text.contains(prd.trim_end()));
        assert!(text.find(interaction.trim_end()) < text.find(prd.trim_end()));
        assert!(!text.contains(crate::wiki_guidance::render_guidance("zh").trim_end()));
    }

    #[test]
    fn launch_files_are_byte_stable_across_launches() {
        // A fork reuses its parent's prompt cache only if the system prompt it
        // is handed is byte-identical; nothing here may vary per launch.
        let _h = Home::new("stable");
        reconcile_guidance("Boss", "en").unwrap();
        let first = launch_args_with(&[], Some(BIN));
        let snap = |args: &[String]| -> Vec<String> {
            args.windows(2)
                .filter(|w| w[0].starts_with("--settings") || w[0].starts_with("--mcp") || w[0].starts_with("--append"))
                .map(|w| fs::read_to_string(&w[1]).unwrap())
                .collect()
        };
        let before = snap(&first);
        let second = launch_args_with(&[], Some(BIN));
        assert_eq!(first, second);
        assert_eq!(before, snap(&second));
    }

    #[test]
    fn nothing_enabled_and_no_lessons_passes_no_system_prompt() {
        let _h = Home::new("empty");
        for f in [
            Feature::InteractionMode,
            Feature::PrdDiscipline,
            Feature::WikiGuidance,
            Feature::ModelGuidance,
            Feature::SessionTitleGuidance,
        ] {
            crate::control_plane_prefs::mark_disabled(f).unwrap();
        }
        reconcile_guidance("Boss", "en").unwrap();
        let args = launch_args_with(&[], Some(BIN));
        assert!(value_after(&args, "--append-system-prompt-file").is_none());
    }
}
