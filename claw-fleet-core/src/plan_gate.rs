//! Plan gate — refuse a turn that ends with the focused plan's tree unfinished.
//!
//! The `parent="..."` backtrack ([`crate::prd_tasks::resolve_backtrack_target`])
//! could only ever *advise*: on completing a child plan it re-pointed the focus
//! record and printed "Continue execution, don't end the turn". Nothing stopped the agent from
//! reading that line and stopping anyway, which is why a plan tree was never
//! observed to run to completion — the mechanism was a suggestion, not a gate.
//!
//! This module is the gate. It hangs off the same `Stop` hook exit-2 channel
//! [`crate::bg_guard`] uses: a non-empty return becomes stderr + exit 2, which
//! Claude Code reads as "don't stop" and feeds back to the agent as its next
//! instruction. Where `bg_guard` guards *background work* a headless turn would
//! kill, this guards *plan work* the agent is walking away from.
//!
//! Three escape hatches keep it from ever trapping a session — see
//! [`gate_reason_in`]. All of them are checked before any filesystem work.

use std::path::Path;

use crate::prd_tasks as pt;
use crate::task_progress::TaskProgressRecord;

/// Decide whether this `Stop` should be refused because the session's focused
/// plan tree still has work. `None` = let the turn end.
///
/// Escape hatches, in order:
///
/// 1. `stop_hook_active` — we already blocked once this turn and the agent has
///    been told. Blocking again fights its judgement (and Claude Code force-stops
///    after 8 rounds anyway). Same rule `bg_guard` uses.
/// 2. `has_registered_exit` — the agent took one of the legitimate exits the
///    refusal text and Rule 4 name: a registered relay (ending the turn is
///    exactly how a handoff fires), an armed watch (it resumes the session
///    when its condition or target sessions settle), a decision card still
///    waiting for the boss's answer, or a running background subagent/workflow
///    (its completion re-invokes the model, and no tool can block on it — see
///    [`crate::bg_guard::awaits_waited_task`]). Blocking any of these forces a
///    pointless second stop, and nudges the agent toward busy-work while it
///    waits. Observed 2026-10-23: a research session that fanned out three
///    background subagents was refused three times, with no exit it could take.
/// 3. A focus record not touched *this turn* — `focus.updated` before
///    `turn_start` means the session has run no `fleet plan` command since the
///    current user prompt arrived, so it isn't executing that plan right now.
///    Two cases this hatch protects: an ordinary conversational turn in a
///    workspace that merely *has* an old focus record, and the boss cutting in
///    mid-plan to redirect ("stop", "drop it, do X instead") — the new turn
///    touches no plan, so the gate stands down instead of dragging the agent
///    back to work the boss just called off.
///
/// Past the hatches: the gate fires when the focused plan still has a pending
/// P-task, or when it is complete but its tree still has a pending plan in
/// [`pt::next_plan_in_tree`] order (the backtrack case — now enforced rather
/// than suggested).
///
/// This is the main-conversation form: no sub-task assignment, and the
/// backtrack avoids nothing. [`gate_reason_scoped`] takes both.
pub fn gate_reason_in(
    cwd: &Path,
    focus: Option<&TaskProgressRecord>,
    turn_start: Option<u64>,
    stop_hook_active: bool,
    has_registered_exit: bool,
) -> Option<String> {
    gate_reason_scoped(
        cwd,
        focus,
        turn_start,
        stop_hook_active,
        has_registered_exit,
        &GateSession {
            assignment: &|| None,
            backtrack: &pt::BacktrackScope::unscoped,
        },
    )
}

/// What the gate needs to know about the session beyond its focus record.
/// Both are read lazily, once the hatches have let the stop through to the
/// plan: each touches the launch-note and claim stores, and the `Stop` hook
/// runs every turn.
pub struct GateSession<'a> {
    /// The spawner's plan and step this sub-task's line was dispatched to
    /// ([`crate::task_model::assignment`]).
    pub assignment: &'a dyn Fn() -> Option<(String, String)>,
    /// How a backtrack for this session is scoped ([`pt::BacktrackScope`]).
    pub backtrack: &'a dyn Fn() -> pt::BacktrackScope,
}

