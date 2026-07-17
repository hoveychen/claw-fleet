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
    crate::session::real_home_dir().map(|h| h.join(".claude"))
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
    let title = if user_title.is_empty() {
        "Boss".to_string()
    } else {
        user_title.to_string()
    };

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
            "# Fleet PRD 纪律 (managed by Claw Fleet — do not edit)\n\
\n\
本模式锁死三个会拖垮长程多步计划的失败模式：\n\
\n\
1. **计划中途的提交唠叨。**代理做完一个 P-task，冒出「现在要提交吗？」的\
条件反射，{title}得不停地说「不用，继续」。\n\
2. **压缩后的任务失忆。**上下文压缩后，代理记得自己刚做完 P2，却丢了\
「P3..Pn 仍待办」这个宏观状态。\n\
3. **进度汇报式打卡。**代理做完一个 P-task 就停下来问「要我继续下一个吗？」\
或「进展不错，P4 前要不要先审一下？」。TASKS.md 勾选框和 worktree 提交已让进度\
一目了然。\n\
\n\
## Rule 1 —— 多步计划期间的提交纪律\n\
\n\
这里的**多步计划**指：任何你拆成 2 个或更多顺序子任务（P1、P2、...、Pn——或\
编号 todo，或任何等价物）的任务。一旦进入这样一个计划，以下规则一直适用到计划\
彻底完成：\n\
\n\
**本规则里「提交」的范围。**整个 Rule 1 里，「提交」指**主/默认分支**上的提交。\
worktree 特性分支（`prd/<plan-id>`）上的提交由 Rule 3 管辖、在每个 P-task 边界\
都明确允许——它们不算 Rule 1 违规，也无需作为与本规则的冲突点出。\n\
\n\
- **不要主动提议在 main 上 `git commit`。** P1 之后不要，P2 之后不要，任何你\
感觉到的「自然检查点」都不要。工作的单位是计划，不是单个 P-task。\n\
- **也不要真的在 main 上跑 `git commit`**，除下面两种情形外。\n\
- **只有以下情形你才可以在 main 上提交：**\n\
  1. {title}在本回合明确要求提交，或\n\
  2. 你刚做完计划里的**最后**一个 P-task（即 TASKS.md 里所有项都已勾选）**且**\
     你已向{title}呈报计划完成。Rule 3（worktree 工作流）生效时，这个 main 上\
     唯一允许的提交采取从 worktree 分支 `git merge --no-ff` 的形式——确切流程\
     见 Rule 3。\n\
- **`git push` 永远受闸控**——无论计划处于什么状态，没有{title}在本回合的明确\
  批准绝不 push。\n\
\n\
### 「完成」意味着什么\n\
\n\
计划完成 = 所有 P-task 在 TASKS.md 已勾选（见 Rule 2）+ 构建/测试已跑 + 已向\
{title}呈报改动摘要。三者未全为真前，计划未完成，不要提议提交。\n\
\n\
### 边缘情形\n\
\n\
- **单步任务**（一个 bug 修复、一次重命名、一处配置微调）：不是多步计划，适用\
  常规提交礼仪。\n\
- **{title}在计划中途问「能把目前做完的提交一下吗？」**：这是上面的情形 1——\
  照做。\n\
- **你撞上一个需要{title}输入的阻塞点**：暂停并通过 AskUserQuestion 提问（该\
  工具不可用时用纯文本）。不要拿阻塞当借口「怕进度丢了」而提交。阻塞解除后再\
  继续。\n\
- **你即将做破坏性操作**（rebase、force-push、删分支）：无论计划状态如何，停\
  下来问。本规则不覆盖既有的破坏性操作确认要求。\n\
\n\
## Rule 2 —— TASKS.md 作为持久的宏观计划\n\
\n\
上下文压缩会把对话历史压平。近期动作（「提交成功」）高保真地留存；宏观状态\
（「P3..P10 仍待办」）被摘要掉。为熬过压缩，宏观计划落在磁盘上。\n\
\n\
**约定：**\n\
\n\
- 当你把任务拆成 2 个或更多子任务时，在开始 P1 **之前**把拆解写进\
  `<workspace_root>/TASKS.md`。\n\
- 每完成一个 P-task，把它在 TASKS.md 里的勾选框更新为 `[x]`。\n\
- 每个回合开始时（压缩之后，或你不确定宏观状态时），活跃计划区域会由 Fleet 的\
  UserPromptSubmit hook 自动作为 system-reminder 重新注入——但你也可以在需要时\
  显式 `Read` 该文件。\n\
- 当你的计划彻底完成，你可以移除自己计划的哨兵块（或留着作历史——提交时由\
  {title}定夺）。不要动其他计划的块。\n\
\n\
### 一个 TASKS.md 里的多个计划\n\
\n\
单个 workspace 的 `TASKS.md` 可以**并行承载多个计划**——{title}可以让你做计划\
A，同时另一个代理（或另一段对话）正在推进计划 B。每个计划活在自己的哨兵对里，\
由唯一的 `id` 标识：\n\
\n\
```markdown\n\
# TASKS\n\
\n\
<!-- fleet:prd:begin id=\"auth-refactor\" v=\"2\" -->\n\
\n\
**Plan:** Migrate session middleware to the new auth crate\n\
\n\
- [x] **P1** — Audit existing call sites\n\
- [ ] **P2** — Swap middleware impl\n\
- [ ] **P3** — Update integration tests\n\
\n\
<!-- fleet:prd:end id=\"auth-refactor\" -->\n\
\n\
<!-- fleet:prd:begin id=\"prd-multiplan\" v=\"2\" -->\n\
\n\
**Plan:** Teach TASKS.md to host parallel plans\n\
\n\
- [ ] **P1** — New sentinel format with `id=\"...\"`\n\
- [ ] **P2** — Hook scans all blocks and re-injects each\n\
\n\
<!-- fleet:prd:end id=\"prd-multiplan\" -->\n\
```\n\
\n\
`begin` 哨兵上的 `v=\"2\"` 属性标记 **v2 schema**。`end` 哨兵只需匹配的 `id`。\
旧版 v1 块（无 `v=\"2\"`）仍可用；跑 `fleet plan migrate` 可就地升级旧的\
TASKS.md。\n\
\n\
**处理多计划 TASKS.md 的规则：**\n\
\n\
1. **为你的计划挑一个唯一的 `id`。**用 kebab-case、≤ 32 字符、描述该工作（如\
   `auth-refactor`、`import-cleanup`）。新建计划前，先 `Read` TASKS.md 确认没有\
   现存块用了同一 id。\n\
2. **只编辑你自己的块。**勾选框或修改计划时，只改*你自己*的 `begin id=\"X\"`\
   与 `end id=\"X\"` 之间的行。把其他每个块都当作只读——它属于另一个可能正在\
   推进的计划。\n\
3. **两个哨兵上的 id 要匹配。** `begin id=\"X\"` 必须与 `end id=\"X\"` 配对。id\
   不匹配会被 hook 忽略。\n\
4. **旧版无标记块仍被识别。**一对裸的 `<!-- fleet:prd:begin -->` /\
   `<!-- fleet:prd:end -->`（无 `id=`）为向后兼容被当作单个匿名计划。不要再以\
   这种形式新建——始终用显式 id。\n\
5. **不要合并或重排别人的计划。**若两个块看着冗余，向{title}点出，而不是自己\
   把它们合掉；另一个块可能属于你看不到的会话。\n\
\n\
### 用 `fleet plan` 更新计划，而非手改\n\
\n\
相较直接编辑 TASKS.md markdown，优先用 `fleet plan` 子命令。它们做出同样的\
文件改动，**并且**记录哪个会话在做哪个计划/P、带时间戳——这样即使多个会话共用\
一个 TASKS.md，桌面端也能显示*你*当前的计划和 P（Fleet 知道你的\
`FLEET_SESSION_ID`；你自己读不到墙上时钟）。命令：\n\
\n\
- `fleet plan create <id> --title \"...\" [--parent <parent-id>]` —— 新增一个\
  v2 计划块**并**把本会话记录为它的执行者。创建计划就是开始它，故无需另行声明。\
  当新计划是**你在父计划中途分出的旁支工作**（一个你必须返回的岔路）时传\
  `--parent`——见下方「子计划与回溯」。\n\
- `fleet plan check <id> <P>` —— 勾选一个任务为完成（`[ ]`→`[x]`）并把本会话的\
  焦点刷新到 `<id>`。如 `fleet plan check auth-refactor P2`。\n\
- `fleet plan uncheck <id> <P>` —— 取消勾选。\n\
- `fleet plan resume <id> [P]` —— 接手一个你没创建的**现存**计划（不改文件；\
  设定你的当前 P，默认第一个待办）。`create` 之后不需要它，交接之后也不需要——\
  Fleet 会替你归属后继者。\n\
- `fleet plan add <id> <P> --text \"...\"` —— 追加一个待办任务。不记录焦点：\
  编辑计划的形态并不说明谁执行它。\n\
- `fleet plan migrate` —— 把本 workspace 的 v1 TASKS.md 升级到 v2（幂等）。\n\
- `fleet plan list` / `fleet plan get <id>` —— 读取。\n\
\n\
手改 TASKS.md 仍有效——文件是勾选框的唯一真相来源——但它不记录归属，所以桌面端\
无法判断你的会话在哪个计划上，你的卡片上什么都不显示。\n\
\n\
### 子计划与回溯\n\
\n\
计划中途你有时得分出一条**旁支**——一块必须先完成、主计划才能继续的独立工作\
（一个前置重构、当前 P 依赖的一个 bug）。把它建成**子计划**，让这段岔路不至于\
把你来时的计划晾在那儿：\n\
\n\
```\n\
fleet plan create <side-id> --title \"...\" --parent <current-plan-id>\n\
```\n\
\n\
这会在旁支计划的哨兵上记录 `parent=\"<current-plan-id>\"`。当你用\
`fleet plan check` 勾掉那个子计划的**最后**一个框时，Fleet 沿 `parent` 链向上\
走到最近的、仍有待办 P-task 的祖先，**把你的焦点重新指回它**，并打印一条指令\
告诉你下一个要恢复的 P。你不用自己跑 `fleet plan resume`——照指令继续就行；\
不要因为子计划完成了就结束回合。prd-context hook 里有个兜底：若你的焦点被留在\
一个已完成的子计划上（例如最后那个框是手改而非用 `fleet plan check` 勾的），它\
每个 prompt 都会重发同样的提醒。\n\
\n\
子计划可以嵌套（子计划可以有自己的子计划），且向上走会跳过已完成的祖先，所以\
回溯总是落在树上最近的未完成工作。没有 `--parent` 的计划是顶层：完成它不回溯到\
任何地方，计划就此结束。\n\
\n\
格式本身的经验法则：\n\
- 待办用 `- [ ]`、完成用 `- [x]`（`fleet plan check/uncheck` 会替你写这些）。\
  不要发明新状态；简单的勾选框就是约定——「此刻谁在做什么」由 Fleet 跟踪，而非\
  文件里的某个标记。\n\
- P-task 标题保持 ≤ 60 字符。长验收备注放进子 bullet。\n\
- {language_line}\n\
\n\
### 跨 worktree 的多源扫描\n\
\n\
因为 Rule 3 在 `.worktrees/<task-id>/` checkout 里开发计划，prd-context hook\
每个 prompt 都会扫描它能为该 repo 找到的**每一个** TASKS.md——主 checkout 的\
`<repo>/TASKS.md` 加上每个存在的 `<repo>/.worktrees/*/TASKS.md`——并把它们全部\
的活跃计划合并进一次注入。无论会话的 cwd 是主 checkout 还是某个 worktree，这都\
一样运作，所以跑在 worktree 里的 worker 代理照样能看到活在主 checkout 里的计划\
（反之亦然）。\n\
\n\
**去重规则：**当同一个 `id=\"X\"` 出现在不止一个 TASKS.md 文件里时，hook 保留\
mtime 最新的那个文件里的版本，丢弃其余。当块来自某个 worktree TASKS.md 时，\
渲染出的计划标题会带一个 `— source: <path>` 后缀，好让代理知道该编辑哪个文件。\
匿名（旧版无标记）块按文件各自独立保留——它们早于多计划格式。\n\
\n\
**因此：把给定的 `id` 只放在一个 TASKS.md 文件里。**把同一个带 id 的块从主\
checkout 复制进 worktree（或在两个 worktree 之间复制）会造出一个幽灵计划，它随\
你最后保存的是哪个文件而闪烁。如果某个计划出于任何原因需要活在 worktree 里，先\
把它从主 TASKS.md 删掉。\n\
\n\
### 让 TASKS.md 别进 git\n\
\n\
TASKS.md 是代理的临时草稿状态——它不该进版本控制。你在某个 workspace 里第一次\
创建 `TASKS.md`（即它在本回合前不存在）时，检查它是否已被 `.gitignore` 覆盖，\
若没有，**向{title}提一句并主动提议往 `.gitignore` 加一行 `TASKS.md`**。不要\
悄悄改写 `.gitignore`——把建议点出、让{title}批准。之后编辑已存在的 TASKS.md\
时，无需再提醒。\n\
\n\
## Rule 3 —— 基于 worktree 的特性工作流\n\
\n\
**任何触碰生产代码的改动都必须在一个隔离的 git worktree 里开发**，位于\
`<repo-root>/.worktrees/<task-id>`、基于新分支 `prd/<task-id>`——**无论这工作是\
多步计划（P1..Pn）还是单次机械改动**。Rule 3 是全局的；它不受 Rule 1 多步计划\
定义的限制。多步计划里，`<task-id>` 就是你为 TASKS.md 哨兵块挑的那个 id（Rule\
2）；单步改动里，当场挑一个短 kebab-case 标识（如 `fix-zombie-pid`、\
`rename-task-fields`）。\n\
\n\
**约定：**\n\
\n\
- **触碰任何生产代码之前**，基于当前 main 在新分支上创建 worktree：\n\
\n\
  ```\n\
  git worktree add -b prd/<task-id> .worktrees/<task-id> main\n\
  ```\n\
\n\
  所有代码工作都在这个 worktree 里跑；主 checkout 全程保持干净。最后一个 P-task\
  （单步改动则是收尾动作）是合并回 main。\n\
- **worktree 内的中间提交明确允许，不违反 Rule 1。** Rule 1 的「不主动提交」\
  针对的是 *main*；`prd/<task-id>` 上的提交是别的工作看不到的进度标记。只要有助于\
  推理下一步（如某个后续改动回退了行为时用 `git diff HEAD~1`），就在 P-task 之间\
  提交（单步改动也可拆成多个提交）。你仍无需*请求*{title}许可才能在 worktree 内\
  提交——那是私有分支上的自由移动。\n\
- **工作以一次原子的合并回 main 结束**（最后一个 P-task，单步改动则是收尾\
  动作）。从主 checkout：\n\
\n\
  ```\n\
  git merge --no-ff prd/<task-id>\n\
  ```\n\
\n\
  `--no-ff` 是强制的。`--ff-only` 和 `--squash` 被禁止——我们让每个 worktree\
  提交在 main 历史里都可见，旁边配一个概括改动的单一合并提交，好让工作在每提交\
  粒度上保持可审计。这个 `git merge --no-ff` 就是 Rule 1 允许的那唯一一次 main\
  上提交；不要在它之前或之后再跑任何 `git commit`。\n\
- **合并或移除 worktree 之前，抢救计划生成的 gitignored / 未跟踪产物。**\
  `git merge --no-ff` 只带过*已提交*的内容。任何被 `.gitignore` 匹配的东西——\
  以及任何你从未 `git add` 的文件——都从未被提交，故它只活在 worktree 的工作\
  目录里。`git worktree remove` 随后会连同这些文件一起删掉那个目录，而因为它们\
  从未被跟踪，没有 git 对象能恢复它们：数据永久丢失。`.gitignore` 意思是「别把\
  这个放进版本控制」，不是「别留着这个」——一个生成的数据集、一个合成的媒体\
  文件、一个下载的资产、一段{title}可能想要的抓取日志、工作中产生的一个\
  `.env`，即便未跟踪，也都是真实数据。所以移除任何东西之前，在 worktree 里跑\
  `git status --ignored`（并检查普通未跟踪文件）。例行可再生的目录——`target/`、\
  `node_modules/`、`dist/`、`.next/`，任何被已提交的构建脚本从头重建的东西——\
  无需抢救；跳过。但若 worktree 里存着一个**不**能从已提交代码轻易重现的生成\
  产物（没提交生成脚本，或输入没了），在移除前停下来向{title}呈报：该把文件拷\
  出 worktree 到安全位置，还是真的应该跟踪它（加进合并，或从 `.gitignore` 移\
  除）？在这解决之前不要 `git worktree remove`——移除是不可逆的那一步。\n\
- **合并成功后，清理。**先确认上面的抢救检查已做。然后跑\
  `git worktree remove .worktrees/<task-id>` 再 `git branch -d prd/<task-id>`。\
  若合并失败（冲突、合并后构建/测试回退），就地解决——不要弃掉 worktree，不要\
  amend 合并提交，不要 `git reset --hard` 抹掉合并。向{title}呈报情况，阻塞解除\
  后再继续。\n\
- **不要把 worktree 分支 push 到远端。** `git push` 仍受 Rule 1 闸控：只凭\
  {title}在本回合的明确批准。本地合并到 main 由 Rule 1 情形 2 允许；push main 是\
  {title}自己拥有的另一个决定。\n\
- **`.worktrees/` 必须在 `.gitignore` 里。**和 TASKS.md 一样对待：你在本 repo\
  第一次创建 worktree 时，检查 `.gitignore`；若 `.worktrees/` 缺席，**向{title}\
  提一句并主动提议加一行 `.worktrees/`**。不要悄悄改写 `.gitignore`。\n\
\n\
### Rule 3 何时不适用\n\
\n\
Rule 3 覆盖任何触碰生产代码的改动，**无论多步还是单步**。单步改动不是跳过\
worktree 的借口——重点就是哪怕一次 50 行的机械编辑也享受同样的隔离。真正的豁免\
关乎你*改什么*，而非它*花几步*：\n\
- 纯文档改动（README、docstring、changelog）。\n\
- 纯配置改动（CI YAML、dotfile、`.gitignore` 本身、格式化器配置）。\n\
- 必须在另一个在飞的 worktree 完成之前落到 main 的紧急热修——先向{title}呈报\
  该热修，好让{title}决定是否暂停活跃的 worktree。\n\
\n\
## Rule 4 —— 计划执行节奏\n\
\n\
多步计划应以一种连续的节奏执行，而非被计划中途的汇报检查点打断。进度的单位是\
*计划*，不是 P-task——{title}本来就能通过 TASKS.md 和（Rule 3 生效时）worktree\
提交看到计划状态，所以显式进度汇报是多余的打断。\n\
\n\
**节奏。**每个非最后的 P-task 遵循同样的三步循环，然后**在同一回合里**立即\
继续下一个 P-task，不为{title}的确认停顿：\n\
\n\
1. **开发** —— 做出该 P-task 要求的代码改动。\n\
2. **测试 / 验证** —— 跑合适的验证（单测、`cargo build`、`pnpm build`、\
   Playwright、类型检查、lint、手动操作 UI——该 P-task 需要什么就跑什么）。\n\
3. **在 worktree 内提交** —— Rule 3 生效时，把该 P-task 记录为 `prd/<plan-id>`\
   上的一个提交，好让后续 P-task 有个干净的参照点。Rule 3 之外（如纯配置\
   计划），此步跳过。\n\
\n\
第 3 步后，用 `fleet plan check <plan-id> <P>` 勾选框——不是手改 TASKS.md——并\
**立即推进到下一个 P-task**。`check` 是让你的会话保持归属到本计划的关键；手改的\
勾选框会让桌面卡片空白。不要停下来做摘要。不要问「要我继续 P2 吗？」或「P4 前\
要不要审一下进度？」。不要提议「我写了不少 P-task 了，要我总结一下吗？」。这些\
正是 Rule 4 存在要消除的主动进度汇报检查点。\n\
\n\
**归属。** Fleet 在会话卡片上显示你当前的计划和 P，但只有当它能把你的会话归属到\
某计划时才行。`fleet plan create`（你写了这计划）和 Fleet 交接（Fleet 把你 spawn\
进去）会自动归属你；`fleet plan check` 随你推进而刷新。唯一需要显式认领的情形是\
**接起一个你没创建、也没被交接的计划**：在你第一个 P-task 之前跑\
`fleet plan resume <plan-id> [P]`。\n\
\n\
### 节奏何时确实要停\n\
\n\
节奏只为以下四种情形之一停顿。「我做了不少，要不要报个到？」永远不是其中之一。\n\
\n\
1. **最后一个 P-task 的验收闸门。** Rule 3 的计划以\
   `git merge --no-ff prd/<plan-id>` 结束。跑合并前，向{title}呈报一份「可以\
   合并了」的摘要并等明确放行。这次合并就是计划的验收时刻；不要在中间检查点\
   征求验收。\n\
2. **一个真正的工作方向问题。**需要{title}判断、因为路上有真岔口的东西——\
   「X 保持向后兼容还是丢掉？」「这数据删还是归档？」「API 设计 A 还是 B？」。\
   引用那个选择和取舍；那是澄清问题，不是进度汇报。\n\
3. **一次挺过一轮修复的测试/验证红灯。** `cargo build` / 单测 / Playwright / \
   hook 在某个 P-task 里第一次失败时，你**可以**试一轮诊断加修复。若那一轮没\
   恢复绿灯，或者你动手前根因就不清楚，停下来作为阻塞点呈报——不要在没有{title}\
   的情况下陷入「修→重试→修→重试」的循环。\n\
4. **一次破坏性操作**（rebase、force-push、删分支、丢弃 migration、\
   `git reset --hard`）。既有的破坏性操作确认要求仍适用；Rule 4 不覆盖它。\n\
\n\
## Rule 5 —— 长上下文交接（`fleet handoff`）\n\
\n\
当你的上下文窗口在计划中途拉长时（上下文用量高，或压缩已经触发），不要死磕到\
窗口耗尽，不要悄悄提前收尾，也不要留下没人执行的「交给下一个会话」的便条。\
Fleet 有一个一等的接力：\n\
\n\
```\n\
fleet handoff --note \"<交接信息>\" [--plan <plan-id>] [--next <P>] [--model <模型>] [--effort <档位>]\n\
```\n\
\n\
- **--note 是强制的**，是后继者在 TASKS.md 之外知道的一切：什么做完了、什么在\
飞、关键文件、坑、下一个具体步骤。像换班简报那样写。\n\
- **当工作是一个 TASKS.md 计划时传 --plan/--next**，好让 Fleet 把后继者自动\
归属到那个计划和 P；它会在那里恢复节奏，无需自己的任何 `fleet plan` 仪式。\n\
- **--model / --effort 可选**：钉死后继者的模型（如 `claude-opus-4-8[1m]`，\
括号后缀原样透传）和推理档位（low|medium|high|max），覆盖否则自动继承的值。\
不传就沿用当前会话的模型与 CLAUDE_EFFORT。\n\
- **然后干净地结束回合**：先按 Rule 3 提交 worktree 进度，再停。你一交出，\
Fleet 的 Stop hook 就消费这个登记，并在同一 workspace spawn 一个全新会话，其\
开场 prompt 就是你的便条；prd-context hook 会自动重新注入 TASKS.md 宏观计划。\n\
- **接力被记录**为一条交接链，显示在会话卡片上（接力 n/N），好让{title}事后\
追溯整个序列。\n\
- 给你会话的一个新用户 prompt 会取消你待定的交接——{title}接管永远优先。链最多\
10 跳；重新登记会覆盖你之前的便条。\n\
\n\
你一旦逮到自己在想「上下文长了，我该收尾了」——那个冲动本身就是信号。去登记\
交接并接力，而不是收尾。\n\
\n\
**叙述一次交接不等于登记一次。**在你的回复文本里写「接下来我起下一棒」/\
「handing off to the next session」/「剩下的我接力」什么都不做：Fleet 的 Stop\
hook 消费的是一次*登记*，不是一句话。如果你本回合没真的跑 `fleet handoff`\
Bash 命令，就没有后继者被 spawn，计划会在你交出的那一刻悄然死掉。所以在结束这样一个\
回合前，你做的最后一件事就是那个工具调用本身：跑 `fleet handoff --note \"...\"`，\
等 `ok: handoff registered` 结果回来，然后才停。绝不让一个回合以只活在文字里的\
交接结束，也绝不去用 ScheduleWakeup / `/loop` / cron 来「稍后继续」——那些在\
Fleet 会话里悄悄空转、什么都不 spawn。真正会跨回合边界触发的 Fleet 接力有两个：\
`fleet handoff` 用于*继续工作*（把简报交给一个全新后继者），`fleet watch` 用于\
*等待一个外部条件*——一次 CI 跑完、一次构建产出产物、一次部署上线。不要坐在\
前台 `Monitor` / 后台 `Bash` 里等这种事件：它们在 `-p` 回合结束的那一刻就死，\
通知永远不到。改跑 `fleet watch create --until '<完成时退出 0 的 shell 命令>'\
--capture '<其 stdout 你想被报告的 shell 命令>' --note '<你在等什么>'`，然后\
结束回合——Fleet 在后台轮询，条件一触发就 `claude --resume` *这个*会话，把捕获\
的结果喂给你的下一回合。`fleet watch stop <id>` 取消它。\n\
\n\
## worktree 工作流的推荐工具\n\
\n\
因为 Rule 3 在一个全新 worktree 里开发每个计划，每个新计划实际上是一个干净的\
checkout——包括依赖树。**按项目**存包的工具（npm 的 `node_modules/`、pip 的\
per-venv site-packages、yarn classic 的 `node_modules/`）会为每个 worktree 重新\
下载、重新安装一切，浪费磁盘和安装时间。带**全局内容寻址缓存**的工具在所有项目\
的所有 worktree 间共享一份副本，所以拉起一个新 worktree 花的是秒，不是分钟。\n\
\n\
这些是*推荐*，不是硬规则——它们不是 Rule 5。若{title}为某个特定项目明确挑了\
别的工具，照那个来。推荐只在{title}尚未做出选择时才生效。\n\
\n\
**新项目优先选 worktree 友好的：**\n\
\n\
- **Node / TypeScript**：优先 **pnpm**（全局 store 在\
  `~/.local/share/pnpm/store`，symlink 进每个项目的 `node_modules/`），而非 npm\
  或 yarn classic。Bun 也用全局缓存、也行；npm 和 yarn classic 是 worktree 密集\
  工作要避开的。\n\
- **Python**：优先 **uv**（全局缓存 + 硬链接的 venv 内容），而非 `pip + venv`。\
  Poetry 若开着缓存共享也可接受，但 uv 在 worktree 拉起上明显更快。\n\
- **Rust**：`cargo` 已全局共享 `~/.cargo/registry`，故无需额外动作。每个\
  worktree 的 `target/` 按设计保持 per-worktree——那是为避免锁竞争的刻意取舍；\
  不要试图在 worktree 间共享 `target/`。\n\
- **Go**：`go` 已全局共享 `$GOMODCACHE` 和 `$GOCACHE`；worktree 在依赖侧花费\
  约等于零。无需动作。\n\
\n\
**对已有项目，不要只因为你要创建 worktree 就悄悄迁移 lockfile 或包管理器。**\
一个 `package-lock.json` 的 repo 在{title}同意切换之前一直留在 npm。切换包\
管理器本身是一个独立计划，有自己的范围、自己的 worktree、自己的验收闸门——动\
lockfile 之前先向{title}呈报成本对迁移的取舍。\n\
\n\
## 本模式何时不适用\n\
\n\
Rule 3（worktree）对任何生产代码改动都是**全局**的。Rule 1、2、4 限于多步计划。\
所以：\n\
\n\
- **单步生产代码改动**：Rule 3 适用（worktree + `--no-ff` 合并回 main）。Rule\
  1、2、4 不适用——无 TASKS.md、无 P-task、无节奏强制。整个改动作为一次机械\
  编辑在 worktree 里发生，然后合并回去。\n\
- **纯对话 / 问答回合，没有代码在改**：四条规则都不适用。用纯文本回复。\n\
- **纯文档、配置或热修工作**（见 Rule 3 自己的「不适用」小节）：除非{title}明确\
  要求把该工作当作多步计划，四条规则都关闭。\n\
- **{title}明确要求把工作保持「非正式」或「快点」**：四条规则都关闭；适用常规\
  提交礼仪，{title}对更轻的流程负责。\n\
\n\
## 与其他模式的交互\n\
\n\
- 本模式**独立于** Fleet 交互模式。它们可以分别启用。\n\
- Bash guard hook（若已安装）仍会运行，仍可能要{title}确认有风险的命令。那是\
  刻意为之——guard 抓风险；本模式抓*不必要*的提交。\n\
",
            title = title,
            language_line = language_line,
        );
    }

    format!(
        "# Fleet PRD Discipline (managed by Claw Fleet — do not edit)\n\
\n\
This mode locks down three failure modes that hurt long multi-step plans:\n\
\n\
1. **Mid-plan commit nagging.** The agent finishes one P-task, gets a \"should \
I commit now?\" reflex, and {title} has to keep saying \"no, keep going.\"\n\
2. **Post-compression task amnesia.** After context compression the agent \
remembers it just finished P2 but loses the macro state that P3..Pn are still \
pending.\n\
3. **Progress-report checkpointing.** The agent finishes a P-task, pauses, \
and asks \"should I continue with the next one?\" or \"I've made good \
progress, want to review before P4?\". TASKS.md checkboxes and worktree \
commits already make progress legible.\n\
\n\
## Rule 1 — Commit discipline during multi-step plans\n\
\n\
A **multi-step plan** here means: any task you decomposed into 2 or more \
sequential subtasks (P1, P2, ..., Pn — or numbered todos, or any equivalent). \
Once you are inside such a plan, the following rules apply until the plan is \
fully done:\n\
\n\
**Scope of \"commit\" in this rule.** Throughout Rule 1, \"commit\" means \
commits on the **main / default branch**. Commits on a worktree feature \
branch (`prd/<plan-id>`) are governed by Rule 3 and are explicitly allowed \
at every P-task boundary — they do NOT count as Rule 1 violations and do \
NOT need to be flagged as a conflict with this rule.\n\
\n\
- **DO NOT proactively propose `git commit` on main.** Not after P1, not \
after P2, not at any \"natural checkpoint\" you sense. The plan is the unit \
of work, not the individual P-task.\n\
- **DO NOT actually run `git commit` on main either**, except in the two \
cases below.\n\
- **You MAY commit on main only when:**\n\
  1. {title} explicitly asks for a commit in this turn, OR\n\
  2. You have just finished the **last** P-task in the plan (i.e. all items \
     in TASKS.md are checked) AND you have surfaced that the plan is complete \
     to {title}. When Rule 3 (worktree workflow) is active, this single \
     allowed commit on main takes the form of `git merge --no-ff` from the \
     worktree branch — see Rule 3 for the exact procedure.\n\
- **`git push` is always gated** — never push without {title}'s explicit \
  approval in the current turn, regardless of plan state.\n\
\n\
### What \"finished\" means\n\
\n\
Plan completion = all P-tasks checked in TASKS.md (see Rule 2) + build/tests \
run + a change summary surfaced to {title}. Until all three are true, the \
plan is not done; do not propose committing.\n\
\n\
### Edge cases\n\
\n\
- **Single-step task** (one bug fix, one rename, one config tweak): not a \
  multi-step plan, normal commit etiquette applies.\n\
- **{title} asks mid-plan, \"can you commit what's done so far?\"**: that's \
  Case 1 above — proceed.\n\
- **You hit a blocker that requires {title}'s input**: pause and ask via \
  AskUserQuestion (or plain text if that tool isn't available). Do NOT use \
  the blocker as an excuse to commit \"in case progress is lost.\" Resume \
  after the blocker resolves.\n\
- **You are about to do something destructive** (rebase, force-push, branch \
  deletion): stop and ask regardless of plan state. This rule does not \
  override the existing destructive-action confirmation requirement.\n\
\n\
## Rule 2 — TASKS.md as the durable macro plan\n\
\n\
Context compression flattens the conversational history. Recent actions \
(\"commit succeeded\") survive in high fidelity; macro state (\"P3..P10 are \
still pending\") gets summarized away. To survive compression, the macro \
plan lives on disk.\n\
\n\
**The contract:**\n\
\n\
- When you decompose a task into 2 or more subtasks, write the decomposition \
  to `<workspace_root>/TASKS.md` BEFORE starting P1.\n\
- After completing each P-task, update its checkbox in TASKS.md to `[x]`.\n\
- At the start of every turn (after a compression, or whenever you are \
  unsure of macro state), the active-plan regions are automatically \
  re-injected as a system-reminder by Fleet's UserPromptSubmit hook — but \
  you can also `Read` the file explicitly when you need it.\n\
- When your plan is fully complete, you may remove your plan's sentinel \
  block (or leave it for history — {title}'s call when committing). Do NOT \
  touch other plans' blocks.\n\
\n\
### Multiple plans in one TASKS.md\n\
\n\
A single workspace's `TASKS.md` may carry **several plans in parallel** — \
{title} can have you working on plan A while another agent (or another \
conversation) is mid-flight on plan B. Each plan lives inside its own \
sentinel pair, identified by a unique `id`:\n\
\n\
```markdown\n\
# TASKS\n\
\n\
<!-- fleet:prd:begin id=\"auth-refactor\" v=\"2\" -->\n\
\n\
**Plan:** Migrate session middleware to the new auth crate\n\
\n\
- [x] **P1** — Audit existing call sites\n\
- [ ] **P2** — Swap middleware impl\n\
- [ ] **P3** — Update integration tests\n\
\n\
<!-- fleet:prd:end id=\"auth-refactor\" -->\n\
\n\
<!-- fleet:prd:begin id=\"prd-multiplan\" v=\"2\" -->\n\
\n\
**Plan:** Teach TASKS.md to host parallel plans\n\
\n\
- [ ] **P1** — New sentinel format with `id=\"...\"`\n\
- [ ] **P2** — Hook scans all blocks and re-injects each\n\
\n\
<!-- fleet:prd:end id=\"prd-multiplan\" -->\n\
```\n\
\n\
The `v=\"2\"` attribute on the `begin` sentinel marks the **v2 schema**. The \
`end` sentinel needs only the matching `id`. Legacy v1 blocks (no `v=\"2\"`) \
still work; run `fleet plan migrate` to upgrade an old TASKS.md in place.\n\
\n\
**Rules for working with multi-plan TASKS.md:**\n\
\n\
1. **Pick a unique `id` for your plan.** Use kebab-case, ≤ 32 chars, \
   describing the work (e.g. `auth-refactor`, `import-cleanup`). Before \
   creating a new plan, `Read` TASKS.md and confirm no existing block uses \
   the same id.\n\
2. **Only edit your own block.** When you tick a checkbox or revise your \
   plan, modify only the lines between *your* `begin id=\"X\"` and \
   `end id=\"X\"`. Treat every other block as read-only — it belongs to \
   another plan that may be in flight.\n\
3. **Match the id on both sentinels.** `begin id=\"X\"` must be paired with \
   `end id=\"X\"`. Mismatched ids will be ignored by the hook.\n\
4. **Legacy unmarked blocks are still recognised.** A bare \
   `<!-- fleet:prd:begin -->` / `<!-- fleet:prd:end -->` pair (no `id=`) is \
   treated as a single anonymous plan for backwards compatibility. Don't \
   create new ones in this form — always use an explicit id.\n\
5. **Don't merge or reorder other people's plans.** If two blocks look \
   redundant, surface that to {title} rather than collapsing them yourself; \
   the other block may belong to a session you can't see.\n\
\n\
### Update plans with `fleet plan`, not by hand-editing\n\
\n\
Prefer the `fleet plan` subcommands over editing TASKS.md markdown directly. \
They make the same file change **and** record which session is working which \
plan/P, with a timestamp — so the desktop app can show *your* current plan and \
P even when several sessions share one TASKS.md (Fleet knows your \
`FLEET_SESSION_ID`; you can't read the wall clock yourself). Commands:\n\
\n\
- `fleet plan create <id> --title \"...\" [--parent <parent-id>]` — add a new \
  v2 plan block **and** record this session as its executor. Creating a plan is \
  starting it, so no separate declaration is needed. Pass `--parent` when the \
  new plan is **side work you spun off mid-parent** (a detour you must return \
  from) — see \"Child plans & backtracking\" below.\n\
- `fleet plan check <id> <P>` — tick a task done (`[ ]`→`[x]`) and refresh this \
  session's focus onto `<id>`. e.g. `fleet plan check auth-refactor P2`.\n\
- `fleet plan uncheck <id> <P>` — untick.\n\
- `fleet plan resume <id> [P]` — take over an **existing** plan you did not \
  create (no file change; sets your current P, defaults to the first pending). \
  You do not need this after `create`, nor after a handoff — Fleet attributes \
  the successor for you.\n\
- `fleet plan add <id> <P> --text \"...\"` — append a pending task. Records no \
  focus: editing a plan's shape says nothing about who executes it.\n\
- `fleet plan migrate` — upgrade this workspace's v1 TASKS.md to v2 (idempotent).\n\
- `fleet plan list` / `fleet plan get <id>` — read.\n\
\n\
Hand-editing TASKS.md still works — the file is the source of truth for \
checkboxes — but it records no attribution, so the desktop app cannot tell \
which plan your session is on and shows nothing on your card.\n\
\n\
### Child plans & backtracking\n\
\n\
Mid-plan you sometimes have to spin off a **side branch** — a distinct chunk \
of work that must finish before the main plan can continue (a prerequisite \
refactor, a bug the current P depends on). Create it as a **child plan** so \
the detour doesn't strand the plan you came from:\n\
\n\
```\n\
fleet plan create <side-id> --title \"...\" --parent <current-plan-id>\n\
```\n\
\n\
This records `parent=\"<current-plan-id>\"` on the side plan's sentinel. When \
you tick the **last** box of that child with `fleet plan check`, Fleet walks \
up the `parent` chain to the nearest ancestor that still has pending P-tasks, \
**re-points your focus back onto it**, and prints a directive telling you the \
next P to resume. You do not run `fleet plan resume` yourself — just follow \
the directive and keep going; do NOT end your turn because the child finished. \
A backstop in the prd-context hook re-issues the same nudge every prompt if \
your focus is ever left on a completed child (e.g. the last box was \
hand-edited rather than ticked via `fleet plan check`).\n\
\n\
Children may nest (a child can have its own child) and the walk skips \
already-complete ancestors, so backtracking always lands on the nearest \
unfinished work up the tree. A plan with no `--parent` is top-level: \
completing it backtracks nowhere and the plan is simply done.\n\
\n\
Rules of thumb for the format itself:\n\
- Use `- [ ]` for pending and `- [x]` for done (`fleet plan check/uncheck` \
  write these for you). Don't invent new statuses; the simple checkbox is the \
  contract — \"who is working what right now\" is tracked by Fleet, not by a \
  marker in the file.\n\
- Keep P-task titles ≤ 60 chars. Long acceptance notes go in sub-bullets.\n\
- {language_line}\n\
\n\
### Multi-source scan across worktrees\n\
\n\
Because Rule 3 develops plans inside `.worktrees/<task-id>/` checkouts, the \
prd-context hook scans **every** TASKS.md it can find for the repo on each \
prompt — the main checkout's `<repo>/TASKS.md` plus every \
`<repo>/.worktrees/*/TASKS.md` that exists — and merges the active plans \
from all of them into a single injection. This works the same whether the \
session's cwd is the main checkout or one of the worktrees, so a worker \
agent running inside a worktree still sees plans living in the main \
checkout (and vice versa).\n\
\n\
**Dedup rule:** when the same `id=\"X\"` appears in more than one TASKS.md \
file, the hook keeps the version from the file whose mtime is most recent \
and drops the rest. The rendered plan header carries a `— source: <path>` \
suffix when the block came from a worktree TASKS.md, so the agent can tell \
which file to edit. Anonymous (legacy unmarked) blocks are kept independently \
per file — they pre-date the multi-plan format.\n\
\n\
**Therefore: keep a given `id` in exactly one TASKS.md file.** Copying the \
same id-tagged block from the main checkout into a worktree (or between two \
worktrees) creates a phantom plan that flickers based on whichever file you \
saved last. If a plan needs to live in a worktree for any reason, delete \
it from the main TASKS.md first.\n\
\n\
### Keep TASKS.md out of git\n\
\n\
TASKS.md is scratch state for the agent — it doesn't belong in version \
control. The first time you create `TASKS.md` in a workspace (i.e. it didn't \
exist before this turn), check whether it's already covered by `.gitignore` \
and, if not, **mention it to {title} and offer to add a `TASKS.md` line \
to `.gitignore`**. Do not silently rewrite `.gitignore` — surface the \
suggestion and let {title} approve. On subsequent edits to an existing \
TASKS.md, no reminder is needed.\n\
\n\
## Rule 3 — Worktree-based feature workflow\n\
\n\
**Any change that touches production code MUST be developed inside an \
isolated git worktree** at `<repo-root>/.worktrees/<task-id>` on a fresh \
branch `prd/<task-id>` — **regardless of whether the work is a multi-step \
plan (P1..Pn) or a single mechanical change**. Rule 3 is global; it is NOT \
gated by Rule 1's multi-step plan definition. For multi-step plans, \
`<task-id>` is the same id you picked for the TASKS.md sentinel block \
(Rule 2); for single-step changes, pick a short kebab-case identifier on \
the spot (e.g. `fix-zombie-pid`, `rename-task-fields`).\n\
\n\
**The contract:**\n\
\n\
- **Before touching any production code**, create the worktree on a fresh \
  branch based on the current main:\n\
\n\
  ```\n\
  git worktree add -b prd/<task-id> .worktrees/<task-id> main\n\
  ```\n\
\n\
  All code work runs inside this worktree; the main checkout stays clean \
  throughout. The final P-task (or, for a single-step change, the finishing \
  move) is the merge back to main.\n\
- **Intermediate commits inside the worktree are explicitly allowed and do \
  NOT violate Rule 1.** Rule 1's \"no proactive commit\" applies to *main*; \
  commits on `prd/<task-id>` inside the worktree are progress markers that \
  no other work can see. Commit between P-tasks (or split a single-step \
  change into several commits) whenever it helps you reason about the next \
  step, e.g. `git diff HEAD~1` when a later change regresses behaviour. You \
  still do not need to *ask* {title} for permission to commit inside the \
  worktree — it's free movement on a private branch.\n\
- **The work ends with one atomic merge back to main** (the final P-task, \
  or the finishing move for a single-step change). From the main checkout:\n\
\n\
  ```\n\
  git merge --no-ff prd/<task-id>\n\
  ```\n\
\n\
  The `--no-ff` is mandatory. `--ff-only` and `--squash` are forbidden — we \
  keep every worktree commit visible in main's history alongside a single \
  merge commit summarising the change, so the work stays auditable at \
  per-commit granularity. This `git merge --no-ff` IS the single \
  Rule-1-allowed commit on main; do not run any additional `git commit` \
  before or after it.\n\
- **Before merging or removing the worktree, rescue gitignored / untracked \
  artifacts the plan generated.** `git merge --no-ff` only carries across \
  *committed* content. Anything matched by `.gitignore` — and any file you \
  never `git add`ed — is never committed, so it lives **only** inside the \
  worktree's working directory. `git worktree remove` then deletes that \
  directory along with those files, and because they were never tracked there \
  is no git object to recover them from: the data is gone for good. \
  `.gitignore` means \"don't put this in version control\", NOT \"don't keep \
  this\" — a generated dataset, a synthesized media file, a downloaded asset, \
  a captured log {title} might want, an `.env` produced during the work, are \
  all real data even though they're untracked. So before you remove anything, \
  run `git status --ignored` (and check plain untracked files) inside the \
  worktree. Routinely-regenerable dirs — `target/`, `node_modules/`, `dist/`, \
  `.next/`, anything a committed build script rebuilds from scratch — need no \
  rescue; skip them. But if the worktree holds a generated artifact that is \
  NOT trivially reproducible from committed code (no generation script was \
  committed, or the inputs are gone), STOP and surface it to {title} before \
  removal: should the file be copied out of the worktree to a safe location, \
  or should it actually be tracked (added to the merge, or removed from \
  `.gitignore`)? Do not `git worktree remove` until that's resolved — removal \
  is the irreversible step.\n\
- **After a successful merge, clean up.** First confirm the rescue check above \
  is done. Then run `git worktree remove \
  .worktrees/<task-id>` then `git branch -d prd/<task-id>`. If the merge \
  fails (conflict, post-merge build/test regression), resolve in place — do \
  NOT abandon the worktree, do NOT amend the merge commit, do NOT \
  `git reset --hard` to wipe the merge. Surface the situation to {title} and \
  resume after the blocker resolves.\n\
- **Do NOT push the worktree branch to a remote.** `git push` remains gated \
  by Rule 1: only {title}'s explicit approval, in the current turn. The \
  local merge to main is allowed by Rule 1 Case 2; pushing main is a \
  separate decision {title} owns.\n\
- **`.worktrees/` must be in `.gitignore`.** Treat it the same as TASKS.md: \
  the first time you create a worktree in this repo, check `.gitignore`; if \
  `.worktrees/` is absent, **mention it to {title} and offer to add a \
  `.worktrees/` line**. Do not silently rewrite `.gitignore`.\n\
\n\
### When Rule 3 does NOT apply\n\
\n\
Rule 3 covers any change that touches production code, **whether multi-step \
or single-step**. Single-step changes are NOT an excuse to skip the \
worktree — the whole point is that even a 50-line mechanical edit gets the \
same isolation. The actual exemptions are about *what* you're changing, \
not *how many steps* it takes:\n\
- Pure documentation changes (READMEs, docstrings, changelogs).\n\
- Configuration-only changes (CI YAML, dotfiles, `.gitignore` itself, \
  formatter configs).\n\
- Urgent hotfixes that must land on main before another in-flight worktree \
  completes — surface the hotfix to {title} first so {title} can decide \
  whether to pause the active worktree.\n\
\n\
## Rule 4 — Plan execution rhythm\n\
\n\
A multi-step plan is meant to be carried out in one continuous rhythm, not \
punctuated by mid-plan reporting checkpoints. The unit of progress is the \
*plan*, not the P-task — {title} can already see plan state via TASKS.md \
and (when Rule 3 is active) worktree commits, so explicit progress reports \
are redundant interruptions.\n\
\n\
**The rhythm.** Each non-final P-task follows the same three-step loop, \
then immediately continues to the next P-task **in the same turn** without \
pausing for {title}'s confirmation:\n\
\n\
1. **Dev** — make the code changes the P-task calls for.\n\
2. **Test / verify** — run the appropriate validation (unit tests, \
   `cargo build`, `pnpm build`, Playwright, type check, lint, hand-exercise \
   the UI — whatever the P-task requires).\n\
3. **Commit inside the worktree** — when Rule 3 is active, record the \
   P-task as a commit on `prd/<plan-id>` so later P-tasks have a clean \
   reference point. Outside Rule 3 (e.g. config-only plans), this step is \
   skipped.\n\
\n\
After step 3, tick the checkbox with `fleet plan check <plan-id> <P>` — not by \
hand-editing TASKS.md — and **proceed to the next P-task immediately**. The \
`check` is what keeps your session attributed to this plan; a hand-edited \
checkbox leaves the desktop card blank. Do NOT pause to summarise. Do NOT ask \
\"should I continue with P2?\" or \"want to review progress before P4?\". Do \
NOT offer \"I've written quite a few P-tasks now, want me to summarise?\". \
Those are exactly the proactive progress-report checkpoints Rule 4 exists to \
eliminate.\n\
\n\
**Attribution.** Fleet shows your current plan and P on the session card, but \
only when it can attribute your session to a plan. `fleet plan create` (you \
authored the plan) and a Fleet handoff (Fleet spawned you into it) attribute \
you automatically; `fleet plan check` refreshes it as you go. The one case \
needing an explicit claim is **picking up a plan you did not create and were \
not handed**: run `fleet plan resume <plan-id> [P]` before your first P-task.\n\
\n\
### When the rhythm DOES pause\n\
\n\
The rhythm pauses ONLY for one of these four cases. \"I've done a lot, want \
to check in?\" is NEVER one of them.\n\
\n\
1. **The final P-task's acceptance gate.** Rule 3's plan ends with \
   `git merge --no-ff prd/<plan-id>`. Before running the merge, surface a \
   \"ready to merge\" summary to {title} and wait for explicit go-ahead. \
   This merge IS the plan's acceptance moment; do NOT solicit acceptance at \
   intermediate checkpoints.\n\
2. **A genuine direction-of-work question.** Something where {title}'s \
   judgement is required because there's a real fork in the road — \"keep \
   backwards compatibility for X or drop it?\", \"delete or archive this \
   data?\", \"API design A vs B?\". Quote the choice and the trade-offs; \
   that's a clarifying question, not a progress report.\n\
3. **A test/verify red light that survives one repair attempt.** The first \
   time `cargo build` / unit tests / Playwright / hooks fail inside a \
   P-task, you MAY try ONE round of diagnosis-and-fix. If that round \
   doesn't restore the green light, OR if the root cause is unclear before \
   you start, stop and surface as a blocker — do NOT enter a fix → retry → \
   fix → retry loop without {title}.\n\
4. **A destructive operation** (rebase, force-push, branch deletion, \
   dropping a migration, `git reset --hard`). The existing destructive-\
   action confirmation requirement still applies; Rule 4 does not override \
   it.\n\
\n\
## Rule 5 — Long-context handoff (`fleet handoff`)\n\
\n\
When your context window is running long mid-plan (context usage high, or \
compaction has already fired), do NOT grind on until the window dies, do NOT \
silently wrap up early, and do NOT leave \"hand off to the next session\" \
notes that nothing acts on. Fleet has a first-class relay:\n\
\n\
```\n\
fleet handoff --note \"<交接信息>\" [--plan <plan-id>] [--next <P>] [--model <model>] [--effort <effort>]\n\
```\n\
\n\
- **--note is mandatory** and is everything the successor knows beyond \
TASKS.md: what's done, what's in flight, key files, gotchas, the next \
concrete step. Write it like a shift-change briefing.\n\
- **Pass --plan/--next when the work is a TASKS.md plan** so Fleet attributes \
the successor to that plan and P automatically; it resumes the rhythm there \
without any `fleet plan` ceremony of its own.\n\
- **--model / --effort are optional** — pin the successor's model (e.g. \
`claude-opus-4-8[1m]`, bracketed suffix passed through verbatim) and reasoning \
effort (low|medium|high|max), overriding the values otherwise auto-inherited. \
Omit them to keep the current session's model and CLAUDE_EFFORT.\n\
- **Then finish the turn cleanly**: commit worktree progress per Rule 3 \
first, then stop. The moment you yield, Fleet's Stop hook consumes the \
registration and spawns a fresh session in the same workspace whose opening \
prompt is your note; the prd-context hook re-injects the TASKS.md macro \
plan automatically.\n\
- **The relay is recorded** as a handoff chain and shown on session cards \
(接力 n/N), so {title} can trace the whole sequence afterwards.\n\
- A new user prompt to your session cancels your pending handoff — {title} \
taking over always wins. Chains are capped at 10 hops; re-registering \
overwrites your previous note.\n\
\n\
The moment you catch yourself thinking \"I should wrap up because context \
is getting long\" — that impulse IS the signal. Register the handoff and \
relay instead of wrapping up.\n\
\n\
**Narrating a handoff is NOT registering one.** Writing \"接下来我起下一棒\" \
/ \"handing off to the next session\" / \"I'll relay the rest\" in your reply \
text does NOTHING: Fleet's Stop hook consumes a *registration*, not a \
sentence. If you did not actually run the `fleet handoff` Bash command this \
turn, no successor spawns and the plan dies silently the moment you yield. \
So the LAST thing you do before ending \
such a turn is the tool call itself: run `fleet handoff --note \"...\"`, wait \
for the `ok: handoff registered` result to come back, and only then stop. \
Never let a turn end with the handoff living only as prose, and never reach \
for ScheduleWakeup / `/loop` / cron to \"continue later\" — those silently \
no-op inside a Fleet session and spawn nothing. Two Fleet relays actually fire \
across the turn boundary: `fleet handoff` for *continuing the work* (hand a \
briefing to a fresh successor), and `fleet watch` for *waiting on an external \
condition* — a CI run finishing, a build producing an artifact, a deploy going \
live. Do NOT sit in a foreground `Monitor` / background `Bash` waiting for such \
an event: they die the instant the `-p` turn ends and their notification never \
arrives. Instead run `fleet watch create --until '<shell cmd that exits 0 when \
done>' --capture '<shell cmd whose stdout you want reported>' --note '<what you \
are waiting for>'`, then end the turn — Fleet polls in the background and \
`claude --resume`s THIS session the moment the condition fires, feeding the \
captured result to your next turn. `fleet watch stop <id>` cancels it.\n\
\n\
## Recommended tooling for the worktree workflow\n\
\n\
Because Rule 3 develops every plan inside a fresh worktree, each new plan \
is effectively a clean checkout — including the dependency tree. Tooling \
that stores packages **per project** (npm's `node_modules/`, pip's per-venv \
site-packages, yarn classic's `node_modules/`) re-downloads and re-installs \
everything for every worktree, wasting disk and install time. Tooling with \
a **global content-addressed cache** shares one copy across all worktrees \
of all projects, so spinning up a new worktree costs seconds, not minutes.\n\
\n\
These are *recommendations*, not hard rules — they're not Rule 5. If \
{title} explicitly picks a different tool for a specific project, follow \
that. Recommendations only kick in when {title} has not already made the \
choice.\n\
\n\
**For new projects, prefer the worktree-friendly choice:**\n\
\n\
- **Node / TypeScript**: prefer **pnpm** (global store at \
  `~/.local/share/pnpm/store`, symlinked into each project's \
  `node_modules/`) over npm or yarn classic. Bun also uses a global cache \
  and is fine; npm and yarn classic are the ones to avoid for \
  worktree-heavy work.\n\
- **Python**: prefer **uv** (global cache + hardlinked venv contents) \
  over `pip + venv`. Poetry is acceptable if cache sharing is left on, but \
  uv is noticeably faster on worktree spin-up.\n\
- **Rust**: `cargo` already shares `~/.cargo/registry` globally, so no \
  extra action is needed. Each worktree's `target/` stays per-worktree by \
  design — that's a deliberate trade-off to avoid lock contention; do NOT \
  try to share `target/` across worktrees.\n\
- **Go**: `go` already shares `$GOMODCACHE` and `$GOCACHE` globally; \
  worktrees cost ~nothing on the dependency side. No action needed.\n\
\n\
**For existing projects, do NOT silently migrate the lockfile or package \
manager just because you're about to create a worktree.** A \
`package-lock.json` repo stays on npm until {title} agrees to the switch. \
Switching package managers is itself a separate plan with its own scope, \
its own worktree, and its own acceptance gate — surface the \
cost-vs-migration trade-off to {title} before touching the lockfile.\n\
\n\
## When this mode does NOT apply\n\
\n\
Rule 3 (worktree) is **global** for any change to production code. Rules \
1, 2, and 4 are scoped to multi-step plans. So:\n\
\n\
- **Single-step production-code change**: Rule 3 applies (worktree + \
  merge `--no-ff` back to main). Rules 1, 2, 4 do NOT — no TASKS.md, no \
  P-tasks, no rhythm enforcement. The whole change happens in the \
  worktree as a single mechanical edit, then merges back.\n\
- **Pure conversation / Q&A turns where no code is changing**: none of \
  the four rules apply. Reply in plain text.\n\
- **Pure documentation, configuration, or hotfix work** (see Rule 3's \
  own \"NOT apply\" subsection): all four rules are off unless {title} \
  explicitly asks to treat the work as a multi-step plan.\n\
- **{title} explicitly asks to keep the work \"informal\" or \"quick\"**: \
  all four rules are off; normal commit etiquette applies and {title} \
  is taking responsibility for the lighter process.\n\
\n\
## Interaction with other modes\n\
\n\
- This mode is **independent of** Fleet Interaction Mode. They can be \
  enabled separately.\n\
- The Bash guard hook (if installed) still runs and may still ask {title} \
  to confirm risky commands. That's by design — guard catches risk; this \
  mode catches *unnecessary* commits.\n\
",
        title = title,
        language_line = language_line,
    )
}

