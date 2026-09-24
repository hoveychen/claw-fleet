//! Plan reviver — wake a fresh session for a plan that still has pending
//! P-tasks but that nobody is responsible for any more.
//!
//! Plans get stranded. A session's process dies mid-plan, or ends its turn
//! claiming it "subscribed to an idle notification" that was never registered
//! (seen 2026-09-23: anatole-mono `rail-failures-and-cost`, P3 sat for 41h with
//! no watch on file). Nothing in Fleet noticed, because every existing wake-up
//! mechanism (watch, schedule, loop, handoff) has to be armed by the session
//! that is about to go quiet.
//!
//! The hard part is not the timer, it is "responsible". Of the seven recently
//! touched orphan-looking plans on the dev machine that day, only one was
//! actually stranded: the rest were waiting on a watch, on a card, or had been
//! closed by the boss on purpose. So a plan counts as covered when any session
//! attributed to it (or to a descendant plan, or a handoff successor of either)
//! is alive, owns a live watch / schedule / loop, has a card waiting, or has a
//! registered handoff; or when the plan is snoozed ([`crate::plan_snooze`]).
//!
//! Policy (decided by the boss, 2026-09-23):
//! - Only plans somebody claimed within [`RECENCY_MS`] are considered; never-
//!   claimed and long-abandoned plans are left alone.
//! - Revival always spawns a **new** session (never resumes the old one) and
//!   pre-attributes it to the plan.
//! - A plan whose newest session was closed by the boss's terminal button is
//!   not revived directly: Fleet raises a card and asks first.
//! - Pressing 「结束任务」 on a session whose plan is fully done continues the
//!   tree at once, in DFS order ([`continue_after_finish`]); a plan with boxes
//!   still open keeps the ask-first path above.
//! - [`MAX_FRUITLESS`] revivals in a row without a checkbox ticked snooze the
//!   plan and raise a card, so a plan that keeps failing cannot burn sessions.
//!
//! Runs from the 30 s ticker in both hosts (desktop and `fleet serve`). The
//! whole tick runs under an exclusive file lock on the state store, so two
//! hosts on one machine serialize and the second sees the first's spawn.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::elicitation::{ElicitationOption, ElicitationQuestion, ElicitationRequest};
use crate::plan_snooze::{self, PlanSnooze};
use crate::prd_tasks as pt;
use crate::task_progress::TaskProgressRecord;

/// A plan must be uncovered this long before a session is woken for it.
pub const ORPHAN_GRACE_MS: u64 = 30 * 60 * 1000;
/// Plans whose newest claim is older than this are out of scope.
pub const RECENCY_MS: u64 = 7 * 24 * 3600 * 1000;
/// At most this many revived sessions alive at once, machine-wide.
pub const MAX_CONCURRENT: usize = 2;
/// Consecutive revivals without progress before Fleet stops and asks.
pub const MAX_FRUITLESS: u32 = 3;
/// Snooze Fleet applies when it gives up after [`MAX_FRUITLESS`] revivals.
const FRUITLESS_SNOOZE_MS: u64 = 24 * 3600 * 1000;
/// Snooze for "静默 7 天" and for a dismissed card.
const WEEK_MS: u64 = 7 * 24 * 3600 * 1000;
/// Snooze when one of our cards vanished without an answer.
const WITHDRAWN_SNOOZE_MS: u64 = 24 * 3600 * 1000;

// Card labels. Product copy (Chinese), matched back on answer.
const OPT_REVIVE: &str = "起新会话继续";
const OPT_RETRY: &str = "再唤醒一次";
const OPT_WEEK: &str = "静默 7 天";
const OPT_FOREVER: &str = "别再管这个计划";

// ── Config ──────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase", default)]
pub struct PlanReviveConfig {
    pub enabled: bool,
}

impl Default for PlanReviveConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

fn config_path() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("plan-revive.json"))
}

impl PlanReviveConfig {
    pub fn load() -> Self {
        config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path().ok_or("cannot determine fleet dir")?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        crate::atomic_json::write_atomic(&path, &bytes).map_err(|e| e.to_string())
    }
}

// ── Persistent per-plan state ───────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AskKind {
    /// The boss closed the plan's last session; asking before reviving.
    BossClosed,
    /// [`MAX_FRUITLESS`] revivals made no progress; Fleet snoozed and asks.
    Fruitless,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct PlanReviveState {
    pub workspace_path: String,
    pub plan_id: String,
    /// First tick at which the plan was seen with nobody responsible.
    pub orphan_since_ms: Option<u64>,
    /// Revivals since the last observed progress.
    pub attempts: u32,
    /// Plan's done count when it was last revived — progress is "more than this".
    pub done_at_last_revive: u32,
    pub last_revive_ms: Option<u64>,
    pub revived_session_id: Option<String>,
    /// Card Fleet raised for this plan and is waiting on.
    pub ask_card_id: Option<String>,
    pub ask_kind: Option<AskKind>,
    /// The boss said "go" on a [`AskKind::BossClosed`] card.
    pub boss_approved: bool,
    /// Free text the boss typed on one of our cards, handed to the next revival.
    pub boss_note: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
struct StateFile {
    plans: BTreeMap<String, PlanReviveState>,
}

fn state_path() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("plan-revive-state.json"))
}

fn load_state(path: &Path) -> Option<StateFile> {
    match crate::atomic_json::load_preserving::<StateFile>(path) {
        crate::atomic_json::JsonLoad::Loaded(s) => Some(s),
        crate::atomic_json::JsonLoad::Missing | crate::atomic_json::JsonLoad::Corrupt => {
            Some(StateFile::default())
        }
        // Transient read failure: skip this tick rather than clobber the store.
        crate::atomic_json::JsonLoad::Unreadable => None,
    }
}

// ── The plan view the decision runs on ──────────────────────────────────────

/// One pending, recently claimed plan, with everything the decision needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanView {
    pub workspace_path: String,
    pub plan_id: String,
    pub title: Option<String>,
    pub done: u32,
    pub total: u32,
    pub next_task: Option<String>,
    /// Every session that can be responsible: direct and descendant-plan
    /// claimants plus their handoff successors. Newest claim first.
    pub owners: Vec<String>,
    /// The session holding the newest claim, after following handoff links.
    pub newest_owner: String,
    /// `true` when `newest_owner` carries a terminal outcome from the boss.
    pub boss_closed: bool,
}

