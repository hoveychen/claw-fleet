//! Pick the sessions that make up a workspace's "recent conversation context".
//!
//! Fleet's analogue of the layer ChatGPT injects as *Recent Conversation
//! Context*: when a context window opens, the agent gets a short list of what
//! this repository has been worked on lately, so it starts out knowing what
//! everyone else has been doing instead of only what it is told this turn.
//!
//! This module is the selection half — it turns a full scan into the ordered,
//! filtered slice that belongs in the block. Rendering it (and capping the
//! byte budget) is a separate step, so the selection can be unit-tested
//! without pinning down any wording.
//!
//! Selection rules, and why each one:
//!
//! - **Repo-scoped, not path-scoped.** A plan's `.worktrees/<task-id>`
//!   checkout is part of the repo it was branched from, so its sessions count
//!   as the same workspace (see [`crate::session::same_repo_root`]).
//! - **Newest first**, by the same clock the UI sorts folder sections with.
//! - **Excluding the caller.** A session has its own transcript; listing
//!   itself back to it is pure noise.
//! - **Subagents excluded by default.** They are implementation detail of a
//!   parent session that is listed in its own right.

use std::collections::HashMap;

use crate::session::{SessionInfo, SessionStatus};

/// Which sessions to gather for one workspace's recent-context block.
#[derive(Debug, Clone)]
pub struct RecentQuery<'a> {
    /// Any checkout of the target repo; worktrees collapse to their root.
    pub workspace_path: &'a str,
    /// The asking session, left out of its own list. `None` lists everything.
    pub exclude_session_id: Option<&'a str>,
    /// Keep subagent rows. Off by default — see the module docs.
    pub include_subagents: bool,
    /// Cap on rows returned. The renderer caps bytes too; this bounds the work.
    pub limit: usize,
}

impl<'a> RecentQuery<'a> {
    /// A query with the defaults 老板 signed off on: 40 rows, no subagents.
    pub fn new(workspace_path: &'a str) -> Self {
        Self {
            workspace_path,
            exclude_session_id: None,
            include_subagents: false,
            limit: DEFAULT_LIMIT,
        }
    }

    /// Leave the asking session out of its own list.
    pub fn excluding(mut self, session_id: &'a str) -> Self {
        self.exclude_session_id = Some(session_id);
        self
    }
}

/// Rows in the block. Matches ChatGPT's ~40-conversation window; measured at
/// ~1.1k tokens of titles on this machine, so the window is bounded by
/// usefulness rather than by budget.
pub const DEFAULT_LIMIT: usize = 40;

/// Is this session still working, as opposed to finished or waiting on a human?
///
/// Drives the "进行中" marker: knowing another session is *right now* touching
/// the same repo is the one thing this block offers that ChatGPT's equivalent
/// cannot. [`SessionStatus::WaitingInput`] is deliberately not "running" — it
/// has stopped and is waiting on a person.
pub fn is_running(status: &SessionStatus) -> bool {
    matches!(
        status,
        SessionStatus::Thinking
            | SessionStatus::Executing
            | SessionStatus::Streaming
            | SessionStatus::Delegating
            | SessionStatus::Processing
            | SessionStatus::Active
    )
}

/// The clock to order rows by.
///
/// Mirrors `workspaceSessionGroups.ts`'s `activityMs`: prefer the agent's own
/// last activity and fall back to the transcript's mtime, so the backend and
/// the folder sections in the UI agree on which session is most recent.
fn activity_ms(session: &SessionInfo) -> u64 {
    if session.agent_last_activity_ms > 0 {
        session.agent_last_activity_ms
    } else {
        session.last_activity_ms
    }
}

/// Select the workspace's recent sessions, newest first.
pub fn select<'a>(sessions: &'a [SessionInfo], query: &RecentQuery<'_>) -> Vec<&'a SessionInfo> {
    let mut picked: Vec<&SessionInfo> = sessions
        .iter()
        .filter(|s| {
            if !query.include_subagents && s.is_subagent {
                return false;
            }
            if query.exclude_session_id == Some(s.id.as_str()) {
                return false;
            }
            crate::session::same_repo_root(&s.workspace_path, query.workspace_path)
        })
        .collect();
    // Descending by activity; ties broken by id so the order is deterministic
    // across scans (a flapping block would defeat the dedup on every client).
    picked.sort_by(|a, b| {
        activity_ms(b)
            .cmp(&activity_ms(a))
            .then_with(|| a.id.cmp(&b.id))
    });
    picked.truncate(query.limit);
    picked
}

