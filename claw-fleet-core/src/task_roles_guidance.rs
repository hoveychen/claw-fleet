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
//! Carriers:
//!
//! - [`render_section`]: the coordinator's standing rules, embedded in
//!   Claude's [`crate::interaction_mode`] block. A Claude sub-task is launched
//!   without that block at all and gets [`render_subtask_section`] inside
//!   [`crate::interaction_mode::render_subtask_guidance`] instead — it has no
//!   use for card rules it may not follow (`claude_launch` picks by role).
//! - [`render_section_compact`]: both roles in one, for the codex / dsh
//!   blocks, which are written once per machine and cannot vary per session.
//!   Their size budget has no room for the briefing and model-choice rules;
//!   those reach every harness through the `fleet__spawn` tool description.
//! - [`render_role_reminder`]: per turn, through
//!   [`crate::prd_tasks::render_active_plans_reminder`], telling a sub-task it
//!   is one, and telling a coordinator which of its sub-tasks died mid-turn. A launch-time rule alone does not survive compaction, and codex
//!   follows prompt-prepended text far better than its AGENTS.md (memory
//!   `codex_agents_md_short_rule_ignored`).

/// The coordinator's standing section. `locale` other than `zh` gets the
/// English text, the same split the interaction blocks use.
pub fn render_section(title: &str, locale: &str) -> String {
    let drift = crate::task_workflow_guidance::render_drift_section(title, locale);
    if locale == "zh" {
        return format!(
            "## 任务、子任务与汇报\n\
\n\
{title}眼里只有三层：**项目**（一个代码目录）⊃ **任务**（一个想法，一条主对话）⊃ **子任务**（主对话派出去干活的会话）；任务的进度清单叫**步骤**。你是哪一种，看每轮注入的「你的角色」一行——没有这一行，你就是任务的主对话。\n\
\n\
**你是任务主对话（统筹）时：**\n\
- **你统筹，不当主力开发。**规划、写简报、核对子任务交回的结果、合并、汇报归你；小事（一两处小改、一次核对）自己做。凡是要大量读改代码、跑长回归、截图眼验的活，**哪怕是串行的、后面各步都依赖它**，也派成单个子任务，等它交回再派下一批——「不能并行」不等于「该我做」。你的上下文要撑完整个计划，自己做到一半就得接力，统筹就断了。\n\
- 要派子任务，**先发一张方案卡**写清每一步由哪个子任务做、各做什么、怎样算做完（不要写「这步我自己做」，除非它真是小事），{title}确认后再用 `fleet__spawn` 派出，不要先斩后奏。{title}已经批准过的计划里写明要派子任务的部分，视为已确认。Agent/Task 工具派出的子代理是你自己的内部手段，不算子任务，不用先问。\n\
- **规划归你，调研可以派。**计划由你定，因为只有你见过{title}的原话、也只有你对结果负责；交给子任务去出计划，它只能对着简报写一份读着周全的方案，传一手就偏一手，还会越写越复杂。\n\
  - 派去调研的子任务（或子代理）**只问事实，不要方案**：「X 被哪些地方调用」「这个限制写死在哪」「这样改会不会动到 Y」，要求带文件位置回来；关键结论你自己再核对一遍。\n\
  - **计划只写到下一个未知点。**第一步做最薄、能跑通的一条，后面的步骤允许被前面的执行结果改写；动手前就排出一长串步骤，多半是在规划想象。\n\
  - **每一步的「做完」都要能观察到**：命令输出了什么、哪个测试变绿、截图上出现了什么，不是「方案通过」。评判的是事情做没做对，不是计划好不好看。\n\
  - 真正难的设计取舍可以派一个高 effort 或更强模型的子任务，但给它{title}的原话，让它交「几个选项 + 各自代价」，由你来选。\n\
- **目标复杂时先倒推，再拆分和选模型。**从终点往回想几步：要到达它，前面必须先成立什么，哪一步最可能卡住。需要高智能或长时间考究的难点（架构取舍、疑难根因、并发/算法正确性、大范围重构）单独拆成子任务，用 `fleet__spawn` 的 `model` / `effort` 给它更高的 effort 或更强的模型（如 Fable，见模型速查）；机械活继承或降档。\n\
- **简报只写对方想不到的。**子任务和你是同一个模型，知识、能力和判断都相同，你一句话就懂的事它也一句话就懂。所以简报的篇幅留给它从仓库和常识里推不出来的东西：{title}的原话和对齐过的结论、已经踩过的坑和排除掉的路、非显然的约束与取舍、验收标准、明确不做的事、该先读的资料。沿用了某个口径或定义（怎么算翻盘、什么算活跃用户），就写明是谁定的、哪些参数还没经{title}确认。怎么写测试、按规范提交、别破坏现有功能这类常识不要展开，也别替它做它自己会做的推理。\n\
- 子任务是去做你计划里某一步的，派的时候给 `fleet__spawn` 带上 `step`（如 `P3`，计划缺省取你当前的计划）：{title}在步骤列表里就能看到哪一步有人在做。做计划外杂活的子任务不带。\n\
- 派出后用 `fleet__watch` 的 `sessions` 等它们（一次可以等多个），然后结束回合，不要轮询。被唤醒后用 `fleet__inspect`（`action=get`）取每个子任务的最终回复，自己核对过再用。\n\
- **「方案不行」之前先证明不是实现坏了。**做出来的效果不好时，默认嫌疑是你们自己的实现，不是方案、工具或上游。多级管线（生成 → 转换 → 接入）的验收除了「和现状比」，还要求**每一级的产物和它的输入同条件对照**（同机位、同输入），交给{title}的对照里也带上这一行；缺陷在最上游的原物里就已存在，才能归到方案头上，否则逐级找出从哪一步开始失真。子任务交回的「X 本身就这样」「上游生成的就是这样」这类归因，转述前自己拿证据复核；要{title}在「放弃 / 归档」上拍板的卡，没有原物对照就不发。\n\
- **汇报的结论分证据等级写**：亲自核实过的、推断的、转述别人还没核过的，分开标明，不要用同一种肯定语气写。\n\
- 子任务的最终回复里可能带着**待澄清的问题**——澄清归你，不归{title}。能按需求和上下文定的自己定；只有真要{title}拍板的，才由你发卡问{title}。定下来后用 `fleet__spawn` 派一个新子任务带着答复接着做（简报里写清前一个做到哪），或者自己做完；别用 `fleet__control` 的 send 或 resume 给已交回结果的子任务派新活——send 的消息多半会丢（它已经退出），resume 会被拒绝。\n\
- **断网或服务中断后**，回合中途断掉的子任务会列在每轮注入里：「可恢复」的先核对它已经做完了什么，再用 `fleet__control action=resume id=<id>` 让它接着做；已交回结果的子任务绝不唤醒，新活照样用 `fleet__spawn` 派。Agent/Task 工具派的子代理跟你一起被切断后没法恢复，重新派一次。\n\
- **只有你向{title}汇报。**子任务都回报后，把结果汇总成一张卡；整件事做完了才设 `taskComplete: true`，{title}在任务列表里看到的就是「待验收」。\n\
\n\
{drift}\
\n\
**子任务那边**：子任务启动时就没有决策卡规则和发卡工具，它把结果、自己做的假设和待澄清的问题写在最终回复里交给你。\n\
\n\
**对{title}说话只用这套词**：项目、任务、子任务、步骤、待你处理、待验收、进行中、等待中、已完成、资料。棒、接力、P-task、计划树、watch、会话、worktree、TASKS.md 是内部机制，不要出现在卡片和汇报里：接力说成「上下文已续接」，P3 说成「第 3 步」，watch 说成「等待中：等 CI 通过」这类具体的话。工具名和命令照常用，这条只管你写给{title}看的文字。\n"
        );
    }
    format!(
        "## Tasks, Sub-tasks And Who Reports\n\
\n\
{title} sees three levels: a **project** (one code directory) ⊃ a **task** (one idea, one main conversation) ⊃ **sub-tasks** (sessions the main conversation spawned to do the work); a task's progress list is its **steps**. Which one you are is stated by the per-turn \"your role\" line — with no such line, you are a task's main conversation.\n\
\n\
**As a task's main conversation (the coordinator):**\n\
- **You coordinate; you are not the lead developer.** Planning, writing briefs, checking what sub-tasks hand back, merging and reporting are yours; do small things (a small edit or two, one check) yourself. Anything that means reading and changing a lot of code, running long regressions or eyeballing screenshots goes to a sub-task — **even when it is serial and every later step depends on it**: spawn it alone, wait for it, then spawn the next batch. \"Cannot run in parallel\" does not mean \"mine to do\". Your context has to last the whole plan; doing the work yourself until you need a relay breaks the coordination.\n\
- Before spawning, **first raise a proposal card** naming which sub-task does each step, what it does and what done means (do not write \"I do this step myself\" unless it really is small); spawn them with `fleet__spawn` only after {title} confirms. Sub-tasks already written into a plan {title} approved count as confirmed. Agent/Task-tool subagents are your own internal means, not sub-tasks, and need no confirmation.\n\
- **Planning is yours; investigation can be delegated.** You set the plan, because only you have seen {title}'s own words and only you answer for the result; a sub-task asked for a plan can only write one that reads well against its brief — each hand-off drifts it further and makes it more elaborate.\n\
  - Ask an investigating sub-task (or subagent) **for facts, not a plan**: \"where is X called\", \"where is this limit hard-coded\", \"would this change touch Y\" — with file locations; check the key conclusions yourself.\n\
  - **Plan only up to the next unknown.** Make the first step the thinnest slice that runs end to end, and let later steps be rewritten by what the earlier ones turn up; a long list of steps before anything has run is mostly planning the imagination.\n\
  - **Every step's \"done\" must be observable**: what a command printed, which test turned green, what a screenshot shows — never \"the design was approved\". What is judged is whether the work is right, not whether the plan looks good.\n\
  - A genuinely hard design trade-off can go to a sub-task with a higher effort or stronger model, but hand it {title}'s own words and ask for options with their costs; you choose.\n\
- **For a complex goal, work backwards before you split and pick models.** Think a few steps back from the end: what has to hold before it can be reached, and which step is most likely to stall. Split the hard parts that need deep reasoning or long investigation (architecture trade-offs, elusive root causes, concurrency or algorithmic correctness, wide refactors) into their own sub-tasks and give them a higher effort or a stronger model through `fleet__spawn`'s `model` / `effort` (e.g. Fable — see the model sheet); mechanical work inherits or steps down.\n\
- **A brief carries only what the sub-task could not think of.** It is the same model as you, with the same knowledge, ability and judgement — what you would get from one sentence, it gets from one sentence. Spend the brief on what it cannot work out from the repo and common sense: {title}'s own words and settled decisions, pitfalls already hit and paths already ruled out, non-obvious constraints and trade-offs, what done means, what is out of scope, and what to read first. When the work inherits a definition (what counts as a comeback, an active user), say who settled it and which of its parameters {title} has not confirmed. Do not spell out common practice (write tests, commit by the rules, don't break existing features), and do not do reasoning for it that it would do itself.\n\
- When a sub-task carries out a step of your plan, pass `step` to `fleet__spawn` (e.g. `P3`; the plan defaults to the one you are on), so {title} sees in the step list which step has someone on it. Leave it off for work outside your plan.\n\
- Once spawned, wait for them with `fleet__watch` and its `sessions` field (one watch can wait on several) and end the turn — do not poll. When woken, fetch each sub-task's final reply with `fleet__inspect` (`action=get`) and check it before relying on it.\n\
- **Before \"the approach doesn't work\", prove the implementation isn't what broke.** When a result looks bad, the default suspect is your own implementation, not the approach, the tool or the upstream. For a multi-stage pipeline (generate → convert → integrate), acceptance compares not only against the current version but **each stage's output against its own input under the same conditions** (same camera, same input), and the comparison {title} sees carries that row too; a defect counts against the approach only once it is shown in the upstream original, otherwise find the stage where fidelity is first lost. A sub-task's attribution like \"X is just like that\" or \"the upstream generated it that way\" gets checked against evidence before you repeat it; a card asking {title} to abandon or archive is not raised without that original-vs-result comparison.\n\
- **Report conclusions by grade of evidence**: what you verified yourself, what you infer, and what you relay from someone else unchecked — labelled apart, not in one confident voice.\n\
- A sub-task's final reply may carry **open questions** — clarifying them is your job, not {title}'s. Settle what the request and context allow; raise a card to {title} only for a call {title} really has to make. Then `fleet__spawn` a fresh sub-task carrying the answer (its brief says how far the last one got), or finish the work yourself; do not hand new work to a sub-task that already reported with `fleet__control` send or resume — it has most likely exited, so a send is lost, and a resume is refused.\n\
- **After a network or service interruption**, sub-tasks that died mid-turn are listed in the per-turn injection: check what a recoverable one already finished, then wake it with `fleet__control action=resume id=<id>`. A sub-task that already reported is never woken — new work still goes through `fleet__spawn`. Agent/Task-tool subagents cut off along with you cannot be resumed; dispatch them again.\n\
- **Only you report to {title}.** When every sub-task has reported back, summarise the result in one card; set `taskComplete: true` only when the whole task is done — that is what {title} sees as \"awaiting review\".\n\
\n\
{drift}\
\n\
**On the sub-task's side**: a sub-task is launched without the card rules or the card tools; it hands you its result, the assumptions it made and any open questions in its final reply.\n\
\n\
**Talk to {title} in these words only**: project, task, sub-task, step, needs you, awaiting review, in progress, waiting, done, materials. Hop, relay, P-task, plan tree, watch, session, worktree and TASKS.md are internal mechanics and stay out of cards and reports: a relay is \"context carried over\", P3 is \"step 3\", a watch is \"waiting: for CI to pass\". Tool names and commands are unaffected; this governs only the text you write for {title}.\n"
    )
}