/// Build the candidate views from every focus record. `successor` follows a
/// handoff link, `closed` says whether the boss closed a session; both are
/// injected so tests stay off the real stores.
pub fn collect_views(
    records: &[(String, TaskProgressRecord)],
    now: u64,
    successor: &dyn Fn(&str) -> Option<String>,
    closed: &dyn Fn(&str) -> bool,
    load_blocks: &dyn Fn(&str) -> Vec<pt::SourcedBlock>,
) -> Vec<PlanView> {
    let mut by_ws: BTreeMap<String, Vec<&(String, TaskProgressRecord)>> = BTreeMap::new();
    for r in records {
        by_ws
            .entry(r.1.workspace_path.trim_end_matches('/').to_string())
            .or_default()
            .push(r);
    }
    let mut out = Vec::new();
    for (ws, recs) in by_ws {
        // Cheap pre-filter: skip reading TASKS.md for workspaces nobody touched.
        if !recs.iter().any(|r| now.saturating_sub(r.1.updated) < RECENCY_MS) {
            continue;
        }
        let blocks = load_blocks(&ws);
        let mut parent: HashMap<&str, &str> = HashMap::new();
        let mut pending: HashSet<&str> = HashSet::new();
        for b in &blocks {
            let Some(id) = b.id.as_deref() else { continue };
            if let Some(p) = b.parent.as_deref() {
                parent.insert(id, p);
            }
            if b.body.lines().any(pt::is_pending_task_line) {
                pending.insert(id);
            }
        }
        // Plans with a pending descendant are skipped: the descendant is the
        // live edge of the tree, and finishing it backtracks to the parent.
        let mut has_pending_descendant: HashSet<&str> = HashSet::new();
        for id in &pending {
            let mut cur = *id;
            let mut hops = 0;
            while let Some(p) = parent.get(cur) {
                has_pending_descendant.insert(p);
                cur = p;
                hops += 1;
                if hops > 64 {
                    break; // parent cycle; tree shaping reports those elsewhere
                }
            }
        }
        let mut borrowed: HashSet<&str> = HashSet::new();
        for b in &blocks {
            let Some(id) = b.id.as_deref() else { continue };
            if !pending.contains(id) || has_pending_descendant.contains(id) {
                continue;
            }
            // The plan and every plan below it.
            let mut family: HashSet<&str> = HashSet::from([id]);
            loop {
                let before = family.len();
                for (c, p) in &parent {
                    if family.contains(p) {
                        family.insert(c);
                    }
                }
                if family.len() == before {
                    break;
                }
            }
            let mut claims: Vec<&(String, TaskProgressRecord)> = recs
                .iter()
                .copied()
                .filter(|r| family.contains(r.1.plan_id.as_str()))
                .collect();
            if claims.is_empty() {
                // Never claimed: a child plan written down but not started yet.
                // Its parent is skipped above (it has a pending descendant), so
                // borrow the nearest ancestor's own claims or the tree would be
                // invisible. One child per ancestor, in file order, so an
                // ancestor never fans out into several revived sessions.
                let mut cur = id;
                let mut hops = 0;
                while let Some(p) = parent.get(cur) {
                    let own: Vec<_> =
                        recs.iter().copied().filter(|r| r.1.plan_id == *p).collect();
                    if !own.is_empty() {
                        if borrowed.insert(*p) {
                            claims = own;
                        }
                        break;
                    }
                    cur = p;
                    hops += 1;
                    if hops > 64 {
                        break;
                    }
                }
            }
            claims.sort_by(|a, b| b.1.updated.cmp(&a.1.updated));
            let Some(newest) = claims.first() else { continue };
            if now.saturating_sub(newest.1.updated) >= RECENCY_MS {
                continue;
            }
            let (owners, newest_owner) = follow_owners(&claims, successor);
            let (done, total) = pt::count_tasks(&b.body);
            out.push(PlanView {
                workspace_path: ws.clone(),
                plan_id: id.to_string(),
                title: pt::extract_plan_name(&b.body),
                done,
                total,
                next_task: pt::first_pending_task(&b.body),
                boss_closed: closed(&newest_owner),
                newest_owner,
                owners,
            });
        }
    }
    out
}

// ── Coverage ────────────────────────────────────────────────────────────────

/// Session-level facts, gathered once per tick.
#[derive(Default, Debug)]
pub struct Coverage {
    pub alive: HashSet<String>,
    pub watching: HashSet<String>,
    pub scheduled: HashSet<String>,
    pub carded: HashSet<String>,
    pub handing_off: HashSet<String>,
}

impl Coverage {
    /// Why the plan is covered, or `None` when nobody is on it.
    pub fn reason(&self, owners: &[String]) -> Option<String> {
        let (sid, state) = self.covering(owners)?;
        let short = &sid[..sid.len().min(8)];
        Some(match state {
            AttendanceState::Running => format!("session {short} is running"),
            AttendanceState::Watching => format!("session {short} owns a live watch"),
            AttendanceState::Scheduled => format!("session {short} owns a pending schedule/loop"),
            AttendanceState::WaitingCard => format!("session {short} has a decision card waiting"),
            _ => format!("session {short} registered a handoff"),
        })
    }

    /// The first owner that covers the plan and how, or `None` when nobody is
    /// on it. Owners are checked in order, each against every kind of cover.
    pub fn covering<'a>(&self, owners: &'a [String]) -> Option<(&'a str, AttendanceState)> {
        for sid in owners {
            let s = sid.as_str();
            let state = if self.alive.contains(s) {
                AttendanceState::Running
            } else if self.watching.contains(s) {
                AttendanceState::Watching
            } else if self.scheduled.contains(s) {
                AttendanceState::Scheduled
            } else if self.carded.contains(s) {
                AttendanceState::WaitingCard
            } else if self.handing_off.contains(s) {
                AttendanceState::HandingOff
            } else {
                continue;
            };
            return Some((s, state));
        }
        None
    }
}

// ── Attendance for the plan-tree view ───────────────────────────────────────

/// How a plan's responsible session stands. The first five mean somebody is on
/// the plan (the reviver's "covered"); the last three mean nobody is.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum AttendanceState {
    Running,
    Watching,
    Scheduled,
    WaitingCard,
    HandingOff,
    /// Claimed recently, but the session is gone and left nothing armed.
    Idle,
    /// Like `Idle`, and the boss closed that session with the terminal button.
    BossClosed,
    /// Newest claim is older than [`RECENCY_MS`]; the reviver ignores it.
    Stale,
}

impl AttendanceState {
    pub fn is_covered(self) -> bool {
        !matches!(self, Self::Idle | Self::BossClosed | Self::Stale)
    }
}

/// Who is responsible for a plan right now.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PlanAttendance {
    /// The covering session when there is one, otherwise the newest claimant
    /// (after following its handoff successors).
    pub session_id: String,
    pub state: AttendanceState,
    /// Epoch ms of the newest claim on the plan.
    pub claimed_at: u64,
}

/// What the reviver will do about a plan nobody is on. Only set on plans the
/// reviver actually tracks (pending, recently claimed, no pending descendant).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ReviveOutlook {
    /// A fresh session will be spawned at about `at` (epoch ms).
    Revive { at: u64 },
    /// The boss closed the last session; Fleet will raise a card first.
    AskBoss,
    /// A card of Fleet's about this plan is already waiting for the boss.
    Asked,
    /// The reviver is switched off in settings.
    Disabled,
}

/// Attendance for every plan of one workspace.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct WorkspaceAttendance {
    pub plans: HashMap<String, PlanAttendance>,
    pub outlook: HashMap<String, ReviveOutlook>,
}

/// Direct claimants of each plan: the sessions whose focus record names it,
/// each followed down its handoff successors. Newest claim first.
///
/// A claim whose relay ended in a session that has since focused on a
/// different plan is dropped: that successor is working over there now, and
/// counting it here would put one live session on every plan its chain ever
/// passed through.
fn direct_claims(
    records: &[(String, TaskProgressRecord)],
    successor: &dyn Fn(&str) -> Option<String>,
) -> HashMap<String, (Vec<String>, String, u64)> {
    let focus: HashMap<&str, &str> =
        records.iter().map(|r| (r.0.as_str(), r.1.plan_id.as_str())).collect();
    let chain_end = |sid: &str| {
        let mut cur = sid.to_string();
        for _ in 0..64 {
            match successor(&cur) {
                Some(next) => cur = next,
                None => break,
            }
        }
        cur
    };
    let mut by_plan: HashMap<&str, Vec<&(String, TaskProgressRecord)>> = HashMap::new();
    for r in records {
        let end = chain_end(&r.0);
        if focus.get(end.as_str()).is_some_and(|p| *p != r.1.plan_id) {
            continue;
        }
        by_plan.entry(r.1.plan_id.as_str()).or_default().push(r);
    }
    let mut out = HashMap::new();
    for (plan, mut claims) in by_plan {
        claims.sort_by(|a, b| b.1.updated.cmp(&a.1.updated));
        let (owners, newest) = follow_owners(&claims, successor);
        out.insert(plan.to_string(), (owners, newest, claims[0].1.updated));
    }
    out
}

/// Every session reachable from `claims` through handoff links, deduplicated,
/// plus where the newest claim's chain ends up.
fn follow_owners(
    claims: &[&(String, TaskProgressRecord)],
    successor: &dyn Fn(&str) -> Option<String>,
) -> (Vec<String>, String) {
    let mut owners: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut newest_owner = claims.first().map(|c| c.0.clone()).unwrap_or_default();
    for (i, c) in claims.iter().enumerate() {
        let mut sid = c.0.clone();
        let mut hops = 0;
        loop {
            if seen.insert(sid.clone()) {
                owners.push(sid.clone());
            }
            match successor(&sid) {
                Some(next) if hops < 64 => {
                    sid = next;
                    hops += 1;
                }
                _ => break,
            }
        }
        if i == 0 {
            newest_owner = sid;
        }
    }
    (owners, newest_owner)
}

/// Is a focus record's workspace the workspace rooted at `main_root`? Records
/// name the main checkout, but a session started inside a worktree may name
/// that instead.
fn in_workspace(record_ws: &str, main_root: &str) -> bool {
    let r = record_ws.trim_end_matches('/');
    r == main_root || r.starts_with(&format!("{main_root}/.worktrees/"))
}

