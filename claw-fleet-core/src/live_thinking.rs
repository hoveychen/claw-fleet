//! Live thinking sidecars.
//!
//! Fleet-spawned sessions run headless `claude --print`. Historically their
//! stdout was discarded ([`crate::session_launch`] set `Stdio::null()`) and
//! Fleet relied entirely on the JSONL transcript the CLI writes on disk. The
//! transcript is written at *message-completion* granularity — an assistant
//! message (thinking + text + tool_use) only lands once its `stop_reason` is
//! set. So a file-tailing observer can render a *completed* thinking block, but
//! never the token-by-token reasoning stream a first-party client (e.g. the
//! VS Code extension) shows live.
//!
//! To close that gap we spawn those sessions with
//! `--output-format stream-json --verbose --include-partial-messages` and point
//! the child's stdout at a *sidecar* file under `~/.fleet/live-thinking/`. The
//! stream-json output is newline-delimited JSON that carries
//! `content_block_delta` events whose `delta.type == "thinking_delta"` — the
//! incremental reasoning tokens. Tailing the sidecar therefore yields live
//! thinking without any new protocol; it is the same SSE payload the client
//! sees, just teed to a file we already know how to tail.
//!
//! The sidecar is named by the child PID (known right after spawn); the
//! session id is recovered from the `session_id` field the stream stamps on
//! every event, so the naming scheme never has to change session-creation
//! semantics.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Directory holding one sidecar per live Fleet-spawned session.
/// `None` only when the real home dir can't be resolved.
pub fn sidecar_dir() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("live-thinking"))
}

/// Sidecar path for a spawned child, keyed by its PID. The stream itself
/// carries the session id, so the filename only needs to be unique per live
/// process.
pub fn sidecar_path_for_pid(pid: u32) -> Option<PathBuf> {
    sidecar_dir().map(|d| d.join(format!("pid-{pid}.jsonl")))
}

/// Ensure the sidecar directory exists; returns the dir path.
pub fn ensure_sidecar_dir() -> Result<PathBuf, String> {
    let dir = sidecar_dir().ok_or("no fleet dir")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// The live thinking snapshot for one session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LiveThinking {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    /// Accumulated text of the most recent thinking block in the current turn.
    pub thinking: String,
    /// True while the turn appears in progress (no terminal `result` line yet
    /// and the sidecar was written to recently).
    pub streaming: bool,
    /// Seconds since the sidecar file was last modified.
    #[serde(rename = "updatedSecsAgo")]
    pub updated_secs_ago: f64,
}

/// Read the live thinking for `session_id` by scanning the sidecar dir for the
/// file whose stream carries a matching `session_id`, then reconstructing the
/// latest thinking block from its `thinking_delta` events.
///
/// Returns `None` when no sidecar matches (session not Fleet-spawned, already
/// pruned, or predates this feature).
pub fn read_live_thinking(session_id: &str) -> Option<LiveThinking> {
    let dir = sidecar_dir()?;
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if let Some(lt) = parse_sidecar(&path, session_id) {
            return Some(lt);
        }
    }
    None
}

