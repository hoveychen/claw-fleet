//! The one-shot migration that takes back what Fleet used to write into the
//! user's global agent config.
//!
//! Fleet used to govern every `claude` and `codex` on the machine through
//! global files: hooks, `model` and commit-attribution keys in
//! `~/.claude/settings.json`, guidance and lessons `@import`s in
//! `~/.claude/CLAUDE.md`, `mcpServers.fleet` in `~/.claude.json`, blocks in
//! `~/.codex/AGENTS.md`. Sessions the user opened by hand behaved differently
//! once Fleet was installed. Fleet now hands all of that to the sessions it
//! starts as launch arguments ([`crate::claude_launch`],
//! [`crate::codex_launch`]); this module removes the global copies.
//!
//! Three phases, each recorded by its own marker under `~/.fleet/migrations/`
//! so a failure part-way retries only what is left:
//!
//! 1. **Snapshot.** What is installed on disk is the only record of which
//!    features the user had on, and phase 2 erases it. So first, on a host
//!    that carried any of Fleet's global config, every feature *not* installed
//!    is recorded as switched off in [`crate::control_plane_prefs`], and the
//!    global `model` becomes Fleet's launch default. A host that never carried
//!    any is left with every feature on — the fresh-install default.
//! 2. **Strip.** Every global write goes, through the modules' `_inner`
//!    removers, which record nothing in prefs: this is Fleet moving its config,
//!    not the user switching features off.
//! 3. **Permissions.** Fleet's `permissions.allow` rules go last, and only once
//!    no Fleet session started by an older build is still running. Those
//!    sessions got the rules from the global file alone, and Claude Code
//!    re-reads permissions while it runs: pulling them mid-session leaves a
//!    headless session on a prompt nobody can answer. Sessions started since
//!    carry the rules in their own `--settings` and are not waited for.
//!    Until then [`run`] retries this phase on every call — the desktop and
//!    `fleet serve` tickers call it every 30s.

use std::fs;
use std::path::PathBuf;

use crate::control_plane::Step;
use crate::control_plane_prefs::{is_disabled, mark_disabled, Feature};

const MIGRATIONS_DIR: &str = "migrations";
const SNAPSHOT_MARKER: &str = "scope-per-launch-v1.snapshot";
const STRIP_MARKER: &str = "scope-per-launch-v1.strip";
const DONE_MARKER: &str = "scope-per-launch-v1.done";

/// The env var every Fleet spawn sets (`session_launch`); nothing else sets it
/// to this value, so it identifies a Fleet-started `claude` process.
const FLEET_SPAWN_ENV: &str = "CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS=0";

/// Minimum time between two process scans while phase 3 waits.
const LEGACY_SCAN_INTERVAL_SECS: u64 = 60;

fn marker(name: &str) -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join(MIGRATIONS_DIR).join(name))
}

fn has_marker(name: &str) -> bool {
    marker(name).is_some_and(|p| p.exists())
}

fn set_marker(name: &str) -> Result<(), String> {
    let path = marker(name).ok_or("cannot determine fleet dir")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let stamp = chrono::Utc::now().to_rfc3339();
    fs::write(&path, stamp).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Whether any part of `feature` is in the global config right now.
fn installed_anywhere(feature: Feature, plan: &crate::hooks::HookSetupPlan) -> bool {
    crate::control_plane::is_installed(feature, plan) || crate::hooks::any_hook_of_installed(feature)
}

/// Phase 1. Returns the steps it took; empty on a host with nothing to record.
fn snapshot() -> Vec<Step> {
    let plan = crate::hooks::global_hook_setup();
    let installed: Vec<(Feature, bool)> = Feature::ALL
        .iter()
        .map(|&f| (f, installed_anywhere(f, &plan)))
        .collect();
    let mut steps = Vec::new();

    // A host with none of Fleet's global config never had anything switched
    // on *or* off — recording every feature as off would leave it bare.
    if installed.iter().any(|&(_, on)| on) {
        for &(feature, on) in &installed {
            if !on && !is_disabled(feature) {
                steps.push(Step {
                    name: feature.key(),
                    result: mark_disabled(feature),
                });
            }
        }
    }
    steps
}

/// Phase 2: remove every global write. Each step is idempotent.
fn strip() -> Vec<Step> {
    let mut steps = Vec::new();
    let mut step = |name: &'static str, result: Result<(), String>| {
        steps.push(Step { name, result });
    };

    step(
        "settings_hooks",
        crate::hooks::strip_all_fleet_hooks().map(|_| ()),
    );
    step("settings_values", strip_settings_values());
    step(
        "interaction_mode",
        crate::interaction_mode::remove_interaction_mode_inner(),
    );
    step(
        "prd_discipline",
        crate::prd_discipline::remove_prd_discipline_inner(),
    );
    step(
        "wiki_guidance",
        crate::wiki_guidance::remove_wiki_guidance_inner(),
    );
    step(
        "model_guidance",
        crate::model_guidance::remove_model_guidance_inner(),
    );
    step(
        "session_title_guidance",
        crate::session_title_guidance::remove_inner(),
    );
    step("lessons_import", crate::lessons_store::remove_import());
    step("claude_md", remove_claude_md_if_blank());
    step(
        "mcp_server",
        crate::mcp_injector::strip_fleet_server()
            .map(|_| ())
            .map_err(|e| format!("~/.claude.json: {e}")),
    );
    step("codex_agents_md", crate::codex_guidance::strip_agents_md());
    steps
}

