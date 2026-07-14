//! Launch a brand-new headless Codex session in a workspace.
//!
//! The Codex analogue of [`crate::session_launch::spawn_new_session`]. Where
//! Claude Code is driven by `claude -p "<prompt>" --session-id <uuid>
//! --output-format stream-json`, Codex is driven by
//! `codex exec --json -C <ws> [-m <model>] [-c model_reasoning_effort=<e>] --
//! "<prompt>"`.
//!
//! Two Codex-specific facts shape this module (both verified against
//! codex-cli 0.144.4, 2026-07-14):
//!
//! 1. **`codex exec` reads stdin.** Left attached to an open stdin it blocks
//!    forever ("Reading additional input from stdin…"). The child's stdin
//!    MUST be redirected to `/dev/null` (`Stdio::null()`), exactly like the
//!    Claude spawn, or the session hangs before it does anything.
//!
//! 2. **Codex mints its own thread id — there is no `--session-id`.** With
//!    `--json`, the *first* line Codex prints to stdout is
//!    `{"type":"thread.started","thread_id":"019f…"}`. Fleet captures the
//!    thread id from that line to correlate the spawned process with the
//!    session the scanner later discovers (the `codex://` rollout keyed by the
//!    same id). This is the analogue of Claude's pre-assigned `--session-id`,
//!    just learned a few milliseconds *after* spawn instead of chosen before.
//!
//! 3. **Resume is `codex exec resume <thread-id>` (verified 2026-07-14,
//!    codex-cli 0.144.4).** `codex exec resume [SESSION_ID] [PROMPT]` takes the
//!    thread id as its first positional and supports `--json`,
//!    `--skip-git-repo-check`, `-m`, `-c` — but **not** `-C` (resume filters by
//!    cwd, and an explicit UUID takes precedence, so the child's `current_dir`
//!    is what scopes it). stdin must still be `/dev/null`, and the resumed run
//!    re-emits the same `thread.started` id on its first stdout line, then the
//!    turn events, ending in `turn.completed` with exit code 0. See
//!    [`build_codex_resume_args`] / [`resume_codex_session`].
//!
//! The transcript itself is NOT read from stdout here — `CodexSource` already
//! reads the on-disk rollout (`~/.codex/sessions/.../rollout-*.jsonl[.zst]`),
//! whose schema differs from the `--json` event stream. After the thread id is
//! captured, the remaining stdout is drained-and-discarded solely to keep the
//! pipe from filling (which would block the child).

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;

use crate::session_launch::{
    augmented_path_with_front, normalize_workspace_path, SpawnSessionResponse,
};

/// How long to wait for Codex to print its `thread.started` line before giving
/// up on id correlation. Codex prints it within a few ms of spawn; a generous
/// ceiling covers a cold binary / slow disk without hanging session launch.
const THREAD_STARTED_TIMEOUT: Duration = Duration::from_secs(30);

/// Codex rollout `originator` value stamped on sessions Fleet launches itself
/// (new spawn + resume), via the `CODEX_INTERNAL_ORIGINATOR_OVERRIDE` env below.
/// This is Codex's analogue of Claude's [`crate::session_launch::NEW_SESSION_ENTRYPOINT`]:
/// the scanner reads `session_meta.payload.originator` back off the rollout and
/// classifies `originator == "fleet"` as Fleet-owned (see
/// [`crate::codex_source`] and [`crate::session_launch::is_fleet_owned_entrypoint`]).
///
/// Verified 2026-07-14: `CODEX_INTERNAL_ORIGINATOR_OVERRIDE=fleet codex exec …`
/// writes `"originator":"fleet"` into the rollout's first `session_meta` line
/// (the SQLite `source` column stays `exec` — only the rollout carries it).
pub const CODEX_FLEET_ORIGINATOR: &str = "fleet";

/// Env value Fleet stamps on Codex children so in-session tooling (e.g.
/// `fleet handoff`) knows its own agent source without scanning. Read back by
/// the CLI as `FLEET_AGENT_SOURCE`; absent → assume Claude (the historical
/// default). Uses Codex's config/`agent_source` name, not the api name.
pub const FLEET_AGENT_SOURCE_CODEX: &str = "codex";

