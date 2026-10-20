//! The task model: which sessions make up one task, and which tasks are
//! sub-tasks of another.
//!
//! A *task* is one idea driven by one main conversation. That conversation may
//! span several sessions (a `fleet handoff` relay chain); all of its hops share
//! one `task_id` — the chain's first session. A session the main conversation
//! spawned (`fleet__spawn` / `fleet spawn`, recorded as
//! `LaunchSpec::spawned_by_session_id`) starts a *sub-task*: its own line, with
//! its own `task_id`, rolled up under the spawning task via `parent_task_id`.
//! Spawns of spawns flatten to the outermost task — the UI has exactly two
//! levels, task and sub-task.
//!
//! Computed at scan time from two on-disk sources (handoff chains and launch
//! notes), never during the cached per-session parse: both change while the
//! session's own transcript does not.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::session::SessionInfo;

/// Where one session sits in the task model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPlace {
    /// Root of the session's own conversation line.
    pub task_id: String,
    /// The top-level task this line rolls up to, when it is a sub-task.
    pub parent_task_id: Option<String>,
}

/// The two edge sets the model is built from.
#[derive(Debug, Default)]
pub struct TaskGraph {
    /// Session id → first hop of the relay chain it is on.
    line_root: HashMap<String, String>,
    /// Session id → the session that spawned it.
    spawned_by: HashMap<String, String>,
}

impl TaskGraph {
    /// Build from relay chains (each as its ordered session ids) and spawn edges.
    pub fn new(
        chains: impl IntoIterator<Item = Vec<String>>,
        spawned_by: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        let mut line_root = HashMap::new();
        for ids in chains {
            let Some(head) = ids.first().cloned() else {
                continue;
            };
            for id in ids {
                line_root.entry(id).or_insert_with(|| head.clone());
            }
        }
        Self {
            line_root,
            spawned_by: spawned_by.into_iter().collect(),
        }
    }

    /// Read both edge sets off disk (cached readers on both sides).
    pub fn load() -> Self {
        let chains = crate::handoff::list_chains().into_iter().map(|c| c.session_ids());
        Self::new(chains, spawn_edges(crate::launch_spec::registry()))
    }

    /// Every spawn edge as `(child, spawner)`, so a caller that already loaded
    /// the graph does not read the launch-note registry a second time.
    pub fn spawn_edges(&self) -> impl Iterator<Item = (String, String)> + '_ {
        self.spawned_by.iter().map(|(c, p)| (c.clone(), p.clone()))
    }

    /// First hop of the relay chain `id` is on (`id` itself when it is on none).
    pub fn line_root(&self, id: &str) -> String {
        self.line_root_of(id).to_string()
    }

    fn line_root_of<'a>(&'a self, id: &'a str) -> &'a str {
        self.line_root.get(id).map(String::as_str).unwrap_or(id)
    }

    /// The line `id` was spawned from, when its line was spawned at all. The
    /// edge is looked up on the line's root and, failing that, on `id` itself:
    /// a sub-task's first hop carries it, but a line whose chain record is
    /// missing still has the edge on the session that was spawned.
    fn spawning_session(&self, id: &str) -> Option<&str> {
        let root = self.line_root_of(id);
        self.spawned_by
            .get(root)
            .or_else(|| self.spawned_by.get(id))
            .map(String::as_str)
    }

    /// Place one session. Cycles in the spawn edges (which only corrupt notes
    /// could produce) stop the walk at the last line not yet seen.
    pub fn place(&self, id: &str) -> TaskPlace {
        let own = self.line_root_of(id).to_string();
        let mut top = own.clone();
        let mut seen: HashSet<String> = HashSet::from([own.clone()]);
        let mut cursor = id.to_string();
        while let Some(parent) = self.spawning_session(&cursor) {
            let parent_root = self.line_root_of(parent).to_string();
            if !seen.insert(parent_root.clone()) {
                break;
            }
            top = parent_root;
            cursor = parent.to_string();
        }
        TaskPlace {
            parent_task_id: (top != own).then_some(top),
            task_id: own,
        }
    }
}

