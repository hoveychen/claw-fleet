//! PRD Discipline mode — injects a guidance block into `~/.claude/CLAUDE.md`
//! that locks down two failure modes the user kept hitting:
//!
//!   1. Mid-PRD commit nagging — the agent finishes P1/P2, gets a "should I
//!      commit now?" reflex, and the user has to keep saying "no, keep going."
//!   2. Post-compression task amnesia — after a context compression the agent
//!      remembers it just committed but forgets P3..Pn are still pending.
//!
//! The discipline rules live in this guidance file. The persistence half
//! (TASKS.md re-injection on every UserPromptSubmit) is implemented as a
//! Claude Code hook in `hooks::apply_user_prompt_submit_hook`.
//!
//! Install strategy mirrors `interaction_mode`:
//!   1. Render `~/.claude/fleet-prd-discipline.md`.
//!   2. Sentinel-wrap an `@import` in `~/.claude/CLAUDE.md`.

use std::fs;
use std::path::PathBuf;

const BEGIN_MARKER: &str = "<!-- fleet:prd-discipline:begin -->";
const END_MARKER: &str = "<!-- fleet:prd-discipline:end -->";

fn claude_dir() -> Option<PathBuf> {
    crate::session::get_claude_dir()
}

fn guidance_file_path() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("fleet-prd-discipline.md"))
}

fn claude_md_path() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("CLAUDE.md"))
}

