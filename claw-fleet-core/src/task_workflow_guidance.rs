//! Per-task-type workflow blocks — how a `dev` or `research` task is run.
//!
//! The task's type lives in its profile ([`crate::task_profile`]), settled when
//! the boss approves a kickoff card ([`crate::task_kickoff`]). That is too late
//! for the launch-time system
//! prompt, so the block rides the per-turn lead section of
//! [`crate::prd_tasks::render_active_plans_reminder`], which reaches all three
//! harnesses (the Claude hook, the codex prompt-prepend, the dsh plugin).
//!
//! The text has to stay byte-stable for a given (type, role, voice):
//! [`crate::prd_context_dedup`] skips re-injecting an unchanged reminder, so
//! anything that varies per turn here would re-send the whole block every turn.
//!
//! `general` gets no block — the behaviour before task types existed.
//!
//! The research lead block is the whole research layer (design
//! `design/task-type-harness` v2 §3 option B, §4): a main conversation's
//! system prompt is frozen at creation, so once a task is kicked off as
//! research, these rules reach it only through the per-turn injection. Two
//! companions cover what that cannot: [`research_next_line`], a one-line
//! nudge that bypasses the dedup, and the sub-task block, which a research
//! task's sub-task also gets in its launch-time system prompt
//! ([`crate::claude_launch`]).
//!
//! The dev block deliberately does not restate the worktree / commit / cadence
//! rules: those already stand in the PRD discipline guidance, and the block
//! only points at them and adds what that guidance lacks.

use crate::task_profile::TaskType;

/// Which side of a task the reader is on. The coordinator plans and reports;
/// a sub-task raises no card and hands its result back in its final reply, so
/// it gets the delivery half of the workflow only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Lead,
    Subtask,
}

/// The workflow block for a task of `task_type`, worded for `role`. `title` is
/// what the agent calls the user; `locale` other than `zh` gets English.
/// `None` for `general`.
pub fn render_block(task_type: TaskType, role: Role, title: &str, locale: &str) -> Option<String> {
    let zh = locale == "zh";
    let title = match (title.trim().is_empty(), zh) {
        (true, true) => "老板",
        (true, false) => "Boss",
        (false, _) => title.trim(),
    };
    let text = match (task_type, role, zh) {
        (TaskType::General, _, _) => return None,
        (TaskType::Research, Role::Lead, true) => research_lead_zh(title),
        (TaskType::Research, Role::Lead, false) => research_lead_en(title),
        (TaskType::Research, Role::Subtask, true) => RESEARCH_SUBTASK_ZH.to_string(),
        (TaskType::Research, Role::Subtask, false) => RESEARCH_SUBTASK_EN.to_string(),
        (TaskType::Dev, Role::Lead, true) => dev_lead_zh(title),
        (TaskType::Dev, Role::Lead, false) => dev_lead_en(title),
        (TaskType::Dev, Role::Subtask, true) => DEV_SUBTASK_ZH.to_string(),
        (TaskType::Dev, Role::Subtask, false) => DEV_SUBTASK_EN.to_string(),
    };
    Some(text)
}

/// The type's display name inside the goal line ("（类型：研究）").
pub fn type_label(task_type: TaskType, locale: &str) -> &'static str {
    match (task_type, locale == "zh") {
        (TaskType::Dev, true) => "开发",
        (TaskType::Research, true) => "研究",
        (TaskType::General, true) => "未定",
        (TaskType::Dev, false) => "dev",
        (TaskType::Research, false) => "research",
        (TaskType::General, false) => "undecided",
    }
}

/// The one-sentence goal-drift test injected every turn for a task's main
/// conversation, next to the goal it is judged against. The full rule and its
/// examples live in the standing guidance ([`render_drift_section`]).
pub fn drift_line(title: &str, locale: &str) -> String {
    if locale == "zh" {
        let title = if title.trim().is_empty() { "老板" } else { title.trim() };
        return format!(
            "目标漂移：{title}的新消息若是本目标的延续或下一阶段（由前面结论直接引出），留在本任务，\
             必要时开不同 kind 的子计划；若是与本目标无关的新目标，先别动手，发卡建议「开为新任务」\
             （选项含「开为新任务 / 并入本任务」，并问原任务结束 / 挂起 / 继续并行），{title}同意后用 \
             `fleet__task action=split` 带完整简报开出去。"
        );
    }
    let title = if title.trim().is_empty() { "Boss" } else { title.trim() };
    format!(
        "Goal drift: if {title}'s new message continues this goal or is its next stage (it follows \
         from what was concluded), stay in this task, opening a child plan of another kind if needed; \
         if it is a new goal unrelated to this one, do not start on it — raise a card proposing \
         \"open as a new task\" (options include open as new task / fold into this task, and ask \
         whether this task ends, is parked or runs on in parallel), and once {title} agrees open it \
         with `fleet__task action=split` carrying a complete brief."
    )
}

