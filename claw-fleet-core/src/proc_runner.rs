//! Workspace command runner — user commands launched from the 文件 page run
//! under a detached per-command **host process** (`<binary> fleet-proc-host
//! <id>`) instead of inside the desktop app / probe server:
//!
//! * the host owns the pty master, so full-terminal semantics (ansi, raw
//!   mode, interactive stdin) work through xterm.js on the frontend;
//! * the host is `setsid`-detached from its spawner, so commands survive the
//!   desktop app quitting — "runs under a daemon" — and any Fleet process can
//!   re-adopt them later purely through the on-disk registry;
//! * the command itself is spawned as a **session leader on the pty slave**,
//!   so its whole process group can be killed with one `killpg` — no orphan
//!   dev-server children left behind (contrast `session::kill_pid_tree`,
//!   which has to reconstruct the tree from `ps`).
//!
//! Registry layout (flat files, like `task_progress`):
//!   `~/.fleet/procs/<id>.json`  — [`ProcRecord`] metadata (host rewrites it)
//!   `~/.fleet/procs/<id>.out`   — raw pty output bytes (ansi preserved)
//!   `~/.fleet/procs/<id>.sock`  — unix control socket (stdin / resize)
//!   `~/.fleet/procs/<id>.err`   — host's own stderr (diagnostics only)
//!
//! The spawner writes the initial `starting` record, then execs the host with
//! just the id; from that point the **host is the only writer** of the meta
//! file (avoids two-writer races). `list_procs` self-heals records whose host
//! died without writing an exit (kill -9, crash) using pid + start-time
//! identity, mirroring the pid-reuse defence in `permissions_injector`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// Marker first-arg both binaries (desktop app + fleet CLI) intercept before
/// their normal argv parsing to run [`host_main`] instead.
pub const HOST_ARGV_MARKER: &str = "fleet-proc-host";

/// How much output a fresh terminal attach replays (and the max chunk per
/// output poll).
const OUTPUT_CHUNK: u64 = 256 * 1024;

/// A `starting` record whose host never reported in after this long is
/// considered dead (host crashed before writing the `running` meta).
const STARTING_TIMEOUT_MS: u64 = 15_000;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum ProcStatus {
    Starting,
    Running,
    Exited,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ProcRecord {
    pub id: String,
    pub workspace_path: String,
    pub command: String,
    pub status: ProcStatus,
    /// Pid of the command's shell — also its pgid (it is a session leader).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub child_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub host_pid: Option<u32>,
    /// Start time of the host process — pid-reuse guard for self-healing.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub host_start_time: Option<u64>,
    /// `None` on an exited record means the exit was inferred (host died
    /// without reporting), not observed.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub exit_code: Option<i32>,
    pub started_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub finished_ms: Option<u64>,
    pub cols: u16,
    pub rows: u16,
}

/// One incremental read of a proc's pty output. `record` piggybacks the
/// current metadata so the terminal view doesn't need a second poll to learn
/// the proc exited.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ProcOutputChunk {
    pub data_b64: String,
    pub next_offset: u64,
    pub record: ProcRecord,
}

/// Body of `POST /proc_run`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SpawnProcRequest {
    pub workspace_path: String,
    pub command: String,
    pub cols: u16,
    pub rows: u16,
}

/// Body of `POST /proc_input`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProcInputRequest {
    pub id: String,
    pub data_b64: String,
}

/// Body of `POST /proc_resize`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProcResizeRequest {
    pub id: String,
    pub cols: u16,
    pub rows: u16,
}

/// Body of `POST /proc_clear`. `id` clears one exited proc; otherwise every
/// exited proc (optionally scoped to `workspace_path`) is cleared.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ClearProcRequest {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub workspace_path: Option<String>,
}

/// Control message sent over the per-proc unix socket (JSON lines).
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "op", rename_all = "lowercase")]
enum ControlMsg {
    Stdin { data_b64: String },
    Resize { cols: u16, rows: u16 },
}

