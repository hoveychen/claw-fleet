//! Skip re-injecting the TASKS.md reminder when the copy already in front of
//! the model is byte-identical.
//!
//! [`crate::prd_tasks::render_active_plans_reminder`] is re-rendered once per
//! turn on every client: Claude through the `fleet prd-context` UserPromptSubmit
//! hook, Codex by prepending it to the prompt of each `codex exec` /
//! `codex exec resume`. Neither compared the new text with the previous one, so
//! a workspace whose TASKS.md never moved still paid 5–12 KB of context per
//! turn — measured worst cases: 21 copies / 156 KB in one Claude session,
//! 9 copies / 106 KB in one Codex rollout. The dsh plugin already dedupes this
//! way (`latestInjectedText` in `dsh-plugin/index.js`); this module gives the
//! other two clients the same behaviour with one shared policy.
//!
//! # Policy
//!
//! Walk the session log **backwards** and stop at the first record that is
//! either a previously injected reminder or a compaction boundary:
//!
//! - identical reminder first → it is still in front of the model, skip;
//! - a different reminder first → the plans moved, inject;
//! - a compaction boundary first → whatever was injected before it has been
//!   summarised away, inject;
//! - nothing found (tail window, unreadable file, foreign format) → inject.
//!
//! Every failure mode therefore lands on "inject", which is exactly today's
//! behaviour: the dedupe can cost a wasted copy, never a missing one.
//!
//! Reading a bounded tail is safe for the same reason. If the window starts
//! *after* a compaction boundary, that boundary precedes every record in the
//! window, so any reminder found there is post-compaction and genuinely still
//! in context.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

/// How much of the session log's tail to inspect. One turn of tool output can
/// be megabytes, and missing the previous copy only costs a redundant
/// injection, so this is a cost ceiling rather than a correctness bound.
const TAIL_BYTES: u64 = 1024 * 1024;

/// What a backwards scan found first.
enum Probe {
    /// A previously injected reminder, with its text.
    Reminder(String),
    /// A compaction boundary: earlier injections are no longer in context.
    Compacted,
}

/// Shared verdict for both formats. `probe` yields the newest relevant record.
fn needs_injection(probe: Option<Probe>, reminder: &str) -> bool {
    match probe {
        Some(Probe::Reminder(prev)) => prev.trim() != reminder.trim(),
        Some(Probe::Compacted) | None => true,
    }
}

/// Read the bounded tail of `path`, dropping the leading partial line.
fn read_tail(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = String::new();
    // A tail cut mid-UTF-8 fails to decode; treat it as "no evidence".
    file.read_to_string(&mut tail).ok()?;
    if start > 0 {
        tail = tail.split_once('\n')?.1.to_string();
    }
    Some(tail)
}

/// Whether the reminder must enter this Claude session, given the transcript
/// Claude Code passes to the hook as `transcript_path`.
pub fn claude_needs_injection(transcript_path: &Path, reminder: &str) -> bool {
    needs_injection(probe_claude(transcript_path), reminder)
}

fn probe_claude(transcript_path: &Path) -> Option<Probe> {
    let tail = read_tail(transcript_path)?;
    for line in tail.lines().rev() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        // A subagent's records share the parent transcript only through
        // `isSidechain`; its context is a different window entirely.
        if value
            .get("isSidechain")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        if is_claude_compaction(&value) {
            return Some(Probe::Compacted);
        }
        if let Some(text) = claude_injected_reminder(&value) {
            return Some(Probe::Reminder(text));
        }
    }
    None
}

/// Claude Code writes two records around a compaction: a `system` event with
/// `subtype: "compact_boundary"` and the summary itself as a user record
/// flagged `isCompactSummary`. Either one ends the scan.
fn is_claude_compaction(value: &Value) -> bool {
    if value
        .get("isCompactSummary")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }
    value.get("type").and_then(Value::as_str) == Some("system")
        && value.get("subtype").and_then(Value::as_str) == Some("compact_boundary")
}

/// The hook's output is persisted as an `attachment` record of type
/// `hook_additional_context`, whose `content` is a list of strings holding the
/// `additionalContext` verbatim.
fn claude_injected_reminder(value: &Value) -> Option<String> {
    if value.get("type").and_then(Value::as_str) != Some("attachment") {
        return None;
    }
    let attachment = value.get("attachment")?;
    if attachment.get("type").and_then(Value::as_str) != Some("hook_additional_context") {
        return None;
    }
    let text = match attachment.get("content")? {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(""),
        _ => return None,
    };
    // Other Fleet hooks (notes hint, guard) use the same record type; only a
    // plan reminder can answer for a plan reminder.
    is_plan_reminder(&text).then_some(text)
}

/// Whether the reminder must be prepended to this Codex turn's prompt, given
/// the rollout of the thread being resumed.
pub fn codex_needs_injection(rollout_path: &Path, reminder: &str) -> bool {
    needs_injection(probe_codex(rollout_path), reminder)
}

fn probe_codex(rollout_path: &Path) -> Option<Probe> {
    let tail = read_tail(rollout_path)?;
    for line in tail.lines().rev() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("compacted") => return Some(Probe::Compacted),
            Some("response_item") => {}
            _ => continue,
        }
        if let Some(text) = codex_injected_reminder(&value) {
            return Some(Probe::Reminder(text));
        }
    }
    None
}