/// Pin `HOME` + augment `PATH` exactly like the Claude spawn, so a
/// launchd-minimal `PATH` doesn't strand the child's tools. Shared by the
/// new-session spawn and the resume launcher. See
/// `session_launch::spawn_claude_detached_with_envs`.
///
/// Also stamps the Fleet-owned markers on every Codex process Fleet starts:
/// `CODEX_INTERNAL_ORIGINATOR_OVERRIDE` writes [`CODEX_FLEET_ORIGINATOR`] into
/// the rollout so the scanner can recognise the session as Fleet-launched, and
/// `FLEET_AGENT_SOURCE` lets in-session tooling identify its own source. Both
/// spawn and resume go through here, so self-spawned and self-resumed Codex
/// sessions are marked identically.
fn apply_codex_launch_env(cmd: &mut std::process::Command) {
    if let Some(home) = crate::session::real_home_dir() {
        cmd.env("HOME", home);
    }
    let front: Vec<std::path::PathBuf> = crate::fleet_cli::fleet_bin_dir().into_iter().collect();
    cmd.env("PATH", augmented_path_with_front(&front));
    cmd.env("CODEX_INTERNAL_ORIGINATOR_OVERRIDE", CODEX_FLEET_ORIGINATOR);
    cmd.env("FLEET_AGENT_SOURCE", FLEET_AGENT_SOURCE_CODEX);
}

/// Build the `codex exec` argv for a headless spawn.
///
/// `workspace_path` is passed to Codex via `-C` (Codex's own working-dir flag)
/// in addition to the child process's `current_dir`; `--skip-git-repo-check`
/// lets a non-git workspace launch (Codex otherwise refuses outside a repo).
/// The prompt is placed after `--` so a prompt starting with `-` is never
/// parsed as a flag.
///
/// Effort maps to Codex's `-c model_reasoning_effort=<value>` config override
/// (Codex has no `--effort` flag). Validation of the value against Codex's
/// accepted set is left to Codex; blank model/effort are dropped.
pub fn build_codex_exec_args(
    workspace_path: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "exec".to_string(),
        "--json".to_string(),
        "--skip-git-repo-check".to_string(),
        "-C".to_string(),
        workspace_path.to_string(),
    ];
    if let Some(m) = model.map(str::trim).filter(|m| !m.is_empty()) {
        args.push("-m".to_string());
        args.push(m.to_string());
    }
    if let Some(e) = effort.map(str::trim).filter(|e| !e.is_empty()) {
        args.push("-c".to_string());
        args.push(format!("model_reasoning_effort={e}"));
    }
    // Everything after `--` is the prompt, verbatim.
    args.push("--".to_string());
    args.push(prompt.to_string());
    args
}