/// Whether `session_id` is a sub-task: it was dispatched to a step of another
/// session's plan, or its line rolls up under another task. The launch note is
/// read first; the graph (every relay chain and launch note) only when it
/// says nothing.
pub fn is_subtask(session_id: &str) -> bool {
    if crate::launch_spec::get(session_id).is_some_and(|s| s.spawned_for_plan.is_some()) {
        return true;
    }
    TaskGraph::load().place(session_id).parent_task_id.is_some()
}

/// The spawner's plan and step `session_id`'s line was dispatched to
/// (`LaunchSpec::spawned_for_plan` / `spawned_for_step`). Only the spawned
/// session's launch note carries it, so a handoff successor reads it off its
/// relay chain's first hop — the same lookup [`TaskGraph::spawning_session`]
/// does for the spawn edge.
pub fn assignment(session_id: &str) -> Option<(String, String)> {
    assignment_with(
        session_id,
        &|id| crate::handoff::chain_containing(id)?.session_ids().into_iter().next(),
        &note_assignment,
    )
}

/// [`assignment`] with the line-root lookup and the launch-note read
/// injected: callers resolving many sessions pass an index, tests pass maps.
pub fn assignment_with(
    session_id: &str,
    line_root: &dyn Fn(&str) -> Option<String>,
    note: &dyn Fn(&str) -> Option<(String, String)>,
) -> Option<(String, String)> {
    note(session_id).or_else(|| {
        let root = line_root(session_id).filter(|r| r != session_id)?;
        note(&root)
    })
}

/// Plans created by any hop of `session_id`'s relay line
/// (`LaunchSpec::created_plans`).
pub fn plans_created_by_line(session_id: &str) -> HashSet<String> {
    let hops = crate::handoff::chain_containing(session_id)
        .map(|c| c.session_ids())
        .unwrap_or_default();
    std::iter::once(session_id.to_string())
        .chain(hops)
        .filter_map(|id| crate::launch_spec::get(&id))
        .flat_map(|spec| spec.created_plans)
        .collect()
}

/// The assignment recorded in `session_id`'s own launch note.
pub fn note_assignment(session_id: &str) -> Option<(String, String)> {
    let spec = crate::launch_spec::get(session_id)?;
    Some((spec.spawned_for_plan?, spec.spawned_for_step?))
}

/// Sub-task edges `(child, spawner)` from launch notes. Only
/// `spawned_by_session_id` makes one: a task split off another
/// (`split_from_task`) is a top-level task of its own.
fn spawn_edges(
    notes: impl IntoIterator<Item = (String, crate::launch_spec::LaunchSpec)>,
) -> impl Iterator<Item = (String, String)> {
    notes
        .into_iter()
        .filter_map(|(id, spec)| spec.spawned_by_session_id.map(|p| (id, p)))
}

/// What a task row shows besides its title and state: stamped on every hop of
/// a top-level task's main conversation, so whichever hop a client folds the
/// line under carries it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    /// Hops of the main conversation.
    pub hop_count: u32,
    /// The plan the main conversation last focused on (`fleet plan`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub plan_id: Option<String>,
    /// The step it is on: that plan's first open P as of the last focus write,
    /// markdown intact (`**P3** — …`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub current_step: Option<String>,
    /// Why the task is parked: the note of the newest `fleet watch` the main
    /// conversation holds.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub wait_reason: Option<String>,
    /// Sub-task lines (a relayed sub-task counts once).
    pub subtask_count: u32,
    /// Sub-task lines with a turn in flight or a watch pending.
    pub subtasks_active: u32,
    /// Earliest session start across the task and its sub-tasks (epoch ms).
    pub started_at_ms: u64,
    /// Latest activity across the task and its sub-tasks (epoch ms).
    pub last_activity_ms: u64,
    /// Spend of every session in the task, sub-agents and sub-tasks included.
    pub cost_usd: f64,
    /// Sub-tasks dispatched for a named step of a plan (`fleet__spawn`
    /// `step`), ordered by sub-task id. Sub-tasks spawned without a step are
    /// absent.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub subtask_steps: Vec<SubtaskStep>,
    /// The task's type and goal (`task_profile`), when one was set.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub profile: Option<crate::task_profile::TaskProfileBrief>,
    /// The task this one was split off from (`fleet__task action=split`),
    /// with that task's display title so a client can link back to it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub split_from: Option<SplitFromRef>,
}