/// The standing goal-drift rule for a task's main conversation, with the
/// design's boundary examples. Part of the coordinator's section in
/// [`crate::task_roles_guidance::render_section`]; the per-turn
/// [`drift_line`] is its one-sentence reminder.
pub fn render_drift_section(title: &str, locale: &str) -> String {
    if locale == "zh" {
        return format!(
            "**目标变了才开新任务，类型变了只算换阶段。**每轮注入里的「本任务目标」是判据：\n\
- {title}的新消息是原目标的**延续或下一阶段**（由前面的结论直接引出）→ 留在本任务，必要时开一个不同 kind 的子计划（研究 → 设计 → 开发是同一个目标的三个阶段）。\n\
- 是**与原目标无关的新目标** → 先不动手，发卡建议「开为新任务」：选项含「开为新任务 / 并入本任务（改目标要写理由）」，并问原任务「结束 / 挂起 / 继续并行」。{title}同意后用 `fleet__task action=split` 带完整简报开出去；绝不自己拆。\n\
- 边界例子：调研任务类型的 harness →「基于这个做产品架构」= 延续；修桌面端 CPU 占用 →「顺便看下手机端为什么登录不上」= 新目标；实现某功能 →「先查一下竞品怎么做的」= 延续，开 research 子计划。\n"
        );
    }
    format!(
        "**A new goal opens a new task; a new type is only a new stage.** The per-turn \"task goal\" line is the test:\n\
- {title}'s new message **continues the goal or is its next stage** (it follows from what was concluded) → stay in this task, opening a child plan of another kind if needed (research → design → dev are three stages of one goal).\n\
- It is **a new goal unrelated to this one** → do not start on it; raise a card proposing \"open as a new task\", with options open as new task / fold into this task (changing the goal needs a reason), and ask whether this task ends, is parked or runs on in parallel. Once {title} agrees, open it with `fleet__task action=split` carrying a complete brief; never split on your own.\n\
- Boundary examples: researching harnesses per task type → \"design the product architecture from this\" = continuation; fixing desktop CPU use → \"also check why the phone cannot log in\" = new goal; implementing a feature → \"first look at how competitors do it\" = continuation, open a research child plan.\n"
    )
}

/// Experiment switch for the task-type ablation: workspaces listed in
/// `~/.fleet/experiments/no-flow-block.txt` (one path per line, `#` comments)
/// get no workflow block, so the block is the only difference between the
/// two arms. A listed path also
/// covers everything below it (its `.worktrees/*`). No file = nobody affected.
pub const FLOW_BLOCK_OFF_FILE: &str = "experiments/no-flow-block.txt";

pub fn flow_block_off(cwd: &std::path::Path) -> bool {
    crate::session::get_fleet_dir()
        .and_then(|d| std::fs::read_to_string(d.join(FLOW_BLOCK_OFF_FILE)).ok())
        .is_some_and(|list| listed_in(&list, cwd))
}

fn listed_in(list: &str, cwd: &std::path::Path) -> bool {
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let cwd = canon(cwd);
    list.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .any(|l| cwd.starts_with(canon(std::path::Path::new(l))))
}

fn research_lead_zh(title: &str) -> String {
    format!(
        "## 本任务的流程：研究（已开工）\n\
研究的交付物是知识，不是代码。PRD 纪律里管生产代码的部分（Rule 1/3/4：worktree、提交、P-task 节奏里的测试）不适用：\
**不需要 worktree、不提交、不跑 test-affected**；一次性探针脚本放 scratchpad。真要改代码，先经{title}同意，再开 `kind=exec` 子计划按开发流程做。\n\
- **按计划推进**：开工卡已建好 `kind=research` 计划，P 就是子问题。新发现改变了拆法，就 `fleet__plan action=replan` 重拆未勾的 P 并写理由。\n\
- **按难度定扇出**：事实查找自己做；对比类派 2–4 个子任务；复杂研究 5 个以上。每个都写完整简报：要回答什么、范围、交回格式（结论｜证据｜置信度｜没查到什么）。子任务交回的结论先自己核过再采信。\n\
- **知识台账**：在该计划块下维护「知识台账」段。每得一条结论就当场记一行，不攒到最后：结论｜证据指针｜类型（数据/文献/推断）｜置信度（高/中/低）｜状态（证实/证伪/不确定）。另列开放问题、矛盾、走不通的路。勾 P 之前，它的结论必须已在台账里。\n\
- **停止规则**：预算用完、最近一轮没有新结论、或问题已答——满足任一条就 `fleet__plan action=stop` 写明理由收尾，剩下的 P 不必硬做，别为勾框而勾框。\n\
- **报告**：按开工卡定的交付形式。wiki 报告用 `fleet__wiki` 发布：每条结论带来源，推断类结论单独标注，并写一节「没覆盖什么」；收尾卡里给结论摘要和报告 slug。"
    )
}

