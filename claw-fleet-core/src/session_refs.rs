//! Session mentions — a prompt that points at another session (or the relay
//! chain it sits on).
//!
//! The composers (desktop new-session form and follow-up composer, mobile
//! composer) let the user `@`-pick a session and insert it into the prose as a
//! markdown link with the `fleet-session:` scheme:
//!
//! ```text
//! 参考一下 [@修 CPU 占用](fleet-session:0b1c…) 的做法
//! ```
//!
//! Every user-facing spawn / resume / enqueue entry point runs the text
//! through [`prepare`] before it reaches the agent. That does two things:
//!
//! 1. Inlines a summary of each referenced chain — its goal, the latest
//!    handoff note and the last hop's final reply — as one tagged block
//!    ([`BLOCK_OPEN`] … [`BLOCK_CLOSE`]) placed after the prose and before the
//!    composer's `Context files:` block. A bare id would be useless on its own:
//!    an agent that has to go dig every time mostly doesn't.
//! 2. Records a persistent grant (`~/.fleet/session-refs/<sid>.json`) so the
//!    receiving session's `fleet__history` may read the referenced chains in
//!    full. History scope is otherwise its own transcript plus its relay
//!    predecessors ([`crate::session_notes::readable_sessions`]).
//!
//! The grant deliberately widens history only, never notes: notes inherit an
//! existing file from any session in their scope on first edit, so a granted
//! session's `checkpoint.md` would silently become the receiver's.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

use crate::handoff::HandoffChain;

/// Link scheme the composers emit for a mention.
pub const SCHEME: &str = "fleet-session:";
/// Opening tag of the inlined summary block.
pub const BLOCK_OPEN: &str = "<fleet-referenced-sessions>";
/// Closing tag of the inlined summary block.
pub const BLOCK_CLOSE: &str = "</fleet-referenced-sessions>";

/// At most this many distinct chains are summarised; further mentions still
/// get the history grant.
const MAX_SUMMARISED: usize = 5;
/// Char budget of one chain's latest handoff note.
const NOTE_CHARS: usize = 4000;
/// Char budget of one chain's last reply.
const REPLY_CHARS: usize = 2000;

/// The composers' `Context files:` marker (see [`crate::context_files`]).
const CONTEXT_FILES_MARKER: &str = "\n\nContext files:\n";

/// One `[@title](fleet-session:<id>)` mention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    pub title: String,
    pub session_id: String,
}

/// Mentions in `text`, in order of first appearance, one per session id.
/// Mentions inside an already-inlined block are ignored.
pub fn parse_mentions(text: &str) -> Vec<Mention> {
    let text = strip_block(text);
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(open) = rest.find("[@") {
        let after = &rest[open + 2..];
        let Some(close) = after.find(']') else { break };
        let title = &after[..close];
        let tail = &after[close + 1..];
        let link_prefix = format!("({SCHEME}");
        if title.contains('\n') || !tail.starts_with(&link_prefix) {
            rest = after;
            continue;
        }
        let id_part = &tail[link_prefix.len()..];
        let Some(end) = id_part.find(')') else {
            rest = after;
            continue;
        };
        let id = &id_part[..end];
        if is_plain_id(id) && seen.insert(id.to_string()) {
            out.push(Mention {
                title: title.trim().to_string(),
                session_id: id.to_string(),
            });
        }
        rest = &id_part[end + 1..];
    }
    out
}

/// `text` with the inlined summary block removed (for titles and previews).
pub fn strip_block(text: &str) -> String {
    let Some(start) = text.find(BLOCK_OPEN) else {
        return text.to_string();
    };
    let Some(rel_end) = text[start..].find(BLOCK_CLOSE) else {
        return text.to_string();
    };
    let end = start + rel_end + BLOCK_CLOSE.len();
    let head = text[..start].trim_end_matches('\n');
    let tail = &text[end..];
    format!("{head}{tail}")
}

/// Marker of the composers' trailing reference-chip list (see
/// `shared-ts/sessionMentions.ts`): `- [@title](fleet-session:<id>)` and
/// `- [[slug]]` lines after the prose, before `Context files:`.
const REFS_LIST_MARKER: &str = "\n\nReferences:\n";

