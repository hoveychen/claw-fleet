//! Task profile — what a task *is* (its type) and what it is *for* (its goal).
//!
//! A task is one idea driven by one main conversation (see
//! [`crate::task_model`]); its id is the first hop of its relay chain. The
//! profile is the durable record the per-turn guidance and the three clients
//! read to know which workflow applies and what counts as done:
//!
//! - `type` — `dev` / `research` / `general`. A task starts `general`, which the
//!   clients show as "undecided". Only the boss decides it: by pressing 开工 on a
//!   kickoff card the agent raised (`fleet__task action=kickoff`, see
//!   [`crate::task_kickoff`]) or by rewriting it from the task list. A type an
//!   agent wrote on its own (older builds let `fleet__task set` do that, and
//!   most of those labels were guesses) reads as `general` — see
//!   [`TaskProfile::type_set_by`]. A missing profile, or an unknown type string
//!   written by a newer build, also reads as `general`.
//! - `goal` — one sentence of "the work is done when …". Changing a goal that
//!   is already set requires a reason, the same rule
//!   [`crate::handoff`] applies to a relay chain's goal; every change is kept in
//!   `goal_history`.
//!
//! Only the task's main conversation defines the task. A sub-task session
//! resolves to the profile of the task it belongs to (read-only for it).
//!
//! File layout: `~/.fleet/task-profile/<task_id>.json`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// What kind of work a task is. Decides which workflow guidance applies.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum TaskType {
    /// Changes code: worktree, tests, merge.
    Dev,
    /// Produces knowledge: a report, findings, a recommendation.
    Research,
    /// Anything else, and the fallback for unknown values.
    #[default]
    #[serde(other)]
    General,
}

impl TaskType {
    /// Lenient parse for tool/CLI input: unknown values are `None` so the
    /// caller can refuse them instead of silently filing the task as general.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dev" => Some(Self::Dev),
            "research" => Some(Self::Research),
            "general" => Some(Self::General),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Research => "research",
            Self::General => "general",
        }
    }
}

/// Who last wrote a profile.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum ProfileSetter {
    /// The task's main conversation, via `fleet__task` / `fleet handoff`.
    #[default]
    Agent,
    /// The boss, from a client's task view.
    Boss,
}

/// One recorded change to a task's goal. The first entry (the goal being set
/// at all) has `from: None` and no `reason`; every later one carries both.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct GoalChange {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub from: Option<String>,
    pub to: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<String>,
    /// Epoch ms of the change.
    pub at: u64,
    pub by: ProfileSetter,
}

/// A task's profile as stored on disk and sent to clients.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TaskProfile {
    pub task_id: String,
    #[serde(rename = "type", default)]
    pub task_type: TaskType,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub goal: Option<String>,
    /// Every change to `goal`, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub goal_history: Vec<GoalChange>,
    #[serde(default)]
    pub set_by: ProfileSetter,
    /// Who wrote `task_type`. `set_by` names the last writer of *any* field, so
    /// it cannot tell a boss-approved type apart from an agent's goal edit made
    /// afterwards. A non-general type only counts when this is `Boss`; anything
    /// else — including its absence on profiles written before it existed —
    /// reads as `general` ([`normalized`]). Kept out of the TS bindings: the
    /// clients only ever see the normalized type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-export", ts(skip))]
    pub type_set_by: Option<ProfileSetter>,
    /// Epoch ms of the last write.
    #[serde(default)]
    pub updated: u64,
}

/// The profile as every reader should see it: a type nobody but an agent ever
/// vouched for is dropped back to `general`. Applied on read rather than by a
/// one-off rewrite of `~/.fleet/task-profile/`, so a profile an older build's
/// still-running MCP server writes after the upgrade is caught too.
fn normalized(mut p: TaskProfile) -> TaskProfile {
    if p.task_type != TaskType::General && p.type_set_by != Some(ProfileSetter::Boss) {
        p.task_type = TaskType::General;
    }
    p
}

