//! Launch a brand-new headless Claude Code session in a workspace.
//!
//! This hosts the generic "spawn a detached `claude` process with stderr
//! logging + a reaper thread" machinery. `auto_resume::spawn_resume` delegates
//! here for its `claude --resume <id> -p continue` shape; the sessions page's
//! "new session" button uses [`spawn_new_session`] for the
//! `claude -p "<initial prompt>"` shape. Launch identity is carried by the
//! `CLAUDE_CODE_ENTRYPOINT` env var ([`NEW_SESSION_ENTRYPOINT`]): the CLI
//! persists it into every `user` record of the session's JSONL, so the
//! scanner can classify Fleet-launched sessions from the transcript alone —
//! no registry bookkeeping (same mechanism the VS Code extension uses with
//! `claude-vscode`; verified against CLI 2.1.201).

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Request body for the remote `/spawn_session` endpoint.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SpawnSessionRequest {
    pub workspace_path: String,
    pub prompt: String,
    /// Optional `--model` override; `None`/empty = the CLI's configured default.
    #[serde(default)]
    pub model: Option<String>,
    /// Optional `--effort` level (low / medium / high / xhigh / max);
    /// `None`/empty = the CLI's configured default.
    #[serde(default)]
    pub effort: Option<String>,
    /// Optional `--permission-mode` (see [`PERMISSION_MODES`]);
    /// `None`/empty = the CLI's own default mode. Headless `-p` sessions in
    /// default mode can't approve file edits, so the launcher usually passes
    /// `acceptEdits`.
    #[serde(default)]
    pub permission_mode: Option<String>,
}

/// `CLAUDE_CODE_ENTRYPOINT` value stamped on sessions launched by the "新会话"
/// button. The CLI writes it verbatim into each `user` record's `entrypoint`
/// field, which is what the history panel filters on.
pub const NEW_SESSION_ENTRYPOINT: &str = "claw-fleet-newsession";

/// `claude --permission-mode` values accepted by the CLI (verified against
/// `claude --help`, CLI 2.1.181).
pub const PERMISSION_MODES: &[&str] = &[
    "acceptEdits",
    "auto",
    "bypassPermissions",
    "default",
    "dontAsk",
    "plan",
];

/// Response body for the remote `/spawn_session` endpoint.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SpawnSessionResponse {
    pub pid: u32,
}

/// Fully-qualified MCP tool name of Fleet's permission-prompt bridge, as the
/// Claude CLI resolves it: `mcp__<server key>__<tool name>` with server key
/// `fleet` and tool `fleet__permission_prompt`.
pub const PERMISSION_PROMPT_TOOL: &str = "mcp__fleet__fleet__permission_prompt";

/// `--permission-prompt-tool` args for headless spawns, or empty when the
/// fleet MCP server is not registered in `~/.claude.json` (naming an
/// unresolvable MCP tool makes the CLI abort at startup, so the flag is only
/// safe while the injection is live).
///
/// With the flag, a headless session's native permission prompts (tool calls
/// that are neither allowed nor denied by permission rules) surface as Fleet
/// Decision Cards instead of being silently auto-denied.
pub fn permission_prompt_tool_args() -> Vec<String> {
    if crate::mcp_injector::fleet_server_registered() {
        vec![
            "--permission-prompt-tool".to_string(),
            PERMISSION_PROMPT_TOOL.to_string(),
        ]
    } else {
        Vec::new()
    }
}

/// Spawn `claude <args>` detached in `workspace_path`, with stderr redirected
/// to `stderr_log` and a background thread that reaps the child and records
/// its exit status — so failures are not silent and we don't accumulate
/// zombies.
///
/// `label` tags the log lines (e.g. `auto_resume`, `new_session`); `detail`
/// carries per-spawn context (e.g. `session=<id>`). `on_exit(success)` fires
/// from the reaper thread when the child exits.
///
/// Returns the child pid.
pub fn spawn_claude_detached(
    claude_path: &str,
    args: &[String],
    workspace_path: &str,
    stderr_log: &Path,
    label: &str,
    detail: &str,
    on_exit: impl FnOnce(bool) + Send + 'static,
) -> Result<u32, String> {
    spawn_claude_detached_with_envs(
        claude_path,
        args,
        workspace_path,
        stderr_log,
        label,
        detail,
        &[],
        on_exit,
    )
}

