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
use std::path::{Path, PathBuf};

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
    /// Optional caller-pre-assigned session id (a UUID). When present and valid
    /// it is threaded straight into `--session-id`, so a caller that lost the
    /// reply frame (e.g. the mobile relay over a flaky WS) can still recognise
    /// the spawned session by this exact id in a later `sessions` snapshot —
    /// the JSONL stem the scanner discovers equals this id. `None`/empty mints a
    /// fresh id server-side (the pre-idempotent behaviour).
    #[serde(default)]
    pub session_id: Option<String>,
}

/// `CLAUDE_CODE_ENTRYPOINT` value stamped on sessions launched by the "新会话"
/// button. The CLI writes it verbatim into each `user` record's `entrypoint`
/// field, which is what the history panel filters on.
pub const NEW_SESSION_ENTRYPOINT: &str = "claw-fleet-newsession";

/// True for the sessions Fleet spawned itself, headless (`-p`): the "新会话"
/// button and the handoff relay. Mirrors `isFleetOwnedEntrypoint` in the
/// desktop's `types.ts`.
///
/// Load-bearing for interrupts. SIGINT only means "abort the tool call" for a
/// headless CLI; an interactive one, attached to a pty, reads Ctrl-C as a
/// keystroke, so a real SIGINT makes it quit and abandon its tool child. Only
/// these sessions may be offered an interrupt.
pub fn is_fleet_owned_entrypoint(entrypoint: Option<&str>) -> bool {
    matches!(
        entrypoint,
        Some(e) if e == NEW_SESSION_ENTRYPOINT || e == crate::handoff::HANDOFF_ENTRYPOINT
    )
}

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

/// Response body for the remote `/spawn_session` endpoint (also the return
/// shape of [`spawn_new_session`] and the Tauri command wrapping it).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SpawnSessionResponse {
    pub pid: u32,
    /// Session id pre-assigned via `--session-id`, so the caller can correlate
    /// the spawned process with its transcript without novelty guessing, and
    /// the scanner can pin the pid to exactly this session. `None` only when
    /// the response came from an older `fleet serve` probe.
    #[serde(default)]
    pub session_id: Option<String>,
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