pub fn procs_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("procs"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn meta_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}
fn out_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.out"))
}
fn sock_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.sock"))
}
fn err_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.err"))
}

/// Ids embed spawn time + spawner pid + an in-process counter — unique across
/// concurrent spawners without a rand dependency. Kept short: the id is part
/// of the control-socket path, and `sockaddr_un` caps paths at ~104 bytes.
fn gen_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!(
        "p{:x}{:x}{:x}",
        now_ms(),
        std::process::id() & 0xfff,
        seq & 0xff
    )
}

fn read_record(dir: &Path, id: &str) -> Result<ProcRecord, String> {
    let raw = fs::read_to_string(meta_path(dir, id))
        .map_err(|e| format!("no such proc {id}: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("corrupt proc meta {id}: {e}"))
}

fn write_record(dir: &Path, rec: &ProcRecord) -> Result<(), String> {
    let json = serde_json::to_string_pretty(rec).map_err(|e| e.to_string())?;
    let path = meta_path(dir, &rec.id);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn require_dir() -> Result<PathBuf, String> {
    let dir = procs_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    Ok(dir)
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API — spawn / list / kill / io (callers: LocalBackend + hooks_server)
// ─────────────────────────────────────────────────────────────────────────────

/// Spawn `command` at `workspace_path`'s cwd under a detached host process.
/// `host_exe` is the binary to re-exec with the [`HOST_ARGV_MARKER`] argv —
/// callers pass their own `std::env::current_exe()` (both the desktop app and
/// the fleet CLI intercept the marker), so no PATH lookup is involved.
pub fn spawn_proc(
    host_exe: &Path,
    workspace_path: &str,
    command: &str,
    cols: u16,
    rows: u16,
) -> Result<ProcRecord, String> {
    let dir = require_dir()?;
    spawn_proc_in(&dir, host_exe, workspace_path, command, cols, rows)
}

pub fn spawn_proc_in(
    dir: &Path,
    host_exe: &Path,
    workspace_path: &str,
    command: &str,
    cols: u16,
    rows: u16,
) -> Result<ProcRecord, String> {
    if command.trim().is_empty() {
        return Err("command is empty".into());
    }
    if !Path::new(workspace_path).is_dir() {
        return Err(format!("workspace path does not exist: {workspace_path}"));
    }
    #[cfg(not(unix))]
    {
        let _ = (dir, host_exe, cols, rows);
        Err("workspace commands are not supported on this platform yet".into())
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        let rec = ProcRecord {
            id: gen_id(),
            workspace_path: workspace_path.to_string(),
            command: command.to_string(),
            status: ProcStatus::Starting,
            child_pid: None,
            host_pid: None,
            host_start_time: None,
            exit_code: None,
            started_ms: now_ms(),
            finished_ms: None,
            cols,
            rows,
        };
        write_record(dir, &rec)?;

        let err_file = fs::File::create(err_path(dir, &rec.id))
            .map_err(|e| format!("create host err log: {e}"))?;
        let mut cmd = std::process::Command::new(host_exe);
        cmd.arg(HOST_ARGV_MARKER)
            .arg(&rec.id)
            .env("FLEET_PROCS_DIR", dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(err_file);
        // Detach the host into its own session: killing / quitting the
        // spawner (desktop app, fleet serve) must not take the host down.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawn proc host {}: {e}", host_exe.display()))?;
        // Reap the direct child so it doesn't linger as a zombie; the host
        // keeps running detached regardless.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(rec)
    }
}

/// All known procs, newest first, after self-healing stale `running` /
/// `starting` records whose host died without reporting an exit.
pub fn list_procs() -> Vec<ProcRecord> {
    let Some(dir) = procs_dir() else {
        return Vec::new();
    };
    list_procs_in(&dir)
}

pub fn list_procs_in(dir: &Path) -> Vec<ProcRecord> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(String::from) else {
            continue;
        };
        let Ok(mut rec) = read_record(dir, &id) else {
            continue;
        };
        if heal_stale(dir, &mut rec) {
            let _ = write_record(dir, &rec);
        }
        out.push(rec);
    }
    out.sort_by(|a, b| b.started_ms.cmp(&a.started_ms));
    out
}

/// Returns true (and mutates `rec` to `exited`) when the record claims to be
/// live but its host process is provably gone.
fn heal_stale(dir: &Path, rec: &mut ProcRecord) -> bool {
    let host_alive = match (rec.host_pid, rec.host_start_time) {
        (Some(pid), start) => {
            crate::session::is_process_alive(pid)
                && (start.is_none() || crate::session::process_start_time(pid) == start)
        }
        (None, _) => false,
    };
    let stale = match rec.status {
        ProcStatus::Exited => false,
        ProcStatus::Running => !host_alive,
        ProcStatus::Starting => {
            !host_alive && now_ms().saturating_sub(rec.started_ms) > STARTING_TIMEOUT_MS
        }
    };
    if !stale {
        return false;
    }
    // Host died without recording the exit — the command group may still be
    // alive (orphaned); leave it to an explicit kill, but stop reporting the
    // proc as running.
    rec.status = ProcStatus::Exited;
    rec.finished_ms = Some(now_ms());
    let _ = fs::remove_file(sock_path(dir, &rec.id));
    true
}

/// Kill the command's whole process group (`killpg`), SIGTERM first with a 2s
/// SIGKILL escalation, or SIGKILL straight away when `force`.
pub fn kill_proc(id: &str, force: bool) -> Result<(), String> {
    let dir = require_dir()?;
    kill_proc_in(&dir, id, force)
}

pub fn kill_proc_in(dir: &Path, id: &str, force: bool) -> Result<(), String> {
    let rec = read_record(dir, id)?;
    if rec.status == ProcStatus::Exited {
        return Ok(());
    }
    let Some(pgid) = rec.child_pid else {
        return Err("proc is still starting; try again in a moment".into());
    };
    #[cfg(unix)]
    {
        let target = -(pgid as libc::pid_t);
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        let ret = unsafe { libc::kill(target, signal) };
        if ret != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            // Group already gone; list_procs will self-heal the record.
            return Ok(());
        }
        if !force {
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(2000));
                if unsafe { libc::kill(target, 0) } == 0 {
                    unsafe { libc::kill(target, libc::SIGKILL) };
                }
            });
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = pgid;
        Err("workspace commands are not supported on this platform yet".into())
    }
}

