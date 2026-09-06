//! Session history — read-only recovery of a session's *own* transcript (and
//! its handoff predecessors') after context compaction.
//!
//! Analogue of Codex's `history` tool namespace (`ext/history-notes`), which
//! searches a server-side copy of the conversation by literal substring and
//! reads one item back by id. Fleet already has the data locally: the Claude
//! Code transcript under `~/.claude/projects/<cwd>/<session_id>.jsonl`, and a
//! SQLite FTS5 index over it ([`crate::search_index`]). This module scopes that
//! index to the calling session — plus the sessions it inherited through a
//! handoff relay — and adds "read this record back" by line number, which the
//! desktop's per-session-collapsed search never needed.
//!
//! Scope is the same as [`crate::session_notes::readable_sessions`]: the
//! session itself and its relay predecessors, never its successors.
//!
//! # Coverage (stated, not hidden)
//!
//! - Claude Code transcripts only. The index does not parse Codex rollouts, so
//!   a Codex session gets an explicit "no transcript indexed" error rather than
//!   silently empty results.
//! - The transcript must be on this machine. An rca remote-workspace session's
//!   transcript lives on the remote host and is reported the same way.
//! - Search covers what the desktop search indexes: user/assistant text,
//!   thinking, tool names and the AI title. `read` renders the full record,
//!   including tool inputs and tool results, so a hit can be expanded.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::search_index::SearchIndex;

/// Characters returned by `read` when the caller gives no limit — the same
/// ceiling Codex puts on a `thread_hint`, so one read stays a bounded cost.
pub const DEFAULT_READ_CHARS: usize = 4_000;

/// One matching transcript record.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryHit {
    pub session_id: String,
    /// 1-based line number of the record in its session's transcript. Pass it
    /// back to `read` together with `session_id`.
    pub line_no: i64,
    pub snippet: String,
    pub rank: f64,
}

/// `(session_id, transcript path)` for every readable session whose transcript
/// exists on this machine, in scope order (own first).
fn transcripts_for(readable: &[String]) -> Vec<(String, PathBuf)> {
    readable
        .iter()
        .filter_map(|sid| crate::session::find_session_jsonl(sid).map(|p| (sid.clone(), p)))
        .collect()
}

fn no_transcript_error(session_id: &str) -> String {
    format!(
        "no transcript found for session {session_id} under ~/.claude/projects — history \
         recovery covers Claude Code sessions whose transcript is on this machine (not Codex \
         sessions, not rca remote workspaces)"
    )
}

/// Full-text search over the calling session's transcript and its relay
/// predecessors'. Re-indexes those transcripts incrementally first, so a record
/// written seconds ago is findable even when nothing else keeps the index warm.
pub fn search(session_id: &str, query: &str, limit: usize) -> Result<Vec<HistoryHit>, String> {
    let readable = crate::session_notes::readable_sessions(session_id);
    let transcripts = transcripts_for(&readable);
    if transcripts.is_empty() {
        return Err(no_transcript_error(session_id));
    }
    let index = SearchIndex::open().map_err(|e| format!("cannot open search index: {e}"))?;
    search_with(&index, &transcripts, query, limit)
}

/// Pure core of [`search`]: `transcripts` are the `(session_id, path)` pairs in
/// scope; `index` is any open [`SearchIndex`].
pub fn search_with(
    index: &SearchIndex,
    transcripts: &[(String, PathBuf)],
    query: &str,
    limit: usize,
) -> Result<Vec<HistoryHit>, String> {
    if query.trim().is_empty() {
        return Err("search query is empty".to_string());
    }
    let mut paths = Vec::with_capacity(transcripts.len());
    for (sid, path) in transcripts {
        let p = path.to_string_lossy().into_owned();
        if let Err(e) = index.index_session(&p, sid) {
            crate::log_debug(&format!("history: incremental index of {p} failed: {e}"));
        }
        paths.push(p);
    }
    let hits = index.search_scoped(&paths, query, limit)?;
    Ok(hits
        .into_iter()
        .map(|h| HistoryHit {
            session_id: h.session_id,
            line_no: h.line_no,
            snippet: h.snippet,
            rank: h.rank,
        })
        .collect())
}