/// [`gate_reason_in`] for a specific session. On top of the escape hatches:
///
/// - A sub-task whose assigned step is ticked may stop, even though the
///   spawner's plan it sits in still has other steps open — those belong to
///   the spawner and its other sub-tasks.
/// - The backtrack is scoped by `session.backtrack`: a sub-task is never sent
///   out of its own root plan's subtree ([`pt::next_plan_for_subtask`]), and
///   nobody is sent onto a plan another live session is on. When that leaves
///   no target, the turn may end.
pub fn gate_reason_scoped(
    cwd: &Path,
    focus: Option<&TaskProgressRecord>,
    turn_start: Option<u64>,
    stop_hook_active: bool,
    has_registered_exit: bool,
    session: &GateSession<'_>,
) -> Option<String> {
    if stop_hook_active || has_registered_exit {
        return None;
    }
    let rec = focus?;
    // Stale focus: no plan action since this turn's prompt arrived.
    if let Some(started) = turn_start {
        if rec.updated < started {
            return None;
        }
    }

    let plan_id = rec.plan_id.as_str();
    let src = pt::find_plan_source(cwd, plan_id)?;
    let content = std::fs::read_to_string(&src).ok()?;
    let block = pt::plan_block(&content, plan_id)?;
    let body = block.body;

    if pt::has_open_work(&body, block.kind) {
        if let Some((plan, step)) = (session.assignment)() {
            let label = format!("**{step}**");
            if plan == plan_id
                && pt::parse_task_items(&body)
                    .iter()
                    .any(|it| it.done && it.text.contains(&label))
            {
                return None;
            }
        }
        // A research plan may legitimately end early; name that exit too, or
        // the gate pushes the agent to tick boxes that no longer mean anything.
        let research_exit = if block.kind == pt::PlanKind::Research {
            format!(
                "\n• 这是 research plan:答案已够(问题已答 / 最近一轮没有新结论 / 预算用完) → \
                 `fleet__plan action=stop plan_id={plan_id} reason=...` 记下停止理由即可收尾;\
                 发现推翻了原拆分 → `fleet__plan action=replan` 重拆未完成的 P"
            )
        } else {
            String::new()
        };
        let next = pt::first_pending_task(&body).unwrap_or_else(|| "第一个未完成的 P".to_string());
        let (done, total) = pt::count_tasks(&body);
        return Some(format!(
            "⛔ Fleet 计划门:你归属的 plan `{plan_id}` 还有未完成的 P-task \
             ({done}/{total} 已完成),本 turn 不能就这样结束。\n\n\
             下一个要做的是:{next}\n\n\
             按 Rule 4 的节奏继续:开发 → 测试/验证 → 在 worktree 内提交 → \
             `fleet plan check {plan_id} <P>` → 立刻进入下一个 P,不要停下来做进度汇报。\n\n\
             如果你确实该停下,用对应的正当出口,而不是直接结束 turn:\n\
             • 需要老板拍板 → 出决策卡问他(真正的工作方向问题,不是进度汇报)\n\
             • 上下文快满了 → `fleet handoff --note \"...\" --plan {plan_id} --next <P>` 登记接力\n\
             • 在等外部条件(CI/构建/部署) → `fleet watch create --until '...'`;\
             在等派出去的子任务 → `fleet watch create --sessions <id,...>`;\
             在等 Agent 工具派出的后台子代理 → 直接结束 turn,它交回时会自动唤醒你\n\
             • 这个 plan 真的被外部阻塞(等账号/等资源) → 说明阻塞点并请示老板改派{research_exit}"
        ));
    }

    // Focused plan is done — climb. This is the case the old advisory backtrack
    // could never enforce.
    let target = pt::resolve_backtrack_target_scoped(cwd, plan_id, &(session.backtrack)())?;
    let next = target.next_task.as_deref().unwrap_or("第一个未完成的 P");
    Some(format!(
        "⤴ Fleet 计划门:plan `{plan_id}` 已全部完成,但这棵计划树还没走完,本 turn 不能结束。\n\n\
         按计划树顺序(子计划先于父计划,兄弟按文件顺序),下一个是 plan `{next_plan}`,\
         要做的是:{next}\n\n\
         先 `fleet plan resume {next_plan}` 认领它,然后直接继续执行,\
         不要因为一个 plan 完成就收工,也不要停下来汇报「子计划完成了」。",
        next_plan = target.plan_id,
    ))
}