/// [`spawn_claude_detached`] plus extra environment variables for the child —
/// e.g. `CLAUDE_CODE_ENTRYPOINT` so the CLI stamps the launch identity into
/// the session's JSONL.
pub fn spawn_claude_detached_with_envs(
    claude_path: &str,
    args: &[String],
    workspace_path: &str,
    stderr_log: &Path,
    label: &str,
    detail: &str,
    extra_envs: &[(&str, &str)],
    on_exit: impl FnOnce(bool) + Send + 'static,
) -> Result<u32, String> {
    if !Path::new(workspace_path).is_dir() {
        return Err(format!("Workspace directory not found: {}", workspace_path));
    }
    if let Some(parent) = stderr_log.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create stderr log dir {}: {}", parent.display(), e))?;
    }

    {
        let mut header = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(stderr_log)
            .map_err(|e| format!("open stderr log {}: {}", stderr_log.display(), e))?;
        let _ = writeln!(
            header,
            "[{}] {} spawn {} cwd={}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            label,
            detail,
            workspace_path
        );
    }

    let stderr_file = std::fs::OpenOptions::new()
        .append(true)
        .open(stderr_log)
        .map_err(|e| format!("reopen stderr log {}: {}", stderr_log.display(), e))?;

    let mut cmd = crate::process_util::command(claude_path);
    cmd.args(args)
        .current_dir(workspace_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(stderr_file));
    // Pin the child's HOME to the real home dir. Origin: the desktop app
    // used to ship sandboxed (macOS App Sandbox), where its own $HOME
    // pointed at the container (~/Library/Containers/.../Data) and an
    // inheriting claude child would write its session JSONL there —
    // invisible to the scanner, which reads the real ~/.claude/projects.
    // The sandbox is gone (entitlements.plist, 2026-07); the pin stays as
    // a cheap defence against any polluted/overridden $HOME.
    if let Some(home) = crate::session::real_home_dir() {
        cmd.env("HOME", home);
    }
    // A GUI app launched by launchd carries a minimal PATH
    // (/usr/bin:/bin:/usr/sbin:/sbin); a child inheriting it can't find
    // user-installed binaries (fleet, cws, node) from its Bash tool.
    // Prepend ~/.claude/fleet/bin (see supervisor::ensure_fleet_cli_link)
    // and the common install dirs; the parent's PATH stays at the tail.
    let mut path = crate::openclaw_source::augmented_path();
    if let Some(home) = crate::session::real_home_dir() {
        path = format!(
            "{}:{}",
            home.join(".claude").join("fleet").join("bin").display(),
            path
        );
    }
    cmd.env("PATH", path);
    for (k, v) in extra_envs {
        cmd.env(k, v);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn claude failed: {e}"))?;
    let pid = child.id();

    let label_owned = label.to_string();
    let detail_owned = detail.to_string();
    let log_path_owned = stderr_log.to_path_buf();
    std::thread::spawn(move || {
        let result = child.wait();
        let success = matches!(&result, Ok(status) if status.success());
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path_owned)
        {
            match result {
                Ok(status) => {
                    let _ = writeln!(
                        f,
                        "[{}] {} exit {} code={:?} success={}",
                        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                        label_owned,
                        detail_owned,
                        status.code(),
                        status.success(),
                    );
                }
                Err(e) => {
                    let _ = writeln!(
                        f,
                        "[{}] {} wait_err {} err={}",
                        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                        label_owned,
                        detail_owned,
                        e
                    );
                }
            }
        }
        // Notify the caller the child exited (and whether it succeeded).
        on_exit(success);
    });

    Ok(pid)
}