/// Pure core of [`workspace_attendance`]: every store is passed in.
#[allow(clippy::too_many_arguments)]
pub fn resolve_attendance(
    records: &[(String, TaskProgressRecord)],
    blocks: &[pt::SourcedBlock],
    now: u64,
    successor: &dyn Fn(&str) -> Option<String>,
    closed: &dyn Fn(&str) -> bool,
    coverage: &dyn Fn(&HashSet<String>) -> Coverage,
    revive_state: &dyn Fn(&str) -> Option<PlanReviveState>,
    snoozed: &dyn Fn(&str) -> bool,
    enabled: bool,
) -> WorkspaceAttendance {
    let direct = direct_claims(records, successor);
    let views = collect_views(records, now, successor, closed, &|_| blocks.to_vec());
    // Claims past the reviver's window are not worth a per-session Codex probe
    // each (hundreds of them in a busy repo); a live one still shows up through
    // the machine-wide process and watch sets.
    let owners: HashSet<String> = direct
        .values()
        .filter(|d| now.saturating_sub(d.2) < RECENCY_MS)
        .flat_map(|d| d.0.iter().cloned())
        .chain(views.iter().flat_map(|v| v.owners.iter().cloned()))
        .collect();
    let cov = coverage(&owners);

    let attend = |owners: &[String], newest: &str, claimed_at: u64| {
        if let Some((sid, state)) = cov.covering(owners) {
            return PlanAttendance { session_id: sid.to_string(), state, claimed_at };
        }
        let state = if now.saturating_sub(claimed_at) >= RECENCY_MS {
            AttendanceState::Stale
        } else if closed(newest) {
            AttendanceState::BossClosed
        } else {
            AttendanceState::Idle
        };
        PlanAttendance { session_id: newest.to_string(), state, claimed_at }
    };

    let mut out = WorkspaceAttendance::default();
    for (plan, (owners, newest, at)) in &direct {
        out.plans.insert(plan.clone(), attend(owners, newest, *at));
    }
    // The reviver's view of a live-edge plan is wider (descendant claimants,
    // a borrowed ancestor claim), and it is the one the reviver acts on, so it
    // wins for those plans.
    for v in &views {
        let at = records
            .iter()
            .filter(|r| v.owners.contains(&r.0))
            .map(|r| r.1.updated)
            .max()
            .unwrap_or(0);
        let a = attend(&v.owners, &v.newest_owner, at);
        let covered = a.state.is_covered();
        out.plans.insert(v.plan_id.clone(), a);
        if covered || snoozed(&v.plan_id) {
            continue;
        }
        let outlook = if !enabled {
            ReviveOutlook::Disabled
        } else {
            let st = revive_state(&v.plan_id).unwrap_or_default();
            if st.ask_card_id.is_some() {
                ReviveOutlook::Asked
            } else if v.boss_closed && !st.boss_approved {
                ReviveOutlook::AskBoss
            } else {
                let since = st.orphan_since_ms.unwrap_or(now);
                ReviveOutlook::Revive { at: since + ORPHAN_GRACE_MS }
            }
        };
        out.outlook.insert(v.plan_id.clone(), outlook);
    }
    out
}

/// Attendance for the workspace rooted at `main_root`, from the real stores.
/// `blocks` are the workspace's already-loaded plan blocks.
pub fn workspace_attendance(main_root: &str, blocks: &[pt::SourcedBlock]) -> WorkspaceAttendance {
    let ws = main_root.trim_end_matches('/');
    let records: Vec<(String, TaskProgressRecord)> = crate::task_progress::all_records()
        .into_iter()
        .filter(|(_, r)| in_workspace(&r.workspace_path, ws))
        .map(|(sid, mut r)| {
            r.workspace_path = ws.to_string();
            (sid, r)
        })
        .collect();
    if records.is_empty() {
        return WorkspaceAttendance::default();
    }
    let state = state_path().and_then(|p| load_state(&p)).unwrap_or_default();
    let successors = crate::handoff::successor_index();
    resolve_attendance(
        &records,
        blocks,
        plan_snooze::now_ms(),
        &|sid| successors.get(sid).cloned(),
        &|sid| crate::task_outcome::read(sid).is_some(),
        &gather_coverage,
        &|plan| state.plans.get(&plan_snooze::plan_key(ws, plan)).cloned(),
        &|plan| plan_snooze::active(ws, plan).is_some(),
        PlanReviveConfig::load().enabled,
    )
}

/// Gather [`Coverage`] for `owners` from the real stores.
fn gather_coverage(owners: &HashSet<String>) -> Coverage {
    let now = plan_snooze::now_ms();
    let mut c = Coverage::default();

    // Liveness: argv-pinned processes (every Fleet spawn) plus the CLI's own
    // registrations (interactive terminal sessions carry no id in argv).
    for p in crate::session::scan_cli_processes() {
        if let Some(id) = p.resume_session_id {
            c.alive.insert(id);
        }
    }
    c.alive.extend(crate::live_inject::live_registered_session_ids());
    // One Codex process scan for every owner, and the ownership lookup only for
    // live threads: for a Claude session id it misses SQLite and falls back to
    // reading every Codex rollout.
    let unknown = owners.iter().filter(|s| !c.alive.contains(*s)).map(String::as_str);
    for sid in crate::codex_source::codex_session_pids(unknown).into_keys() {
        if crate::codex_source::codex_fleet_owned_cwd(&sid).is_some() {
            c.alive.insert(sid);
        }
    }

    for w in crate::watch::list() {
        if w.is_live(now) {
            c.watching.insert(w.session_id);
        }
    }
    for s in crate::schedule::list() {
        if s.is_pending() {
            if let Some(by) = s.created_by_session {
                c.scheduled.insert(by);
            }
        }
    }
    for l in crate::agent_loop::list() {
        if l.is_live(now) {
            if let Some(by) = l.created_by_session {
                c.scheduled.insert(by);
            }
        }
    }

    for id in crate::mcp_ipc::list_pending_requests() {
        if let Some(r) = crate::mcp_ipc::read_request(&id) {
            c.carded.insert(r.session_id);
        }
    }
    for id in crate::elicitation::list_pending_requests() {
        if let Some(r) = crate::elicitation::read_request(&id) {
            c.carded.insert(r.session_id);
        }
    }
    for id in crate::plan_approval::list_pending_requests() {
        if let Some(r) = crate::plan_approval::read_request(&id) {
            c.carded.insert(r.session_id);
        }
    }
    for id in crate::mcp_a2ui_ipc::list_pending_requests() {
        if let Some(r) = crate::mcp_a2ui_ipc::read_request(&id) {
            c.carded.insert(r.session_id);
        }
    }
    for p in crate::parked::list() {
        c.carded.insert(p.session_id);
    }

    for sid in owners {
        if crate::handoff::read_pending(sid).is_some() {
            c.handing_off.insert(sid.clone());
        }
    }
    c
}

// ── The decision ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Idle,
    Revive,
    AskBossClosed,
    GiveUp,
}

/// Advance one plan's state by one tick and say what to do. Pure: the caller
/// performs the step and records its effects.
pub fn decide(
    view: &PlanView,
    covered: bool,
    snoozed: bool,
    st: &mut PlanReviveState,
    now: u64,
    slot_free: bool,
) -> Step {
    if st.ask_card_id.is_some() {
        return Step::Idle;
    }
    if view.done > st.done_at_last_revive {
        st.attempts = 0;
    }
    if snoozed || covered {
        st.orphan_since_ms = None;
        return Step::Idle;
    }
    let since = *st.orphan_since_ms.get_or_insert(now);
    if now.saturating_sub(since) < ORPHAN_GRACE_MS {
        return Step::Idle;
    }
    if view.boss_closed && !st.boss_approved {
        return Step::AskBossClosed;
    }
    if st.attempts >= MAX_FRUITLESS {
        return Step::GiveUp;
    }
    if !slot_free {
        return Step::Idle;
    }
    Step::Revive
}