/// [`gate_reason_in`] wired to the real side-channels, for the `Stop` hook.
///
/// **Call order matters**: this must run before `idle::mark_idle` (which would
/// overwrite the previous-yield timestamp the staleness hatch compares against)
/// and before `handoff::consume_and_spawn` (which would take the pending relay
/// the second hatch looks for). Both hold in `fleet session idle`, where this
/// sits next to the [`crate::bg_guard`] check.
///
/// `awaiting_background` is [`crate::bg_guard::awaits_waited_task`] over the
/// Stop payload.
pub fn gate_reason(
    cwd: &Path,
    session_id: &str,
    stop_hook_active: bool,
    awaiting_background: bool,
) -> Option<String> {
    if !crate::resume_triggers::ResumeTriggersConfig::load().plan_gate {
        return None;
    }
    let focus = crate::task_progress::read(session_id);
    let turn_start = crate::idle::read_turn_start(session_id);
    gate_reason_scoped(
        cwd,
        focus.as_ref(),
        turn_start,
        stop_hook_active,
        awaiting_background || has_registered_exit(session_id),
        &GateSession {
            // Read off the relay chain's first hop: a handoff successor's own
            // launch note carries no assignment.
            assignment: &|| crate::task_model::assignment(session_id),
            backtrack: &|| pt::BacktrackScope::for_session(cwd, Some(session_id)),
        },
    )
}