/// Incremental output read. `offset = None` tails: the first attach replays
/// at most the last [`OUTPUT_CHUNK`] bytes instead of the whole log.
pub fn proc_output(id: &str, offset: Option<u64>) -> Result<ProcOutputChunk, String> {
    let dir = require_dir()?;
    proc_output_in(&dir, id, offset)
}

pub fn proc_output_in(dir: &Path, id: &str, offset: Option<u64>) -> Result<ProcOutputChunk, String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut record = read_record(dir, id)?;
    if heal_stale(dir, &mut record) {
        let _ = write_record(dir, &record);
    }
    let path = out_path(dir, id);
    let (data, next_offset) = match fs::File::open(&path) {
        Ok(mut f) => {
            let len = f.metadata().map_err(|e| e.to_string())?.len();
            let mut start = offset.unwrap_or_else(|| len.saturating_sub(OUTPUT_CHUNK));
            if start > len {
                // Log was truncated/replaced behind our back — restart.
                start = 0;
            }
            let take = (len - start).min(OUTPUT_CHUNK);
            let mut buf = vec![0u8; take as usize];
            f.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
            f.read_exact(&mut buf).map_err(|e| e.to_string())?;
            (buf, start + take)
        }
        // Host hasn't created the file yet.
        Err(_) => (Vec::new(), offset.unwrap_or(0)),
    };
    Ok(ProcOutputChunk {
        data_b64: base64::engine::general_purpose::STANDARD.encode(&data),
        next_offset,
        record,
    })
}

