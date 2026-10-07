//! Job — a long shell command that Fleet runs on the agent's behalf, so it
//! outlives the turn (or subagent) that started it and can be waited on by id.
//!
//! The gap this closes: an agent's own ways to run a long command all die or
//! time out somewhere. A foreground `Bash` call caps at 600s and is then moved to
//! the background; a background shell or `Monitor` belongs to the agent that
//! started it and is killed when that agent's turn ends; `nohup … &` escapes the
//! harness entirely. Whoever wants the result afterwards — the same agent, its
//! parent, a successor — is left grepping `ps` and tailing a log, and a
//! `ps | grep <name>` loop happily matches the agent's own `claude -p` argv and
//! waits forever on a process that is long gone.
//!
//! A job is a thin layer over [`crate::proc_runner`]: the command runs under a
//! `setsid`-detached host with an on-disk record, so it survives every caller,
//! has a stable id, a real exit code and a log file, and shows up in the
//! desktop's workspace command list for free. This module adds what an agent
//! needs on top: a bounded blocking [`wait`] and a plain-text rendering with the
//! ANSI-stripped tail of the output.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::proc_runner::{self, ProcRecord, ProcStatus};

/// Longest a single [`wait`] blocks. Below the 600s ceiling of a foreground
/// `Bash` call (so `fleet job wait` fits in one) with headroom for the tail read.
pub const MAX_WAIT_SECS: u64 = 540;

/// Lines of output tail shown by default.
pub const DEFAULT_TAIL_LINES: usize = 20;
const MAX_TAIL_LINES: usize = 200;

/// Bytes read from the end of the log to build the tail.
const TAIL_BYTES: u64 = 64 * 1024;

/// Pty geometry: wide, so test runners don't wrap their summary lines.
const JOB_COLS: u16 = 200;
const JOB_ROWS: u16 = 50;

const WAIT_POLL: Duration = Duration::from_millis(500);

/// Start `command` in `workspace` as a detached job. `host_exe` is the binary
/// that intercepts [`proc_runner::HOST_ARGV_MARKER`] (the caller's own exe).
pub fn run(host_exe: &Path, workspace: &str, command: &str) -> Result<ProcRecord, String> {
    if command.trim().is_empty() {
        return Err("command is empty".into());
    }
    proc_runner::spawn_proc(host_exe, workspace, command, JOB_COLS, JOB_ROWS)
}

pub fn is_done(rec: &ProcRecord) -> bool {
    rec.status == ProcStatus::Exited
}

/// Block until the job exits or `max_secs` (clamped to [`MAX_WAIT_SECS`])
/// passes. Returns the latest record either way; check [`is_done`].
pub fn wait(id: &str, max_secs: u64) -> Result<ProcRecord, String> {
    let dir = procs_dir()?;
    wait_in(&dir, id, Duration::from_secs(max_secs.min(MAX_WAIT_SECS)))
}