/// Start a brand-new headless Claude Code session: spawns
/// `claude -p "<prompt>" [--model <m>] [--effort <e>]` detached in
/// `workspace_path`. Returns as soon as the child is spawned; the session's
/// JSONL will be created by the claude process itself and discovered by the
/// scanner.
pub fn spawn_new_session(
    workspace_path: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    permission_mode: Option<&str>,
) -> Result<u32, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("prompt is required".to_string());
    }
    let permission_mode = permission_mode.map(str::trim).filter(|m| !m.is_empty());
    if let Some(m) = permission_mode {
        if !PERMISSION_MODES.contains(&m) {
            return Err(format!(
                "invalid permission mode '{}' (expected one of: {})",
                m,
                PERMISSION_MODES.join(", ")
            ));
        }
    }
    let (found, claude_path) = crate::check_cli_installed();
    if !found {
        return Err("Claude CLI not found on PATH".to_string());
    }
    let claude = claude_path.unwrap_or_else(|| "claude".to_string());
    let stderr_log = crate::session::get_fleet_dir()
        .map(|d| d.join("new_session_stderr.log"))
        .ok_or_else(|| "no fleet dir".to_string())?;
    let mut args = vec!["-p".to_string(), prompt.to_string()];
    if let Some(m) = model.map(str::trim).filter(|m| !m.is_empty()) {
        args.push("--model".to_string());
        args.push(m.to_string());
    }
    if let Some(e) = effort.map(str::trim).filter(|e| !e.is_empty()) {
        args.push("--effort".to_string());
        args.push(e.to_string());
    }
    if let Some(m) = permission_mode {
        args.push("--permission-mode".to_string());
        args.push(m.to_string());
    }
    args.extend(permission_prompt_tool_args());
    crate::log_debug(&format!(
        "new_session: claude {} <prompt {} chars> (cwd={}, stderr_log={})",
        args[2..].join(" "),
        prompt.len(),
        workspace_path,
        stderr_log.display()
    ));
    let pid = spawn_claude_detached_with_envs(
        &claude,
        &args,
        workspace_path,
        &stderr_log,
        "new_session",
        "",
        &[("CLAUDE_CODE_ENTRYPOINT", NEW_SESSION_ENTRYPOINT)],
        |_success| {},
    )?;
    crate::log_debug(&format!(
        "new_session: spawned pid {} in {}",
        pid, workspace_path
    ));
    Ok(pid)
}

#[cfg(test)]
mod tests {
    #[test]
    fn spawn_new_session_rejects_empty_prompt() {
        // Prompt validation must fire before any CLI/filesystem checks so the
        // frontend gets a stable error regardless of the host environment.
        let err = super::spawn_new_session("/", "   ", None, None, None).unwrap_err();
        assert_eq!(err, "prompt is required");
    }