/// Build the PRD-discipline guidance markdown.
///
/// Two halves:
/// - **Commit discipline** (the static rule): no proactive commits mid-PRD.
/// - **TASKS.md workflow** (paired with the UserPromptSubmit hook): how to
///   write and read the durable plan file so context compression can't erase
///   the macro state.
pub fn render_guidance(user_title: &str, locale: &str) -> String {
    let language_line = match locale {
        "zh" => "本规则配套的 TASKS.md 也用中文书写（task 标题、备注皆中文）。",
        "ja" => "本ルールに対応する TASKS.md も日本語で書いてください。",
        "ko" => "이 규칙과 짝을 이루는 TASKS.md도 한국어로 작성하세요.",
        _ => "Write the paired TASKS.md in English.",
    };

    if locale == "zh" {
        let title = if user_title.is_empty() {
            "老板".to_string()
        } else {
            user_title.to_string()
        };
        return format!(
            r##"# Fleet PRD 纪律 (managed by Claw Fleet — do not edit)

本模式锁死三个会拖垮长程多步计划的失败模式：计划中途的提交唠叨、压缩后的任务失忆、进度汇报式打卡。

**多步计划** = 你拆成 2 个或更多顺序子任务（P1..Pn，或编号 todo，或任何等价物）的任务。

## Rule 1 —— main 上的提交纪律（仅多步计划）

- 计划进行中，不要提议、也不要跑 main 上的 `git commit`。没有「自然检查点」，工作的单位是计划，不是单个 P-task。
- 例外只有两个：① {title}本回合明确要求；② 所有 P-task 已勾选 + 构建/测试已跑 + 已向{title}呈报摘要，此时唯一那次 main 提交就是 `git merge --no-ff prd/<id>`。
- worktree 分支 `prd/<id>` 上的**中间提交明确允许，不违反 Rule 1**，随便提，无需请示。
- **`git push` 永远需要{title}本回合的明确批准**，与计划状态无关。
- 撞上阻塞点就提问，不要拿「怕进度丢」当理由提交。破坏性操作（rebase、force-push、删分支、`git reset --hard`）先问。
- 单步任务（一个 bug 修复、一次重命名、一处配置微调）不是多步计划，适用常规提交礼仪。

## Rule 2 —— TASKS.md 是持久的宏观计划

上下文压缩会把宏观状态摘要掉，所以计划落在磁盘上。

- 拆成 2 个或更多子任务时，**开始 P1 之前**把拆解写进 `<workspace_root>/TASKS.md`。
- 每完成一个 P-task 把框改成 `[x]`。活跃计划每回合由 hook 自动重新注入。
- 格式（一个文件可并行承载多个计划，各自一对哨兵，`id` 唯一、kebab-case、≤32 字符）：

```markdown
<!-- fleet:prd:begin id="auth-refactor" v="2" -->

**Plan:** Migrate session middleware to the new auth crate

- [x] **P1** — Audit existing call sites
- [ ] **P2** — Swap middleware impl

<!-- fleet:prd:end id="auth-refactor" -->
```

- **只编辑你自己 id 的块**，其他块当只读（属于另一个可能正在推进的计划）。`fleet:prd:begin id=` 与 `fleet:prd:end id=` 的 id 必须匹配，不匹配会被 hook 忽略。不要合并或重排别人的计划。
- 旧版无 `id=` 的裸哨兵对仍被当作单个匿名计划识别，但不要再以这种形式新建。`fleet plan migrate` 可把 v1 就地升级到 v2。
- **同一个 `id` 只放在一个 TASKS.md 文件里。**hook 会合并扫描主 checkout 与所有 `.worktrees/*/TASKS.md`；重复 id 取 mtime 最新的那份，来自 worktree 的块标题会带 `— source: <path>` 后缀，告诉你该编辑哪个文件。
- {language_line} P-task 标题 ≤60 字符，长验收备注放子 bullet。只用 `- [ ]` / `- [x]`，不要发明新状态。
- 你在某 workspace **第一次**创建 TASKS.md 时，若 `.gitignore` 没覆盖它，向{title}提一句并提议加一行（它是临时草稿状态，不该进版本控制）；不要悄悄改写 `.gitignore`。

### 用 `fleet plan` 更新计划，而非手改

手改 TASKS.md 仍有效（文件是勾选框的唯一真相来源），但不记录归属，桌面端就显示不出你在做哪个计划。

> **工具列表里有 `fleet__plan` / `fleet__handoff` / `fleet__watch` / `fleet__loop` / `fleet__schedule` / `fleet__wiki` 这些 MCP 工具时（Fleet 起的会话都有），一律优先用它们而不是 `fleet …` 命令行（传 `action` 参数，语义与 CLI 子命令一一对应）。远端（rca）会话里 Bash 跑 `fleet …` 会被路由到没有 fleet 的远端而 exit 127。**

- `fleet plan create <id> --title "..." [--parent <id> | --root --root-reason "..."] [--kind explore|exec]` —— 新建计划块并把本会话记为执行者。创建即开始。
  - **默认：你在执行某计划时新建的计划自动成为它的子计划**，一个 flag 都不用传。从一个计划里派生出来的计划，默认就是它的儿子。`--parent` 挂到别处；`--root` 另起顶层树，**手上有计划时必须同时给 `--root-reason`**（一句话说明这活为什么不属于当前计划），否则被拒。手上没计划时 root 本来就是默认，什么都不用传。
- `fleet plan check <id> <P>` / `uncheck` —— 勾选/取消，并把焦点刷新到该计划。
- `fleet plan resume <id> [P]` —— 接手一个你没创建、也没被交接给你的现存计划。
- `fleet plan add <id> <P> --text "..."` / `migrate` / `list` / `get <id>`。

### explore 计划与 exec 计划

`--kind` 说明这个计划的 P-task 是干什么用的。`exec`（缺省）会改代码；`explore` 产出的是理解，它的交付物是**派生出的 exec 子计划**，不是自己的代码改动。

凡是以「先搞清楚……」开头、你还叫不出具体改动名字的工作都用 `--kind explore`，并且**不改生产代码**（一次性探针脚本可以）。调研完成后把结论变成一批 `--parent <explore-id>` 的子计划，让{title}在动手前逐条读到要做什么。把调研和实现塞进同一个计划，正是长程工作走歪的方式：P3 的发现会悄悄重新定义 P4 的含义，等有人察觉时实现已经和没人拍板过的需求耦合在一起了。

### 子计划与回溯

计划中途要分出一条必须先完成的旁支时，用 `--parent <current-plan-id>` 建成子计划。用 `fleet plan check` 勾掉子计划**最后**一个框时，Fleet 沿 `parent` 链走到最近的仍有待办的祖先，把你的焦点指回它并打印下一个要恢复的 P。照指令继续，不要因为子计划完成就结束回合。子计划可嵌套，向上走会跳过已完成的祖先。没有 `--parent` 的计划是顶层，完成它就结束。

## Rule 3 —— 基于 worktree 的特性工作流

Rule 3 是**全局**的：**任何触碰生产代码的改动都必须在隔离的 git worktree 里开发**，无论多步还是单步，不受 Rule 1「多步计划」这个限定影响。

```
git worktree add -b prd/<task-id> .worktrees/<task-id> main
```

`<task-id>` 多步计划用 TASKS.md 的计划 id，单步改动当场挑一个短 kebab-case 标识。所有代码工作在 worktree 里跑，主 checkout 全程保持干净。

- 结束时从主 checkout 一次原子合并：`git merge --no-ff prd/<task-id>`。`--no-ff` 强制；**禁止** `--ff-only` 和 `--squash`——我们要每个 worktree 提交在 main 历史里都可见。
- **合并或移除 worktree 前，抢救 gitignored / 未跟踪产物。** merge 只带走已提交内容，`git worktree remove` 会连同其余一起永久删除，没有 git 对象能恢复。先跑 `git status --ignored` 并检查未跟踪文件。`target/`、`node_modules/`、`dist/`、`.next/` 这类例行可再生的目录跳过。若有**不**能从已提交代码重现的产物，停下来问{title}（拷出去，还是该跟踪它），解决前不要 remove——移除是不可逆的那一步。
- 合并成功后：`git worktree remove .worktrees/<task-id>`，然后 `git branch -d prd/<task-id>`。合并失败就地解决——不要弃掉 worktree、不要 amend 合并提交、不要 `git reset --hard` 抹掉合并。
- 不要把 worktree 分支 push 到远端。
- 本 repo 第一次创建 worktree 时，若 `.gitignore` 没有 `.worktrees/`，向{title}提一句并提议加；不要悄悄改写。

**Rule 3 不适用于**：纯文档改动；纯配置改动（CI YAML、dotfile、`.gitignore` 本身、格式化器配置）；必须先落 main 的紧急热修（先向{title}呈报，好让{title}决定是否暂停活跃的 worktree）。

## Rule 4 —— 计划执行节奏

每个非最后的 P-task：**开发 → 测试/验证 → 在 worktree 内提交 → `fleet plan check <id> <P>` → 同一回合里立即做下一个**，不为确认停顿。

不要停下来做摘要，不要问「要我继续 P2 吗」「P4 前要不要审一下进度」。进度的单位是计划，不是 P-task；TASKS.md 和 worktree 提交已让进度一目了然。

接起一个你没创建、也没被交接的计划时，第一个 P-task 之前先 `fleet plan resume <plan-id> [P]`。`create` 与 Fleet 交接会自动归属你，`check` 随你推进而刷新。

两道机制在强制这个节奏。**聚焦注入**：你被归属到某计划后，每轮注入只展开你这一个，其余折叠成一行计数——摆在你面前的下一个任务在构造上只有一个。**计划门**：你在焦点计划（或它的祖先）仍有未完成 P-task 时试图结束回合，`Stop` 钩子会拒绝并点名下一个 P；它只在你本回合确实推进过计划时介入，并对每一个正当出口让路（已登记的 handoff、一个 watch、一张等答复的决策卡）。

**只为以下四种情形停顿**（「我做了不少，要不要报个到」永远不是其中之一）：

1. **最后一个 P-task 的验收闸门** —— 跑 `git merge --no-ff` 前呈报「可以合并了」并等明确放行。这次合并就是计划的验收时刻，不要在中间检查点征求验收。
2. **一个真正的方向问题** —— 路上有真岔口、需要{title}判断（「保持向后兼容还是丢掉？」「删还是归档？」「API 设计 A 还是 B？」）。
3. **挺过一轮修复的验证红灯** —— 构建/测试第一次失败可以试一轮诊断加修复；没恢复绿灯，或动手前根因就不清楚，停下来作为阻塞点呈报，不要陷入「修→重试→修→重试」循环。
4. **一次破坏性操作**（rebase、force-push、删分支、丢弃 migration、`git reset --hard`）。

## Rule 5 —— 长上下文交接与跨回合等待

### `fleet handoff`

上下文在计划中途拉长时，不要死磕到窗口耗尽、不要悄悄提前收尾、也不要留下没人执行的「交给下一个会话」的便条：

```
fleet handoff --note "<换班简报：什么做完了、什么在飞、关键文件、坑、下一个具体步骤>" [--goal <本链目标>] [--plan <plan-id>] [--next <P>] [--model <模型>] [--effort <档位>]
```

- `--note` 强制。`--plan/--next` 让 Fleet 把后继者自动归属到该计划和 P。`--model/--effort` 可选，不传就继承当前会话。
- **第一棒交接时用 `--goal` 写下这条链的目标**——一句话的「什么做完了这条链才算完」。你是在回合*末尾*登记的，所以哪怕这活是聊到一半才定下来的，此刻你也已经知道{title}要什么了。它会被每轮注入到后继者眼前，并成为**收工的判据**：计划树全勾了、只要 goal 没达成，就不该把决策卡标成 `taskComplete: true`。
- **目标变了是正常的**（{title}中途改路线、或原目标已不成立），改就是了——但要用 `--goal <新目标> --goal-reason <为什么>` 显式改，并在卡里告诉{title}。没有理由的改动会被拒：那条规则挡的不是「目标变了」，而是把链的目标**悄悄缩成你手上那个 plan**，然后宣布达成。纯探索、本来就没有终点的链，不写 goal 也完全可以。
- 登记后**干净地结束回合**（先按 Rule 3 提交 worktree 进度）。Stop hook 消费登记并 spawn 后继者，开场 prompt 就是你的便条。
- **叙述一次交接不等于登记一次。**在回复文本里写「接下来我起下一棒」什么都不做：没真的调用工具就没有后继者，计划在你交出的那一刻悄然死掉。结束这样一个回合前的最后一件事就是那个调用本身，等 `ok: handoff registered` 回来才停。
- **登记就是把便条定稿了，也是本回合最后一个动作。之后一张决策卡都不要再发**（连不带决策的收尾卡也不要）：接力靠回合*结束*触发，卡会把回合挂住等人点，后继者就起不来；卡上的答案也进不了已冻结的 note，会被静默丢弃。要问就先问、拿到答案、再按答案写 note 去登记。
- 收到 `[Fleet] 上下文已用 250K` 提示就该准备交接了——超过 250K 模型开始变钝。接力换回来的是一个清醒的头脑，不是一次损失。
- 整条链可读：`fleet__handoff` 传 `action="show"` 列出每一棒的 session id 与 note 全文。**{title}问「最开始的问题」指的是第 1 棒的起点，不是你手上的 plan**，先 `show` 再答。
- 你挂的 `fleet watch` 会跟着棒一起转给后继者（含条件、deadline 与 model/effort）。交接前不用停它；作为后继者读到「你继承了 watch X」时，也别再创建条件相同的第二个。

### 增量笔记：`fleet__notes` 与 `fleet__history`

交接是换人；本节管的是同一个会话跨上下文窗口。

- **边做边记，别等到最后。**从一开始就用 `fleet__notes`（CLI：`fleet notes`）维护一份 checkpoint（目标、已定决策、进展、教训、下一步，以及能回捞细节的指针），每完成一个 P-task 或撞上一个值得记的坑就 `append`。笔记不受压缩影响，handoff 后继者也读得到。
- 压缩后新窗口开头会注入 `<fleet_notes>`：先读它恢复宏观状态；缺细节就用 `fleet__history search`（CLI：`fleet history`）搜自己（和前任）transcript 里的原话，拿到 `line_no` 后 `read` 那一条。
- 它们是**内部记账**，不要在给{title}的回复里复述笔记或提这两个工具。

### 绝不用 Claude Code 自带的跨回合调度器

**NEVER 调用 `ScheduleWakeup` 或 `CronCreate`，也不要用 `/loop` 斜杠命令。**在 Fleet 会话里它们全是空转：回合就此结束，没有登记、没有后继者、计划死在原地，而工具还返回一个像是成功的结果。这**与你上下文剩多少无关**——等后台任务、等构建、想稍后再看一眼，全都算。

**按需求挑 Fleet 的机制：**

- **周期性重复跑一件事（cron 语义）→ `fleet loop`**（CLI 别名 `fleet cron`）。Fleet 托管、durable，每个 interval spawn 一个全新的本地 detached 会话，不随本会话消亡。
- **未来某个绝对时刻只跑一次 → `fleet schedule`**（`--at` / `--in`）。
- **等一个外部条件满足后继续*本*会话 → `fleet watch`**：`fleet watch create --until '<完成时退出 0 的命令>' --capture '<其 stdout 你想被报告的命令>' --note '<你在等什么>'`，然后结束回合；条件触发时 Fleet 会 `claude --resume` 这个会话，把捕获的结果喂给你的下一回合。`fleet watch stop <id>` 取消。
- **把工作交给全新后继者 → `fleet handoff`**。

`fleet loop` / `fleet schedule` 创建时**务必给 `--title <几个字>`**，否则计划任务列表只显示 prompt 的头两行。两者的可选 `--until <shell 命令>` 是廉价的非 LLM 门：每个 tick 先跑这条便宜探测，只有它退出 0 才 spawn 会花钱的 LLM 会话。别默认每个 tick 都起一个 LLM 会话。

### 绝不用空转命令保活回合

**别为了撑住回合发什么都不做的命令**——`echo waiting`、`true`、`:`、裸 `sleep 30`，以及它们用 `;` / `&&` 串起来的组合。一次空转不比一次真工作便宜：你每个回合都要重读整个上下文。按你在等什么挑：

- **能前台跑的命令**（编译、测试、脚本）→ 直接前台跑，把 Bash 的 `timeout` 调大（上限 600000 毫秒），一次调用等到底。
- **已经在跑的条件** → `Monitor` 的 until 轮询（回合内阻塞，轮询本身不花 round trip）。
- **跨回合的事**（CI、构建产物、部署上线）→ `fleet watch`，然后干净地结束回合。
- **真的无事可等** → 直接结束回合。

`sleep 45; <真正的检查命令>` **不**算空转——一次 round trip 换一次真观察，随便用。被禁的只有零信息量的那种。

## Rule 6 —— 需求保真：别把不存在的需求写进计划

长程计划最贵的失败不是做得慢，而是做歪：计划里混进了{title}从没要求的需求，实现又和这些幻觉需求强耦合。

- **计划里每一条 P-task、每一个需求，都必须能追溯到{title}本回合实际说过的话，或由它直接推导出的必要项。**把每条默默分成三类：明说的、由明说项推导的、你自己加的。凡是「你自己加的」（「顺手抽象一层」「为了将来好扩展」「这类功能一般还得有 X」），要么删掉，要么单独拎出来问{title}一句。**绝不静默写进计划，没有无源头的需求。**
- **「该写个 RFC / 设计文档 / 要签字过一版」这个冲动是信号，但它指向的不是「停下」，而是「先做一次范围审计」**——RFC 奖励穷尽，而对你来说穷尽就等于编造。
- **先做能跑通的最薄一条竖切，跑通了再加。**在出现第二个具体用例逼你之前，不要为想象中的需求建抽象层、配置面或插件点——幻觉需求往往是架构性的，一旦变成承重墙就拆不动了。
- 需要设计文档不是罪；把它当成「已批准的需求合同」逐字实现才是。审阅时盘的是那张标注了 明说／推导／我加的 需求清单，不是那段读起来很合理的散文。

本规则无论多步还是单步都适用。

## worktree 工作流的推荐工具

Rule 3 给每个计划一个干净的 checkout，所以按项目存包的工具会为每个 worktree 重装一遍。这些是推荐，不是硬规则；{title}为某项目明确挑了别的工具就照那个来。

- **Node / TypeScript**：优先 **pnpm**（全局 store + symlink）或 bun，别用 npm / yarn classic。
- **Python**：优先 **uv**（全局缓存 + 硬链接 venv），而非 `pip + venv`。
- **Rust**：`cargo` 已全局共享 `~/.cargo/registry`，无需动作。`target/` 按设计 per-worktree，别试图共享。
- **Go**：已全局共享 `$GOMODCACHE` 和 `$GOCACHE`，无需动作。

**不要只因为要创建 worktree 就悄悄迁移已有项目的 lockfile 或包管理器**——一个 `package-lock.json` 的 repo 在{title}同意切换之前一直留在 npm。切换包管理器是一个独立计划，动 lockfile 之前先呈报取舍。

## 本模式何时不适用

Rule 3 对任何生产代码改动都是全局的；Rule 1、2、4 只限多步计划。

- **单步生产代码改动**：只走 Rule 3（worktree + `--no-ff` 合并），无 TASKS.md、无 P-task、无节奏强制。
- **纯对话 / 问答回合**、**纯文档 / 配置 / 热修工作**、**{title}明确要求「非正式」或「快点」**：四条规则全部关闭，适用常规提交礼仪。

## 与其他模式的交互

本模式**独立于** Fleet 交互模式，可分别启用。Bash guard hook（若已安装）仍会运行，仍可能要{title}确认有风险的命令——guard 抓风险，本模式抓*不必要*的提交。
"##,
            title = title,
            language_line = language_line,
        );
    }

    let title = if user_title.is_empty() {
        "Boss".to_string()
    } else {
        user_title.to_string()
    };
    format!(
        r##"# Fleet PRD Discipline (managed by Claw Fleet — do not edit)

This mode locks down three failure modes that hurt long multi-step plans: **Mid-plan commit nagging**, **Post-compression task amnesia**, and **Progress-report checkpointing**.

A **multi-step plan** = any task you decomposed into 2 or more sequential subtasks (P1..Pn, numbered todos, or any equivalent).

## Rule 1 — Commit discipline on main (multi-step plans only)

**Scope of "commit" in this rule.** Throughout Rule 1, "commit" means the **main / default branch**. Commits on a worktree branch (`prd/<plan-id>`) are governed by Rule 3, are explicitly allowed at every P-task boundary, and are not a Rule 1 violation.

- While a plan is in flight, do not propose `git commit` on main. Not after P1, not after P2, not at any "natural checkpoint" — the unit of work is the plan, not one P-task.
- Do not actually run `git commit` on main either. You may commit on main only when: ① {title} explicitly asks for it in the current turn; or ② every P-task is checked, build/tests have run, and you have surfaced a summary to {title} — and that single allowed commit takes the form `git merge --no-ff prd/<plan-id>`.
- **Intermediate commits** on the worktree branch `prd/<plan-id>` are explicitly allowed and do NOT violate Rule 1 — they are governed by Rule 3. Commit freely there without asking.
- **`git push` is always gated** — never push without {title}'s explicit approval in the current turn, regardless of plan state.
- Hit a blocker? Ask. Do not use "I'm afraid of losing progress" as a reason to commit. Destructive operations (rebase, force-push, deleting branches, `git reset --hard`) always stop and ask first.
- Single-step tasks (one bug fix, one rename, one config tweak) are not multi-step plans; normal commit etiquette applies.

## Rule 2 — TASKS.md is the durable macro plan

Context compression summarizes the macro state away, so the plan lives on disk.

- When you decompose into 2 or more subtasks, write the breakdown into `<workspace_root>/TASKS.md` **before** starting P1.
- Tick each finished P-task to `[x]`. Active plans are re-injected every turn by Fleet's UserPromptSubmit hook.
- Format (one file can host several plans in parallel, each in its own sentinel pair, `id` unique, kebab-case, ≤ 32 chars):

```markdown
<!-- fleet:prd:begin id="auth-refactor" v="2" -->

**Plan:** Migrate session middleware to the new auth crate

- [x] **P1** — Audit existing call sites
- [ ] **P2** — Swap middleware impl

<!-- fleet:prd:end id="auth-refactor" -->
```

- **Only edit your own** `id`'s block; treat every other block as read-only (it belongs to another plan that may be in flight). The ids on `fleet:prd:begin id=` and `fleet:prd:end id=` must match, or the hook ignores the block. Never merge or reorder someone else's plan.
- A bare legacy sentinel pair with no `id=` is still recognized as one anonymous plan, but do not create new ones that way. `fleet plan migrate` upgrades a v1 file in place.
- **Multi-source scan across worktrees:** the hook scans the main checkout's TASKS.md plus every `.worktrees/*/TASKS.md`. When the same `id` appears in more than one file the most recent by mtime wins, and a block from a worktree renders with a `— source: <path>` suffix so you know which file to edit. So keep a given `id` in exactly one TASKS.md file.
- {language_line} Keep P-task titles ≤ 60 chars; long acceptance notes go in sub-bullets. Use only `- [ ]` and `- [x]`; do not invent new states.
- The **first** time you create TASKS.md in a workspace, check whether `.gitignore` covers it; if not, mention it to {title} and offer to add the line (it is scratch state — don't put this in version control). Never rewrite `.gitignore` silently.

### Update plans with `fleet plan`, not by hand

Hand-editing TASKS.md still works (the file is the source of truth for checkboxes) but records no attribution, so the desktop cannot tell which plan your session is on.

> **When the MCP tools `fleet__plan` / `fleet__handoff` / `fleet__watch` / `fleet__loop` / `fleet__schedule` / `fleet__wiki` are in your tool list (every Fleet-spawned session has them), always prefer them over the `fleet …` CLI (pass `action`, one-to-one with the CLI subcommands). In a remote (rca) session, running `fleet …` through Bash is routed to a remote executor that has no fleet and fails with exit 127.**

- `fleet plan create <id> --title "..." [--parent <id> | --root --root-reason "..."] [--kind explore|exec]` — adds a plan block and records this session as its executor. Creating a plan is starting it.
  - **Default: a plan you author while executing another plan automatically becomes that plan's child.** No flag needed — a plan spawned out of a plan is by default its son. `--parent` attaches it elsewhere; `--root` starts a new top-level tree and **requires `--root-reason` whenever you are on a plan** (one sentence on why this work does not belong to the current plan), otherwise it is rejected. With no plan in flight, root is the default anyway and you pass nothing.
- `fleet plan check <id> <P>` / `uncheck` — tick or untick, and refresh your focus onto that plan.
- `fleet plan resume <id> [P]` — take over an existing plan you neither created nor were handed off into.
- `fleet plan add <id> <P> --text "..."` / `migrate` / `list` / `get <id>`.

### Explore plans vs exec plans

`--kind` says what a plan's P-tasks are *for*. `exec` (the default) changes code. `explore` produces understanding, and its deliverable is **the exec child plans it spawns**, not code changes of its own.

Any work that starts with "first figure out …", where you cannot yet name the concrete change, uses `--kind explore` and **does not touch production code** (throwaway probe scripts are fine). When the investigation lands, turn the findings into a batch of `--parent <explore-id>` children so {title} can read what is to be done before anyone builds it. Putting investigation and implementation in one plan is exactly how long-range work goes wrong: P3's findings quietly redefine what P4 means, and by the time anyone notices, the implementation is coupled to a requirement nobody signed off on.

### Sub-plans and backtracking

When a plan needs a side branch that must land first, create it with `--parent <current-plan-id>`. When `fleet plan check` ticks the **last** box of a sub-plan, Fleet walks the `parent` chain to the nearest ancestor that still has unchecked P-tasks, points your focus back at it and prints the next P to resume. Follow that instruction — do not end the turn just because the sub-plan finished. Sub-plans nest, and walking up skips completed ancestors. A plan with no `--parent` is top-level; finishing it ends the work.

## Rule 3 — Worktree-based feature workflow

Rule 3 is global — it covers **every change that touches production code, developed in an isolated git worktree**, whether multi-step or single-step, and it is not narrowed by Rule 1's multi-step framing.

```
git worktree add -b prd/<task-id> .worktrees/<task-id> main
```

For a multi-step plan `<task-id>` is the TASKS.md plan id; for a single mechanical change, pick a short kebab-case id on the spot. All code work happens in the worktree; the main checkout stays clean throughout.

- **Commit inside the worktree** between P-tasks — those commits are progress markers nobody else sees, and they need no approval.
- Finish with one atomic merge from the main checkout: `git merge --no-ff prd/<task-id>`. `--no-ff` is mandatory; `--ff-only` and `--squash` are **forbidden** — every worktree commit stays visible in main's history.
- **Before merging or removing a worktree, rescue gitignored / untracked artifacts.** The merge **only carries across** committed content; `git worktree remove` then deletes the rest permanently, and since they were never tracked there is **no git object to recover** them. `.gitignore` means "don't put this in version control", not "don't keep this" — a generated dataset, a downloaded asset, a `.env` is real data even when untracked. Run `git status --ignored` and check untracked files first. Routinely regenerable directories — `target/`, `node_modules/`, `dist/`, `.next/` — can be skipped. If the worktree holds an artifact that cannot be reproduced from committed code, stop and ask {title} (copy it out, or should it be tracked?) before removing anything — removal is the irreversible step.
- After a successful merge: `git worktree remove .worktrees/<task-id>`, then `git branch -d prd/<task-id>`. If the merge fails, resolve it in place — do not abandon the worktree, do not amend the merge commit, do not `git reset --hard` the merge away.
- Do not push the worktree branch to a remote.
- The first time you create a worktree in a repo, check `.gitignore`; if `.worktrees/` is absent, mention it to {title} and offer to add it. Never rewrite `.gitignore` silently.

### When Rule 3 does NOT apply

Rule 3 covers any change touching production code, **whether multi-step or single-step**. The exemptions are about *what* you change, not how many steps it takes:

- documentation-only changes; - configuration-only changes (CI YAML, dotfiles, `.gitignore` itself, formatter config);
- an urgent hotfix that must land on main before an in-flight worktree finishes (surface the hotfix to {title} first, so they can decide whether to pause the active worktree).

## Rule 4 — Plan execution rhythm

Every non-final P-task: **Dev** → **Test / verify** → **Commit inside the worktree** → `fleet plan check <id> <P>` → move straight to the next P-task **in the same turn**, without pausing for confirmation.

Do not stop to summarize. Do not ask "shall I continue with P2?" or "should I review progress before P4?". Do not offer "I've written quite a few P-tasks, want a summary?". Progress is measured in plans, not P-tasks, and TASKS.md plus the worktree commits already make it legible.

When you pick up a plan you neither created nor were handed off into, run `fleet plan resume <plan-id> [P]` before your first P-task. `create` and a Fleet handoff attribute you automatically; `check` refreshes it as you advance.

Two mechanisms now enforce this rhythm. **Focused injection**: once you are attributed to a plan, each turn's injection expands only that one, collapsing the rest to a one-line count — so there is structurally only one next task in front of you. **The plan gate**: if you try to end a turn while your focus plan (or one of its ancestors) still has unchecked P-tasks, the `Stop` hook refuses and hands back an instruction naming the next P. It only fires when you actually advanced a plan this turn, and it yields to every legitimate exit (a registered handoff, a `fleet watch`, a decision card awaiting an answer).

**The rhythm stops for exactly four things** ("I've made a lot of progress, should I check in?" is never one of them):

1. **The final P-task's acceptance gate** — before running `git merge --no-ff`, surface a "ready to merge" summary and wait for explicit clearance. That merge is the plan's acceptance moment; do not solicit acceptance at intermediate checkpoints.
2. **A genuine question about direction** — a real fork in the road that needs {title}'s judgement ("keep backwards compatibility or drop it?", "delete or archive?", "API design A or B?").
3. **A red build/test that survived one round of fixes** — you may try ONE round of diagnosis and repair; if that round does not restore green, or the root cause was unclear before you started, stop and surface it as a blocker instead of looping fix → retry → fix → retry.
4. **A destructive operation** (rebase, force-push, deleting a branch, dropping a migration, `git reset --hard`).

## Rule 5 — Long-context handoff and cross-turn waiting

### `fleet handoff`

When your context window grows long mid-plan, do not grind it to exhaustion, do not quietly wrap up early, and do not leave a "for the next session" note nobody will execute:

```
fleet handoff --note "<shift briefing: what is done, what is in flight, key files, traps, the next concrete step>" [--goal <chain goal>] [--plan <plan-id>] [--next <P>] [--model <model>] [--effort <tier>]
```

- `--note` is mandatory. `--plan/--next` let Fleet attribute the successor to that plan and P. `--model/--effort` are optional and otherwise inherited.
- **On the first baton, state the chain's goal with `--goal`** — one sentence of "this chain is done when …". You register at the *end* of your turn, so even when the work only took shape mid-conversation you already know what {title} settled on. It is injected in front of every later baton and becomes **the test for finishing**: with the goal unmet, a fully ticked plan tree is still not grounds for `taskComplete: true`.
- **A goal changing is normal** ({title} changes course, or the original no longer holds) — just change it explicitly with `--goal <new> --goal-reason <why>`, and tell {title} on a card. A change with no reason is refused: the rule does not forbid the goal moving, it forbids **quietly shrinking it down to the plan in your hands** and then declaring victory. A purely exploratory chain with no finish line can leave the goal unset.
- Then **end the turn cleanly** (commit worktree progress first, per Rule 3). The Stop hook consumes the registration and spawns a successor whose opening prompt is your note.
- **Narrating a handoff is not registering one.** Writing "I'll start the next baton" in your reply does nothing: with no actual tool call there is no successor and the plan dies the moment you stop. So the last thing you do in such a turn is that call itself — wait for `ok: handoff registered` before stopping.
- **Registering freezes the note, and it is the last action of the turn. Afterwards raise **no decision card at all** — not even a decision-free closing card. Here is why: the relay fires when the turn *ends*; a card holds the turn open waiting to be clicked, so the successor never starts, and an answer on that card cannot reach the already-frozen note — it is silently dropped. Ask first, get the answer, then write the note and register.
- Once you see `[Fleet] context used 250K`, start preparing a handoff — past 250K the model dulls. A fresh head is what you get back, not a loss.
- The whole chain is readable: `fleet__handoff` with `action="show"` lists every baton's session id and full note. **When {title} asks about "the original question", they mean baton 1's starting point, not the plan in your hands** — run `show` before answering.
- A `fleet watch` you armed **moves to the successor with the baton** (conditions, deadline, model/effort included). Do not stop it before handing off; and if you are the successor reading "you inherited watch X", do not create a second one with the same condition.

### Incremental notes: `fleet__notes` and `fleet__history`

A handoff changes *who*; this section covers one session crossing context windows.

- **Take notes as you go**, not at the end. From the start, keep a checkpoint with `fleet__notes` (CLI: `fleet notes`) (goal, decisions taken, progress, lessons, next steps, and pointers for recovering detail), appending after each P-task or each trap worth recording. Notes survive compression, and a handoff successor can read them.
- After a compression the new window opens with a `<fleet_notes>` injection: read it to restore the macro state, then use `fleet__history search` (CLI: `fleet history`) to find the verbatim text in your (or a predecessor's) transcript and `read` that `line_no` for the details.
- These are **internal bookkeeping** — do not narrate the notes or these tools back to {title}.

### Never use Claude Code's built-in cross-turn schedulers

**NEVER call `ScheduleWakeup` or `CronCreate`, and do not use the `/loop` slash command.** In a Fleet session they all spin: the turn ends, nothing is registered, no successor is spawned, the plan dies in place — and the tool still returns something that looks like success. This holds **regardless of how much context you have left** — waiting on a background task, waiting on a build, wanting to look again later, all of it.

**Pick the Fleet mechanism by what you need:**

- **Run something repeatedly on an interval (cron semantics) → `fleet loop`** (CLI alias `fleet cron`). Fleet-managed and durable; each interval spawns a fresh local detached session that outlives this one.
- **Run once at an absolute future time → `fleet schedule`** (`--at` / `--in`).
- **Wait for an external condition and then continue *this* session → `fleet watch`**: `fleet watch create --until '<command that exits 0 when done>' --capture '<command whose stdout you want reported>' --note '<what you are waiting for>'`, then end the turn. Fleet polls in the background and `claude --resume`s this session with the captured result. `fleet watch stop <id>` cancels it.
- **Hand the work to a fresh successor → `fleet handoff`**.

Always pass `--title <a few words>` when creating a `fleet loop` or `fleet schedule`, or the scheduled-task list can only show the prompt's first two lines. Both also take an optional `--until <shell command>` as a cheap non-LLM gate: each tick runs that cheap probe first and only spawns the expensive LLM session when it exits 0. Do not default to spawning an LLM session every tick.

### Never spin a no-op command to hold the turn open

**Do not send a command that does nothing just to keep the turn alive** — `echo waiting`, `true`, `:`, a bare `sleep 30`, and any of them chained with `;` or `&&`. A spin is not cheaper than real work: you re-read the whole context every turn. Pick by what you are waiting for:

- **A command you can run in the foreground** (a compile, a test, a script) → just run it, raising Bash's `timeout` (up to 600000 ms), and wait it out in one call.
- **A condition already running** → `Monitor`'s until-polling, which blocks *inside* the turn and costs no extra round trip.
- **Something that spans turns** (CI, build artifacts, a deploy) → `fleet watch`, then end the turn cleanly.
- **Nothing to wait for** → just end the turn.

`sleep 45; <the real check command>` is **not** a spin — one round trip buys one real observation. Only the zero-information kind is banned.

## Rule 6 — Requirement fidelity: never write a requirement nobody asked for

The most expensive failure in a long plan is not slowness, it is building the wrong thing: requirements {title} never asked for slip into the plan, and the implementation couples itself to those hallucinated requirements.

- **Every P-task and every requirement in the plan must be traceable to something {title} actually said this turn, or a necessary consequence of it.** Silently sort each one into three buckets: stated, derived from a stated one, and added by you. Anything in the third bucket ("let me abstract this while I'm here", "so it's extensible later", "features like this usually also need X") is either dropped or raised to {title} as its own question. **No silent scope — there is no requirement without a source.**
- **The urge to "write an RFC / a design doc / get a version signed off" is a signal**, but it points at a scope audit, not at stopping. An RFC rewards exhaustiveness, and for you exhaustiveness means invention.
- **Ship the thinnest vertical slice that runs**, then add. Do not build an abstraction layer, a config surface or a plugin point for an imagined requirement until a second concrete use case forces it — hallucinated requirements are usually architectural, and once one is load-bearing you cannot take it out.
- Needing a design doc is not a sin; treating it as an approved requirements contract and implementing it verbatim is. Review the enumerable requirement list (each marked stated / derived / mine), not the prose that merely reads well.

This rule applies whether multi-step or single-step.

## Recommended tooling for the worktree workflow

Rule 3 gives each plan a clean checkout, so per-project package stores get reinstalled for every worktree. These are **recommendations**, not a Rule — if {title} explicitly picked another tool for a project, use that one.

- **Node / TypeScript**: prefer **pnpm** (global store + symlinks) or bun; avoid npm and yarn classic.
- **Python**: prefer **uv** (global cache + hardlinked venvs) over `pip` + venv.
- **Rust**: `cargo` already shares `~/.cargo/registry` globally — nothing to do. `target/` stays per-worktree by design; do not try to share it.
- **Go**: `$GOMODCACHE` and `$GOCACHE` are already global — nothing to do.

Creating a worktree is **not** a reason to change how an existing project installs its dependencies — **do not silently migrate** its lockfile or package manager — a repo with `package-lock.json` stays on npm until {title} agrees to switch. Switching package managers is its own plan; surface the cost/benefit before touching a lockfile.

## When this mode does NOT apply

**Rule 3 is global** for any production-code change; Rules 1, 2, 4 do NOT apply outside multi-step plans.

- **Single-step production-code change**: Rule 3 only (worktree + `--no-ff` merge) — no TASKS.md, no P-tasks, no rhythm enforcement.
- **Pure conversation / Q&A turns**, **documentation-, configuration- or hotfix-only work**, and **{title} explicitly asking to keep it informal or quick**: all four rules are off; normal commit etiquette applies.

## Interaction with other modes

This mode is **independent of** Fleet Interaction Mode; they can be enabled separately. The Bash guard hook (if installed) still runs and may still ask {title} to confirm risky commands — guard catches risk, this mode catches *unnecessary* commits.
"##,
        title = title,
        language_line = language_line,
    )
}

/// Apply PRD-discipline mode: write the guidance file and inject the
/// `@import` sentinel block into `~/.claude/CLAUDE.md`. Idempotent.
pub fn apply_prd_discipline(user_title: &str, locale: &str) -> Result<(), String> {
    crate::control_plane_prefs::note_intent(
        apply_prd_discipline_inner(user_title, locale),
        crate::control_plane_prefs::Feature::PrdDiscipline,
        false,
    )
}

fn apply_prd_discipline_inner(user_title: &str, locale: &str) -> Result<(), String> {
    let dir = claude_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create ~/.claude: {e}"))?;

    let guidance_path = guidance_file_path().ok_or("cannot determine home dir")?;
    let guidance = render_guidance(user_title, locale);
    fs::write(&guidance_path, guidance).map_err(|e| format!("write guidance file: {e}"))?;

    // Locked read-modify-write — see `claude_md_lock`.
    let claude_md = claude_md_path().ok_or("cannot determine home dir")?;
    let block = format!(
        "{begin}\n@{path}\n{end}\n",
        begin = BEGIN_MARKER,
        end = END_MARKER,
        path = guidance_path.display(),
    );
    crate::claude_md_lock::with_lock(&claude_md, || {
        let existing = fs::read_to_string(&claude_md).unwrap_or_default();
        let new_content = compose_claude_md(&existing, &block);
        crate::atomic_json::write_atomic(&claude_md, new_content.as_bytes()).map_err(|e| format!("write CLAUDE.md: {e}"))
    })?;
    Ok(())
}

/// Thin wrapper over [`crate::claude_md_block::compose`] — see there for why
/// the blank-line accounting is what it is.
fn compose_claude_md(existing: &str, block: &str) -> String {
    crate::claude_md_block::compose(existing, block, BEGIN_MARKER, END_MARKER)
}

/// Remove PRD-discipline mode: strip the sentinel block and delete the
/// guidance file. Idempotent.
pub fn remove_prd_discipline() -> Result<(), String> {
    crate::control_plane_prefs::note_intent(
        remove_prd_discipline_inner(),
        crate::control_plane_prefs::Feature::PrdDiscipline,
        true,
    )
}

fn remove_prd_discipline_inner() -> Result<(), String> {
    if let Some(claude_md) = claude_md_path() {
        crate::claude_md_lock::with_lock(&claude_md, || {
            if let Ok(existing) = fs::read_to_string(&claude_md) {
                let stripped = strip_sentinel_block(&existing);
                if stripped != existing {
                    crate::atomic_json::write_atomic(&claude_md, stripped.as_bytes()).map_err(|e| format!("write CLAUDE.md: {e}"))?;
                }
            }
            Ok::<(), String>(())
        })?;
    }
    if let Some(path) = guidance_file_path() {
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("remove guidance file: {e}"))?;
        }
    }
    Ok(())
}