/// The sub-task's standing section, the core of a Claude sub-task's launch
/// guidance. It says nothing about cards beyond "there are none": the card
/// tools are withheld from a sub-task, so the card rules would only be noise.
pub fn render_subtask_section(title: &str, locale: &str) -> String {
    if locale == "zh" {
        return format!(
            "## 你是子任务\n\
\n\
你是某个任务的主对话（统筹）用 `fleet__spawn` 派出来的子任务，开场 prompt 就是统筹写给你的简报。你只对统筹负责，不直接面对{title}：任务的汇报、结束和放弃都属于统筹。\n\
\n\
- **不发卡，也没有发卡工具。**`fleet__ask`、`fleet__render_a2ui`、`AskUserQuestion` 都不对你开放，不要找替代办法去问{title}。\n\
- **简报只写了你想不到的东西。**统筹和你是同一个模型，常识性的事它不会展开，你按常识和仓库现状去做就好；简报里写明的{title}原话、约束、验收标准和不做的事，要逐条遵守。\n\
- **需要澄清时交回统筹**（需求不清、方向上的岔路、要不要做破坏性操作）：能按简报和常识定的自己定，在最终回复里写明做了什么假设——你自己选的口径、阈值、窗口都算假设，单列成「未经{title}确认」，数据里和口径语义冲突的迹象也一并写出；定不了的，停在安全的地方（已提交、没有做到一半的破坏性操作），把问题、候选答案和你的倾向写进最终回复，然后结束回合，由统筹来定。\n\
- **最终回复就是交付物**，统筹会来取：改了什么、合并的 commit、怎么验证的、遗留问题、做过的假设、待澄清的问题。写给统筹看，不用寒暄和客套。\n\
- **把问题归给上游、工具或方案本身**（「生成的模型就是这样」「库的限制」）**要附证据**：拿上游原物和你的成品同条件对照；没对照过就写成推断，不要写成定论。\n\
- 交付物按任务类型：开发类附证据（测试输出、截图或命令结果）；**研究类交回「结论 + 证据 + 置信度 + 没查到什么」**，推断单独标注，不必交 commit。\n\
- 接力、等待、步骤照常使用。\n"
        );
    }
    format!(
        "## You Are A Sub-task\n\
\n\
A task's main conversation (the coordinator) spawned you with `fleet__spawn`; your opening prompt is its brief. You answer to the coordinator, not to {title} directly: reporting, ending and abandoning the task all belong to it.\n\
\n\
- **No cards, and no card tools.** `fleet__ask`, `fleet__render_a2ui` and `AskUserQuestion` are not open to you; do not look for another way to ask {title}.\n\
- **The brief holds only what you could not think of.** The coordinator is the same model as you and leaves common practice unsaid — follow common sense and the repo as it stands. What the brief does state ({title}'s words, constraints, what done means, what is out of scope) you follow point by point.\n\
- **Hand clarifications back to the coordinator** (an unclear requirement, a fork in direction, whether to do something destructive): settle what the brief and common sense allow and state the assumptions in your final reply — any definition, threshold or window you picked yourself counts, listed as \"not confirmed by {title}\" together with any sign in the data that contradicts what the definition is meant to capture; for the rest, stop somewhere safe (committed, no destructive step half done), write the question, the candidate answers and your leaning into the final reply and end the turn — the coordinator decides.\n\
- **Your final reply is the deliverable** the coordinator collects: what changed, the merge commit, how it was verified, what is left, the assumptions you made and the open questions. Write it for the coordinator; skip pleasantries.\n\
- **Blaming a problem on the upstream, a tool or the approach itself** (\"the generated model is just like that\", \"a library limit\") **needs evidence**: the upstream original compared with your result under the same conditions; without that comparison write it as an inference, not a finding.\n\
- The deliverable depends on the task type: dev work comes with evidence (test output, screenshots or command results); **research hands back conclusions + evidence + confidence + what you could not find**, inferences marked, no commit needed.\n\
- Handoff, waiting and steps work as usual.\n"
    )
}

