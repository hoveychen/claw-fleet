//! Per-session trace of every call an agent makes into Fleet.
//!
//! Before this module the only record of a `fleet__watch` / `fleet__handoff` /
//! `fleet plan check` call was the `tool_use` block in the agent's own
//! transcript and the `PostToolUse` payload in the shared `hooks.jsonl` —
//! both written by the harness, neither by Fleet. Neither says how long the
//! call took inside Fleet, and a call that hangs or is killed mid-flight (a
//! `fleet__ask` waiting on a card nobody answered) leaves no trace at all until
//! it returns.
//!
//! Every MCP `tools/call` and every agent-facing `fleet` CLI invocation appends
//! to `~/.fleet/call-trace/<session_id>.jsonl`:
//!
//! - a `start` record (tool, full arguments, pid) the moment the call arrives;
//! - an `end` record with the same `call` id, the outcome (`ok` / `error`), the
//!   full result payload and `ms` elapsed. MCP calls always get one; CLI calls
//!   only record `start`, because their many `process::exit` paths skip any
//!   epilogue (the harness transcript holds their stdout).
//!
//! A `start` with no matching `end` is a call that never returned. The chain
//! debug bundle ([`crate::chain_export`]) ships these files verbatim.
//!
//! Writing never fails loudly: an unwritable trace must not break the call it
//! records.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use serde_json::{json, Value};

/// A trace file past this size is rotated to `<sid>.1.jsonl` (replacing any
/// older rotation), so one runaway session cannot grow it without bound.
const ROTATE_BYTES: u64 = 32 * 1024 * 1024;

/// Any single string inside a recorded payload longer than this is cut, with
/// the original length noted. Large enough to keep a whole decision card's
/// html or a long `fleet__inspect` dump; small enough that an inlined base64
/// image does not bloat every line.
const MAX_STRING_BYTES: usize = 256 * 1024;

pub fn call_trace_dir() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("call-trace"))
}

/// The live trace file for a session.
pub fn trace_path(session_id: &str) -> Option<PathBuf> {
    call_trace_dir().map(|d| d.join(format!("{}.jsonl", file_stem(session_id))))
}

/// The rotated-out predecessor of [`trace_path`], if any.
pub fn rotated_path(session_id: &str) -> Option<PathBuf> {
    call_trace_dir().map(|d| d.join(format!("{}.1.jsonl", file_stem(session_id))))
}

/// Session ids are uuids / thread ids in practice; keep anything else from
/// escaping the directory.
fn file_stem(session_id: &str) -> String {
    let s: String = session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if s.is_empty() { "_unknown".to_string() } else { s }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Unique per call within a process: pid + a process-local counter. Nanosecond
/// timestamps alone collide (see `systemtime_nanos_not_unique`).
fn next_call_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{}-{}", std::process::id(), now_ms(), n)
}

/// Cut over-long strings anywhere inside `v`.
fn clamp(v: &Value) -> Value {
    match v {
        Value::String(s) if s.len() > MAX_STRING_BYTES => {
            let mut end = MAX_STRING_BYTES;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            Value::String(format!("{}…[truncated, {} bytes total]", &s[..end], s.len()))
        }
        Value::Array(a) => Value::Array(a.iter().map(clamp).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), clamp(v))).collect()),
        other => other.clone(),
    }
}

fn append(session_id: &str, record: &Value) {
    let (Some(path), Some(dir)) = (trace_path(session_id), call_trace_dir()) else {
        return;
    };
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    if fs::metadata(&path).map(|m| m.len() > ROTATE_BYTES).unwrap_or(false) {
        if let Some(rot) = rotated_path(session_id) {
            let _ = fs::rename(&path, rot);
        }
    }
    let Ok(line) = serde_json::to_string(record) else {
        return;
    };
    // One write_all of a whole line on an O_APPEND file: concurrent writers
    // (the MCP server and a CLI call from the same session) do not interleave
    // within a line for writes of this size on local filesystems.
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(format!("{line}\n").as_bytes());
    }
}

