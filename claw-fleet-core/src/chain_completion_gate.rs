//! Chain-aware gate on `taskComplete: true`.
//!
//! A card's terminal button reads 「结束任务」 when the agent sets
//! `taskComplete: true`, and pressing it closes the session as a *success*. The
//! agent decides that flag alone and Fleet takes it at face value — which is
//! fine for a one-session task and wrong on a long relay chain, because the
//! agent's sense of "the work" is the plan in front of it, not what the boss
//! asked for twenty-six hops ago.
//!
//! Observed 2026-09-18 on chain `1f783a5d` (26 hops, anatole-datas): the chain
//! started as "rewrite all 27 Jinja pages as React + implement the Go backend".
//! Hop 26 was handed a three-task plan to untangle a double alembic head — a
//! genuine but tiny prerequisite. It finished those three tasks and raised a
//! card with `taskComplete: true`, so the boss was shown a 「结束任务」 button
//! while the phase-2 deployment had never happened. Two structural facts let
//! that through: the plan it held was created as a *root* plan, so Rule 4's
//! backtracking had no ancestor to climb to; and the chain's real objective
//! lived only as prose at the top of each handoff note, checkable by nobody.
//!
//! This module is the cheap half of the fix. It does not judge whether the work
//! is done — it cannot. It bounces the call **once per session**, naming the
//! hop position and quoting the opening of hop 1's note, so the agent re-reads
//! the chain's origin before the boss ever sees a success button. Re-sending
//! the same card goes through untouched: an agent that has looked and still
//! believes it is done is exactly the case we want to allow.

use std::path::PathBuf;

/// How much of hop 1's note to quote back. Long enough to carry the "what the
/// boss originally asked" paragraph these notes open with, short enough not to
/// bury the instruction under it.
const ORIGIN_EXCERPT_CHARS: usize = 400;

fn nudged_dir() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("chain-completion-nudged"))
}

/// One marker file per session. Session ids are uuids, but sanitize defensively
/// so no id can escape the directory.
fn nudged_path(session_id: &str) -> Option<PathBuf> {
    let safe: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    nudged_dir().map(|d| d.join(format!("{safe}.marker")))
}

fn already_nudged(session_id: &str) -> bool {
    nudged_path(session_id).map(|p| p.exists()).unwrap_or(false)
}

fn mark_nudged(session_id: &str) {
    let Some(path) = nudged_path(session_id) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, b"");
}

/// Clip `note` to `limit` chars on a char boundary.
fn excerpt(note: &str, limit: usize) -> String {
    let note = note.trim();
    match note.char_indices().nth(limit) {
        None => note.to_string(),
        Some((byte, _)) => format!("{}…", &note[..byte]),
    }
}

/// Build the refusal text for a session at `hop` of `chain_len`, quoting the
/// chain's first note. Split out from [`refusal_for`] so the wording can be
/// tested without a `~/.fleet` on disk.
pub fn refusal_text(hop: u32, chain_len: u32, first_note: &str) -> String {
    format!(
        "你在接力链的第 {hop}/{chain_len} 棒，却把这张卡标成了 `taskComplete: true`，\
         卡底的按钮会渲染成「结束任务」，老板一按本会话就记为成功收工。\n\n\
         收工的判据是**链的起点**达成，不是你手上这个 plan 做完 —— \
         计划树全勾不等于老板最初要的那件事做完了，而链的目标只以散文形式活在 note 里，\
         没人替你核对。\n\n\
         第 1 棒的 note 是这么开头的：\n\n> {origin}\n\n\
         现在做一件事：确认那件事真的完成了。不确定就 `fleet__handoff` 传 `action=\"show\"` \
         把整条链读一遍。\n\
         - **确认已完成** → 把这张卡原样再发一次，`taskComplete` 保持 true，本会话不会再拦第二次。\n\
         - **没完成** → 把 `taskComplete` 改成 false，并在卡里点名还剩什么、下一步该干哪件。",
        hop = hop,
        chain_len = chain_len,
        origin = excerpt(first_note, ORIGIN_EXCERPT_CHARS).replace('\n', "\n> "),
    )
}

