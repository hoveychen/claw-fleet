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

/// Env var Fleet stamps on a **new** Codex spawn carrying a launch token.
///
/// Codex mints its own thread id *after* spawn and — unlike Claude's
/// `CLAUDE_CODE_SESSION_ID` — exposes no session-id env to its shell tools, so
/// a Codex child has no way to learn its own id. Fleet can't inject the thread
/// id at spawn (it isn't known yet), so instead it injects a token it *does*
/// know up front, and writes a `token → thread-id` note the moment
/// `thread.started` arrives. In-session tooling (`fleet handoff`, `fleet plan`)
/// resolves the token back to the thread id via [`resolve_launch_token`], the
/// third fallback in the CLI's `read_fleet_session_id`. The resume path needs
/// none of this — it already knows the thread id and stamps `FLEET_SESSION_ID`
/// directly.
pub const FLEET_CODEX_LAUNCH_TOKEN_ENV: &str = "FLEET_CODEX_LAUNCH_TOKEN";

/// Directory holding the `token → thread-id` notes (`<token>` file whose
/// contents are the Codex thread id). Lives under the Fleet dir alongside
/// `launch-spec`.
fn launch_token_dir() -> Option<std::path::PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("codex-launch-tokens"))
}

fn launch_token_path(token: &str) -> Option<std::path::PathBuf> {
    // Tokens are Fleet-minted uuids; reject anything that could escape the dir.
    if token.is_empty()
        || token.contains('/')
        || token.contains('\\')
        || token.contains("..")
    {
        return None;
    }
    launch_token_dir().map(|d| d.join(token))
}

/// Record that the Codex spawn tagged with `token` produced thread `thread_id`,
/// so the child can resolve its own session id from the token env.
pub fn record_launch_token(token: &str, thread_id: &str) {
    let Some(path) = launch_token_path(token) else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            crate::log_debug(&format!("codex_launch: create token dir: {e}"));
            return;
        }
    }
    if let Err(e) = std::fs::write(&path, thread_id) {
        crate::log_debug(&format!("codex_launch: write token {token}: {e}"));
    }
}

/// Resolve a Codex launch token to the thread id Fleet recorded for it, or
/// `None` if the token is unknown (thread.started hasn't landed yet, or this is
/// not a Fleet spawn).
pub fn resolve_launch_token(token: &str) -> Option<String> {
    let path = launch_token_path(token)?;
    let id = std::fs::read_to_string(&path).ok()?.trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// Pure precedence between the two env-carried session ids: `FLEET_SESSION_ID`
/// (Fleet-stamped) wins over `CLAUDE_CODE_SESSION_ID` (Claude Code's own), and
/// an empty value never counts. Split out so it can be tested without touching
/// the real environment. See [`resolve_fleet_session_id_from_env`].
fn fleet_or_claude_session_id(fleet: Option<String>, claude: Option<String>) -> Option<String> {
    fleet
        .filter(|s| !s.is_empty())
        .or_else(|| claude.filter(|s| !s.is_empty()))
}

/// Resolve the current process's Fleet session id from the environment, across
/// agent sources. This is the single precedence shared by the `fleet mcp`
/// server ([`crate::mcp_server`]) and the `fleet` CLI (`read_fleet_session_id`),
/// so a decision card or a `fleet plan` call attributes to the right session
/// whether it came from Claude or Codex:
///   1. `FLEET_SESSION_ID`        — Fleet-stamped (Codex resume + explicit)
///   2. `CLAUDE_CODE_SESSION_ID`  — Claude Code exposes this to MCP/hooks
///   3. `FLEET_CODEX_LAUNCH_TOKEN` → thread id via [`resolve_launch_token`]
///      (a new Codex spawn, whose thread id isn't minted until after launch, so
///      Fleet injects a token up front and writes the token→id note later)
///
/// Returns `None` when none resolve to a non-empty id.
pub fn resolve_fleet_session_id_from_env() -> Option<String> {
    if let Some(id) = fleet_or_claude_session_id(
        std::env::var("FLEET_SESSION_ID").ok(),
        std::env::var("CLAUDE_CODE_SESSION_ID").ok(),
    ) {
        return Some(id);
    }
    std::env::var(FLEET_CODEX_LAUNCH_TOKEN_ENV)
        .ok()
        .filter(|t| !t.is_empty())
        .and_then(|t| resolve_launch_token(&t))
}

// ── Spawn-pid notes (new-session liveness) ───────────────────────────────────
//
// A freshly spawned `codex exec` does NOT carry its thread id in argv — Codex
// mints the id after launch — so a new session mid-first-turn is invisible to
// the argv-based liveness check (`codex_source::codex_proc_alive`). Left
// unpatched, the enqueue-drain gate reads a still-running new session as idle
// and fires a *second* `codex exec resume` on it, corrupting the transcript
// (the exact hazard `pending_message` exists to avoid).
//
// So at spawn time — the one moment Fleet holds both the freshly-minted thread
// id and the child pid — we drop a `thread-id → pid` note. Liveness then means
// "that pid is still a live Codex process" (checked against the scanned live
// set, so a recycled pid belonging to some other process never reads as alive).
// The note is deleted when the child exits, so a finished session is drainable.

/// Directory holding `thread-id → spawn-pid` notes (`<thread-id>` file whose
/// contents are the decimal pid). Lives under the Fleet dir alongside
/// `codex-launch-tokens`.
fn spawn_pid_dir() -> Option<std::path::PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("codex-spawn-pids"))
}

