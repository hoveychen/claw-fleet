pub mod account;
pub mod acp;
pub mod agent_loop;
pub mod agent_source;
pub mod artifacts;
pub mod atomic_json;
pub mod audit;
pub mod auto_resume;
pub mod bg_guard;
pub mod bom;
pub mod browse_paths;
pub mod chain_completion_gate;
pub mod chat_workspace;
pub mod claude_analyze;
pub mod claude_binary;
pub mod claude_cli;
pub(crate) mod claude_md_block;
pub mod claude_md_lock;
pub mod claude_source;
pub mod cmd_ast;
pub mod codex_guidance;
pub mod codex_explain;
pub mod codex_image;
pub mod codex_launch;
pub mod codex_source;
pub mod codex_usage_history;
pub mod console;
pub mod consumer_heartbeat;
pub mod context_files;
pub mod context_pressure;
pub mod control_plane;
pub mod control_plane_prefs;
pub mod daily_report;
pub mod decision_history;
pub mod decision_panel_config;
#[cfg(windows)]
pub mod dpapi;
pub mod dsh_attachments;
pub mod dsh_balance;
pub mod dsh_chat_preset;
pub mod dsh_client;
pub mod dsh_cost;
pub mod dsh_decisions;
pub mod dsh_events;
pub mod dsh_guidance;
pub mod dsh_messages;
pub mod dsh_plugin;
pub mod dsh_server;
pub mod dsh_source;
pub mod dsh_speed;
pub mod elicitation;
pub mod feature_flags;
pub mod file_explorer;
pub mod fleet_cli;
pub mod fleet_event;
pub mod foxy;
pub mod git_ops;
pub mod guard;
pub mod handoff;
pub mod harness_install;
pub mod harness_login;
pub mod harness_status;
pub mod headless_runtime;
pub mod hook_timing;
pub mod hooks;
pub mod hooks_server;
pub mod host_identity;
pub mod idle;
pub mod idle_spin;
pub mod image_api;
pub mod injector_watchdog;
pub mod interaction_mode;
pub mod interaction_mode_diagnostics;
pub mod interaction_mode_test;
pub mod jsonl_tail;
pub mod lan_access;
pub mod launch_spec;
pub mod launchd;
pub mod lessons_store;
pub mod live_inject;
pub mod live_thinking;
pub mod llm_provider;
pub mod llm_usage;
pub mod mcp_a2ui_ipc;
pub mod mcp_control;
pub mod mcp_injector;
pub mod mcp_inspect;
pub mod mcp_ipc;
pub mod mcp_server;
pub mod memory;
pub mod message_trim;
pub mod mirror_guard;
pub mod mobile_relay;
pub mod model_catalog;
pub mod model_cost;
pub mod model_guidance;
pub mod off_runtime;
pub mod orphan_reaper;
pub mod parked;
pub mod pattern_update;
pub mod pending_decisions;
pub mod pending_message;
pub mod permission_prompt_ipc;
pub mod permissions_injector;
pub mod proc_runner;
pub mod process_util;
pub mod queued_command;
pub mod relay_crypto;
pub mod relay_region;
pub mod relay_role;
pub mod routes;
pub mod ui_types;
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

    /// The whole `FLEET_HOME` claim in one value: takes [`fleet_home_lock`],
    /// points `FLEET_HOME` at `dir`, and puts the previous value back when it
    /// drops.
    ///
    /// Prefer this over calling [`fleet_home_lock`] and `set_var` by hand. Two
    /// bugs keep coming back from the hand-rolled version, and this type is
    /// immune to both:
    ///
    /// 1. **A forgotten lock.** `FLEET_HOME` is process-global, so a test that
    ///    sets it without the lock silently redirects whatever a sibling test
    ///    is doing in parallel. That is what turned CI red on
    ///    `codex_image::…survives_a_stderr_flood` (2026-09-16): its marker
    ///    file landed in a neighbour's temp dir.
    /// 2. **A restore that a panic skips.** A test that restores `FLEET_HOME`
    ///    on its last line never runs that line when an assert fires, leaving
    ///    every later test in the process pointed at a deleted temp dir. A
    ///    `Drop` impl runs during unwind, so the claim is released either way.
    ///
    /// Not `#[cfg(test)]`: integration tests are separate crates and need it
    /// too. Production code must never construct one.
    pub struct FleetHomeGuard {
        home: std::path::PathBuf,
        prev: Option<std::ffi::OsString>,
        // Released only after this type's `Drop` has put `prev` back, so the
        // next waiter never observes the temp value.
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl FleetHomeGuard {
        /// The directory this guard pointed `FLEET_HOME` at.
        pub fn home(&self) -> &std::path::Path {
            &self.home
        }
    }

    /// Claim `FLEET_HOME` for `dir` until the returned guard drops.
    pub fn fleet_home_guard(dir: impl AsRef<std::path::Path>) -> FleetHomeGuard {
        let dir = dir.as_ref().to_path_buf();
        fleet_home_guard_with(|| dir)
    }

    /// Same, but `make_home` runs **after** the lock is taken.
    ///
    /// Use this whenever the directory is minted per call from a clock, a pid
    /// or a counter: the repo's usual `format!("…-{pid}-{nanos}")` temp name
    /// is only unique because the lock happens to serialise the two tests
    /// racing to build it. Mint it before the lock and two tests can land on
    /// the same path — then one removes the directory the other is still
    /// writing into, and the victim fails with whatever errno the next syscall
    /// happens to produce (EEXIST, EINVAL, …), never with anything that names
    /// the real cause. Observed 2026-09-16 in `injector_watchdog`.
    pub fn fleet_home_guard_with(make_home: impl FnOnce() -> std::path::PathBuf) -> FleetHomeGuard {
        let lock = fleet_home_lock();
        let home = make_home();
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialised by the lock this guard holds.
        unsafe { std::env::set_var("FLEET_HOME", &home) };
        FleetHomeGuard {
            home,
            prev,
            _lock: lock,
        }
    }

    impl Drop for FleetHomeGuard {
        fn drop(&mut self) {
            // SAFETY: still inside the critical section — `_lock` outlives this.
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("FLEET_HOME", v),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
        }
    }
}
pub mod plan_approval;
pub mod plan_forest;
pub mod plan_gate;
pub mod plan_ops;
pub mod plugins;
pub mod prd_context_dedup;
pub mod prd_discipline;
pub mod prd_tasks;
pub mod rate_limit_parser;
pub mod recent_sessions;
pub mod remote_disconnect;
pub mod remote_host;
pub mod remote_workspace;
pub mod scan_cache_disk;
pub mod schedule;
pub mod search_index;
pub mod session;
pub mod session_explain;
pub mod session_history;
pub mod session_launch;
pub mod session_mark;
pub mod session_notes;
pub mod session_snapshot;
pub mod session_title;
pub mod session_title_guidance;
pub mod session_todos;
pub mod skill_history;
pub mod skill_sync;
pub mod skills;
pub mod subagent_caller;
pub mod task_outcome;
pub mod task_progress;
pub mod task_review;
pub mod tcc;
pub mod today_usage;
pub mod token_analysis;
pub mod transcript_chain;
pub mod turn_completion_card;
pub mod user_attachments;
pub mod wakeup_guard;
pub mod watch;
pub mod web_assets;
pub mod wiki;
pub mod wiki_guidance;
pub mod workflow;
pub mod workflow_sidecar;
pub mod workspace_browse;
pub mod zip_stream;