/// Forward raw stdin bytes (base64) to the command's pty.
pub fn proc_input(id: &str, data_b64: &str) -> Result<(), String> {
    let dir = require_dir()?;
    send_control(
        &dir,
        id,
        &ControlMsg::Stdin {
            data_b64: data_b64.to_string(),
        },
    )
}

/// Resize the command's pty.
pub fn proc_resize(id: &str, cols: u16, rows: u16) -> Result<(), String> {
    let dir = require_dir()?;
    send_control(&dir, id, &ControlMsg::Resize { cols, rows })
}

fn send_control(dir: &Path, id: &str, msg: &ControlMsg) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::net::UnixStream;

        let rec = read_record(dir, id)?;
        if rec.status == ProcStatus::Exited {
            return Err("proc has exited".into());
        }
        let mut stream = UnixStream::connect(sock_path(dir, id))
            .map_err(|e| format!("proc control socket unavailable: {e}"))?;
        let mut line = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        line.push('\n');
        stream
            .write_all(line.as_bytes())
            .map_err(|e| format!("write control message: {e}"))
    }
    #[cfg(not(unix))]
    {
        let _ = (dir, id, msg);
        Err("workspace commands are not supported on this platform yet".into())
    }
}

/// Remove one exited proc's registry files. Refuses while running.
pub fn clear_proc(id: &str) -> Result<(), String> {
    let dir = require_dir()?;
    clear_proc_in(&dir, id)
}

pub fn clear_proc_in(dir: &Path, id: &str) -> Result<(), String> {
    let mut rec = read_record(dir, id)?;
    let _ = heal_stale(dir, &mut rec);
    if rec.status != ProcStatus::Exited {
        return Err("proc is still running; kill it first".into());
    }
    for p in [
        meta_path(dir, id),
        out_path(dir, id),
        sock_path(dir, id),
        err_path(dir, id),
    ] {
        let _ = fs::remove_file(p);
    }
    Ok(())
}

/// Clear every exited proc, optionally scoped to one workspace. Returns how
/// many were cleared.
pub fn clear_finished_procs(workspace_path: Option<&str>) -> Result<u32, String> {
    let dir = require_dir()?;
    let mut cleared = 0;
    for rec in list_procs_in(&dir) {
        if rec.status != ProcStatus::Exited {
            continue;
        }
        if let Some(ws) = workspace_path {
            if rec.workspace_path != ws {
                continue;
            }
        }
        if clear_proc_in(&dir, &rec.id).is_ok() {
            cleared += 1;
        }
    }
    Ok(cleared)
}

// ─────────────────────────────────────────────────────────────────────────────
// Host process — runs as `<binary> fleet-proc-host <id>`
// ─────────────────────────────────────────────────────────────────────────────

/// Entry point both binaries call when argv[1] == [`HOST_ARGV_MARKER`].
/// Never returns.
pub fn host_main(id: &str) -> ! {
    let dir = std::env::var_os("FLEET_PROCS_DIR")
        .map(PathBuf::from)
        .or_else(procs_dir);
    let code = match dir {
        Some(dir) => match run_host(&dir, id) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("fleet-proc-host {id}: {e}");
                mark_host_failed(&dir, id, &e);
                70 // EX_SOFTWARE
            }
        },
        None => {
            eprintln!("fleet-proc-host {id}: cannot determine procs dir");
            70
        }
    };
    std::process::exit(code)
}

/// Best-effort: record the host-side failure so the UI shows an exited proc
/// instead of a forever-`starting` ghost.
fn mark_host_failed(dir: &Path, id: &str, _err: &str) {
    if let Ok(mut rec) = read_record(dir, id) {
        rec.status = ProcStatus::Exited;
        rec.finished_ms = Some(now_ms());
        let _ = write_record(dir, &rec);
    }
    let _ = fs::remove_file(sock_path(dir, id));
}

