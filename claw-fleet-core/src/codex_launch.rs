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

use std::io::Write;
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

/// Directory holding the transient stdout sinks for Fleet-spawned Codex turns
/// (`codex-live/<key>.jsonl`). Lives under the Fleet dir alongside the spawn-pid
/// notes.
fn codex_live_dir() -> Option<std::path::PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("codex-live"))
}

/// Create (truncating) a per-launch stdout sink file for a Codex child and
/// return `(write_handle, path)`. `key` is a filesystem-safe id known *before*
/// spawn — the launch token for a new spawn, the thread id for a resume.
///
/// Why a file and not a pipe: a pipe's read end is owned by the Fleet app, so
/// when the app quits/restarts the read end closes and Codex dies on its next
/// stdout write with `Broken pipe`, aborting the turn mid-flight (observed in the
/// wild: an app update-restart killed a live turn 17s in). A file fd never breaks
/// that way, so the child survives — the same reason Claude sessions, whose
/// stdout goes to a file/null, outlive an app restart. The transcript is read
/// from the on-disk rollout, not this sink; the sink exists only to keep the
/// child from blocking and to let the spawn path tail back the `thread.started`
/// line. The reaper thread removes it when the turn ends.
fn open_codex_stdout_sink(key: &str) -> Result<(std::fs::File, std::path::PathBuf), String> {
    // Same path-safety rejection as the spawn-pid notes.
    if key.is_empty() || key.contains('/') || key.contains('\\') || key.contains("..") {
        return Err(format!("unsafe codex stdout sink key: {key:?}"));
    }
    let dir = codex_live_dir().ok_or_else(|| "no fleet dir".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create codex-live dir: {e}"))?;
    let path = dir.join(format!("{key}.jsonl"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .map_err(|e| format!("open codex stdout sink {}: {e}", path.display()))?;
    Ok((file, path))
}

/// Extract the Codex thread id from the FIRST complete line of a stdout sink, or
/// `None` if no complete line has landed yet or it isn't a `thread.started`.
///
/// Reads only the first 8 KiB: `thread.started` is always Codex's first stdout
/// line, so that is always enough, and it bounds the read on a long turn that
/// (pathologically) never prints one.
fn read_first_thread_started(path: &std::path::Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf = [0u8; 8192];
    let n = f.read(&mut buf).ok()?;
    let s = String::from_utf8_lossy(&buf[..n]);
    let nl = s.find('\n')?;
    parse_thread_started(&s[..nl])
}

/// Tail `path` (a Codex stdout sink being written by `child`) for the first
/// `thread.started` line, returning its thread id. Gives up when `deadline`
/// passes or the child exits before emitting it, sleeping `poll` between reads.
///
/// Unlike a pipe, a file read returns EOF the instant it catches up to the
/// writer, so we poll: try the first line, and if it isn't there yet, sleep and
/// retry — checking child liveness so a child that dies before printing the line
/// doesn't hang us until the deadline.
fn tail_thread_started(
    path: &std::path::Path,
    child: &mut std::process::Child,
    deadline: std::time::Instant,
    poll: Duration,
) -> Option<String> {
    loop {
        if let Some(tid) = read_first_thread_started(path) {
            return Some(tid);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        if matches!(child.try_wait(), Ok(Some(_))) {
            // Child gone: one last look for a line it may have flushed on the way
            // out, then give up.
            return read_first_thread_started(path);
        }
        std::thread::sleep(poll);
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
/// The user's own `notify` command from codex's `config.toml`, so the Fleet
/// notify injection can chain-forward to it (Fleet's `-c notify` override
/// replaces the config value for the invocation but never edits the file, so the
/// user's command is still readable here).
///
/// Reads `$CODEX_HOME/config.toml` (falling back to `~/.codex/config.toml`),
/// expecting `notify = ["prog", "arg", …]`. Returns `None` when the file/key is
/// absent, malformed, empty, or — to prevent a self-invocation loop — when the
/// command is itself Fleet's `codex-notify` relay.
pub fn read_user_codex_notify() -> Option<Vec<String>> {
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| crate::session::real_home_dir().map(|h| h.join(".codex")))?;
    let cfg = std::fs::read_to_string(codex_home.join("config.toml")).ok()?;
    let value: toml::Value = cfg.parse().ok()?;
    let arr = value.get("notify")?.as_array()?;
    let cmd: Vec<String> = arr
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    if cmd.is_empty() {
        return None;
    }
    // Guard against forwarding to ourselves (user config already points at the
    // fleet relay) — that would loop.
    if cmd.iter().any(|a| a == "codex-notify") {
        return None;
    }
    Some(cmd)
}

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

/// `-c notify=[…]` override that wires a Fleet-spawned codex session's turn-end
/// to `fleet session codex-notify` — the codex analogue of Claude's `Stop` hook.
///
/// Codex fires the configured `notify` program once per turn (verified live for
/// both `codex exec` and `codex exec resume`, codex-cli 0.144.4), appending the
/// `agent-turn-complete` JSON payload as the final arg. Routing it to
/// `fleet session codex-notify` lets a pending `fleet handoff` relay at turn end
/// even after the Fleet desktop app quits — the reaper-thread approach could not,
/// since that thread dies with the spawning process. This is why the reaper
/// turn-exit call was removed in favour of this.
///
/// `notify` is a **single slot**: this `-c` override replaces any `notify` in the
/// user's `~/.codex/config.toml` for this invocation. `-c` does not modify the
/// file, so `fleet session codex-notify` re-reads the user's original `notify`
/// from `config.toml` at runtime and forwards the payload to it (see
/// `cmd_codex_notify`) — the user's own notify still runs. Returns empty when no
/// `fleet` binary resolves (the session still runs, just without the turn-end
/// relay).
pub fn fleet_notify_args() -> Vec<String> {
    let Some(fleet) = crate::fleet_cli::resolve_fleet_binary() else {
        crate::log_debug("fleet_notify_args: no fleet binary resolved; skipping codex notify relay");
        return Vec::new();
    };
    let fleet = fleet.to_string_lossy().into_owned();
    vec![
        "-c".to_string(),
        format!(
            "notify=[{},{},{}]",
            toml_basic_string(&fleet),
            toml_basic_string("session"),
            toml_basic_string("codex-notify"),
        ),
    ]
}

/// `-c` overrides that disable Codex's WebSocket transport by routing through an
/// HTTP-only custom provider — **only** when Codex is logged in with a ChatGPT
/// account. Empty otherwise.
///
/// Codex tries a `responses_websocket` transport first; on many machines each
/// attempt times out (~15s) before it falls back to HTTP/SSE, adding seconds of
/// dead time to every `codex exec` turn and logging the misleading
/// `timeout waiting for child process to exit` (openai/codex#22634). Setting
/// `supports_websockets = false` skips that. Codex forbids overriding the
/// built-in `openai` provider ("Built-in providers cannot be overridden"), so the
/// only way to flip the flag is to declare a custom provider — which requires a
/// hardcoded `base_url`. That URL is the ChatGPT backend, so these overrides are
/// safe to emit **only** for a ChatGPT login; for an API-key / Azure / OpenRouter
/// / custom-endpoint login they would repoint the session at the wrong backend
/// and break it, hence the [`codex_uses_chatgpt_auth`] gate.
///
/// Measured effect: median `codex exec` turn ~19s→16s on a ChatGPT-team account
/// (n=4 interleaved A/B). The variance is large; this shaves a few seconds off
/// every spawned/resumed Codex turn without touching the user's `config.toml`.
pub fn codex_ws_disable_args() -> Vec<String> {
    if !codex_uses_chatgpt_auth() {
        return Vec::new();
    }
    ws_disable_provider_args()
}

/// The concrete `-c` provider overrides, factored out (no I/O) for testing.
fn ws_disable_provider_args() -> Vec<String> {
    [
        "model_provider=chatgpt-http",
        "model_providers.chatgpt-http.name=ChatGPT HTTP",
        "model_providers.chatgpt-http.base_url=https://chatgpt.com/backend-api/codex",
        "model_providers.chatgpt-http.wire_api=responses",
        "model_providers.chatgpt-http.requires_openai_auth=true",
        "model_providers.chatgpt-http.supports_websockets=false",
    ]
    .iter()
    .flat_map(|kv| ["-c".to_string(), (*kv).to_string()])
    .collect()
}

/// Whether Codex is authenticated with a ChatGPT login (vs an API key / other),
/// per `$CODEX_HOME/auth.json` (falling back to `~/.codex/auth.json`). Gates
/// [`codex_ws_disable_args`]. Any read/parse failure returns `false` — we never
/// repoint a session we can't positively confirm is a ChatGPT login.
fn codex_uses_chatgpt_auth() -> bool {
    let Some(codex_home) = std::env::var_os("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| crate::session::real_home_dir().map(|h| h.join(".codex")))
    else {
        return false;
    };
    let Ok(raw) = std::fs::read_to_string(codex_home.join("auth.json")) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    auth_json_is_chatgpt(&v)
}

/// Pure predicate over a parsed `auth.json`: a ChatGPT login iff `auth_mode` is
/// `"chatgpt"` and no `OPENAI_API_KEY` is present. Split out for unit testing.
fn auth_json_is_chatgpt(v: &Value) -> bool {
    let mode_is_chatgpt = v.get("auth_mode").and_then(Value::as_str) == Some("chatgpt");
    let no_api_key = match v.get("OPENAI_API_KEY") {
        None => true,
        Some(k) => k.is_null() || k.as_str() == Some(""),
    };
    mode_is_chatgpt && no_api_key
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

/// Channel B (prompt-prepend) of codex guidance: if the codex-guidance toggle
/// is on (its `~/.codex/AGENTS.md` sentinel is present) AND the workspace has
/// active TASKS.md plans, prepend the same `<system-reminder>` block Claude gets
/// via the `fleet prd-context` UserPromptSubmit hook to the codex prompt.
///
/// Because Fleet drives every codex turn with a fresh `codex exec` /
/// `codex exec resume … -- <prompt>`, prepending here on each spawn and resume
/// gives the per-turn re-injection Claude's hook provides — this is the B2 path
/// chosen in P1 (B1 codex hooks are an experimental, already-renamed feature).
///
/// Gated on the single codex-guidance toggle so static (AGENTS.md) and dynamic
/// (this) injection move together. No toggle / no active plan → returns the
/// prompt unchanged (AC3 graceful degradation).
fn maybe_prepend_active_plans(
    workspace_path: &str,
    session_id: Option<&str>,
    prompt: &str,
) -> String {
    if !crate::codex_guidance::is_codex_guidance_installed() {
        return prompt.to_string();
    }
    match crate::prd_tasks::render_active_plans_reminder(Path::new(workspace_path), session_id) {
        Some(reminder) => format!("{reminder}\n\n{prompt}"),
        None => prompt.to_string(),
    }
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
    // Turn-end relay: route codex's `notify` to `fleet session codex-notify` (the
    // codex analogue of Claude's Stop hook), so a pending `fleet handoff` fires
    // when this session's turn ends — even after the Fleet app quits.
    let mut pre_prompt = decision_args;
    pre_prompt.extend(fleet_notify_args());
    // Skip Codex's flaky WebSocket transport on ChatGPT logins (no-op otherwise).
    pre_prompt.extend(codex_ws_disable_args());
    // Channel B: prepend the workspace's active TASKS.md plans (new session has
    // no thread id yet, so no backtrack backstop — pass None).
    let prompt = maybe_prepend_active_plans(&workspace_path, None, prompt);
    let args = build_codex_exec_args(&workspace_path, &prompt, model, effort, &pre_prompt);

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

    // Redirect stdout to a file sink instead of a pipe the app holds, so the
    // child outlives a Fleet app restart (see `open_codex_stdout_sink`). Keyed by
    // the launch token — the thread id isn't minted yet.
    let (stdout_sink, sink_path) = open_codex_stdout_sink(&launch_token)?;

    let mut cmd = crate::process_util::command(&codex);
    cmd.args(&args)
        .current_dir(&workspace_path)
        // MUST be null: `codex exec` otherwise blocks reading stdin forever.
        .stdin(std::process::Stdio::null())
        // File, not pipe: a pipe's read end dies with the app and takes the turn
        // down with `Broken pipe`; a file fd survives. Tailed below for the
        // `thread.started` line.
        .stdout(std::process::Stdio::from(stdout_sink))
        .stderr(std::process::Stdio::from(stderr_file));
    apply_codex_launch_env(&mut cmd);
    cmd.env(FLEET_CODEX_LAUNCH_TOKEN_ENV, &launch_token);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn codex failed: {e}"))?;
    let pid = child.id();

    // Tail/reaper thread: tail the stdout sink for the `thread.started` line to
    // learn the thread id (Codex mints it post-spawn), record the spawn pid, then
    // reap the child and clean up. The tail reads a FILE, not a pipe, so nothing
    // here keeps the child tethered to the app — the app can quit and the child
    // (and this turn) keep running; only this reaper dies with it.
    let (tx, rx) = mpsc::channel::<String>();
    let stderr_log_owned = stderr_log.clone();
    let sink_path_owned = sink_path.clone();
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + THREAD_STARTED_TIMEOUT;
        // Thread id captured from `thread.started`, kept so we can drop its
        // spawn-pid note once the child exits below.
        let mut spawned_thread: Option<String> = None;
        if let Some(thread_id) =
            tail_thread_started(&sink_path_owned, &mut child, deadline, Duration::from_millis(50))
        {
            // Note the spawn pid so new-session liveness can recognise this
            // still-running session before its id lands in any argv.
            record_spawn_pid(&thread_id, pid);
            spawned_thread = Some(thread_id.clone());
            let _ = tx.send(thread_id);
        }
        drop(tx);
        // Reap the child and log its exit.
        let result = child.wait();
        // Session is gone: drop the spawn-pid note so a later pid reuse can never
        // read this dead session as alive.
        if let Some(tid) = spawned_thread.take() {
            clear_spawn_pid(&tid);
            // Note: the turn-end relay (mark idle + consume handoff) is NOT fired
            // here. It runs via codex's own `notify` hook → `fleet session
            // codex-notify` → on_codex_turn_exit (injected by fleet_notify_args),
            // which fires in codex's process and survives the Fleet app quitting —
            // unlike this reaper thread, which dies with the spawning process.
        }
        // Transient stdout sink: drop it now the turn is over. (Leaks only if the
        // app dies first — harmless and tiny, and the child surviving is the point.)
        let _ = std::fs::remove_file(&sink_path_owned);
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
    // Turn-end relay (see spawn): route codex `notify` to `fleet session
    // codex-notify` so a pending handoff fires when this resumed turn ends.
    let mut pre_prompt = decision_args;
    pre_prompt.extend(fleet_notify_args());
    // Skip Codex's flaky WebSocket transport on ChatGPT logins (no-op otherwise).
    pre_prompt.extend(codex_ws_disable_args());
    // Channel B: prepend the workspace's active TASKS.md plans; this resume knows
    // its thread id, so the backtrack backstop can fire for a completed child.
    let prompt = maybe_prepend_active_plans(&workspace_path, Some(session_id), prompt);
    let args = build_codex_resume_args(session_id, &prompt, model, effort, &pre_prompt);

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

    // File sink, not a pipe: a resumed turn must survive a Fleet app restart too.
    // Keyed by the thread id (known up front on resume).
    let (stdout_sink, sink_path) = open_codex_stdout_sink(session_id)?;

    let mut cmd = crate::process_util::command(&codex);
    cmd.args(&args)
        .current_dir(&workspace_path)
        // MUST be null: `codex exec` otherwise blocks reading stdin forever.
        .stdin(std::process::Stdio::null())
        // File, not pipe (see spawn): keeps the child off the app's lifetime. The
        // transcript is read from the on-disk rollout, not this stream.
        .stdout(std::process::Stdio::from(stdout_sink))
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

    // Reaper thread: reap the child, log its exit, and invoke `on_exit`. stdout
    // goes to the file sink (not a pipe), so — unlike before — nothing needs to
    // drain it and the child is not tethered to the app's lifetime.
    let stderr_log_owned = stderr_log.clone();
    let sink_path_owned = sink_path.clone();
    std::thread::spawn(move || {
        let result = child.wait();
        let success = matches!(&result, Ok(status) if status.success());
        // Transient stdout sink: drop it now the turn is over.
        let _ = std::fs::remove_file(&sink_path_owned);
        // Note: the turn-end relay runs via codex's `notify` hook (see spawn
        // reaper), not here — so it fires even after the Fleet app quits.
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
    fn auth_json_gate_only_true_for_chatgpt_login() {
        use serde_json::json;
        // ChatGPT login: auth_mode == chatgpt, no API key → gated ON.
        assert!(auth_json_is_chatgpt(&json!({"auth_mode": "chatgpt", "OPENAI_API_KEY": null})));
        assert!(auth_json_is_chatgpt(&json!({"auth_mode": "chatgpt"})));
        assert!(auth_json_is_chatgpt(&json!({"auth_mode": "chatgpt", "OPENAI_API_KEY": ""})));
        // API-key login (even if auth_mode still says chatgpt) → OFF: repointing
        // the base_url would break a non-ChatGPT backend.
        assert!(!auth_json_is_chatgpt(&json!({"auth_mode": "chatgpt", "OPENAI_API_KEY": "sk-xyz"})));
        assert!(!auth_json_is_chatgpt(&json!({"auth_mode": "apikey", "OPENAI_API_KEY": "sk-xyz"})));
        // Missing / malformed → OFF.
        assert!(!auth_json_is_chatgpt(&json!({})));
        assert!(!auth_json_is_chatgpt(&json!({"auth_mode": "other"})));
    }

    #[test]
    fn ws_disable_provider_args_shape_pairs_c_flags() {
        let args = ws_disable_provider_args();
        // Even count: every value is preceded by its own `-c`.
        assert_eq!(args.len() % 2, 0);
        for pair in args.chunks(2) {
            assert_eq!(pair[0], "-c");
        }
        // Selects the custom provider and disables WebSockets.
        assert!(args.contains(&"model_provider=chatgpt-http".to_string()));
        assert!(args.contains(&"model_providers.chatgpt-http.supports_websockets=false".to_string()));
        // Never touches the reserved built-in `openai` provider (Codex rejects that).
        assert!(!args.iter().any(|a| a.contains("model_providers.openai.")));
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
    fn reads_user_notify_from_codex_config_and_guards_self_and_absent() {
        // Serialize env mutation on the shared home lock (TmpHome holds it too).
        let _lock = crate::session::fleet_home_lock();
        let dir = std::env::temp_dir().join(format!(
            "fleet-codexcfg-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::var_os("CODEX_HOME");
        unsafe { std::env::set_var("CODEX_HOME", &dir) };

        // User has a notify → returned verbatim.
        std::fs::write(dir.join("config.toml"), "notify = [\"my-notifier\", \"--flag\"]\n").unwrap();
        assert_eq!(
            read_user_codex_notify(),
            Some(vec!["my-notifier".to_string(), "--flag".to_string()])
        );
        // Self-reference (points at the fleet relay) → None, so we never loop.
        std::fs::write(
            dir.join("config.toml"),
            "notify = [\"fleet\", \"session\", \"codex-notify\"]\n",
        )
        .unwrap();
        assert_eq!(read_user_codex_notify(), None);
        // No notify key → None.
        std::fs::write(dir.join("config.toml"), "model = \"gpt-5.6-sol\"\n").unwrap();
        assert_eq!(read_user_codex_notify(), None);

        unsafe {
            match prev {
                Some(v) => std::env::set_var("CODEX_HOME", v),
                None => std::env::remove_var("CODEX_HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
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

    /// The turn-end relay is now driven by codex's `notify` hook, not the reaper:
    /// `fleet_notify_args` must inject a `-c notify=[…]` override pointing codex at
    /// `fleet session codex-notify`. (The full live e2e — codex fires notify →
    /// fleet consumes the handoff → codex successor — is a shell test run against
    /// the freshly-built fleet binary; a cargo #[ignore] test can't guarantee
    /// `resolve_fleet_binary` returns THIS build.)
    #[test]
    fn fleet_notify_args_inject_codex_notify_override() {
        let args = fleet_notify_args();
        // Degrades to empty only when no fleet binary resolves; in the test tree
        // one is resolvable, so assert the shape. If empty (no binary), skip.
        if args.is_empty() {
            return;
        }
        assert_eq!(args[0], "-c");
        assert!(
            args[1].starts_with("notify=[") && args[1].ends_with(']'),
            "must be a TOML notify array: {}",
            args[1]
        );
        assert!(
            args[1].contains("\"session\"") && args[1].contains("\"codex-notify\""),
            "notify must route to `fleet session codex-notify`: {}",
            args[1]
        );
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

    /// Unique temp file path for a stdout-sink test.
    fn tmp_sink(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "fleet-codexsink-{}-{}-{}.jsonl",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    #[test]
    fn read_first_thread_started_needs_a_complete_first_line() {
        let path = tmp_sink("partial");
        // No file yet → None.
        assert_eq!(read_first_thread_started(&path), None);
        // Partial line (no trailing newline) → None: the writer is mid-flush.
        std::fs::write(&path, r#"{"type":"thread.started","thread_id":"019abc"#).unwrap();
        assert_eq!(read_first_thread_started(&path), None);
        // Completed line → the id.
        std::fs::write(&path, "{\"type\":\"thread.started\",\"thread_id\":\"019abc\"}\n").unwrap();
        assert_eq!(read_first_thread_started(&path).as_deref(), Some("019abc"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_first_thread_started_only_looks_at_the_first_line() {
        let path = tmp_sink("firstline");
        // thread.started is always Codex's first line; a non-matching first line
        // means None even if a later line would match.
        std::fs::write(
            &path,
            "{\"type\":\"turn.started\"}\n{\"type\":\"thread.started\",\"thread_id\":\"late\"}\n",
        )
        .unwrap();
        assert_eq!(read_first_thread_started(&path), None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn tail_thread_started_picks_up_a_delayed_line() {
        // A child that writes its `thread.started` line to the sink after a short
        // delay; the tailer must poll the file and return the id.
        let path = tmp_sink("tail");
        let f = std::fs::File::create(&path).unwrap();
        let mut child = std::process::Command::new("sh")
            .args([
                "-c",
                "sleep 0.2; printf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"t-123\"}'; sleep 0.2",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(f))
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let got = tail_thread_started(&path, &mut child, deadline, Duration::from_millis(20));
        assert_eq!(got.as_deref(), Some("t-123"));
        let _ = child.wait();
        let _ = std::fs::remove_file(&path);
    }

    /// The root-cause regression: a child whose stdout is a PIPE dies when the
    /// read end goes away (the app restart that aborted a Codex turn 17s in),
    /// whereas a child whose stdout is a FILE survives and exits cleanly. Fleet's
    /// fix is exactly this pipe→file swap for Codex stdout.
    #[test]
    fn file_backed_stdout_survives_reader_loss_but_pipe_does_not() {
        // Pipe variant: drop the read end, then the child's delayed write hits a
        // broken pipe (SIGPIPE → death, so it does NOT exit successfully).
        let mut pipe_child = std::process::Command::new("sh")
            .args(["-c", "sleep 0.3; echo late"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        drop(pipe_child.stdout.take()); // the app/reader going away
        let pipe_status = pipe_child.wait().unwrap();

        // File variant: the delayed write lands in the file; the child exits 0.
        let path = tmp_sink("survive");
        let f = std::fs::File::create(&path).unwrap();
        let mut file_child = std::process::Command::new("sh")
            .args(["-c", "sleep 0.3; echo late"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(f))
            .spawn()
            .unwrap();
        let file_status = file_child.wait().unwrap();

        assert!(
            file_status.success(),
            "file-backed child must survive the reader leaving and exit 0"
        );
        assert!(std::fs::read_to_string(&path).unwrap().contains("late"));
        assert!(
            !pipe_status.success(),
            "pipe-backed child is killed by the broken-pipe write — the bug the fix removes"
        );
        let _ = std::fs::remove_file(&path);
    }
}