/// Whether `session_id` has a legitimate reason to end the turn with plan work
/// left: a registered handoff, a live watch it owns, or an unanswered decision
/// card. Cheapest check first — each is a small directory scan at most, and
/// the `Stop` hook runs every turn. Scheduled `fleet schedule` / `fleet loop`
/// runs are deliberately not counted: they spawn fresh sessions rather than
/// resuming this one, and the guidance does not name them as an exit.
fn has_registered_exit(session_id: &str) -> bool {
    if crate::handoff::read_pending(session_id).is_some() {
        return true;
    }
    crate::watch::session_has_live_watch(session_id)
        || crate::parked::has_parked_for_session(session_id)
        || crate::codex_source::session_has_pending_decision(session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// A workspace whose TASKS.md holds `body`, plus a focus record factory.
    fn ws(body: &str) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("TASKS.md");
        fs::write(&path, body).unwrap();
        let root = tmp.path().to_path_buf();
        (tmp, root)
    }

    fn focus(plan_id: &str, updated: u64) -> TaskProgressRecord {
        TaskProgressRecord {
            workspace_path: "/irrelevant".to_string(),
            plan_id: plan_id.to_string(),
            current_task: None,
            updated,
            backtracked_from: None,
        }
    }

    const ONE_PENDING: &str = "\
<!-- fleet:prd:begin id=\"mine\" v=\"2\" -->

**Plan:** My work

- [x] **P1** — done
- [ ] **P2** — still open

<!-- fleet:prd:end id=\"mine\" -->
";

    /// The core property: focused plan has pending work, focus was touched this
    /// turn, no relay registered ⇒ the turn is refused.
    #[test]
    fn blocks_when_focused_plan_has_pending_work() {
        let (_t, root) = ws(ONE_PENDING);
        let reason = gate_reason_in(&root, Some(&focus("mine", 200)), Some(100), false, false)
            .expect("pending plan + fresh focus ⇒ gate fires");
        assert!(reason.contains("mine"), "names the plan: {reason}");
        assert!(
            reason.contains("**P2** — still open"),
            "names the next P: {reason}"
        );
        assert!(reason.contains("1/2"), "shows progress: {reason}");
        // Must offer the legitimate exits, or the gate is a trap.
        assert!(
            reason.contains("fleet handoff"),
            "offers the relay exit: {reason}"
        );
        assert!(
            reason.contains("fleet watch"),
            "offers the wait exit: {reason}"
        );
    }

    /// Hatch 1: already blocked once this turn.
    #[test]
    fn releases_when_stop_hook_already_active() {
        let (_t, root) = ws(ONE_PENDING);
        assert_eq!(
            gate_reason_in(&root, Some(&focus("mine", 200)), Some(100), true, false),
            None
        );
    }

    /// Hatch 2: a registered handoff — ending the turn is how it fires.
    /// (The watch and pending-card exits feed the same flag; see
    /// `watch::tests::live_watch_*` for the watch read.)
    #[test]
    fn releases_when_a_handoff_is_registered() {
        let (_t, root) = ws(ONE_PENDING);
        assert_eq!(
            gate_reason_in(&root, Some(&focus("mine", 200)), Some(100), false, true),
            None
        );
    }

    /// Hatch 3: the focus record predates this turn's prompt, so the session has
    /// run no plan command this turn and isn't executing that plan now. Guards
    /// ordinary conversational turns in a workspace that merely *has* a focus
    /// record.
    #[test]
    fn releases_when_focus_predates_this_turn() {
        let (_t, root) = ws(ONE_PENDING);
        assert_eq!(
            gate_reason_in(&root, Some(&focus("mine", 100)), Some(200), false, false),
            None,
            "focus older than turn_start ⇒ stale"
        );
    }

    /// The interrupt case, and the reason the hatch keys on turn *start* rather
    /// than the previous turn's end: the boss cuts in mid-plan to call the work
    /// off. The new turn touches no plan, so the gate must not drag the agent
    /// back to a plan the boss just stopped.
    #[test]
    fn releases_when_the_boss_interrupts_without_any_plan_action() {
        let (_t, root) = ws(ONE_PENDING);
        // Focus was recorded at t=150 (previous turn); the boss's new prompt
        // starts this turn at t=300 and the agent runs no `fleet plan` command.
        assert_eq!(
            gate_reason_in(&root, Some(&focus("mine", 150)), Some(300), false, false),
            None,
            "no plan action this turn ⇒ the boss's redirect wins"
        );
        // Whereas if the agent *did* advance the plan this turn (t=350 > 300),
        // stopping early is exactly what the gate is for.
        assert!(
            gate_reason_in(&root, Some(&focus("mine", 350)), Some(300), false, false).is_some(),
            "plan advanced this turn ⇒ gate fires"
        );
    }

    /// No focus at all ⇒ nothing to gate on.
    #[test]
    fn releases_without_a_focus_record() {
        let (_t, root) = ws(ONE_PENDING);
        assert_eq!(gate_reason_in(&root, None, Some(100), false, false), None);
    }

    /// A session with no turn-start stamp (predates the sentinel, or a spawn
    /// whose opening prompt bypassed the hook) must still be gated rather than
    /// treating "unknown" as stale.
    #[test]
    fn blocks_on_the_first_turn_when_there_is_no_idle_record() {
        let (_t, root) = ws(ONE_PENDING);
        assert!(
            gate_reason_in(&root, Some(&focus("mine", 200)), None, false, false).is_some(),
            "no idle_since ⇒ cannot be stale"
        );
    }

    /// The backtrack case, now enforced: the focused child is done but its
    /// parent still has work, so the tree isn't finished.
    #[test]
    fn blocks_with_backtrack_when_child_done_but_parent_pending() {
        let body = "\
<!-- fleet:prd:begin id=\"par\" v=\"2\" -->

**Plan:** Parent

- [ ] **P4** — parent tail

<!-- fleet:prd:end id=\"par\" -->

<!-- fleet:prd:begin id=\"kid\" v=\"2\" parent=\"par\" -->

**Plan:** Child

- [x] **P1** — child done

<!-- fleet:prd:end id=\"kid\" -->
";
        let (_t, root) = ws(body);
        let reason = gate_reason_in(&root, Some(&focus("kid", 200)), Some(100), false, false)
            .expect("completed child + pending parent ⇒ gate fires");
        assert!(reason.contains("kid"), "names the child: {reason}");
        assert!(reason.contains("par"), "names the parent: {reason}");
        assert!(
            reason.contains("**P4** — parent tail"),
            "names the next P: {reason}"
        );
    }

    /// Whole tree complete ⇒ the turn may end.
    #[test]
    fn releases_when_the_whole_tree_is_complete() {
        let body = "\
<!-- fleet:prd:begin id=\"par\" v=\"2\" -->

**Plan:** Parent

- [x] **P1** — parent done

<!-- fleet:prd:end id=\"par\" -->

<!-- fleet:prd:begin id=\"kid\" v=\"2\" parent=\"par\" -->

**Plan:** Child

- [x] **P1** — child done

<!-- fleet:prd:end id=\"kid\" -->
";
        let (_t, root) = ws(body);
        assert_eq!(
            gate_reason_in(&root, Some(&focus("kid", 200)), Some(100), false, false),
            None
        );
    }

    const RESEARCH_PENDING: &str = "\
<!-- fleet:prd:begin id=\"mine\" v=\"2\" kind=\"research\" -->

**Plan:** Study

- [x] **P1** — survey
- [ ] **P2** — dig deeper

<!-- fleet:prd:end id=\"mine\" -->
";

    /// Research plan without a stop reason: the gate fires and names the stop
    /// and replan exits alongside the usual ones.
    #[test]
    fn research_plan_blocks_until_stopped_and_names_the_stop_exit() {
        let (_t, root) = ws(RESEARCH_PENDING);
        let reason = gate_reason_in(&root, Some(&focus("mine", 200)), Some(100), false, false)
            .expect("pending research plan ⇒ gate fires");
        assert!(reason.contains("action=stop plan_id=mine"), "{reason}");
        assert!(reason.contains("replan"), "{reason}");
    }

    /// Research plan with a recorded stop reason counts as complete.
    #[test]
    fn stopped_research_plan_releases_the_gate() {
        let stopped = pt::record_stop(RESEARCH_PENDING, "mine", "饱和:最近一轮没有新结论").unwrap();
        let (_t, root) = ws(&stopped);
        assert_eq!(
            gate_reason_in(&root, Some(&focus("mine", 200)), Some(100), false, false),
            None
        );
    }

    /// A stopped research child still sends the agent up to its pending parent.
    #[test]
    fn stopped_research_child_still_backtracks_to_parent() {
        let body = "\
<!-- fleet:prd:begin id=\"par\" v=\"2\" -->

**Plan:** Parent

- [ ] **P4** — parent tail

<!-- fleet:prd:end id=\"par\" -->

<!-- fleet:prd:begin id=\"kid\" v=\"2\" parent=\"par\" kind=\"research\" -->

**Plan:** Child

- [ ] **P1** — open question

> 停止：问题已答

<!-- fleet:prd:end id=\"kid\" -->
";
        let (_t, root) = ws(body);
        let reason = gate_reason_in(&root, Some(&focus("kid", 200)), Some(100), false, false)
            .expect("stopped child + pending parent ⇒ climb");
        assert!(reason.contains("**P4** — parent tail"), "{reason}");
    }

    /// Exec behaviour is unchanged: a stop marker in an exec plan is inert and
    /// the refusal text does not offer the research-only exit.
    #[test]
    fn exec_plan_ignores_a_stop_marker() {
        let body = ONE_PENDING.replace(
            "- [ ] **P2** — still open\n",
            "- [ ] **P2** — still open\n\n> 停止：nope\n",
        );
        let (_t, root) = ws(&body);
        let reason = gate_reason_in(&root, Some(&focus("mine", 200)), Some(100), false, false)
            .expect("exec plan with open box ⇒ gate fires");
        assert!(!reason.contains("action=stop"), "{reason}");
    }

    /// A parent plan the main conversation split into two child plans, one
    /// per sub-task: `a` is finished, `b` is still open.
    const SPLIT: &str = "\
<!-- fleet:prd:begin id=\"par\" v=\"2\" -->

**Plan:** Parent

- [x] **P1** — split the work
- [ ] **P4** — integrate

<!-- fleet:prd:end id=\"par\" -->

<!-- fleet:prd:begin id=\"a\" v=\"2\" parent=\"par\" -->

**Plan:** Child A

- [x] **P1** — a's work

<!-- fleet:prd:end id=\"a\" -->

<!-- fleet:prd:begin id=\"b\" v=\"2\" parent=\"par\" -->

**Plan:** Child B

- [ ] **P1** — b's work

<!-- fleet:prd:end id=\"b\" -->
";

    /// [`gate_reason_scoped`] with a fresh focus on `plan`, a sub-task flag, the
    /// plans other live sessions are on, and an optional step assignment.
    fn scoped(
        root: &Path,
        plan: &str,
        subtask: bool,
        claimed: &'static [&'static str],
        assignment: Option<(&str, &str)>,
    ) -> Option<String> {
        scoped_created(root, plan, subtask, claimed, assignment, &[])
    }

    /// [`scoped`] plus the plans the sub-task's line created.
    fn scoped_created(
        root: &Path,
        plan: &str,
        subtask: bool,
        claimed: &'static [&'static str],
        assignment: Option<(&str, &str)>,
        created: &[&str],
    ) -> Option<String> {
        let assignment = assignment.map(|(p, s)| (p.to_string(), s.to_string()));
        let created: std::collections::HashSet<String> =
            created.iter().map(|s| s.to_string()).collect();
        gate_reason_scoped(
            root,
            Some(&focus(plan, 200)),
            Some(100),
            false,
            false,
            &GateSession {
                assignment: &|| assignment.clone(),
                backtrack: &|| pt::BacktrackScope {
                    subtask: subtask.then(|| pt::SubtaskScope {
                        assignment: assignment.clone(),
                        created: created.clone(),
                    }),
                    claimed_elsewhere: Box::new(|id| claimed.contains(&id)),
                    held_step: Box::new(|_, _| false),
                },
            },
        )
    }

    /// A sub-task that split its own work into a plan tree (`own` with
    /// children `c1`, `c2`) and finished `c1` walks on to `c2`, like a main
    /// conversation would, instead of being let go halfway.
    #[test]
    fn a_subtask_walks_the_tree_it_created() {
        const OWN_TREE: &str = "\
<!-- fleet:prd:begin id=\"own\" v=\"2\" -->

**Plan:** sub-task's own parent

- [ ] **P9** — wrap up

<!-- fleet:prd:end id=\"own\" -->

<!-- fleet:prd:begin id=\"c1\" v=\"2\" parent=\"own\" -->

**Plan:** child 1

- [x] **P1** — c1 work

<!-- fleet:prd:end id=\"c1\" -->

<!-- fleet:prd:begin id=\"c2\" v=\"2\" parent=\"own\" -->

**Plan:** child 2

- [ ] **P1** — c2 work

<!-- fleet:prd:end id=\"c2\" -->
";
        let (_t, root) = ws(OWN_TREE);
        let own = ["own", "c1", "c2"];
        let reason = scoped_created(&root, "c1", true, &[], None, &own).expect("c2 still open");
        assert!(reason.contains("plan `c2`"), "{reason}");
        // `c2` done: the root's own box is next.
        let rest = OWN_TREE.replace("- [ ] **P1** — c2 work", "- [x] **P1** — c2 work");
        let (_t, root) = ws(&rest);
        let reason = scoped_created(&root, "c2", true, &[], None, &own).expect("own P9 open");
        assert!(reason.contains("**P9** — wrap up"), "{reason}");
        // A child another live session is on is skipped as for anyone else.
        let (_t, root) = ws(OWN_TREE);
        assert_eq!(scoped_created(&root, "c1", true, &["c2"], None, &own), None);
    }

    /// A sub-task put on a plan the spawner wrote (`a` under `par`, none of
    /// them created by it) is not handed that plan's sibling or parent, even
    /// when it created a child plan of its own elsewhere.
    #[test]
    fn a_subtask_never_climbs_into_plans_it_did_not_create() {
        let (_t, root) = ws(SPLIT);
        assert_eq!(scoped_created(&root, "a", true, &[], None, &["elsewhere"]), None);
        // Even `a` itself being its own does not open up the spawner's `par`.
        assert_eq!(scoped_created(&root, "a", true, &[], None, &["a"]), None);
    }

    /// The spawner's plan with a child plan the sub-task wrote for its step.
    const STEP_CHILD: &str = "\
<!-- fleet:prd:begin id=\"par\" v=\"2\" -->

