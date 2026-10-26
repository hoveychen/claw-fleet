//! A per-turn stdin holder for headless Claude Code sessions.
//!
//! Fleet launches every Claude turn as `claude -p <prompt>`, so nothing holds
//! the child's stdin and the only way to reach a running turn is the CLI's
//! cross-session socket ([`crate::live_inject`]). That path has two problems
//! that matter exactly when the user interrupts a busy turn:
//!
//! * the socket stamps every message as a *peer's*, never the user's;
//! * the CLI only drains its input queue at a tool boundary, so a message sent
//!   while a five-minute foreground Bash runs waits five minutes.
//!
//! A follow-up typed mid-turn usually means something is going wrong and has
//! to be adjusted *now*. So for the turn's lifetime a small host process sits
//! between Fleet and the CLI: it starts `claude -p --input-format stream-json`,
//! writes the opening prompt to its stdin, and keeps that stdin open until the
//! turn's first `result`. A user follow-up is written to the same stdin — the
//! CLI records it as the user's own words — and every foreground task still in
//! flight gets a targeted `background_tasks` control request, which turns it
//! into a background task at once and lets the message be absorbed at the
//! resulting tool boundary.
//!
//! One host per turn, never across turns: closing stdin at the first `result`
//! makes the CLI behave exactly like `-p <prompt>` from then on (it still waits
//! for background shells and still answers a message written just before the
//! close — both measured on CLI 2.1.310).
//!
//! The host is the same binary re-exec'd with [`HOST_ARGV_MARKER`] (desktop app
//! or `fleet` CLI, both register themselves via [`register_host_exe`]); it is
//! detached so quitting the spawner does not end the turn (`setsid` on unix; on
//! Windows a child outlives its parent anyway and `CREATE_NO_WINDOW` gives it
//! a hidden console of its own). The host also takes over the live-thinking
//! sidecar: it copies the CLI's stdout into `live-thinking/pid-<claude
//! pid>.jsonl` line by line, the format the reader already understands.
//!
//! Clients reach the host through one of two transports, named in its
//! [`HostRecord`]: a unix socket, or — on Windows, where there is none — an
//! append-only JSON-lines control file the host tails every 50ms (the same
//! shape as `proc_runner`'s `<id>.ctl`), answered through a per-request reply
//! file. The file transport compiles everywhere and [`TRANSPORT_ENV`] selects
//! it on unix, so it is tested on every platform.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// `argv[1]` that turns the desktop app / `fleet` CLI into a claude host.
pub const HOST_ARGV_MARKER: &str = "fleet-claude-host";

/// Env var carrying the JSON-encoded [`HostSpec`] from spawner to host.
pub const SPEC_ENV: &str = "FLEET_CLAUDE_HOST_SPEC";

/// Kill switch: `FLEET_CLAUDE_HOST=0` launches claude directly, as before.
pub const DISABLE_ENV: &str = "FLEET_CLAUDE_HOST";

/// `FLEET_CLAUDE_HOST_TRANSPORT=file` makes a unix host serve the control
/// file instead of the socket (Windows always does).
pub const TRANSPORT_ENV: &str = "FLEET_CLAUDE_HOST_TRANSPORT";

/// How long a client waits for the host to answer a follow-up.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);

/// A control-file request older than this is refused instead of delivered:
/// its client may already have timed out and queued the text, and delivering
/// it as well would give the session the same message twice. Shorter than
/// [`CLIENT_TIMEOUT`] so the two cannot both happen.
const FILE_REQUEST_TTL: Duration = Duration::from_secs(4);

/// Host-side poll interval of the control file (proc_runner uses the same).
const FILE_POLL: Duration = Duration::from_millis(50);

/// A foreground task that started less than this long ago is given the rest
/// of the window before it is backgrounded: a follow-up is often followed by a
/// quick command of the agent's own, and backgrounding a two-second `ls` only
/// adds a round trip.
pub const BACKGROUND_GRACE: Duration = Duration::from_secs(2);

static HOST_EXE: OnceLock<PathBuf> = OnceLock::new();

/// Record the binary that can serve as a host. Only the desktop app and the
/// `fleet` CLI call this — anything else (test binaries in particular) keeps
/// the direct launch, so a test can never re-exec itself as a host.
pub fn register_host_exe(exe: PathBuf) {
    let _ = HOST_EXE.set(exe);
}

/// The binary the desktop app should register as its host: itself, except on
/// Windows, where it is the `fleet` CLI shipped beside it when there is one.
///
/// Tauri's NSIS installer (`CheckIfAppIsRunning "${MAINBINARYNAME}.exe"` in
/// tauri-bundler's `installer.nsi`, used by both the Install and Uninstall
/// sections) looks up every process named after the app's main binary and
/// `TerminateProcess`es all of them. A host running as that exe would be
/// killed mid-turn by every update. The sidecar is only copied over, never
/// looked up by name. A running sidecar makes that copy fail rather than kill
/// anything — and every running Fleet turn already holds it as its `fleet mcp`
/// server, so hosting turns on it adds no new case.
pub fn desktop_host_exe(current_exe: PathBuf) -> PathBuf {
    if cfg!(windows) {
        if let Some(fleet) = current_exe.parent().and_then(crate::fleet_cli::sidecar_in) {
            return fleet;
        }
    }
    current_exe
}

/// What the host needs to start the turn. Crosses a process boundary as JSON.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct HostSpec {
    pub claude_path: String,
    /// The launch argv with the prompt taken out and `--input-format
    /// stream-json` added (see [`rewrite_args`]).
    pub args: Vec<String>,
    pub cwd: String,
    /// Written to stdin as the turn's opening user message.
    pub prompt: String,
    pub session_id: String,
    /// `~/.fleet/live-thinking`; `None` drops the CLI's stdout like before.
    pub sidecar_dir: Option<PathBuf>,
    /// `~/.fleet/claude-hosts`, resolved by the spawner so host and client
    /// agree even when the host's environment differs.
    pub registry_dir: PathBuf,
}

/// The host's entry in [`registry_dir`]: how a client reaches the turn that is
/// running right now.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct HostRecord {
    pub session_id: String,
    pub host_pid: u32,
    pub host_start_time: Option<u64>,
    pub claude_pid: u32,
    /// Unix control socket; empty when the host serves [`Self::ctl`] instead.
    #[serde(default)]
    pub sock: PathBuf,
    /// Append-only control file (Windows, or [`TRANSPORT_ENV`]` = file`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctl: Option<PathBuf>,
}

/// One control request, a JSON line on either transport.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
struct Request {
    op: String,
    #[serde(default)]
    text: String,
    /// Control file only: names the reply file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    /// Control file only: when the client wrote it, ms since the epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sent_ms: Option<u64>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn use_file_transport() -> bool {
    cfg!(not(unix)) || std::env::var(TRANSPORT_ENV).as_deref() == Ok("file")
}