/// Where a split-off task came from: the source task's id (its first hop's
/// session id) and its title as of this scan. `title` is `None` when the
/// source task is no longer in the scanned sessions.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SplitFromRef {
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<String>,
}

/// One sub-task's step link: which plan step it was dispatched to work on.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SubtaskStep {
    /// The sub-task's line id (its `task_id`).
    pub task_id: String,
    pub plan_id: String,
    /// Step id, e.g. `P3`.
    pub step: String,
}

fn is_active(s: &SessionInfo) -> bool {
    use crate::session::SessionStatus as S;
    s.proc_alive
        || matches!(
            s.status,
            S::Thinking
                | S::Executing
                | S::Streaming
                | S::Delegating
                | S::Processing
                | S::Active
                | S::Watching
        )
}

/// Build every top-level task's summary from stamped sessions (see
/// [`enrich_sessions`]) and write it onto the task's main-line hops.
/// `focus` resolves a session's `fleet plan` focus — injected for tests.
fn stamp_summaries(
    sessions: &mut [SessionInfo],
    focus: impl Fn(&str) -> Option<crate::task_progress::TaskProgressRecord>,
    spawn_step: impl Fn(&str) -> Option<(String, String)>,
    profile: impl Fn(&str) -> Option<crate::task_profile::TaskProfileBrief>,
    split_from: impl Fn(&str) -> Option<String>,
) {
    let mut by_task: HashMap<String, TaskSummary> = HashMap::new();
    let mut sub_lines: HashMap<String, HashSet<String>> = HashMap::new();
    let mut active_lines: HashMap<String, HashSet<String>> = HashMap::new();
    let mut newest_watch: HashMap<String, (u64, String)> = HashMap::new();
    let mut newest_focus: HashMap<String, (u64, crate::task_progress::TaskProgressRecord)> =
        HashMap::new();
    let mut step_links: HashMap<String, std::collections::BTreeMap<String, (String, String)>> =
        HashMap::new();

    for s in sessions.iter() {
        let Some(line) = s.task_id.as_deref() else {
            continue;
        };
        let top = s.parent_task_id.as_deref().unwrap_or(line);
        let sum = by_task.entry(top.to_string()).or_default();
        let created = s.created_at_ms;
        if created > 0 && (sum.started_at_ms == 0 || created < sum.started_at_ms) {
            sum.started_at_ms = created;
        }
        sum.last_activity_ms = sum
            .last_activity_ms
            .max(s.agent_last_activity_ms.max(s.last_activity_ms));
        sum.cost_usd += s.agent_total_cost_usd.max(s.total_cost_usd);

        if s.parent_task_id.is_some() {
            sub_lines.entry(top.to_string()).or_default().insert(line.to_string());
            if is_active(s) {
                active_lines.entry(top.to_string()).or_default().insert(line.to_string());
            }
            // The spawn note sits on the line's first hop, whose id is the line id.
            if s.id == line {
                if let Some(link) = spawn_step(line) {
                    step_links.entry(top.to_string()).or_default().insert(line.to_string(), link);
                }
            }
            continue;
        }
        sum.hop_count += 1;
        for w in &s.watches {
            if let Some(note) = w.note.as_deref().filter(|n| !n.trim().is_empty()) {
                let slot = newest_watch.entry(top.to_string()).or_insert((0, String::new()));
                if w.created >= slot.0 {
                    *slot = (w.created, note.to_string());
                }
            }
        }
        if let Some(rec) = focus(&s.id).filter(|r| r.workspace_path == s.workspace_path) {
            let newer = newest_focus
                .get(top)
                .map(|(t, _)| rec.updated > *t)
                .unwrap_or(true);
            if newer {
                newest_focus.insert(top.to_string(), (rec.updated, rec));
            }
        }
    }

    for (top, sum) in by_task.iter_mut() {
        sum.subtask_count = sub_lines.get(top).map_or(0, |l| l.len() as u32);
        sum.subtasks_active = active_lines.get(top).map_or(0, |l| l.len() as u32);
        sum.wait_reason = newest_watch.get(top).map(|(_, n)| n.clone());
        if let Some((_, rec)) = newest_focus.get(top) {
            sum.plan_id = Some(rec.plan_id.clone());
            sum.current_step = rec.current_task.clone();
        }
        sum.subtask_steps = step_links
            .remove(top)
            .unwrap_or_default()
            .into_iter()
            .map(|(task_id, (plan_id, step))| SubtaskStep { task_id, plan_id, step })
            .collect();
        sum.profile = profile(top);
        // The split note sits on the new task's first hop, whose id is the task id.
        sum.split_from = split_from(top).map(|from| SplitFromRef {
            title: line_title(sessions, &from, true),
            task_id: from,
        });
    }

    for s in sessions.iter_mut() {
        s.task_summary = match (&s.task_id, &s.parent_task_id) {
            (Some(line), None) => by_task.get(line).cloned(),
            _ => None,
        };
    }
}

