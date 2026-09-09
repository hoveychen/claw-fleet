//! "Did a subagent make this call?" — answered from the transcript, because the
//! MCP server cannot tell otherwise.
//!
//! The `fleet` MCP server is one stdio child per Claude CLI *process*, and its
//! session id comes from the `CLAUDE_CODE_SESSION_ID` captured in that child's
//! env ([`crate::codex_launch::resolve_fleet_session_id_from_env`]). A subagent
//! (Agent/Task tool sidechain) runs inside the same CLI process and routes its
//! tool calls to that same server child, so every call it makes arrives stamped
//! with the **parent's** session id and is otherwise indistinguishable from one
//! the parent made itself.
//!
//! That is how, on 2026-09-08, session `03f41a3c` ended up with three decision
//! cards raised by its `general-purpose` subagent 「datahub knowledge API 线」:
//! the cards were filed under the parent's id, the boss pressed 「结束任务」 on
//! one of them, the parent's task was recorded complete while it was still
//! working, and the `TASK FINISHED` string went back to the *subagent*, which
//! then reported its whole day's work as the four characters 「收工，老板」.
//!
//! The one signal that does distinguish them is the transcript. Claude Code
//! flushes each assistant block to disk before the tool call is dispatched (the
//! subagent's `tool_use` line was written ~50 ms before the ask request file
//! appeared), and a subagent's blocks go to
//! `~/.claude/projects/<proj>/<parent-uuid>/subagents/agent-<id>.jsonl`, never
//! to the parent's `<parent-uuid>.jsonl`. So: look for a freshly written
//! `fleet__ask` `tool_use` under `subagents/` whose first question is byte-equal
//! to the one being asked. Verified against the real 03f41a3c data — of its 20
//! `fleet__ask` records, 17 matched the parent transcript only and 3 matched
//! exactly one subagent file; nothing matched both. Replaying that history
//! through this module finds 2 of the 3, because the oldest one sits 960 KB back
//! in a since-grown transcript; at call time it was the last line written, which
//! is the only position this gate ever has to look at.
//!
//! Fails open by design: no match anywhere means "treat it as the parent". A
//! missed detection costs the old behaviour, whereas a false positive would
//! block a legitimate card.

use std::path::{Path, PathBuf};

/// How stale a subagent transcript may be and still be considered the caller.
/// The flush happens milliseconds before the call, so this is only a cheap
/// prefilter that keeps a long-finished agent's file out of the comparison.
const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(120);

/// Bytes read from the tail of each candidate transcript. Agent transcripts run
/// to megabytes; the block we want is the last one written.
const TAIL_BYTES: u64 = 512 * 1024;

/// Lines scanned backwards from the tail.
const MAX_LINES: usize = 200;

/// Shown to a subagent that tried to raise a decision card.
pub const SUBAGENT_ASK_REFUSAL: &str = "\
fleet__ask is not available to subagents. You were spawned by the Agent/Task tool, and Fleet's \
decision cards belong to the session that owns the transcript: a card you raise is filed under \
your PARENT's session id, and the terminal button on it closes the PARENT's task — pressing it \
also hands you back `TASK FINISHED`, cutting your report short. Do not retry, and do not call \
fleet__plan or fleet__set_session_title either. Return everything you would have put on the card \
— the report, the options, the question — as your final text result. Your parent reads it and \
decides whether it warrants a card.";

/// `Some(agent_type)` when this `fleet__ask` call came from a subagent of
/// `session_id`, `None` when it came from the session itself (or when the
/// transcript cannot be read — see the fail-open note on the module).
///
/// `first_question` is the `question` string of the call's first question, which
/// is what gets compared against the transcript.
pub fn detect_ask_caller(session_id: &str, first_question: &str) -> Option<String> {
    let parent_jsonl = crate::session::find_session_jsonl(session_id)?;
    let dir = parent_jsonl.with_extension("").join("subagents");
    detect_in_subagents_dir(&dir, first_question)
}

/// Pure core of [`detect_ask_caller`]: scan one `subagents/` directory. Split
/// out so tests can build a directory instead of a whole `~/.claude` tree.
pub(crate) fn detect_in_subagents_dir(dir: &Path, first_question: &str) -> Option<String> {
    detect_in_subagents_dir_within(dir, first_question, MAX_AGE)
}

/// [`detect_in_subagents_dir`] with the freshness window injected, so a test can
/// exercise the prefilter without back-dating a file's mtime.
pub(crate) fn detect_in_subagents_dir_within(
    dir: &Path,
    first_question: &str,
    max_age: std::time::Duration,
) -> Option<String> {
    for transcript in agent_transcripts(dir) {
        if !recently_written(&transcript, max_age) {
            continue;
        }
        if tail_has_ask(&transcript, first_question) {
            return Some(agent_type_of(&transcript));
        }
    }
    None
}

/// Direct subagents (`subagents/agent-*.jsonl`) plus Claude Code Workflow
/// fan-out agents (`subagents/workflows/wf_*/agent-*.jsonl`) — both are
/// sidechains and both must be caught.
fn agent_transcripts(dir: &Path) -> Vec<PathBuf> {
    let mut out = agent_jsonls_in(dir);
    if let Ok(runs) = std::fs::read_dir(dir.join("workflows")) {
        for run in runs.filter_map(|e| e.ok()) {
            out.extend(agent_jsonls_in(&run.path()));
        }
    }
    out
}