/// Drop `model` and the attribution switches from the global settings, and
/// keep the model as Fleet's own launch default — unless Fleet already has
/// one, which is the more deliberate of the two.
fn strip_settings_values() -> Result<(), String> {
    let stripped = crate::hooks::strip_fleet_settings_values()?;
    if let Some(model) = stripped.model {
        if crate::claude_launch::load_config().default_model.is_none() {
            crate::claude_launch::set_default_model(&model)?;
        }
    }
    Ok(())
}

/// Delete `~/.claude/CLAUDE.md` when nothing but whitespace is left in it.
///
/// Each remover leaves the blank lines around its block behind, so after all of
/// them a file that only ever held Fleet's imports is a page of newlines —
/// harmless to Claude Code, but not the file the user would have had without
/// Fleet, which is none.
fn remove_claude_md_if_blank() -> Result<(), String> {
    let Some(path) = crate::session::get_claude_dir().map(|d| d.join("CLAUDE.md")) else {
        return Ok(());
    };
    crate::claude_md_lock::with_lock(&path, || {
        let blank = fs::read_to_string(&path).is_ok_and(|c| c.trim().is_empty());
        if blank {
            fs::remove_file(&path).map_err(|e| format!("remove {}: {e}", path.display()))?;
        }
        Ok::<(), String>(())
    })
}

/// Whether a process is a Fleet session that depends on the global allow
/// rules: a `claude` Fleet spawned (by env) without a `--settings` file from
/// [`crate::claude_launch`], i.e. by a build that predates per-launch settings.
fn is_legacy_fleet_session(cmd: &[String], env: &[String]) -> bool {
    let is_claude = cmd
        .first()
        .map(|c| c.rsplit(['/', '\\']).next().unwrap_or(c))
        .is_some_and(|b| b == "claude" || b == "claude.exe");
    let fleet_spawned = env.iter().any(|e| e == FLEET_SPAWN_ENV);
    let per_launch = cmd.windows(2).any(|w| {
        w[0] == "--settings" && w[1].replace('\\', "/").contains("/claude-launch/")
    });
    is_claude && fleet_spawned && !per_launch
}

fn legacy_fleet_sessions_alive() -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut sys = System::new();
    // cmd + environ only: refreshing cwd for every process on macOS raises TCC
    // prompts for unrelated apps (see `orphan_reaper`).
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_environ(UpdateKind::Always),
    );
    let strings = |v: &[std::ffi::OsString]| -> Vec<String> {
        v.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    };
    sys.processes()
        .values()
        .any(|p| is_legacy_fleet_session(&strings(p.cmd()), &strings(p.environ())))
}

/// [`legacy_fleet_sessions_alive`], but a busy answer is reused for a minute:
/// the scan reads every process's environment, and phase 3 can wait hours.
fn legacy_sessions_alive_throttled() -> bool {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST_BUSY_SCAN: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let last = LAST_BUSY_SCAN.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < LEGACY_SCAN_INTERVAL_SECS {
        return true;
    }
    let alive = legacy_fleet_sessions_alive();
    LAST_BUSY_SCAN.store(if alive { now } else { 0 }, Ordering::Relaxed);
    alive
}