/// Args that turn a headless spawn's stdout into the live-thinking stream:
/// emit the streaming JSON payload (incremental `thinking_delta` events) so it
/// can be teed to a sidecar (see [`crate::live_thinking`]). This only changes
/// stdout, which Fleet otherwise discards; the JSONL transcript the scanner
/// reads is unaffected. `--verbose` is required for stream-json in `--print`
/// mode; `--include-partial-messages` adds the token-level deltas.
pub(crate) fn live_thinking_stream_args() -> Vec<String> {
    vec![
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--include-partial-messages".to_string(),
        // Claude 5-family / Opus 4.8 default `thinking.display` to "omitted",
        // which turns every thinking_delta (and the persisted JSONL thinking
        // block) into empty text. Explicitly request the summarized display so
        // headless sessions stream readable thinking again. Hidden CLI flag
        // (not in --help), present since 2.1.206; models whose summarizer is
        // unavailable (opus-4-8 as of 2026-07-11) still return empty text, and
        // models that never had a display distinction ignore it — the flag is
        // additive-only either way.
        "--thinking-display".to_string(),
        "summarized".to_string(),
    ]
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
        false,
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
    // When true, the child's stdout (the `--output-format stream-json` payload
    // the caller must request via `args`) is teed to a live-thinking sidecar
    // under `~/.fleet/live-thinking/` instead of being discarded. See
    // [`crate::live_thinking`].
    live_thinking: bool,
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

    // Live-thinking sidecar: open a uniquely-named temp file now (PID isn't
    // known until after spawn), point the child's stdout at it, then rename to
    // the pid-keyed name once we have the pid. Renaming an open file is safe on
    // Unix — the child keeps writing to the same inode via its held fd. Best
    // effort: if the sidecar can't be created we fall back to discarding stdout
    // so a filesystem hiccup never blocks session launch.
    let mut sidecar_tmp: Option<PathBuf> = None;
    let stdout_stdio = if live_thinking {
        let opened = crate::live_thinking::ensure_sidecar_dir().ok().and_then(|dir| {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let tmp = dir.join(format!("spawn-{}-{}.jsonl", std::process::id(), nanos));
            std::fs::File::create(&tmp).ok().map(|f| (tmp, f))
        });
        match opened {
            Some((tmp, file)) => {
                sidecar_tmp = Some(tmp);
                std::process::Stdio::from(file)
            }
            None => std::process::Stdio::null(),
        }
    } else {
        std::process::Stdio::null()
    };

    let mut cmd = crate::process_util::command(claude_path);
    cmd.args(args)
        .current_dir(workspace_path)
        .stdin(std::process::Stdio::null())
        .stdout(stdout_stdio)
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
    // Fleet sessions are headless `claude -p`: after the final result the CLI
    // waits for still-running background subagents/workflows, but by default
    // only up to CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS (10 min, CC v2.1.182+) —
    // past the ceiling they are killed mid-flight and their report is lost.
    // Lift the ceiling so long-running background subagents always complete
    // (bg_guard stopped blocking on them for the same reason; see its module
    // docs for the isolation experiment). `extra_envs` below can still override.
    cmd.env("CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS", "0");
    for (k, v) in extra_envs {
        cmd.env(k, v);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn claude failed: {e}"))?;
    let pid = child.id();

    // Now that we have the pid, give the sidecar its stable pid-keyed name.
    // The child's held fd is unaffected by the rename.
    if let Some(tmp) = sidecar_tmp {
        if let Some(final_path) = crate::live_thinking::sidecar_path_for_pid(pid) {
            let _ = std::fs::rename(&tmp, &final_path);
        }
    }

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

/// Validate and append the per-session override flags (`--model`, `--effort`,
/// `--permission-mode`) to `args`; blank/whitespace values are dropped. Shared
/// by the new-session spawn and the history panel's resume so both accept the
/// same overrides.
pub fn push_session_override_args(
    args: &mut Vec<String>,
    model: Option<&str>,
    effort: Option<&str>,
    permission_mode: Option<&str>,
) -> Result<(), String> {
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
    Ok(())
}

/// Start a brand-new headless Claude Code session: spawns
/// `claude -p "<prompt>" --session-id <uuid> [--model <m>] [--effort <e>]`
/// detached in `workspace_path`. Returns as soon as the child is spawned; the
/// session's JSONL will be created by the claude process itself and discovered
/// by the scanner. The pre-assigned `--session-id` keeps the session id in the
/// process argv, which is what lets the scanner attribute the pid to exactly
/// this session (and detect its death) — see `resolve_pid` in session.rs.
pub fn spawn_new_session(
    workspace_path: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    permission_mode: Option<&str>,
) -> Result<SpawnSessionResponse, String> {
    spawn_new_session_with_entrypoint(
        workspace_path,
        prompt,
        model,
        effort,
        permission_mode,
        NEW_SESSION_ENTRYPOINT,
    )
}

/// Resolve the `--session-id` for a new spawn: reuse a caller-provided UUID, or
/// mint a fresh one. A provided id is validated as a real UUID so a malformed or
/// injected value can never reach the process argv; empty/whitespace is treated
/// as absent. This is the seam that makes [`spawn_new_session_with_id`]
/// idempotent from the caller's point of view — the caller picks the id up front
/// and can correlate the running session by it even if the spawn reply is lost.
pub fn resolve_new_session_id(provided: Option<&str>) -> Result<String, String> {
    match provided.map(str::trim) {
        Some(s) if !s.is_empty() => {
            uuid::Uuid::parse_str(s).map_err(|_| format!("invalid session_id: {s}"))?;
            Ok(s.to_string())
        }
        _ => Ok(uuid::Uuid::new_v4().to_string()),
    }
}

/// [`spawn_new_session`] but with a caller-pre-assigned `session_id` (a UUID).
/// Used by the mobile relay's `spawn_session` for end-to-end idempotent
/// confirmation: the phone generates the id, so even when the WS reply frame is
/// dropped it can still confirm success by finding this exact id in a later
/// `sessions` snapshot. `session_id == None`/empty mints one server-side.
pub fn spawn_new_session_with_id(
    workspace_path: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    permission_mode: Option<&str>,
    session_id: Option<&str>,
) -> Result<SpawnSessionResponse, String> {
    spawn_new_session_impl(
        workspace_path,
        prompt,
        model,
        effort,
        permission_mode,
        session_id,
        NEW_SESSION_ENTRYPOINT,
    )
}

/// [`spawn_new_session`] with an explicit `CLAUDE_CODE_ENTRYPOINT` stamp, so
/// programmatic launchers (e.g. the handoff relay) stay distinguishable from
/// the "新会话" button in transcripts and the history panel.
pub fn spawn_new_session_with_entrypoint(
    workspace_path: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    permission_mode: Option<&str>,
    entrypoint: &str,
) -> Result<SpawnSessionResponse, String> {
    spawn_new_session_impl(
        workspace_path,
        prompt,
        model,
        effort,
        permission_mode,
        None,
        entrypoint,
    )
}

fn spawn_new_session_impl(
    workspace_path: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    permission_mode: Option<&str>,
    session_id: Option<&str>,
    entrypoint: &str,
) -> Result<SpawnSessionResponse, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("prompt is required".to_string());
    }
    // Validate overrides (permission mode) before any CLI/filesystem checks so
    // the frontend gets a stable error regardless of the host environment.
    let mut override_args = Vec::new();
    push_session_override_args(&mut override_args, model, effort, permission_mode)?;
    // Resolve the session id before any CLI/filesystem work so a malformed
    // caller-provided id fails fast and identically on every host.
    let session_id = resolve_new_session_id(session_id)?;
    let (found, claude_path) = crate::check_cli_installed();
    if !found {
        return Err("Claude CLI not found on PATH".to_string());
    }
    let claude = claude_path.unwrap_or_else(|| "claude".to_string());
    let stderr_log = crate::session::get_fleet_dir()
        .map(|d| d.join("new_session_stderr.log"))
        .ok_or_else(|| "no fleet dir".to_string())?;
    let mut args = vec![
        "-p".to_string(),
        prompt.to_string(),
        "--session-id".to_string(),
        session_id.clone(),
    ];
    args.extend(live_thinking_stream_args());
    args.extend(override_args);
    args.extend(permission_prompt_tool_args());
    // The pure-chat workspace is Fleet-owned and may not exist yet — create it
    // before the `is_dir` gate below rejects the spawn — and launch it as a chat
    // rather than a coding agent (see `chat_workspace`).
    if crate::chat_workspace::is_chat_workspace(workspace_path) {
        crate::chat_workspace::ensure_chat_workspace()?;
        args.extend(crate::chat_workspace::chat_session_args());
    }
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
        &[("CLAUDE_CODE_ENTRYPOINT", entrypoint)],
        true, // tee stdout to a live-thinking sidecar
        |_success| {},
    )?;
    crate::log_debug(&format!(
        "new_session: spawned pid {} session {} in {}",
        pid, session_id, workspace_path
    ));
    Ok(SpawnSessionResponse {
        pid,
        session_id: Some(session_id),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn live_thinking_stream_args_request_summarized_thinking() {
        // Claude 5-family and Opus 4.8 default `thinking.display` to "omitted",
        // so a headless `-p` spawn gets thinking_delta events (and JSONL
        // thinking blocks) with empty text — the live-thinking sidecar renders
        // nothing. The spawn must explicitly request "summarized" to get
        // content back (verified against CLI 2.1.206 / fable-5, 2026-07-11).
        let args = super::live_thinking_stream_args();
        let idx = args.iter().position(|a| a == "--thinking-display");
        assert!(
            idx.is_some(),
            "headless spawns must pass --thinking-display, got: {args:?}"
        );
        assert_eq!(
            args.get(idx.unwrap() + 1).map(String::as_str),
            Some("summarized")
        );
    }

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

    #[test]
    fn resolve_new_session_id_reuses_valid_uuid() {
        // The caller-provided id (mobile idempotent confirm) must be threaded
        // through verbatim so the phone can match it in a later snapshot.
        let id = "3f8c1e2a-4b5d-6e7f-8a9b-0c1d2e3f4a5b";
        assert_eq!(super::resolve_new_session_id(Some(id)).unwrap(), id);
        // Surrounding whitespace is trimmed, not rejected.
        let padded = format!("  {id}  ");
        assert_eq!(super::resolve_new_session_id(Some(&padded)).unwrap(), id);
    }

    #[test]
    fn resolve_new_session_id_mints_when_absent() {
        // None or empty/whitespace → a fresh, valid UUID (pre-idempotent path).
        for provided in [None, Some(""), Some("   ")] {
            let id = super::resolve_new_session_id(provided).unwrap();
            assert!(
                uuid::Uuid::parse_str(&id).is_ok(),
                "minted id is not a uuid: {id}"
            );
        }
    }

    #[test]
    fn resolve_new_session_id_rejects_malformed() {
        // A non-uuid must never reach the process argv (`--session-id <x>`).
        let err = super::resolve_new_session_id(Some("not-a-uuid; rm -rf /")).unwrap_err();
        assert!(err.contains("invalid session_id"), "unexpected error: {err}");
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

#[cfg(test)]
mod fleet_owned_tests {
    use super::*;

    #[test]
    fn only_fleet_spawned_headless_sessions_are_interruptible() {
        assert!(is_fleet_owned_entrypoint(Some(NEW_SESSION_ENTRYPOINT)));
        assert!(is_fleet_owned_entrypoint(Some(
            crate::handoff::HANDOFF_ENTRYPOINT
        )));

        // Interactive / foreign launches must never be offered an interrupt:
        // SIGINT makes those quit and orphan their tool child.
        assert!(!is_fleet_owned_entrypoint(Some("cli")));
        assert!(!is_fleet_owned_entrypoint(Some("claude-vscode")));
        assert!(!is_fleet_owned_entrypoint(Some("sdk-py")));
        assert!(!is_fleet_owned_entrypoint(None));
    }
}