/// Whether the guidance file on disk needs rewriting with what this build
/// renders. True when it is missing, and when its text drifted while staying
/// the *same* locale variant.
///
/// The sentinel block in `CLAUDE.md` says the feature is *installed*; it says
/// nothing about the *wording* of the file it points at. A Fleet upgrade that
/// edits the guidance text therefore reached no existing host, because the
/// appliers only run on install/toggle. This is what lets `heal` notice.
///
/// The first-line guard is why a drifted locale is not "stale": `fleet serve`
/// resolves its locale from `FLEET_LOCALE`, which a hand-run one on a desktop
/// host does not have, so an exact-match check would let it rewrite the user's
/// Chinese guidance in English on every start.
pub fn guidance_file_is_stale(user_title: &str, locale: &str) -> bool {
    let Some(path) = guidance_file_path() else {
        return false;
    };
    let Ok(on_disk) = fs::read_to_string(&path) else {
        return true; // missing or unreadable — rewrite it
    };
    let fresh = render_guidance(user_title, locale);
    on_disk.lines().next() == fresh.lines().next() && on_disk != fresh
}

/// Whether the sentinel block is present in `~/.claude/CLAUDE.md`.
pub fn is_prd_discipline_installed() -> bool {
    let Some(claude_md) = claude_md_path() else {
        return false;
    };
    let Ok(content) = fs::read_to_string(&claude_md) else {
        return false;
    };
    content.contains(BEGIN_MARKER) && content.contains(END_MARKER)
}

