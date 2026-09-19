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
pub fn is_running(status: SessionStatus) -> bool {
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

    #[test]
    fn running_covers_the_working_states_but_not_waiting() {
        assert!(is_running(SessionStatus::Thinking));
        assert!(is_running(SessionStatus::Executing));
        assert!(is_running(SessionStatus::Delegating));
        assert!(!is_running(SessionStatus::WaitingInput));
        assert!(!is_running(SessionStatus::Idle));
    }
}