/// Extract the Codex thread id from a single `--json` stdout line, if that line
/// is the `thread.started` event. Returns `None` for any other line.
///
/// Shape: `{"type":"thread.started","thread_id":"019f60cf-…"}`.
pub fn parse_thread_started(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("thread.started") {
        return None;
    }
    v.get("thread_id")
        .and_then(|t| t.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Start a brand-new headless Codex session: spawns
/// `codex exec --json -C <ws> [-m <model>] [-c model_reasoning_effort=<e>] --
/// "<prompt>"` detached in `workspace_path`, captures the Codex-minted thread
/// id from the first stdout line, and returns once that id is known (or the
/// timeout elapses). The child keeps running headless; its rollout is
/// discovered by [`crate::codex_source::CodexSource`].
///
/// Returns the child pid plus the thread id as `session_id` (mirroring
/// [`SpawnSessionResponse`]). `session_id` is `None` only in the degraded case
/// where Codex never printed `thread.started` within [`THREAD_STARTED_TIMEOUT`]
/// — the session still runs and the scanner will find it, just without
/// immediate correlation.
pub fn spawn_new_codex_session(
    workspace_path: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<SpawnSessionResponse, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("prompt is required".to_string());
    }
    let workspace_path = normalize_workspace_path(workspace_path)?;
    if !Path::new(&workspace_path).is_dir() {
        return Err(format!("Workspace directory not found: {workspace_path}"));
    }

    let codex = crate::codex_source::find_codex_binary()
        .ok_or_else(|| "Codex CLI not found (no standalone install, VSCode extension, or `codex` on PATH)".to_string())?;

    let stderr_log = crate::session::get_fleet_dir()
        .map(|d| d.join("codex_new_session_stderr.log"))
        .ok_or_else(|| "no fleet dir".to_string())?;
    if let Some(parent) = stderr_log.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create stderr log dir {}: {e}", parent.display()))?;
    }

    let args = build_codex_exec_args(&workspace_path, prompt, model, effort);

    crate::log_debug(&format!(
        "new_codex_session: {} exec … (cwd={}, model={:?}, effort={:?})",
        codex.display(),
        workspace_path,
        model,
        effort
    ));

    {
        let mut header = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stderr_log)
            .map_err(|e| format!("open stderr log {}: {e}", stderr_log.display()))?;
        let _ = writeln!(
            header,
            "[{}] new_codex_session spawn cwd={}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            workspace_path
        );
    }
    let stderr_file = std::fs::OpenOptions::new()
        .append(true)
        .open(&stderr_log)
        .map_err(|e| format!("reopen stderr log {}: {e}", stderr_log.display()))?;

    let mut cmd = crate::process_util::command(&codex);
    cmd.args(&args)
        .current_dir(&workspace_path)
        // MUST be null: `codex exec` otherwise blocks reading stdin forever.
        .stdin(std::process::Stdio::null())
        // Piped so we can read the `thread.started` line; drained afterward.
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::from(stderr_file));
    apply_codex_launch_env(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn codex failed: {e}"))?;
    let pid = child.id();

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "codex child stdout unavailable".to_string())?;

    // Reader/reaper thread: read stdout, send the thread id over the channel as
    // soon as `thread.started` arrives, then keep draining to EOF (so the pipe
    // never fills and blocks the child) and reap the child.
    let (tx, rx) = mpsc::channel::<String>();
    let stderr_log_owned = stderr_log.clone();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        let mut tx = Some(tx);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if let Some(sender) = tx.as_ref() {
                if let Some(thread_id) = parse_thread_started(&line) {
                    let _ = sender.send(thread_id);
                    tx = None; // fire once
                }
            }
            // Any remaining lines are drained-and-discarded on purpose.
        }
        // stdout closed → reap the child and log its exit.
        let result = child.wait();
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stderr_log_owned)
        {
            match result {
                Ok(status) => {
                    let _ = writeln!(
                        f,
                        "[{}] new_codex_session exit code={:?} success={}",
                        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                        status.code(),
                        status.success()
                    );
                }
                Err(e) => {
                    let _ = writeln!(
                        f,
                        "[{}] new_codex_session wait_err err={e}",
                        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f")
                    );
                }
            }
        }
    });

    let session_id = match rx.recv_timeout(THREAD_STARTED_TIMEOUT) {
        Ok(thread_id) => {
            // Record the launch flags against the Codex thread id, so relaunch
            // paths (resume / handoff) inherit the same model/effort — the same
            // note Claude spawns write, keyed by session id.
            crate::launch_spec::record(&thread_id, model, effort);
            Some(thread_id)
        }
        Err(_) => {
            crate::log_debug(
                "new_codex_session: no thread.started within timeout; \
                 returning pid without session_id (scanner will still find it)",
            );
            None
        }
    };

    Ok(SpawnSessionResponse { pid, session_id })
}

/// Build the `codex exec resume` argv for headlessly resuming a thread.
///
/// Shape (verified against codex-cli 0.144.4):
/// `codex exec resume <thread-id> --json --skip-git-repo-check
/// [-m <model>] [-c model_reasoning_effort=<e>] -- <prompt>`.
///
/// The thread id is `resume`'s first positional; the prompt goes after `--`
/// (so a prompt starting with `-` is never parsed as a flag). Unlike the spawn
/// builder there is **no `-C`** — `codex exec resume` has no working-dir flag;
/// it scopes by the child's cwd, and the explicit thread id takes precedence
/// over cwd filtering anyway. Blank model/effort are dropped.
pub fn build_codex_resume_args(
    session_id: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "exec".to_string(),
        "resume".to_string(),
        session_id.to_string(),
        "--json".to_string(),
        "--skip-git-repo-check".to_string(),
    ];
    if let Some(m) = model.map(str::trim).filter(|m| !m.is_empty()) {
        args.push("-m".to_string());
        args.push(m.to_string());
    }
    if let Some(e) = effort.map(str::trim).filter(|e| !e.is_empty()) {
        args.push("-c".to_string());
        args.push(format!("model_reasoning_effort={e}"));
    }
    args.push("--".to_string());
    args.push(prompt.to_string());
    args
}