pub fn registry_dir() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("claude-hosts"))
}

fn record_path(dir: &Path, session_id: &str) -> PathBuf {
    dir.join(format!("{session_id}.json"))
}

/// Rewrite a `-p <prompt>` launch into its stream-json-input form.
///
/// Returns the new argv and the prompt, or `None` when the launch is not one
/// the host can drive: no `-p <prompt>` pair, a blank prompt, an explicit
/// `--input-format` already, no stream-json output to read events from, or a
/// `--fork-session` (whose id is minted by the CLI, so no client could find
/// the host by session id).
pub fn rewrite_args(args: &[String]) -> Option<(Vec<String>, String)> {
    if args.iter().any(|a| a == "--fork-session" || a == "--input-format") {
        return None;
    }
    let stream_out = args
        .windows(2)
        .any(|w| w[0] == "--output-format" && w[1] == "stream-json");
    if !stream_out {
        return None;
    }
    let p = args.iter().position(|a| a == "-p" || a == "--print")?;
    let prompt = args.get(p + 1)?.clone();
    if prompt.trim().is_empty() || prompt.starts_with("--") {
        return None;
    }
    let mut out: Vec<String> = Vec::with_capacity(args.len() + 1);
    out.extend_from_slice(&args[..=p]);
    out.extend_from_slice(&args[p + 2..]);
    out.push("--input-format".into());
    out.push("stream-json".into());
    Some((out, prompt))
}

/// Decide whether this launch goes through a host, and with which spec.
pub(crate) fn plan(
    claude_path: &str,
    args: &[String],
    cwd: &str,
    session_id: Option<String>,
) -> Option<(PathBuf, HostSpec)> {
    if std::env::var(DISABLE_ENV).as_deref() == Ok("0") {
        return None;
    }
    let exe = HOST_EXE.get()?.clone();
    let session_id = session_id?;
    let (args, prompt) = rewrite_args(args)?;
    let registry_dir = registry_dir()?;
    Some((
        exe,
        HostSpec {
            claude_path: claude_path.to_string(),
            args,
            cwd: cwd.to_string(),
            prompt,
            session_id,
            sidecar_dir: crate::live_thinking::ensure_sidecar_dir().ok(),
            registry_dir,
        },
    ))
}

/// Result of [`spawn`].
pub(crate) enum Spawned {
    /// The host is up and claude is running under it.
    Host { host: std::process::Child, claude_pid: u32 },
    /// The host could not start claude and has exited; nothing is running, so
    /// the caller may launch claude directly instead.
    Fallback(String),
}

/// Start a host for `spec`, carrying over every env change `template` (the
/// command the caller would otherwise have run) makes. Blocks until the host
/// reports claude's pid.
pub(crate) fn spawn(
    exe: &Path,
    spec: &HostSpec,
    template: &std::process::Command,
    stderr_log: &Path,
) -> Result<Spawned, String> {
    use std::io::BufRead;

    let spec_json = serde_json::to_string(spec).map_err(|e| format!("encode host spec: {e}"))?;
    let stderr_file = std::fs::OpenOptions::new()
        .append(true)
        .open(stderr_log)
        .map_err(|e| format!("reopen stderr log {}: {e}", stderr_log.display()))?;
    // CREATE_NO_WINDOW on Windows: no conhost flash, and a hidden console of
    // the host's own, so closing or Ctrl-C-ing the spawner's console does not
    // reach the turn.
    let mut cmd = crate::process_util::command(exe);
    cmd.arg(HOST_ARGV_MARKER);
    for (k, v) in template.get_envs() {
        match v {
            Some(v) => cmd.env(k, v),
            None => cmd.env_remove(k),
        };
    }
    cmd.env(SPEC_ENV, spec_json)
        .current_dir(&spec.cwd)
        .stdin(std::process::Stdio::null())
        // Handshake only: one `pid <n>` / `err <msg>` line, then silence.
        .stdout(std::process::Stdio::piped())
        .stderr(stderr_file);
    // Own session: quitting the desktop app / `fleet serve` must not end the
    // turn. No `process_group(0)` here — setsid already makes it a group
    // leader, and setsid on an existing group leader fails. Windows needs
    // nothing: a child is not killed with its parent (no job object).
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut host = cmd
        .spawn()
        .map_err(|e| format!("spawn claude host {}: {e}", exe.display()))?;
    // Release our copies of the child's pipe ends now (see the proc_runner
    // pty hang): only the handshake reader below may hold one.
    drop(cmd);

    let stdout = host.stdout.take().ok_or("claude host: no stdout")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(line) => line,
        Err(_) => {
            // Whether claude started is unknown; killing the host closes its
            // stdin, so a claude that did start ends without a prompt.
            let _ = host.kill();
            let _ = host.wait();
            return Err("claude host did not report within 15s".into());
        }
    };
    let line = line.trim();
    if let Some(pid) = line.strip_prefix("pid ").and_then(|p| p.parse::<u32>().ok()) {
        return Ok(Spawned::Host { host, claude_pid: pid });
    }
    let _ = host.wait();
    Ok(Spawned::Fallback(if line.is_empty() {
        "claude host exited before reporting".into()
    } else {
        line.trim_start_matches("err ").to_string()
    }))
}

/// Receipt for a delivered follow-up.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Reply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Tool-use ids of the foreground tasks scheduled to be backgrounded.
    #[serde(default)]
    pub backgrounded: Vec<String>,
}

/// Control op for a message the user typed: delivered as their own words, and
/// it backgrounds whatever foreground task blocks the next tool boundary.
const OP_USER: &str = "user";

/// Control op for a message from another agent (`Sender::Agent`): delivered in
/// Fleet's peer frame and absorbed at the next tool boundary on its own, the
/// way the CLI socket delivers it — no task is backgrounded for it.
const OP_AGENT: &str = "agent";

/// Whether agent messages go through the host. Only where the CLI's own
/// socket ([`crate::live_inject`]) is unavailable, i.e. Windows: on unix the
/// socket already delivers them with a real peer origin, and unlike the host
/// it keeps working after the turn's first `result` closes stdin.
pub const AGENT_MESSAGES_VIA_HOST: bool = cfg!(not(unix));

/// Control op that ends claude when its turn is over and it is only waiting
/// for background tasks. Refused while the model is working.
const OP_END_IDLE: &str = "end_idle";

/// Write `text` into `session_id`'s running turn as the user's own message.
///
/// `Err` whenever there is no live host for the session or its turn is over;
/// the caller falls back to the socket injection / the queue.
pub fn send_user_message(session_id: &str, text: &str) -> Result<Reply, String> {
    let dir = registry_dir().ok_or("no fleet dir")?;
    send_in(&dir, session_id, OP_USER, text)
}

