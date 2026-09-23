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
            claims.sort_by(|a, b| b.1.updated.cmp(&a.1.updated));
            let Some(newest) = claims.first() else { continue };
            if now.saturating_sub(newest.1.updated) >= RECENCY_MS {
                continue;
            }
            let mut owners: Vec<String> = Vec::new();
            let mut seen: HashSet<String> = HashSet::new();
            let mut newest_owner = newest.0.clone();
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
        for sid in owners {
            let s = sid.as_str();
            let short = &s[..s.len().min(8)];
            if self.alive.contains(s) {
                return Some(format!("session {short} is running"));
            }
            if self.watching.contains(s) {
                return Some(format!("session {short} owns a live watch"));
            }
            if self.scheduled.contains(s) {
                return Some(format!("session {short} owns a pending schedule/loop"));
            }
            if self.carded.contains(s) {
                return Some(format!("session {short} has a decision card waiting"));
            }
            if self.handing_off.contains(s) {
                return Some(format!("session {short} registered a handoff"));
            }
        }
        None
    }
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
    for sid in owners {
        if !c.alive.contains(sid)
            && crate::codex_source::codex_fleet_owned_cwd(sid).is_some()
            && crate::codex_source::codex_session_pid(sid).is_some()
        {
            c.alive.insert(sid.clone());
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
    let projects = crate::session::real_home_dir()?.join(".claude").join("projects");
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
    let sid = uuid::Uuid::new_v4().to_string();
    let prompt = revive_prompt(view, boss_note, transcript_of(&view.newest_owner).as_deref());
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
    let title = format!("唤醒：{}", view.title.as_deref().unwrap_or(&view.plan_id));
    let _ = crate::session_title::set_title(&sid, &view.workspace_path, Some(title));
    Ok(sid)
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
    let views = collect_views(
        &records,
        now,
        &|sid| crate::handoff::successor_session_of(sid),
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
    let views = collect_views(
        &records,
        now,
        &|sid| crate::handoff::successor_session_of(sid),
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
