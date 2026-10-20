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
        (TaskType::General, true) => "通用",
        (TaskType::Dev, false) => "dev",
        (TaskType::Research, false) => "research",
        (TaskType::General, false) => "general",
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

fn research_lead_zh(title: &str) -> String {
    format!(
        "## 本任务的流程：研究\n\
- **开工先发计划卡**：研究问题、子问题拆分、预算档位，{title}改过再开跑。\n\
- **按难度定扇出**：事实查找自己做；对比类派 2–4 个子 agent；复杂研究 5 个以上，每个都写完整简报（要回答什么、范围、交回格式）。\n\
- **用 `kind=research` 建计划**。结论记在该计划块下的「知识台账」段，每条一行：结论｜证据指针｜类型（数据/文献/推断）｜置信度｜状态（证实/证伪/不确定）；另列开放问题、矛盾、走不通的路。\n\
- 新发现改变了拆法，就 `fleet__plan action=replan` 重拆未开始的步骤并写理由。\n\
- **停止规则**：预算用完、最近一轮没有新结论、或问题已答——满足任一条就 `fleet__plan action=stop` 写明理由收尾，别为勾框而勾框。\n\
- **报告发 `fleet__wiki`**：每条结论带来源，推断类结论单独标注，并写一节「没覆盖什么」。研究不需要 worktree 和提交。"
    )
}

fn research_lead_en(title: &str) -> String {
    format!(
        "## This task's workflow: research\n\
- **Open with a plan card**: the research question, its sub-questions and a budget tier — let {title} edit it before you start.\n\
- **Fan out by difficulty**: look up facts yourself; send 2–4 subagents for a comparison; 5+ for complex research, each with a complete brief (what to answer, scope, return format).\n\
- **Create the plan with `kind=research`**. Record conclusions in a \"findings ledger\" section under that plan block, one line each: conclusion | evidence pointer | type (data/literature/inference) | confidence | status (confirmed/refuted/uncertain); also list open questions, contradictions and dead ends.\n\
- When a finding changes how the question splits, `fleet__plan action=replan` the unstarted steps with a reason.\n\
- **Stop rule**: budget spent, the last round produced no new finding, or the question is answered — any one of them, then `fleet__plan action=stop` with the reason; do not tick boxes for their own sake.\n\
- **Publish the report with `fleet__wiki`**: a source for every conclusion, inferences marked as such, and a section on what it does not cover. Research needs no worktree or commits."
    )
}

const RESEARCH_SUBTASK_ZH: &str = "## 本任务的流程：研究（你是子任务）\n\
- 你的交付物是**结论 + 证据 + 置信度 + 没查到什么**，写进最终回复；不必交 commit。\n\
- 每条结论标明证据指针和类型（数据/文献/推断），推断单独标注；查过但走不通的路也写上。";

const RESEARCH_SUBTASK_EN: &str = "## This task's workflow: research (you are a sub-task)\n\
- Your deliverable is **conclusions + evidence + confidence + what you could not find**, in your final reply; no commit needed.\n\
- Give each conclusion an evidence pointer and a type (data/literature/inference), mark inferences as such, and list the paths you tried that went nowhere.";

fn dev_lead_zh(title: &str) -> String {
    format!(
        "## 本任务的流程：开发\n\
提交纪律、worktree、计划节奏照 PRD 纪律 Rule 1/3/4 执行，此处只补三条：\n\
- **交付前独立核验**：多步 exec 计划在最终汇报前，派一个干净上下文的核验子任务，拿{title}的原始症状和验收标准去证伪你的改动，而不是复述你的结论。\n\
- **接力后先冒烟**：后继者动手前，先把已勾掉的步骤的验收命令跑一遍，确认没被破坏。\n\
- **汇报附证据**：贴测试输出、截图或命令结果，不只说「已通过」。"
    )
}

fn dev_lead_en(title: &str) -> String {
    format!(
        "## This task's workflow: dev\n\
Commit discipline, worktrees and plan cadence follow PRD discipline Rules 1/3/4; this adds three things:\n\
- **Independent check before delivery**: before the final report of a multi-step exec plan, spawn a verification sub-task with a clean context that tries to refute your change against {title}'s original symptom and acceptance criteria — not one that restates your conclusion.\n\
- **Smoke-test after a relay**: before a successor changes anything, it reruns the acceptance commands of the steps already ticked.\n\
- **Report with evidence**: paste test output, screenshots or command results, not just \"passes\"."
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
        for needle in ["计划卡", "2–4", "5 个以上", "知识台账", "证实/证伪/不确定", "没覆盖什么", "老板"] {
            assert!(zh.contains(needle), "zh research block lacks {needle}");
        }
    }

    #[test]
    fn dev_lead_points_at_existing_rules_and_adds_three() {
        let zh = render_block(TaskType::Dev, Role::Lead, "", "zh").unwrap();
        assert!(zh.contains("Rule 1/3/4"));
        for needle in ["独立核验", "冒烟", "证据"] {
            assert!(zh.contains(needle), "zh dev block lacks {needle}");
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
    }

    /// Each block is injected every turn; keep it to the ~600-character
    /// budget the design set (counted in chars, so CJK and English compare).
    #[test]
    fn blocks_stay_within_budget() {
        for locale in LOCALES {
            for role in ROLES {
                for t in [TaskType::Dev, TaskType::Research] {
                    let b = render_block(t, role, "老板", locale).unwrap();
                    let limit = if locale == "zh" { 600 } else { 1500 };
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