fn spawn_pid_path(thread_id: &str) -> Option<std::path::PathBuf> {
    // Thread ids are Codex-minted uuids; reject anything that could escape the dir.
    if thread_id.is_empty()
        || thread_id.contains('/')
        || thread_id.contains('\\')
        || thread_id.contains("..")
    {
        return None;
    }
    spawn_pid_dir().map(|d| d.join(thread_id))
}

/// Record that Fleet spawned Codex thread `thread_id` as OS process `pid`, so the
/// new-session liveness check can recognise it while its thread id is not yet in
/// any argv.
pub fn record_spawn_pid(thread_id: &str, pid: u32) {
    let Some(path) = spawn_pid_path(thread_id) else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            crate::log_debug(&format!("codex_launch: create spawn-pid dir: {e}"));
            return;
        }
    }
    if let Err(e) = std::fs::write(&path, pid.to_string()) {
        crate::log_debug(&format!("codex_launch: write spawn-pid {thread_id}: {e}"));
    }
}

/// The pid Fleet recorded for a spawned Codex thread, or `None` if unknown (not a
/// Fleet spawn, or the session has already exited and its note was cleared).
pub fn resolve_spawn_pid(thread_id: &str) -> Option<u32> {
    let path = spawn_pid_path(thread_id)?;
    std::fs::read_to_string(&path).ok()?.trim().parse().ok()
}

/// Drop a spawned Codex thread's pid note (called when the child exits).
/// Idempotent — a missing note is success.
pub fn clear_spawn_pid(thread_id: &str) {
    let Some(path) = spawn_pid_path(thread_id) else {
        return;
    };
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => crate::log_debug(&format!("codex_launch: clear spawn-pid {thread_id}: {e}")),
    }
}

