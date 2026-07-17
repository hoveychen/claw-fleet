pub mod account;
pub mod agent_loop;
pub mod agent_source;
pub mod audit;
pub mod auto_resume;
pub mod backend;
pub mod bg_guard;
pub mod bom;
pub mod claude_analyze;
pub mod claude_binary;
pub mod claude_cli;
pub mod claude_source;
pub mod chat_workspace;
pub mod cmd_ast;
pub mod codex_guidance;
pub mod codex_launch;
pub mod codex_source;
pub mod codex_usage_history;
pub mod console;
pub mod consumer_heartbeat;
pub mod daily_report;
pub mod decision_history;
pub mod decision_panel_config;
#[cfg(windows)]
pub mod dpapi;
pub mod elicitation;
pub mod file_explorer;
pub mod fleet_cli;
pub mod foxy;
pub mod git_ops;
pub mod guard;
pub mod handoff;
pub mod hooks;
pub mod hooks_server;
pub mod idle;
pub mod injector_watchdog;
pub mod interaction_mode;
pub mod interaction_mode_diagnostics;
pub mod interaction_mode_test;
pub mod jsonl_tail;
pub mod launchd;
pub mod live_thinking;
pub mod llm_provider;
pub mod llm_usage;
pub mod mcp_injector;
pub mod mcp_ipc;
pub mod mcp_a2ui_ipc;
pub mod mcp_server;
pub mod lessons_store;
pub mod memory;
pub mod message_trim;
pub mod mobile_relay;
pub mod relay_crypto;
pub mod model_cost;
pub mod model_guidance;
pub mod launch_spec;
pub mod parked;
pub mod pending_message;
pub mod pattern_update;
pub mod permission_prompt_ipc;
pub mod permissions_injector;
pub mod proc_runner;
pub mod process_util;
pub mod routes;
/// Path helpers, formerly re-exported from the (removed) `claw-fleet-task`
/// crate. `real_home_dir` / `get_fleet_dir` live in [`session`];
/// `fleet_home_lock` is the process-wide `FLEET_HOME` test mutex whose
/// canonical implementation now lives here.
pub mod paths {
    pub use crate::session::{get_fleet_dir, real_home_dir};

    /// Process-wide mutex used by tests to serialise access to the
    /// `FLEET_HOME` environment variable. Exposed unconditionally so
    /// integration tests (separate test binaries) can serialise against the
    /// same notion of "claim FLEET_HOME" as in-crate unit tests.
    pub fn fleet_home_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = LOCK.get_or_init(|| Mutex::new(()));
        match lock.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }
}
pub mod plan_approval;
pub mod plugins;
pub mod prd_discipline;
pub mod prd_tasks;
pub mod task_progress;
pub mod rate_limit_parser;
pub mod scan_cache_disk;
pub mod search_index;
pub mod session;
pub mod session_launch;
pub mod session_mark;
pub mod session_read;
pub mod session_title;
pub mod session_todos;
pub mod skill_history;
pub mod skill_sync;
pub mod skills;
pub mod tcc;
pub mod today_usage;
pub mod token_analysis;
pub mod user_attachments;
pub mod watch;
pub mod wiki;
pub mod wiki_guidance;
pub mod workflow;
pub mod workflow_sidecar;
pub mod workspace_browse;

use std::fs;
use session::SessionInfo;

pub fn log_debug(msg: &str) {
    if let Some(log_path) = debug_log_path() {
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let line = format!("[{timestamp}] {msg}\n");
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
    }
}

/// Where [`log_debug`] writes. Production: `~/.fleet/claw-fleet-debug.log`.
/// Test builds redirect to a temp-dir file: unit tests exercise the real
/// modules, whose fixture ids (`sess-idle`, `loop l1`, `watch w1`, …) would
/// otherwise interleave with production entries in the real log and drown out
/// the events a live diagnosis greps for (bit us on 2026-07-16 tracing codex
/// turn aborts).
fn debug_log_path() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return Some(std::env::temp_dir().join("claw-fleet-debug-test.log"));
    }
    session::real_home_dir().map(|h| h.join(".fleet").join("claw-fleet-debug.log"))
}

#[cfg(test)]
mod log_debug_tests {
    /// `cargo test` must never append fixture noise to the real
    /// `~/.fleet/claw-fleet-debug.log`. Probe is time-unique so reruns don't
    /// trip over lines an older (pre-fix) run leaked into the real file.
    #[test]
    fn test_build_log_debug_stays_out_of_the_real_fleet_log() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let probe = format!("log-isolation-probe-{nanos}");
        super::log_debug(&probe);

        if let Some(home) = crate::session::real_home_dir() {
            let real = home.join(".fleet").join("claw-fleet-debug.log");
            if let Ok(content) = std::fs::read_to_string(&real) {
                assert!(
                    !content.contains(&probe),
                    "test-build log_debug leaked into {}",
                    real.display()
                );
            }
        }
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
            { process_util::command("which").arg("codex").output().map_or(false, |o| o.status.success()) }
            #[cfg(not(unix))]
            { process_util::command("where").arg("codex").output().map_or(false, |o| o.status.success()) }
        };

    let config = agent_source::SourcesConfig::load();
    let claude_enabled = config.is_source_enabled("claude");
    let cli = cli && claude_enabled;
    let vscode = vscode && claude_enabled;
    let jetbrains = jetbrains && claude_enabled;
    let desktop = desktop && claude_enabled;
    let codex = codex && config.is_source_enabled("codex");

    backend::DetectedTools { cli, vscode, jetbrains, desktop, codex }
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

/// `(display name, installation detection dir, native skills dir)`.
pub const SKILL_TARGETS: &[(&str, &str, &str)] = &[
    ("Claude Code", ".claude", ".claude/skills"),
    ("Codex", ".codex", ".agents/skills"),
    ("GitHub Copilot", ".copilot", ".copilot/skills"),
    ("Gemini CLI", ".gemini", ".gemini/skills"),
];