/// Headlessly resume an existing Codex thread: spawns
/// `codex exec resume <thread-id> --json … -- "<prompt>"` detached in
/// `workspace_path`, then returns once the child is launched. The Codex
/// analogue of [`crate::auto_resume::spawn_resume_tracked_prompt`].
///
/// `on_exit(success)` is invoked from the reaper thread when the resume process
/// exits, so the auto-resume scheduler can free its concurrency slot and record
/// backoff. Blank `prompt` falls back to "continue" (matching the Claude
/// resume). stdin is `/dev/null` (Codex otherwise blocks reading stdin); the
/// resumed process's stdout is drained-and-discarded (the on-disk rollout is
/// what the scanner reads).
pub fn resume_codex_session(
    session_id: &str,
    workspace_path: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    on_exit: Box<dyn FnOnce(bool) + Send>,
) -> Result<(), String> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err("session_id is required".to_string());
    }
    let prompt = prompt.trim();
    let prompt = if prompt.is_empty() { "continue" } else { prompt };
    let workspace_path = normalize_workspace_path(workspace_path)?;
    if !Path::new(&workspace_path).is_dir() {
        return Err(format!("Workspace directory not found: {workspace_path}"));
    }

    let codex = crate::codex_source::find_codex_binary()
        .ok_or_else(|| "Codex CLI not found (no standalone install, VSCode extension, or `codex` on PATH)".to_string())?;

    let stderr_log = crate::session::get_fleet_dir()
        .map(|d| d.join("codex_resume_stderr.log"))
        .ok_or_else(|| "no fleet dir".to_string())?;
    if let Some(parent) = stderr_log.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create stderr log dir {}: {e}", parent.display()))?;
    }

    let args = build_codex_resume_args(session_id, prompt, model, effort);

    // A resume may carry its own `--model` / `--effort`; record them so the
    // launch note describes what the session is running *now* (same as the
    // Claude resume path). No overrides → leaves the original note standing.
    crate::launch_spec::record(session_id, model, effort);

    crate::log_debug(&format!(
        "resume_codex_session: {} exec resume {} (cwd={}, prompt=<{} chars>, model={:?}, effort={:?})",
        codex.display(),
        session_id,
        workspace_path,
        prompt.len(),
        model,
        effort
    ));

    {
        let mut header = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stderr_log)
            .map_err(|e| format!("open stderr log {}: {e}", stderr_log.display()))?;
        let _ = writeln!(
            header,
            "[{}] codex_resume spawn session={} cwd={}",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            session_id,
            workspace_path
        );
    }
    let stderr_file = std::fs::OpenOptions::new()
        .append(true)
        .open(&stderr_log)
        .map_err(|e| format!("reopen stderr log {}: {e}", stderr_log.display()))?;

    let mut cmd = crate::process_util::command(&codex);
    cmd.args(&args)
        .current_dir(&workspace_path)
        // MUST be null: `codex exec` otherwise blocks reading stdin forever.
        .stdin(std::process::Stdio::null())
        // Piped so the reader thread can drain it; the transcript is read from
        // the on-disk rollout, not this stream.
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::from(stderr_file));
    apply_codex_launch_env(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn codex resume failed: {e}"))?;
    let pid = child.id();

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "codex resume child stdout unavailable".to_string())?;

    // Reader/reaper thread: drain stdout to EOF (so the pipe never fills and
    // blocks the child), reap the child, log its exit, and invoke `on_exit`.
    let stderr_log_owned = stderr_log.clone();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            if line.is_err() {
                break;
            }
            // Drained-and-discarded on purpose (see module doc).
        }
        let result = child.wait();
        let success = matches!(&result, Ok(status) if status.success());
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stderr_log_owned)
        {
            match result {
                Ok(status) => {
                    let _ = writeln!(
                        f,
                        "[{}] codex_resume exit code={:?} success={}",
                        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                        status.code(),
                        status.success()
                    );
                }
                Err(e) => {
                    let _ = writeln!(
                        f,
                        "[{}] codex_resume wait_err err={e}",
                        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f")
                    );
                }
            }
        }
        on_exit(success);
    });

    crate::log_debug(&format!(
        "resume_codex_session: spawned pid {} for thread {}",
        pid, session_id
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arg_builder_minimal_has_exec_json_cwd_and_prompt_after_dashdash() {
        let args = build_codex_exec_args("/ws", "do the thing", None, None);
        assert_eq!(args[0], "exec");
        assert!(args.contains(&"--json".to_string()));
        // -C <ws>
        let ci = args.iter().position(|a| a == "-C").expect("has -C");
        assert_eq!(args[ci + 1], "/ws");
        // prompt is the final arg, preceded by `--`
        assert_eq!(args.last().unwrap(), "do the thing");
        let dd = args.iter().position(|a| a == "--").expect("has --");
        assert_eq!(dd, args.len() - 2, "-- immediately precedes the prompt");
        // no model/effort flags when not given
        assert!(!args.contains(&"-m".to_string()));
    }

    #[test]
    fn arg_builder_maps_model_and_effort() {
        let args = build_codex_exec_args("/ws", "hi", Some("gpt-5.6-sol"), Some("high"));
        let mi = args.iter().position(|a| a == "-m").expect("has -m");
        assert_eq!(args[mi + 1], "gpt-5.6-sol");
        // effort → `-c model_reasoning_effort=high`
        let ci = args
            .iter()
            .position(|a| a == "model_reasoning_effort=high")
            .expect("has effort config");
        assert_eq!(args[ci - 1], "-c");
    }

    #[test]
    fn arg_builder_drops_blank_model_and_effort() {
        let args = build_codex_exec_args("/ws", "hi", Some("  "), Some(""));
        assert!(!args.contains(&"-m".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("model_reasoning_effort=")));
    }

    #[test]
    fn parses_thread_id_from_thread_started_line() {
        let line = r#"{"type":"thread.started","thread_id":"019f60cf-af8a-7b30-b43a-5ad17d5bb0f2"}"#;
        assert_eq!(
            parse_thread_started(line).as_deref(),
            Some("019f60cf-af8a-7b30-b43a-5ad17d5bb0f2")
        );
    }

    /// Live smoke: actually spawn a real Codex session and confirm the launcher
    /// captures the minted thread id end-to-end (reader thread + channel +
    /// `thread.started` parse + launch-spec record). Ignored by default — it
    /// invokes the real `codex` binary and the model, so it needs a working
    /// Codex install + auth. Run explicitly:
    ///   `cargo test -p claw-fleet-core codex_launch::tests::live -- --ignored`
    #[test]
    #[ignore = "spawns a real codex session; run manually with --ignored"]
    fn live_spawn_returns_a_thread_id() {
        let ws = std::env::temp_dir().join(format!("fleet-codex-live-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        let resp = spawn_new_codex_session(
            ws.to_str().unwrap(),
            "reply with exactly: OK",
            None,
            None,
        )
        .expect("spawn should succeed");
        assert!(resp.pid > 0, "got a pid");
        let sid = resp.session_id.expect("thread id captured from stdout");
        assert!(
            sid.len() >= 8 && sid.contains('-'),
            "thread id looks like a uuid: {sid}"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// Live end-to-end (M1 acceptance): route a spawn through the tool
    /// dispatcher with tool="codex", then confirm the scanner surfaces the
    /// just-spawned session as a `codex` source with the captured thread id.
    /// Exercises P1 (launcher) + P2 (AgentSource::spawn dispatch) + the codex
    /// scan integration. Ignored — invokes the real codex binary + model.
    ///   `cargo test -p claw-fleet-core codex_launch::tests::live_dispatch -- --ignored`
    #[test]
    #[ignore = "spawns a real codex session; run manually with --ignored"]
    fn live_dispatch_spawn_appears_as_codex_in_scan() {
        use crate::agent_source::{AgentSource, SpawnSpec};

        let ws = std::env::temp_dir().join(format!("fleet-codex-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();

        let spec = SpawnSpec {
            workspace_path: ws.to_string_lossy().into_owned(),
            prompt: "reply with exactly: OK".to_string(),
            ..Default::default()
        };
        let resp = crate::agent_source::spawn_session("codex", &spec)
            .expect("dispatcher should route codex spawn");
        let sid = resp.session_id.expect("codex thread id captured");

        // Poll the scanner: the rollout + sqlite row land within a moment of
        // thread.started. Give it a few tries rather than a fixed sleep.
        let source = crate::codex_source::CodexSource::new();
        let mut found = None;
        for _ in 0..40 {
            if let Some(s) = source
                .scan_sessions()
                .into_iter()
                .find(|s| s.id == sid)
            {
                found = Some(s);
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let session = found.unwrap_or_else(|| panic!("session {sid} not found in codex scan"));
        assert_eq!(session.agent_source, "codex", "scanned as codex source");
        assert_eq!(session.id, sid, "thread id correlates");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn resume_arg_builder_minimal_has_exec_resume_id_and_prompt_after_dashdash() {
        let args = build_codex_resume_args("019f-abc", "keep going", None, None);
        assert_eq!(args[0], "exec");
        assert_eq!(args[1], "resume");
        assert_eq!(args[2], "019f-abc", "thread id is resume's first positional");
        assert!(args.contains(&"--json".to_string()));
        assert!(args.contains(&"--skip-git-repo-check".to_string()));
        // prompt is the final arg, preceded by `--`
        assert_eq!(args.last().unwrap(), "keep going");
        let dd = args.iter().position(|a| a == "--").expect("has --");
        assert_eq!(dd, args.len() - 2, "-- immediately precedes the prompt");
        // resume has no -C working-dir flag.
        assert!(!args.contains(&"-C".to_string()));
        assert!(!args.contains(&"-m".to_string()));
    }

    #[test]
    fn resume_arg_builder_maps_model_and_effort() {
        let args = build_codex_resume_args("tid", "hi", Some("gpt-5.6-sol"), Some("high"));
        let mi = args.iter().position(|a| a == "-m").expect("has -m");
        assert_eq!(args[mi + 1], "gpt-5.6-sol");
        let ci = args
            .iter()
            .position(|a| a == "model_reasoning_effort=high")
            .expect("has effort config");
        assert_eq!(args[ci - 1], "-c");
    }

    #[test]
    fn resume_arg_builder_drops_blank_model_and_effort() {
        let args = build_codex_resume_args("tid", "hi", Some(" "), Some(""));
        assert!(!args.contains(&"-m".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("model_reasoning_effort=")));
    }

    /// Live smoke (M2 P6 acceptance): spawn a real Codex session, then resume it
    /// with a follow-up prompt and confirm the resume process exits successfully
    /// (exit code 0 → `on_exit(true)`). Exercises the resume argv + stdin=null +
    /// drain/reap/on_exit path against the real binary. Ignored by default.
    ///   `cargo test -p claw-fleet-core codex_launch::tests::live_resume -- --ignored`
    #[test]
    #[ignore = "spawns + resumes a real codex session; run manually with --ignored"]
    fn live_resume_continues_and_exits_ok() {
        let ws = std::env::temp_dir().join(format!("fleet-codex-resume-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        let resp = spawn_new_codex_session(
            ws.to_str().unwrap(),
            "reply with exactly: FIRST",
            None,
            None,
        )
        .expect("initial spawn should succeed");
        let tid = resp.session_id.expect("thread id captured");

        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ok = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done_cb = done.clone();
        let ok_cb = ok.clone();
        resume_codex_session(
            &tid,
            ws.to_str().unwrap(),
            "reply with exactly: SECOND",
            None,
            None,
            Box::new(move |success| {
                ok_cb.store(success, std::sync::atomic::Ordering::SeqCst);
                done_cb.store(true, std::sync::atomic::Ordering::SeqCst);
            }),
        )
        .expect("resume should spawn");

        // Wait up to 90s for the resumed turn to complete + reaper to fire.
        let mut waited = Duration::ZERO;
        while waited < Duration::from_secs(90) && !done.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(500));
            waited += Duration::from_millis(500);
        }
        assert!(done.load(std::sync::atomic::Ordering::SeqCst), "resume on_exit never fired");
        assert!(ok.load(std::sync::atomic::Ordering::SeqCst), "resume exited non-zero");
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn ignores_non_thread_started_lines() {
        assert_eq!(parse_thread_started(r#"{"type":"turn.started"}"#), None);
        assert_eq!(
            parse_thread_started(r#"{"type":"item.completed","item":{"id":"item_0"}}"#),
            None
        );
        assert_eq!(parse_thread_started("not json"), None);
        // thread.started without a usable id → None
        assert_eq!(parse_thread_started(r#"{"type":"thread.started","thread_id":""}"#), None);
    }
}
