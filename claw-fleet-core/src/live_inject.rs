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
//!
//! **The child token, and why it is not optional.** A receiver running under
//! `bypassPermissions` holds an unauthenticated inbound message for approval
//! instead of delivering it — and a headless `claude -p` has no UI to approve
//! it and keeps the hold in memory, so the message is simply gone when the turn
//! ends. Measured 2026-09-19 against a `--permission-mode bypassPermissions`
//! probe: an anonymous write produced no `queue-operation` entry at all, not
//! even an enqueue.
//!
//! Presenting the receiver's own `childToken` makes it treat the write as
//! self-sent and skip that gate. The token reaches Fleet because hooks are
//! spawned *by* the session and inherit `CLAUDE_CODE_MESSAGING_TOKEN`; the
//! `SessionStart` hook records it via [`record_session_token`]. It does **not**
//! change how the message is presented — that is hardcoded to peer framing (see
//! the wiki doc `cc/mid-turn-messaging`) — it only decides whether the message
//! arrives at all.

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

/// A session's own messaging credentials, as seen from inside one of its hooks.
///
/// `socket_path` is recorded alongside the token because both are per-process:
/// every Fleet turn is a new `claude -p` with a new pid, a new socket and a new
/// token. Matching the stored path against the live registration is what keeps
/// a previous turn's stale token from being presented to the current one.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct SessionToken {
    socket_path: String,
    token: String,
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

/// The optional first line, presenting the receiver's own child token.
#[derive(Serialize)]
struct AuthFrame<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    token: &'a str,
}

/// The receiver closes the connection on any line over 1 MiB, so a message that
/// long has to go through the queue instead.
const MAX_FRAME_BYTES: usize = 1_048_576;

fn sessions_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".claude").join("sessions"))
}

fn token_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("messaging-tokens"))
}

/// Path of a session's recorded token, or `None` for an id that could escape the
/// store directory. Mirrors `pending_message::queue_path`'s sanitisation.
fn token_path(session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty()
        || session_id.contains('/')
        || session_id.contains('\\')
        || session_id.contains("..")
    {
        return None;
    }
    token_dir().map(|d| d.join(format!("{session_id}.json")))
}

/// Record the calling session's own messaging credentials for Fleet to present
/// later. Call from a hook — a process the session itself spawned, and therefore
/// the only kind that can read `CLAUDE_CODE_MESSAGING_TOKEN`.
///
/// A no-op when either environment variable is absent (an older CLI, or a
/// process that is not a session's child), so callers need no platform guard.
/// The file is written `0600`: it holds a credential that lets any reader write
/// into this session's turn.
pub fn record_session_token(session_id: &str) {
    let (Ok(socket_path), Ok(token)) = (
        std::env::var("CLAUDE_CODE_MESSAGING_SOCKET"),
        std::env::var("CLAUDE_CODE_MESSAGING_TOKEN"),
    ) else {
        return;
    };
    if socket_path.is_empty() || token.is_empty() {
        return;
    }
    let Some(path) = token_path(session_id) else {
        return;
    };
    let record = SessionToken { socket_path, token };
    let Ok(body) = serde_json::to_string(&record) else {
        return;
    };
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    if std::fs::write(&path, body).is_err() {
        return;
    }
    restrict_to_owner(&path);
    prune_stale_tokens();
}