use session::SessionInfo;
use std::fs;

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
        // Pins FLEET_HOME for the duration: without the lock a sibling test can
        // have it redirected at a temp dir, and then this assert inspects a
        // file the probe was never meant to reach — a false green.
        let _env_guard = crate::paths::fleet_home_lock();
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
pub fn detect_installed_tools(sessions: &[SessionInfo]) -> ui_types::DetectedTools {
    let home = session::real_home_dir();

    let (cli, _) = check_cli_installed();

    let vscode = home.as_ref().map_or(false, |h| {
        let ext_dirs = [
            h.join(".vscode").join("extensions"),
            h.join(".vscode-insiders").join("extensions"),
        ];
        ext_dirs.iter().any(|dir| {
            dir.is_dir()
                && fs::read_dir(dir).map_or(false, |entries| {
                    entries.filter_map(|e| e.ok()).any(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with("anthropic.claude-code")
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
            n.contains("intellij")
                || n.contains("webstorm")
                || n.contains("pycharm")
                || n.contains("goland")
                || n.contains("rustrover")
                || n.contains("phpstorm")
                || n.contains("rider")
                || n.contains("clion")
                || n.contains("jetbrains")
        })
    });

    let desktop = {
        #[cfg(target_os = "macos")]
        {
            std::path::Path::new("/Applications/Claude.app").exists()
        }
        #[cfg(target_os = "windows")]
        {
            std::env::var("LOCALAPPDATA").map_or(false, |appdata| {
                std::path::Path::new(&appdata)
                    .join("Programs")
                    .join("Claude")
                    .join("Claude.exe")
                    .exists()
            })
        }
        #[cfg(target_os = "linux")]
        {
            false
        }
    };

    let codex = home.as_ref().map_or(false, |h| h.join(".codex").is_dir()) || {
        #[cfg(unix)]
        {
            process_util::command("which")
                .arg("codex")
                .output()
                .map_or(false, |o| o.status.success())
        }
        #[cfg(not(unix))]
        {
            process_util::command("where")
                .arg("codex")
                .output()
                .map_or(false, |o| o.status.success())
        }
    };

    let config = agent_source::SourcesConfig::load();
    let claude_enabled = config.is_source_enabled("claude");
    let cli = cli && claude_enabled;
    let vscode = vscode && claude_enabled;
    let jetbrains = jetbrains && claude_enabled;
    let desktop = desktop && claude_enabled;
    let codex = codex && config.is_source_enabled("codex");

    ui_types::DetectedTools {
        cli,
        vscode,
        jetbrains,
        desktop,
        codex,
    }
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
pub const IMAGE_SKILL_MD: &str = include_str!("../../skills/image-generation/SKILL.md");

/// `(display name, installation detection dir, native skills dir)`.
pub const SKILL_TARGETS: &[(&str, &str, &str)] = &[
    ("Claude Code", ".claude", ".claude/skills"),
    ("Codex", ".codex", ".codex/skills"),
    ("GitHub Copilot", ".copilot", ".copilot/skills"),
    ("Gemini CLI", ".gemini", ".gemini/skills"),
];

/// One skill Fleet ships inside its own binary.
///
/// The body is `include_str!`d from `skills/<name>/SKILL.md` so the repo copy is
/// the single source of truth — editing the markdown is the whole change, and
/// nothing can drift between a Rust string and the file people read.
pub struct BundledSkill {
    /// Directory name under a runtime's `skills/`, and the skill's `name:` in
    /// the frontmatter. These must match or the runtime will not find it.
    pub name: &'static str,
    pub body: &'static str,
    /// [`SKILL_TARGETS`] display names this skill must NOT be installed into.
    pub skip_targets: &'static [&'static str],
}

impl BundledSkill {
    /// Should this skill be installed into the named target?
    pub fn applies_to(&self, target_display_name: &str) -> bool {
        !self.skip_targets.contains(&target_display_name)
    }
}

/// Every skill Fleet installs. Iterate this rather than naming skills one by
/// one — each install site got exactly one arm wrong the last time a registry
/// like this was hand-maintained.
pub const BUNDLED_SKILLS: &[BundledSkill] = &[
    BundledSkill {
        name: "fleet",
        body: FLEET_SKILL_MD,
        skip_targets: &[],
    },
    BundledSkill {
        name: "image-generation",
        body: IMAGE_SKILL_MD,
        // Codex ships its own `imagegen` skill over a built-in `image_gen`
        // tool, and `mcp_server` withholds `fleet__image` from a Codex client
        // for that reason. Installing this there would hand Codex a document
        // teaching it to use a tool it cannot see, competing with the skill it
        // already has.
        skip_targets: &["Codex"],
    },
];

/// Resolve a [`SKILL_TARGETS`] entry to absolute `(detect_dir, skills_dir)`.
///
/// Claude Code and Codex relocate their whole config dir via `$CLAUDE_CONFIG_DIR`
/// / `$CODEX_HOME`, so the table's relative `.claude` / `.codex` paths must defer
/// to those env vars — otherwise skill detection/install targets the wrong dir
/// for a relocated setup. Other tools resolve under `home` as before.
pub fn resolve_skill_target(
    name: &str,
    detect_dir: &str,
    skills_dir: &str,
    home: &std::path::Path,
) -> (std::path::PathBuf, std::path::PathBuf) {
    // Only an *explicitly set* env var relocates the dir; unset falls back to
    // `home.join(rel)` (the original behaviour), so callers that pass a scoped
    // `home` for isolation keep it.
    let env_dir = match name {
        "Claude Code" => std::env::var_os("CLAUDE_CONFIG_DIR"),
        "Codex" => std::env::var_os("CODEX_HOME"),
        _ => None,
    };
    if let Some(dir) = env_dir {
        let d = std::path::PathBuf::from(dir);
        if !d.as_os_str().is_empty() {
            let skills = d.join("skills");
            return (d, skills);
        }
    }
    (home.join(detect_dir), home.join(skills_dir))
}

#[cfg(test)]
mod bundled_skill_tests {
    use super::{BUNDLED_SKILLS, SKILL_TARGETS};

    fn skill(name: &str) -> &'static super::BundledSkill {
        BUNDLED_SKILLS
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} must be bundled"))
    }

    #[test]
    fn the_image_skill_is_never_installed_into_codex() {
        // Codex ships its own `imagegen` skill, and mcp_server withholds
        // fleet__image from a Codex client. Installing this there would teach
        // Codex to reach for a tool it cannot see.
        let image = skill("image-generation");
        assert!(!image.applies_to("Codex"));
        for (name, _, _) in SKILL_TARGETS {
            if *name != "Codex" {
                assert!(image.applies_to(name), "{name} should get the image skill");
            }
        }
    }

    #[test]
    fn the_fleet_skill_still_goes_everywhere() {
        let fleet = skill("fleet");
        for (name, _, _) in SKILL_TARGETS {
            assert!(fleet.applies_to(name), "{name} must keep the fleet skill");
        }
    }

    #[test]
    fn every_skip_target_names_a_real_target() {
        // A typo here fails open — the skill silently installs everywhere —
        // so the roster has to be checked against the target table.
        for skill in BUNDLED_SKILLS {
            for skipped in skill.skip_targets {
                assert!(
                    SKILL_TARGETS.iter().any(|(name, _, _)| name == skipped),
                    "{} skips unknown target {skipped}",
                    skill.name
                );
            }
        }
    }

    #[test]
    fn every_bundled_body_is_a_skill_whose_frontmatter_name_matches_its_dir() {
        // The runtime resolves a skill by its directory name; a frontmatter
        // `name:` that disagrees makes it unfindable.
        for skill in BUNDLED_SKILLS {
            assert!(
                skill.body.starts_with("---\n"),
                "{} must open with frontmatter",
                skill.name
            );
            let declared = skill
                .body
                .lines()
                .find_map(|l| l.strip_prefix("name: "))
                .unwrap_or_else(|| panic!("{} has no name: in frontmatter", skill.name));
            assert_eq!(declared.trim(), skill.name);
            assert!(
                skill.body.contains("description:"),
                "{} needs a description: — it is what decides whether the body loads",
                skill.name
            );
        }
    }

    #[test]
    fn the_image_skill_states_the_plan_backend_limitation() {
        // The single most surprising fact about this capability, measured
        // 2026-09-20: the plan-quota backend accepts model/quality/size and
        // ignores all three. An agent that does not know this will report a
        // `max` render that never happened.
        let body = skill("image-generation").body;
        assert!(body.contains("OPENAI_API_KEY"), "must name the gate");
        assert!(
            body.contains("ignore") || body.contains("ignored") || body.contains("ignores"),
            "must say the controls get ignored"
        );
        assert!(
            body.contains("fleet__image"),
            "must name the tool it drives"
        );
        assert!(
            body.contains("fleet__image_edit"),
            "must name the edit tool too"
        );
    }
}

#[cfg(test)]
mod resolve_skill_target_tests {
    use super::resolve_skill_target;
    use std::path::{Path, PathBuf};

    /// Restores `CLAUDE_CONFIG_DIR` on drop so a panic can't leak the override.
    struct CfgGuard(Option<std::ffi::OsString>);
    impl Drop for CfgGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.0 {
                    Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
                    None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
                }
            }
        }
    }

    #[test]
    fn claude_row_honors_config_dir_others_use_home() {
        let _g = CfgGuard(std::env::var_os("CLAUDE_CONFIG_DIR"));
        let home = Path::new("/home/x");

        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", "/relocated/agents") };
        let (detect, skills) =
            resolve_skill_target("Claude Code", ".claude", ".claude/skills", home);
        assert_eq!(detect, PathBuf::from("/relocated/agents"));
        assert_eq!(skills, PathBuf::from("/relocated/agents/skills"));

        // Non-Claude/Codex tools ignore the env and resolve under `home`.
        let (detect, skills) =
            resolve_skill_target("GitHub Copilot", ".copilot", ".copilot/skills", home);
        assert_eq!(detect, PathBuf::from("/home/x/.copilot"));
        assert_eq!(skills, PathBuf::from("/home/x/.copilot/skills"));
    }
}