fn agent_jsonls_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                return false;
            };
            name.starts_with("agent-") && name.ends_with(".jsonl")
        })
        .collect()
}

fn recently_written(path: &Path, max_age: std::time::Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().map(|age| age <= max_age).unwrap_or(true))
        .unwrap_or(false)
}

/// Does the tail of `path` hold a `fleet__ask` `tool_use` whose first question
/// is `question`? Matched on the full question string: it carries the card's
/// whole report body, so an accidental collision between two different calls is
/// not a realistic concern.
fn tail_has_ask(path: &Path, question: &str) -> bool {
    let Some(tail) = read_tail(path, TAIL_BYTES) else {
        return false;
    };
    tail.lines()
        .rev()
        .take(MAX_LINES)
        .any(|line| line_asks(line, question))
}

fn line_asks(line: &str, question: &str) -> bool {
    let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    let Some(blocks) = record.pointer("/message/content").and_then(|c| c.as_array()) else {
        return false;
    };
    blocks.iter().any(|b| {
        b.get("type").and_then(|t| t.as_str()) == Some("tool_use")
            && b.get("name")
                .and_then(|n| n.as_str())
                .is_some_and(|n| n.ends_with("fleet__ask"))
            && b.pointer("/input/questions/0/question").and_then(|q| q.as_str()) == Some(question)
    })
}

/// Last `bytes` of the file as UTF-8 (lossy), with any partial leading line
/// dropped so every line handed to the parser is whole.
fn read_tail(path: &Path, bytes: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let from = len.saturating_sub(bytes);
    f.seek(SeekFrom::Start(from)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    if from == 0 {
        return Some(text);
    }
    Some(text.split_once('\n').map(|(_, rest)| rest.to_string()).unwrap_or_default())
}

/// The `agentType` Claude Code records in the sidecar `agent-<id>.meta.json`
/// (the `subagent_type` argument of the Agent/Task call). Falls back to a
/// generic label when the sidecar is missing.
fn agent_type_of(transcript: &Path) -> String {
    let meta = transcript.with_extension("meta.json");
    std::fs::read_to_string(meta)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("agentType").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_else(|| "subagent".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask_line(question: &str) -> String {
        serde_json::json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use",
                "name": "mcp__fleet__fleet__ask",
                "input": {"questions": [{"question": question, "header": "h"}]}
            }]}
        })
        .to_string()
    }

    fn write_agent(dir: &Path, id: &str, question: &str, agent_type: Option<&str>) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join(format!("agent-{id}.jsonl")),
            format!("{}\n{}\n", ask_line("some earlier card"), ask_line(question)),
        )
        .unwrap();
        if let Some(t) = agent_type {
            std::fs::write(
                dir.join(format!("agent-{id}.meta.json")),
                serde_json::json!({"agentType": t, "spawnDepth": 1}).to_string(),
            )
            .unwrap();
        }
    }

    #[test]
    fn matching_question_in_subagent_transcript_names_the_agent_type() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        write_agent(&dir, "abc", "已合进 main（未 push）", Some("general-purpose"));
        assert_eq!(
            detect_in_subagents_dir(&dir, "已合进 main（未 push）"),
            Some("general-purpose".to_string())
        );
    }

    #[test]
    fn workflow_fanout_agents_are_caught_too() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        std::fs::create_dir_all(&dir).unwrap();
        write_agent(&dir.join("workflows").join("wf_run1"), "xyz", "q from a workflow agent", Some("Explore"));
        assert_eq!(
            detect_in_subagents_dir(&dir, "q from a workflow agent"),
            Some("Explore".to_string())
        );
    }

    #[test]
    fn missing_meta_json_still_reports_a_subagent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        write_agent(&dir, "abc", "no sidecar here", None);
        assert_eq!(detect_in_subagents_dir(&dir, "no sidecar here"), Some("subagent".to_string()));
    }

    #[test]
    fn a_question_no_subagent_asked_is_the_parents_own() {
        // The fail-open case that must stay open: the parent raises 17 of its
        // own cards while five subagents are alive, and not one of them may be
        // mistaken for a sidechain call.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        write_agent(&dir, "abc", "the subagent's card", Some("general-purpose"));
        assert_eq!(detect_in_subagents_dir(&dir, "the parent's own card"), None);
    }

    #[test]
    fn no_subagents_dir_is_not_a_subagent() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(detect_in_subagents_dir(&tmp.path().join("subagents"), "anything"), None);
    }

    #[test]
    fn a_long_finished_agent_transcript_is_ignored() {
        // Freshness window of zero stands in for "this file was written long
        // ago": the text still matches, and the prefilter still rejects it.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        write_agent(&dir, "old", "stale card text", Some("general-purpose"));
        assert_eq!(detect_in_subagents_dir(&dir, "stale card text"), Some("general-purpose".into()));
        assert_eq!(
            detect_in_subagents_dir_within(&dir, "stale card text", std::time::Duration::ZERO),
            None
        );
    }

    #[test]
    fn tail_read_drops_the_partial_first_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.jsonl");
        std::fs::write(&path, "AAAA\nBBBB\nCCCC\n").unwrap();
        // 9 bytes = "BB\nCCCC\n" plus one byte of the BBBB line → that partial
        // line must be dropped, leaving only the whole ones.
        assert_eq!(read_tail(&path, 9).unwrap(), "CCCC\n");
    }
}