/// The nudge to hand back instead of raising this card, or `None` to let it
/// through. Returns `Some` only for a `taskComplete: true` card raised by a
/// session that is not the first hop of its chain, and only the first time per
/// session — the marker is written as a side effect of returning `Some`.
pub fn refusal_for(session_id: &str, task_complete: bool) -> Option<String> {
    if !task_complete || session_id.trim().is_empty() {
        return None;
    }
    let chain = crate::handoff::chain_containing(session_id)?;
    let hop = chain.hop_of(session_id)?;
    // Hop 1 raised no successor's card: its own task *is* the chain's task, so
    // there is no origin it could have drifted away from.
    if hop <= 1 {
        return None;
    }
    if already_nudged(session_id) {
        return None;
    }
    let first_note = chain
        .links
        .first()
        .map(|l| l.note.as_str())
        .unwrap_or_default();
    mark_nudged(session_id);
    Some(refusal_text(
        hop,
        chain.session_ids().len() as u32,
        first_note,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handoff::{HandoffChain, HandoffLink};

    fn link(from: &str, to: &str, note: &str) -> HandoffLink {
        HandoffLink {
            from_session_id: from.into(),
            to_session_id: to.into(),
            note: note.into(),
            plan_id: None,
            next_task: None,
            handed_at: 0,
        }
    }

    #[test]
    fn refusal_names_the_hop_and_quotes_the_origin() {
        let text = refusal_text(26, 26, "# 接力简报\n\n老板要把 27 页前端一次性全重写");
        assert!(
            text.contains("第 26/26 棒"),
            "hop position is named: {text}"
        );
        assert!(
            text.contains("27 页前端一次性全重写"),
            "origin is quoted: {text}"
        );
        assert!(
            text.contains("action=\"show\""),
            "points at the chain reader: {text}"
        );
        assert!(
            text.contains("taskComplete"),
            "names the flag to reconsider: {text}"
        );
    }

    #[test]
    fn excerpt_clips_on_a_char_boundary() {
        // Multi-byte chars: a naive byte slice would panic here.
        let long = "老板要的是什么".repeat(200);
        let out = excerpt(&long, 10);
        assert_eq!(out.chars().count(), 11, "10 chars plus the ellipsis: {out}");
        assert!(out.ends_with('…'));
        assert_eq!(excerpt("短", 10), "短", "short notes are quoted whole");
    }

    #[test]
    fn hop_one_and_incomplete_cards_pass_through() {
        let dir = tempfile::tempdir().unwrap();
        let _home = crate::paths::fleet_home_guard(dir.path());

        // `taskComplete: false` never gates, whatever the chain looks like.
        assert!(refusal_for("whatever-session", false).is_none());
        // An unknown session is on no chain, so there is no origin to check.
        assert!(refusal_for("session-on-no-chain", true).is_none());
    }

    /// Write `chain` where [`crate::handoff::chain_containing`] will find it
    /// under the currently claimed `FLEET_HOME`.
    fn seed_chain(chain: &HandoffChain) {
        let dir = crate::session::get_fleet_dir()
            .unwrap()
            .join("handoffs")
            .join("chain");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{}.json", chain.chain_id)),
            serde_json::to_string(chain).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn gates_a_later_hop_once_then_lets_the_resend_through() {
        let dir = tempfile::tempdir().unwrap();
        let _home = crate::paths::fleet_home_guard(dir.path());

        seed_chain(&HandoffChain {
            chain_id: "c-gate".into(),
            workspace_path: "/tmp/ws".into(),
            plan_id: None,
            goal: None,
            goal_history: Vec::new(),
            links: vec![
                link("s1", "s2", "老板要把 27 页前端一次性全重写"),
                link("s2", "s3", "顺手理掉 alembic 双 head"),
            ],
        });

        // Hop 1 owns the chain's own task — never gated.
        assert!(refusal_for("s1", true).is_none(), "hop 1 passes through");

        // Hop 3 claiming completion gets the nudge, quoting hop 1's note (not
        // the note that was handed to it, which is the corner task).
        let first = refusal_for("s3", true).expect("later hop is gated");
        assert!(first.contains("第 3/3 棒"), "names the hop: {first}");
        assert!(
            first.contains("27 页前端一次性全重写"),
            "quotes hop 1's note: {first}"
        );
        assert!(
            !first.contains("alembic"),
            "does not quote the corner task: {first}"
        );

        // Re-sending the same card goes through: the agent has looked once.
        assert!(
            refusal_for("s3", true).is_none(),
            "second attempt is not gated"
        );
    }

    #[test]
    fn chain_hop_lookup_skips_the_first_hop() {
        let chain = HandoffChain {
            chain_id: "c1".into(),
            workspace_path: "/tmp/ws".into(),
            plan_id: None,
            goal: None,
            goal_history: Vec::new(),
            links: vec![
                link("s1", "s2", "origin note"),
                link("s2", "s3", "later note"),
            ],
        };
        assert_eq!(chain.hop_of("s1"), Some(1), "first hop is not gated");
        assert_eq!(chain.hop_of("s3"), Some(3));
        assert_eq!(chain.session_ids().len(), 3);
    }
}