/// `text` without a trailing reference-chip list (pass it text whose
/// `Context files:` block is already gone). Unchanged unless every line of the
/// list is one of the two shapes the composers emit.
pub fn strip_refs_list(text: &str) -> &str {
    let Some(idx) = text.rfind(REFS_LIST_MARKER) else {
        return text;
    };
    let tail = &text[idx + REFS_LIST_MARKER.len()..];
    let tail = tail.strip_suffix('\n').unwrap_or(tail);
    let is_ref_line = |l: &str| {
        (l.starts_with("- [@") && l.contains(&format!("]({SCHEME}")) && l.ends_with(')'))
            || (l.starts_with("- [[") && l.ends_with("]]") && l.len() > 6)
    };
    if tail.is_empty() || !tail.split('\n').all(is_ref_line) {
        return text;
    }
    &text[..idx]
}

// ── Grants ───────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct GrantFile {
    /// Session ids the owner referenced; each stands for its whole chain.
    refs: Vec<String>,
}

fn grants_dir() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("session-refs"))
}

fn is_plain_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn read_grants_in(dir: &Path, session_id: &str) -> Vec<String> {
    if !is_plain_id(session_id) {
        return Vec::new();
    }
    fs::read_to_string(dir.join(format!("{session_id}.json")))
        .ok()
        .and_then(|s| serde_json::from_str::<GrantFile>(&s).ok())
        .map(|g| g.refs)
        .unwrap_or_default()
}

/// Add `refs` to `session_id`'s grant file (union, order kept).
pub fn grant_in(dir: &Path, session_id: &str, refs: &[String]) -> Result<(), String> {
    if !is_plain_id(session_id) {
        return Err(format!("session id `{session_id}` is not a plain identifier"));
    }
    let mut all = read_grants_in(dir, session_id);
    let before = all.len();
    for r in refs {
        if r != session_id && is_plain_id(r) && !all.contains(r) {
            all.push(r.clone());
        }
    }
    if all.len() == before {
        return Ok(());
    }
    fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join(format!("{session_id}.json"));
    let body = serde_json::to_string_pretty(&GrantFile { refs: all }).map_err(|e| e.to_string())?;
    crate::atomic_json::write_atomic(&path, body.as_bytes())
        .map_err(|e| format!("write {}: {e}", path.display()))
}

/// Sessions `session_id`'s `fleet__history` may read: its own scope
/// ([`crate::session_notes::readable_sessions`]) followed by every chain that
/// it or one of its relay predecessors referenced.
pub fn history_scope(session_id: &str) -> Vec<String> {
    let own = crate::session_notes::readable_sessions(session_id);
    let Some(dir) = grants_dir() else {
        return own;
    };
    history_scope_with(own, |s| read_grants_in(&dir, s), crate::handoff::chain_containing)
}