fn research_lead_en(title: &str) -> String {
    format!(
        "## This task's workflow: research (kicked off)\n\
Research delivers knowledge, not code. The parts of PRD discipline that govern production code (Rules 1/3/4: worktrees, commits, the tests in the P-task cadence) do not apply: \
**no worktree, no commits, no test-affected**; one-off probe scripts go in the scratchpad. If code really has to change, get {title}'s agreement first, then open a `kind=exec` child plan and follow the dev workflow.\n\
- **Work the plan**: the kickoff card already created a `kind=research` plan whose P-tasks are the sub-questions. When a finding changes how the question splits, `fleet__plan action=replan` the unticked P-tasks with a reason.\n\
- **Fan out by difficulty**: look up facts yourself; send 2–4 sub-tasks for a comparison; 5+ for complex research. Give each a complete brief: what to answer, scope, return format (conclusion | evidence | confidence | what was not found). Check a sub-task's conclusions yourself before relying on them.\n\
- **Findings ledger**: keep a \"findings ledger\" section under that plan block. Record each conclusion the moment you have it, not at the end: conclusion | evidence pointer | type (data/literature/inference) | confidence (high/medium/low) | status (confirmed/refuted/uncertain). Also list open questions, contradictions and dead ends. Before ticking a P, its conclusions must be in the ledger.\n\
- **Stop rule**: budget spent, the last round produced no new finding, or the question is answered — any one of them, then `fleet__plan action=stop` with the reason; the remaining P-tasks need not be done, do not tick boxes for their own sake.\n\
- **Report**: in the form the kickoff card set. A wiki report goes out with `fleet__wiki`: a source for every conclusion, inferences marked as such, and a section on what it does not cover; the closing card gives the conclusions in brief and the report's slug."
    )
}

const RESEARCH_SUBTASK_ZH: &str = "## 本任务的流程：研究（你是子任务）\n\
- 你的交付物是**结论 + 证据 + 置信度 + 没查到什么**，写进最终回复；不必交 commit。\n\
- 每条结论标明证据指针和类型（数据/文献/推断），推断单独标注；查过但走不通的路也写上。\n\
- 只读调查，不需要 worktree、提交或 test-affected；一次性探针脚本放 scratchpad。发现非改代码不可，写进最终回复交给统筹定。";

const RESEARCH_SUBTASK_EN: &str = "## This task's workflow: research (you are a sub-task)\n\
- Your deliverable is **conclusions + evidence + confidence + what you could not find**, in your final reply; no commit needed.\n\
- Give each conclusion an evidence pointer and a type (data/literature/inference), mark inferences as such, and list the paths you tried that went nowhere.\n\
- Investigate read-only: no worktree, commits or test-affected; one-off probe scripts go in the scratchpad. If the answer needs a code change, say so in your final reply and let the coordinator decide.";

/// Where a research task's main conversation stands, for [`research_next_line`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResearchStep {
    /// The research plan's first unticked P-task, as written in TASKS.md
    /// (e.g. `**P2** — 对比三家的定价`).
    Next { plan_id: String, task: String },
    /// The research plan has no open work left (all ticked, or stopped).
    Done { plan_id: String },
    /// No research plan is attributed to the session.
    NoPlan,
}