/// Apply the boss's answer on one of our cards to the state. Returns the
/// snooze to set, if any: `Some(Some(ms))` for a timed one, `Some(None)` for
/// indefinite.
pub fn apply_answer(
    st: &mut PlanReviveState,
    declined: bool,
    answer: Option<&str>,
    now: u64,
) -> Option<Option<u64>> {
    let kind = st.ask_kind.take();
    st.ask_card_id = None;
    if declined {
        return Some(Some(WEEK_MS));
    }
    let wake_now = |st: &mut PlanReviveState| {
        st.orphan_since_ms = Some(now.saturating_sub(ORPHAN_GRACE_MS));
    };
    match answer.map(str::trim) {
        Some(OPT_WEEK) => Some(Some(WEEK_MS)),
        Some(OPT_FOREVER) => Some(None),
        Some(OPT_REVIVE) | Some(OPT_RETRY) | None | Some("") => {
            if kind == Some(AskKind::Fruitless) {
                st.attempts = MAX_FRUITLESS - 1;
            }
            st.boss_approved = true;
            wake_now(st);
            None
        }
        // Free text: treat as "go, and here is what I want".
        Some(other) => {
            if kind == Some(AskKind::Fruitless) {
                st.attempts = MAX_FRUITLESS - 1;
            }
            st.boss_approved = true;
            st.boss_note = Some(other.to_string());
            wake_now(st);
            None
        }
    }
}

// ── Side effects ────────────────────────────────────────────────────────────

fn plan_label(view: &PlanView) -> String {
    match &view.title {
        Some(t) => format!("`{}`（{t}）", view.plan_id),
        None => format!("`{}`", view.plan_id),
    }
}

fn workspace_label(ws: &str) -> String {
    Path::new(ws)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| ws.to_string())
}

fn transcript_of(sid: &str) -> Option<String> {
    let projects = crate::session::get_claude_dir()?.join("projects");
    for dir in std::fs::read_dir(projects).ok()?.flatten() {
        let p = dir.path().join(format!("{sid}.jsonl"));
        if p.exists() {
            return Some(p.to_string_lossy().to_string());
        }
    }
    None
}

/// The revived session's opening prompt. Product text (Chinese).
pub fn revive_prompt(view: &PlanView, boss_note: Option<&str>, previous: Option<&str>) -> String {
    let pending = view.total.saturating_sub(view.done);
    let next = view.next_task.as_deref().unwrap_or("第一个未完成的 P");
    let mut s = format!(
        "（Fleet 自动唤醒 —— 这是 Fleet 发起的接手，不是老板刚打的字）\n\n\
         计划 {label} 还有 {pending} 个 P-task 没完成（{done}/{total}），下一个是：{next}。\
         但已经超过 30 分钟没有任何会话在负责它：没有活着的进程、没有挂着的 watch/schedule、\
         没有等答复的决策卡、也没有登记中的接力。你是 Fleet 为它起的新会话，已经归属到这个计划。\n\n\
         先恢复上下文：读 TASKS.md 里这个计划块（含子 bullet 备注），看 git log 和 `.worktrees/` \
         下对应的 worktree 分支有没有未合并的进度。",
        label = plan_label(view),
        done = view.done,
        total = view.total,
    );
    match previous {
        Some(path) => s.push_str(&format!(
            "上一个负责它的会话是 `{}`，转录在 `{path}`，从尾部往前读它最后在干什么、为什么停下。",
            view.newest_owner
        )),
        None => s.push_str(&format!(
            "上一个负责它的会话是 `{}`（本机没找到它的 Claude 转录）。",
            view.newest_owner
        )),
    }
    s.push_str(
        "\n\n然后三选一：\n\
         - **能推进** → 按 Rule 4 的节奏直接做下一个 P，不用先问。\n\
         - **被真实阻塞**（在等老板拍板、等外部条件、缺登录/权限、需要真机或人工）→ 不要硬干。\
         能用 `fleet__watch` 等的外部条件就挂 watch（挂着 watch 就算有人负责，不会再被唤醒）；\
         否则用 `fleet__plan` 的 `snooze`（`plan_id`、`duration` 如 `8h`/`3d`、`reason` 写清卡在哪）设静默期，\
         再用决策卡把阻塞报给老板。\n\
         - **计划已经没有意义了**（被别的计划取代、老板改了方向）→ 用决策卡问老板要不要删掉剩下的 P，别自己删。\n\n\
         注意：Fleet 连续唤醒这个计划 3 次都没勾掉任何一个框，就会自动静默并去问老板。",
    );
    if let Some(note) = boss_note {
        s.push_str(&format!("\n\n老板在 Fleet 的询问卡上回复了：{note}\n请按这句回复办。"));
    }
    s
}

fn spawn_revival(view: &PlanView, boss_note: Option<&str>) -> Result<String, String> {
    let prompt = revive_prompt(view, boss_note, transcript_of(&view.newest_owner).as_deref());
    spawn_for_plan(view, prompt, "唤醒")
}

/// Spawn a fresh Claude session on `view`'s plan and attribute it there.
/// `title_prefix` labels it in the session list (`唤醒：…`, `接续：…`).
fn spawn_for_plan(view: &PlanView, prompt: String, title_prefix: &str) -> Result<String, String> {
    let sid = uuid::Uuid::new_v4().to_string();
    // Continue on the model the plan was being worked on, when it is a Claude
    // one: the reviver pre-assigns a Claude session id so it can attribute the
    // spawn to the plan, which Codex (self-minted thread ids) cannot take.
    let model = crate::session::resolve_session_model_spec(&view.newest_owner)
        .filter(|m| m.starts_with("claude-"));
    let resp = crate::agent_source::spawn_session(
        "claude",
        &crate::agent_source::SpawnSpec {
            workspace_path: view.workspace_path.clone(),
            prompt,
            model,
            effort: None,
            permission_mode: None,
            session_id: Some(sid.clone()),
            entrypoint: crate::session_launch::NEW_SESSION_ENTRYPOINT.to_string(),
            images: Vec::new(),
        },
    )?;
    let sid = resp.session_id.unwrap_or(sid);
    let ws = Path::new(&view.workspace_path);
    let current = pt::resolve_current_task(ws, &view.plan_id, None).ok().flatten();
    if let Err(e) =
        crate::task_progress::set_current(&sid, &view.workspace_path, &view.plan_id, current)
    {
        crate::log_debug(&format!("plan revive: attribute {sid}: {e}"));
    }
    let title = format!("{title_prefix}：{}", view.title.as_deref().unwrap_or(&view.plan_id));
    let _ = crate::session_title::set_title(&sid, &view.workspace_path, Some(title));
    Ok(sid)
}

// ── Finish-button continuation ──────────────────────────────────────────────
//
// When the boss presses 「结束任务」 on a session whose plan is fully done, the
// plan tree may still have work elsewhere. Rather than wait out the orphan
// grace and then ask (the boss just closed that session, so `boss_closed`
// would hold the reviver back), continue the tree right away: the button *is*
// the boss's go-ahead. Only 「结束任务」 does this; 「放弃任务」 means stop.

/// The plan where the tree's work continues once `finished` is done, in DFS
/// order: the first pending plan below `finished`, else below its nearest
/// ancestor that still has pending work anywhere in its subtree, descending to
/// the deepest pending plan (children before their parent, file order among
/// siblings). `None` when `finished` itself still has pending tasks — the boss
/// decided that case keeps the existing ask-first behaviour — or when the whole
/// tree is done.
pub fn next_plan_after(blocks: &[pt::SourcedBlock], finished: &str) -> Option<String> {
    let mut parent: HashMap<&str, &str> = HashMap::new();
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut pending: HashSet<&str> = HashSet::new();
    for b in blocks {
        let Some(id) = b.id.as_deref() else { continue };
        if let Some(p) = b.parent.as_deref() {
            parent.insert(id, p);
            children.entry(p).or_default().push(id);
        }
        if b.body.lines().any(pt::is_pending_task_line) {
            pending.insert(id);
        }
    }
    if pending.contains(finished) {
        return None;
    }
    // `depth` bounds both recursions against parent cycles.
    fn subtree_pending(
        id: &str,
        children: &HashMap<&str, Vec<&str>>,
        pending: &HashSet<&str>,
        depth: u32,
    ) -> bool {
        depth < 64
            && (pending.contains(id)
                || children.get(id).is_some_and(|cs| {
                    cs.iter().any(|c| subtree_pending(c, children, pending, depth + 1))
                }))
    }
    fn descend(
        id: &str,
        children: &HashMap<&str, Vec<&str>>,
        pending: &HashSet<&str>,
        depth: u32,
    ) -> Option<String> {
        if depth >= 64 {
            return None;
        }
        for c in children.get(id).into_iter().flatten() {
            if subtree_pending(c, children, pending, depth + 1) {
                return descend(c, children, pending, depth + 1);
            }
        }
        pending.contains(id).then(|| id.to_string())
    }
    let mut cursor = finished;
    for _ in 0..64 {
        if let Some(t) = descend(cursor, &children, &pending, 0) {
            return Some(t);
        }
        cursor = parent.get(cursor)?;
    }
    None
}