/// Pure core of [`history_scope`].
pub fn history_scope_with(
    own: Vec<String>,
    grants_of: impl Fn(&str) -> Vec<String>,
    chain_of: impl Fn(&str) -> Option<HandoffChain>,
) -> Vec<String> {
    let mut out = own.clone();
    for holder in &own {
        for r in grants_of(holder) {
            let ids = chain_of(&r).map(|c| c.session_ids()).unwrap_or_else(|| vec![r]);
            for id in ids {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
    }
    out
}

// ── Inlining ─────────────────────────────────────────────────────────────────

/// What the summary of one referenced chain is built from.
#[derive(Debug, Clone, Default)]
pub struct RefSummary {
    pub title: String,
    pub session_id: String,
    pub chain: Option<HandoffChain>,
    pub workspace: Option<String>,
    /// Final reply of the chain's latest hop, when its transcript is readable.
    pub last_reply: Option<String>,
}

fn clip(s: &str, limit: usize) -> String {
    let s = s.trim();
    match s.char_indices().nth(limit) {
        None => s.to_string(),
        Some((byte, _)) => format!("{}…（已截断，原文用 fleet__history 读）", &s[..byte]),
    }
}

fn render_one(r: &RefSummary) -> String {
    let title = if r.title.is_empty() { "（无标题）" } else { r.title.as_str() };
    let mut out = format!("## {title}\n- 被引用的会话：{}\n", r.session_id);
    if let Some(ws) = &r.workspace {
        out.push_str(&format!("- workspace：{ws}\n"));
    }
    if let Some(chain) = &r.chain {
        let ids = chain.session_ids();
        out.push_str(&format!(
            "- 它在一条接力链上，共 {} 棒（旧→新）：{}\n",
            ids.len(),
            ids.join(" → ")
        ));
        if let Some(goal) = &chain.goal {
            out.push_str(&format!("- 链目标：{goal}\n"));
        }
        if let Some(link) = chain.links.last() {
            out.push_str(&format!(
                "\n### 最新一版交接文档\n{}\n",
                clip(&link.note, NOTE_CHARS)
            ));
        }
    }
    if let Some(reply) = &r.last_reply {
        out.push_str(&format!("\n### 最后一条回复\n{}\n", clip(reply, REPLY_CHARS)));
    }
    out
}

/// Render the inlined block for `refs`. `extra` counts mentions that were
/// granted but not summarised (over [`MAX_SUMMARISED`]).
pub fn render_block(refs: &[RefSummary], extra: usize) -> String {
    let mut out = String::new();
    out.push_str(BLOCK_OPEN);
    out.push_str(
        "\n老板在上面的消息里引用了下列会话（链）。以下是 Fleet 自动附上的摘要；\
         需要细节时用 fleet__history 读原文——这些会话已加入你的 history 范围，\
         search 会一并搜到，read 时把 session 参数设成命中所在的会话 id。\n\n",
    );
    let parts: Vec<String> = refs.iter().map(render_one).collect();
    out.push_str(&parts.join("\n"));
    if extra > 0 {
        out.push_str(&format!(
            "\n另有 {extra} 个被引用的会话未展开摘要，同样可以用 fleet__history 读。\n"
        ));
    }
    out.push_str(BLOCK_CLOSE);
    out
}

/// Insert `block` after the prose, before a trailing `Context files:` block.
pub fn insert_block(text: &str, block: &str) -> String {
    let (prose, files) = match text.rfind(CONTEXT_FILES_MARKER) {
        Some(i) if crate::context_files::split(text).1.len() > 0 => (&text[..i], &text[i..]),
        _ => (text, ""),
    };
    format!("{}\n\n{block}{files}", prose.trim_end_matches('\n'))
}

fn summarise(m: &Mention) -> RefSummary {
    let chain = crate::handoff::chain_containing(&m.session_id);
    let latest = chain
        .as_ref()
        .and_then(|c| c.session_ids().last().cloned())
        .unwrap_or_else(|| m.session_id.clone());
    let last_reply = crate::session::find_session_jsonl(&latest).and_then(|p| {
        let lines = crate::jsonl_tail::read_tail_lines_as_json(&p, 100).ok()?;
        crate::session::extract_last_text_full(&lines)
    });
    RefSummary {
        title: m.title.clone(),
        session_id: m.session_id.clone(),
        workspace: chain.as_ref().map(|c| c.workspace_path.clone()),
        chain,
        last_reply,
    }
}

/// Run a user-authored prompt for an existing `session_id` (resume, follow-up)
/// through the mention pipeline: grant history access to every referenced
/// chain and inline their summaries.
///
/// Text without mentions is returned untouched. A grant failure is logged and
/// does not block the message — the inlined summary still lands.
pub fn prepare(session_id: &str, text: &str) -> String {
    grant_mentions(session_id, text);
    inline(text, Some(session_id))
}

/// Spawn a new session whose prompt may carry mentions: inline the summaries
/// before the spawn, grant once the spawned session's id is known (codex only
/// learns its thread id after the child starts).
pub fn spawn_session(
    tool: &str,
    spec: &crate::agent_source::SpawnSpec,
) -> Result<crate::session_launch::SpawnSessionResponse, String> {
    if parse_mentions(&spec.prompt).is_empty() {
        return crate::agent_source::spawn_session(tool, spec);
    }
    let mut spec = spec.clone();
    spec.prompt = inline(&spec.prompt, spec.session_id.as_deref());
    let resp = crate::agent_source::spawn_session(tool, &spec)?;
    match resp.session_id.as_deref().or(spec.session_id.as_deref()) {
        Some(sid) => grant_mentions(sid, &spec.prompt),
        None => crate::log_debug("[session_refs] spawned session has no id; mentions not granted"),
    }
    Ok(resp)
}

/// Record `session_id`'s history grant for every mention in `text`.
pub fn grant_mentions(session_id: &str, text: &str) {
    let ids: Vec<String> = parse_mentions(text)
        .into_iter()
        .map(|m| m.session_id)
        .filter(|id| id != session_id)
        .collect();
    if ids.is_empty() {
        return;
    }
    if let Some(dir) = grants_dir() {
        if let Err(e) = grant_in(&dir, session_id, &ids) {
            crate::log_debug(&format!("[session_refs] grant for {session_id} failed: {e}"));
        }
    }
}

/// `text` with the summary block of its mentions inserted. Idempotent: a text
/// that already carries a block is returned as is. `self_id` (the receiving
/// session, when known) is never summarised.
pub fn inline(text: &str, self_id: Option<&str>) -> String {
    if text.contains(BLOCK_OPEN) {
        return text.to_string();
    }
    let mentions: Vec<Mention> = parse_mentions(text)
        .into_iter()
        .filter(|m| Some(m.session_id.as_str()) != self_id)
        .collect();
    if mentions.is_empty() {
        return text.to_string();
    }
    // Two mentions of sessions on the same chain summarise that chain once.
    let mut seen_chains = BTreeSet::new();
    let mut summaries = Vec::new();
    let mut extra = 0;
    for m in &mentions {
        let s = summarise(m);
        let key = s
            .chain
            .as_ref()
            .map(|c| c.chain_id.clone())
            .unwrap_or_else(|| s.session_id.clone());
        if !seen_chains.insert(key) {
            continue;
        }
        if summaries.len() < MAX_SUMMARISED {
            summaries.push(s);
        } else {
            extra += 1;
        }
    }
    insert_block(text, &render_block(&summaries, extra))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handoff::HandoffLink;

    fn chain(ids: &[&str]) -> HandoffChain {
        HandoffChain {
            chain_id: "c1".into(),
            workspace_path: "/ws".into(),
            plan_id: None,
            goal: Some("把 CPU 压下来".into()),
            goal_history: Vec::new(),
            links: ids
                .windows(2)
                .enumerate()
                .map(|(i, w)| HandoffLink {
                    from_session_id: w[0].into(),
                    to_session_id: w[1].into(),
                    note: format!("note {i}"),
                    plan_id: None,
                    next_task: None,
                    handed_at: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn parses_mentions_in_order_and_dedups() {
        let t = "看 [@修 CPU](fleet-session:aaa-1) 和 [@别的](fleet-session:bbb_2)，再看 [@修 CPU](fleet-session:aaa-1)";
        let m = parse_mentions(t);
        assert_eq!(
            m,
            vec![
                Mention { title: "修 CPU".into(), session_id: "aaa-1".into() },
                Mention { title: "别的".into(), session_id: "bbb_2".into() },
            ]
        );
    }

    #[test]
    fn ignores_plain_links_and_unsafe_ids() {
        assert!(parse_mentions("[@x](https://e.com) [x](fleet-session:abc) [@y](fleet-session:../etc)").is_empty());
    }

    #[test]
    fn text_without_mentions_is_untouched() {
        assert_eq!(prepare("self", "just prose"), "just prose");
    }

    #[test]
    fn block_goes_between_prose_and_context_files() {
        let text = "看下 [@a](fleet-session:a)\n\nContext files:\n- /x.png";
        let out = insert_block(text, "<B>");
        assert_eq!(out, "看下 [@a](fleet-session:a)\n\n<B>\n\nContext files:\n- /x.png");
        // The attachment block is still parseable after insertion.
        assert_eq!(crate::context_files::split(&out).1, vec!["/x.png"]);
        assert_eq!(insert_block("hi\n", "<B>"), "hi\n\n<B>");
    }

    #[test]
    fn chip_list_mentions_parse_and_block_lands_before_context_files() {
        let text = "看下\n\nReferences:\n- [@a](fleet-session:a)\n- [[arch/x]]\n\nContext files:\n- /x.png";
        assert_eq!(parse_mentions(text).len(), 1);
        let out = insert_block(text, "<B>");
        assert_eq!(
            out,
            "看下\n\nReferences:\n- [@a](fleet-session:a)\n- [[arch/x]]\n\n<B>\n\nContext files:\n- /x.png"
        );
        let real = insert_block(text, &format!("{BLOCK_OPEN}\nx\n{BLOCK_CLOSE}"));
        let prose = strip_block(&real);
        let (prose, _) = crate::context_files::split(&prose);
        assert_eq!(strip_refs_list(prose), "看下");
    }

    #[test]
    fn strip_refs_list_leaves_prose_alone() {
        let text = "a\n\nReferences:\n- some book";
        assert_eq!(strip_refs_list(text), text);
        assert_eq!(strip_refs_list("plain"), "plain");
    }

    #[test]
    fn strip_block_restores_prose() {
        let text = "看下 [@a](fleet-session:a)\n\n<fleet-referenced-sessions>\nx\n</fleet-referenced-sessions>\n\nContext files:\n- /x.png";
        assert_eq!(strip_block(text), "看下 [@a](fleet-session:a)\n\nContext files:\n- /x.png");
        assert_eq!(strip_block("plain"), "plain");
    }

    #[test]
    fn mentions_inside_the_block_do_not_count() {
        let text = "hi\n\n<fleet-referenced-sessions>\n[@z](fleet-session:zzz)\n</fleet-referenced-sessions>";
        assert!(parse_mentions(text).is_empty());
    }

    #[test]
    fn render_block_carries_goal_note_and_reply() {
        let r = RefSummary {
            title: "修 CPU".into(),
            session_id: "b".into(),
            chain: Some(chain(&["a", "b", "c"])),
            workspace: Some("/ws".into()),
            last_reply: Some("搞定了".into()),
        };
        let out = render_block(&[r], 2);
        assert!(out.starts_with(BLOCK_OPEN) && out.ends_with(BLOCK_CLOSE));
        assert!(out.contains("## 修 CPU"));
        assert!(out.contains("a → b → c"));
        assert!(out.contains("链目标：把 CPU 压下来"));
        assert!(out.contains("note 1"), "latest note wins: {out}");
        assert!(!out.contains("note 0"));
        assert!(out.contains("搞定了"));
        assert!(out.contains("另有 2 个"));
    }

    #[test]
    fn inline_is_idempotent_and_skips_the_receiver() {
        let text = "看 [@x](fleet-session:zz-no-such-session-1)";
        let once = inline(text, None);
        assert!(once.contains(BLOCK_OPEN) && once.contains("zz-no-such-session-1"));
        assert_eq!(inline(&once, None), once);
        assert_eq!(inline(text, Some("zz-no-such-session-1")), text);
    }

    #[test]
    fn clip_marks_truncation() {
        assert_eq!(clip("abc", 5), "abc");
        assert!(clip(&"字".repeat(10), 3).starts_with("字字字…"));
    }

    #[test]
    fn grants_union_and_skip_self() {
        let dir = tempfile::tempdir().unwrap();
        grant_in(dir.path(), "me", &["a".into(), "me".into()]).unwrap();
        grant_in(dir.path(), "me", &["b".into(), "a".into()]).unwrap();
        assert_eq!(read_grants_in(dir.path(), "me"), vec!["a", "b"]);
        assert!(grant_in(dir.path(), "../x", &["a".into()]).is_err());
        assert!(read_grants_in(dir.path(), "nobody").is_empty());
    }

    #[test]
    fn history_scope_expands_referenced_chains_and_inherits_through_relay() {
        // `me` relayed from `pred`; `pred` referenced `b` (chain a→b→c), `me`
        // referenced the lone session `solo`.
        let own = vec!["me".to_string(), "pred".to_string()];
        let scope = history_scope_with(
            own,
            |s| match s {
                "pred" => vec!["b".into()],
                "me" => vec!["solo".into()],
                _ => vec![],
            },
            |s| (s == "b").then(|| chain(&["a", "b", "c"])),
        );
        assert_eq!(scope, vec!["me", "pred", "solo", "a", "b", "c"]);
    }
}