/// Stamp [`TaskSummary`] onto top-level task hops. Runs last in the scan's
/// enrich pass: it reads watches, statuses and outcomes the other enrichers set.
pub fn enrich_summaries(sessions: &mut [SessionInfo]) {
    let notes = crate::launch_spec::registry();
    let profiles = crate::task_profile::load_all();
    stamp_summaries(
        sessions,
        crate::task_progress::read,
        |id| {
            let spec = notes.get(id)?;
            Some((spec.spawned_for_plan.clone()?, spec.spawned_for_step.clone()?))
        },
        |task| profiles.get(task).map(Into::into),
        |task| notes.get(task)?.split_from_task.clone(),
    );
}

/// Refresh just the profile slice of already-stamped summaries, after a
/// profile write, without re-running the whole summary pass.
pub fn restamp_profiles(sessions: &mut [SessionInfo]) {
    let profiles = crate::task_profile::load_all();
    for s in sessions.iter_mut() {
        if let (Some(sum), Some(task)) = (s.task_summary.as_mut(), s.task_id.as_deref()) {
            sum.profile = profiles.get(task).map(Into::into);
        }
    }
}

/// Which task a decision card belongs to, for the inbox breadcrumb
/// (`project › task › from sub-task`).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DecisionTaskRef {
    /// The top-level task.
    pub task_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub task_title: Option<String>,
    /// Set when the card came from a sub-task: that sub-task's line id.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub subtask_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub subtask_title: Option<String>,
}

/// Display title of a conversation line: its newest hop's title.
fn line_title(sessions: &[SessionInfo], line: &str, top_level: bool) -> Option<String> {
    sessions
        .iter()
        .filter(|s| s.task_id.as_deref() == Some(line) && s.parent_task_id.is_none() == top_level)
        .max_by_key(|s| (s.handoff.as_ref().map_or(0, |h| h.hop), s.created_at_ms))
        .and_then(|s| s.title_override.clone().or_else(|| s.ai_title.clone()))
}

