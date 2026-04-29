pub mod account;
pub mod agent_source;
pub mod audit;
pub mod auto_resume;
pub mod backend;
pub mod claude_analyze;
pub mod claude_binary;
pub mod claude_source;
pub mod codex_source;
pub mod consumer_heartbeat;
pub mod cursor;
pub mod daily_report;
pub mod decision_history;
pub mod elicitation;
pub mod feishu;
pub mod guard;
pub mod hooks;
pub mod interaction_mode;
pub mod jsonl_tail;
pub mod llm_provider;
pub mod llm_usage;
pub mod memory;
pub mod model_cost;
pub mod openclaw_source;
pub mod pattern_update;
pub mod plan_approval;
pub mod rate_limit_parser;
pub mod search_index;
pub mod session;
pub mod session_todos;
pub mod skill_history;
pub mod skills;
pub mod tcc;

use std::fs;
use session::SessionInfo;

pub fn log_debug(msg: &str) {
    if let Some(home) = session::real_home_dir() {
        let log_path = home.join(".fleet").join("claw-fleet-debug.log");
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let line = format!("[{timestamp}] {msg}\n");
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
    }
}

// ── Shared functions (used by both GUI app and fleet-cli probe) ──────────────

/// Detect which Claude-related tools are installed on the local machine.
pub fn detect_installed_tools(sessions: &[SessionInfo]) -> backend::DetectedTools {
    let home = session::real_home_dir();

    let (cli, _) = check_cli_installed();

    let vscode = home.as_ref().map_or(false, |h| {
        let ext_dirs = [
            h.join(".vscode").join("extensions"),
            h.join(".vscode-insiders").join("extensions"),
        ];
        ext_dirs.iter().any(|dir| {
            dir.is_dir() && fs::read_dir(dir).map_or(false, |entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.file_name().to_string_lossy().starts_with("anthropic.claude-code")
                })
            })
        })
    }) || sessions.iter().any(|s| {
        s.ide_name.as_deref().map_or(false, |name| {
            let n = name.to_lowercase();
            n.contains("vscode") || n.contains("vs code")
        })
    });

    let cursor = home.as_ref().map_or(false, |h| h.join(".cursor").is_dir());

    let openclaw = home.as_ref().map_or(false, |h| h.join(".openclaw").is_dir())
        || {
            #[cfg(unix)]
            { std::process::Command::new("which").arg("openclaw").output().map_or(false, |o| o.status.success()) }
            #[cfg(not(unix))]
            { std::process::Command::new("where").arg("openclaw").output().map_or(false, |o| o.status.success()) }
        };

    let jetbrains = sessions.iter().any(|s| {
        s.ide_name.as_deref().map_or(false, |name| {
            let n = name.to_lowercase();
            n.contains("intellij") || n.contains("webstorm") || n.contains("pycharm")
                || n.contains("goland") || n.contains("rustrover") || n.contains("phpstorm")
                || n.contains("rider") || n.contains("clion") || n.contains("jetbrains")
        })
    });

    let desktop = {
        #[cfg(target_os = "macos")]
        { std::path::Path::new("/Applications/Claude.app").exists() }
        #[cfg(target_os = "windows")]
        {
            std::env::var("LOCALAPPDATA").map_or(false, |appdata| {
                std::path::Path::new(&appdata).join("Programs").join("Claude").join("Claude.exe").exists()
            })
        }
        #[cfg(target_os = "linux")]
        { false }
    };

    let codex = home.as_ref().map_or(false, |h| h.join(".codex").is_dir())
        || {
            #[cfg(unix)]
            { std::process::Command::new("which").arg("codex").output().map_or(false, |o| o.status.success()) }
            #[cfg(not(unix))]
            { std::process::Command::new("where").arg("codex").output().map_or(false, |o| o.status.success()) }
        };

    let config = agent_source::SourcesConfig::load();
    let claude_enabled = config.is_source_enabled("claude");
    let cli = cli && claude_enabled;
    let vscode = vscode && claude_enabled;
    let jetbrains = jetbrains && claude_enabled;
    let desktop = desktop && claude_enabled;
    let cursor = cursor && config.is_source_enabled("cursor");
    let openclaw = openclaw && config.is_source_enabled("openclaw");
    let codex = codex && config.is_source_enabled("codex");

    backend::DetectedTools { cli, vscode, jetbrains, desktop, cursor, openclaw, codex }
}

/// Resolve the Claude CLI binary fleet should use, honouring the user override.
///
/// Wraps [`claude_binary::resolve`] with the persisted [`claude_binary::ClaudeBinaryConfig`]
/// override so callers don't need to thread the config through.  Returns `(found, path)`
/// for backwards compatibility with older call sites.
pub fn check_cli_installed() -> (bool, Option<String>) {
    let config = claude_binary::ClaudeBinaryConfig::load();
    match claude_binary::resolve(config.override_path.as_deref()) {
        Some(b) => (true, Some(b.path)),
        None => (false, None),
    }
}

// ── Shared constants ─────────────────────────────────────────────────────────

pub const FLEET_SKILL_MD: &str = include_str!("../../skills/fleet/SKILL.md");

pub const SKILL_TARGETS: &[(&str, &str)] = &[
    ("Claude Code", ".claude"),
    ("GitHub Copilot", ".copilot"),
    ("Cursor", ".cursor"),
    ("Gemini CLI", ".gemini"),
    ("OpenClaw", ".openclaw"),
];