#[cfg(not(unix))]
fn run_host(_dir: &Path, _id: &str) -> Result<i32, String> {
    Err("workspace commands are not supported on this platform yet".into())
}

/// The host loop: open a pty, spawn the command as a session leader on the
/// slave side, pump master → `<id>.out`, serve stdin/resize over the control
/// socket, and record the exit code when the command finishes.
#[cfg(unix)]
fn run_host(dir: &Path, id: &str) -> Result<i32, String> {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;

    let mut rec = read_record(dir, id)?;

    // 1. pty pair, sized to the requesting terminal.
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let mut winsize = libc::winsize {
        ws_row: rec.rows.max(2),
        ws_col: rec.cols.max(2),
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let ret = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut winsize,
        )
    };
    if ret != 0 {
        return Err(format!("openpty: {}", std::io::Error::last_os_error()));
    }
    // Owned wrappers so the fds close on every exit path.
    let master_fd: OwnedFd = unsafe { OwnedFd::from_raw_fd(master) };
    let slave_fd: OwnedFd = unsafe { OwnedFd::from_raw_fd(slave) };
    // A test process can run several hosts concurrently. Without CLOEXEC, a
    // command spawned by one host inherits every other host's pty fds, keeps
    // their slave sides open, and prevents their master reads from ever
    // observing EOF. Production also benefits from not leaking unrelated pty
    // descriptors into commands.
    for fd in [&master_fd, &slave_fd] {
        let raw = fd.as_raw_fd();
        let flags = unsafe { libc::fcntl(raw, libc::F_GETFD) };
        if flags < 0 || unsafe { libc::fcntl(raw, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
            return Err(format!(
                "set pty close-on-exec: {}",
                std::io::Error::last_os_error()
            ));
        }
    }

    // 2. Spawn `$SHELL -lc <command>` at the workspace cwd, as a session
    //    leader with the pty slave as its controlling terminal. Login shell
    //    (-l) so the user's PATH/profile applies even when the spawner was a
    //    GUI app with a minimal environment.
    let shell = std::env::var("SHELL").unwrap_or_else(|_| {
        if Path::new("/bin/zsh").exists() {
            "/bin/zsh".into()
        } else {
            "/bin/sh".into()
        }
    });
    let stdio = |fd: &OwnedFd| -> Result<std::process::Stdio, String> {
        // Keep the duplicate close-on-exec as well. Command's child setup
        // moves it onto fd 0/1/2, which clears CLOEXEC for the intended stdio,
        // while concurrent commands cannot inherit the temporary duplicate.
        let dup = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if dup < 0 {
            return Err(format!(
                "dup pty slave: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(unsafe { std::process::Stdio::from_raw_fd(dup) })
    };
    let mut cmd = std::process::Command::new(&shell);
    cmd.arg("-lc")
        .arg(&rec.command)
        .current_dir(&rec.workspace_path)
        .env("TERM", "xterm-256color")
        .stdin(stdio(&slave_fd)?)
        .stdout(stdio(&slave_fd)?)
        .stderr(stdio(&slave_fd)?);
    unsafe {
        cmd.pre_exec(|| {
            // New session + controlling tty: makes the command (and every
            // descendant that doesn't detach itself) one killable group.
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    // 3. Control socket: stdin + resize, JSON lines per connection. Bound
    //    BEFORE the command spawns — a bind failure must not leave an
    //    unmanaged command behind.
    let sock = sock_path(dir, id);
    let _ = fs::remove_file(&sock);
    let listener = std::os::unix::net::UnixListener::bind(&sock)
        .map_err(|e| format!("bind {}: {e}", sock.display()))?;

    let mut child = cmd.spawn().map_err(|e| format!("spawn {shell}: {e}"))?;
    drop(slave_fd);
    // `Command::spawn` borrows `&mut cmd` and does NOT consume it, so `cmd`
    // keeps its three configured Stdio handles — the parent-side dups of the
    // pty slave — open until it drops at end of scope. On Linux the master
    // read at step 5 only reports EIO once *every* slave fd is closed, so
    // those retained dups would keep this pump loop blocked forever after the
    // command exits (a 6h CI hang). macOS reports EOF as soon as the child's
    // slave fds close, which masked the leak. Drop `cmd` now to release them.
    drop(cmd);

    // 4. Report in: from here the host owns the meta file.
    let host_pid = std::process::id();
    rec.status = ProcStatus::Running;
    rec.child_pid = Some(child.id());
    rec.host_pid = Some(host_pid);
    rec.host_start_time = crate::session::process_start_time(host_pid);
    write_record(dir, &rec)?;

    {
        use std::os::fd::AsRawFd;
        let master_raw = master_fd.as_raw_fd();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let reader = std::io::BufReader::new(stream);
                for line in std::io::BufRead::lines(reader).map_while(Result::ok) {
                    let Ok(msg) = serde_json::from_str::<ControlMsg>(&line) else {
                        continue;
                    };
                    match msg {
                        ControlMsg::Stdin { data_b64 } => {
                            if let Ok(bytes) =
                                base64::engine::general_purpose::STANDARD.decode(data_b64)
                            {
                                let mut off = 0;
                                while off < bytes.len() {
                                    let n = unsafe {
                                        libc::write(
                                            master_raw,
                                            bytes[off..].as_ptr() as *const libc::c_void,
                                            bytes.len() - off,
                                        )
                                    };
                                    if n <= 0 {
                                        break;
                                    }
                                    off += n as usize;
                                }
                            }
                        }
                        ControlMsg::Resize { cols, rows } => {
                            let ws = libc::winsize {
                                ws_row: rows.max(2),
                                ws_col: cols.max(2),
                                ws_xpixel: 0,
                                ws_ypixel: 0,
                            };
                            unsafe { libc::ioctl(master_raw, libc::TIOCSWINSZ as _, &ws) };
                        }
                    }
                }
            }
        });
    }

    // 5. Pump pty output to the log until the command side closes.
    let mut out = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(out_path(dir, id))
        .map_err(|e| format!("open out log: {e}"))?;
    let mut buf = [0u8; 8192];
    loop {
        use std::os::fd::AsRawFd;
        let n = unsafe {
            libc::read(
                master_fd.as_raw_fd(),
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        if n > 0 {
            let _ = out.write_all(&buf[..n as usize]);
            continue;
        }
        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            // EIO = every slave fd closed → command (group) is done.
        }
        break;
    }
    let _ = out.flush();

    // 6. Record the exit.
    let status = child.wait().map_err(|e| format!("wait: {e}"))?;
    let exit_code = status.code().unwrap_or_else(|| {
        use std::os::unix::process::ExitStatusExt;
        status.signal().map(|s| 128 + s).unwrap_or(1)
    });
    let mut rec = read_record(dir, id)?;
    rec.status = ProcStatus::Exited;
    rec.exit_code = Some(exit_code);
    rec.finished_ms = Some(now_ms());
    write_record(dir, &rec)?;
    let _ = fs::remove_file(sock_path(dir, id));
    Ok(0)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        // Short name on purpose: the control socket lives here and unix
        // socket paths are capped at ~104 bytes.
        let dir = std::env::temp_dir().join(format!("fpr-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn seed_record(dir: &Path, command: &str, cwd: &Path) -> ProcRecord {
        let rec = ProcRecord {
            id: gen_id(),
            workspace_path: cwd.to_string_lossy().into_owned(),
            command: command.to_string(),
            status: ProcStatus::Starting,
            child_pid: None,
            host_pid: None,
            host_start_time: None,
            exit_code: None,
            started_ms: now_ms(),
            finished_ms: None,
            cols: 80,
            rows: 24,
        };
        write_record(dir, &rec).unwrap();
        rec
    }

    #[test]
    fn host_runs_command_captures_output_and_exit_code() {
        let dir = temp_dir("run");
        let rec = seed_record(&dir, "echo hello-from-pty; exit 3", &std::env::temp_dir());

        let code = run_host(&dir, &rec.id).unwrap();
        assert_eq!(code, 0);

        let after = read_record(&dir, &rec.id).unwrap();
        assert_eq!(after.status, ProcStatus::Exited);
        assert_eq!(after.exit_code, Some(3));
        assert!(after.finished_ms.is_some());

        let chunk = proc_output_in(&dir, &rec.id, None).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(chunk.data_b64)
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("hello-from-pty"),
            "pty output missing: {text:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn kill_proc_terminates_whole_group() {
        let dir = temp_dir("kill");
        // A parent that spawns a grandchild: killpg must take both down.
        let rec = seed_record(
            &dir,
            "sleep 300 & child=$!; echo started; wait $child",
            &std::env::temp_dir(),
        );

        let dir2 = dir.clone();
        let id = rec.id.clone();
        let host = std::thread::spawn(move || run_host(&dir2, &id));

        // Wait for the host to report running.
        let mut running = None;
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if let Ok(r) = read_record(&dir, &rec.id) {
                if r.status == ProcStatus::Running {
                    running = Some(r);
                    break;
                }
            }
        }
        let running = running.expect("proc never reached running state");
        let pgid = running.child_pid.unwrap();

        kill_proc_in(&dir, &rec.id, false).unwrap();
        let code = host.join().unwrap().unwrap();
        assert_eq!(code, 0);

        let after = read_record(&dir, &rec.id).unwrap();
        assert_eq!(after.status, ProcStatus::Exited);
        // SIGTERM'd shell reports 128+15.
        assert_eq!(after.exit_code, Some(143));

        // The whole group must be gone (ESRCH on signal 0 to -pgid).
        std::thread::sleep(std::time::Duration::from_millis(200));
        let ret = unsafe { libc::kill(-(pgid as libc::pid_t), 0) };
        assert_eq!(ret, -1, "process group {pgid} still alive");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stdin_over_control_socket_reaches_command() {
        let dir = temp_dir("stdin");
        let rec = seed_record(&dir, "read line; echo got:$line", &std::env::temp_dir());

        let dir2 = dir.clone();
        let id = rec.id.clone();
        let host = std::thread::spawn(move || run_host(&dir2, &id));

        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if sock_path(&dir, &rec.id).exists() {
                break;
            }
        }
        let payload = base64::engine::general_purpose::STANDARD.encode(b"ping\n");
        send_control(&dir, &rec.id, &ControlMsg::Stdin { data_b64: payload }).unwrap();

        host.join().unwrap().unwrap();
        let chunk = proc_output_in(&dir, &rec.id, None).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(chunk.data_b64)
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("got:ping"), "stdin echo missing: {text:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_heals_stale_running_record_and_clear_removes_files() {
        let dir = temp_dir("heal");
        let mut rec = seed_record(&dir, "true", &std::env::temp_dir());
        // Fabricate a "running" record owned by a dead host pid.
        rec.status = ProcStatus::Running;
        rec.host_pid = Some(u32::MAX - 1);
        rec.child_pid = Some(u32::MAX - 1);
        write_record(&dir, &rec).unwrap();
        fs::write(out_path(&dir, &rec.id), b"x").unwrap();

        let listed = list_procs_in(&dir);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].status, ProcStatus::Exited);
        assert_eq!(listed[0].exit_code, None);

        clear_proc_in(&dir, &rec.id).unwrap();
        assert!(!meta_path(&dir, &rec.id).exists());
        assert!(!out_path(&dir, &rec.id).exists());
        assert!(list_procs_in(&dir).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