/// How many of the newest rows may carry a task-review summary.
///
/// Summaries are worth ~376 characters each against a ~26-character title, so
/// they buy depth on the rows most likely to matter and would drown the rest.
/// Only 29% of sessions have one at all (a review is written when a task is
/// ended from the card), so this is a ceiling, not a quota.
pub const DEFAULT_SUMMARY_DEPTH: usize = 3;

/// Longest title kept before eliding. The agent-written titles measure 26
/// characters on average and 84 at the longest; the cap exists for the
/// `last_message_preview` fallback, which is prose and unbounded.
const MAX_TITLE_CHARS: usize = 80;

/// One line of the block.
#[derive(Debug, Clone, PartialEq)]
pub struct RecentRow {
    pub session_id: String,
    /// Best available one-liner — see [`title_for`].
    pub title: String,
    /// Epoch ms, for the timestamp the renderer prints.
    pub activity_ms: u64,
    /// Still working right now, as opposed to finished or awaiting a human.
    pub running: bool,
    /// Retrospective prose, on the newest [`DEFAULT_SUMMARY_DEPTH`] rows that
    /// have one.
    pub summary: Option<String>,
}

/// Collapse prose to a single line and elide it to [`MAX_TITLE_CHARS`].
fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_TITLE_CHARS {
        return flat;
    }
    let kept: String = flat.chars().take(MAX_TITLE_CHARS).collect();
    format!("{kept}…")
}

/// The best one-liner describing a session, in the order the UI displays them.
///
/// `title_override` covers 99% of recent sessions on this machine, because the
/// session-title guidance has every agent name its own session; the rest of
/// the chain is fallback for sessions that predate it or ran without it.
/// Returns `None` when a session has nothing to say for itself, so the caller
/// can drop the row rather than print an empty one.
pub fn title_for(session: &SessionInfo) -> Option<String> {
    [
        session.title_override.as_deref(),
        session.ai_title.as_deref(),
        session.slug.as_deref(),
        session.last_message_preview.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|candidate| !candidate.is_empty())
    .map(one_line)
}

/// Index every session id in a chain to its review, so a row can find its
/// summary whether it was the task's first session or a later handoff hop
/// (`task_reviews` is keyed by the chain's root, not by each session).
fn reviews_by_session(reviews: &[crate::task_review::TaskReview]) -> HashMap<&str, &str> {
    let mut map = HashMap::new();
    for review in reviews {
        if review.summary.trim().is_empty() {
            continue;
        }
        for id in &review.session_ids {
            map.insert(id.as_str(), review.summary.as_str());
        }
        map.insert(review.root_session_id.as_str(), review.summary.as_str());
    }
    map
}

