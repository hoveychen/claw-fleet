//! Task / sub-task roles — the one place that words how a task's main
//! conversation (the coordinator) and the sub-tasks it spawns behave, and which
//! words the agent uses with the user.
//!
//! The user sees three levels: project (a code directory) ⊃ task (one idea,
//! one main conversation) ⊃ sub-task (a session the main conversation spawned,
//! see [`crate::task_model`]). Only the main conversation reports to the user;
//! a sub-task returns its result as its final reply, which the coordinator
//! collects through a session watch. A sub-task raises no card at all: an open
//! question goes back in that final reply and the coordinator settles it,
//! asking the user itself only when it has to (the server refuses a sub-task's
//! `fleet__ask` as the safety net).
//!
//! Two carriers:
//!
//! - [`render_section`]: the standing rules, embedded in every interaction
//!   block (Claude's [`crate::interaction_mode`], and the English-only codex /
//!   dsh blocks).
//! - [`render_role_reminder`]: one line per turn, through
//!   [`crate::prd_tasks::render_active_plans_reminder`], telling a sub-task it
//!   is one. A launch-time rule alone does not survive compaction, and codex
//!   follows prompt-prepended text far better than its AGENTS.md (memory
//!   `codex_agents_md_short_rule_ignored`).

/// The standing section. `locale` other than `zh` gets the English text, the
/// same split the interaction blocks use.
pub fn render_section(title: &str, locale: &str) -> String {
    if locale == "zh" {
        return format!(
            "## 任务、子任务与汇报\n\
\n\
{title}眼里只有三层：**项目**（一个代码目录）⊃ **任务**（一个想法，一条主对话）⊃ **子任务**（主对话派出去并行干活的会话）；任务的进度清单叫**步骤**。你是哪一种，看每轮注入的「你的角色」一行——没有这一行，你就是任务的主对话。\n\
\n\
**你是任务主对话（统筹）时：**\n\
- 小事自己做。值得并行拆出去的活，**先发一张方案卡**写清要派哪几个子任务、各做什么、怎样算做完，{title}确认后再用 `fleet__spawn` 派出，不要先斩后奏。{title}已经批准过的计划里写明要并行的部分，视为已确认。Agent/Task 工具派出的子代理是你自己的内部手段，不算子任务，不用先问。\n\
- 子任务是去做你计划里某一步的，派的时候给 `fleet__spawn` 带上 `step`（如 `P3`，计划缺省取你当前的计划）：{title}在步骤列表里就能看到哪一步有人在做。做计划外杂活的子任务不带。\n\
- 派出后用 `fleet__watch` 的 `sessions` 等它们（一次可以等多个），然后结束回合，不要轮询。被唤醒后用 `fleet__inspect`（`action=get`）取每个子任务的最终回复，自己核对过再用。\n\
- 子任务的最终回复里可能带着**待澄清的问题**——澄清归你，不归{title}。能按需求和上下文定的自己定；只有真要{title}拍板的，才由你发卡问{title}。定下来后用 `fleet__spawn` 派一个新子任务带着答复接着做（简报里写清前一个做到哪），或者自己做完；别用 `fleet__control` 的 send 给已交回结果的子任务派新活——它多半已经退出，消息会丢。\n\
- **只有你向{title}汇报。**子任务都回报后，把结果汇总成一张卡；整件事做完了才设 `taskComplete: true`，{title}在任务列表里看到的就是「待验收」。\n\
\n\
**你是子任务时：**\n\
- **一张卡都不发**——汇报卡、决策卡、澄清卡都不发，也不设 `taskComplete`。你只对主对话负责，任务的汇报、结束和放弃只属于主对话；服务端也会拒掉子任务的 `fleet__ask`。做完后把结果（改了什么、合并的 commit、怎么验证的、遗留问题）写成最终回复，主对话会来取。\n\
- **需要澄清时交回主对话**（需求不清、方向上的岔路、要不要做破坏性操作）：能按简报和常识定的自己定，并在最终回复里写明做了什么假设；定不了的，停在安全的地方（已提交、没有做到一半的破坏性操作），把问题、候选答案和你的倾向写进最终回复后结束回合，由主对话来定。\n\
- 接力、等待、步骤照常使用。\n\
\n\
**对{title}说话只用这套词**：项目、任务、子任务、步骤、待你处理、待验收、进行中、等待中、已完成、资料。棒、接力、P-task、计划树、watch、会话、worktree、TASKS.md 是内部机制，不要出现在卡片和汇报里：接力说成「上下文已续接」，P3 说成「第 3 步」，watch 说成「等待中：等 CI 通过」这类具体的话。工具名和命令照常用，这条只管你写给{title}看的文字。\n"
        );
    }
    format!(
        "## Tasks, Sub-tasks And Who Reports\n\
\n\
{title} sees three levels: a **project** (one code directory) ⊃ a **task** (one idea, one main conversation) ⊃ **sub-tasks** (sessions the main conversation spawned to work in parallel); a task's progress list is its **steps**. Which one you are is stated by the per-turn \"your role\" line — with no such line, you are a task's main conversation.\n\
\n\
**As a task's main conversation (the coordinator):**\n\
- Do small things yourself. When work is worth splitting off in parallel, **first raise a proposal card** naming the sub-tasks, what each does and what done means; spawn them with `fleet__spawn` only after {title} confirms. Parallel work already written into a plan {title} approved counts as confirmed. Agent/Task-tool subagents are your own internal means, not sub-tasks, and need no confirmation.\n\
- When a sub-task carries out a step of your plan, pass `step` to `fleet__spawn` (e.g. `P3`; the plan defaults to the one you are on), so {title} sees in the step list which step has someone on it. Leave it off for work outside your plan.\n\
- Once spawned, wait for them with `fleet__watch` and its `sessions` field (one watch can wait on several) and end the turn — do not poll. When woken, fetch each sub-task's final reply with `fleet__inspect` (`action=get`) and check it before relying on it.\n\
- A sub-task's final reply may carry **open questions** — clarifying them is your job, not {title}'s. Settle what the request and context allow; raise a card to {title} only for a call {title} really has to make. Then `fleet__spawn` a fresh sub-task carrying the answer (its brief says how far the last one got), or finish the work yourself; do not hand new work to a sub-task that already reported with `fleet__control` send — it has most likely exited and the message is lost.\n\
- **Only you report to {title}.** When every sub-task has reported back, summarise the result in one card; set `taskComplete: true` only when the whole task is done — that is what {title} sees as \"awaiting review\".\n\
\n\
**As a sub-task:**\n\
- **Raise no card at all** — no report, decision or clarification card — and never set `taskComplete`. You answer to the main conversation; reporting, ending and abandoning the task belong to it, and the server refuses a sub-task's `fleet__ask`. When done, write the result (what changed, the merge commit, how it was verified, what is left) as your final reply; the main conversation collects it.\n\
- **Hand clarifications back to the main conversation** (an unclear requirement, a fork in direction, whether to do something destructive): settle what the brief and common sense allow and state the assumptions in your final reply; for the rest, stop somewhere safe (committed, no destructive step half done), write the question, the candidate answers and your leaning into the final reply and end the turn — the main conversation decides.\n\
- Handoff, waiting and steps work as usual.\n\
\n\
**Talk to {title} in these words only**: project, task, sub-task, step, needs you, awaiting review, in progress, waiting, done, materials. Hop, relay, P-task, plan tree, watch, session, worktree and TASKS.md are internal mechanics and stay out of cards and reports: a relay is \"context carried over\", P3 is \"step 3\", a watch is \"waiting: for CI to pass\". Tool names and commands are unaffected; this governs only the text you write for {title}.\n"
    )
}