/// Phase 3, unless older Fleet sessions are still alive.
fn permissions(legacy_alive: &dyn Fn() -> bool) -> Option<Step> {
    if legacy_alive() {
        return None;
    }
    Some(Step {
        name: "permissions",
        result: crate::permissions_injector::deactivate()
            .map_err(|e| format!("settings.json permissions: {e}")),
    })
}

/// Run whatever is left of the migration. Cheap once done — one `stat` — so
/// every long-lived Fleet process calls it at startup and on its ticker.
///
/// Returns the steps taken this call; empty when there was nothing left to do.
/// A phase whose steps did not all succeed is not marked done and runs again
/// next time.
pub fn run() -> Vec<Step> {
    run_with(&legacy_sessions_alive_throttled)
}

fn run_with(legacy_alive: &dyn Fn() -> bool) -> Vec<Step> {
    if has_marker(DONE_MARKER) {
        return Vec::new();
    }
    let mut steps = Vec::new();

    if !has_marker(SNAPSHOT_MARKER) {
        let snap = snapshot();
        let ok = snap.iter().all(|s| s.result.is_ok());
        steps.extend(snap);
        if !ok {
            return steps;
        }
        if let Err(e) = set_marker(SNAPSHOT_MARKER) {
            // Without the marker a retry would snapshot a half-stripped host.
            steps.push(Step {
                name: "snapshot_marker",
                result: Err(e),
            });
            return steps;
        }
    }

    if !has_marker(STRIP_MARKER) {
        let stripped = strip();
        let ok = stripped.iter().all(|s| s.result.is_ok());
        steps.extend(stripped);
        if !ok {
            return steps;
        }
        if let Err(e) = set_marker(STRIP_MARKER) {
            steps.push(Step {
                name: "strip_marker",
                result: Err(e),
            });
            return steps;
        }
    }

    let Some(perms) = permissions(legacy_alive) else {
        return steps;
    };
    let ok = perms.result.is_ok();
    steps.push(perms);
    if ok {
        if let Err(e) = set_marker(DONE_MARKER) {
            steps.push(Step {
                name: "done_marker",
                result: Err(e),
            });
        }
    }
    steps
}