/// The one-line research nudge injected on every turn of a research task's
/// main conversation — deliberately outside the deduplicated reminder (see
/// [`crate::prd_tasks::render_task_nudge`]): the full research block above is
/// sent once and drifts further up a long transcript, this line does not.
pub fn research_next_line(step: &ResearchStep, locale: &str) -> String {
    let zh = locale == "zh";
    match (step, zh) {
        (ResearchStep::Next { plan_id, task }, true) => format!(
            "研究：下一步 {} — 计划 `{plan_id}`；结论记进知识台账，满足停止规则就 `fleet__plan action=stop`。",
            task.trim()
        ),
        (ResearchStep::Next { plan_id, task }, false) => format!(
            "Research: next is {} — plan `{plan_id}`; record conclusions in the findings ledger, and `fleet__plan action=stop` once a stop rule holds.",
            task.trim()
        ),
        (ResearchStep::Done { plan_id }, true) => format!(
            "研究：计划 `{plan_id}` 已无待办；按开工卡的交付形式出报告（`fleet__wiki`，含「没覆盖什么」）并发收尾卡。"
        ),
        (ResearchStep::Done { plan_id }, false) => format!(
            "Research: plan `{plan_id}` has nothing left open; deliver the report in the kickoff card's form (`fleet__wiki`, with what it does not cover) and raise the closing card."
        ),
        (ResearchStep::NoPlan, true) => "研究：还没有归属本会话的 research 计划——用 `fleet__plan action=create kind=research` 建一个，P 就是子问题，再开始查。".to_string(),
        (ResearchStep::NoPlan, false) => "Research: no research plan is attributed to this session — create one with `fleet__plan action=create kind=research`, one P-task per sub-question, before you start.".to_string(),
    }
}

fn dev_lead_zh(_title: &str) -> String {
    format!(
        "## 本任务的流程：开发\n\
提交纪律、worktree、计划节奏照 PRD 纪律 Rule 1/3/4 执行，此处只补一条：\n\
- **接力后先冒烟**：后继者动手前，先把已勾掉的步骤的验收命令跑一遍，确认没被破坏。"
    )
}

fn dev_lead_en(_title: &str) -> String {
    format!(
        "## This task's workflow: dev\n\
Commit discipline, worktrees and plan cadence follow PRD discipline Rules 1/3/4; this adds one thing:\n\
- **Smoke-test after a relay**: before a successor changes anything, it reruns the acceptance commands of the steps already ticked."
    )
}

const DEV_SUBTASK_ZH: &str = "## 本任务的流程：开发（你是子任务）\n\
- 最终回复附证据：测试输出、截图或命令结果，以及合并的 commit。\n\
- 接着前一棒做时，先把已勾掉步骤的验收命令跑一遍再动手。";

const DEV_SUBTASK_EN: &str = "## This task's workflow: dev (you are a sub-task)\n\
- Back your final reply with evidence: test output, screenshots or command results, and the merge commit.\n\
- When continuing someone else's work, rerun the acceptance commands of the ticked steps before changing anything.";

#[cfg(test)]
mod tests {
    use super::*;

    const LOCALES: [&str; 2] = ["zh", "en"];
    const ROLES: [Role; 2] = [Role::Lead, Role::Subtask];

    #[test]
    fn flow_block_off_list_matches_workspace_and_below() {
        let ws = tempfile::tempdir().unwrap();
        let wt = ws.path().join(".worktrees/x");
        std::fs::create_dir_all(&wt).unwrap();
        let other = tempfile::tempdir().unwrap();
        let list = format!("# ablation control arm\n\n  {}  \n", ws.path().display());
        assert!(listed_in(&list, ws.path()));
        assert!(listed_in(&list, &wt));
        assert!(!listed_in(&list, other.path()));
        assert!(!listed_in("", ws.path()));
        // A sibling sharing a name prefix is not "below" the listed path.
        let sib = std::path::PathBuf::from(format!("{}-2", ws.path().display()));
        assert!(!listed_in(&list, &sib));
    }

    #[test]
    fn general_has_no_block() {
        for locale in LOCALES {
            for role in ROLES {
                assert!(render_block(TaskType::General, role, "", locale).is_none());
            }
        }
    }

    #[test]
    fn research_lead_carries_every_mechanism() {
        for locale in LOCALES {
            let b = render_block(TaskType::Research, Role::Lead, "Chief", locale).unwrap();
            for needle in ["research", "replan", "action=stop", "fleet__wiki", "Chief"] {
                assert!(b.contains(needle), "{locale} research block lacks {needle}: {b}");
            }
        }
        let zh = render_block(TaskType::Research, Role::Lead, "", "zh").unwrap();
        for needle in [
            "2–4",
            "5 个以上",
            "知识台账",
            "证实/证伪/不确定",
            "没覆盖什么",
            "老板",
            "不需要 worktree、不提交、不跑 test-affected",
        ] {
            assert!(zh.contains(needle), "zh research block lacks {needle}");
        }
        let en = render_block(TaskType::Research, Role::Lead, "", "en").unwrap();
        assert!(en.contains("no worktree, no commits, no test-affected"), "{en}");
    }