/// The compact English rendering for the codex / dsh interaction blocks,
/// which share codex's 32 KiB AGENTS.md budget. The sub-task rule that matters
/// most also arrives every turn through [`render_role_reminder`].
pub fn render_section_compact(title: &str) -> String {
    format!(
        "## Tasks, Sub-tasks And Who Reports\n\
\n\
- With no \"Your role: sub-task\" / \"你的角色：子任务\" line in the per-turn injection you are a task's main conversation: do small things yourself, delegate heavy work even when it is serial; before spawning sub-tasks with `fleet__spawn`, raise a proposal card and wait for {title} to confirm; then wait on them with `fleet__watch` `sessions`, read each final reply with `fleet__inspect`, and report the summary — only you report to {title}.\n\
- As a sub-task: raise no card at all and never set `taskComplete`; your final reply is the result. Put anything needing clarification in that reply (question, options, your leaning) and let the main conversation settle it.\n\
- As the main conversation, settle a sub-task's open questions yourself, asking {title} only for a real call; continue with a fresh `fleet__spawn`, not `send`/`resume` on the finished sub-task.\n\
- In cards say project / task / sub-task / step / waiting, never hop, relay, P-task, plan tree, watch or session."
    )
}

/// The per-turn role line for a sub-task; for a task's main conversation (the
/// default the section assumes) and unknown sessions, no role line.
/// Worded in the voice (title, locale) Fleet's launches use, like the section
/// it points at.
///
/// Also carries the coordinator's recovery block ([`recovery_block_for`]) when
/// sub-tasks this session's chain spawned died mid-turn or failed in a way the
/// coordinator has to act on — so every per-turn carrier (the Claude hook, the
/// codex prompt-prepend, the dsh plugin) gets it without wiring of its own.
pub fn render_role_reminder(session_id: Option<&str>) -> Option<String> {
    let sid = session_id?;
    let graph = crate::task_model::TaskGraph::load();
    let (title, locale) = crate::claude_launch::guidance_voice();
    let parts: Vec<String> = [
        role_reminder_for(&graph, sid, &title, &locale),
        recovery_reminder(&graph, sid, &title, &locale),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// Whether a sub-task in this class belongs in the coordinator's per-turn
/// recovery block: the ones it can resume, and the two it must not resume
/// blindly but should know about. Done / Running / Parked / Blocked and the
/// rest stay out — listing a Done one would invite waking it.
pub fn needs_coordinator_attention(class: crate::subtask_recovery::RecoveryClass) -> bool {
    use crate::subtask_recovery::RecoveryClass as C;
    matches!(class, C::Recoverable | C::AuthFailed | C::UserInterrupted)
}

/// How long one turn's reminder may spend probing uncached sub-tasks. The
/// hook runs before every prompt, and the dsh plugin gives its whole guidance
/// command 5 s; children left unprobed are picked up on a later turn.
const RECOVERY_PROBE_BUDGET: std::time::Duration = std::time::Duration::from_millis(800);

fn recovery_reminder(
    graph: &crate::task_model::TaskGraph,
    sid: &str,
    user_title: &str,
    locale: &str,
) -> Option<String> {
    let rows: Vec<(crate::subtask_recovery::SubtaskStatus, Option<String>)> =
        crate::subtask_recovery::children_needing(
            sid,
            graph,
            needs_coordinator_attention,
            RECOVERY_PROBE_BUDGET,
        )
            .into_iter()
            .map(|c| {
                let name = crate::session_title::read(&c.session_id);
                (c, name)
            })
            .collect();
    recovery_block_for(&rows, user_title, locale)
}

/// The coordinator's recovery block: one line per sub-task that needs it
/// (short id, title, step, class, why), then what to do about each class
/// present. `None` when no row qualifies. Rows are filtered here too, so a
/// caller cannot leak a Done sub-task into the list.
pub(crate) fn recovery_block_for(
    rows: &[(crate::subtask_recovery::SubtaskStatus, Option<String>)],
    user_title: &str,
    locale: &str,
) -> Option<String> {
    use crate::subtask_recovery::{short_id, RecoveryClass as C};
    let rows: Vec<_> = rows.iter().filter(|(c, _)| needs_coordinator_attention(c.class)).collect();
    if rows.is_empty() {
        return None;
    }
    let has = |k: C| rows.iter().any(|(c, _)| c.class == k);
    let zh = locale == "zh";
    let title = match (user_title.is_empty(), zh) {
        (false, _) => user_title,
        (true, true) => "老板",
        (true, false) => "Boss",
    };
    let mut out = String::new();
    if zh {
        out.push_str("你派出的子任务里有中途断掉、需要你处理的：\n");
    } else {
        out.push_str("Some sub-tasks you spawned stopped mid-way and need you:\n");
    }
    for (c, name) in &rows {
        let mut line = format!("- `{}`", short_id(&c.session_id));
        if let Some(n) = name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            line.push_str(&if zh { format!("「{n}」") } else { format!(" \"{n}\"") });
        }
        if let Some(step) = c.step.as_deref() {
            line.push_str(&if zh { format!(" 步骤 {step}") } else { format!(" step {step}") });
        }
        let class = match (c.class, zh) {
            (C::Recoverable, true) => "可恢复",
            (C::AuthFailed, true) => "鉴权/账号失败",
            (C::UserInterrupted, true) => "被手动打断",
            (C::Recoverable, false) => "recoverable",
            (C::AuthFailed, false) => "auth/account failure",
            (C::UserInterrupted, false) => "interrupted by hand",
            _ => unreachable!("filtered by needs_coordinator_attention"),
        };
        line.push_str(&format!(" · {class}"));
        let detail = c.detail.trim();
        if !detail.is_empty() {
            line.push_str(&if zh { format!("：{detail}") } else { format!(": {detail}") });
        }
        out.push_str(&line);
        out.push('\n');
    }
    if zh {
        if has(C::Recoverable) {
            out.push_str("「可恢复」的：先核对它已经做完了什么（git 状态、已有提交、已写的文件），再用 `fleet__control action=resume id=<id>` 让它接着做。\n");
        }
        if has(C::AuthFailed) {
            out.push_str(&format!("「鉴权/账号失败」的：先修好凭证或问{title}，不要盲目 resume——修好之前只会再失败一次。\n"));
        }
        if has(C::UserInterrupted) {
            out.push_str(&format!("「被手动打断」的：是{title}有意停下的，除非{title}要求，不要 resume。\n"));
        }
        out.push_str("已交回结果的子任务不会列在这里，也绝不唤醒；要给它新活，用 `fleet__spawn` 派新子任务。");
    } else {
        if has(C::Recoverable) {
            out.push_str("Recoverable: first check what it already finished (git status, existing commits, files written), then wake it with `fleet__control action=resume id=<id>`.\n");
        }
        if has(C::AuthFailed) {
            out.push_str(&format!("Auth/account failure: fix the credentials or ask {title} first; do not resume blindly — it would just fail again.\n"));
        }
        if has(C::UserInterrupted) {
            out.push_str(&format!("Interrupted by hand: {title} stopped it on purpose; do not resume it unless {title} asks.\n"));
        }
        out.push_str("Sub-tasks that already handed back their result are never listed here and are never woken; new work for them goes through `fleet__spawn`.");
    }
    Some(out)
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
    crate::task_model::place_session(session_id).parent_task_id
}

/// Whether the session should see the sub-task's tool set. A Fleet fork (an
/// explain-mark question) takes its origin's role: its tool list is part of
/// the cached prefix it shares with that origin.
pub fn session_has_subtask_role(session_id: &str) -> bool {
    if session_id.is_empty() {
        return false;
    }
    let role_id = crate::launch_spec::get(session_id)
        .and_then(|spec| spec.parent_session_id)
        .unwrap_or_else(|| session_id.to_string());
    subtask_parent(&role_id).is_some()
}

pub(crate) fn role_reminder_for(
    graph: &crate::task_model::TaskGraph,
    sid: &str,
    user_title: &str,
    locale: &str,
) -> Option<String> {
    let parent = graph.place(sid).parent_task_id?;
    Some(subtask_role_line(&parent, user_title, locale))
}

/// The sub-task role line for a session whose task's main conversation is
/// `parent`. Split out so a new codex thread, which has no id to look up yet,
/// can be given the same line on its first prompt.
pub(crate) fn subtask_role_line(parent: &str, user_title: &str, locale: &str) -> String {
    if locale == "zh" {
        let title = if user_title.is_empty() { "老板" } else { user_title };
        return format!(
            "你的角色：子任务（所属任务的主对话 {parent}）。按「任务、子任务与汇报」一节：一张卡都不发、不设 `taskComplete`，\
             结果写进最终回复由主对话来取；要澄清的事也写进最终回复交给主对话，不直接问{title}。"
        );
    }
    let title = if user_title.is_empty() { "Boss" } else { user_title };
    format!(
        "Your role: sub-task (of the task whose main conversation is {parent}). Per \"Tasks, Sub-tasks And Who Reports\": \
         raise no card at all and never set `taskComplete`; your final reply is the result, which the main conversation \
         collects. Anything needing clarification goes in that reply for the main conversation to settle; do not ask {title} directly."
    )
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
    fn coordinator_section_covers_its_rules_in_both_locales() {
        for (locale, needles) in [
            ("zh", ["先发一张方案卡", "待澄清的问题", "sessions", "第 3 步", "哪怕是串行的", "先倒推", "同一个模型", "只问事实", "下一个未知点", "能观察到", "先证明不是实现坏了", "证据等级"]),
            ("en", ["first raise a proposal card", "open questions", "sessions", "step 3", "even when it is serial", "work backwards", "same model", "for facts, not a plan", "next unknown", "observable", "prove the implementation isn't what broke", "grade of evidence"]),
        ] {
            let s = render_section("老板", locale);
            for n in needles {
                assert!(s.contains(n), "{locale}: missing {n:?}");
            }
        }
    }

    /// The coordinator no longer carries the sub-task's rulebook, and the
    /// sub-task's section carries no card rules — each role reads only its own.
    #[test]
    fn subtask_section_is_separate_and_card_free() {
        for (locale, own, coordinator_only) in [
            ("zh", ["不发卡", "交回统筹", "最终回复就是交付物", "要附证据"], "先发一张方案卡"),
            ("en", ["No cards", "Hand clarifications back", "final reply is the deliverable", "needs evidence"], "first raise a proposal card"),
        ] {
            let sub = render_subtask_section("老板", locale);
            for n in own {
                assert!(sub.contains(n), "{locale}: missing {n:?}");
                assert!(!render_section("老板", locale).contains(n), "{locale}: coordinator has {n:?}");
            }
            assert!(!sub.contains(coordinator_only), "{locale}");
            assert!(!sub.contains("taskComplete") && !sub.contains("TTS"), "{locale}");
        }
    }

    /// A coordinator that reads "split off what can run in parallel" concludes
    /// that serial foundation work is its own, fills its context and relays
    /// before a single sub-task is spawned. Every carrier must say otherwise.
    #[test]
    fn coordinator_delegates_serial_heavy_work() {
        assert!(!render_section("老板", "zh").contains("值得并行拆出去"));
        assert!(!render_section("Boss", "en").contains("worth splitting off in parallel"));
        assert!(render_section_compact("Boss").contains("even when it is serial"));
    }

    fn status(id: &str, class: crate::subtask_recovery::RecoveryClass) -> crate::subtask_recovery::SubtaskStatus {
        crate::subtask_recovery::SubtaskStatus {
            session_id: id.to_string(),
            effective_id: id.to_string(),
            spawned_by: "coord".to_string(),
            via: None,
            step: Some("P2".to_string()),
            workspace: None,
            source: Some("claude".to_string()),
            phase: crate::session_probe::Phase::Failed,
            class,
            detail: "进程在回合中途退出".to_string(),
        }
    }

    const ALL_CLASSES: [crate::subtask_recovery::RecoveryClass; 10] = {
        use crate::subtask_recovery::RecoveryClass as C;
        [
            C::Recoverable,
            C::Done,
            C::Running,
            C::Blocked,
            C::Parked,
            C::UserInterrupted,
            C::AuthFailed,
            C::Failed,
            C::Closed,
            C::Unknown,
        ]
    };

    #[test]
    fn recovery_block_only_for_recoverable_auth_or_interrupted_children() {
        use crate::subtask_recovery::RecoveryClass as C;
        for class in ALL_CLASSES {
            let rows = vec![(status("aaaaaaaa-1111", class), None)];
            let wanted = matches!(class, C::Recoverable | C::AuthFailed | C::UserInterrupted);
            assert_eq!(needs_coordinator_attention(class), wanted, "{class:?}");
            for locale in ["zh", "en"] {
                assert_eq!(
                    recovery_block_for(&rows, "", locale).is_some(),
                    wanted,
                    "{class:?} {locale}"
                );
            }
        }
        // Done siblings never leak into a block that exists for another child.
        let rows = vec![
            (status("dddddddd-done", C::Done), None),
            (status("rrrrrrrr-rec", C::Recoverable), None),
        ];
        let block = recovery_block_for(&rows, "", "zh").unwrap();
        assert!(block.contains("`rrrrrrrr`") && !block.contains("dddddddd"), "{block}");
        assert!(recovery_block_for(&[], "", "zh").is_none());
    }

    #[test]
    fn recovery_block_says_what_to_do_per_class() {
        use crate::subtask_recovery::RecoveryClass as C;
        let rows = vec![
            (status("11111111-x", C::Recoverable), Some("修登录页".to_string())),
            (status("22222222-x", C::AuthFailed), None),
            (status("33333333-x", C::UserInterrupted), None),
        ];
        let zh = recovery_block_for(&rows, "", "zh").unwrap();
        for n in [
            "`11111111`「修登录页」 步骤 P2 · 可恢复：进程在回合中途退出",
            "`22222222` 步骤 P2 · 鉴权/账号失败",
            "`33333333` 步骤 P2 · 被手动打断",
            "fleet__control action=resume id=<id>",
            "先核对它已经做完了什么",
            "先修好凭证或问老板",
            "老板有意停下",
            "绝不唤醒",
            "fleet__spawn",
        ] {
            assert!(zh.contains(n), "zh missing {n:?}:\n{zh}");
        }
        let en = recovery_block_for(&rows, "Alex", "en").unwrap();
        for n in [
            "`11111111` \"修登录页\" step P2 · recoverable",
            "fleet__control action=resume id=<id>",
            "ask Alex first",
            "Alex stopped it on purpose",
            "never woken",
            "fleet__spawn",
        ] {
            assert!(en.contains(n), "en missing {n:?}:\n{en}");
        }
        assert!(!en.contains("老板") && !en.contains("Boss"), "{en}");
        // Only the instructions for the classes present are spelled out.
        let only = recovery_block_for(&rows[..1], "", "zh").unwrap();
        assert!(!only.contains("鉴权") && !only.contains("手动打断"), "{only}");
    }

    #[test]
    fn coordinator_guidance_covers_mid_turn_recovery() {
        for (locale, needles) in [
            ("zh", ["断网或服务中断后", "action=resume", "绝不唤醒", "重新派一次", "send 或 resume"]),
            ("en", ["network or service interruption", "action=resume", "never woken", "dispatch them again", "send or resume"]),
        ] {
            let s = render_section("老板", locale);
            for n in needles {
                assert!(s.contains(n), "{locale}: missing {n:?}");
            }
        }
        // The compact codex / dsh block has ~8 bytes of AGENTS.md headroom;
        // the how-to arrives with the per-turn block itself.
        assert!(render_section_compact("Boss").contains("not `send`/`resume` on the finished"));
    }
}