/// Codex's analogue of the Claude `Stop` hook (`fleet session idle`).
///
/// Claude Code fires `fleet session idle` from its `Stop` hook when a turn ends;
/// that entrypoint marks the session idle, consumes any pending `fleet handoff`
/// relay, and re-arms stranded loop timers (see
/// `fleet-cli/commands/session::cmd_session_idle`). Codex has no hook system and
/// reads none of `~/.claude/settings.json`, so a Codex turn ending would
/// otherwise trigger none of this — a `fleet handoff` registered by a Codex
/// session would write its pending file and never fire.
///
/// A headless `codex exec` turn ends when the process exits, which is exactly
/// when the spawn/resume reaper thread reaps the child. That reaper is Fleet's
/// only in-process signal of a Codex turn boundary, so it calls this to mirror
/// the Claude Stop hook. Every step is idempotent and errors are swallowed to a
/// log (a relay problem must never take the reaper down), matching
/// `cmd_session_idle`.
///
/// One asymmetry: Claude clears the idle sentinel from its `UserPromptSubmit`
/// hook (`fleet session resume`); Codex has no such hook, so a Codex idle
/// sentinel is never cleared. That is harmless today — nothing reads the idle
/// sentinel for card state (the supervisor that used to consume it was removed;
/// see [`crate::idle`]) — and marking idle keeps parity for any future reader.
/// Extract the codex thread id from a `notify` payload iff it is the
/// `agent-turn-complete` event. Returns `None` for any other event type or
/// malformed input.
///
/// Codex invokes the program configured in `notify = [...]` once per turn,
/// appending a single JSON argument. Verified against codex-cli 0.144.4
/// (headless `codex exec` AND `codex exec resume`, 2026-07-16):
/// ```json
/// {"type":"agent-turn-complete","thread-id":"019f…","turn-id":"019f…",
///  "cwd":"…","client":"codex_exec","input-messages":[…],
///  "last-assistant-message":"…"}
/// ```
/// Note the keys are hyphenated (`thread-id`, not `thread_id`). The thread id
/// is codex's session id — the key Fleet relays a handoff on.
pub fn parse_agent_turn_complete_thread_id(payload: &str) -> Option<String> {
    let v: Value = serde_json::from_str(payload.trim()).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("agent-turn-complete") {
        return None;
    }
    v.get("thread-id")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub fn on_codex_turn_exit(session_id: &str) {
    if session_id.is_empty() {
        return;
    }
    if let Err(e) = crate::idle::mark_idle(session_id) {
        crate::log_debug(&format!("codex turn exit: mark_idle {session_id}: {e}"));
    }
    match crate::handoff::consume_and_spawn(session_id) {
        Ok(Some(to)) => crate::log_debug(&format!(
            "codex turn exit: handoff relayed {session_id} -> {to}"
        )),
        Ok(None) => {}
        Err(e) => crate::log_debug(&format!(
            "codex turn exit: handoff relay failed for {session_id}: {e}"
        )),
    }
    // Re-arm any loop timer stranded by a reboot/kill — cheap, idempotent,
    // duplicate-safe via the loop generation. Piggy-backs on the turn boundary
    // exactly like the Claude Stop hook does.
    let rearmed = crate::agent_loop::reconcile();
    if !rearmed.is_empty() {
        crate::log_debug(&format!(
            "codex turn exit: re-armed {} stranded loop timer(s)",
            rearmed.len()
        ));
    }
}

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

/// Quote a value as a TOML basic string for a Codex `-c key=<value>` override.
///
/// Codex parses the `value` of a `-c` override as TOML, falling back to a raw
/// literal only when the parse fails. An unquoted path usually survives as a
/// literal, but a value containing spaces / brackets / quotes would be
/// mis-parsed — so we always emit a properly-escaped TOML basic string, which
/// parses deterministically regardless of the content.
fn toml_basic_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Args that bridge a Fleet-spawned Codex session to Fleet's Decision Panel
/// (`fleet__ask` / `fleet__render_a2ui` cards), appended before the `--` prompt
/// separator by the exec/resume arg builders.
///
/// Two pieces, both verified against codex-cli 0.144.4:
///
/// 1. **`--dangerously-bypass-approvals-and-sandbox`.** In headless `codex exec`
///    (stdin = /dev/null, no interactive approval channel), Codex *auto-cancels*
///    every MCP `tools/call` — the call surfaces as
///    `error: "user cancelled MCP tool call"` and never reaches the server. Only
///    with this bypass does the tool call go through, block awaiting the user's
///    Decision-Panel answer, and relay it back. The bypass ALSO disables Codex's
///    shell-command sandbox (Codex flags it EXTREMELY DANGEROUS); that trade-off
///    is accepted deliberately for Fleet-launched headless sessions, which have
///    no interactive approver anyway.
///
/// 2. **`-c mcp_servers.fleet.*` overrides.** Register the `fleet mcp` stdio
///    server per-invocation (no global `~/.codex/config.toml` mutation — the
///    Claude analogue mutates `~/.claude.json`, but Codex accepts inline `-c`
///    overrides so we keep the user's config untouched). `session_env` is passed
///    as the MCP server's own `env` so `fleet mcp` can attribute the card to
///    this session and resolve its workspace: a new spawn passes
///    `FLEET_CODEX_LAUNCH_TOKEN` (the thread id isn't minted yet) + the
///    workspace as `CLAUDE_PROJECT_DIR`; a resume passes `FLEET_SESSION_ID`
///    directly (see [`crate::mcp_server`] for the resolution).
///
/// Returns empty when no `fleet` binary can be resolved to point Codex at — the
/// session still runs, just without the Decision-Panel bridge.
pub fn fleet_decision_card_args(session_env: &[(String, String)]) -> Vec<String> {
    let Some(fleet) = crate::fleet_cli::resolve_fleet_binary() else {
        crate::log_debug(
            "fleet_decision_card_args: no fleet binary resolved; skipping Decision-Panel bridge",
        );
        return Vec::new();
    };
    let fleet = fleet.to_string_lossy().into_owned();
    let mut args = vec![
        "--dangerously-bypass-approvals-and-sandbox".to_string(),
        "-c".to_string(),
        format!("mcp_servers.fleet.command={}", toml_basic_string(&fleet)),
        "-c".to_string(),
        format!("mcp_servers.fleet.args=[{}]", toml_basic_string("mcp")),
    ];
    for (k, v) in session_env {
        args.push("-c".to_string());
        args.push(format!("mcp_servers.fleet.env.{}={}", k, toml_basic_string(v)));
    }
    args
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
///
/// `pre_prompt_args` (e.g. [`fleet_decision_card_args`]) are inserted verbatim
/// **before** the `--` separator, so they are parsed as flags and never as part
/// of the prompt. Pass an empty slice for none.
pub fn build_codex_exec_args(
    workspace_path: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    pre_prompt_args: &[String],
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
    // Flags (bypass + MCP overrides) must precede `--`; everything after `--`
    // is the prompt, verbatim.
    args.extend(pre_prompt_args.iter().cloned());
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

    // Token the child reads back (via `FLEET_CODEX_LAUNCH_TOKEN`) to learn its
    // own thread id once `thread.started` lands and we write the note below.
    let launch_token = uuid::Uuid::new_v4().to_string();

    // Bridge this session to Fleet's Decision Panel: the thread id isn't minted
    // yet, so the `fleet mcp` server is handed the launch token (resolved back to
    // the thread id by `mcp_server`) plus the workspace as `CLAUDE_PROJECT_DIR`.
    let decision_args = fleet_decision_card_args(&[
        (
            FLEET_CODEX_LAUNCH_TOKEN_ENV.to_string(),
            launch_token.clone(),
        ),
        ("CLAUDE_PROJECT_DIR".to_string(), workspace_path.clone()),
    ]);
    let args = build_codex_exec_args(&workspace_path, prompt, model, effort, &decision_args);

    crate::log_debug(&format!(
        "new_codex_session: {} exec … (cwd={}, model={:?}, effort={:?}, token={})",
        codex.display(),
        workspace_path,
        model,
        effort,
        launch_token,
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
    cmd.env(FLEET_CODEX_LAUNCH_TOKEN_ENV, &launch_token);

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
        // Thread id captured from `thread.started`, kept so we can drop its
        // spawn-pid note once the child exits below.
        let mut spawned_thread: Option<String> = None;
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if let Some(sender) = tx.as_ref() {
                if let Some(thread_id) = parse_thread_started(&line) {
                    // Note the spawn pid so new-session liveness can recognise
                    // this still-running session before its id lands in any argv.
                    record_spawn_pid(&thread_id, pid);
                    spawned_thread = Some(thread_id.clone());
                    let _ = sender.send(thread_id);
                    tx = None; // fire once
                }
            }
            // Any remaining lines are drained-and-discarded on purpose.
        }
        // stdout closed → reap the child and log its exit.
        let result = child.wait();
        // Session is gone: drop the spawn-pid note so a later pid reuse can never
        // read this dead session as alive.
        if let Some(tid) = spawned_thread.take() {
            clear_spawn_pid(&tid);
            // Turn ended (process exited) — mirror the Claude Stop hook so a
            // `fleet handoff` this session registered actually relays. Codex has
            // no hook of its own; this reaper is the only turn-boundary signal.
            on_codex_turn_exit(&tid);
        }
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
            // Resolve the launch token to this thread id so the child's
            // `fleet handoff` / `fleet plan` can learn its own session id.
            record_launch_token(&launch_token, &thread_id);
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
///
/// `pre_prompt_args` (e.g. [`fleet_decision_card_args`]) are inserted before the
/// `--` separator, same as the spawn builder. Pass an empty slice for none.
pub fn build_codex_resume_args(
    session_id: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    pre_prompt_args: &[String],
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
    args.extend(pre_prompt_args.iter().cloned());
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

    // Bridge to Fleet's Decision Panel: a resume already knows its thread id, so
    // hand `fleet mcp` the session id directly (no launch-token indirection) plus
    // the workspace as `CLAUDE_PROJECT_DIR`.
    let decision_args = fleet_decision_card_args(&[
        ("FLEET_SESSION_ID".to_string(), session_id.to_string()),
        ("CLAUDE_PROJECT_DIR".to_string(), workspace_path.clone()),
    ]);
    let args = build_codex_resume_args(session_id, prompt, model, effort, &decision_args);

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
    // Resume knows the thread id up front (it *is* the one being resumed), so
    // stamp it directly — no launch-token indirection needed. The child's
    // `read_fleet_session_id` reads this as its session id.
    cmd.env("FLEET_SESSION_ID", session_id);

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
    // Owned copy for the reaper: the turn-exit hook needs the thread id after the
    // borrowed `session_id` is out of scope.
    let sid_owned = session_id.to_string();
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
        // Turn ended — mirror the Claude Stop hook (mark idle + fire any pending
        // `fleet handoff` relay). Codex has no hook of its own; this reaper is the
        // only turn-boundary signal. Runs before `on_exit` so a relayed successor
        // is spawned before the auto-resume scheduler frees this slot.
        on_codex_turn_exit(&sid_owned);
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

    /// Pin `FLEET_HOME` to a temp dir so the token store writes there, not the
    /// real `~/.fleet`. Mirrors `launch_spec`'s test harness.
    struct TmpHome {
        dir: std::path::PathBuf,
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl TmpHome {
        fn new(tag: &str) -> Self {
            let lock = crate::session::fleet_home_lock();
            let dir = std::env::temp_dir().join(format!(
                "fleet-codextok-{}-{}-{}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let prev = std::env::var_os("FLEET_HOME");
            unsafe { std::env::set_var("FLEET_HOME", &dir) };
            Self { dir, prev, _lock: lock }
        }
    }

    impl Drop for TmpHome {
        fn drop(&mut self) {
            unsafe {
                match self.prev.take() {
                    Some(p) => std::env::set_var("FLEET_HOME", p),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn launch_token_roundtrip_resolves_thread_id() {
        let _home = TmpHome::new("token-rt");
        // Unknown token before any spawn recorded it.
        assert_eq!(resolve_launch_token("tok-1"), None);
        record_launch_token("tok-1", "019f-thread-abc");
        assert_eq!(
            resolve_launch_token("tok-1").as_deref(),
            Some("019f-thread-abc")
        );
    }

    #[test]
    fn fleet_session_id_precedence_fleet_over_claude() {
        // FLEET_SESSION_ID wins over CLAUDE_CODE_SESSION_ID; empty never counts;
        // Claude is the fallback. Mirrors the CLI's read_fleet_session_id order
        // so a card/plan call attributes identically across both crates.
        assert_eq!(
            fleet_or_claude_session_id(Some("fleet".into()), Some("claude".into())),
            Some("fleet".into())
        );
        assert_eq!(
            fleet_or_claude_session_id(Some(String::new()), Some("claude".into())),
            Some("claude".into()),
            "empty FLEET_SESSION_ID falls back to Claude"
        );
        assert_eq!(
            fleet_or_claude_session_id(None, Some("claude".into())),
            Some("claude".into())
        );
        assert_eq!(fleet_or_claude_session_id(None, None), None);
    }

    #[test]
    fn resolve_session_id_from_env_uses_codex_launch_token() {
        // The codex new-spawn attribution seam: with no FLEET_SESSION_ID /
        // CLAUDE_CODE_SESSION_ID but a FLEET_CODEX_LAUNCH_TOKEN whose note Fleet
        // wrote, the shared resolver must return the recorded thread id — this is
        // how a `fleet__ask` card from a fresh codex session gets attributed.
        // Guards all three env vars under the fleet-home lock TmpHome holds.
        let _home = TmpHome::new("resolve-token");
        let prev_fleet = std::env::var_os("FLEET_SESSION_ID");
        let prev_claude = std::env::var_os("CLAUDE_CODE_SESSION_ID");
        let prev_token = std::env::var_os(FLEET_CODEX_LAUNCH_TOKEN_ENV);
        unsafe {
            std::env::remove_var("FLEET_SESSION_ID");
            std::env::remove_var("CLAUDE_CODE_SESSION_ID");
            std::env::set_var(FLEET_CODEX_LAUNCH_TOKEN_ENV, "tok-resolve");
        }
        record_launch_token("tok-resolve", "019f-thread-xyz");
        assert_eq!(
            resolve_fleet_session_id_from_env().as_deref(),
            Some("019f-thread-xyz"),
            "codex launch token must resolve to the recorded thread id"
        );
        // FLEET_SESSION_ID, when present, must win over the token path.
        unsafe { std::env::set_var("FLEET_SESSION_ID", "explicit-sid") };
        assert_eq!(
            resolve_fleet_session_id_from_env().as_deref(),
            Some("explicit-sid"),
            "explicit FLEET_SESSION_ID outranks the launch token"
        );
        // Restore prior env so sibling tests are unaffected.
        unsafe {
            match prev_fleet {
                Some(v) => std::env::set_var("FLEET_SESSION_ID", v),
                None => std::env::remove_var("FLEET_SESSION_ID"),
            }
            match prev_claude {
                Some(v) => std::env::set_var("CLAUDE_CODE_SESSION_ID", v),
                None => std::env::remove_var("CLAUDE_CODE_SESSION_ID"),
            }
            match prev_token {
                Some(v) => std::env::set_var(FLEET_CODEX_LAUNCH_TOKEN_ENV, v),
                None => std::env::remove_var(FLEET_CODEX_LAUNCH_TOKEN_ENV),
            }
        }
    }

    #[test]
    fn launch_token_rejects_path_traversal() {
        let _home = TmpHome::new("token-traversal");
        // A token that could escape the store dir must resolve to nothing and
        // write nothing (no panic, no file outside the dir).
        record_launch_token("../evil", "x");
        assert_eq!(resolve_launch_token("../evil"), None);
        assert_eq!(resolve_launch_token(""), None);
    }

    #[test]
    fn arg_builder_minimal_has_exec_json_cwd_and_prompt_after_dashdash() {
        let args = build_codex_exec_args("/ws", "do the thing", None, None, &[]);
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
    fn toml_basic_string_escapes_backslash_and_quote() {
        assert_eq!(toml_basic_string("plain"), "\"plain\"");
        assert_eq!(toml_basic_string("/abs/fleet"), "\"/abs/fleet\"");
        // A backslash and a double-quote must both be escaped so Codex's TOML
        // parser reads the value back verbatim (Windows paths, odd names).
        assert_eq!(toml_basic_string(r#"a\b"#), r#""a\\b""#);
        assert_eq!(toml_basic_string(r#"a"b"#), r#""a\"b""#);
    }

    #[test]
    fn exec_builder_inserts_pre_prompt_args_before_dashdash() {
        // The bypass flag + `-c mcp_servers.*` overrides MUST land before `--`,
        // or Codex would swallow them into the prompt. This is the correctness
        // property fleet_decision_card_args depends on.
        let pre = vec![
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
            "-c".to_string(),
            "mcp_servers.fleet.command=\"/bin/fleet\"".to_string(),
        ];
        let args = build_codex_exec_args("/ws", "the prompt", None, None, &pre);
        let dd = args.iter().position(|a| a == "--").expect("has --");
        let bypass = args
            .iter()
            .position(|a| a == "--dangerously-bypass-approvals-and-sandbox")
            .expect("has bypass flag");
        assert!(bypass < dd, "bypass flag must precede --");
        assert!(
            args.iter().position(|a| a.starts_with("mcp_servers.fleet.command")).unwrap() < dd,
            "mcp override must precede --"
        );
        assert_eq!(args.last().unwrap(), "the prompt", "prompt stays last");
    }

    #[test]
    fn resume_builder_inserts_pre_prompt_args_before_dashdash() {
        let pre = vec!["--dangerously-bypass-approvals-and-sandbox".to_string()];
        let args = build_codex_resume_args("tid", "cont", None, None, &pre);
        let dd = args.iter().position(|a| a == "--").expect("has --");
        let bypass = args
            .iter()
            .position(|a| a == "--dangerously-bypass-approvals-and-sandbox")
            .expect("has bypass flag");
        assert!(bypass < dd, "bypass flag must precede --");
        assert_eq!(args.last().unwrap(), "cont", "prompt stays last");
    }

    #[test]
    fn arg_builder_maps_model_and_effort() {
        let args = build_codex_exec_args("/ws", "hi", Some("gpt-5.6-sol"), Some("high"), &[]);
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
        let args = build_codex_exec_args("/ws", "hi", Some("  "), Some(""), &[]);
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

    /// Live end-to-end (M3 P10 acceptance): a Fleet-spawned Codex session must
    /// be resolvable back to its thread id from the launch token alone — the
    /// exact path a Codex child's `fleet handoff` / `fleet plan` takes, since
    /// Codex exposes no session-id env of its own. Spawn a real session, find
    /// the token note Fleet wrote for its thread id, then prove
    /// `read_fleet_session_id`'s resolver (`resolve_launch_token`) round-trips
    /// the token back to that id.
    ///   `cargo test -p claw-fleet-core codex_launch::tests::live_fleet_session_id -- --ignored`
    #[test]
    #[ignore = "spawns a real codex session; run manually with --ignored"]
    fn live_fleet_session_id_resolves_from_launch_token() {
        let ws = std::env::temp_dir().join(format!("fleet-codex-tok-live-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        let resp = spawn_new_codex_session(ws.to_str().unwrap(), "reply with exactly: OK", None, None)
            .expect("spawn should succeed");
        let sid = resp.session_id.expect("thread id captured");

        // Find the token whose note points at this thread id (the spawn just
        // wrote it once `thread.started` landed).
        let dir = launch_token_dir().expect("token dir");
        let mut found_token = None;
        for entry in std::fs::read_dir(&dir).expect("token dir readable").flatten() {
            let content = std::fs::read_to_string(entry.path()).unwrap_or_default();
            if content.trim() == sid {
                found_token = entry.file_name().to_str().map(str::to_string);
                break;
            }
        }
        let token = found_token.expect("a launch-token note maps to the spawned thread id");
        // This is exactly what the child does: token env → thread id.
        assert_eq!(
            resolve_launch_token(&token).as_deref(),
            Some(sid.as_str()),
            "launch token must resolve to the session's own thread id"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// Live (M4 P13 acceptance): the Fleet kill path must tear down a real Codex
    /// process. Spawn one, then `kill_pid_impl` its pid (the same tree-SIGTERM
    /// the `Backend::kill_pid` command runs) and confirm the pid is gone. The
    /// kill is source-agnostic — `CodexSource::kill_pid` and the Claude source
    /// both delegate to this function — so this exercises the codex process
    /// shape end-to-end. (Interrupt→resume *turn continuation* additionally
    /// needs a working Codex model; on a box where `codex exec` turns 400 on the
    /// model, only the signal/lifecycle half is observable — which is all the
    /// kill path depends on.)
    ///   `cargo test -p claw-fleet-core codex_launch::tests::live_kill -- --ignored`
    #[test]
    #[ignore = "spawns a real codex session; run manually with --ignored"]
    fn live_kill_tears_down_codex_process() {
        let ws = std::env::temp_dir().join(format!("fleet-codex-kill-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        let resp = spawn_new_codex_session(ws.to_str().unwrap(), "wait quietly", None, None)
            .expect("spawn should succeed");
        let pid = resp.pid;
        crate::session::kill_pid_impl(pid).expect("kill should not error");
        // Give the tree-SIGTERM a moment to land, then confirm the pid is dead.
        std::thread::sleep(Duration::from_millis(800));
        assert!(
            !crate::session::is_process_alive(pid),
            "codex pid {pid} must be dead after kill_pid_impl"
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
        let args = build_codex_resume_args("019f-abc", "keep going", None, None, &[]);
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
        let args = build_codex_resume_args("tid", "hi", Some("gpt-5.6-sol"), Some("high"), &[]);
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
        let args = build_codex_resume_args("tid", "hi", Some(" "), Some(""), &[]);
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

    /// Live end-to-end (P3 acceptance): a Fleet-spawned codex session must reach
    /// Fleet's Decision Panel via `fleet__ask`. Spawns real `codex exec` with the
    /// exact wiring [`fleet_decision_card_args`] emits (bypass + `-c
    /// mcp_servers.fleet.*` pointing at the built `fleet` binary) and asserts
    /// codex invokes the `fleet` MCP server's `fleet__ask` tool.
    ///
    /// Ignored: needs a real codex binary + model AND a Fleet decision consumer.
    /// Run against a *test* consumer (or with no consumer, asserting the
    /// "consumer not running" tool_error) so it does not queue a card into a live
    /// Fleet desktop. Verified manually 2026-07-15: with the real `fleet mcp`
    /// server the tool call reaches `handle_fleet_ask_call` and the card is
    /// attributed to the codex session id (launch token → thread id).
    ///   `cargo test -p claw-fleet-core codex_launch::tests::live_decision_card -- --ignored`
    #[test]
    #[ignore = "spawns real codex + needs a decision consumer; run manually"]
    fn live_decision_card_reaches_fleet_ask() {
        let fleet = crate::fleet_cli::resolve_fleet_binary()
            .expect("a fleet binary must be resolvable for the MCP bridge");
        let codex = crate::codex_source::find_codex_binary().expect("codex binary");
        let ws = std::env::temp_dir().join(format!("fleet-codex-card-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();

        let decision_args = fleet_decision_card_args(&[(
            "CLAUDE_PROJECT_DIR".to_string(),
            ws.to_string_lossy().into_owned(),
        )]);
        assert!(
            decision_args.iter().any(|a| a == "--dangerously-bypass-approvals-and-sandbox"),
            "bypass flag present"
        );
        let prompt = "Call the fleet__ask MCP tool from the \"fleet\" server with a single \
                      yes/no question, then reply with what it returned.";
        let args = build_codex_exec_args(
            &ws.to_string_lossy(),
            prompt,
            None,
            None,
            &decision_args,
        );
        let out = crate::process_util::command(&codex)
            .args(&args)
            .current_dir(&ws)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("codex exec runs");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("\"server\":\"fleet\"") && stdout.contains("fleet__ask"),
            "codex must invoke the fleet server's fleet__ask tool; stdout:\n{stdout}"
        );
        let _ = fleet; // fleet path is baked into decision_args above
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// Gap-2 unit: the codex turn-exit hook must write the idle sentinel even
    /// with no pending handoff (parity with the Claude Stop hook), and consuming
    /// an absent handoff must be a clean no-op (no spawn, no error). Cheap — no
    /// real codex; the no-pending path never reaches the spawner.
    #[test]
    fn on_codex_turn_exit_marks_idle_and_no_pending_is_noop() {
        let _home = TmpHome::new("turn-exit-idle");
        on_codex_turn_exit("sid-turnexit");
        let idle_path = crate::session::get_fleet_dir()
            .unwrap()
            .join("idle")
            .join("sid-turnexit.json");
        assert!(
            idle_path.exists(),
            "codex turn exit must write the idle sentinel (Stop-hook parity)"
        );
        // Empty id is a guarded no-op — no file, no panic.
        on_codex_turn_exit("");
        assert!(!crate::session::get_fleet_dir().unwrap().join("idle").join(".json").exists());
    }

    #[test]
    fn parses_thread_id_from_agent_turn_complete_notify_payload() {
        // The exact shape codex passes to the notify program (verified live).
        let payload = r#"{"type":"agent-turn-complete","thread-id":"019f669f-2269-7122-93ab-51e783755798","turn-id":"019f669f-22c0","cwd":"/ws","client":"codex_exec","input-messages":["hi"],"last-assistant-message":"ONE"}"#;
        assert_eq!(
            parse_agent_turn_complete_thread_id(payload).as_deref(),
            Some("019f669f-2269-7122-93ab-51e783755798")
        );
    }

    #[test]
    fn ignores_non_turn_complete_notify_payloads() {
        // Wrong event type → None (codex may add other notify events later).
        assert_eq!(
            parse_agent_turn_complete_thread_id(r#"{"type":"other","thread-id":"x"}"#),
            None
        );
        // Missing / empty thread-id → None.
        assert_eq!(
            parse_agent_turn_complete_thread_id(r#"{"type":"agent-turn-complete"}"#),
            None
        );
        assert_eq!(
            parse_agent_turn_complete_thread_id(
                r#"{"type":"agent-turn-complete","thread-id":""}"#
            ),
            None
        );
        // Garbage / non-JSON → None, no panic.
        assert_eq!(parse_agent_turn_complete_thread_id("not json"), None);
        assert_eq!(parse_agent_turn_complete_thread_id(""), None);
    }

    /// Gap-2 acceptance (live, end-to-end): a `fleet handoff` registered by a
    /// codex session must actually relay when its turn ends. Exercises the REAL
    /// resume reaper (not a direct helper call): spawn a codex session, register
    /// a pending handoff for its thread id (as `fleet handoff` would), resume it,
    /// and confirm that when the resumed turn ends the reaper's
    /// `on_codex_turn_exit` consumed the handoff and spawned a codex successor
    /// (a chain link now exists from the predecessor). Before this fix, codex had
    /// no Stop hook so the pending handoff was written but never consumed.
    /// Ignored — invokes the real codex binary + model across three turns.
    ///   `cargo test -p claw-fleet-core codex_launch::tests::live_codex_handoff_fires -- --ignored`
    #[test]
    #[ignore = "spawns + resumes real codex across turns; run manually"]
    fn live_codex_handoff_fires_from_resume_reaper() {
        let ws = std::env::temp_dir().join(format!("fleet-codex-hofire-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        let resp =
            spawn_new_codex_session(ws.to_str().unwrap(), "reply with exactly: FIRST", None, None)
                .expect("initial spawn");
        let tid = resp.session_id.expect("thread id captured");

        // Register a pending codex handoff, exactly as `fleet handoff` does.
        crate::handoff::register(
            &tid,
            ws.to_str().unwrap(),
            None,
            "relay successor: reply with exactly SUCCESSOR",
            None,
            None,
            None,
            None,
            "codex",
        )
        .expect("register pending handoff");

        // Resume; the reaper on THIS turn's exit must fire on_codex_turn_exit,
        // which consumes the handoff and spawns the successor synchronously
        // before on_exit sets `done`.
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done_cb = done.clone();
        resume_codex_session(
            &tid,
            ws.to_str().unwrap(),
            "reply with exactly: RESUMED",
            None,
            None,
            Box::new(move |_| done_cb.store(true, std::sync::atomic::Ordering::SeqCst)),
        )
        .expect("resume spawn");

        let mut waited = Duration::ZERO;
        while waited < Duration::from_secs(150) && !done.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(500));
            waited += Duration::from_millis(500);
        }
        assert!(done.load(std::sync::atomic::Ordering::SeqCst), "resume never completed");

        // The reaper spawned the successor before on_exit fired, so the chain
        // link from the predecessor must now exist.
        let chain = crate::handoff::chain_containing(&tid)
            .expect("handoff must have relayed from the codex turn exit");
        assert!(
            chain.links.iter().any(|l| l.from_session_id == tid),
            "chain must contain a relay from the predecessor {tid}: {chain:?}"
        );
        // Best-effort cleanup of the spawned successor process.
        if let Some(link) = chain.links.iter().find(|l| l.from_session_id == tid) {
            if let Some(pid) = resolve_spawn_pid(&link.to_session_id) {
                let _ = crate::session::kill_pid_impl(pid);
            }
        }
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