/// Resolve the task a card raised by `session_id` belongs to, from sessions
/// already stamped by [`enrich_sessions`]. A sub-agent's card is attributed
/// through its owning session. `None` when the session is not in `sessions`.
pub fn decision_task_ref(sessions: &[SessionInfo], session_id: &str) -> Option<DecisionTaskRef> {
    let mut s = sessions.iter().find(|s| s.id == session_id)?;
    if s.is_subagent {
        let owner = s.parent_session_id.as_deref()?;
        s = sessions.iter().find(|x| x.id == owner)?;
    }
    let line = s.task_id.clone()?;
    Some(match s.parent_task_id.clone() {
        Some(top) => DecisionTaskRef {
            task_title: line_title(sessions, &top, true),
            task_id: top,
            subtask_title: line_title(sessions, &line, false),
            subtask_id: Some(line),
        },
        None => DecisionTaskRef {
            task_title: line_title(sessions, &line, true),
            task_id: line,
            subtask_id: None,
            subtask_title: None,
        },
    })
}

/// Body of the accept action on every client: the sessions whose tasks the
/// boss accepts (any hop of each task will do).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct AcceptTasksRequest {
    pub session_ids: Vec<String>,
}

/// Card id recorded on outcomes set by the accept action rather than a card.
pub const ACCEPT_CARD_ID: &str = "accept";

/// Accept tasks from the task list ("验收" / "全部验收"): mark each task
/// completed across its whole relay chain and start its retrospective — what
/// the finish button on a card does, minus `plan_revive::continue_after_finish`.
/// Accepting is a verdict on finished work, so it must not relaunch a task
/// whose plan still has open steps; accepting ten rows would otherwise spawn
/// up to ten sessions. Returns how many tasks were stamped.
pub fn accept_tasks(session_ids: &[String]) -> usize {
    let stamped = stamp_accepted(session_ids);
    // The retrospective runs on a detached thread that would outlive a unit
    // test's temporary HOME and write into the real one.
    if !cfg!(test) {
        for (id, ws) in &stamped {
            let outcome = crate::task_outcome::TaskOutcome::Completed;
            crate::task_review::on_task_terminated(id, ws, outcome);
        }
    }
    stamped.len()
}

/// The stamping half of [`accept_tasks`]: one `(session, workspace)` per task.
fn stamp_accepted(session_ids: &[String]) -> Vec<(String, String)> {
    let mut done: HashSet<String> = HashSet::new();
    let mut stamped = Vec::new();
    for id in session_ids.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        // Two hops of one chain in the same batch are one task.
        let hops = crate::task_review::task_sessions(id);
        if hops.iter().any(|h| done.contains(h)) {
            continue;
        }
        let outcome = crate::task_outcome::TaskOutcome::Completed;
        let ws = crate::task_review::stamp_terminal_chain(id, outcome, ACCEPT_CARD_ID, false);
        done.extend(hops);
        stamped.push((id.to_string(), ws));
    }
    stamped
}

/// Reopen tasks from the task list ("改回进行中" / "撤销验收"): the inverse of
/// [`accept_tasks`] and of a card's terminal button. Clears the outcome and the
/// `Done` review mark on every hop of each task's relay chain, so the row falls
/// back to whatever its sessions are doing (running, waiting, awaiting review).
/// Relaunches nothing — reopening is a change of verdict, not a resume — and
/// leaves an already-written retrospective in place. Returns how many tasks
/// were reopened (tasks that had no outcome count too: the call is idempotent).
pub fn reopen_tasks(session_ids: &[String]) -> usize {
    let mut done: HashSet<String> = HashSet::new();
    let mut reopened = 0;
    for id in session_ids.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        let hops = crate::task_review::task_sessions(id);
        if hops.iter().any(|h| done.contains(h)) {
            continue;
        }
        for hop in hops.iter().chain(std::iter::once(&id.to_string())) {
            let ws = crate::task_outcome::read(hop)
                .map(|r| r.workspace_path)
                .unwrap_or_default();
            if let Err(e) = crate::task_outcome::set_outcome(hop, &ws, None, "", false) {
                crate::log_debug(&format!("reopen task: clear outcome of {hop}: {e}"));
            }
            if crate::session_mark::read(hop) == Some(crate::session_mark::SessionMark::Done) {
                if let Err(e) = crate::session_mark::set_mark(hop, &ws, None) {
                    crate::log_debug(&format!("reopen task: clear mark of {hop}: {e}"));
                }
            }
        }
        done.extend(hops);
        reopened += 1;
    }
    reopened
}