/// Read one transcript record back. `target_session` defaults to the caller
/// and must be in the caller's readable scope. `line_no` is the 1-based number
/// a search hit reported. Output is `limit_chars` characters (default
/// [`DEFAULT_READ_CHARS`]) starting at `offset_chars`, with a continuation
/// marker when more remains.
pub fn read(
    session_id: &str,
    target_session: Option<&str>,
    line_no: i64,
    offset_chars: usize,
    limit_chars: Option<usize>,
) -> Result<String, String> {
    let readable = crate::session_notes::readable_sessions(session_id);
    let target = ensure_readable(&readable, target_session.unwrap_or(session_id))?;
    let path = crate::session::find_session_jsonl(target).ok_or_else(|| no_transcript_error(target))?;
    read_record(&path, line_no, offset_chars, limit_chars.unwrap_or(DEFAULT_READ_CHARS))
}

/// Refuse a target outside the caller's scope (a successor's transcript, or an
/// unrelated session), returning the target when it is allowed.
pub fn ensure_readable<'a>(readable: &[String], target: &'a str) -> Result<&'a str, String> {
    if readable.iter().any(|s| s == target) {
        Ok(target)
    } else {
        Err(format!(
            "session {target} is not in this session's history scope (own session and handoff \
             predecessors only)"
        ))
    }
}

/// Pure core of [`read`]: fetch line `line_no` of `path`, render the record,
/// and slice `[offset_chars, offset_chars + limit_chars)`.
pub fn read_record(
    path: &Path,
    line_no: i64,
    offset_chars: usize,
    limit_chars: usize,
) -> Result<String, String> {
    if line_no < 1 {
        return Err(format!("line_no must be ≥ 1, got {line_no}"));
    }
    let file = fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let line = BufReader::new(file)
        .lines()
        .nth((line_no - 1) as usize)
        .ok_or_else(|| format!("transcript has fewer than {line_no} lines"))?
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    let val: Value = serde_json::from_str(&line)
        .map_err(|_| format!("line {line_no} is not a transcript record"))?;
    let text = render_record(&val);
    Ok(slice_chars(&text, offset_chars, limit_chars))
}

/// Render a transcript record for an agent reading it back: a role header, the
/// searchable text, and — unlike the index — tool inputs and tool results, so
/// "what did that command print" is answerable from a hit on the command.
pub(crate) fn render_record(val: &Value) -> String {
    let kind = val["type"].as_str().unwrap_or("record");
    let role = val["message"]["role"].as_str().unwrap_or(kind);
    let mut out = format!("[{role}]");
    if let Some(ts) = val["timestamp"].as_str() {
        out.push_str(&format!(" {ts}"));
    }
    out.push('\n');

    let body = crate::search_index::extract_searchable_text(val);
    if !body.is_empty() {
        out.push_str(&body);
        out.push('\n');
    }
    if let Some(blocks) = val["message"]["content"].as_array() {
        for block in blocks {
            match block["type"].as_str() {
                Some("tool_use") => {
                    let name = block["name"].as_str().unwrap_or("tool");
                    let input = block["input"].to_string();
                    out.push_str(&format!("<tool_use name=\"{name}\">{input}</tool_use>\n"));
                }
                Some("tool_result") => {
                    let content = tool_result_text(&block["content"]);
                    let err = if block["is_error"].as_bool().unwrap_or(false) {
                        " is_error=\"true\""
                    } else {
                        ""
                    };
                    out.push_str(&format!("<tool_result{err}>{content}</tool_result>\n"));
                }
                _ => {}
            }
        }
    }
    // Records the index ignores (progress, summary, system…) still have a body
    // worth a glance when read explicitly.
    if out.trim_end().ends_with(']') || out.lines().count() <= 1 {
        if let Some(s) = val["summary"].as_str() {
            out.push_str(s);
            out.push('\n');
        } else if val.get("message").is_none() {
            out.push_str(&val.to_string());
            out.push('\n');
        }
    }
    out
}