**Plan:** spawner's plan

- [x] **P1** — one
- [ ] **P2** — sub-task's step
- [ ] **P3** — spawner's own

<!-- fleet:prd:end id=\"par\" -->

<!-- fleet:prd:begin id=\"s\" v=\"2\" parent=\"par\" -->

**Plan:** sub-task's breakdown of P2

- [x] **P1** — done

<!-- fleet:prd:end id=\"s\" -->

<!-- fleet:prd:begin id=\"other\" v=\"2\" parent=\"par\" -->

**Plan:** the spawner's other child

- [ ] **P1** — not the sub-task's

<!-- fleet:prd:end id=\"other\" -->
";

    /// A sub-task dispatched to `par` P2 that finished its own child plan `s`
    /// is pointed back at P2 — not at `par`'s other child or P3 — and let go
    /// once P2 is ticked.
    #[test]
    fn a_step_subtask_returns_to_its_step_and_no_further() {
        let (_t, root) = ws(STEP_CHILD);
        let reason = scoped_created(&root, "s", true, &[], Some(("par", "P2")), &["s"])
            .expect("assigned step still open");
        assert!(reason.contains("plan `par`"), "{reason}");
        assert!(reason.contains("**P2** — sub-task's step"), "{reason}");
        assert!(!reason.contains("not the sub-task's"), "{reason}");
        let ticked = STEP_CHILD.replace("- [ ] **P2**", "- [x] **P2**");
        let (_t, root) = ws(&ticked);
        assert_eq!(scoped_created(&root, "s", true, &[], Some(("par", "P2")), &["s"]), None);
        // Focused on `par` itself with P2 ticked: released by the step rule.
        assert_eq!(scoped_created(&root, "par", true, &[], Some(("par", "P2")), &["s"]), None);
    }

    /// The spawner's plan with two child plans a sub-task hung under it.
    const SIBLINGS_UNDER_SPAWNER: &str = "\
<!-- fleet:prd:begin id=\"par\" v=\"2\" -->