/// The compact English rendering for the codex / dsh interaction blocks,
/// which share codex's 32 KiB AGENTS.md budget. The sub-task rule that matters
/// most also arrives every turn through [`render_role_reminder`].
pub fn render_section_compact(title: &str) -> String {
    format!(
        "## Tasks, Sub-tasks And Who Reports\n\
\n\
- With no \"Your role: sub-task\" / \"你的角色：子任务\" line in the per-turn injection you are a task's main conversation: do small things yourself; before spawning sub-tasks with `fleet__spawn`, raise a proposal card and wait for {title} to confirm; then wait on them with `fleet__watch` `sessions`, read each final reply with `fleet__inspect`, and report the summary — only you report to {title}.\n\
- As a sub-task: raise no card at all and never set `taskComplete`; your final reply is the result. Put anything needing clarification in that reply (question, options, your leaning) and let the main conversation settle it.\n\
- As the main conversation, settle a sub-task's open questions yourself, asking {title} only for a real call; continue with a fresh `fleet__spawn`, not a `send` to the finished sub-task.\n\
- In cards say project / task / sub-task / step / waiting, never hop, relay, P-task, plan tree, watch or session."
    )
}

/// The per-turn role line for a sub-task; `None` for a task's main
/// conversation (the default the section assumes) and for unknown sessions.
/// Worded in the voice (title, locale) Fleet's launches use, like the section
/// it points at.
pub fn render_role_reminder(session_id: Option<&str>) -> Option<String> {
    let sid = session_id?;
    let graph = crate::task_model::TaskGraph::load();
    let (title, locale) = crate::claude_launch::guidance_voice();
    role_reminder_for(&graph, sid, &title, &locale)
}