/// Write another agent's `text` into `session_id`'s running turn, wrapped in
/// [`crate::live_inject::frame_as_agent`] so neither the receiving model nor
/// the transcript readers take it for the user's words.
///
/// `Err` as for [`send_user_message`]; a host from before this op existed
/// answers "unknown op", which the caller treats like any other miss.
pub fn send_agent_message(session_id: &str, text: &str) -> Result<Reply, String> {
    let dir = registry_dir().ok_or("no fleet dir")?;
    send_in(&dir, session_id, OP_AGENT, &crate::live_inject::frame_as_agent(text))
}

/// End `session_id`'s claude if its turn already produced its `result` and
/// the CLI is only waiting for background tasks to finish.
///
/// That wait has no ceiling in Fleet sessions, so a background command that
/// never exits (a log stream, a dev server) keeps the process — and with it
/// the session's drive lease — alive for as long as the command's own
/// timeout, while nothing visibly happens. On SIGTERM the CLI marks its
/// background tasks killed and exits within seconds (measured on 2.1.310).
///
/// `Err` when there is no live host, the turn is still working, or the host
/// predates this op ("unknown op").
pub fn end_idle_turn(session_id: &str) -> Result<(), String> {
    let dir = registry_dir().ok_or("no fleet dir")?;
    send_in(&dir, session_id, OP_END_IDLE, "").map(|_| ())
}

/// Whether `session_id` has a live host right now — the host-side analogue
/// of [`crate::live_inject::can_inject`].
pub fn has_live_host(session_id: &str) -> bool {
    registry_dir().is_some_and(|dir| live_record(&dir, session_id).is_ok())
}

fn live_record(dir: &Path, session_id: &str) -> Result<HostRecord, String> {
    let raw = std::fs::read_to_string(record_path(dir, session_id))
        .map_err(|_| "no claude host for this session".to_string())?;
    let rec: HostRecord = serde_json::from_str(&raw).map_err(|e| format!("host record: {e}"))?;
    if crate::session::process_start_time(rec.host_pid) != rec.host_start_time
        || rec.host_start_time.is_none()
    {
        return Err("claude host is gone".into());
    }
    Ok(rec)
}

fn send_in(dir: &Path, session_id: &str, op: &str, text: &str) -> Result<Reply, String> {
    let rec = live_record(dir, session_id)?;
    let reply = match &rec.ctl {
        Some(ctl) => file_request(ctl, op, text)?,
        None => socket_request(&rec.sock, op, text)?,
    };
    if reply.ok {
        Ok(reply)
    } else {
        Err(reply.error.unwrap_or_else(|| "claude host refused".into()))
    }
}

#[cfg(unix)]
fn socket_request(sock: &Path, op: &str, text: &str) -> Result<Reply, String> {
    use std::io::{BufRead, Write};

    let stream = std::os::unix::net::UnixStream::connect(sock)
        .map_err(|e| format!("connect claude host: {e}"))?;
    stream.set_read_timeout(Some(CLIENT_TIMEOUT)).ok();
    stream.set_write_timeout(Some(CLIENT_TIMEOUT)).ok();
    let mut req = serde_json::to_string(&serde_json::json!({"op": op, "text": text}))
        .map_err(|e| e.to_string())?;
    req.push('\n');
    (&stream)
        .write_all(req.as_bytes())
        .map_err(|e| format!("write claude host: {e}"))?;
    let mut line = String::new();
    std::io::BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|e| format!("read claude host: {e}"))?;
    serde_json::from_str(line.trim()).map_err(|e| format!("claude host reply: {e}"))
}

#[cfg(not(unix))]
fn socket_request(_sock: &Path, _op: &str, _text: &str) -> Result<Reply, String> {
    Err("claude host record names no control file".into())
}

/// `h<pid>.ctl` → `h<pid>.<id>.reply`.
fn reply_path(ctl: &Path, id: &str) -> PathBuf {
    ctl.with_extension(format!("{id}.reply"))
}