**Plan:** spawner's plan

- [x] **P1** — one
- [ ] **P2** — sub step
- [ ] **P3** — spawner's own

<!-- fleet:prd:end id=\"par\" -->

<!-- fleet:prd:begin id=\"s1\" v=\"2\" parent=\"par\" -->

**Plan:** sub-task part 1

- [x] **P1** — s1 work

<!-- fleet:prd:end id=\"s1\" -->

<!-- fleet:prd:begin id=\"other\" v=\"2\" parent=\"par\" -->

**Plan:** the spawner's other child

- [ ] **P1** — not the sub-task's

<!-- fleet:prd:end id=\"other\" -->

<!-- fleet:prd:begin id=\"s2\" v=\"2\" parent=\"par\" -->

**Plan:** sub-task part 2

- [ ] **P1** — s2 work

<!-- fleet:prd:end id=\"s2\" -->
";

    /// A sub-task dispatched to `par` P2 that split the step into `s1` and
    /// `s2` under `par` goes from `s1` to `s2`, then back to P2 — even while
    /// the spawner is live on `par`. Before, each child was its own root and
    /// finishing `s1` pointed straight at P2, so ticking P2 let it go with
    /// `s2` undone.
    #[test]
    fn a_step_subtask_walks_all_the_children_it_hung_under_the_spawners_plan() {
        let created = ["s1", "s2"];
        let step = Some(("par", "P2"));
        let (_t, root) = ws(SIBLINGS_UNDER_SPAWNER);
        let reason = scoped_created(&root, "s1", true, &["par"], step, &created).expect("s2 open");
        assert!(reason.contains("plan `s2`"), "{reason}");
        assert!(!reason.contains("not the sub-task's"), "{reason}");
        let rest = SIBLINGS_UNDER_SPAWNER.replace("- [ ] **P1** — s2 work", "- [x] **P1** — s2 work");
        let (_t, root) = ws(&rest);
        let reason = scoped_created(&root, "s2", true, &["par"], step, &created).expect("P2 open");
        assert!(reason.contains("plan `par`"), "{reason}");
        assert!(reason.contains("**P2** — sub step"), "{reason}");
        assert!(!reason.contains("spawner's own"), "{reason}");
    }

    /// A sub-task with no step that resumed the spawner's plan (`par` here)
    /// and created `s1` and `s2` under it goes from `s1` to `s2`, and is let
    /// go after `s2` without being handed `par`'s own steps or its other
    /// child.
    #[test]
    fn a_subtask_without_a_step_walks_its_children_under_a_resumed_plan() {
        let created = ["s1", "s2"];
        let (_t, root) = ws(SIBLINGS_UNDER_SPAWNER);
        let reason = scoped_created(&root, "s1", true, &[], None, &created).expect("s2 open");
        assert!(reason.contains("plan `s2`"), "{reason}");
        let rest = SIBLINGS_UNDER_SPAWNER.replace("- [ ] **P1** — s2 work", "- [x] **P1** — s2 work");
        let (_t, root) = ws(&rest);
        assert_eq!(scoped_created(&root, "s2", true, &[], None, &created), None);
        // A sibling another live session took over is skipped, as anywhere.
        let (_t, root) = ws(SIBLINGS_UNDER_SPAWNER);
        assert_eq!(scoped_created(&root, "s1", true, &["s2"], None, &created), None);
    }

    /// The handoff successor of a step sub-task inherits the assignment (it is
    /// read off the relay line's first hop, [`crate::task_model::assignment`]),
    /// so it too is let go once the step is ticked.
    #[test]
    fn a_step_subtask_successor_is_released_once_the_step_is_ticked() {
        let (_t, root) = ws(&STEP_CHILD.replace("- [ ] **P2**", "- [x] **P2**"));
        let notes = |sid: &str| (sid == "S").then(|| ("par".to_string(), "P2".to_string()));
        let line_root = |_: &str| Some("S".to_string());
        let inherited = crate::task_model::assignment_with("S2", &line_root, &notes);
        assert_eq!(inherited, Some(("par".to_string(), "P2".to_string())));
        let inherited = inherited.as_ref().map(|(p, s)| (p.as_str(), s.as_str()));
        assert_eq!(scoped(&root, "par", true, &[], inherited), None);
        // Without the line lookup the successor had no assignment and was held.
        assert!(scoped(&root, "par", true, &[], None).is_some());
    }

    /// The reported bug: the session that finished `a` was sent to sibling
    /// `b`, which another live session is working, and could not end its
    /// turn. Skipping `b` leaves nothing (the parent waits for `b`), so the
    /// turn ends.
    #[test]
    fn releases_when_the_sibling_is_claimed_by_another_live_session() {
        let (_t, root) = ws(SPLIT);
        assert_eq!(scoped(&root, "a", false, &["b"], None), None);
    }

    /// A sub-task that finished the plan it was put on is released, even with
    /// an unclaimed sibling still open: the main conversation that spawned it
    /// decides what comes next.
    #[test]
    fn releases_a_subtask_that_finished_its_plan() {
        let (_t, root) = ws(SPLIT);
        assert_eq!(scoped(&root, "a", true, &[], None), None);
    }

    /// A sub-task dispatched to one step of the spawner's plan may stop once
    /// that step is ticked, though the plan's other steps are still open; an
    /// unticked assigned step still holds it.
    #[test]
    fn releases_a_subtask_once_its_assigned_step_is_ticked() {
        let (_t, root) = ws(ONE_PENDING);
        assert_eq!(scoped(&root, "mine", true, &[], Some(("mine", "P1"))), None);
        assert!(
            scoped(&root, "mine", true, &[], Some(("mine", "P2"))).is_some(),
            "assigned step still open ⇒ gate fires"
        );
        assert!(
            scoped(&root, "mine", true, &[], Some(("other", "P1"))).is_some(),
            "assignment in another plan says nothing about this one"
        );
    }

    /// Regression: a main conversation walking its own tree alone still
    /// backtracks in tree order — the unclaimed sibling comes next.
    #[test]
    fn main_conversation_still_backtracks_to_an_unclaimed_sibling() {
        let (_t, root) = ws(SPLIT);
        let reason = scoped(&root, "a", false, &[], None).expect("unclaimed sibling ⇒ climb");
        assert!(reason.contains("plan `b`"), "{reason}");
        assert!(reason.contains("**P1** — b's work"), "{reason}");
        // The unscoped entry point agrees.
        let plain = gate_reason_in(&root, Some(&focus("a", 200)), Some(100), false, false)
            .expect("unscoped ⇒ climb");
        assert!(plain.contains("plan `b`"), "{plain}");
    }

    /// A focus record naming a plan that exists in no TASKS.md here (another
    /// workspace's plan) must not block — and must not panic.
    #[test]
    fn releases_when_the_focused_plan_is_not_in_this_workspace() {
        let (_t, root) = ws(ONE_PENDING);
        assert_eq!(
            gate_reason_in(
                &root,
                Some(&focus("elsewhere", 200)),
                Some(100),
                false,
                false
            ),
            None
        );
    }
}