/// The slice of a profile a task row carries (`TaskSummary::profile`).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TaskProfileBrief {
    #[serde(rename = "type")]
    pub task_type: TaskType,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub goal: Option<String>,
    pub set_by: ProfileSetter,
}

impl From<&TaskProfile> for TaskProfileBrief {
    fn from(p: &TaskProfile) -> Self {
        Self {
            task_type: p.task_type,
            goal: p.goal.clone(),
            set_by: p.set_by,
        }
    }
}

/// Request body for the boss rewriting a profile from a client. `session_id`
/// may be any hop of the task (or a sub-task session: it resolves to the
/// owning task). Absent fields are left alone.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct SetTaskProfileRequest {
    pub session_id: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none", default)]
    pub task_type: Option<TaskType>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub goal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub goal_reason: Option<String>,
}

/// Request body for reading the profile of the task a session belongs to.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskProfileQuery {
    pub session_id: String,
}

fn profile_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("task-profile"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && !id.contains(['/', '\\']) && id != "." && id != ".."
}

/// The profile of task `task_id`, if one was ever written.
pub fn read(task_id: &str) -> Option<TaskProfile> {
    read_in(&profile_dir()?, task_id)
}

fn read_in(dir: &Path, task_id: &str) -> Option<TaskProfile> {
    if !valid_id(task_id) {
        return None;
    }
    let raw = std::fs::read_to_string(dir.join(format!("{task_id}.json"))).ok()?;
    serde_json::from_str(&raw).ok().map(normalized)
}

/// Every stored profile keyed by task id (cached directory read; the scan's
/// enrich pass calls this once per scan).
pub fn load_all() -> HashMap<String, TaskProfile> {
    static CACHE: crate::json_dir_cache::JsonDirCache<TaskProfile> =
        crate::json_dir_cache::JsonDirCache::new();
    let Some(dir) = profile_dir() else {
        return HashMap::new();
    };
    CACHE
        .load(&dir)
        .into_iter()
        .map(|(_, p)| (p.task_id.clone(), normalized(p)))
        .collect()
}

/// Write a profile change for `task_id`. `task_type: None` and `goal: None`
/// leave those fields alone; a goal different from the current one needs a
/// non-empty `goal_reason`.
pub fn set(
    task_id: &str,
    task_type: Option<TaskType>,
    goal: Option<&str>,
    goal_reason: Option<&str>,
    by: ProfileSetter,
) -> Result<TaskProfile, String> {
    let dir = profile_dir().ok_or("cannot determine home dir")?;
    set_in(&dir, task_id, task_type, goal, goal_reason, by, now_ms())
}

fn set_in(
    dir: &Path,
    task_id: &str,
    task_type: Option<TaskType>,
    goal: Option<&str>,
    goal_reason: Option<&str>,
    by: ProfileSetter,
    now: u64,
) -> Result<TaskProfile, String> {
    if !valid_id(task_id) {
        return Err(format!("invalid task id {task_id:?}"));
    }
    let goal = goal.map(str::trim).filter(|g| !g.is_empty());
    let reason = goal_reason.map(str::trim).filter(|r| !r.is_empty());
    if task_type.is_none() && goal.is_none() {
        return Err("nothing to set: pass a type, a goal, or both".to_string());
    }
    let path = dir.join(format!("{task_id}.json"));
    crate::atomic_json::with_file_lock(&path, || {
        let mut p = read_in(dir, task_id).unwrap_or_else(|| TaskProfile {
            task_id: task_id.to_string(),
            ..Default::default()
        });
        if let Some(g) = goal {
            crate::handoff::check_goal_change(p.goal.as_deref(), g, reason)?;
            if p.goal.as_deref() != Some(g) {
                p.goal_history.push(GoalChange {
                    from: p.goal.clone(),
                    to: g.to_string(),
                    reason: p.goal.as_ref().and(reason.map(str::to_string)),
                    at: now,
                    by,
                });
                p.goal = Some(g.to_string());
            }
        }
        if let Some(t) = task_type {
            p.task_type = t;
            p.type_set_by = Some(by);
        }
        p.set_by = by;
        p.updated = now;
        let json = serde_json::to_vec_pretty(&p).map_err(|e| format!("serialize: {e}"))?;
        crate::atomic_json::write_atomic(&path, &json)
            .map_err(|e| format!("write task profile: {e}"))?;
        crate::json_dir_cache::note_changed(&path);
        Ok(normalized(p))
    })
}