/// Build the block's rows: [`select`] the sessions, then attach titles and —
/// to the newest `summary_depth` rows that have one — their task summary.
pub fn build_rows(
    sessions: &[SessionInfo],
    query: &RecentQuery<'_>,
    reviews: &[crate::task_review::TaskReview],
    summary_depth: usize,
) -> Vec<RecentRow> {
    let summaries = reviews_by_session(reviews);
    let mut attached = 0usize;
    select(sessions, query)
        .into_iter()
        .filter_map(|session| {
            let title = title_for(session)?;
            let summary = if attached < summary_depth {
                summaries.get(session.id.as_str()).map(|s| {
                    attached += 1;
                    s.trim().to_string()
                })
            } else {
                None
            };
            Some(RecentRow {
                session_id: session.id.clone(),
                title,
                activity_ms: activity_ms(session),
                running: is_running(&session.status),
                summary,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, workspace: &str, activity: u64) -> SessionInfo {
        SessionInfo {
            id: id.to_string(),
            workspace_path: workspace.to_string(),
            last_activity_ms: activity,
            ..Default::default()
        }
    }

    #[test]
    fn keeps_only_the_target_repo() {
        let all = vec![
            session("a", "/w/proj", 3),
            session("b", "/w/other", 2),
            session("c", "/w/proj", 1),
        ];
        let got = select(&all, &RecentQuery::new("/w/proj"));
        let ids: Vec<&str> = got.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "c"]);
    }

    #[test]
    fn folds_a_worktree_into_its_repo() {
        let all = vec![
            session("main", "/w/proj", 2),
            session("wt", "/w/proj/.worktrees/some-task", 1),
        ];
        // Asking from either checkout sees both.
        for asking_from in ["/w/proj", "/w/proj/.worktrees/some-task"] {
            let got = select(&all, &RecentQuery::new(asking_from));
            assert_eq!(got.len(), 2, "from {asking_from}");
        }
    }

    #[test]
    fn orders_newest_first() {
        let all = vec![
            session("old", "/w/proj", 1),
            session("new", "/w/proj", 9),
            session("mid", "/w/proj", 5),
        ];
        let got = select(&all, &RecentQuery::new("/w/proj"));
        let ids: Vec<&str> = got.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["new", "mid", "old"]);
    }

    #[test]
    fn prefers_agent_activity_over_file_mtime() {
        let mut stale_file = session("agent-fresh", "/w/proj", 1);
        stale_file.agent_last_activity_ms = 100;
        let all = vec![session("file-fresh", "/w/proj", 50), stale_file];
        let got = select(&all, &RecentQuery::new("/w/proj"));
        assert_eq!(got[0].id, "agent-fresh");
    }

    #[test]
    fn leaves_the_asking_session_out() {
        let all = vec![session("me", "/w/proj", 2), session("other", "/w/proj", 1)];
        let got = select(&all, &RecentQuery::new("/w/proj").excluding("me"));
        let ids: Vec<&str> = got.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["other"]);
    }

    #[test]
    fn drops_subagents_unless_asked() {
        let mut sub = session("sub", "/w/proj", 2);
        sub.is_subagent = true;
        let all = vec![sub, session("parent", "/w/proj", 1)];

        let got = select(&all, &RecentQuery::new("/w/proj"));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "parent");

        let mut q = RecentQuery::new("/w/proj");
        q.include_subagents = true;
        assert_eq!(select(&all, &q).len(), 2);
    }

    #[test]
    fn honours_the_row_limit() {
        let all: Vec<SessionInfo> = (0..10)
            .map(|i| session(&format!("s{i}"), "/w/proj", i as u64))
            .collect();
        let mut q = RecentQuery::new("/w/proj");
        q.limit = 3;
        assert_eq!(select(&all, &q).len(), 3);
    }

    #[test]
    fn ties_break_deterministically() {
        let all = vec![session("b", "/w/proj", 5), session("a", "/w/proj", 5)];
        let got = select(&all, &RecentQuery::new("/w/proj"));
        let ids: Vec<&str> = got.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    fn review(root: &str, chain: &[&str], summary: &str) -> crate::task_review::TaskReview {
        crate::task_review::TaskReview {
            root_session_id: root.to_string(),
            session_ids: chain.iter().map(|s| s.to_string()).collect(),
            workspace_name: "proj".to_string(),
            workspace_path: "/w/proj".to_string(),
            outcome: crate::task_outcome::TaskOutcome::Completed,
            agent_claimed_complete: true,
            title: "t".to_string(),
            summary: summary.to_string(),
            lessons: Vec::new(),
            terminated_at: 0,
            generated_at: 0,
        }
    }

    #[test]
    fn title_prefers_the_override_then_walks_the_chain() {
        let mut s = session("x", "/w/proj", 1);
        s.last_message_preview = Some("preview".into());
        assert_eq!(title_for(&s).as_deref(), Some("preview"));
        s.slug = Some("slug".into());
        assert_eq!(title_for(&s).as_deref(), Some("slug"));
        s.ai_title = Some("ai".into());
        assert_eq!(title_for(&s).as_deref(), Some("ai"));
        s.title_override = Some("pinned".into());
        assert_eq!(title_for(&s).as_deref(), Some("pinned"));
    }

    #[test]
    fn title_skips_blank_candidates() {
        let mut s = session("x", "/w/proj", 1);
        s.title_override = Some("   ".into());
        s.ai_title = Some("".into());
        s.slug = Some("real".into());
        assert_eq!(title_for(&s).as_deref(), Some("real"));
    }

    #[test]
    fn a_session_with_nothing_to_say_has_no_title() {
        assert_eq!(title_for(&session("x", "/w/proj", 1)), None);
    }

    #[test]
    fn a_prose_fallback_is_flattened_and_elided() {
        let mut s = session("x", "/w/proj", 1);
        s.last_message_preview = Some(format!("line one\nline two {}", "z".repeat(200)));
        let got = title_for(&s).unwrap();
        assert!(!got.contains('\n'), "should be one line: {got}");
        assert_eq!(got.chars().count(), MAX_TITLE_CHARS + 1, "80 chars + ellipsis");
        assert!(got.ends_with('…'));
    }

    #[test]
    fn rows_drop_sessions_that_have_no_title() {
        let mut titled = session("titled", "/w/proj", 2);
        titled.title_override = Some("has one".into());
        let all = vec![titled, session("untitled", "/w/proj", 1)];
        let rows = build_rows(&all, &RecentQuery::new("/w/proj"), &[], 0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "titled");
    }

    #[test]
    fn summary_attaches_to_a_later_hop_of_the_chain() {
        // The review is keyed by the chain root, but the listed session is the
        // second hop — it must still find its summary.
        let mut hop = session("hop2", "/w/proj", 1);
        hop.title_override = Some("t".into());
        let reviews = vec![review("hop1", &["hop1", "hop2"], "what happened")];
        let rows = build_rows(&[hop], &RecentQuery::new("/w/proj"), &reviews, 3);
        assert_eq!(rows[0].summary.as_deref(), Some("what happened"));
    }

    #[test]
    fn summary_depth_caps_how_many_rows_carry_one() {
        let all: Vec<SessionInfo> = (0..4)
            .map(|i| {
                let mut s = session(&format!("s{i}"), "/w/proj", 10 - i as u64);
                s.title_override = Some(format!("title {i}"));
                s
            })
            .collect();
        let reviews: Vec<_> = (0..4)
            .map(|i| {
                let id = format!("s{i}");
                review(&id, &[&id], "prose")
            })
            .collect();
        let rows = build_rows(&all, &RecentQuery::new("/w/proj"), &reviews, 2);
        let with: Vec<&str> = rows
            .iter()
            .filter(|r| r.summary.is_some())
            .map(|r| r.session_id.as_str())
            .collect();
        // Newest two only — s0 has the highest activity.
        assert_eq!(with, vec!["s0", "s1"]);
    }

    #[test]
    fn a_blank_summary_never_occupies_a_depth_slot() {
        let mut first = session("blank", "/w/proj", 9);
        first.title_override = Some("a".into());
        let mut second = session("real", "/w/proj", 8);
        second.title_override = Some("b".into());
        let reviews = vec![
            review("blank", &["blank"], "   "),
            review("real", &["real"], "kept"),
        ];
        let rows = build_rows(&[first, second], &RecentQuery::new("/w/proj"), &reviews, 1);
        assert_eq!(rows[0].summary, None);
        assert_eq!(rows[1].summary.as_deref(), Some("kept"));
    }

    #[test]
    fn rows_carry_the_running_flag() {
        let mut live = session("live", "/w/proj", 2);
        live.title_override = Some("t".into());
        live.status = SessionStatus::Executing;
        let rows = build_rows(&[live], &RecentQuery::new("/w/proj"), &[], 0);
        assert!(rows[0].running);
    }

    #[test]
    fn running_covers_the_working_states_but_not_waiting() {
        assert!(is_running(&SessionStatus::Thinking));
        assert!(is_running(&SessionStatus::Executing));
        assert!(is_running(&SessionStatus::Delegating));
        assert!(!is_running(&SessionStatus::WaitingInput));
        assert!(!is_running(&SessionStatus::Idle));
    }
}