/// Thin wrapper over [`crate::claude_md_block::strip`] — the markers are this
/// module's, the blank-line accounting is shared.
fn strip_sentinel_block(content: &str) -> String {
    crate::claude_md_block::strip(content, BEGIN_MARKER, END_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_claude_md_is_idempotent() {
        let block = format!("{BEGIN_MARKER}\n@~/.claude/fleet-prd-discipline.md\n{END_MARKER}\n");
        // Existing doc that already carries the block after a blank line — the
        // real-world shape (another managed block above it).
        let existing = format!("user stuff\n\nother block end\n\n{block}");
        let once = compose_claude_md(&existing, &block);
        let twice = compose_claude_md(&once, &block);
        assert_eq!(once, twice, "composing twice must not accumulate blank lines");
        // Exactly one blank line between prior content and the block.
        assert!(once.contains("other block end\n\n<!--"), "one blank-line separator: {once:?}");
        assert!(!once.contains("\n\n\n"), "no triple newline: {once:?}");
    }

    #[test]
    fn strip_removes_block_preserves_rest() {
        let input = format!(
            "user content above\n\n{BEGIN_MARKER}\n@~/.claude/fleet-prd-discipline.md\n{END_MARKER}\n\nuser content below\n",
        );
        let out = strip_sentinel_block(&input);
        assert!(!out.contains(BEGIN_MARKER));
        assert!(!out.contains(END_MARKER));
        assert!(out.contains("user content above"));
        assert!(out.contains("user content below"));
    }

    #[test]
    fn strip_noop_when_absent() {
        let input = "plain content\nno markers here\n";
        assert_eq!(strip_sentinel_block(input), input);
    }

    #[test]
    fn render_uses_title_and_locale() {
        let g = render_guidance("师父", "zh");
        assert!(g.contains("师父"));
        assert!(g.contains("中文书写"));
        let g2 = render_guidance("", "en");
        assert!(g2.contains("Boss"));
        assert!(g2.contains("English"));
    }

    #[test]
    fn render_carries_both_rules() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains("Rule 1") && g.contains("Commit discipline"),
            "guidance must include the commit discipline rule"
        );
        assert!(
            g.contains("Rule 2") && g.contains("TASKS.md"),
            "guidance must include the TASKS.md persistence rule"
        );
        assert!(
            g.contains("UserPromptSubmit"),
            "guidance must mention the hook so the agent knows where the auto-injection comes from"
        );
    }

    /// Two failure modes seen on one relay (mslug3 chain, 2026-09-14): hop 73
    /// registered its handoff and *then* asked the boss which side the next hop
    /// should prioritise — an answer that can no longer reach the frozen note —
    /// while hop 72 had told its successor to re-arm a watch, leaving two
    /// watches for one condition. Rule 5 has to say both out loud, in both
    /// locales, or the next hop repeats them.
    #[test]
    fn rule_5_freezes_the_note_and_carries_the_watch_in_both_locales() {
        for locale in ["zh", "en"] {
            let g = render_guidance("Boss", locale);
            assert!(
                g.contains("登记就是把便条定稿了") || g.contains("Registering freezes the note"),
                "[{locale}] Rule 5 must say registering freezes the note"
            );
            // The ban is now total — a decision-free wrap-up card is barred too,
            // because the card is what keeps the Stop hook (and hence the
            // successor) from firing, not just a place to lose an answer.
            assert!(
                g.contains("一张决策卡都不要再发") || g.contains("raise **no decision card at all**"),
                "[{locale}] it must ban every post-register card, wrap-up included"
            );
            assert!(
                g.contains("接力靠回合*结束*触发") || g.contains("the relay fires when the turn *ends*"),
                "[{locale}] it must say why: a card holds the turn open, stranding the successor"
            );
            assert!(
                g.contains("跟着棒一起转给后继者")
                    || g.contains("moves to the successor with the baton"),
                "[{locale}] it must say a watch transfers with the baton"
            );
        }
    }

    #[test]
    fn scheduler_ban_is_its_own_section_in_both_locales() {
        // The ban used to be one subordinate clause inside Rule 5's
        // "long-context handoff" prose. An agent waiting on a background task
        // doesn't self-label as "my context is long", so it never looked there
        // and called ScheduleWakeup anyway (session f5c27989). It now needs its
        // own heading, next to the relay-picking table.
        for locale in ["zh", "en"] {
            let g = render_guidance("Boss", locale);
            assert!(
                g.contains("### 绝不用 Claude Code 自带的跨回合调度器")
                    || g.contains("### Never use Claude Code's built-in cross-turn schedulers"),
                "[{locale}] the ban needs its own heading, not a clause in Rule 5's prose"
            );
            for tool in ["ScheduleWakeup", "CronCreate"] {
                assert!(g.contains(tool), "[{locale}] ban must name {tool}");
            }
            // The section is only useful if the replacements sit next to it.
            for relay in ["fleet watch", "fleet handoff", "fleet loop"] {
                assert!(g.contains(relay), "[{locale}] guidance must name {relay}");
            }
            // Scope must be explicit: the failure mode was an agent reading the
            // ban as handoff-only advice.
            assert!(
                g.contains("无关") || g.contains("regardless of how much context"),
                "[{locale}] ban must say it applies regardless of remaining context"
            );
        }
    }

    /// Notes are the layer between TASKS.md (checkboxes) and a handoff note
    /// (one-shot briefing): same session, across context windows. The section
    /// has to teach the three moves — append as you go, read the injected hint
    /// after a compaction, recover detail via history — and mark all of it as
    /// internal bookkeeping, in both locales.
    #[test]
    fn incremental_notes_section_teaches_the_loop_in_both_locales() {
        for locale in ["zh", "en"] {
            let g = render_guidance("Boss", locale);
            assert!(
                g.contains("### 增量笔记") || g.contains("### Incremental notes"),
                "[{locale}] notes need their own heading"
            );
            for tool in ["fleet__notes", "fleet__history", "fleet notes", "fleet history"] {
                assert!(g.contains(tool), "[{locale}] must name {tool}");
            }
            // The injected block the agent is told to read first.
            assert!(g.contains("<fleet_notes>"), "[{locale}] must name the injected hint block");
            assert!(g.contains("line_no"), "[{locale}] must teach history search → read by line_no");
            assert!(
                g.contains("内部记账") || g.contains("internal bookkeeping"),
                "[{locale}] must mark notes as internal, not for the user"
            );
            assert!(
                g.contains("边做边记") || g.contains("as you go"),
                "[{locale}] must say notes are incremental, not end-of-task"
            );
        }
    }

    #[test]
    fn render_pins_down_when_commit_is_allowed() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains("explicitly asks") && g.contains("last"),
            "guidance must spell out the two cases when commit is allowed"
        );
        assert!(
            g.contains("push") && (g.contains("approval") || g.contains("approve")),
            "guidance must separately gate `git push` so users can't lose remote state by accident"
        );
    }

    #[test]
    fn render_specifies_tasks_md_format() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains("fleet:prd:begin") && g.contains("fleet:prd:end"),
            "guidance must define the active-plan sentinel inside TASKS.md so the hook knows what to re-inject"
        );
        assert!(
            g.contains("- [ ]") && g.contains("- [x]"),
            "guidance must specify the checkbox format so completion state is machine-readable"
        );
    }

    #[test]
    fn render_teaches_v2_and_fleet_plan() {
        let g = render_guidance("Boss", "en");
        assert!(g.contains("v=\"2\""), "guidance must show the v2 sentinel attribute");
        assert!(
            g.contains("fleet plan check") && g.contains("fleet plan migrate"),
            "guidance must teach the fleet plan subcommands for updating plans"
        );
    }

    #[test]
    fn render_keeps_distinct_marker_from_interaction_mode() {
        // The two modes share ~/.claude/CLAUDE.md — their sentinels must not
        // collide, otherwise applying one removes the other.
        assert_ne!(BEGIN_MARKER, "<!-- fleet:interaction-mode:begin -->");
        assert_ne!(END_MARKER, "<!-- fleet:interaction-mode:end -->");
    }

    #[test]
    fn render_documents_multi_plan_id_format() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains("fleet:prd:begin id=") && g.contains("fleet:prd:end id="),
            "guidance must teach the id-tagged sentinel form so multiple plans can coexist"
        );
        assert!(
            g.contains("unique") && g.contains("id"),
            "guidance must require ids to be unique within one TASKS.md"
        );
        assert!(
            g.contains("only edit your own") || g.contains("Only edit your own"),
            "guidance must instruct the agent to leave other plans' blocks untouched"
        );
    }

    #[test]
    fn render_documents_multi_source_scan_and_dedup() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains("Multi-source scan across worktrees"),
            "guidance must call out the multi-source scan section"
        );
        assert!(
            g.contains(".worktrees/*/TASKS.md") || g.contains(".worktrees/<task-id>"),
            "guidance must show that worktree TASKS.md files are scanned alongside the main one"
        );
        assert!(
            g.contains("mtime") && g.contains("most recent"),
            "guidance must spell out the mtime-newest-wins dedup rule so agents don't guess"
        );
        assert!(
            g.contains("keep a given `id` in exactly one TASKS.md file"),
            "guidance must tell agents not to clone an id-tagged block across files"
        );
        assert!(
            g.contains("source:"),
            "guidance must explain that the rendered header carries a `source:` annotation for worktree blocks"
        );
    }

    #[test]
    fn render_keeps_legacy_unmarked_block_compatibility() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.to_lowercase().contains("legacy") || g.to_lowercase().contains("backwards"),
            "guidance must call out backwards compatibility for the unmarked sentinel form"
        );
    }

    #[test]
    fn render_includes_gitignore_reminder() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains(".gitignore") && g.contains("TASKS.md"),
            "guidance must remind the agent to surface a .gitignore entry for TASKS.md"
        );
        assert!(
            g.contains("offer") || g.contains("mention") || g.contains("ask"),
            "guidance must say to surface the suggestion to the user, not silently edit .gitignore"
        );
    }

    #[test]
    fn render_includes_rule_3_worktree_workflow() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains("## Rule 3") && g.contains("Worktree"),
            "guidance must include the Rule 3 worktree workflow section"
        );
        assert!(
            g.contains("git worktree add"),
            "guidance must show the exact worktree-creation command so the agent doesn't guess the syntax"
        );
    }

    #[test]
    fn render_mandates_no_ff_merge_and_forbids_squash() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains("--no-ff"),
            "merge strategy must be --no-ff to preserve P-task-granularity history on main"
        );
        assert!(
            g.contains("--squash") && g.contains("forbidden"),
            "guidance must explicitly forbid --squash so agents don't substitute it for --no-ff"
        );
        assert!(
            g.contains("--ff-only") && g.contains("forbidden"),
            "guidance must explicitly forbid --ff-only so the merge commit is always materialised"
        );
    }

    #[test]
    fn render_specifies_worktree_path_and_branch_conventions() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains(".worktrees/"),
            "guidance must pin the worktree directory convention"
        );
        assert!(
            g.contains("prd/<task-id>"),
            "Rule 3's worktree branch uses the generic <task-id> placeholder so it works for both multi-step plans and single-step changes"
        );
        assert!(
            g.contains("prd/<plan-id>"),
            "Rule 1/4 cross-references in multi-step contexts continue to use <plan-id> — both forms must coexist"
        );
    }

    #[test]
    fn render_allows_intermediate_commits_inside_worktree() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains("Intermediate commits") || g.contains("intermediate commits"),
            "guidance must explicitly state that intermediate commits are allowed inside the worktree"
        );
        assert!(
            g.contains("do NOT violate Rule 1") || g.contains("don't violate Rule 1"),
            "guidance must cross-reference Rule 1 so the agent doesn't second-guess and ask permission"
        );
    }

    #[test]
    fn render_rule_1_cross_references_rule_3() {
        let g = render_guidance("Boss", "en");
        let r1_pos = g.find("## Rule 1").expect("Rule 1 section must exist");
        let r2_pos = g.find("## Rule 2").expect("Rule 2 section must exist");
        let r1_body = &g[r1_pos..r2_pos];
        assert!(
            r1_body.contains("Rule 3") && r1_body.contains("--no-ff"),
            "Rule 1's allowed-commit clause must point at Rule 3's merge form so the two rules stay coherent"
        );
    }

    #[test]
    fn render_includes_worktrees_gitignore_reminder() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains(".worktrees/") && g.contains(".gitignore"),
            "guidance must remind the agent to surface a .gitignore entry for .worktrees/"
        );
    }

    #[test]
    fn render_specifies_cleanup_steps_for_completed_worktree() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains("git worktree remove") && g.contains("git branch -d"),
            "guidance must spell out cleanup commands so worktrees don't accumulate"
        );
    }

    #[test]
    fn render_header_lists_three_failure_modes() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains("three failure modes"),
            "header must reflect that Rule 4 adds a third failure mode beyond Rule 1/2"
        );
        assert!(
            g.contains("Progress-report checkpointing"),
            "header must name the third failure mode explicitly so agents recognise it"
        );
    }

    #[test]
    fn render_includes_rule_4_execution_rhythm() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains("## Rule 4") && g.contains("rhythm"),
            "guidance must include Rule 4 — Plan execution rhythm"
        );
    }

    /// The three P-tree mechanisms must be documented **in both locales**, or an
    /// agent meets them only as unexplained friction: a refused `create`, a
    /// narrowed injection, and a `Stop` that won't take.
    ///
    /// Checking both is the point: `render_guidance` early-returns an entirely
    /// separate Chinese body for `locale == "zh"`, so editing the English half
    /// alone leaves a zh session — the common case here — with none of it.
    #[test]
    fn render_forbids_no_op_spins_in_both_locales() {
        // Prevention half of the idle-spin guard (see `crate::idle_spin`). The
        // ban alone is not enough — an agent spins because it correctly fears
        // losing backgrounded work, so both locales must also carry the four
        // replacements and the `sleep N; <probe>` carve-out. Without those, the
        // text reads as "don't" and the agent has nowhere to go but back.
        let en = render_guidance("Boss", "en");
        assert!(
            en.contains("Never spin a no-op command to hold the turn open")
                && en.contains("echo waiting"),
            "[en] the no-op spin ban must be documented"
        );
        for replacement in ["timeout", "Monitor", "fleet watch", "just end the turn"] {
            assert!(
                en.contains(replacement),
                "[en] the ban must point at {replacement}"
            );
        }
        assert!(
            en.contains("sleep 45"),
            "[en] the cheaper sleep-then-probe neighbour must stay explicitly allowed"
        );

        let zh = render_guidance("老板", "zh");
        assert!(
            zh.contains("绝不用空转命令保活回合") && zh.contains("echo waiting"),
            "[zh] the no-op spin ban must be documented"
        );
        for replacement in ["timeout", "Monitor", "fleet watch", "直接结束回合"] {
            assert!(
                zh.contains(replacement),
                "[zh] the ban must point at {replacement}"
            );
        }
        assert!(
            zh.contains("sleep 45"),
            "[zh] the cheaper sleep-then-probe neighbour must stay explicitly allowed"
        );
    }

    #[test]
    fn render_documents_the_plan_tree_mechanisms_in_both_locales() {
        let en = render_guidance("Boss", "en");
        assert!(
            en.contains("--root") && en.contains("becomes that plan's child"),
            "[en] create must document the inherited-parent default"
        );
        assert!(
            en.contains("Explore plans vs exec plans") && en.contains("--kind explore"),
            "[en] the explore→exec contract must be documented"
        );
        assert!(
            en.contains("The plan gate") && en.contains("Focused injection"),
            "[en] the gate and the narrowed injection must be documented"
        );

        let zh = render_guidance("老板", "zh");
        assert!(
            zh.contains("--root") && zh.contains("自动成为它的子计划"),
            "[zh] create must document the inherited-parent default"
        );
        assert!(
            zh.contains("explore 计划与 exec 计划") && zh.contains("--kind explore"),
            "[zh] the explore→exec contract must be documented"
        );
        assert!(
            zh.contains("计划门") && zh.contains("聚焦注入"),
            "[zh] the gate and the narrowed injection must be documented"
        );
    }

    /// Both halves of the tree-position rule must be documented in both locales:
    /// the inherited-parent default, and the `--root-reason` needed to opt out.
    /// Documenting only the flag would leave the agent meeting the default as
    /// unexplained behaviour ("why did my plan get a parent I never asked for?")
    /// and the gate as an unexplained refusal. Same both-locales reasoning as the
    /// test above: the zh body is a separate string, so editing only the English
    /// half leaves zh sessions blind.
    #[test]
    fn render_documents_the_inherited_parent_default_in_both_locales() {
        let en = render_guidance("Boss", "en");
        assert!(
            en.contains("--root-reason") && en.contains("is by default its son"),
            "[en] the default must be stated as the mechanism, not just the flag"
        );
        assert!(
            en.contains("plan in flight"),
            "[en] the no-focus case must be documented or the default reads as always-on"
        );
        let zh = render_guidance("老板", "zh");
        assert!(
            zh.contains("--root-reason") && zh.contains("默认就是"),
            "[zh] the default must be stated as the mechanism, not just the flag"
        );
        assert!(
            zh.contains("root 本来就是默认"),
            "[zh] the no-focus case must be documented or the default reads as always-on"
        );
    }

    #[test]
    fn render_rule_4_specifies_three_step_loop() {
        let g = render_guidance("Boss", "en");
        let r4_pos = g.find("## Rule 4").expect("Rule 4 section must exist");
        let r4_body = &g[r4_pos..];
        assert!(
            r4_body.contains("**Dev**")
                && r4_body.contains("**Test / verify**")
                && r4_body.contains("**Commit inside the worktree**"),
            "Rule 4 must spell out dev / test-verify / commit as the three-step loop, in that order"
        );
    }

    #[test]
    fn render_rule_4_forbids_progress_report_checkpoints() {
        let g = render_guidance("Boss", "en");
        let r4_pos = g.find("## Rule 4").expect("Rule 4 section must exist");
        let r4_body = &g[r4_pos..];
        assert!(
            r4_body.contains("should I continue") || r4_body.contains("shall I continue"),
            "Rule 4 must name the exact prompt pattern it forbids so agents recognise themselves doing it"
        );
        assert!(
            r4_body.contains("review") && r4_body.contains("progress"),
            "Rule 4 must forbid the `want to review progress` style checkpoint by name"
        );
        assert!(
            r4_body.contains("written quite a few P-tasks") || r4_body.contains("a lot of progress"),
            "Rule 4 must call out the 'I've done a lot, want to check in?' pattern that Boss reported as the actual failure mode"
        );
    }

    #[test]
    fn render_rule_4_allows_one_repair_attempt_for_test_red() {
        let g = render_guidance("Boss", "en");
        let r4_pos = g.find("## Rule 4").expect("Rule 4 section must exist");
        let r4_body = &g[r4_pos..];
        assert!(
            r4_body.contains("ONE round"),
            "Rule 4 must pin the test-red threshold to exactly one repair attempt (capitalised for emphasis) so agents don't loop indefinitely"
        );
        assert!(
            r4_body.contains("fix → retry → fix → retry") || r4_body.contains("fix -> retry"),
            "Rule 4 must explicitly forbid the unbounded fix/retry loop"
        );
    }

    #[test]
    fn render_rule_4_acceptance_gate_at_final_merge() {
        let g = render_guidance("Boss", "en");
        let r4_pos = g.find("## Rule 4").expect("Rule 4 section must exist");
        let r4_body = &g[r4_pos..];
        assert!(
            r4_body.contains("acceptance gate") || r4_body.contains("acceptance moment"),
            "Rule 4 must label the final-merge pause point as an acceptance gate so it's the only sign-off moment"
        );
        assert!(
            r4_body.contains("git merge --no-ff"),
            "Rule 4 must reference Rule 3's exact merge command so the two rules stay aligned"
        );
    }

    #[test]
    fn render_summary_section_separates_rule_3_from_rules_1_2_4() {
        let g = render_guidance("Boss", "en");
        let sum_pos = g
            .find("## When this mode does NOT apply")
            .expect("summary section must exist");
        let sum_body = &g[sum_pos..];
        assert!(
            sum_body.contains("Rule 3") && sum_body.contains("global"),
            "summary must label Rule 3 as global so single-step production-code changes still trigger it"
        );
        assert!(
            sum_body.contains("Single-step production-code change"),
            "summary must explicitly enumerate the single-step case so agents don't fall back to the old 'single-step → no worktree' interpretation"
        );
        assert!(
            sum_body.contains("Rules 1, 2, 4 do NOT")
                || sum_body.contains("Rule 1, 2, 4 do NOT")
                || sum_body.contains("Rules 1/2/4"),
            "summary must spell out which rules a single-step change is exempt from, to prevent re-emergence of the misread"
        );
        assert!(
            !sum_body.contains("ignore all four rules"),
            "the old 'ignore all four rules' line must be gone — Rule 3 is no longer in the same bucket"
        );
    }

    #[test]
    fn render_rule_3_applies_to_single_step_changes() {
        let g = render_guidance("Boss", "en");
        let r3_pos = g.find("## Rule 3").expect("Rule 3 must exist");
        let r3_end = g[r3_pos..].find("## Rule 4").expect("Rule 4 must exist");
        let r3_body = &g[r3_pos..r3_pos + r3_end];
        assert!(
            r3_body.contains("single mechanical change")
                || r3_body.contains("single-step changes"),
            "Rule 3 must explicitly cover single-step changes in its opening so agents don't infer multi-step gating"
        );
        assert!(
            r3_body.contains("Rule 3 is global"),
            "Rule 3 must call itself 'global' to overpower Rule 1's multi-step framing when read in isolation"
        );
    }

    #[test]
    fn render_rule_3_warns_about_gitignored_artifact_loss_on_worktree_remove() {
        let g = render_guidance("Boss", "en");
        let r3_pos = g.find("## Rule 3").expect("Rule 3 must exist");
        let r3_end = g[r3_pos..].find("## Rule 4").expect("Rule 4 must follow");
        let r3_body = &g[r3_pos..r3_pos + r3_end];
        assert!(
            r3_body.contains("only carries across"),
            "Rule 3 must explain that `git merge --no-ff` only brings across committed content, so gitignored/untracked files never reach main"
        );
        assert!(
            r3_body.contains("no git object to recover"),
            "Rule 3 must spell out that `git worktree remove` deletes untracked files with no git object to recover them from — the irreversible data-loss step Boss flagged"
        );
        assert!(
            r3_body.contains("don't put this in version control") && r3_body.contains("don't keep"),
            "Rule 3 must correct the `.gitignore` misconception: ignored means not-version-controlled, NOT not-kept"
        );
        assert!(
            r3_body.contains("git status --ignored"),
            "Rule 3 must name the concrete pre-removal self-check command"
        );
    }

    #[test]
    fn render_rule_3_not_apply_drops_single_step_exemption() {
        let g = render_guidance("Boss", "en");
        let na_pos = g
            .find("### When Rule 3 does NOT apply")
            .expect("Rule 3 NOT-apply subsection must exist");
        let na_end = g[na_pos..]
            .find("## Rule 4")
            .expect("Rule 4 must follow the NOT-apply subsection");
        let na_body = &g[na_pos..na_pos + na_end];
        assert!(
            !na_body.contains("Single-step tasks (already exempted by Rule 1)"),
            "the old 'Single-step tasks → exempted' line must be removed — that wording was the source of the misread"
        );
        assert!(
            na_body.contains("whether multi-step or single-step"),
            "Rule 3's NOT-apply subsection must affirm both step-counts are covered, killing the loophole at the source"
        );
    }

    #[test]
    fn render_includes_tooling_recommendations_section() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains("## Recommended tooling"),
            "guidance must include the tooling-recommendations section paired with Rule 3 worktrees"
        );
    }

    #[test]
    fn render_tooling_section_is_advice_not_rule_5() {
        let g = render_guidance("Boss", "en");
        let tooling_pos = g
            .find("## Recommended tooling")
            .expect("tooling section must exist");
        let tooling_body = &g[tooling_pos..];
        assert!(
            tooling_body.contains("not Rule 5") || tooling_body.contains("not a Rule"),
            "tooling section must explicitly disclaim Rule-5 status so agents treat it as advice, not discipline"
        );
        assert!(
            tooling_body.contains("recommendations"),
            "tooling section must use the word 'recommendations' so the soft nature is unmistakable"
        );
    }

    #[test]
    fn render_recommends_pnpm_and_uv_for_worktree_friendliness() {
        let g = render_guidance("Boss", "en");
        let tooling_pos = g
            .find("## Recommended tooling")
            .expect("tooling section must exist");
        let tooling_body = &g[tooling_pos..];
        assert!(
            tooling_body.contains("pnpm") && tooling_body.contains("npm"),
            "tooling section must recommend pnpm and contrast it with npm explicitly"
        );
        assert!(
            tooling_body.contains("uv") && tooling_body.contains("pip"),
            "tooling section must recommend uv and contrast it with pip explicitly"
        );
    }

    #[test]
    fn render_tooling_notes_rust_and_go_default_global_cache() {
        let g = render_guidance("Boss", "en");
        let tooling_pos = g
            .find("## Recommended tooling")
            .expect("tooling section must exist");
        let tooling_body = &g[tooling_pos..];
        assert!(
            tooling_body.contains("cargo") && tooling_body.contains("~/.cargo/registry"),
            "tooling section must reassure agents that cargo is already worktree-friendly so they don't try to 'fix' it"
        );
        assert!(
            tooling_body.contains("$GOMODCACHE") || tooling_body.contains("GOMODCACHE"),
            "tooling section must note Go's global module cache so agents don't second-guess Go projects"
        );
    }

    #[test]
    fn render_rule_1_pins_commit_scope_to_main_branch() {
        let g = render_guidance("Boss", "en");
        let r1_pos = g.find("## Rule 1").expect("Rule 1 section must exist");
        let r2_pos = g.find("## Rule 2").expect("Rule 2 section must exist");
        let r1_body = &g[r1_pos..r2_pos];
        assert!(
            r1_body.contains("Scope of \"commit\""),
            "Rule 1 must carry a top-level scope clarifier so agents read it before the DO NOTs"
        );
        assert!(
            r1_body.contains("main / default branch"),
            "Rule 1 scope clarifier must name 'main / default branch' so worktree commits are clearly out of scope"
        );
        assert!(
            r1_body.contains("governed by Rule 3"),
            "Rule 1 scope clarifier must point at Rule 3 so worktree commits don't trigger false conflict reports"
        );
        assert!(
            r1_body.contains("propose `git commit` on main")
                && r1_body.contains("run `git commit` on main")
                && r1_body.contains("commit on main only when"),
            "all three DO NOT/MAY clauses in Rule 1 must say 'on main' so the scope is unambiguous even read in isolation"
        );
    }

    #[test]
    fn render_tooling_warns_against_silent_lockfile_migration() {
        let g = render_guidance("Boss", "en");
        let tooling_pos = g
            .find("## Recommended tooling")
            .expect("tooling section must exist");
        let tooling_body = &g[tooling_pos..];
        assert!(
            tooling_body.contains("do NOT silently migrate")
                || tooling_body.contains("do not silently migrate"),
            "tooling section must forbid silent lockfile/package-manager migration on existing projects"
        );
        assert!(
            tooling_body.contains("package-lock.json"),
            "tooling section must name the lockfile so the rule is concrete, not abstract"
        );
    }

    #[test]
    fn render_includes_rule_6_scope_fidelity() {
        let g = render_guidance("Boss", "en");
        let r6_pos = g.find("## Rule 6").expect("Rule 6 section must exist");
        let r6_body = &g[r6_pos..];
        assert!(
            r6_body.contains("traceable") && r6_body.contains("actually said"),
            "Rule 6 must require every planned requirement to be traceable to what Boss actually said"
        );
        assert!(
            r6_body.contains("No silent scope"),
            "Rule 6 must forbid silently adding un-asked-for requirements to the plan"
        );
        assert!(
            r6_body.contains("thinnest vertical slice"),
            "Rule 6 must prescribe a thin vertical slice before building abstractions for imagined needs"
        );
        assert!(
            r6_body.contains("RFC") && r6_body.contains("scope audit"),
            "Rule 6 must reframe the 'I need an RFC' urge as a scope-audit trigger, not a design contract"
        );
    }
}