    /// The plan card moved into the kickoff card; a kicked-off research task
    /// must not be told to raise it again.
    #[test]
    fn research_lead_no_longer_opens_with_a_plan_card() {
        let zh = render_block(TaskType::Research, Role::Lead, "", "zh").unwrap();
        assert!(!zh.contains("计划卡"), "{zh}");
        let en = render_block(TaskType::Research, Role::Lead, "", "en").unwrap();
        assert!(!en.contains("plan card"), "{en}");
    }

    #[test]
    fn research_next_line_names_the_step_or_the_missing_plan() {
        let next = ResearchStep::Next {
            plan_id: "hf-events".into(),
            task: "**P2** — 对比三家".into(),
        };
        let zh = research_next_line(&next, "zh");
        assert!(zh.starts_with("研究：下一步 **P2** — 对比三家"), "{zh}");
        assert!(zh.contains("知识台账") && zh.contains("hf-events"), "{zh}");
        let none = research_next_line(&ResearchStep::NoPlan, "zh");
        assert!(none.contains("kind=research") && !none.contains("kickoff"), "{none}");
        let done = research_next_line(&ResearchStep::Done { plan_id: "x".into() }, "en");
        assert!(done.contains("fleet__wiki"), "{done}");
        // The nudge must never be mistaken for a plan reminder by the dedup.
        for step in [next, ResearchStep::NoPlan, ResearchStep::Done { plan_id: "x".into() }] {
            for locale in LOCALES {
                let line = research_next_line(&step, locale);
                assert!(!line.contains(crate::prd_tasks::PLAN_REMINDER_MARK), "{line}");
                assert!(!line.contains('\n'), "one line: {line}");
            }
        }
    }

    #[test]
    fn dev_lead_points_at_existing_rules_and_adds_the_smoke_test() {
        let zh = render_block(TaskType::Dev, Role::Lead, "", "zh").unwrap();
        assert!(zh.contains("Rule 1/3/4"));
        assert!(zh.contains("冒烟"), "zh dev block lacks the relay smoke test");
        // The independent-check and report-with-evidence items were dropped
        // after the 2026-10-20 ablation measured 0/4 compliance with and
        // without them (wiki experiments/task-type-flow-block-ablation).
        for gone in ["独立核验", "汇报附证据"] {
            assert!(!zh.contains(gone), "zh dev block still has {gone}");
        }
        // Points at the worktree rule, does not restate it.
        assert!(!zh.contains("git worktree add"));
    }

    #[test]
    fn subtask_blocks_never_ask_for_cards() {
        for locale in LOCALES {
            for t in [TaskType::Dev, TaskType::Research] {
                let b = render_block(t, Role::Subtask, "", locale).unwrap();
                assert!(!b.contains("计划卡") && !b.contains("plan card"), "{b}");
            }
        }
        let r = render_block(TaskType::Research, Role::Subtask, "", "zh").unwrap();
        assert!(r.contains("不必交 commit") && r.contains("没查到什么"));
        assert!(r.contains("不需要 worktree"), "{r}");
    }

    /// Each block is injected every turn (deduplicated); keep it to the
    /// ~600-character budget the design set — 1000 for the research lead,
    /// which carries the full research layer (counted in chars, so CJK and
    /// English compare).
    #[test]
    fn blocks_stay_within_budget() {
        for locale in LOCALES {
            for role in ROLES {
                for t in [TaskType::Dev, TaskType::Research] {
                    let b = render_block(t, role, "老板", locale).unwrap();
                    // The research lead is the whole research layer (design
                    // v2 §4), not an add-on, so it gets a larger allowance.
                    let limit = match (t, role, locale) {
                        (TaskType::Research, Role::Lead, "zh") => 1000,
                        (TaskType::Research, Role::Lead, _) => 2400,
                        (_, _, "zh") => 600,
                        _ => 1500,
                    };
                    assert!(
                        b.chars().count() <= limit,
                        "{t:?}/{role:?}/{locale} is {} chars",
                        b.chars().count()
                    );
                }
            }
        }
    }

    #[test]
    fn drift_line_names_split_and_the_user() {
        let zh = drift_line("", "zh");
        assert!(zh.contains("fleet__task action=split") && zh.contains("老板"));
        assert!(zh.contains("开为新任务") && zh.contains("并入本任务"));
        let en = drift_line("Chief", "en");
        assert!(en.contains("fleet__task action=split") && en.contains("Chief"));
    }
}