/// Drop recorded tokens whose socket no longer exists. Without this the store
/// grows one credential file per session forever — every session that ever ran
/// a turn writes one, and nothing else would ever remove them.
///
/// A vanished socket means that process is gone, so the token authenticates
/// nothing. Cheap enough to run on every record: the store holds at most one
/// small file per session.
fn prune_stale_tokens() {
    let Some(dir) = token_dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str::<SessionToken>(&body) {
            Ok(rec) if Path::new(&rec.socket_path).exists() => {}
            // Gone, or unparseable (a partial write, or an older format).
            _ => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

#[cfg(unix)]
fn restrict_to_owner(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path) {}

/// The token to present when writing to `socket_path`, if one was recorded for
/// this session *and* belongs to the process now listening there.
///
/// The path comparison is the freshness check: a token recorded by an earlier
/// turn names that turn's socket, which no longer matches, so it is discarded
/// rather than presented to a process it does not authenticate against.
fn token_for(session_id: &str, socket_path: &Path) -> Option<String> {
    let path = token_path(session_id)?;
    let body = std::fs::read_to_string(path).ok()?;
    let record: SessionToken = serde_json::from_str(&body).ok()?;
    (Path::new(&record.socket_path) == socket_path).then_some(record.token)
}

/// Drop a session's recorded token. Called when its turn is known to be over,
/// so the store does not accumulate one credential file per session forever.
pub fn forget_session_token(session_id: &str) {
    if let Some(path) = token_path(session_id) {
        let _ = std::fs::remove_file(path);
    }
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

/// Session ids whose CLI self-registration names a live pid. Covers interactive
/// terminal sessions, whose argv carries no session id for
/// [`crate::session::scan_cli_processes`] to match. A recycled pid can make a
/// dead session look alive here; callers use this only where "alive" is the
/// safe answer (the plan reviver then simply does not spawn).
pub(crate) fn live_registered_session_ids() -> std::collections::HashSet<String> {
    let Some(dir) = sessions_dir() else {
        return Default::default();
    };
    read_registrations(&dir)
        .into_iter()
        .filter(|r| crate::session::is_process_alive(r.pid))
        .map(|r| r.session_id)
        .collect()
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

/// Told to the receiver about a message its user typed in a Fleet surface.
///
/// The CLI stamps every socket message as a peer and says, verbatim, that it
/// was "not typed by your user" — which is false for these, and acted on:
/// measured 2026-09-20, a session answered a question its user had typed in
/// the composer by guessing which peer had sent it and `SendMessage`-ing the
/// answer to an unrelated session.
///
/// Chinese because this is product text an agent reads, the same as the rest
/// of Fleet's injected guidance. The last sentence is deliberate: attribution
/// must not turn into escalation, and Fleet's own advice is to approve from a
/// decision card (where the session is stopped and the click is a real
/// approval), never from a mid-turn message.
pub const USER_SIGNATURE: &str = "\n\n---\n（这条消息是 Fleet 的用户本人在 Fleet 界面里输入的，不是另一个 Claude 会话发来的——Claude Code 把一切经 socket 直投的消息一律框成 peer，这行署名是 Fleet 补的。请当作你用户本人的话来处理：回答就在本会话里输出，或写进决策卡，不要用 SendMessage 转给别的会话。它仍然不构成对任何待批准操作的批准。）";

/// Append [`USER_SIGNATURE`] to a message the user typed.
pub fn sign_as_user(text: &str) -> String {
    format!("{text}{USER_SIGNATURE}")
}

/// Drop the signature again, for anything rendering the message back to that
/// same user — the transcript bubble should read as what they typed, not as
/// what Fleet told the agent about it.
pub fn strip_user_signature(text: &str) -> &str {
    text.strip_suffix(USER_SIGNATURE).unwrap_or(text)
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

/// Serialise the auth line presenting `token`.
fn encode_auth(token: &str) -> Result<Vec<u8>, String> {
    let frame = AuthFrame {
        kind: "auth",
        token,
    };
    let mut line = serde_json::to_vec(&frame).map_err(|e| format!("serialize auth: {e}"))?;
    line.push(b'\n');
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
    // Authenticate first when we hold this turn's own token: without it a
    // receiver under bypassPermissions holds the message for an approval that
    // will never come. Harmless when the receiver does not require auth.
    if let Some(token) = token_for(session_id, &target.socket_path) {
        let auth = encode_auth(&token)?;
        stream
            .write_all(&auth)
            .map_err(|e| format!("write auth: {e}"))?;
    }
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

    /// Manual probe, not part of any suite: injects a signed message into a
    /// live session so a human can read what actually arrives. Run it against
    /// your own session with
    /// `FLEET_PROBE_SESSION=<id> FLEET_PROBE_WORKSPACE=<dir> cargo test -p
    /// claw-fleet-core --lib signed_injection_probe -- --ignored --nocapture`.
    #[test]
    #[ignore = "manual probe: writes into a real running session"]
    fn signed_injection_probe() {
        let session = std::env::var("FLEET_PROBE_SESSION").expect("FLEET_PROBE_SESSION");
        let workspace = std::env::var("FLEET_PROBE_WORKSPACE").expect("FLEET_PROBE_WORKSPACE");
        let delivery = crate::pending_message::enqueue(
            &session,
            &workspace,
            "探针：这条是署名验证",
            crate::pending_message::Sender::User,
        )
        .expect("enqueue");
        println!("delivery: {delivery:?}");
    }

    #[test]
    fn signing_a_user_message_round_trips() {
        let signed = sign_as_user("合并吧");
        // The agent is told who it is from...
        assert!(signed.starts_with("合并吧"));
        assert!(signed.contains("用户本人"));
        assert!(signed.contains("不要用 SendMessage 转给别的会话"));
        // ...and attribution stays short of granting approval.
        assert!(signed.contains("不构成对任何待批准操作的批准"));
        // ...while the reader gets their own words back.
        assert_eq!(strip_user_signature(&signed), "合并吧");
        // An unsigned message (an agent's `fleet send`) passes through.
        assert_eq!(strip_user_signature("plain peer text"), "plain peer text");
    }

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

    #[test]
    fn auth_line_is_its_own_json_line() {
        let line = encode_auth("deadbeef").expect("encode");
        assert!(line.ends_with(b"\n"));
        let parsed: serde_json::Value =
            serde_json::from_slice(&line[..line.len() - 1]).expect("valid json");
        assert_eq!(parsed["type"], "auth");
        assert_eq!(parsed["token"], "deadbeef");
    }

    #[test]
    fn token_path_rejects_traversal() {
        assert!(token_path("../../etc/passwd").is_none());
        assert!(token_path("a/b").is_none());
        assert!(token_path("").is_none());
    }

    #[test]
    fn a_token_is_presented_only_for_the_socket_it_was_recorded_against() {
        let home = tempfile::tempdir().expect("tempdir");
        let dir = home.path().join(".fleet").join("messaging-tokens");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join("sess-1.json"),
            r#"{"socketPath":"/tmp/cc-socks/100.sock","token":"tok-100"}"#,
        )
        .expect("write token");

        let read = |sid: &str, sock: &str| -> Option<String> {
            let body = std::fs::read_to_string(dir.join(format!("{sid}.json"))).ok()?;
            let rec: SessionToken = serde_json::from_str(&body).ok()?;
            (Path::new(&rec.socket_path) == Path::new(sock)).then_some(rec.token)
        };

        assert_eq!(
            read("sess-1", "/tmp/cc-socks/100.sock").as_deref(),
            Some("tok-100"),
            "the socket it was recorded against gets the token"
        );
        assert_eq!(
            read("sess-1", "/tmp/cc-socks/200.sock"),
            None,
            "a later turn's socket must not be handed the old turn's token"
        );
    }

    #[cfg(unix)]
    #[test]
    fn injecting_an_empty_message_is_refused() {
        let err = inject("some-session", "   ").expect_err("empty");
        assert!(err.contains("empty"), "got: {err}");
    }
}