/// Opening prompt for a session started by the finish button. Product text.
pub fn finish_prompt(view: &PlanView, finished_plan: &str, previous: Option<&str>) -> String {
    let pending = view.total.saturating_sub(view.done);
    let next = view.next_task.as_deref().unwrap_or("第一个未完成的 P");
    let mut s = format!(
        "（Fleet 自动接续 —— 这是 Fleet 发起的接手，不是老板刚打的字）\n\n\
         老板刚在计划 '{finished_plan}' 的会话上按了「结束任务」，那个计划已全部完成。\
         按计划树的顺序，下一个要做的是计划 {label}：还有 {pending} 个 P-task 没完成（{done}/{total}），\
         下一个是：{next}。你是 Fleet 为它起的新会话，已经归属到这个计划。\n\n\
         先恢复上下文：读 TASKS.md 里这个计划块（含子 bullet 备注）和它的父计划，看 git log 和 `.worktrees/` \
         下对应的 worktree 分支有没有未合并的进度。",
        label = plan_label(view),
        done = view.done,
        total = view.total,
    );
    if let Some(path) = previous {
        s.push_str(&format!(
            "刚结束的会话是 `{}`，转录在 `{path}`，需要时从尾部往前读它的结论。",
            view.newest_owner
        ));
    }
    s.push_str(
        "\n\n然后按 Rule 4 的节奏直接做下一个 P，不用先问。被真实阻塞（等老板拍板、等外部条件、\
         缺登录/权限、需要真机）就不要硬干：能等的外部条件挂 `fleet__watch`，否则用 `fleet__plan` 的 \
         `snooze` 写清卡在哪，再用决策卡报给老板。",
    );
    s
}

/// Called when the boss presses 「结束任务」 on `session_id`'s card: if the
/// plan that session (or the newest hop of its handoff chain) was focused on is
/// done and the tree has a next plan in DFS order that nobody is on, spawn a
/// session for it now. Best-effort — every failure is logged, never raised,
/// because the caller is unblocking an agent waiting on the card.
pub fn continue_after_finish(session_id: &str) {
    if !PlanReviveConfig::load().enabled {
        return;
    }
    let Some(focus) = crate::task_review::task_sessions(session_id)
        .iter()
        .rev()
        .find_map(|s| crate::task_progress::read(s))
    else {
        return;
    };
    let ws = focus.workspace_path.trim_end_matches('/').to_string();
    let blocks = load_workspace_blocks(&ws);
    let Some(target) = next_plan_after(&blocks, &focus.plan_id) else { return };
    let Some(block) = blocks.iter().find(|b| b.id.as_deref() == Some(target.as_str())) else {
        return;
    };
    if plan_snooze::active(&ws, &target).is_some() {
        return;
    }
    let owners: HashSet<String> = crate::task_progress::all_records()
        .into_iter()
        .filter(|(_, r)| r.plan_id == target && r.workspace_path.trim_end_matches('/') == ws)
        .map(|(sid, _)| sid)
        .filter(|sid| sid != session_id)
        .collect();
    let owner_list: Vec<String> = owners.iter().cloned().collect();
    if let Some(why) = gather_coverage(&owners).reason(&owner_list) {
        crate::log_debug(&format!("finish continuation: {target} already covered ({why})"));
        return;
    }
    let (done, total) = pt::count_tasks(&block.body);
    let view = PlanView {
        workspace_path: ws,
        plan_id: target.clone(),
        title: pt::extract_plan_name(&block.body),
        done,
        total,
        next_task: pt::first_pending_task(&block.body),
        owners: owner_list,
        newest_owner: session_id.to_string(),
        boss_closed: false,
    };
    let prompt = finish_prompt(&view, &focus.plan_id, transcript_of(session_id).as_deref());
    match spawn_for_plan(&view, prompt, "接续") {
        Ok(sid) => crate::log_debug(&format!(
            "finish continuation: {sid} picks up {target} after {}",
            focus.plan_id
        )),
        Err(e) => crate::log_debug(&format!("finish continuation: spawn for {target}: {e}")),
    }
}

fn build_card(view: &PlanView, kind: AskKind) -> ElicitationRequest {
    let pending = view.total.saturating_sub(view.done);
    let next = view.next_task.as_deref().unwrap_or("—");
    let (header, question, first) = match kind {
        AskKind::BossClosed => (
            "计划唤醒",
            format!(
                "计划 {label} 还剩 {pending} 个 P 没做，要起新会话接着干吗？\n---\n\
                 你之前已经按「结束任务」收了负责它的会话，所以 Fleet 没有自动唤醒，先来问你。\n\n\
                 - 进度：{done}/{total}，下一个：{next}\n\
                 - 上一个会话：`{owner}`\n\n\
                 要让新会话接手吗？",
                label = plan_label(view),
                done = view.done,
                total = view.total,
                owner = view.newest_owner,
            ),
            ElicitationOption {
                label: OPT_REVIVE.into(),
                description: "起一个新会话认领这个计划，从下一个 P 接着做".into(),
                preview: None,
            },
        ),
        AskKind::Fruitless => (
            "唤醒无进展",
            format!(
                "计划 {label} 连续唤醒 {n} 次都没推进，已自动静默 24 小时。\n---\n\
                 Fleet 为它起过 {n} 个新会话，没有一个勾掉任何一个框，所以先停手来问你。\n\n\
                 - 进度：{done}/{total}，下一个：{next}\n\
                 - 最近一次唤醒的会话：`{owner}`\n\n\
                 接下来怎么处理？",
                label = plan_label(view),
                n = MAX_FRUITLESS,
                done = view.done,
                total = view.total,
                owner = view.newest_owner,
            ),
            ElicitationOption {
                label: OPT_RETRY.into(),
                description: "解除静默，马上再起一个新会话试一次".into(),
                preview: None,
            },
        ),
    };
    ElicitationRequest {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: view.newest_owner.clone(),
        workspace_name: workspace_label(&view.workspace_path),
        ai_title: view.title.clone(),
        questions: vec![ElicitationQuestion {
            question,
            header: header.to_string(),
            options: vec![
                first,
                ElicitationOption {
                    label: OPT_WEEK.into(),
                    description: "7 天内不再唤醒，也不再问".into(),
                    preview: None,
                },
                ElicitationOption {
                    label: OPT_FOREVER.into(),
                    description: "一直静默，直到有人手动解除（fleet plan unsnooze）".into(),
                    preview: None,
                },
            ],
            multi_select: false,
        }],
        timestamp: chrono::Utc::now().to_rfc3339(),
        parked: false,
        turn_completion: false,
    }
}

// ── The tick ────────────────────────────────────────────────────────────────

/// How often a host actually runs a pass. One pass reads every recently used
/// workspace's TASKS.md and scans the process table (measured 2026-09-23:
/// ~5 s wall in a debug build over 18 candidate plans), and the grace window
/// is 30 minutes, so the 30 s ticker cadence would be pure waste.
const PASS_INTERVAL_MS: u64 = 5 * 60 * 1000;