/// Stamp `task_id` / `parent_task_id` onto scanned sessions. Sub-agents
/// (Claude Code's in-process `Task` agents) are not part of the model and are
/// left unset.
pub fn enrich_sessions(sessions: &mut [SessionInfo]) {
    let graph = TaskGraph::load();
    stamp(&graph, sessions);
}

fn stamp(graph: &TaskGraph, sessions: &mut [SessionInfo]) {
    for s in sessions.iter_mut() {
        if s.is_subagent {
            s.task_id = None;
            s.parent_task_id = None;
            continue;
        }
        let place = graph.place(&s.id);
        s.task_id = Some(place.task_id);
        s.parent_task_id = place.parent_task_id;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session's own note wins; a handoff successor, whose note carries no
    /// assignment, reads its line's first hop.
    #[test]
    fn assignment_is_read_off_the_line_root() {
        let notes = |sid: &str| match sid {
            "S" => Some(("par".to_string(), "P2".to_string())),
            "T" => Some(("own".to_string(), "P1".to_string())),
            _ => None,
        };
        let root = |sid: &str| Some(if sid.starts_with('S') { "S" } else { sid }.to_string());
        let want = Some(("par".to_string(), "P2".to_string()));
        assert_eq!(assignment_with("S", &root, &notes), want);
        assert_eq!(assignment_with("S3", &root, &notes), want);
        assert_eq!(assignment_with("T", &root, &notes).unwrap().0, "own");
        assert_eq!(assignment_with("M", &root, &notes), None);
        assert_eq!(assignment_with("S3", &|_| None, &notes), None);
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn edge(child: &str, parent: &str) -> (String, String) {
        (child.to_string(), parent.to_string())
    }

    fn place(task: &str, parent: Option<&str>) -> TaskPlace {
        TaskPlace {
            task_id: task.into(),
            parent_task_id: parent.map(str::to_string),
        }
    }

    #[test]
    fn lone_session_is_its_own_task() {
        let g = TaskGraph::new(vec![], vec![]);
        assert_eq!(g.place("a"), place("a", None));
    }

    #[test]
    fn every_hop_of_a_relay_shares_the_first_hops_id() {
        let g = TaskGraph::new(vec![ids(&["a1", "a2", "a3"])], vec![]);
        for id in ["a1", "a2", "a3"] {
            assert_eq!(g.place(id), place("a1", None), "{id}");
        }
    }

    #[test]
    fn spawned_line_is_a_subtask_of_the_spawners_task() {
        // a1 → a2 relay; a2 spawned b1, which relayed to b2.
        let g = TaskGraph::new(
            vec![ids(&["a1", "a2"]), ids(&["b1", "b2"])],
            vec![edge("b1", "a2")],
        );
        assert_eq!(g.place("b1"), place("b1", Some("a1")));
        assert_eq!(g.place("b2"), place("b1", Some("a1")));
        assert_eq!(g.place("a2"), place("a1", None));
    }

    #[test]
    fn nested_spawns_flatten_to_the_outermost_task() {
        let g = TaskGraph::new(vec![], vec![edge("b", "a"), edge("c", "b")]);
        assert_eq!(g.place("c"), place("c", Some("a")));
    }

    #[test]
    fn spawn_cycle_terminates() {
        let g = TaskGraph::new(vec![], vec![edge("a", "b"), edge("b", "a")]);
        assert_eq!(g.place("a"), place("a", Some("b")));
    }

    #[test]
    fn a_split_task_is_top_level_while_a_spawn_is_a_subtask() {
        use crate::launch_spec::LaunchSpec;
        let notes = vec![
            (
                "split".to_string(),
                LaunchSpec { split_from_task: Some("a1".into()), ..Default::default() },
            ),
            (
                "sub".to_string(),
                LaunchSpec { spawned_by_session_id: Some("a1".into()), ..Default::default() },
            ),
        ];
        let g = TaskGraph::new(vec![], spawn_edges(notes));
        assert_eq!(g.place("split"), place("split", None));
        assert_eq!(g.place("sub"), place("sub", Some("a1")));
    }

    #[test]
    fn subagents_are_left_out() {
        let g = TaskGraph::new(vec![], vec![edge("b", "a")]);
        let mut rows = vec![
            SessionInfo { id: "b".into(), ..Default::default() },
            SessionInfo { id: "b-sub".into(), is_subagent: true, ..Default::default() },
        ];
        stamp(&g, &mut rows);
        assert_eq!(rows[0].task_id.as_deref(), Some("b"));
        assert_eq!(rows[0].parent_task_id.as_deref(), Some("a"));
        assert_eq!(rows[1].task_id, None);
    }

    #[test]
    fn summary_rolls_up_hops_and_subtasks_onto_main_hops_only() {
        use crate::session::SessionStatus;
        use crate::task_progress::TaskProgressRecord;
        let g = TaskGraph::new(
            vec![ids(&["a1", "a2"]), ids(&["b1", "b2"])],
            vec![edge("b1", "a2"), edge("c", "a1")],
        );
        let row = |id: &str, created: u64, cost: f64, status: SessionStatus| SessionInfo {
            id: id.into(),
            workspace_path: "/w".into(),
            created_at_ms: created,
            last_activity_ms: created + 10,
            agent_total_cost_usd: cost,
            status,
            ..Default::default()
        };
        let mut rows = vec![
            row("a1", 100, 1.0, SessionStatus::Idle),
            row("a2", 200, 2.0, SessionStatus::Watching),
            row("b1", 150, 0.5, SessionStatus::Idle),
            row("b2", 300, 0.25, SessionStatus::Executing),
            row("c", 50, 0.25, SessionStatus::Idle),
        ];
        rows[1].watches = vec![serde_json::from_value(serde_json::json!({
            "id": "w", "note": "CI run", "created": 5,
            "pollSecs": 30, "deadlineAt": 0, "pollCount": 0
        }))
        .unwrap()];
        stamp(&g, &mut rows);
        stamp_summaries(&mut rows, |id| {
            (id == "a2").then(|| TaskProgressRecord {
                workspace_path: "/w".into(),
                plan_id: "p".into(),
                current_task: Some("**P2** — x".into()),
                updated: 1,
                backtracked_from: None,
            })
        }, |id| {
            // Only a line's first hop carries the spawn note; a later hop's
            // lookup must not add a second link for the same line.
            matches!(id, "b1" | "b2").then(|| ("p".to_string(), "P2".to_string()))
        }, |task| {
            (task == "a1").then(|| crate::task_profile::TaskProfileBrief {
                task_type: crate::task_profile::TaskType::Research,
                goal: Some("g".into()),
                set_by: crate::task_profile::ProfileSetter::Agent,
            })
        }, |_| None);
        let sum = rows[0].task_summary.clone().expect("main hop carries it");
        assert_eq!(rows[1].task_summary.as_ref(), Some(&sum));
        assert!(rows[2].task_summary.is_none() && rows[4].task_summary.is_none());
        assert_eq!(sum.hop_count, 2);
        assert_eq!(
            sum.profile.as_ref().map(|p| (p.task_type, p.goal.as_deref())),
            Some((crate::task_profile::TaskType::Research, Some("g"))),
            "the task's profile rides on its summary"
        );
        assert_eq!(sum.subtask_count, 2, "b-line counts once, c once");
        assert_eq!(sum.subtasks_active, 1);
        assert_eq!(sum.started_at_ms, 50);
        assert_eq!(sum.last_activity_ms, 310);
        assert!((sum.cost_usd - 4.0).abs() < 1e-9);
        assert_eq!(sum.wait_reason.as_deref(), Some("CI run"));
        assert_eq!(sum.plan_id.as_deref(), Some("p"));
        assert_eq!(sum.current_step.as_deref(), Some("**P2** — x"));
        assert_eq!(
            sum.subtask_steps,
            vec![SubtaskStep { task_id: "b1".into(), plan_id: "p".into(), step: "P2".into() }],
            "the b-line links once; c was spawned without a step"
        );
    }

    #[test]
    fn split_off_task_links_back_to_its_source_by_title() {
        let g = TaskGraph::new(vec![ids(&["a1", "a2"])], vec![]);
        let titled = |id: &str, title: Option<&str>| SessionInfo {
            id: id.into(),
            ai_title: title.map(Into::into),
            ..Default::default()
        };
        let mut rows = vec![
            titled("a1", Some("old title")),
            titled("a2", Some("Refactor auth")),
            titled("s", Some("Write docs")),
            titled("orphan", None),
        ];
        stamp(&g, &mut rows);
        stamp_summaries(&mut rows, |_| None, |_| None, |_| None, |task| match task {
            "s" => Some("a1".to_string()),
            "orphan" => Some("gone".to_string()),
            _ => None,
        });
        assert_eq!(
            rows[2].task_summary.as_ref().and_then(|s| s.split_from.clone()),
            Some(SplitFromRef { task_id: "a1".into(), title: Some("Refactor auth".into()) }),
            "the source's title is its newest hop's"
        );
        assert_eq!(
            rows[3].task_summary.as_ref().and_then(|s| s.split_from.clone()),
            Some(SplitFromRef { task_id: "gone".into(), title: None }),
            "a source no longer scanned still links by id"
        );
        assert_eq!(rows[0].task_summary.as_ref().and_then(|s| s.split_from.clone()), None);
    }

    #[test]
    fn card_from_a_subtask_names_both_task_and_subtask() {
        let g = TaskGraph::new(vec![ids(&["a1", "a2"])], vec![edge("b", "a2")]);
        let titled = |id: &str, title: &str| SessionInfo {
            id: id.into(),
            ai_title: Some(title.into()),
            ..Default::default()
        };
        let mut rows = vec![
            titled("a1", "old title"),
            titled("a2", "Refactor auth"),
            titled("b", "Write tests"),
            SessionInfo {
                id: "b-agent".into(),
                is_subagent: true,
                parent_session_id: Some("b".into()),
                ..Default::default()
            },
        ];
        rows[1].handoff = Some(crate::handoff::SessionHandoffInfo {
            chain_id: "c".into(),
            hop: 2,
            chain_len: 2,
        });
        stamp(&g, &mut rows);
        let want = DecisionTaskRef {
            task_id: "a1".into(),
            task_title: Some("Refactor auth".into()),
            subtask_id: Some("b".into()),
            subtask_title: Some("Write tests".into()),
        };
        assert_eq!(decision_task_ref(&rows, "b"), Some(want.clone()));
        assert_eq!(decision_task_ref(&rows, "b-agent"), Some(want));
        let main = decision_task_ref(&rows, "a1").unwrap();
        assert_eq!((main.task_id.as_str(), main.subtask_id), ("a1", None));
        assert_eq!(decision_task_ref(&rows, "nope"), None);

        let mut req = crate::guard::GuardRequest {
            id: "g".into(),
            session_id: "b".into(),
            workspace_name: String::new(),
            ai_title: None,
            task: None,
            tool_name: "Bash".into(),
            command: String::new(),
            command_summary: String::new(),
            risk_tags: Vec::new(),
            timestamp: String::new(),
            structured_command: None,
        };
        crate::pending_decisions::decorate(&mut req, &rows);
        assert_eq!(req.ai_title.as_deref(), Some("Write tests"));
        assert_eq!(req.task.as_ref().map(|t| t.task_id.as_str()), Some("a1"));
    }
}