    #[test]
    fn spawn_new_session_rejects_unknown_permission_mode() {
        // Same host-independence contract as the prompt check: validate the
        // mode against PERMISSION_MODES before touching the CLI/filesystem.
        let err = super::spawn_new_session("/", "hi", None, None, Some("yolo")).unwrap_err();
        assert!(
            err.contains("invalid permission mode 'yolo'"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn spawn_claude_detached_overrides_polluted_home() {
        // A spawned claude must NOT inherit a polluted $HOME (historically:
        // the App-Sandbox container path ~/Library/Containers/.../Data,
        // before the sandbox was dropped in 2026-07) — it would read config
        // and write session JSONLs there, invisible to the scanner. The
        // child has to see real_home_dir() instead.
        //
        // FLEET_HOME stands in for the real home here: it is
        // real_home_dir()'s first-priority source on every platform. Without
        // it the assertion would depend on how real_home_dir() resolves the
        // fallback — getpwuid on macOS (immune to a polluted $HOME) vs
        // dirs::home_dir() on Linux (which reads the polluted $HOME and made
        // this test fail on CI).
        let _guard = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet_test_spawn_home_{}",
            std::process::id()
        ));
        let fake_home = tmp.join("container-home");
        let real_home = tmp.join("real-home");
        std::fs::create_dir_all(&fake_home).unwrap();
        std::fs::create_dir_all(&real_home).unwrap();
        let out = tmp.join("observed-home.txt");
        let _ = std::fs::remove_file(&out);
        let log = tmp.join("stderr.log");

        let prev_home = std::env::var_os("HOME");
        let prev_fleet_home = std::env::var_os("FLEET_HOME");
        std::env::set_var("HOME", &fake_home);
        std::env::set_var("FLEET_HOME", &real_home);
        let (tx, rx) = std::sync::mpsc::channel();
        let spawn_result = super::spawn_claude_detached(
            "/bin/sh",
            &[
                "-c".to_string(),
                format!("printf %s \"$HOME\" > '{}'", out.display()),
            ],
            tmp.to_str().unwrap(),
            &log,
            "test",
            "",
            move |ok| {
                let _ = tx.send(ok);
            },
        );
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_fleet_home {
            Some(v) => std::env::set_var("FLEET_HOME", v),
            None => std::env::remove_var("FLEET_HOME"),
        }
        spawn_result.unwrap();
        let exited_ok = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("child did not exit in time");
        assert!(exited_ok, "child exited nonzero");

        let observed = std::fs::read_to_string(&out).unwrap();
        assert_eq!(
            observed,
            real_home.display().to_string(),
            "spawned child must see the real home, not the parent's polluted $HOME"
        );
    }

    #[cfg(unix)]
    #[test]
    fn spawn_claude_detached_augments_minimal_gui_path() {
        // A GUI app launched by launchd carries a minimal PATH
        // (/usr/bin:/bin:/usr/sbin:/sbin). A spawned claude inherits it, so
        // the agent's Bash can't find user-installed binaries (fleet, cws,
        // claude itself in ~/.local/bin). The child must instead see a PATH
        // covering the common install dirs plus ~/.claude/fleet/bin, with
        // the parent's PATH preserved at the tail.
        let _guard = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet_test_spawn_path_{}",
            std::process::id()
        ));
        let real_home = tmp.join("real-home");
        std::fs::create_dir_all(&real_home).unwrap();
        let out = tmp.join("observed-path.txt");
        let _ = std::fs::remove_file(&out);
        let log = tmp.join("stderr.log");

        const MINIMAL_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";
        let prev_path = std::env::var_os("PATH");
        let prev_fleet_home = std::env::var_os("FLEET_HOME");
        std::env::set_var("PATH", MINIMAL_PATH);
        std::env::set_var("FLEET_HOME", &real_home);
        let (tx, rx) = std::sync::mpsc::channel();
        let spawn_result = super::spawn_claude_detached(
            "/bin/sh",
            &[
                "-c".to_string(),
                format!("printf %s \"$PATH\" > '{}'", out.display()),
            ],
            tmp.to_str().unwrap(),
            &log,
            "test",
            "",
            move |ok| {
                let _ = tx.send(ok);
            },
        );
        match prev_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        match prev_fleet_home {
            Some(v) => std::env::set_var("FLEET_HOME", v),
            None => std::env::remove_var("FLEET_HOME"),
        }
        spawn_result.unwrap();
        let exited_ok = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("child did not exit in time");
        assert!(exited_ok, "child exited nonzero");

        let observed = std::fs::read_to_string(&out).unwrap();
        let fleet_bin = real_home.join(".claude").join("fleet").join("bin");
        let local_bin = real_home.join(".local").join("bin");
        assert!(
            observed.contains(&fleet_bin.display().to_string()),
            "child PATH must include ~/.claude/fleet/bin, got: {observed}"
        );
        assert!(
            observed.contains(&local_bin.display().to_string()),
            "child PATH must include ~/.local/bin, got: {observed}"
        );
        assert!(
            observed.ends_with(MINIMAL_PATH),
            "parent PATH must be preserved at the tail, got: {observed}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn spawn_claude_detached_rejects_missing_workspace() {
        let log = std::env::temp_dir().join(format!(
            "fleet_test_spawn_detached_{}.log",
            std::process::id()
        ));
        let err = super::spawn_claude_detached(
            "/bin/sh",
            &["-c".to_string(), "true".to_string()],
            "/nonexistent/fleet/workspace/dir",
            &log,
            "test",
            "",
            |_| {},
        )
        .unwrap_err();
        assert!(
            err.contains("Workspace directory not found"),
            "unexpected error: {err}"
        );
    }
}