/// Where a session sits: its own line's task id, and the top-level task it
/// rolls up to when it is a sub-task.
pub fn place_of(session_id: &str) -> crate::task_model::TaskPlace {
    crate::task_model::TaskGraph::load().place(session_id)
}

/// The top-level task a session belongs to — its own task for a main
/// conversation hop, the spawning task for a sub-task session.
pub fn owning_task_id(session_id: &str) -> String {
    let place = place_of(session_id);
    place.parent_task_id.unwrap_or(place.task_id)
}

/// The profile of the task `session_id` belongs to.
pub fn for_session(session_id: &str) -> Option<TaskProfile> {
    read(&owning_task_id(session_id))
}

/// Set the profile of the task `session_id` is the main conversation of.
/// Refused for a sub-task session: only the task's main conversation (or the
/// boss) defines the task.
pub fn set_for_session(
    session_id: &str,
    task_type: Option<TaskType>,
    goal: Option<&str>,
    goal_reason: Option<&str>,
    by: ProfileSetter,
) -> Result<TaskProfile, String> {
    let place = place_of(session_id);
    let task_id = match (place.parent_task_id, by) {
        (Some(_), ProfileSetter::Agent) => {
            return Err("this session is a sub-task: only the task's main conversation can \
                 define the task (type/goal). Report what you found in your final reply \
                 instead."
                .to_string())
        }
        (Some(top), ProfileSetter::Boss) => top,
        (None, _) => place.task_id,
    };
    set(&task_id, task_type, goal, goal_reason, by)
}

/// The profile goal that governs `session_id`'s own conversation line, when
/// it is a main-line hop. A sub-task line is judged by its own relay chain's
/// goal, never by the parent task's profile — that goal is the boss's finish
/// line for the whole task, not the sub-task's.
pub fn main_line_goal(session_id: &str) -> Option<String> {
    let place = place_of(session_id);
    if place.parent_task_id.is_some() {
        return None;
    }
    read(&place.task_id)?.goal
}

/// The goal a relay chain is judged against: the task profile's goal when the
/// chain is a task's main line and has one, else the chain's own recorded goal
/// (chains from before task profiles existed, and sub-task chains).
pub fn effective_chain_goal(session_id: &str, chain_goal: Option<&str>) -> Option<String> {
    pick_goal(main_line_goal(session_id), chain_goal)
}

fn pick_goal(profile_goal: Option<String>, chain_goal: Option<&str>) -> Option<String> {
    profile_goal
        .filter(|g| !g.trim().is_empty())
        .or_else(|| chain_goal.map(str::to_string))
}

/// Mirror a relay's `--goal` into the task profile. Only main-line hops write
/// it; a sub-task's chain goal stays on its chain. `check_only` validates the
/// change against the profile's current goal without writing — the handoff
/// path checks before it registers and writes only once registration succeeded.
pub fn sync_handoff_goal(
    session_id: &str,
    goal: &str,
    goal_reason: Option<&str>,
    check_only: bool,
) -> Result<(), String> {
    let goal = goal.trim();
    if goal.is_empty() {
        return Ok(());
    }
    let place = place_of(session_id);
    if place.parent_task_id.is_some() {
        return Ok(());
    }
    if check_only {
        let current = read(&place.task_id).and_then(|p| p.goal);
        return crate::handoff::check_goal_change(current.as_deref(), goal, goal_reason);
    }
    set(&place.task_id, None, Some(goal), goal_reason, ProfileSetter::Agent).map(|_| ())
}

