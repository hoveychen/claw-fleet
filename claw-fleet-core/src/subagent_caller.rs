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

/// Shown to a subagent that tried to call one of the parent-scoped tools.
/// `tool` is the tool name, `effect` one clause naming what it would have done
/// to the parent.
pub fn subagent_tool_refusal(tool: &str, effect: &str, agent_type: &str) -> String {
    format!(
        "{tool} is not available to subagents (detected agent type: `{agent_type}`). You were \
         spawned by the Agent/Task tool and share your parent's session id, so this call would \
         have {effect} — not yours. Do not retry. If your parent needs it done, say so in your \
         final text result and let the parent make the call."
    )
}

/// `Some(agent_type)` when this `fleet__ask` call came from a subagent of
/// `session_id`, `None` when it came from the session itself (or when the
/// transcript cannot be read — see the fail-open note on the module).
///
/// `first_question` is the `question` string of the call's first question, which
/// is what gets compared against the transcript.
pub fn detect_ask_caller(session_id: &str, first_question: &str) -> Option<String> {
    let question = first_question.to_string();
    detect_caller(session_id, "fleet__ask", move |input| {
        input.pointer("/questions/0/question").and_then(|q| q.as_str()) == Some(question.as_str())
    })
}

/// `Some(agent_type)` when a subagent of `session_id` just called the tool whose
/// name ends with `tool_suffix` with exactly these `arguments`.
///
/// Used for the parent-scoped tools that carry no long unique payload the way a
/// card's report body does — `fleet__set_session_title` renames the parent
/// session, `fleet__plan` moves the parent's plan focus. Their arguments are
/// short, so this matches the whole input object (order-insensitive: this
/// workspace's `serde_json` keeps `Map` as a `BTreeMap`). Two identical calls
/// from parent and subagent inside the same freshness window would collide, and
/// then the parent's own call is the one refused — recoverable, unlike the
/// silent cross-session write it replaces. Real data agrees it is not a live
/// concern: across 03f41a3c's 26 `fleet__plan` and 4 `fleet__set_session_title`
/// calls, no subagent's arguments equalled any of the parent's, and the parent
/// never repeated a plan/title payload even once.
pub fn detect_tool_caller(
    session_id: &str,
    tool_suffix: &str,
    arguments: &serde_json::Value,
) -> Option<String> {
    let want = arguments.clone();
    detect_caller(session_id, tool_suffix, move |input| input == &want)
}

fn detect_caller(
    session_id: &str,
    tool_suffix: &str,
    matches: impl Fn(&serde_json::Value) -> bool,
) -> Option<String> {
    let parent_jsonl = crate::session::find_session_jsonl(session_id)?;
    let dir = parent_jsonl.with_extension("").join("subagents");
    detect_in_subagents_dir(&dir, tool_suffix, &matches)
}

/// Pure core of [`detect_caller`]: scan one `subagents/` directory. Split out so
/// tests can build a directory instead of a whole `~/.claude` tree.
pub(crate) fn detect_in_subagents_dir(
    dir: &Path,
    tool_suffix: &str,
    matches: &dyn Fn(&serde_json::Value) -> bool,
) -> Option<String> {
    detect_in_subagents_dir_within(dir, tool_suffix, matches, MAX_AGE)
}