/// [`render_role_reminder`] on its own, wrapped as a `<system-reminder>`: the
/// per-turn carrier for a session whose plan injection is switched off
/// (PRD discipline / the prd-context hook). The role line is part of the
/// task-roles rules, not of the plans, so it must not go quiet with them.
pub fn render_role_only_reminder(session_id: Option<&str>) -> Option<String> {
    render_role_reminder(session_id)
        .map(|line| format!("<system-reminder>\n{line}\n</system-reminder>"))
}

/// Shown to a sub-task that tried to raise a decision card. Agent-facing, so
/// English like the other tool refusals.
pub const SUBTASK_ASK_REFUSAL: &str = "\
Asking the user (fleet__ask, AskUserQuestion, ask_user_question) is not available to sub-tasks. \
You were spawned by a task's main conversation, and only that conversation talks to the user. Do \
not retry and do not fall back to another asking tool. Put \
the report — and any question that needs clarifying, with the candidate answers and your leaning \
— in your final reply and end the turn; the main conversation reads it and settles the question \
itself, asking the user only if it has to.";

/// The main conversation `session_id` is a sub-task of, if it is one.
pub fn subtask_parent(session_id: &str) -> Option<String> {
    crate::task_model::TaskGraph::load().place(session_id).parent_task_id
}

pub(crate) fn role_reminder_for(
    graph: &crate::task_model::TaskGraph,
    sid: &str,
    user_title: &str,
    locale: &str,
) -> Option<String> {
    let parent = graph.place(sid).parent_task_id?;
    if locale == "zh" {
        let title = if user_title.is_empty() { "老板" } else { user_title };
        return Some(format!(
            "你的角色：子任务（所属任务的主对话 {parent}）。按「任务、子任务与汇报」一节：一张卡都不发、不设 `taskComplete`，\
             结果写进最终回复由主对话来取；要澄清的事也写进最终回复交给主对话，不直接问{title}。"
        ));
    }
    let title = if user_title.is_empty() { "Boss" } else { user_title };
    Some(format!(
        "Your role: sub-task (of the task whose main conversation is {parent}). Per \"Tasks, Sub-tasks And Who Reports\": \
         raise no card at all and never set `taskComplete`; your final reply is the result, which the main conversation \
         collects. Anything needing clarification goes in that reply for the main conversation to settle; do not ask {title} directly."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_model::TaskGraph;

    #[test]
    fn sub_task_gets_a_role_line_naming_its_task() {
        let graph = TaskGraph::new(
            vec![vec!["main-1".to_string(), "main-2".to_string()]],
            vec![("sub-1".to_string(), "main-2".to_string())],
        );
        let line = role_reminder_for(&graph, "sub-1", "", "zh").expect("sub-task gets a role line");
        assert!(line.contains("子任务") && line.contains("main-1"), "{line}");
        assert!(line.contains("老板"), "{line}");
        assert!(line.contains("一张卡都不发") && !line.contains("才发决策卡"), "{line}");
        assert!(role_reminder_for(&graph, "main-2", "", "zh").is_none());
        assert!(role_reminder_for(&graph, "stranger", "", "zh").is_none());
    }

    #[test]
    fn role_line_follows_title_and_locale() {
        let graph = TaskGraph::new(
            vec![vec!["main-1".to_string()]],
            vec![("sub-1".to_string(), "main-1".to_string())],
        );
        let zh = role_reminder_for(&graph, "sub-1", "主人", "zh").unwrap();
        assert!(zh.starts_with("你的角色：子任务") && zh.contains("主人") && !zh.contains("老板"), "{zh}");
        for locale in ["en", "ja", "ko"] {
            let en = role_reminder_for(&graph, "sub-1", "", locale).unwrap();
            assert!(en.starts_with("Your role: sub-task") && en.contains("Boss"), "{locale}: {en}");
            assert!(!en.contains("老板"), "{locale}: {en}");
        }
        let named = role_reminder_for(&graph, "sub-1", "Alex", "en").unwrap();
        assert!(named.contains("Alex") && !named.contains("Boss"), "{named}");
    }

    #[test]
    fn section_covers_both_roles_in_both_locales() {
        for (locale, needles) in [
            ("zh", ["先发一张方案卡", "一张卡都不发", "待澄清的问题", "sessions", "第 3 步"]),
            ("en", ["first raise a proposal card", "Raise no card at all", "open questions", "sessions", "step 3"]),
        ] {
            let s = render_section("老板", locale);
            for n in needles {
                assert!(s.contains(n), "{locale}: missing {n:?}");
            }
        }
    }
}