fn tool_result_text(content: &Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    content
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|b| match b["type"].as_str() {
                    Some("text") => b["text"].as_str().map(str::to_string),
                    Some("image") => Some("[image]".to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Character-based window with a continuation marker, so an agent can page
/// through a long record without ever pulling the whole thing into context.
fn slice_chars(text: &str, offset_chars: usize, limit_chars: usize) -> String {
    let total = text.chars().count();
    if offset_chars >= total {
        return String::new();
    }
    let limit = limit_chars.max(1);
    let window: String = text.chars().skip(offset_chars).take(limit).collect();
    let end = offset_chars + window.chars().count();
    if end < total {
        format!(
            "{window}\n… [{} more chars; pass offset_chars={end} to continue]",
            total - end
        )
    } else {
        window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fleet-history-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    const OWN: &str = concat!(
        "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"zqhist please fix the tokenizer\"}}\n",
        "{\"type\":\"progress\"}\n",
        "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[",
        "{\"type\":\"text\",\"text\":\"looking at zqhist now\"},",
        "{\"type\":\"tool_use\",\"name\":\"Bash\",\"input\":{\"command\":\"cargo test\"}}]}}\n",
        "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[",
        "{\"type\":\"tool_result\",\"is_error\":true,\"content\":\"error[E0425]: cannot find value zqfail\"}]}}\n",
    );

    #[test]
    fn search_is_scoped_and_hits_carry_line_numbers() {
        let dir = fresh_dir("search");
        let own = dir.join("own.jsonl");
        let pred = dir.join("pred.jsonl");
        let other = dir.join("other.jsonl");
        fs::write(&own, OWN).unwrap();
        fs::write(&pred, "{\"type\":\"user\",\"message\":{\"content\":\"zqhist from the predecessor\"}}\n").unwrap();
        fs::write(&other, "{\"type\":\"user\",\"message\":{\"content\":\"zqhist unrelated session\"}}\n").unwrap();

        let idx = SearchIndex::open_at(&dir.join("idx.db")).unwrap();
        // The unrelated transcript is indexed too, and must never surface.
        idx.index_session(other.to_str().unwrap(), "other").unwrap();

        let scope = vec![("own".to_string(), own.clone()), ("pred".to_string(), pred.clone())];
        let mut hits = search_with(&idx, &scope, "zqhist", 10).unwrap();
        hits.sort_by(|a, b| a.session_id.cmp(&b.session_id).then(a.line_no.cmp(&b.line_no)));
        let located: Vec<(&str, i64)> = hits.iter().map(|h| (h.session_id.as_str(), h.line_no)).collect();
        assert_eq!(located, vec![("own", 1), ("own", 3), ("pred", 1)], "{hits:?}");
        assert!(search_with(&idx, &scope, "   ", 10).is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_renders_tool_use_and_tool_result_and_pages() {
        let dir = fresh_dir("read");
        let own = dir.join("own.jsonl");
        fs::write(&own, OWN).unwrap();

        let l3 = read_record(&own, 3, 0, 10_000).unwrap();
        assert!(l3.starts_with("[assistant]"), "{l3}");
        assert!(l3.contains("looking at zqhist now"));
        assert!(l3.contains("<tool_use name=\"Bash\">{\"command\":\"cargo test\"}</tool_use>"), "{l3}");

        let l4 = read_record(&own, 4, 0, 10_000).unwrap();
        assert!(l4.contains("<tool_result is_error=\"true\">error[E0425]: cannot find value zqfail</tool_result>"), "{l4}");

        // Paging: a 10-char window carries a continuation marker with the next offset.
        let page = read_record(&own, 1, 0, 10).unwrap();
        assert!(page.starts_with("[user]\nzqh"), "{page}");
        assert!(page.contains("pass offset_chars=10 to continue"), "{page}");
        let rest = read_record(&own, 1, 10, 10_000).unwrap();
        assert!(!rest.contains("to continue"));
        assert!(rest.ends_with("tokenizer\n"), "{rest:?}");

        // A non-message record is still readable rather than blank.
        let l2 = read_record(&own, 2, 0, 10_000).unwrap();
        assert!(l2.contains("progress"), "{l2}");

        assert!(read_record(&own, 0, 0, 10).is_err());
        assert!(read_record(&own, 99, 0, 10).is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scope_check_refuses_sessions_outside_the_chain() {
        let readable = vec!["me".to_string(), "pred".to_string()];
        assert_eq!(ensure_readable(&readable, "me").unwrap(), "me");
        assert_eq!(ensure_readable(&readable, "pred").unwrap(), "pred");
        assert!(ensure_readable(&readable, "successor").is_err());
    }

    #[test]
    fn slice_chars_is_char_safe_for_cjk() {
        let s = "决策卡走 fleet 工具"; // 13 chars
        assert_eq!(slice_chars(s, 0, 3), "决策卡\n… [10 more chars; pass offset_chars=3 to continue]");
        assert_eq!(slice_chars(s, 3, 100), "走 fleet 工具");
        assert_eq!(slice_chars(s, 100, 5), "");
    }
}
