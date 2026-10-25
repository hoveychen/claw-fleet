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
//! `setsid`-detached so quitting the spawner does not end the turn. The host
//! also takes over the live-thinking sidecar: it copies the CLI's stdout into
//! `live-thinking/pid-<claude pid>.jsonl` line by line, the format the reader
//! already understands.
//!
//! Unix only. On Windows the spawn path stays the direct `claude -p` launch —
//! there is no live socket injection there either, so nothing regresses.

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

/// The host's entry in [`registry_dir`]: how a client finds the socket of the
/// turn that is running right now.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct HostRecord {
    pub session_id: String,
    pub host_pid: u32,
    pub host_start_time: Option<u64>,
    pub claude_pid: u32,
    pub sock: PathBuf,
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
#[cfg(unix)]
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

#[cfg(not(unix))]
pub(crate) fn plan(
    _claude_path: &str,
    _args: &[String],
    _cwd: &str,
    _session_id: Option<String>,
) -> Option<(PathBuf, HostSpec)> {
    None
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
#[cfg(unix)]
pub(crate) fn spawn(
    exe: &Path,
    spec: &HostSpec,
    template: &std::process::Command,
    stderr_log: &Path,
) -> Result<Spawned, String> {
    use std::io::BufRead;
    use std::os::unix::process::CommandExt;

    let spec_json = serde_json::to_string(spec).map_err(|e| format!("encode host spec: {e}"))?;
    let stderr_file = std::fs::OpenOptions::new()
        .append(true)
        .open(stderr_log)
        .map_err(|e| format!("reopen stderr log {}: {e}", stderr_log.display()))?;
    let mut cmd = std::process::Command::new(exe);
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
    // leader, and setsid on an existing group leader fails.
    unsafe {
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

#[cfg(not(unix))]
pub(crate) fn spawn(
    _exe: &Path,
    _spec: &HostSpec,
    _template: &std::process::Command,
    _stderr_log: &Path,
) -> Result<Spawned, String> {
    Ok(Spawned::Fallback("claude host is unix-only".into()))
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

/// Write `text` into `session_id`'s running turn as the user's own message.
///
/// `Err` whenever there is no live host for the session or its turn is over;
/// the caller falls back to the socket injection / the queue.
#[cfg(unix)]
pub fn send_user_message(session_id: &str, text: &str) -> Result<Reply, String> {
    use std::io::{BufRead, Write};

    let dir = registry_dir().ok_or("no fleet dir")?;
    let raw = std::fs::read_to_string(record_path(&dir, session_id))
        .map_err(|_| "no claude host for this session".to_string())?;
    let rec: HostRecord = serde_json::from_str(&raw).map_err(|e| format!("host record: {e}"))?;
    if crate::session::process_start_time(rec.host_pid) != rec.host_start_time
        || rec.host_start_time.is_none()
    {
        return Err("claude host is gone".into());
    }
    let stream = std::os::unix::net::UnixStream::connect(&rec.sock)
        .map_err(|e| format!("connect claude host: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
    let mut req = serde_json::to_string(&serde_json::json!({"op": "user", "text": text}))
        .map_err(|e| e.to_string())?;
    req.push('\n');
    (&stream)
        .write_all(req.as_bytes())
        .map_err(|e| format!("write claude host: {e}"))?;
    let mut line = String::new();
    std::io::BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|e| format!("read claude host: {e}"))?;
    let reply: Reply =
        serde_json::from_str(line.trim()).map_err(|e| format!("claude host reply: {e}"))?;
    if reply.ok {
        Ok(reply)
    } else {
        Err(reply.error.unwrap_or_else(|| "claude host refused".into()))
    }
}

#[cfg(not(unix))]
pub fn send_user_message(_session_id: &str, _text: &str) -> Result<Reply, String> {
    Err("claude host is unix-only".into())
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
    Other,
}

fn classify(line: &str) -> Event {
    // The bulk of the stream is token deltas; skip them without parsing.
    if line.starts_with(r#"{"type":"stream_event""#) {
        return Event::Other;
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
                self.foreground.retain(|f| !ids.contains(&f.tool_use_id));
                self.awaiting_boundary = false;
                (vec![], false)
            }
            Event::Result if !self.over => {
                self.over = true;
                self.foreground.clear();
                self.awaiting_boundary = false;
                (vec![], true)
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
    #[cfg(unix)]
    let code = match host::run() {
        Ok(code) => code,
        Err(e) => {
            // Before the handshake this tells the spawner to launch directly.
            println!("err {e}");
            eprintln!("[fleet-claude-host] {e}");
            70
        }
    };
    #[cfg(not(unix))]
    let code = {
        println!("err claude host is unix-only");
        70
    };
    std::process::exit(code)
}

#[cfg(unix)]
mod host {
    use super::*;
    use std::io::{BufRead, Write};
    use std::os::unix::process::CommandExt;
    use std::process::ChildStdin;
    use std::sync::{Arc, Mutex};

    struct Shared {
        turn: Turn,
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

    fn serve(state: State, listener: std::os::unix::net::UnixListener) {
        for conn in listener.incoming() {
            let Ok(stream) = conn else { continue };
            let state = state.clone();
            std::thread::spawn(move || {
                stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
                let mut line = String::new();
                if std::io::BufReader::new(&stream).read_line(&mut line).is_err() {
                    return;
                }
                let reply = handle(&state, line.trim());
                let mut out = serde_json::to_string(&reply).unwrap_or_default();
                out.push('\n');
                let _ = (&stream).write_all(out.as_bytes());
            });
        }
    }

    fn handle(state: &State, request: &str) -> Reply {
        let fail = |e: String| Reply { ok: false, error: Some(e), backgrounded: vec![] };
        let req: serde_json::Value = match serde_json::from_str(request) {
            Ok(v) => v,
            Err(e) => return fail(format!("bad request: {e}")),
        };
        if req.get("op").and_then(|o| o.as_str()) != Some("user") {
            return fail("unknown op".into());
        }
        let Some(text) = req.get("text").and_then(|t| t.as_str()).filter(|t| !t.trim().is_empty())
        else {
            return fail("empty text".into());
        };
        let orders = {
            let mut shared = state.lock().unwrap_or_else(|e| e.into_inner());
            if shared.turn.over {
                return fail("turn over".into());
            }
            if let Err(e) = write_stdin(&mut shared, &user_line(text)) {
                return fail(e);
            }
            shared.turn.on_user_message(std::time::Instant::now())
        };
        let backgrounded = orders.iter().map(|o| o.tool_use_id.clone()).collect();
        eprintln!("[fleet-claude-host] user message delivered, background {backgrounded:?}");
        dispatch(state, orders);
        Reply { ok: true, error: None, backgrounded }
    }

    pub(super) fn run() -> Result<i32, String> {
        let raw = std::env::var(SPEC_ENV).map_err(|_| format!("{SPEC_ENV} not set"))?;
        let spec: HostSpec = serde_json::from_str(&raw).map_err(|e| format!("bad host spec: {e}"))?;

        let mut cmd = crate::process_util::command(&spec.claude_path);
        cmd.args(&spec.args)
            .current_dir(&spec.cwd)
            .env_remove(SPEC_ENV)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            // Same as the direct launch: a group signal aimed at the host
            // does not interrupt the turn.
            .process_group(0);
        let mut child = cmd.spawn().map_err(|e| format!("spawn claude failed: {e}"))?;
        drop(cmd);
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
            stdin: Some(stdin),
            next_request: 0,
        }));

        // Control socket + registry entry. A failure here only costs the fast
        // path: clients find no host and fall back to the socket injection.
        let my_pid = std::process::id();
        let sock = spec.registry_dir.join(format!("h{my_pid}.sock"));
        let record = record_path(&spec.registry_dir, &spec.session_id);
        let listening = std::fs::create_dir_all(&spec.registry_dir)
            .map_err(|e| e.to_string())
            .and_then(|()| {
                let _ = std::fs::remove_file(&sock);
                std::os::unix::net::UnixListener::bind(&sock).map_err(|e| e.to_string())
            })
            .and_then(|listener| {
                let rec = HostRecord {
                    session_id: spec.session_id.clone(),
                    host_pid: my_pid,
                    host_start_time: crate::session::process_start_time(my_pid),
                    claude_pid,
                    sock: sock.clone(),
                };
                let tmp = record.with_extension("json.tmp");
                std::fs::write(&tmp, serde_json::to_vec(&rec).unwrap_or_default())
                    .and_then(|()| std::fs::rename(&tmp, &record))
                    .map_err(|e| e.to_string())?;
                Ok(listener)
            });
        match listening {
            Ok(listener) => {
                let state = state.clone();
                std::thread::spawn(move || serve(state, listener));
            }
            Err(e) => eprintln!("[fleet-claude-host] control socket unavailable: {e}"),
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
        let _ = std::fs::remove_file(&sock);
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
        use std::os::unix::process::ExitStatusExt;
        Ok(status.code().unwrap_or_else(|| 128 + status.signal().unwrap_or(0)))
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
        assert_eq!(classify(r#"{"type":"stream_event","event":{}}"#), Event::Other);
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
}