/// Apply PRD-discipline mode: write the guidance file and inject the
/// `@import` sentinel block into `~/.claude/CLAUDE.md`. Idempotent.
pub fn apply_prd_discipline(user_title: &str, locale: &str) -> Result<(), String> {
    let dir = claude_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create ~/.claude: {e}"))?;

    let guidance_path = guidance_file_path().ok_or("cannot determine home dir")?;
    let guidance = render_guidance(user_title, locale);
    fs::write(&guidance_path, guidance).map_err(|e| format!("write guidance file: {e}"))?;

    let claude_md = claude_md_path().ok_or("cannot determine home dir")?;
    let existing = fs::read_to_string(&claude_md).unwrap_or_default();
    let block = format!(
        "{begin}\n@{path}\n{end}\n",
        begin = BEGIN_MARKER,
        end = END_MARKER,
        path = guidance_path.display(),
    );
    let new_content = compose_claude_md(&existing, &block);
    fs::write(&claude_md, new_content).map_err(|e| format!("write CLAUDE.md: {e}"))?;
    Ok(())
}

/// Re-attach the `@import` sentinel block to CLAUDE.md content: strip any prior
/// block, then append `block` separated by one blank line.
fn compose_claude_md(existing: &str, block: &str) -> String {
    let stripped = strip_sentinel_block(existing);
    if stripped.trim().is_empty() {
        block.to_string()
    } else {
        // Trim trailing newlines the strip left behind, then re-add exactly one
        // blank-line separator. Without the trim, re-applying accumulates a
        // blank line each time (strip leaves the prior separator in place).
        format!("{base}\n\n{block}", base = stripped.trim_end_matches('\n'))
    }
}

/// Remove PRD-discipline mode: strip the sentinel block and delete the
/// guidance file. Idempotent.
pub fn remove_prd_discipline() -> Result<(), String> {
    if let Some(claude_md) = claude_md_path() {
        if let Ok(existing) = fs::read_to_string(&claude_md) {
            let stripped = strip_sentinel_block(&existing);
            if stripped != existing {
                fs::write(&claude_md, stripped).map_err(|e| format!("write CLAUDE.md: {e}"))?;
            }
        }
    }
    if let Some(path) = guidance_file_path() {
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("remove guidance file: {e}"))?;
        }
    }
    Ok(())
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

fn strip_sentinel_block(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_block = false;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == BEGIN_MARKER {
            in_block = true;
            continue;
        }
        if trimmed == END_MARKER {
            in_block = false;
            continue;
        }
        if !in_block {
            out.push_str(line);
        }
    }
    while out.ends_with("\n\n\n") {
        out.pop();
    }
    out
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
}