/// [`detect_in_subagents_dir`] with the freshness window injected, so a test can
/// exercise the prefilter without back-dating a file's mtime.
pub(crate) fn detect_in_subagents_dir_within(
    dir: &Path,
    tool_suffix: &str,
    matches: &dyn Fn(&serde_json::Value) -> bool,
    max_age: std::time::Duration,
) -> Option<String> {
    for transcript in agent_transcripts(dir) {
        if !recently_written(&transcript, max_age) {
            continue;
        }
        if tail_has_call(&transcript, tool_suffix, matches, max_age) {
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

/// Does the tail of `path` hold a matching `tool_use` written within `max_age`?
///
/// The record's own `timestamp` is checked as well as the file's mtime: an agent
/// transcript stays fresh for as long as its agent keeps writing anything, so
/// without this a matching call from ten minutes ago would still count.
fn tail_has_call(
    path: &Path,
    tool_suffix: &str,
    matches: &dyn Fn(&serde_json::Value) -> bool,
    max_age: std::time::Duration,
) -> bool {
    let Some(tail) = read_tail(path, TAIL_BYTES) else {
        return false;
    };
    tail.lines()
        .rev()
        .take(MAX_LINES)
        .any(|line| line_calls(line, tool_suffix, matches, max_age))
}

fn line_calls(
    line: &str,
    tool_suffix: &str,
    matches: &dyn Fn(&serde_json::Value) -> bool,
    max_age: std::time::Duration,
) -> bool {
    let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };
    if !record_is_recent(&record, max_age) {
        return false;
    }
    let Some(blocks) = record.pointer("/message/content").and_then(|c| c.as_array()) else {
        return false;
    };
    blocks.iter().any(|b| {
        b.get("type").and_then(|t| t.as_str()) == Some("tool_use")
            && b.get("name")
                .and_then(|n| n.as_str())
                .is_some_and(|n| n.ends_with(tool_suffix))
            && b.get("input").is_some_and(|i| matches(i))
    })
}

/// A record with no parsable `timestamp` counts as recent: the mtime prefilter
/// already bounded it, and refusing to detect would fail toward the bug.
fn record_is_recent(record: &serde_json::Value, max_age: std::time::Duration) -> bool {
    let Some(ts) = record.get("timestamp").and_then(|t| t.as_str()) else {
        return true;
    };
    let Ok(when) = chrono::DateTime::parse_from_rfc3339(ts) else {
        return true;
    };
    let age = chrono::Utc::now().signed_duration_since(when.with_timezone(&chrono::Utc));
    age <= chrono::Duration::from_std(max_age).unwrap_or(chrono::Duration::MAX)
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

    fn asked(question: &str) -> impl Fn(&serde_json::Value) -> bool + '_ {
        move |input: &serde_json::Value| {
            input.pointer("/questions/0/question").and_then(|q| q.as_str()) == Some(question)
        }
    }

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
            detect_in_subagents_dir(&dir, "fleet__ask", &asked("已合进 main（未 push）")),
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
            detect_in_subagents_dir(&dir, "fleet__ask", &asked("q from a workflow agent")),
            Some("Explore".to_string())
        );
    }

    #[test]
    fn missing_meta_json_still_reports_a_subagent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        write_agent(&dir, "abc", "no sidecar here", None);
        assert_eq!(detect_in_subagents_dir(&dir, "fleet__ask", &asked("no sidecar here")), Some("subagent".to_string()));
    }

    #[test]
    fn a_question_no_subagent_asked_is_the_parents_own() {
        // The fail-open case that must stay open: the parent raises 17 of its
        // own cards while five subagents are alive, and not one of them may be
        // mistaken for a sidechain call.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        write_agent(&dir, "abc", "the subagent's card", Some("general-purpose"));
        assert_eq!(detect_in_subagents_dir(&dir, "fleet__ask", &asked("the parent's own card")), None);
    }

    #[test]
    fn no_subagents_dir_is_not_a_subagent() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(detect_in_subagents_dir(&tmp.path().join("subagents"), "fleet__ask", &asked("anything")), None);
    }

    #[test]
    fn a_long_finished_agent_transcript_is_ignored() {
        // Freshness window of zero stands in for "this file was written long
        // ago": the text still matches, and the prefilter still rejects it.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        write_agent(&dir, "old", "stale card text", Some("general-purpose"));
        assert_eq!(detect_in_subagents_dir(&dir, "fleet__ask", &asked("stale card text")), Some("general-purpose".into()));
        assert_eq!(
            detect_in_subagents_dir_within(&dir, "fleet__ask", &asked("stale card text"), std::time::Duration::ZERO),
            None
        );
    }

    fn tool_line(name: &str, input: serde_json::Value, ts: Option<&str>) -> String {
        let mut record = serde_json::json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "name": name, "input": input}]}
        });
        if let Some(ts) = ts {
            record["timestamp"] = serde_json::json!(ts);
        }
        record.to_string()
    }

    /// The three parent-scoped calls the 03f41a3c subagents actually made:
    /// `fleet__set_session_title` (three of the five renamed the parent) and two
    /// `fleet__plan` calls. Whole-input equality has to catch them.
    #[test]
    fn parent_scoped_tool_calls_are_matched_on_whole_input() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        std::fs::create_dir_all(&dir).unwrap();
        let title = serde_json::json!({"title": "datahub-knowledge-api：knowledge 端点搬进 datahub gateway"});
        let plan = serde_json::json!({"action": "resume", "plan_id": "datahub-knowledge-api", "task": "P1"});
        std::fs::write(
            dir.join("agent-e.jsonl"),
            format!(
                "{}\n{}\n",
                tool_line("mcp__fleet__fleet__set_session_title", title.clone(), None),
                tool_line("mcp__fleet__fleet__plan", plan.clone(), None)
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("agent-e.meta.json"),
            serde_json::json!({"agentType": "general-purpose"}).to_string(),
        )
        .unwrap();

        let eq = |want: serde_json::Value| move |got: &serde_json::Value| got == &want;
        assert_eq!(
            detect_in_subagents_dir(&dir, "fleet__set_session_title", &eq(title)),
            Some("general-purpose".to_string())
        );
        assert_eq!(
            detect_in_subagents_dir(&dir, "fleet__plan", &eq(plan)),
            Some("general-purpose".to_string())
        );
        // A different plan payload — the parent's own call — must not match.
        assert_eq!(
            detect_in_subagents_dir(
                &dir,
                "fleet__plan",
                &eq(serde_json::json!({"action": "check", "plan_id": "other", "task": "P2"}))
            ),
            None
        );
    }

    #[test]
    fn a_matching_call_from_an_hour_ago_does_not_count() {
        // An agent transcript stays mtime-fresh while its agent keeps writing,
        // so the record's own timestamp is what bounds a low-entropy payload
        // like `{"action":"list"}` to the call actually in flight.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("subagents");
        std::fs::create_dir_all(&dir).unwrap();
        let input = serde_json::json!({"action": "list"});
        let old = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        let now = chrono::Utc::now().to_rfc3339();
        let eq = |want: serde_json::Value| move |got: &serde_json::Value| got == &want;

        std::fs::write(
            dir.join("agent-x.jsonl"),
            format!("{}\n", tool_line("mcp__fleet__fleet__plan", input.clone(), Some(&old))),
        )
        .unwrap();
        assert_eq!(detect_in_subagents_dir(&dir, "fleet__plan", &eq(input.clone())), None);

        std::fs::write(
            dir.join("agent-x.jsonl"),
            format!("{}\n", tool_line("mcp__fleet__fleet__plan", input.clone(), Some(&now))),
        )
        .unwrap();
        assert_eq!(
            detect_in_subagents_dir(&dir, "fleet__plan", &eq(input)),
            Some("subagent".to_string())
        );
    }

    #[test]
    fn refusal_names_the_tool_the_effect_and_the_agent_type() {
        let msg = subagent_tool_refusal(
            "fleet__set_session_title",
            "renamed your PARENT session",
            "general-purpose",
        );
        assert!(msg.contains("fleet__set_session_title"));
        assert!(msg.contains("renamed your PARENT session"));
        assert!(msg.contains("general-purpose"));
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