/// An in-flight traced call. Finish it with [`CallTrace::end`].
pub struct CallTrace {
    session_id: String,
    call: String,
    tool: String,
    started: Instant,
}

impl CallTrace {
    /// Record the start of a call. `surface` is `"mcp"` or `"cli"`. With an
    /// empty `session_id` nothing is recorded (there is no file to put it in),
    /// but the returned handle is still safe to `end`.
    pub fn begin(session_id: &str, surface: &str, tool: &str, args: &Value) -> Self {
        let trace = CallTrace {
            session_id: session_id.to_string(),
            call: next_call_id(),
            tool: tool.to_string(),
            started: Instant::now(),
        };
        if !session_id.is_empty() {
            let cwd = std::env::current_dir().ok().map(|p| p.display().to_string());
            append(
                session_id,
                &json!({
                    "ts": now_ms(),
                    "phase": "start",
                    "call": trace.call,
                    "surface": surface,
                    "tool": tool,
                    "args": clamp(args),
                    "pid": std::process::id(),
                    "cwd": cwd,
                }),
            );
        }
        trace
    }

    /// Record the outcome. `is_error` mirrors the MCP `isError` flag (or a
    /// JSON-RPC error); `result` is the full payload handed back to the agent.
    pub fn end(self, is_error: bool, result: &Value) {
        if self.session_id.is_empty() {
            return;
        }
        append(
            &self.session_id,
            &json!({
                "ts": now_ms(),
                "phase": "end",
                "call": self.call,
                "tool": self.tool,
                "outcome": if is_error { "error" } else { "ok" },
                "ms": self.started.elapsed().as_millis() as u64,
                "result": clamp(result),
            }),
        );
    }
}

/// Every record in a session's trace, rotated file first, oldest to newest.
/// Unparseable lines are skipped.
pub fn read(session_id: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for path in [rotated_path(session_id), trace_path(session_id)].into_iter().flatten() {
        if let Ok(text) = fs::read_to_string(&path) {
            out.extend(text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_home<F: FnOnce()>(f: F) {
        let _g = crate::session::fleet_home_lock();
        let dir = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        unsafe { std::env::set_var("FLEET_HOME", dir.path()) };
        f();
        match prev {
            Some(v) => unsafe { std::env::set_var("FLEET_HOME", v) },
            None => unsafe { std::env::remove_var("FLEET_HOME") },
        }
    }

    #[test]
    fn begin_and_end_pair_by_call_id() {
        with_home(|| {
            let t = CallTrace::begin("sess-1", "mcp", "fleet__watch", &json!({"action": "create"}));
            t.end(false, &json!({"content": [{"type": "text", "text": "ok"}]}));
            let recs = read("sess-1");
            assert_eq!(recs.len(), 2);
            assert_eq!(recs[0]["phase"], "start");
            assert_eq!(recs[0]["args"]["action"], "create");
            assert_eq!(recs[1]["phase"], "end");
            assert_eq!(recs[1]["outcome"], "ok");
            assert_eq!(recs[0]["call"], recs[1]["call"]);
        });
    }

    #[test]
    fn empty_session_id_records_nothing() {
        with_home(|| {
            CallTrace::begin("", "cli", "plan", &json!([])).end(true, &Value::Null);
            assert!(read("").is_empty());
            assert!(!call_trace_dir().unwrap().exists());
        });
    }

    #[test]
    fn long_strings_are_clamped() {
        let big = "x".repeat(MAX_STRING_BYTES + 10);
        let v = clamp(&json!({"html": big}));
        let s = v["html"].as_str().unwrap();
        assert!(s.len() < MAX_STRING_BYTES + 64);
        assert!(s.contains("truncated"));
    }

    #[test]
    fn session_id_cannot_escape_the_directory() {
        assert_eq!(file_stem("../../etc/passwd"), "______etc_passwd");
    }
}