fn wait_in(dir: &Path, id: &str, max: Duration) -> Result<ProcRecord, String> {
    let deadline = Instant::now() + max;
    loop {
        let rec = proc_runner::get_proc_in(dir, id)?;
        if is_done(&rec) || Instant::now() >= deadline {
            return Ok(rec);
        }
        std::thread::sleep(WAIT_POLL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

pub fn get(id: &str) -> Result<ProcRecord, String> {
    proc_runner::get_proc(id)
}

pub fn stop(id: &str) -> Result<(), String> {
    proc_runner::kill_proc(id, false)
}

/// Jobs (any proc) started in `workspace` or below it (a plan's
/// `.worktrees/<id>` checkout), newest first.
pub fn list(workspace: &str) -> Vec<ProcRecord> {
    proc_runner::list_procs()
        .into_iter()
        .filter(|r| Path::new(&r.workspace_path).starts_with(workspace))
        .collect()
}

pub fn log_path(id: &str) -> Result<PathBuf, String> {
    Ok(proc_runner::output_file_in(&procs_dir()?, id))
}

fn procs_dir() -> Result<PathBuf, String> {
    proc_runner::procs_dir().ok_or_else(|| "cannot determine home dir".to_string())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn fmt_duration(ms: u64) -> String {
    let s = ms / 1000;
    match (s / 3600, (s / 60) % 60, s % 60) {
        (0, 0, sec) => format!("{sec}s"),
        (0, m, sec) => format!("{m}m{sec:02}s"),
        (h, m, sec) => format!("{h}h{m:02}m{sec:02}s"),
    }
}

/// One-line status: `running for 3m02s` / `exited 0 after 12m03s`.
pub fn status_line(rec: &ProcRecord) -> String {
    let elapsed = rec.finished_ms.unwrap_or_else(now_ms).saturating_sub(rec.started_ms);
    match (rec.status, rec.exit_code) {
        (ProcStatus::Exited, Some(code)) => {
            format!("exited {code} after {}", fmt_duration(elapsed))
        }
        // The host died without reporting (kill -9, crash, reboot) — the exit
        // was inferred, not observed.
        (ProcStatus::Exited, None) => format!(
            "exited with unknown status after {} (its host process died)",
            fmt_duration(elapsed)
        ),
        (ProcStatus::Starting, _) => "starting".into(),
        (ProcStatus::Running, _) => format!("running for {}", fmt_duration(elapsed)),
    }
}

/// Agent-facing report: id, status, command, log path and the output tail.
pub fn render(rec: &ProcRecord, tail_lines: usize) -> String {
    let log = log_path(&rec.id)
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let mut out = format!(
        "job {}: {}\ncommand: {}\nworkspace: {}\nlog: {log}\n",
        rec.id,
        status_line(rec),
        rec.command,
        rec.workspace_path
    );
    let n = tail_lines.min(MAX_TAIL_LINES);
    if n > 0 {
        let tail = std::fs::read(&log)
            .map(|b| tail_text(&b, n))
            .unwrap_or_default();
        if tail.is_empty() {
            out.push_str("(no output yet)\n");
        } else {
            out.push_str(&format!("--- last {n} lines ---\n{tail}\n"));
        }
    }
    out
}

/// The last `n` non-blank lines of raw pty output, with ANSI escapes removed
/// and `\r`-redrawn progress lines collapsed to their final state.
pub fn tail_text(raw: &[u8], n: usize) -> String {
    let start = raw.len().saturating_sub(TAIL_BYTES as usize);
    let text = String::from_utf8_lossy(&raw[start..]);
    let clean = strip_ansi(&text);
    let lines: Vec<&str> = clean
        .split('\n')
        .map(|l| {
            let l = l.strip_suffix('\r').unwrap_or(l);
            l.rsplit('\r').next().unwrap_or(l)
        })
        .filter(|l| !l.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Remove CSI (`ESC [ … final`) and OSC (`ESC ] … BEL|ESC \`) sequences and
/// other two-byte escapes. Unlike `harness_login::strip_ansi` it inserts
/// nothing in their place: test runners color words, they don't position them.
fn strip_ansi(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_strips_color_and_collapses_carriage_returns() {
        let raw = b"\x1b[32mPASS\x1b[0m  a\r\n\r\nprogress 10%\rprogress 100%\r\n\x1b]0;title\x07done\r\n";
        assert_eq!(tail_text(raw, 10), "PASS  a\nprogress 100%\ndone");
        assert_eq!(tail_text(raw, 1), "done");
    }

    #[test]
    fn status_line_distinguishes_observed_and_inferred_exit() {
        let mut rec = ProcRecord {
            id: "p1".into(),
            workspace_path: "/ws".into(),
            command: "true".into(),
            status: ProcStatus::Exited,
            child_pid: None,
            host_pid: None,
            host_start_time: None,
            exit_code: Some(3),
            started_ms: 0,
            finished_ms: Some(725_000),
            cols: 80,
            rows: 24,
        };
        assert_eq!(status_line(&rec), "exited 3 after 12m05s");
        rec.exit_code = None;
        assert!(status_line(&rec).contains("unknown status"));
    }

    #[test]
    fn fmt_duration_units() {
        assert_eq!(fmt_duration(9_000), "9s");
        assert_eq!(fmt_duration(3_725_000), "1h02m05s");
    }

    fn write(dir: &Path, rec: &ProcRecord) {
        std::fs::write(
            dir.join(format!("{}.json", rec.id)),
            serde_json::to_string(rec).unwrap(),
        )
        .unwrap();
    }

    /// The real detached host is exercised end to end in
    /// `fleet-cli/tests/job_e2e.rs`; here a live record (host = this test
    /// process, so self-healing leaves it alone) flips to exited mid-wait.
    #[test]
    fn wait_returns_on_deadline_then_on_exit() {
        let dir = tempfile::tempdir().unwrap();
        let pid = std::process::id();
        let mut rec = ProcRecord {
            id: "pwait".into(),
            workspace_path: "/ws".into(),
            command: "x".into(),
            status: ProcStatus::Running,
            child_pid: Some(1),
            host_pid: Some(pid),
            host_start_time: crate::session::process_start_time(pid),
            exit_code: None,
            started_ms: now_ms(),
            finished_ms: None,
            cols: 80,
            rows: 24,
        };
        write(dir.path(), &rec);

        let early = wait_in(dir.path(), "pwait", Duration::from_millis(300)).unwrap();
        assert!(!is_done(&early));

        let d = dir.path().to_path_buf();
        let flip = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            rec.status = ProcStatus::Exited;
            rec.exit_code = Some(7);
            rec.finished_ms = Some(now_ms());
            write(&d, &rec);
        });
        let started = Instant::now();
        let done = wait_in(dir.path(), "pwait", Duration::from_secs(30)).unwrap();
        flip.join().unwrap();
        assert!(is_done(&done));
        assert_eq!(done.exit_code, Some(7));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