/// Parse one sidecar file. Returns `Some` only when the file's stream belongs
/// to `want_session`.
fn parse_sidecar(path: &Path, want_session: &str) -> Option<LiveThinking> {
    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    let mut file_session: Option<String> = None;
    // thinking text accumulated per content-block index for the *current*
    // message; a `message_start` resets it so we only ever surface the latest
    // turn's reasoning.
    let mut blocks: std::collections::BTreeMap<i64, String> = std::collections::BTreeMap::new();
    let mut last_thinking_index: Option<i64> = None;
    let mut saw_result = false;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Recover the session id from any event that stamps it.
        if file_session.is_none() {
            if let Some(sid) = v.get("session_id").and_then(|s| s.as_str()) {
                file_session = Some(sid.to_string());
            }
        }

        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match ty {
            "result" | "message_stop" => saw_result = true,
            "stream_event" => {
                let event = match v.get("event") {
                    Some(e) => e,
                    None => continue,
                };
                let etype = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match etype {
                    "message_start" => {
                        // New assistant message → drop prior turn's blocks.
                        blocks.clear();
                        last_thinking_index = None;
                        saw_result = false;
                    }
                    "content_block_start" => {
                        let idx = event.get("index").and_then(|i| i.as_i64()).unwrap_or(0);
                        let is_thinking = event
                            .get("content_block")
                            .and_then(|b| b.get("type"))
                            .and_then(|t| t.as_str())
                            == Some("thinking");
                        if is_thinking {
                            blocks.entry(idx).or_default();
                            last_thinking_index = Some(idx);
                        }
                    }
                    "content_block_delta" => {
                        let delta = match event.get("delta") {
                            Some(d) => d,
                            None => continue,
                        };
                        if delta.get("type").and_then(|t| t.as_str()) == Some("thinking_delta") {
                            let idx = event.get("index").and_then(|i| i.as_i64()).unwrap_or(0);
                            if let Some(chunk) = delta.get("thinking").and_then(|t| t.as_str()) {
                                blocks.entry(idx).or_default().push_str(chunk);
                                last_thinking_index = Some(idx);
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let file_session = file_session?;
    if file_session != want_session {
        return None;
    }

    let thinking = last_thinking_index
        .and_then(|idx| blocks.get(&idx).cloned())
        .unwrap_or_default();

    let updated_secs_ago = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(f64::INFINITY);

    // Still streaming if we never saw the terminal result and the file was
    // touched within the streaming window.
    let streaming = !saw_result && updated_secs_ago < 120.0;

    Some(LiveThinking {
        session_id: file_session,
        thinking,
        streaming,
        updated_secs_ago,
    })
}

/// Remove sidecars older than `max_age_secs`. Called on startup so stale files
/// from finished turns don't accumulate. Best-effort: errors are ignored.
pub fn prune_old(max_age_secs: u64) {
    let dir = match sidecar_dir() {
        Some(d) => d,
        None => return,
    };
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let stale = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|d| d.as_secs() > max_age_secs)
            .unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_sidecar(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        p
    }

    #[test]
    fn parses_thinking_deltas_for_matching_session() {
        let tmp = std::env::temp_dir().join(format!("lt-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = write_sidecar(
            &tmp,
            "pid-1.jsonl",
            &[
                r#"{"type":"system","session_id":"abc"}"#,
                r#"{"type":"stream_event","event":{"type":"message_start"},"session_id":"abc"}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}},"session_id":"abc"}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"17 * 23"}},"session_id":"abc"}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":" = 391"}},"session_id":"abc"}"#,
            ],
        );
        let lt = parse_sidecar(&path, "abc").expect("should match");
        assert_eq!(lt.session_id, "abc");
        assert_eq!(lt.thinking, "17 * 23 = 391");
        assert!(lt.streaming, "no result line + fresh file → streaming");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn returns_none_for_non_matching_session() {
        let tmp = std::env::temp_dir().join(format!("lt-test2-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = write_sidecar(
            &tmp,
            "pid-2.jsonl",
            &[r#"{"type":"system","session_id":"other"}"#],
        );
        assert!(parse_sidecar(&path, "abc").is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn message_start_resets_prior_turn() {
        let tmp = std::env::temp_dir().join(format!("lt-test3-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = write_sidecar(
            &tmp,
            "pid-3.jsonl",
            &[
                r#"{"type":"stream_event","event":{"type":"message_start"},"session_id":"s"}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking"}},"session_id":"s"}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"OLD"}},"session_id":"s"}"#,
                r#"{"type":"stream_event","event":{"type":"message_start"},"session_id":"s"}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking"}},"session_id":"s"}"#,
                r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"NEW"}},"session_id":"s"}"#,
            ],
        );
        let lt = parse_sidecar(&path, "s").unwrap();
        assert_eq!(lt.thinking, "NEW");
        let _ = std::fs::remove_file(&path);
    }
}
