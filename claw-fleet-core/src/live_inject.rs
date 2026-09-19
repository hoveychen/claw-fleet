//! Deliver a follow-up message into a session's turn *while it is still
//! running*, instead of queueing it until the turn ends.
//!
//! Claude Code opens a unix socket per live session — `/tmp/cc-socks/<pid>.sock`
//! on macOS/Linux — and registers itself in `~/.claude/sessions/<pid>.json` with
//! its `sessionId`, pid and `messagingSocketPath`. That is the transport behind
//! the CLI's own cross-session `SendMessage`. A headless `claude -p` (which is
//! what every Fleet turn is) listens on it too, which is the part that makes
//! this module possible at all: Fleet holds the session id, so it can look up
//! the socket and write to it.
//!
//! The wire format is newline-delimited JSON. One line is enough:
//!
//! ```text
//! {"type":"user","message":{"role":"user","content":"…"},"priority":"next"}
//! ```
//!
//! Measured on 2026-09-19 against an isolated probe session mid-`sleep`: the
//! line landed at 18:38:44 and the receiver logged it out of its queue at
//! 18:38:55 with `"reason":"absorbed_mid_turn"` — the moment its in-flight Bash
//! call returned, not the end of the turn.
//!
//! **Authentication.** The receiver's `authRequired` defaults to true only on
//! Windows; on macOS/Linux the gate is the socket's `0600` mode plus a matching
//! uid, so a same-user process needs no token. Windows uses a named pipe rather
//! than a unix socket and is not supported here — [`inject`] returns an error
//! there and the caller falls back to the queue.
//!
//! **This is a best-effort fast path, never the only path.** The target process
//! can exit between the lookup and the write (its turn ends whenever it ends),
//! so every failure mode is reported to the caller, which re-queues the message
//! via [`crate::pending_message`] rather than dropping it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A live session's registration, as written by the Claude CLI.
///
/// Only the fields Fleet needs are named; the file carries more (peer features,
/// cwd, entrypoint, a derived display name) that this module has no use for.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct SessionRegistration {
    pid: u32,
    session_id: String,
    messaging_socket_path: Option<String>,
}

/// A running session we can write to right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveTarget {
    pub session_id: String,
    pub pid: u32,
    pub socket_path: PathBuf,
}

/// The frame written to the socket. `content` must be a non-empty string; the
/// receiver drops the line otherwise.
#[derive(Serialize)]
struct UserFrame<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    message: FrameMessage<'a>,
    priority: &'a str,
}

#[derive(Serialize)]
struct FrameMessage<'a> {
    role: &'a str,
    content: &'a str,
}

/// The receiver closes the connection on any line over 1 MiB, so a message that
/// long has to go through the queue instead.
const MAX_FRAME_BYTES: usize = 1_048_576;

fn sessions_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".claude").join("sessions"))
}