static LAST_PASS_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PASS_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Called from the hosts' 30 s tickers. Runs a pass at most every
/// [`PASS_INTERVAL_MS`], on its own thread so the ticker's other jobs are not
/// held up. Never blocks.
pub fn maybe_tick_in_background() {
    use std::sync::atomic::Ordering;
    let now = plan_snooze::now_ms();
    if now.saturating_sub(LAST_PASS_MS.load(Ordering::Relaxed)) < PASS_INTERVAL_MS {
        return;
    }
    if PASS_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    LAST_PASS_MS.store(now, Ordering::Relaxed);
    let spawned = std::thread::Builder::new()
        .name("plan-revive".into())
        .spawn(|| {
            tick();
            PASS_RUNNING.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        PASS_RUNNING.store(false, Ordering::Release);
    }
}

/// Run one reviver pass. Cheap when nothing is pending; safe to call from
/// several processes at once (see the module docs).
pub fn tick() {
    if !PlanReviveConfig::load().enabled {
        return;
    }
    let Some(path) = state_path() else { return };
    crate::atomic_json::with_file_lock(&path, || tick_locked(&path));
}

fn load_workspace_blocks(ws: &str) -> Vec<pt::SourcedBlock> {
    let root = Path::new(ws);
    let main_root = pt::discover_main_checkout_root(root);
    let sources = pt::collect_task_sources(root, main_root.as_deref());
    let (raw, _) = pt::collect_from_sources(&sources, false);
    pt::dedup_blocks_keep_latest_mtime(raw)
}

fn tick_locked(path: &Path) {
    let Some(mut state) = load_state(path) else { return };
    let now = plan_snooze::now_ms();
    let records = crate::task_progress::all_records();
    // One pass over the chain store: a per-claim lookup re-reads every chain
    // file, and there are thousands of claims.
    let successors = crate::handoff::successor_index();
    let views = collect_views(
        &records,
        now,
        &|sid| successors.get(sid).cloned(),
        &|sid| crate::task_outcome::read(sid).is_some(),
        &load_workspace_blocks,
    );

    // Answers first: a card resolved since the last tick may unlock a revival
    // in this same tick.
    for st in state.plans.values_mut() {
        let Some(card) = st.ask_card_id.clone() else { continue };
        let snooze = if let Some(resp) = crate::elicitation::try_read_response(&card) {
            let answer = resp.answers.values().map(|s| s.trim()).find(|s| !s.is_empty());
            let out = apply_answer(st, resp.declined, answer, now);
            crate::elicitation::cleanup(&card);
            match out {
                Some(Some(ms)) => Some((Some(ms), "老板在 Fleet 的询问卡上选择了静默")),
                Some(None) => Some((None, "老板在 Fleet 的询问卡上选择了不再管这个计划")),
                None => {
                    plan_snooze::unsnooze(&st.workspace_path, &st.plan_id);
                    None
                }
            }
        } else if crate::elicitation::read_request(&card).is_none() {
            // Withdrawn without an answer; back off instead of re-asking at once.
            st.ask_card_id = None;
            st.ask_kind = None;
            Some((Some(WITHDRAWN_SNOOZE_MS), "Fleet 的询问卡被撤回、没有得到答复"))
        } else {
            None
        };
        if let Some((ms, reason)) = snooze {
            let r = match ms {
                Some(ms) => plan_snooze::snooze(&st.workspace_path, &st.plan_id, ms, reason, "fleet"),
                None => plan_snooze::snooze_indefinitely(&st.workspace_path, &st.plan_id, reason, "fleet"),
            };
            if let Err(e) = r {
                crate::log_debug(&format!("plan revive: snooze {}: {e}", st.plan_id));
            }
        }
    }

    let owners: HashSet<String> = views.iter().flat_map(|v| v.owners.iter().cloned()).collect();
    if views.is_empty() {
        write_state(path, &state, &views);
        return;
    }
    let coverage = gather_coverage(&owners);
    let mut running = state
        .plans
        .values()
        .filter_map(|s| s.revived_session_id.as_deref())
        .filter(|sid| coverage.alive.contains(*sid))
        .count();

    for view in &views {
        let key = plan_snooze::plan_key(&view.workspace_path, &view.plan_id);
        let st = state.plans.entry(key).or_insert_with(|| PlanReviveState {
            workspace_path: view.workspace_path.clone(),
            plan_id: view.plan_id.clone(),
            done_at_last_revive: view.done,
            ..Default::default()
        });
        let covered = coverage.reason(&view.owners);
        let snoozed = plan_snooze::active(&view.workspace_path, &view.plan_id).is_some();
        match decide(view, covered.is_some(), snoozed, st, now, running < MAX_CONCURRENT) {
            Step::Idle => {}
            Step::Revive => match spawn_revival(view, st.boss_note.as_deref()) {
                Ok(sid) => {
                    crate::log_debug(&format!(
                        "plan revive: woke {sid} for {} in {}",
                        view.plan_id, view.workspace_path
                    ));
                    st.attempts += 1;
                    st.done_at_last_revive = view.done;
                    st.last_revive_ms = Some(now);
                    st.revived_session_id = Some(sid);
                    st.orphan_since_ms = None;
                    st.boss_approved = false;
                    st.boss_note = None;
                    running += 1;
                }
                Err(e) => {
                    // Count it: a spawn that can never succeed must still hit
                    // the fruitless cap instead of retrying every tick.
                    crate::log_debug(&format!("plan revive: spawn for {}: {e}", view.plan_id));
                    st.attempts += 1;
                    st.orphan_since_ms = Some(now);
                }
            },
            step @ (Step::AskBossClosed | Step::GiveUp) => {
                let kind = if step == Step::AskBossClosed {
                    AskKind::BossClosed
                } else {
                    AskKind::Fruitless
                };
                if kind == AskKind::Fruitless {
                    let _ = plan_snooze::snooze(
                        &view.workspace_path,
                        &view.plan_id,
                        FRUITLESS_SNOOZE_MS,
                        "Fleet 连续唤醒 3 次都没有推进，自动静默并询问老板",
                        "fleet",
                    );
                }
                let card = build_card(view, kind);
                match crate::elicitation::write_request(&card) {
                    Ok(()) => {
                        st.ask_card_id = Some(card.id);
                        st.ask_kind = Some(kind);
                        st.orphan_since_ms = None;
                    }
                    Err(e) => crate::log_debug(&format!("plan revive: card for {}: {e}", view.plan_id)),
                }
            }
        }
    }
    write_state(path, &state, &views);
}

/// Persist, dropping state for plans that are no longer candidates (finished,
/// deleted, gone stale) unless a card of ours is still out for them.
fn write_state(path: &Path, state: &StateFile, views: &[PlanView]) {
    let live: HashSet<String> = views
        .iter()
        .map(|v| plan_snooze::plan_key(&v.workspace_path, &v.plan_id))
        .collect();
    let kept = StateFile {
        plans: state
            .plans
            .iter()
            .filter(|(k, s)| live.contains(*k) || s.ask_card_id.is_some())
            .map(|(k, s)| (k.clone(), s.clone()))
            .collect(),
    };
    match serde_json::to_vec_pretty(&kept) {
        Ok(bytes) => {
            if let Err(e) = crate::atomic_json::write_atomic(path, &bytes) {
                crate::log_debug(&format!("plan revive: write state: {e}"));
            }
        }
        Err(e) => crate::log_debug(&format!("plan revive: encode state: {e}")),
    }
}

// ── Dry run (for `fleet plan orphans`) ──────────────────────────────────────

/// One line of the dry-run report.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrphanReport {
    pub workspace_path: String,
    pub plan_id: String,
    pub done: u32,
    pub total: u32,
    pub newest_owner: String,
    /// `covered: <why>`, `snoozed: <reason>`, `closed by boss`, or `orphan`.
    pub verdict: String,
}

/// What the reviver would conclude right now for every candidate plan, without
/// touching any state.
pub fn dry_run() -> Vec<OrphanReport> {
    let now = plan_snooze::now_ms();
    let records = crate::task_progress::all_records();
    // One pass over the chain store: a per-claim lookup re-reads every chain
    // file, and there are thousands of claims.
    let successors = crate::handoff::successor_index();
    let views = collect_views(
        &records,
        now,
        &|sid| successors.get(sid).cloned(),
        &|sid| crate::task_outcome::read(sid).is_some(),
        &load_workspace_blocks,
    );
    let owners: HashSet<String> = views.iter().flat_map(|v| v.owners.iter().cloned()).collect();
    let coverage = gather_coverage(&owners);
    views
        .into_iter()
        .map(|v| {
            let verdict = if let Some(s) = plan_snooze::active(&v.workspace_path, &v.plan_id) {
                format!("snoozed: {}", snooze_summary(&s, now))
            } else if let Some(why) = coverage.reason(&v.owners) {
                format!("covered: {why}")
            } else if v.boss_closed {
                "orphan, closed by boss (would ask first)".to_string()
            } else {
                "orphan".to_string()
            };
            OrphanReport {
                workspace_path: v.workspace_path,
                plan_id: v.plan_id,
                done: v.done,
                total: v.total,
                newest_owner: v.newest_owner,
                verdict,
            }
        })
        .collect()
}