/// What the clients show for a session's task: the stored profile of the task
/// it belongs to, or an empty `general` one (`updated == 0`) when none exists.
pub fn view_for_session(session_id: &str) -> TaskProfile {
    let task_id = owning_task_id(session_id);
    read(&task_id).unwrap_or(TaskProfile {
        task_id,
        ..Default::default()
    })
}

/// The boss rewriting a task's profile from a client (desktop, `fleet serve`,
/// mobile relay all land here).
pub fn set_by_boss(req: &SetTaskProfileRequest) -> Result<TaskProfile, String> {
    set_for_session(
        req.session_id.trim(),
        req.task_type,
        req.goal.as_deref(),
        req.goal_reason.as_deref(),
        ProfileSetter::Boss,
    )
}

/// Plain-text rendering for `fleet__task action=get` and `fleet task get`.
pub fn render(task_id: &str, p: Option<&TaskProfile>) -> String {
    let Some(p) = p else {
        return format!(
            "task {task_id}: no profile yet (type undecided). \
             The type is settled by the boss pressing Start on a kickoff card \
             (fleet__task action=kickoff), not by you; you may write the goal \
             with fleet__task action=set goal=....\n"
        );
    };
    let mut out = format!("task {task_id}\ntype: {}\n", p.task_type.as_str());
    match p.goal.as_deref() {
        Some(g) => out.push_str(&format!("goal: {g}\n")),
        None => out.push_str("goal: (not set)\n"),
    }
    let by = match p.set_by {
        ProfileSetter::Agent => "agent",
        ProfileSetter::Boss => "boss",
    };
    out.push_str(&format!("last set by: {by}\n"));
    for c in p.goal_history.iter().filter(|c| c.from.is_some()) {
        out.push_str(&format!(
            "  goal changed: \"{}\" -> \"{}\" ({})\n",
            c.from.as_deref().unwrap_or(""),
            c.to,
            c.reason.as_deref().unwrap_or("no reason"),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "fleet-task-profile-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn unknown_type_reads_as_general() {
        let p: TaskProfile =
            serde_json::from_str(r#"{"taskId":"t","type":"ops","setBy":"agent"}"#).unwrap();
        assert_eq!(p.task_type, TaskType::General);
        let p: TaskProfile = serde_json::from_str(r#"{"taskId":"t"}"#).unwrap();
        assert_eq!(p.task_type, TaskType::General);
        assert_eq!(TaskType::parse("Research"), Some(TaskType::Research));
        assert_eq!(TaskType::parse("ops"), None);
    }

    #[test]
    fn type_serializes_under_the_type_key() {
        let p = TaskProfile {
            task_id: "t".into(),
            task_type: TaskType::Dev,
            ..Default::default()
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["type"], "dev");
        assert_eq!(v["setBy"], "agent");
    }

    #[test]
    fn first_goal_needs_no_reason_and_a_change_needs_one() {
        let dir = tmp("goal");
        let p = set_in(&dir, "t", Some(TaskType::Research), Some("答清 X"), None, ProfileSetter::Boss, 1)
            .unwrap();
        assert_eq!(p.goal.as_deref(), Some("答清 X"));
        assert_eq!(p.goal_history.len(), 1);
        assert_eq!(p.goal_history[0].from, None);
        assert_eq!(p.goal_history[0].reason, None);

        let err = set_in(&dir, "t", None, Some("别的"), None, ProfileSetter::Agent, 2).unwrap_err();
        assert!(err.contains("without saying why"), "{err}");

        // Restating the same goal is not a change.
        set_in(&dir, "t", None, Some("答清 X"), None, ProfileSetter::Agent, 3).unwrap();

        let p = set_in(&dir, "t", None, Some("答清 Y"), Some("老板改了路线"), ProfileSetter::Boss, 4)
            .unwrap();
        assert_eq!(p.goal.as_deref(), Some("答清 Y"));
        assert_eq!(p.task_type, TaskType::Research, "type untouched by a goal-only set");
        assert_eq!(p.set_by, ProfileSetter::Boss);
        let last = p.goal_history.last().unwrap();
        assert_eq!(last.from.as_deref(), Some("答清 X"));
        assert_eq!(last.reason.as_deref(), Some("老板改了路线"));
        assert_eq!(last.by, ProfileSetter::Boss);
        assert_eq!(read_in(&dir, "t"), Some(p));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn type_only_set_keeps_the_goal_and_empty_set_is_refused() {
        let dir = tmp("type");
        set_in(&dir, "t", None, Some("g"), None, ProfileSetter::Agent, 1).unwrap();
        let p = set_in(&dir, "t", Some(TaskType::Dev), None, None, ProfileSetter::Boss, 2).unwrap();
        assert_eq!((p.task_type, p.goal.as_deref()), (TaskType::Dev, Some("g")));
        assert!(set_in(&dir, "t", None, Some("  "), None, ProfileSetter::Agent, 3).is_err());
        assert!(set_in(&dir, "../x", Some(TaskType::Dev), None, None, ProfileSetter::Agent, 3).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn profile_goal_wins_over_the_chain_goal_and_old_chains_still_resolve() {
        assert_eq!(pick_goal(Some("profile".into()), Some("chain")).as_deref(), Some("profile"));
        assert_eq!(pick_goal(None, Some("chain")).as_deref(), Some("chain"));
        assert_eq!(pick_goal(Some(" ".into()), Some("chain")).as_deref(), Some("chain"));
        assert_eq!(pick_goal(None, None), None);
    }

    #[test]
    fn a_relay_goal_is_checked_against_and_mirrored_into_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        let _home = crate::paths::fleet_home_guard(dir.path());
        sync_handoff_goal("h1", "目标 A", None, true).unwrap();
        assert_eq!(read("h1"), None, "check_only writes nothing");
        sync_handoff_goal("h1", "目标 A", None, false).unwrap();
        assert_eq!(read("h1").unwrap().goal.as_deref(), Some("目标 A"));
        assert!(sync_handoff_goal("h1", "目标 B", None, true).is_err());
        sync_handoff_goal("h1", "目标 B", Some("老板改了"), false).unwrap();
        assert_eq!(main_line_goal("h1").as_deref(), Some("目标 B"));
    }

    /// v1 let agents label their own task on the first turn, mostly by guessing.
    /// Those labels must read as undecided; a boss-written type survives, and
    /// so does it across a later agent goal edit (which flips `set_by`).
    #[test]
    fn only_a_boss_written_type_counts() {
        let dir = tmp("norm");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("legacy.json"),
            r#"{"taskId":"legacy","type":"research","setBy":"agent","updated":1}"#,
        )
        .unwrap();
        assert_eq!(read_in(&dir, "legacy").unwrap().task_type, TaskType::General);
        // A legacy boss-written profile predates `typeSetBy` too: also undecided.
        std::fs::write(
            dir.join("old-boss.json"),
            r#"{"taskId":"old-boss","type":"dev","setBy":"boss","updated":1}"#,
        )
        .unwrap();
        assert_eq!(read_in(&dir, "old-boss").unwrap().task_type, TaskType::General);

        let p = set_in(&dir, "a", Some(TaskType::Dev), None, None, ProfileSetter::Agent, 1).unwrap();
        assert_eq!(p.task_type, TaskType::General, "an agent-written type never counts");

        set_in(&dir, "b", Some(TaskType::Research), None, None, ProfileSetter::Boss, 1).unwrap();
        let p = set_in(&dir, "b", None, Some("g"), None, ProfileSetter::Agent, 2).unwrap();
        assert_eq!((p.task_type, p.set_by), (TaskType::Research, ProfileSetter::Agent));
        assert_eq!(read_in(&dir, "b").unwrap().task_type, TaskType::Research);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_without_profile_tells_the_agent_to_set_one() {
        assert!(render("t", None).contains("action=set"));
        let p = TaskProfile {
            task_id: "t".into(),
            task_type: TaskType::Research,
            goal: Some("g".into()),
            ..Default::default()
        };
        let text = render("t", Some(&p));
        assert!(text.contains("type: research") && text.contains("goal: g"), "{text}");
    }
}