/// Whether a directory entry is a registration file (`<pid>.json`) rather than
/// one of the `<pid>.<sha256>.key` token files sitting beside it.
fn is_registration_file(name: &str) -> bool {
    match name.strip_suffix(".json") {
        Some(stem) => !stem.is_empty() && stem.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

fn read_registrations(dir: &Path) -> Vec<SessionRegistration> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_registration_file(name) {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        if let Ok(reg) = serde_json::from_str::<SessionRegistration>(&body) {
            out.push(reg);
        }
    }
    out
}

/// Find the live process serving `session_id`, or `None` when it is not running,
/// is not a Claude session, or left a stale registration behind.
///
/// Staleness matters because registrations outlive the process that wrote them —
/// `/tmp/cc-socks` on this machine holds sockets going back days. A dead pid can
/// also be *reused* by an unrelated program, so the pid is cross-checked against
/// Fleet's own process-table scan ([`crate::parked::session_pid`], which matches
/// on the session id in the argv): only a pid both sides agree on is written to.
pub fn resolve(session_id: &str) -> Option<LiveTarget> {
    if session_id.is_empty() {
        return None;
    }
    let dir = sessions_dir()?;
    let reg = read_registrations(&dir)
        .into_iter()
        .find(|r| r.session_id == session_id)?;

    if !crate::session::is_process_alive(reg.pid) {
        return None;
    }
    // The argv-matched pid is the authority on which process owns this session.
    // When it disagrees with the registration, the registration is stale (or the
    // pid was recycled) and writing to that socket would reach a stranger.
    if crate::parked::session_pid(session_id) != Some(reg.pid) {
        return None;
    }

    let socket_path = PathBuf::from(reg.messaging_socket_path?);
    if !socket_path.exists() {
        return None;
    }
    Some(LiveTarget {
        session_id: session_id.to_string(),
        pid: reg.pid,
        socket_path,
    })
}

/// Whether a message for this session can be delivered mid-turn right now.
pub fn can_inject(session_id: &str) -> bool {
    cfg!(unix) && resolve(session_id).is_some()
}

/// Serialise the one line written to the socket.
fn encode_frame(text: &str) -> Result<Vec<u8>, String> {
    let frame = UserFrame {
        kind: "user",
        message: FrameMessage {
            role: "user",
            content: text,
        },
        priority: "next",
    };
    let mut line = serde_json::to_vec(&frame).map_err(|e| format!("serialize frame: {e}"))?;
    line.push(b'\n');
    if line.len() > MAX_FRAME_BYTES {
        return Err(format!(
            "message is {} bytes, over the {MAX_FRAME_BYTES} byte frame limit",
            line.len()
        ));
    }
    Ok(line)
}

/// Deliver `text` into the running turn of `session_id`.
///
/// `Ok(())` means the line reached the socket; the receiver absorbs it at its
/// next tool round. Every `Err` is a reason to fall back to the queue — the
/// session may have finished between the lookup and the connect, which is normal
/// rather than exceptional.
#[cfg(unix)]
pub fn inject(session_id: &str, text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::net::UnixStream;

    if text.trim().is_empty() {
        return Err("refusing to inject an empty message".to_string());
    }
    let line = encode_frame(text)?;
    let target = resolve(session_id).ok_or_else(|| format!("no live socket for {session_id}"))?;

    let mut stream = UnixStream::connect(&target.socket_path)
        .map_err(|e| format!("connect {}: {e}", target.socket_path.display()))?;
    stream
        .write_all(&line)
        .map_err(|e| format!("write frame: {e}"))?;
    stream.flush().map_err(|e| format!("flush frame: {e}"))?;
    Ok(())
}

/// Windows exposes the same channel over a named pipe with a mandatory token,
/// which this module does not speak. Callers fall back to the queue.
#[cfg(not(unix))]
pub fn inject(_session_id: &str, _text: &str) -> Result<(), String> {
    Err("mid-turn injection is not supported on this platform".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_files_are_pid_json_not_key_files() {
        assert!(is_registration_file("9567.json"));
        assert!(is_registration_file("12249.json"));
        assert!(!is_registration_file(
            "9567.b408b4c7daca0c8aedf5f06a1e2a4c3555e78e84d9b465840a3f2772ed86a665.key"
        ));
        assert!(!is_registration_file(
            "9567.b408b4c7daca0c8aedf5f06a1e2a4c3555e78e84d9b465840a3f2772ed86a665.json"
        ));
        assert!(!is_registration_file("notes.json"));
        assert!(!is_registration_file(".json"));
        assert!(!is_registration_file("9567"));
    }

    #[test]
    fn registrations_parse_the_real_cli_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Verbatim shape of ~/.claude/sessions/<pid>.json, extra fields included
        // so the parser is proven tolerant of the ones Fleet ignores.
        std::fs::write(
            dir.path().join("9567.json"),
            r#"{"pid":9567,"sessionId":"886c18b2-98b5-411a-a142-e6a89bb7b88a",
                "cwd":"/Users/x/workspace/claude-fleet","startedAt":1789842702289,
                "procStart":"Sat Sep 19 18:31:41 2026","version":"2.1.263",
                "peerProtocol":1,"peerFeatures":["notify_idle"],"kind":"interactive",
                "entrypoint":"claw-fleet-newsession","pidDomain":"darwin",
                "messagingSocketPath":"/tmp/cc-socks/9567.sock",
                "name":"claude-fleet-11","nameSource":"derived"}"#,
        )
        .expect("write registration");
        std::fs::write(dir.path().join("9567.abc123.key"), r#"{"peerToken":"x"}"#)
            .expect("write key");
        std::fs::write(dir.path().join("garbage.json"), "not json").expect("write garbage");

        let regs = read_registrations(dir.path());
        assert_eq!(regs.len(), 1, "only the <pid>.json file is a registration");
        assert_eq!(regs[0].pid, 9567);
        assert_eq!(regs[0].session_id, "886c18b2-98b5-411a-a142-e6a89bb7b88a");
        assert_eq!(
            regs[0].messaging_socket_path.as_deref(),
            Some("/tmp/cc-socks/9567.sock")
        );
    }

    #[test]
    fn a_registration_without_a_socket_path_is_not_a_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("4242.json"),
            r#"{"pid":4242,"sessionId":"no-socket","cwd":"/tmp"}"#,
        )
        .expect("write registration");
        let regs = read_registrations(dir.path());
        assert_eq!(regs.len(), 1);
        assert!(regs[0].messaging_socket_path.is_none());
    }

    #[test]
    fn unknown_session_has_no_target() {
        assert!(resolve("00000000-0000-0000-0000-000000000000").is_none());
        assert!(resolve("").is_none());
    }

    #[test]
    fn frame_is_one_json_line_the_receiver_accepts() {
        let line = encode_frame("hello 老板").expect("encode");
        assert!(line.ends_with(b"\n"), "receiver splits on newlines");
        assert_eq!(
            line.iter().filter(|b| **b == b'\n').count(),
            1,
            "exactly one line per frame"
        );
        let parsed: serde_json::Value =
            serde_json::from_slice(&line[..line.len() - 1]).expect("valid json");
        assert_eq!(parsed["type"], "user");
        assert_eq!(parsed["message"]["role"], "user");
        assert_eq!(parsed["message"]["content"], "hello 老板");
        assert_eq!(parsed["priority"], "next");
    }

    #[test]
    fn a_message_with_newlines_stays_one_line() {
        let line = encode_frame("line one\nline two\nline three").expect("encode");
        assert_eq!(
            line.iter().filter(|b| **b == b'\n').count(),
            1,
            "embedded newlines must be JSON-escaped, not split the frame"
        );
    }

    #[test]
    fn oversized_messages_are_rejected_rather_than_truncated() {
        let huge = "x".repeat(MAX_FRAME_BYTES + 1);
        let err = encode_frame(&huge).expect_err("over the limit");
        assert!(err.contains("frame limit"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn injecting_an_empty_message_is_refused() {
        let err = inject("some-session", "   ").expect_err("empty");
        assert!(err.contains("empty"), "got: {err}");
    }
}