/// Request ids become file names: keep them to a safe alphabet.
fn valid_request_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn new_request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!(
        "{:x}-{:x}-{:x}-{:x}",
        std::process::id(),
        now_ms(),
        nanos,
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// Client side of the control-file transport: append one request line, then
/// wait for the host to rename its answer into place.
fn file_request(ctl: &Path, op: &str, text: &str) -> Result<Reply, String> {
    use std::io::Write;

    let id = new_request_id();
    let req = Request {
        op: op.into(),
        text: text.into(),
        id: Some(id.clone()),
        sent_ms: Some(now_ms()),
    };
    let mut line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
    line.push('\n');
    // No `create`: the host makes the file at start and removes it at exit,
    // so a missing file means there is nobody to read the request.
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(ctl)
        .map_err(|e| format!("open claude host control file: {e}"))?;
    // One write: the host only consumes complete lines, and an append-mode
    // write lands at the end as a unit, so concurrent clients do not interleave.
    f.write_all(line.as_bytes())
        .map_err(|e| format!("write claude host control file: {e}"))?;
    drop(f);

    let reply_path = reply_path(ctl, &id);
    let deadline = std::time::Instant::now() + CLIENT_TIMEOUT;
    loop {
        if let Ok(raw) = std::fs::read(&reply_path) {
            let _ = std::fs::remove_file(&reply_path);
            return serde_json::from_slice(&raw).map_err(|e| format!("claude host reply: {e}"));
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("claude host did not answer within {CLIENT_TIMEOUT:?}"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Host side of the control-file transport: tail `ctl` and answer each
/// request through its reply file until the file is removed (host exit).
fn serve_file(ctl: &Path, mut handle: impl FnMut(&Request) -> Reply) {
    let mut offset = 0;
    while ctl.exists() {
        let (lines, next) = crate::proc_runner::drain_lines(ctl, offset);
        offset = next;
        for line in lines {
            let Ok(req) = serde_json::from_slice::<Request>(&line) else { continue };
            // Without a usable id there is nowhere to answer; the client
            // times out and falls back.
            let Some(id) = req.id.as_deref().filter(|id| valid_request_id(id)) else { continue };
            let age = req.sent_ms.map(|t| now_ms().saturating_sub(t));
            let reply = if age.is_none_or(|a| a > FILE_REQUEST_TTL.as_millis() as u64) {
                Reply { ok: false, error: Some("request expired".into()), backgrounded: vec![] }
            } else {
                handle(&req)
            };
            let path = reply_path(ctl, id);
            let tmp = path.with_extension("tmp");
            let body = serde_json::to_vec(&reply).unwrap_or_default();
            if std::fs::write(&tmp, body).and_then(|()| std::fs::rename(&tmp, &path)).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        }
        std::thread::sleep(FILE_POLL);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Turn state machine — pure, so it is unit-tested without a CLI.
// ─────────────────────────────────────────────────────────────────────────────

/// What a line of the CLI's stdout means to the host.
#[derive(Debug, PartialEq)]
enum Event {
    TaskStarted { task_id: String, tool_use_id: String, foreground: bool },
    /// Backgrounded, finished, or otherwise no longer blocking the turn.
    TaskReleased { task_id: String },
    /// A `user` row carrying tool results: the CLI just crossed a tool
    /// boundary, which is where it absorbs queued input.
    ToolResults(Vec<String>),
    Result,
    /// The model is working: a token delta, an assistant row, an API retry.
    /// After a `result` this means a background task woke the agent for
    /// another round, so the turn is no longer idle.
    Activity,
    Other,
}

fn classify(line: &str) -> Event {
    // The bulk of the stream is token deltas; classify them without parsing.
    if line.starts_with(r#"{"type":"stream_event""#) {
        return Event::Activity;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return Event::Other;
    };
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    match (v.get("type").and_then(|t| t.as_str()), v.get("subtype").and_then(|t| t.as_str())) {
        (Some("system"), Some("task_started")) => {
            let backgrounded = v.get("is_backgrounded").and_then(|b| b.as_bool()) == Some(true);
            // A subagent's own tool calls are its business; backgrounding the
            // subagent itself (when it runs in the foreground) is what helps.
            let subagent = v.get("owned_by_subagent").is_some_and(|o| !o.is_null() && o != false);
            Event::TaskStarted {
                task_id: s("task_id"),
                tool_use_id: s("tool_use_id"),
                foreground: !backgrounded && !subagent,
            }
        }
        (Some("system"), Some("task_updated")) => {
            let patch = &v["patch"];
            let released = patch.get("is_backgrounded").and_then(|b| b.as_bool()) == Some(true)
                || patch.get("status").and_then(|x| x.as_str()).is_some_and(|st| st != "running");
            if released {
                Event::TaskReleased { task_id: s("task_id") }
            } else {
                Event::Other
            }
        }
        (Some("system"), Some("task_notification")) => Event::TaskReleased { task_id: s("task_id") },
        (Some("user"), _) => {
            let ids: Vec<String> = v["message"]["content"]
                .as_array()
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                        .filter_map(|b| b.get("tool_use_id").and_then(|t| t.as_str()))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            if ids.is_empty() {
                Event::Other
            } else {
                Event::ToolResults(ids)
            }
        }
        (Some("result"), _) => Event::Result,
        (Some("assistant"), _) | (Some("system"), Some("api_retry")) => Event::Activity,
        _ => Event::Other,
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Foreground {
    task_id: String,
    tool_use_id: String,
    started: std::time::Instant,
}

/// A `background_tasks` request to send at `at`, if the task is still
/// running in the foreground by then.
#[derive(Debug, Clone, PartialEq)]
struct BgOrder {
    tool_use_id: String,
    at: std::time::Instant,
}

#[derive(Debug, Default)]
struct Turn {
    foreground: Vec<Foreground>,
    /// A user message is waiting for the next tool boundary: background the
    /// next foreground task that starts before that boundary.
    awaiting_boundary: bool,
    /// The first `result` has been seen and stdin closed.
    over: bool,
    /// The last thing the CLI reported was a `result`: it is only waiting for
    /// background tasks (with Fleet's unlimited wait ceiling, possibly for
    /// hours) and nothing would be lost by ending it.
    idle: bool,
}

impl Turn {
    fn order_for(f: &Foreground, now: std::time::Instant) -> BgOrder {
        BgOrder { tool_use_id: f.tool_use_id.clone(), at: (f.started + BACKGROUND_GRACE).max(now) }
    }

    /// A user message was just written to stdin.
    fn on_user_message(&mut self, now: std::time::Instant) -> Vec<BgOrder> {
        self.awaiting_boundary = true;
        self.foreground.iter().map(|f| Self::order_for(f, now)).collect()
    }

    /// Apply one stdout event. Returns the bg orders it makes due and whether
    /// stdin must be closed now.
    fn on_event(&mut self, ev: Event, now: std::time::Instant) -> (Vec<BgOrder>, bool) {
        match ev {
            Event::TaskStarted { task_id, tool_use_id, foreground: true } if !tool_use_id.is_empty() => {
                self.idle = false;
                let f = Foreground { task_id, tool_use_id, started: now };
                let orders = if self.awaiting_boundary { vec![Self::order_for(&f, now)] } else { vec![] };
                self.foreground.push(f);
                (orders, false)
            }
            Event::TaskReleased { task_id } => {
                self.foreground.retain(|f| f.task_id != task_id);
                (vec![], false)
            }
            Event::ToolResults(ids) => {
                self.idle = false;
                self.foreground.retain(|f| !ids.contains(&f.tool_use_id));
                self.awaiting_boundary = false;
                (vec![], false)
            }
            Event::Result if !self.over => {
                self.over = true;
                self.idle = true;
                self.foreground.clear();
                self.awaiting_boundary = false;
                (vec![], true)
            }
            Event::Result => {
                self.idle = true;
                (vec![], false)
            }
            Event::Activity => {
                self.idle = false;
                (vec![], false)
            }
            _ => (vec![], false),
        }
    }

    /// Whether an order made earlier still applies.
    fn still_due(&self, tool_use_id: &str) -> bool {
        !self.over && self.foreground.iter().any(|f| f.tool_use_id == tool_use_id)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Host process — runs as `<binary> fleet-claude-host`
// ─────────────────────────────────────────────────────────────────────────────

/// Entry point both binaries call when `argv[1] == HOST_ARGV_MARKER`.
/// Exits with claude's exit code; never returns.
pub fn host_main() -> ! {
    let code = match host::run() {
        Ok(code) => code,
        Err(e) => {
            // Before the handshake this tells the spawner to launch directly.
            println!("err {e}");
            eprintln!("[fleet-claude-host] {e}");
            70
        }
    };
    std::process::exit(code)
}

mod host {
    use super::*;
    use std::io::{BufRead, Write};
    use std::process::ChildStdin;
    use std::sync::{Arc, Mutex};

    struct Shared {
        turn: Turn,
        claude_pid: u32,
        stdin: Option<ChildStdin>,
        next_request: u64,
    }

    type State = Arc<Mutex<Shared>>;

    fn user_line(text: &str) -> String {
        let mut s = serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": text},
        })
        .to_string();
        s.push('\n');
        s
    }

    fn write_stdin(shared: &mut Shared, line: &str) -> Result<(), String> {
        let stdin = shared.stdin.as_mut().ok_or("turn over")?;
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|e| format!("write claude stdin: {e}"))
    }

    /// Carry out bg orders: each on its own short-lived timer thread, since
    /// they are rare (one per follow-up) and usually due immediately.
    fn dispatch(state: &State, orders: Vec<BgOrder>) {
        for order in orders {
            let state = state.clone();
            std::thread::spawn(move || {
                let wait = order.at.saturating_duration_since(std::time::Instant::now());
                if !wait.is_zero() {
                    std::thread::sleep(wait);
                }
                let mut shared = state.lock().unwrap_or_else(|e| e.into_inner());
                if !shared.turn.still_due(&order.tool_use_id) {
                    return;
                }
                shared.next_request += 1;
                let req = serde_json::json!({
                    "type": "control_request",
                    "request_id": format!("fleet-bg-{}", shared.next_request),
                    "request": {"subtype": "background_tasks", "tool_use_id": order.tool_use_id},
                });
                let res = write_stdin(&mut shared, &format!("{req}\n"));
                eprintln!(
                    "[fleet-claude-host] background_tasks {} -> {}",
                    order.tool_use_id,
                    res.err().unwrap_or_else(|| "sent".into())
                );
            });
        }
    }

    #[cfg(unix)]
    fn listen_socket(state: &State, sock: &Path) -> Result<(), String> {
        let _ = std::fs::remove_file(sock);
        let listener = std::os::unix::net::UnixListener::bind(sock).map_err(|e| e.to_string())?;
        let state = state.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(stream) = conn else { continue };
                let state = state.clone();
                std::thread::spawn(move || {
                    stream.set_read_timeout(Some(CLIENT_TIMEOUT)).ok();
                    let mut line = String::new();
                    if std::io::BufReader::new(&stream).read_line(&mut line).is_err() {
                        return;
                    }
                    let reply = match serde_json::from_str::<Request>(line.trim()) {
                        Ok(req) => handle(&state, &req),
                        Err(e) => Reply {
                            ok: false,
                            error: Some(format!("bad request: {e}")),
                            backgrounded: vec![],
                        },
                    };
                    let mut out = serde_json::to_string(&reply).unwrap_or_default();
                    out.push('\n');
                    let _ = (&stream).write_all(out.as_bytes());
                });
            }
        });
        Ok(())
    }

    #[cfg(not(unix))]
    fn listen_socket(_state: &State, _sock: &Path) -> Result<(), String> {
        Err("no unix sockets on this platform".into())
    }

    fn listen_file(state: &State, ctl: &Path) -> Result<(), String> {
        // Fresh and empty: the tail starts at offset 0.
        std::fs::File::create(ctl).map_err(|e| e.to_string())?;
        let state = state.clone();
        let ctl = ctl.to_path_buf();
        std::thread::spawn(move || serve_file(&ctl, |req| handle(&state, req)));
        Ok(())
    }

    fn handle(state: &State, req: &Request) -> Reply {
        let fail = |e: String| Reply { ok: false, error: Some(e), backgrounded: vec![] };
        if req.op == OP_END_IDLE {
            let shared = state.lock().unwrap_or_else(|e| e.into_inner());
            if !shared.turn.idle {
                return fail("turn is still working".into());
            }
            // Under the lock, so a wake-up cannot slip in between the check and
            // the signal. The tree, not just claude: its background commands are
            // what kept it alive.
            eprintln!("[fleet-claude-host] end_idle: terminating claude {}", shared.claude_pid);
            return match crate::session::kill_pid_tree(shared.claude_pid, false) {
                Ok(()) => Reply { ok: true, error: None, backgrounded: vec![] },
                Err(e) => fail(e),
            };
        }
        let from_user = match req.op.as_str() {
            OP_USER => true,
            OP_AGENT => false,
            _ => return fail("unknown op".into()),
        };
        if req.text.trim().is_empty() {
            return fail("empty text".into());
        }
        let orders = {
            let mut shared = state.lock().unwrap_or_else(|e| e.into_inner());
            if shared.turn.over {
                return fail("turn over".into());
            }
            if let Err(e) = write_stdin(&mut shared, &user_line(&req.text)) {
                return fail(e);
            }
            // An agent message waits for the next tool boundary like a socket
            // delivery does; only the user's words cut a running task short.
            if from_user {
                shared.turn.on_user_message(std::time::Instant::now())
            } else {
                Vec::new()
            }
        };
        let backgrounded = orders.iter().map(|o| o.tool_use_id.clone()).collect();
        eprintln!("[fleet-claude-host] {} message delivered, background {backgrounded:?}", req.op);
        dispatch(state, orders);
        Reply { ok: true, error: None, backgrounded }
    }

    pub(super) fn run() -> Result<i32, String> {
        let raw = std::env::var(SPEC_ENV).map_err(|_| format!("{SPEC_ENV} not set"))?;
        let spec: HostSpec = serde_json::from_str(&raw).map_err(|e| format!("bad host spec: {e}"))?;
        run_spec(&spec, use_file_transport())
    }

    pub(super) fn run_spec(spec: &HostSpec, file_transport: bool) -> Result<i32, String> {
        let mut cmd = crate::process_util::command(&spec.claude_path);
        cmd.args(&spec.args)
            .current_dir(&spec.cwd)
            .env_remove(SPEC_ENV)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());
        // Same as the direct launch. Unix: own process group, so a group
        // signal aimed at the host does not interrupt the turn. Windows:
        // CREATE_NO_WINDOW (process_util::command) gives claude a hidden
        // console of its own — the one `fleet win-interrupt` attaches to for
        // CTRL_C — so an interrupt reaches claude and its tools, not the host.
        crate::process_util::detach_process_group(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| format!("spawn claude failed: {e}"))?;
        drop(cmd);
        // The interrupt's CTRL_C goes to every process on claude's console.
        // That should not include the host, but if it ever does, the host
        // must outlive it to reap claude and clean up. Only after the spawn:
        // children created later inherit the ignore flag, and claude must not.
        #[cfg(windows)]
        unsafe {
            let _ = windows::Win32::System::Console::SetConsoleCtrlHandler(None, true);
        }
        let claude_pid = child.id();
        let mut stdin = child.stdin.take().ok_or("claude stdin missing")?;
        let stdout = child.stdout.take().ok_or("claude stdout missing")?;
        if let Err(e) = stdin
            .write_all(user_line(&spec.prompt).as_bytes())
            .and_then(|()| stdin.flush())
        {
            // Claude is running but has no prompt; it ends on stdin EOF. Do not
            // tell the spawner to fall back — that would start a second one.
            eprintln!("[fleet-claude-host] write prompt: {e}");
        }

        let sidecar_path = spec
            .sidecar_dir
            .as_ref()
            .map(|d| d.join(format!("pid-{claude_pid}.jsonl")));
        let mut sidecar = sidecar_path.as_ref().and_then(|p| std::fs::File::create(p).ok());

        let state: State = Arc::new(Mutex::new(Shared {
            turn: Turn::default(),
            claude_pid,
            stdin: Some(stdin),
            next_request: 0,
        }));

        // Control endpoint + registry entry. A failure here only costs the
        // fast path: clients find no host and fall back to the socket
        // injection / the queue.
        let my_pid = std::process::id();
        let sock = spec.registry_dir.join(format!("h{my_pid}.sock"));
        let ctl = spec.registry_dir.join(format!("h{my_pid}.ctl"));
        let record = record_path(&spec.registry_dir, &spec.session_id);
        let listening = std::fs::create_dir_all(&spec.registry_dir)
            .map_err(|e| e.to_string())
            .and_then(|()| {
                if file_transport {
                    listen_file(&state, &ctl)
                } else {
                    listen_socket(&state, &sock)
                }
            })
            .and_then(|()| {
                let rec = HostRecord {
                    session_id: spec.session_id.clone(),
                    host_pid: my_pid,
                    host_start_time: crate::session::process_start_time(my_pid),
                    claude_pid,
                    sock: if file_transport { PathBuf::new() } else { sock.clone() },
                    ctl: file_transport.then(|| ctl.clone()),
                };
                let tmp = record.with_extension("json.tmp");
                std::fs::write(&tmp, serde_json::to_vec(&rec).unwrap_or_default())
                    .and_then(|()| std::fs::rename(&tmp, &record))
                    .map_err(|e| e.to_string())
            });
        if let Err(e) = listening {
            eprintln!(
                "[fleet-claude-host] control {} unavailable: {e}",
                if file_transport { "file" } else { "socket" }
            );
        }

        // Handshake: the spawner records claude's pid as the session's.
        println!("pid {claude_pid}");
        let _ = std::io::stdout().flush();

        let mut reader = std::io::BufReader::new(stdout);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if let Some(f) = sidecar.as_mut() {
                if f.write_all(&buf).is_err() {
                    sidecar = None;
                }
            }
            let line = String::from_utf8_lossy(&buf);
            let ev = classify(line.trim_end());
            if ev == Event::Other {
                continue;
            }
            let orders = {
                let mut shared = state.lock().unwrap_or_else(|e| e.into_inner());
                let (orders, close) = shared.turn.on_event(ev, std::time::Instant::now());
                if close {
                    // From here on the CLI behaves like `-p <prompt>`: finish
                    // background work, answer anything already written, exit.
                    shared.stdin = None;
                }
                orders
            };
            dispatch(&state, orders);
        }

        state.lock().unwrap_or_else(|e| e.into_inner()).stdin = None;
        let status = child.wait().map_err(|e| format!("wait claude: {e}"))?;
        // Removing the control file also ends the thread tailing it.
        let _ = std::fs::remove_file(&sock);
        let _ = std::fs::remove_file(&ctl);
        // Answers nobody collected (their client timed out).
        let prefix = format!("h{my_pid}.");
        if let Ok(entries) = std::fs::read_dir(&spec.registry_dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with(&prefix) && (name.ends_with(".reply") || name.ends_with(".tmp")) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        // An incognito turn's streamed text does not outlive the turn. The
        // Stop hook removes it on a normal turn end; this covers a killed one.
        if let Some(p) = sidecar_path.filter(|_| crate::incognito::is_incognito(&spec.session_id)) {
            drop(sidecar);
            let _ = std::fs::remove_file(p);
        }
        // Only our own entry: a later turn's host may have replaced it.
        if let Ok(raw) = std::fs::read_to_string(&record) {
            if serde_json::from_str::<HostRecord>(&raw).is_ok_and(|r| r.host_pid == my_pid) {
                let _ = std::fs::remove_file(&record);
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            Ok(status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0)))
        }
        #[cfg(not(unix))]
        Ok(status.code().unwrap_or(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn rewrite_moves_the_prompt_to_stdin() {
        let args = s(&["-p", "do it", "--session-id", "abc", "--output-format", "stream-json", "--verbose"]);
        let (out, prompt) = rewrite_args(&args).unwrap();
        assert_eq!(prompt, "do it");
        assert_eq!(
            out,
            s(&["-p", "--session-id", "abc", "--output-format", "stream-json", "--verbose", "--input-format", "stream-json"])
        );
        // Resume shape.
        let args = s(&["--resume", "abc", "-p", "next", "--output-format", "stream-json"]);
        let (out, prompt) = rewrite_args(&args).unwrap();
        assert_eq!(prompt, "next");
        assert_eq!(out, s(&["--resume", "abc", "-p", "--output-format", "stream-json", "--input-format", "stream-json"]));
    }

    #[test]
    fn rewrite_declines_launches_it_cannot_drive() {
        for args in [
            s(&["-p", "x", "--output-format", "json"]),
            s(&["-p", "x"]),
            s(&["-p", "x", "--output-format", "stream-json", "--fork-session"]),
            s(&["-p", "x", "--output-format", "stream-json", "--input-format", "text"]),
            s(&["-p", "  ", "--output-format", "stream-json"]),
            s(&["--resume", "abc", "--output-format", "stream-json"]),
        ] {
            assert!(rewrite_args(&args).is_none(), "{args:?}");
        }
    }

    #[test]
    fn classify_reads_the_cli_event_shapes() {
        // Captured from CLI 2.1.310 (probe fb963505).
        let started = r#"{"type":"system","subtype":"task_started","task_id":"b1","tool_use_id":"toolu_1","is_backgrounded":false,"task_type":"local_bash","session_id":"s"}"#;
        assert_eq!(
            classify(started),
            Event::TaskStarted { task_id: "b1".into(), tool_use_id: "toolu_1".into(), foreground: true }
        );
        let bg = r#"{"type":"system","subtype":"task_started","task_id":"b2","tool_use_id":"toolu_2","is_backgrounded":true}"#;
        assert!(matches!(classify(bg), Event::TaskStarted { foreground: false, .. }));
        let sub = r#"{"type":"system","subtype":"task_started","task_id":"b3","tool_use_id":"toolu_3","is_backgrounded":false,"owned_by_subagent":"agent-1"}"#;
        assert!(matches!(classify(sub), Event::TaskStarted { foreground: false, .. }));
        let patched = r#"{"type":"system","subtype":"task_updated","task_id":"b1","patch":{"is_backgrounded":true}}"#;
        assert_eq!(classify(patched), Event::TaskReleased { task_id: "b1".into() });
        let done = r#"{"type":"system","subtype":"task_updated","task_id":"b1","patch":{"status":"completed","end_time":1}}"#;
        assert_eq!(classify(done), Event::TaskReleased { task_id: "b1".into() });
        let result_row = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"ok"}]}}"#;
        assert_eq!(classify(result_row), Event::ToolResults(vec!["toolu_1".into()]));
        assert_eq!(classify(r#"{"type":"result","subtype":"success"}"#), Event::Result);
        assert_eq!(classify(r#"{"type":"user","message":{"role":"user","content":"hi"}}"#), Event::Other);
        assert_eq!(classify("not json"), Event::Other);
    }

    fn started(id: &str) -> Event {
        Event::TaskStarted { task_id: format!("t-{id}"), tool_use_id: id.into(), foreground: true }
    }

    #[test]
    fn a_message_backgrounds_running_foreground_tasks_after_the_grace() {
        let t0 = Instant::now();
        let mut turn = Turn::default();
        turn.on_event(started("a"), t0);
        // Arrives 0.5s into the command: wait out the rest of the grace.
        let orders = turn.on_user_message(t0 + Duration::from_millis(500));
        assert_eq!(orders, vec![BgOrder { tool_use_id: "a".into(), at: t0 + BACKGROUND_GRACE }]);
        // Arrives 10s in: now.
        let late = t0 + Duration::from_secs(10);
        assert_eq!(turn.on_user_message(late)[0].at, late);
        assert!(turn.still_due("a"));
        turn.on_event(Event::TaskReleased { task_id: "t-a".into() }, late);
        assert!(!turn.still_due("a"));
    }

    #[test]
    fn a_message_with_nothing_running_catches_the_next_task_before_the_boundary() {
        let t0 = Instant::now();
        let mut turn = Turn::default();
        assert!(turn.on_user_message(t0).is_empty());
        let (orders, _) = turn.on_event(started("b"), t0);
        assert_eq!(orders, vec![BgOrder { tool_use_id: "b".into(), at: t0 + BACKGROUND_GRACE }]);
        // The boundary absorbs the message; later tasks run undisturbed.
        turn.on_event(Event::ToolResults(vec!["b".into()]), t0);
        assert!(!turn.still_due("b"));
        let (orders, _) = turn.on_event(started("c"), t0);
        assert!(orders.is_empty());
    }

    #[test]
    fn no_message_means_no_backgrounding() {
        let mut turn = Turn::default();
        let (orders, close) = turn.on_event(started("a"), Instant::now());
        assert!(orders.is_empty() && !close);
    }

    #[test]
    fn the_first_result_closes_stdin_once() {
        let t0 = Instant::now();
        let mut turn = Turn::default();
        turn.on_event(started("a"), t0);
        assert_eq!(turn.on_event(Event::Result, t0), (vec![], true));
        assert!(!turn.still_due("a"));
        assert_eq!(turn.on_event(Event::Result, t0), (vec![], false));
    }

    #[test]
    fn idle_from_a_result_until_the_model_works_again() {
        let t0 = Instant::now();
        let mut turn = Turn::default();
        assert!(!turn.idle, "a turn that has not finished is not idle");
        turn.on_event(Event::Activity, t0);
        turn.on_event(Event::Result, t0);
        assert!(turn.idle);
        // A background task finishing does not wake the model by itself.
        turn.on_event(Event::TaskReleased { task_id: "b".into() }, t0);
        assert!(turn.idle);
        // Its notification woke the agent for another round.
        turn.on_event(Event::Activity, t0);
        assert!(!turn.idle);
        turn.on_event(Event::Result, t0);
        assert!(turn.idle, "every result, not only the first, ends the work");
        turn.on_event(started("c"), t0);
        assert!(!turn.idle);
    }

    #[test]
    fn model_output_and_api_retries_are_activity() {
        assert_eq!(classify(r#"{"type":"stream_event","event":{}}"#), Event::Activity);
        assert_eq!(classify(r#"{"type":"assistant","message":{"content":[]}}"#), Event::Activity);
        assert_eq!(classify(r#"{"type":"system","subtype":"api_retry","attempt":2}"#), Event::Activity);
        assert_eq!(classify(r#"{"type":"system","subtype":"background_tasks_changed","tasks":[]}"#), Event::Other);
    }

    #[test]
    fn spec_round_trips() {
        let spec = HostSpec {
            claude_path: "claude".into(),
            args: s(&["-p"]),
            cwd: "/w".into(),
            prompt: "hi".into(),
            session_id: "abc".into(),
            sidecar_dir: None,
            registry_dir: "/r".into(),
        };
        let back: HostSpec = serde_json::from_str(&serde_json::to_string(&spec).unwrap()).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn a_record_from_before_the_file_transport_still_reads() {
        let old = r#"{"session_id":"s","host_pid":1,"host_start_time":2,"claude_pid":3,"sock":"/r/h1.sock"}"#;
        let rec: HostRecord = serde_json::from_str(old).unwrap();
        assert_eq!(rec.ctl, None);
        assert_eq!(rec.sock, PathBuf::from("/r/h1.sock"));
        let file_only = r#"{"session_id":"s","host_pid":1,"host_start_time":2,"claude_pid":3,"ctl":"/r/h1.ctl"}"#;
        let rec: HostRecord = serde_json::from_str(file_only).unwrap();
        assert_eq!(rec.ctl, Some(PathBuf::from("/r/h1.ctl")));
    }

    #[test]
    fn request_ids_are_unique_and_file_safe() {
        let a = new_request_id();
        let b = new_request_id();
        assert_ne!(a, b);
        assert!(valid_request_id(&a), "{a}");
        for bad in ["", "../x", "a/b", "a.b", &"x".repeat(65)] {
            assert!(!valid_request_id(bad), "{bad}");
        }
        assert_eq!(reply_path(Path::new("/r/h7.ctl"), "ab-1"), PathBuf::from("/r/h7.ab-1.reply"));
    }

    /// The Windows transport, exercised on every platform: requests go in as
    /// lines of the control file, answers come back through reply files, and
    /// removing the file stops the tail.
    #[test]
    fn control_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let ctl = dir.path().join("h1.ctl");
        std::fs::File::create(&ctl).unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let server = {
            let ctl = ctl.clone();
            let seen = seen.clone();
            std::thread::spawn(move || {
                serve_file(&ctl, |req| {
                    seen.lock().unwrap().push(req.text.clone());
                    Reply { ok: true, error: None, backgrounded: vec![format!("bg-{}", req.text)] }
                })
            })
        };

        // Two clients at once: each gets its own answer.
        let c1 = {
            let ctl = ctl.clone();
            std::thread::spawn(move || file_request(&ctl, OP_USER, "one"))
        };
        let r2 = file_request(&ctl, OP_USER, "two").unwrap();
        let r1 = c1.join().unwrap().unwrap();
        assert_eq!(r1.backgrounded, vec!["bg-one".to_string()]);
        assert_eq!(r2.backgrounded, vec!["bg-two".to_string()]);

        // A request the client may have given up on is refused, not handled.
        let stale = Request {
            op: "user".into(),
            text: "late".into(),
            id: Some("stale-1".into()),
            sent_ms: Some(now_ms() - 10_000),
        };
        let unsafe_id = Request { id: Some("../evil".into()), sent_ms: Some(now_ms()), ..stale.clone() };
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&ctl).unwrap();
            for r in [&unsafe_id, &stale] {
                writeln!(f, "{}", serde_json::to_string(r).unwrap()).unwrap();
            }
        }
        let stale_reply = reply_path(&ctl, "stale-1");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !stale_reply.exists() {
            assert!(std::time::Instant::now() < deadline, "no answer to the stale request");
            std::thread::sleep(Duration::from_millis(20));
        }
        let reply: Reply = serde_json::from_slice(&std::fs::read(&stale_reply).unwrap()).unwrap();
        assert_eq!(reply.error.as_deref(), Some("request expired"));
        let mut handled = seen.lock().unwrap().clone();
        handled.sort();
        assert_eq!(handled, vec!["one".to_string(), "two".to_string()]);
        assert!(!dir.path().join("evil.reply").exists());

        std::fs::remove_file(&ctl).unwrap();
        server.join().unwrap();
        assert!(file_request(&ctl, OP_USER, "gone").is_err(), "no control file, no host");
    }

    /// The whole host against a fake CLI: prompt in on stdin, a foreground
    /// task, a follow-up delivered as a user line plus a `background_tasks`
    /// request for that task, stdin closed at the first `result`.
    #[cfg(unix)]
    fn host_runs_a_turn(file_transport: bool) {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("stdin.log");
        let script = dir.path().join("fake-claude.sh");
        std::fs::write(
            &script,
            r#"#!/bin/sh
read -r prompt; echo "$prompt" > "$1"
echo '{"type":"system","subtype":"task_started","task_id":"b1","tool_use_id":"toolu_1","is_backgrounded":false}'
read -r msg; echo "$msg" >> "$1"
read -r ctl; echo "$ctl" >> "$1"
echo '{"type":"result","subtype":"success"}'
while read -r rest; do echo "$rest" >> "$1"; done
exit 3
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let registry = dir.path().join("hosts");
        let sidecars = dir.path().join("sidecars");
        std::fs::create_dir_all(&sidecars).unwrap();
        let spec = HostSpec {
            claude_path: script.to_string_lossy().into_owned(),
            args: vec![log.to_string_lossy().into_owned()],
            cwd: dir.path().to_string_lossy().into_owned(),
            prompt: "opening".into(),
            session_id: "sess-1".into(),
            sidecar_dir: Some(sidecars.clone()),
            registry_dir: registry.clone(),
        };
        let host = std::thread::spawn(move || host::run_spec(&spec, file_transport));

        let record = record_path(&registry, "sess-1");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !record.exists() {
            assert!(std::time::Instant::now() < deadline, "host never registered");
            std::thread::sleep(Duration::from_millis(20));
        }
        let rec: HostRecord = serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
        assert_eq!(rec.ctl.is_some(), file_transport);
        let reply = send_in(&registry, "sess-1", OP_USER, "stop that").unwrap();
        assert!(reply.ok);

        assert_eq!(host.join().unwrap().unwrap(), 3, "host exits with claude's code");
        let lines: Vec<serde_json::Value> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(lines[0]["message"]["content"], "opening");
        assert_eq!(lines[1]["type"], "user");
        assert_eq!(lines[1]["message"]["content"], "stop that");
        assert_eq!(lines[2]["request"]["subtype"], "background_tasks");
        assert_eq!(lines[2]["request"]["tool_use_id"], "toolu_1");

        // Cleaned up: no record, no endpoint; the turn is over for clients.
        assert!(!record.exists());
        let left: Vec<_> = std::fs::read_dir(&registry).unwrap().flatten().map(|e| e.file_name()).collect();
        assert!(left.is_empty(), "{left:?}");
        assert!(send_in(&registry, "sess-1", OP_USER, "late").is_err());
        // The sidecar carries the CLI's stdout.
        let side = std::fs::read_dir(&sidecars).unwrap().flatten().next().unwrap().path();
        assert!(std::fs::read_to_string(side).unwrap().contains("task_started"));
    }

    #[cfg(unix)]
    #[test]
    fn host_runs_a_turn_over_the_socket() {
        host_runs_a_turn(false);
    }

    #[cfg(unix)]
    #[test]
    fn host_runs_a_turn_over_the_control_file() {
        host_runs_a_turn(true);
    }

    /// The Windows agent path, over the control file: the message reaches
    /// stdin in Fleet's peer frame and no `background_tasks` follows, even
    /// with a foreground task running well past the grace window.
    #[cfg(unix)]
    #[test]
    fn an_agent_message_is_framed_and_backgrounds_nothing() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("stdin.log");
        let script = dir.path().join("fake-claude.sh");
        std::fs::write(
            &script,
            r#"#!/bin/sh
read -r prompt; echo "$prompt" > "$1"
echo '{"type":"system","subtype":"task_started","task_id":"b1","tool_use_id":"toolu_1","is_backgrounded":false}'
read -r msg; printf '%s\n' "$msg" >> "$1"
sleep 3
echo '{"type":"result","subtype":"success"}'
while read -r rest; do printf '%s\n' "$rest" >> "$1"; done
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let registry = dir.path().join("hosts");
        let spec = HostSpec {
            claude_path: script.to_string_lossy().into_owned(),
            args: vec![log.to_string_lossy().into_owned()],
            cwd: dir.path().to_string_lossy().into_owned(),
            prompt: "opening".into(),
            session_id: "sess-a".into(),
            sidecar_dir: None,
            registry_dir: registry.clone(),
        };
        let host = std::thread::spawn(move || host::run_spec(&spec, true));
        let record = record_path(&registry, "sess-a");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !record.exists() {
            assert!(std::time::Instant::now() < deadline, "host never registered");
            std::thread::sleep(Duration::from_millis(20));
        }
        // Let the task_started line reach the host before the message.
        std::thread::sleep(Duration::from_millis(300));
        let framed = crate::live_inject::frame_as_agent("watch fired");
        let reply = send_in(&registry, "sess-a", OP_AGENT, &framed).unwrap();
        assert!(reply.ok && reply.backgrounded.is_empty(), "{reply:?}");

        assert_eq!(host.join().unwrap().unwrap(), 0);
        let lines: Vec<serde_json::Value> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2, "nothing but the prompt and the message: {lines:?}");
        assert_eq!(lines[1]["type"], "user");
        let text = lines[1]["message"]["content"].as_str().unwrap();
        assert_eq!(crate::live_inject::strip_agent_frame(text), Some("watch fired"));
    }
}