pub fn snooze_summary(s: &PlanSnooze, now: u64) -> String {
    match s.until_ms {
        Some(u) => format!("{} (for {}m more)", s.reason, u.saturating_sub(now) / 60_000),
        None => format!("{} (indefinitely)", s.reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: u64 = 3600 * 1000;

    fn rec(ws: &str, plan: &str, updated: u64) -> TaskProgressRecord {
        TaskProgressRecord {
            workspace_path: ws.into(),
            plan_id: plan.into(),
            current_task: None,
            updated,
        }
    }

    fn block(id: &str, parent: Option<&str>, body: &str) -> pt::SourcedBlock {
        pt::SourcedBlock {
            id: Some(id.into()),
            body: body.into(),
            source: PathBuf::from("/w/TASKS.md"),
            mtime: std::time::UNIX_EPOCH,
            parent: parent.map(str::to_string),
            kind: pt::PlanKind::default(),
        }
    }

    const PENDING: &str = "**Plan:** t\n\n- [x] **P1** — a\n- [ ] **P2** — b\n";
    const DONE: &str = "**Plan:** t\n\n- [x] **P1** — a\n";

    fn views(
        records: &[(String, TaskProgressRecord)],
        blocks: Vec<pt::SourcedBlock>,
        now: u64,
    ) -> Vec<PlanView> {
        collect_views(records, now, &|_| None, &|_| false, &|_| blocks.clone())
    }

    #[test]
    fn stale_and_unclaimed_plans_are_out_of_scope() {
        let now = 100 * 24 * H;
        let records = vec![
            ("s-old".to_string(), rec("/w", "old", now - 8 * 24 * H)),
            ("s-new".to_string(), rec("/w", "fresh", now - H)),
        ];
        let v = views(
            &records,
            vec![block("old", None, PENDING), block("fresh", None, PENDING), block("never", None, PENDING)],
            now,
        );
        let ids: Vec<_> = v.iter().map(|v| v.plan_id.as_str()).collect();
        assert_eq!(ids, vec!["fresh"]);
    }

    #[test]
    fn finish_continues_the_tree_in_dfs_order() {
        // root ── a (done) ── a1 (done)
        //      ├─ b (done) ── b1 (pending) ── b1x (pending)
        //      └─ c (pending)
        let tree = vec![
            block("root", None, PENDING),
            block("a", Some("root"), DONE),
            block("a1", Some("a"), DONE),
            block("b", Some("root"), DONE),
            block("b1", Some("b"), PENDING),
            block("b1x", Some("b1"), PENDING),
            block("c", Some("root"), PENDING),
        ];
        // Climbs past a finished parent, descends into the first pending
        // sibling subtree, deepest plan first.
        assert_eq!(next_plan_after(&tree, "a1").as_deref(), Some("b1x"));
        // A finished plan's own pending children come before anything above it.
        assert_eq!(next_plan_after(&tree, "b").as_deref(), Some("b1x"));
        // A plan with boxes still open does not continue anywhere.
        assert_eq!(next_plan_after(&tree, "c"), None);

        // Only the root's own tasks left: continue on the root.
        let rest = vec![block("root", None, PENDING), block("a", Some("root"), DONE)];
        assert_eq!(next_plan_after(&rest, "a").as_deref(), Some("root"));
        // Whole tree done, and an unrelated top-level plan is not "next".
        let done = vec![
            block("root", None, DONE),
            block("a", Some("root"), DONE),
            block("other", None, PENDING),
        ];
        assert_eq!(next_plan_after(&done, "a"), None);
        // Parent cycle terminates.
        let cyc = vec![block("x", Some("y"), DONE), block("y", Some("x"), DONE)];
        assert_eq!(next_plan_after(&cyc, "x"), None);
    }

    #[test]
    fn completed_plans_are_ignored() {
        let now = 10 * H;
        let records = vec![("s".to_string(), rec("/w", "p", now - H))];
        assert!(views(&records, vec![block("p", None, DONE)], now).is_empty());
    }

    #[test]
    fn parent_with_pending_child_defers_to_the_child_and_child_owners_count() {
        let now = 10 * H;
        let records = vec![
            ("s-parent".to_string(), rec("/w", "parent", now - 2 * H)),
            ("s-child".to_string(), rec("/w", "child", now - H)),
        ];
        let v = views(
            &records,
            vec![block("parent", None, PENDING), block("child", Some("parent"), PENDING)],
            now,
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].plan_id, "child");

        // Child done: the parent is the edge, and the child's claimant covers it.
        let v = views(
            &records,
            vec![block("parent", None, PENDING), block("child", Some("parent"), DONE)],
            now,
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].plan_id, "parent");
        assert_eq!(v[0].owners, vec!["s-child".to_string(), "s-parent".to_string()]);
        assert_eq!(v[0].newest_owner, "s-child");
    }

    #[test]
    fn unclaimed_pending_child_inherits_the_nearest_claimed_ancestor() {
        // Nobody ever claimed `c1`/`c2`, so they have no owners of their own,
        // and `parent` is skipped because it has a pending descendant. Without
        // the fallback the whole tree is invisible to the reviver.
        let now = 10 * H;
        let records = vec![("s-parent".to_string(), rec("/w", "parent", now - H))];
        let v = views(
            &records,
            vec![
                block("parent", None, PENDING),
                block("c1", Some("parent"), PENDING),
                block("c2", Some("parent"), PENDING),
            ],
            now,
        );
        // Only the first unclaimed child borrows the claim, so one ancestor
        // never fans out into several revived sessions at once.
        let ids: Vec<_> = v.iter().map(|v| v.plan_id.as_str()).collect();
        assert_eq!(ids, vec!["c1"]);
        assert_eq!(v[0].owners, vec!["s-parent".to_string()]);
        assert_eq!(v[0].newest_owner, "s-parent");

        // A child with a claim of its own keeps it; its unclaimed sibling
        // still falls back to the parent.
        let records = vec![
            ("s-parent".to_string(), rec("/w", "parent", now - 2 * H)),
            ("s-c1".to_string(), rec("/w", "c1", now - H)),
        ];
        let v = views(
            &records,
            vec![
                block("parent", None, PENDING),
                block("c1", Some("parent"), PENDING),
                block("c2", Some("parent"), PENDING),
            ],
            now,
        );
        let got: Vec<_> = v.iter().map(|v| (v.plan_id.as_str(), v.newest_owner.as_str())).collect();
        assert_eq!(got, vec![("c1", "s-c1"), ("c2", "s-parent")]);
    }

    #[test]
    fn handoff_successors_join_the_owners_and_become_newest() {
        let now = 10 * H;
        let records = vec![("s1".to_string(), rec("/w", "p", now - H))];
        let blocks = vec![block("p", None, PENDING)];
        let v = collect_views(
            &records,
            now,
            &|sid| (sid == "s1").then(|| "s2".to_string()),
            &|sid| sid == "s2",
            &|_| blocks.clone(),
        );
        assert_eq!(v[0].owners, vec!["s1".to_string(), "s2".to_string()]);
        assert_eq!(v[0].newest_owner, "s2");
        assert!(v[0].boss_closed);
    }

    #[test]
    fn coverage_names_the_first_reason_found() {
        let mut c = Coverage::default();
        let owners = vec!["aaaaaaaa-1".to_string(), "bbbbbbbb-2".to_string()];
        assert_eq!(c.reason(&owners), None);
        c.watching.insert("bbbbbbbb-2".into());
        assert_eq!(c.reason(&owners).unwrap(), "session bbbbbbbb owns a live watch");
        c.alive.insert("aaaaaaaa-1".into());
        assert_eq!(c.reason(&owners).unwrap(), "session aaaaaaaa is running");
    }

    fn attendance(
        records: &[(String, TaskProgressRecord)],
        blocks: &[pt::SourcedBlock],
        now: u64,
        cov: impl Fn(&mut Coverage),
        state: Option<PlanReviveState>,
        closed: &[&str],
    ) -> WorkspaceAttendance {
        resolve_attendance(
            records,
            blocks,
            now,
            &|_| None,
            &|sid| closed.contains(&sid),
            &|_| {
                let mut c = Coverage::default();
                cov(&mut c);
                c
            },
            &|_| state.clone(),
            &|_| false,
            true,
        )
    }

    #[test]
    fn attendance_names_the_covering_session_and_its_cover() {
        let now = 100 * H;
        let records = vec![
            ("s-parent".to_string(), rec("/w", "root", now - 2 * H)),
            ("s-child".to_string(), rec("/w", "child", now - H)),
        ];
        let blocks = [block("root", None, PENDING), block("child", Some("root"), PENDING)];
        let a = attendance(&records, &blocks, now, |c| {
            c.watching.insert("s-child".into());
        }, None, &[]);
        let child = &a.plans["child"];
        assert_eq!((child.session_id.as_str(), child.state), ("s-child", AttendanceState::Watching));
        // The parent's own claimant is gone, but the parent is not the live
        // edge, so the reviver has no outlook for it.
        assert_eq!(a.plans["root"].state, AttendanceState::Idle);
        assert!(a.outlook.is_empty());
    }

    #[test]
    fn a_relayed_claim_follows_the_successor_to_its_own_plan() {
        let now = 100 * H;
        let records = vec![
            ("s-old".to_string(), rec("/w", "first", now - 3 * H)),
            ("s-new".to_string(), rec("/w", "second", now - H)),
        ];
        let blocks = [block("first", None, DONE), block("second", None, PENDING)];
        let a = resolve_attendance(
            &records,
            &blocks,
            now,
            &|sid| (sid == "s-old").then(|| "s-new".to_string()),
            &|_| false,
            &|_| {
                let mut c = Coverage::default();
                c.alive.insert("s-new".into());
                c
            },
            &|_| None,
            &|_| false,
            true,
        );
        assert_eq!(a.plans["second"].state, AttendanceState::Running);
        assert!(!a.plans.contains_key("first"), "s-new moved on; it is not on `first`");
    }

    #[test]
    fn uncovered_live_edge_gets_a_revive_outlook() {
        let now = 100 * H;
        let records = vec![("s1".to_string(), rec("/w", "p", now - H))];
        let blocks = [block("p", None, PENDING)];
        let since = PlanReviveState { orphan_since_ms: Some(now - 60_000), ..Default::default() };
        let a = attendance(&records, &blocks, now, |_| {}, Some(since), &[]);
        assert_eq!(a.plans["p"].state, AttendanceState::Idle);
        assert_eq!(a.outlook["p"], ReviveOutlook::Revive { at: now - 60_000 + ORPHAN_GRACE_MS });

        let a = attendance(&records, &blocks, now, |_| {}, None, &["s1"]);
        assert_eq!(a.plans["p"].state, AttendanceState::BossClosed);
        assert_eq!(a.outlook["p"], ReviveOutlook::AskBoss);
    }

    #[test]
    fn old_claims_are_stale_and_get_no_outlook() {
        let now = 100 * 24 * H;
        let records = vec![("s1".to_string(), rec("/w", "p", now - 8 * 24 * H))];
        let a = attendance(&records, &[block("p", None, PENDING)], now, |_| {}, None, &[]);
        assert_eq!(a.plans["p"].state, AttendanceState::Stale);
        assert!(a.outlook.is_empty());
    }

    #[test]
    fn worktree_records_count_for_their_main_checkout() {
        assert!(in_workspace("/w/", "/w"));
        assert!(in_workspace("/w/.worktrees/x", "/w"));
        assert!(!in_workspace("/w2", "/w"));
    }

    fn view(done: u32, boss_closed: bool) -> PlanView {
        PlanView {
            workspace_path: "/w".into(),
            plan_id: "p".into(),
            title: None,
            done,
            total: 5,
            next_task: None,
            owners: vec!["s".into()],
            newest_owner: "s".into(),
            boss_closed,
        }
    }

    #[test]
    fn revives_only_after_the_grace_window() {
        let mut st = PlanReviveState::default();
        let v = view(1, false);
        assert_eq!(decide(&v, false, false, &mut st, 0, true), Step::Idle);
        assert_eq!(decide(&v, false, false, &mut st, ORPHAN_GRACE_MS - 1, true), Step::Idle);
        assert_eq!(decide(&v, false, false, &mut st, ORPHAN_GRACE_MS, true), Step::Revive);
    }

    #[test]
    fn coverage_or_snooze_resets_the_orphan_clock() {
        let mut st = PlanReviveState::default();
        let v = view(1, false);
        decide(&v, false, false, &mut st, 0, true);
        assert_eq!(decide(&v, true, false, &mut st, ORPHAN_GRACE_MS, true), Step::Idle);
        assert_eq!(st.orphan_since_ms, None);
        decide(&v, false, false, &mut st, ORPHAN_GRACE_MS + 1, true);
        assert_eq!(decide(&v, false, true, &mut st, 3 * ORPHAN_GRACE_MS, true), Step::Idle);
        assert_eq!(st.orphan_since_ms, None);
    }

    #[test]
    fn no_free_slot_waits_without_losing_the_clock() {
        let mut st = PlanReviveState::default();
        let v = view(1, false);
        decide(&v, false, false, &mut st, 0, true);
        assert_eq!(decide(&v, false, false, &mut st, ORPHAN_GRACE_MS, false), Step::Idle);
        assert_eq!(decide(&v, false, false, &mut st, ORPHAN_GRACE_MS + 1, true), Step::Revive);
    }

    #[test]
    fn boss_closed_plans_ask_instead_of_reviving_until_approved() {
        let mut st = PlanReviveState::default();
        let v = view(1, true);
        decide(&v, false, false, &mut st, 0, true);
        assert_eq!(decide(&v, false, false, &mut st, ORPHAN_GRACE_MS, true), Step::AskBossClosed);
        st.ask_card_id = Some("c".into());
        st.ask_kind = Some(AskKind::BossClosed);
        assert_eq!(decide(&v, false, false, &mut st, ORPHAN_GRACE_MS * 2, true), Step::Idle);
        let snooze = apply_answer(&mut st, false, Some(OPT_REVIVE), ORPHAN_GRACE_MS * 3);
        assert_eq!(snooze, None);
        assert_eq!(decide(&v, false, false, &mut st, ORPHAN_GRACE_MS * 3, true), Step::Revive);
    }

    #[test]
    fn fruitless_revivals_give_up_and_progress_resets_the_count() {
        let mut st = PlanReviveState { attempts: MAX_FRUITLESS, done_at_last_revive: 1, ..Default::default() };
        let v = view(1, false);
        decide(&v, false, false, &mut st, 0, true);
        assert_eq!(decide(&v, false, false, &mut st, ORPHAN_GRACE_MS, true), Step::GiveUp);

        // A ticked box since the last revival wipes the slate.
        let v2 = view(2, false);
        assert_eq!(decide(&v2, false, false, &mut st, ORPHAN_GRACE_MS, true), Step::Revive);
        assert_eq!(st.attempts, 0);
    }

    #[test]
    fn answers_map_to_snoozes_or_a_retry() {
        let mut st = PlanReviveState { ask_card_id: Some("c".into()), ask_kind: Some(AskKind::Fruitless), attempts: 3, ..Default::default() };
        assert_eq!(apply_answer(&mut st, false, Some(OPT_RETRY), 10 * ORPHAN_GRACE_MS), None);
        assert_eq!(st.attempts, MAX_FRUITLESS - 1);
        assert_eq!(st.ask_card_id, None);

        let mut st = PlanReviveState::default();
        assert_eq!(apply_answer(&mut st, false, Some(OPT_WEEK), 0), Some(Some(WEEK_MS)));
        assert_eq!(apply_answer(&mut st, false, Some(OPT_FOREVER), 0), Some(None));
        assert_eq!(apply_answer(&mut st, true, None, 0), Some(Some(WEEK_MS)));

        let mut st = PlanReviveState::default();
        assert_eq!(apply_answer(&mut st, false, Some("先把 P3 拆成两步"), 0), None);
        assert_eq!(st.boss_note.as_deref(), Some("先把 P3 拆成两步"));
        assert!(st.boss_approved);
    }

    #[test]
    fn revive_prompt_names_the_plan_and_the_snooze_exit() {
        let mut v = view(1, false);
        v.title = Some("标题".into());
        v.next_task = Some("**P2** — b".into());
        let p = revive_prompt(&v, Some("先做 P3"), Some("/t/s.jsonl"));
        assert!(p.contains("`p`（标题）"));
        assert!(p.contains("**P2** — b"));
        assert!(p.contains("snooze"));
        assert!(p.contains("/t/s.jsonl"));
        assert!(p.contains("先做 P3"));
    }
}