/// Codex has no side channel for context, so the reminder rides at the head of
/// the user message (`<system-reminder>…</system-reminder>\n\n<prompt>`) —
/// see `codex_launch::maybe_prepend_active_plans`. Only that prefix is
/// compared; the prompt after it differs every turn by construction.
fn codex_injected_reminder(value: &Value) -> Option<String> {
    let payload = value.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("message")
        || payload.get("role").and_then(Value::as_str) != Some("user")
    {
        return None;
    }
    let text: String = payload
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect();
    let end = text.find(REMINDER_CLOSE)? + REMINDER_CLOSE.len();
    let prefix = &text[..end];
    is_plan_reminder(prefix).then(|| prefix.to_string())
}

const REMINDER_OPEN: &str = "<system-reminder>";
const REMINDER_CLOSE: &str = "</system-reminder>";
/// Sentence unique to the plan reminder, present in every rendering of it (see
/// `prd_tasks::render_active_plans_reminder`).
const PLAN_REMINDER_MARK: &str = "re-injected on every prompt by Fleet PRD Discipline mode";

fn is_plan_reminder(text: &str) -> bool {
    text.trim_start().starts_with(REMINDER_OPEN) && text.contains(PLAN_REMINDER_MARK)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reminder shaped like the real one: the marker sentence is what tells
    /// a plan reminder apart from Fleet's other injected context.
    fn reminder(body: &str) -> String {
        format!("{REMINDER_OPEN}\nThe workspace `TASKS.md` ({PLAN_REMINDER_MARK}) holds {body}\n{REMINDER_CLOSE}")
    }

    fn write(lines: &[String]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
        (dir, path)
    }

    fn claude_attachment(text: &str) -> String {
        serde_json::json!({
            "type": "attachment",
            "isSidechain": false,
            "attachment": {"type": "hook_additional_context", "content": [text]},
        })
        .to_string()
    }

    fn claude_user(text: &str) -> String {
        serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": [{"type": "text", "text": text}]},
        })
        .to_string()
    }

    fn codex_user(text: &str) -> String {
        serde_json::json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": text}],
            },
        })
        .to_string()
    }

    #[test]
    fn claude_skips_an_identical_previous_copy() {
        let text = reminder("1 active plan");
        let (_dir, path) = write(&[claude_attachment(&text), claude_user("go on")]);
        assert!(!claude_needs_injection(&path, &text));
        // Trailing whitespace differences are not a change in the plans.
        assert!(!claude_needs_injection(&path, &format!("{text}\n")));
    }

    #[test]
    fn claude_injects_when_the_plans_moved() {
        let (_dir, path) = write(&[claude_attachment(&reminder("1 active plan"))]);
        assert!(claude_needs_injection(&path, &reminder("2 active plans")));
    }

    #[test]
    fn claude_injects_after_a_compaction_swallowed_the_copy() {
        let text = reminder("1 active plan");
        for boundary in [
            serde_json::json!({"type": "system", "subtype": "compact_boundary",
                               "compactMetadata": {"preTokens": 500, "postTokens": 50}})
            .to_string(),
            serde_json::json!({"type": "user", "isCompactSummary": true}).to_string(),
        ] {
            let (_dir, path) = write(&[claude_attachment(&text), boundary]);
            assert!(claude_needs_injection(&path, &text));
        }
    }

    #[test]
    fn claude_ignores_sidechain_and_foreign_hook_context() {
        let text = reminder("1 active plan");
        let sidechain = serde_json::json!({
            "type": "attachment",
            "isSidechain": true,
            "attachment": {"type": "hook_additional_context", "content": [text]},
        })
        .to_string();
        // A subagent's copy is in a different context window, and the notes
        // hint is a different hook — neither answers for this session's plans.
        let notes_hint = claude_attachment("<system-reminder>\n<fleet_notes>…</fleet_notes>\n</system-reminder>");
        let (_dir, path) = write(&[sidechain, notes_hint]);
        assert!(claude_needs_injection(&path, &text));
    }

    #[test]
    fn claude_injects_when_the_transcript_is_missing_or_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(claude_needs_injection(&dir.path().join("absent.jsonl"), &reminder("x")));
        let (_dir, path) = write(&[claude_user("first prompt")]);
        assert!(claude_needs_injection(&path, &reminder("x")));
    }

    #[test]
    fn codex_compares_only_the_prepended_prefix() {
        let text = reminder("1 active plan");
        let (_dir, path) = write(&[codex_user(&format!("{text}\n\nfix the flaky test"))]);
        // Same plans, brand-new prompt → still a skip.
        assert!(!codex_needs_injection(&path, &text));
        assert!(codex_needs_injection(&path, &reminder("2 active plans")));
    }

    #[test]
    fn codex_injects_after_compaction_and_for_a_bare_prompt() {
        let text = reminder("1 active plan");
        let compacted = serde_json::json!({"type": "compacted", "payload": {}}).to_string();
        let (_dir, path) = write(&[codex_user(&format!("{text}\n\nprompt")), compacted]);
        assert!(codex_needs_injection(&path, &text));

        // A turn that carried no reminder (PRD block off, or no active plan)
        // leaves nothing to compare against.
        let (_dir2, path2) = write(&[codex_user("just a prompt")]);
        assert!(codex_needs_injection(&path2, &text));
    }

    #[test]
    fn a_bounded_tail_falls_back_to_injecting() {
        let text = reminder("1 active plan");
        let filler = codex_user(&"x".repeat(300 * 1024));
        let (_dir, path) = write(&[
            codex_user(&format!("{text}\n\nprompt")),
            filler.clone(),
            filler.clone(),
            filler.clone(),
            filler,
        ]);
        // The previous copy is now older than the tail window: a wasted
        // injection, never a missing one.
        assert!(codex_needs_injection(&path, &text));
    }
}