/// Log the steps [`run`] took, one line each. Silent when it took none.
pub fn run_and_log(caller: &str) {
    for step in run() {
        match &step.result {
            Ok(()) => crate::log_debug(&format!("scope migration ({caller}): {}", step.name)),
            Err(e) => crate::log_debug(&format!(
                "scope migration ({caller}): {} failed: {e}",
                step.name
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    struct Home {
        dir: PathBuf,
        _fleet: crate::paths::FleetHomeGuard,
    }

    impl Home {
        fn new(tag: &str) -> Self {
            let fleet = crate::paths::fleet_home_guard_with(|| {
                let dir = std::env::temp_dir().join(format!(
                    "fleet-scope-migration-{tag}-{}-{}",
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

        fn claude(&self) -> PathBuf {
            crate::session::get_claude_dir().unwrap()
        }

        fn settings(&self) -> Value {
            fs::read_to_string(self.claude().join("settings.json"))
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(json!({}))
        }

        fn write_settings(&self, v: &Value) {
            fs::create_dir_all(self.claude()).unwrap();
            fs::write(self.claude().join("settings.json"), v.to_string()).unwrap();
        }

        /// What an older Fleet's guidance apply left behind: the rendered file
        /// plus its `@import` block appended to CLAUDE.md.
        fn write_legacy_guidance(&self, name: &str) {
            fs::create_dir_all(self.claude()).unwrap();
            let file = self.claude().join(format!("fleet-{name}.md"));
            fs::write(&file, "old guidance\n").unwrap();
            let md = self.claude().join("CLAUDE.md");
            let mut content = fs::read_to_string(&md).unwrap_or_default();
            if !content.is_empty() {
                content.push('\n');
            }
            content.push_str(&format!(
                "<!-- fleet:{name}:begin -->\n@{}\n<!-- fleet:{name}:end -->\n",
                file.display()
            ));
            fs::write(&md, content).unwrap();
        }

        fn claude_json(&self) -> Value {
            fs::read_to_string(crate::session::get_claude_config_json().unwrap())
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(json!({}))
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// No older Fleet session alive — the tests must not depend on what the
    /// developer's machine is running.
    fn quiet() -> Vec<Step> {
        run_with(&|| false)
    }

    const BIN: &str = "/Applications/Claw Fleet.app/Contents/MacOS/fleet";

    fn fleet_hook(sub: &str) -> Value {
        json!({ "type": "command", "command": format!(
            r#"sh -c 'if [ -x "{BIN}" ]; then exec "{BIN}" {sub}; else exit 0; fi'"#
        ) })
    }

    /// What the old global writers left on a typical host: event-log groups,
    /// the guard, both idle hooks, one prd-context part, a user hook, the
    /// model and attribution keys, user permissions and an unrelated key.
    fn legacy_settings() -> Value {
        json!({
            "model": "opus[1m]",
            "includeCoAuthoredBy": false,
            "attribution": { "commitTrailers": false, "sessionUrl": false },
            "theme": "auto",
            "hooks": {
                "PreToolUse": [
                    { "hooks": [{ "type": "command", "command": r#"sh -c 'cat >> "$HOME/.fleet/hooks.jsonl"'"#, "async": true }] },
                    { "matcher": "Bash|PowerShell", "hooks": [fleet_hook("guard")] },
                    { "matcher": "Edit", "hooks": [{ "type": "command", "command": "my-linter" }] },
                ],
                "Stop": [ { "hooks": [fleet_hook("session idle")] } ],
                "UserPromptSubmit": [
                    { "hooks": [fleet_hook("session resume")] },
                    { "hooks": [fleet_hook("prd-context")] },
                ],
            }
        })
    }

    #[test]
    fn a_fleet_host_keeps_its_switches_and_loses_every_global_write() {
        let h = Home::new("legacy");
        h.write_settings(&legacy_settings());
        h.write_legacy_guidance("interaction-mode");
        fs::write(
            crate::session::get_claude_config_json().unwrap(),
            json!({ "numStartups": 3, "mcpServers": {
                "fleet": { "command": BIN, "args": ["mcp"] },
                "other": { "command": "x" },
            }})
            .to_string(),
        )
        .unwrap();

        let steps = quiet();
        for s in &steps {
            assert!(s.result.is_ok(), "{} failed: {:?}", s.name, s.result);
        }

        // Switches: what was installed stays on, what was absent reads off.
        for on in [
            Feature::GuardHook,
            Feature::IdleHooks,
            Feature::PrdContextHook,
            Feature::InteractionMode,
        ] {
            assert!(!is_disabled(on), "{} was installed", on.key());
        }
        for off in [
            Feature::ElicitationHook,
            Feature::PlanApprovalHook,
            Feature::WakeupGuardHook,
            Feature::PrdDiscipline,
            Feature::WikiGuidance,
        ] {
            assert!(is_disabled(off), "{} was absent", off.key());
        }
        assert_eq!(
            crate::claude_launch::load_config().default_model.as_deref(),
            Some("opus[1m]")
        );

        // Global writes: gone, the user's own config untouched.
        let settings = h.settings();
        assert_eq!(
            settings,
            json!({
                "theme": "auto",
                "hooks": { "PreToolUse": [
                    { "matcher": "Edit", "hooks": [{ "type": "command", "command": "my-linter" }] }
                ]}
            })
        );
        assert!(!h.claude().join("CLAUDE.md").exists(), "only Fleet's imports were in it");
        assert!(!h.claude().join("fleet-interaction-mode.md").exists());
        assert_eq!(
            h.claude_json(),
            json!({ "numStartups": 3, "mcpServers": { "other": { "command": "x" } } })
        );

        assert!(quiet().is_empty(), "done means done");
    }

    #[test]
    fn a_host_without_fleet_config_keeps_every_feature_on() {
        let h = Home::new("fresh");
        h.write_settings(&json!({ "theme": "dark" }));
        for s in quiet() {
            assert!(s.result.is_ok(), "{} failed: {:?}", s.name, s.result);
        }
        for f in Feature::ALL {
            assert!(!is_disabled(f), "{} must stay on", f.key());
        }
        assert_eq!(h.settings(), json!({ "theme": "dark" }));
    }

    #[test]
    fn the_users_own_claude_md_and_true_attribution_survive() {
        let h = Home::new("user");
        h.write_settings(&json!({
            "attribution": { "commitTrailers": true, "sessionUrl": false },
            "hooks": { "Stop": [ { "hooks": [fleet_hook("session idle")] } ] },
        }));
        fs::write(h.claude().join("CLAUDE.md"), "# Mine\n\nbe terse\n").unwrap();
        h.write_legacy_guidance("prd-discipline");

        for s in quiet() {
            assert!(s.result.is_ok(), "{} failed: {:?}", s.name, s.result);
        }
        assert_eq!(
            h.settings(),
            json!({ "attribution": { "commitTrailers": true } })
        );
        let md = fs::read_to_string(h.claude().join("CLAUDE.md")).unwrap();
        assert!(md.contains("be terse"));
        assert!(!md.contains("fleet:"), "{md}");
    }

    #[test]
    fn a_same_named_mcp_server_that_is_not_fleets_stays() {
        let h = Home::new("mcp");
        let theirs = json!({ "mcpServers": { "fleet": { "command": "/usr/bin/fleet-of-ships" } } });
        fs::write(crate::session::get_claude_config_json().unwrap(), theirs.to_string()).unwrap();
        for s in quiet() {
            assert!(s.result.is_ok(), "{} failed: {:?}", s.name, s.result);
        }
        assert_eq!(h.claude_json(), theirs);
    }

    #[test]
    fn a_retry_after_the_snapshot_does_not_snapshot_the_stripped_host() {
        // Phase 2 erases the evidence phase 1 reads; re-running phase 1 over a
        // half-stripped host would switch everything off.
        let h = Home::new("retry");
        h.write_settings(&legacy_settings());
        set_marker(SNAPSHOT_MARKER).unwrap();
        crate::hooks::strip_all_fleet_hooks().unwrap();

        for s in quiet() {
            assert!(s.result.is_ok(), "{} failed: {:?}", s.name, s.result);
        }
        assert!(!is_disabled(Feature::GuardHook));
    }

    #[test]
    fn the_allow_rules_wait_for_older_fleet_sessions_to_exit() {
        let h = Home::new("perms");
        h.write_settings(&json!({
            "permissions": { "allow": ["Bash(*)", "Bash(ls)"], "deny": ["Bash(rm:*)"] },
            "hooks": { "Stop": [ { "hooks": [fleet_hook("session idle")] } ] },
        }));

        let busy = run_with(&|| true);
        assert!(busy.iter().all(|s| s.name != "permissions"));
        assert_eq!(
            h.settings(),
            json!({ "permissions": { "allow": ["Bash(*)", "Bash(ls)"], "deny": ["Bash(rm:*)"] } }),
            "hooks go at once, the rules stay while older sessions run"
        );
        assert!(!has_marker(DONE_MARKER));

        let later = run_with(&|| false);
        assert_eq!(later.iter().map(|s| s.name).collect::<Vec<_>>(), vec!["permissions"]);
        assert_eq!(
            h.settings(),
            json!({ "permissions": { "allow": ["Bash(ls)"], "deny": ["Bash(rm:*)"] } })
        );
        assert!(has_marker(DONE_MARKER));
    }

    #[test]
    fn only_fleet_spawned_claude_without_launch_settings_is_waited_for() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let fleet_env = v(&["HOME=/u", FLEET_SPAWN_ENV]);
        assert!(is_legacy_fleet_session(
            &v(&["/usr/local/bin/claude", "-p", "--resume", "x"]),
            &fleet_env
        ));
        assert!(!is_legacy_fleet_session(
            &v(&["claude", "--settings", "/u/.fleet/claude-launch/settings.json"]),
            &fleet_env
        ), "a per-launch session carries its own rules");
        assert!(!is_legacy_fleet_session(&v(&["claude"]), &v(&["HOME=/u"])), "hand-opened");
        assert!(!is_legacy_fleet_session(&v(&["/bin/sleep", "60"]), &fleet_env));
    }
}
