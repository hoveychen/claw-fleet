# 正文 [?…] 可点击标注：让 agent 自己标出「欠解释处」，点击即展开追问

子计划 `explain-annotations`（explore，parent=`selection-explain`）。v1 选区追问已合并 main `5c47ee0d`，本文只盘点方案，不改生产代码；结论拆成 exec 子计划呈报拍板。

## 1. 需求溯源

老板原话（第 1 棒 transcript `86c80432…`，2026-09-21 00:14，逐字）：

> 这个功能我希望还能做进一步扩展，要能**要求 agent 在输出回答时**，对于它觉得可能欠解释或者用户很有可能要进一步提问的地方，做特殊的格式标注，从而允许用户点击某行标注文本，不输入问题就能展开了解更多信息。

同一条消息里老板附的截图是一个 **Chat 会话**（`~/.fleet/chat`，Claude Opus 5）的 assistant 正文，老板用红框圈出了一句话。第 1 棒决策卡上老板选「拆成子计划，v1 先跑通」；第 7 棒合并卡上选「开 [?…] 标注子计划」。

### 三分需求清单

| 条目 | 来源 |
|---|---|
| agent **在输出回答时**就对欠解释 / 用户很可能追问的地方做格式标注 | 明说 |
| 判定者是 agent 自己（「它觉得」） | 明说 |
| 用户点击标注文本即展开更多信息，**不用输入问题** | 明说 |
| 展开 = 复用 v1 的 fork 单轮解释（不污染主对话、命中缓存）而不是新会话 | 推导（v1 的两条硬约束继承） |
| 标注在 Chat 会话里必须生效（截图场景） | 推导（截图） |
| 三种 harness（Claude / codex / dsh）都要 | 推导（CLAUDE.md 三端原则 + v1 已拍板三源） |
| 桌面 / 移动端都渲染可点击；决策卡正文也是「agent 给我的文本」，同样渲染 | 推导 |
| 标注语法在其他渲染端（终端、纯 markdown）退化为可读原文 | 推导（agent 输出不只 Fleet 在读） |
| 标注可附带 agent 预写的追问句（`[?文本|问题]`），点击时用它代替默认「解释」 | **我加的，待拍板** |
| 对没有标注的历史回复，提供「给这条打标注」按钮走事后 fork 补标 | **我加的，待拍板** |
| 标注密度上限 / 每条回复最多 N 处 | **我加的，待拍板** |

上一棒交接 note 把「谁判定」列成「fork 让模型产 JSON / 本地启发式 / 混合」三条路，**漏了老板原话直接指向的那条**：主 agent 在原回复里内联标注。下面把它列为路线 A 并作为推荐。

## 2. 四条路线盘点

| | A 内联标注（老板原话） | B 事后 fork 产标注 | C 本地启发式 | D 混合（A 主 + B 按需） |
|---|---|---|---|---|
| 谁判定 | 主 agent 自己，在写回复时 | 同一会话的 fork（复用 `session_explain::ask` 机制），输出 JSON 列表 | 前端正则：术语、标识符、数字、`code` | A 为主；无标注的旧回复由 B 补 |
| 触发 | 无需触发，回复自带 | 每条 assistant 消息落地后自动，或老板手动「标注这条」 | 渲染时 | A 自动；B 手动 |
| 额外成本 | 每条回复多几十～百来个**输出 token**，零额外请求 | 每次一个完整 fork：Claude 活跃会话 $0.07～0.14（v1 实测 read 108K～180K），冷会话 $1.2～1.6；codex 热 $0.17 / 冷 $1.67；dsh ≈ $0.002。自动触发 = 每回合一次 | 0 | A 的成本 + 按需的 B |
| 延迟 | 0（与回复同时到） | 10～20 秒（v1 实测 8～20 秒） | 0 | 0 / 按需 |
| 判定质量 | 主 agent 最清楚自己哪里省略了推理；但靠 guidance 遵从，会漂（漏标 / 过标 / 忘了） | 专注任务、可要求结构化输出；但读的是成品文本，不知道作者省了什么 | 命中的是「形式」不是「欠解释」，噪音大 | 取 A 的质量，B 兜底 |
| 覆盖范围 | 只有 guidance 落地后的**新回复** | 任意历史回复 | 全部 | 全部 |
| 锚定 | 标记就在正文里，渲染时直接变节点，**不需要引文匹配** | 引文子串 → 正文匹配（复用 `shared-ts/sessionExplain.ts` 的 `selectQuoteIn` 折叠空白算法），跨 code / 加粗节点时会漏 | 同 A | A 部分免匹配 |
| 三 harness | 各写一段 guidance（见 §4） | 三源 fork 后端 v1 已齐 | 无关 harness | 两者之和 |
| 对非 Fleet 读者 | 正文里多出 `[?…]` 符号 | 无影响 | 无 | 同 A |

**推荐：先做 A（最薄竖切，就是老板原话），B 作为「我加的」可选子计划单独拍板，C 不做。**理由：A 的边际成本是几十个输出 token，B 自动触发意味着每回合再花一次 fork 的钱和 10 秒以上延迟，而且 B 读成品文本判断「欠解释」本身就不如作者自标。C 与「欠解释」语义无关。

## 3. 标注语法选型

用 remark-parse 11 + remark-gfm 4（两端渲染链的真实版本）做了 tokenization 探针（2026-09-21，临时脚本，未入库）：

| 写法 | remark 行为 | 结论 |
|---|---|---|
| `[?AUROC 纹丝不动] 的意思` | 纯 text 节点 | ✓ 安全 |
| `[?第二个](后面有括号)` | 变成 link，url=`后面有括号` | ✗ 标注后紧跟**半角** `(` 会被吃成链接；全角 `（` 安全 |
| `[?引用] [x]` / `[?a][auroc]` + 定义 | 前者纯文本；后者变 linkReference | 仅有定义时才触发，实际回复里极少 |
| `[?带 \`code\` 的]` / `[?标注**加粗**混合]` | 被切成 text + inlineCode/strong + text 三个节点 | 标注内含行内格式时**不能**在渲染后的字符串层做替换，要在 mdast 层做 |
| `{?花括号}` / `==高亮==` | 纯文本 | 也安全，但没有 `[?…]` 直观 |
| `<q data-why="x" class="fleet-q">…</q>` / `<span class data-q>` | rehype-raw 保留标签，**rehype-sanitize 默认 schema 剥掉 class 与所有 data-\*** | 走 HTML 要改两端的 sanitize schema，且 `<mark>`/`<abbr>`/`<u>` 整个标签被删 |

**选 `[?…]`**：老板原话用的就是这个记号，纯 markdown 里退化成「[?原文]」可读，不依赖 rehype-raw，不用碰 sanitize schema。两条护栏写进 guidance：标注后不要紧跟半角 `(`；标注内不要嵌套 `code`/加粗（或由插件在 mdast 层容忍）。

可选扩展（我加的）：`[?文本|追问句]`，竖线后是 agent 预写的问题。GFM 表格单元格里 `|` 会切列，所以表格里禁用扩展形式。

## 4. Guidance 载体（路线 A 的落点）

Fleet 现有的 guidance 分四路，标注指令每路都要落，缺一路那种会话就没有标注：

| 会话类型 | 载体 | 现状与约束 |
|---|---|---|
| Claude 工程会话 | `~/.claude/fleet-*.md`（control plane 的 11 个 Feature 之一，或并入 `interaction_mode.rs`） | 新建独立 Feature 要改 `control_plane_prefs.rs` / `control_plane.rs::apply` / `is_installed` / 底层 apply+remove 四处（见记忆 adding-a-control-plane-feature-surfaces）；并入 interaction-mode 最省事但语义上不属于「决策卡纪律」，且关掉交互模式就丢标注 |
| **Claude Chat 会话（截图场景）** | `~/.fleet/chat/CLAUDE.md`，`chat_workspace.rs::CHAT_CLAUDE_MD` + `chat_claude_md()` 的 `installed_section` 拼接模式 | Chat 用 `--setting-sources project` **排除全局 `~/.claude/*.md`**，所以上一行的载体到不了这里，必须像 session-title 那样单独追加一段 |
| codex | `~/.codex/AGENTS.md` 的 fleet 哨兵块（`codex_guidance.rs`） | 文件已 **31,549 B，上限 32 KiB**（`project_doc_max_bytes`），余量约 1.2 KB；标注指令要压到 ~300 字，或从 interaction 块里挤空间 |
| dsh | 启动注入 `dsh_guidance::render_dsh_sections`（无落盘）+ Chat 走 `dsh_chat_preset` | 加一个 section 即可；dsh chat preset 丢掉全局 AGENTS.md 只留项目 `CLAUDE.md`，所以 Chat 那段与 Claude 共用 `~/.fleet/chat/CLAUDE.md` |

指令本体一段（中英各一版，`locale` 分支），要点：只标「我做了取舍 / 用了术语 / 给了结论没给推导」的地方；一条回复 3～5 处以内；只包一个短语或一句话；不要在 code、表格、标题、链接文本里标；不要紧跟半角括号。生成式 guidance 落盘后需要 explicit refresh（桌面端 App 挂载时会自动重写；`fleet webui` 走 `control_plane::heal` 的 stale 比对）。

## 5. 渲染与点击（三端）

- **解析**：写一个 remark 插件 `remarkExplainMarks`，模板是 `claw-fleet-desktop/app/markdown/wikiLinks.tsx` 的 `remarkWikiLinks`（在 mdast 上切 text 节点、发出自定义节点）。放 `shared-ts/`，desktop 与 mobile 的 remark 链各挂一次。跳过 `link` / `inlineCode` / 表格单元格的扩展形式。
- **节点 → 元素**：插件产出的节点经 remark-rehype 变 `<span class="explain-mark" data-q="…">`；自定义节点走 `data.hName`/`hProperties`，不经 rehype-raw，所以 sanitize 不会剥（它只剥 raw HTML；但 `className` 与 `data-*` 是否被 schema 放行要在实现里核对——两端 schema 已给 `span` 加过 math 的 className 白名单，照样扩一项）。
- **点击**：桌面 `TextBlock` / 移动 `LazyMarkdown` 的 `components.span`（或直接 `explain-mark` 元素）绑 onClick → 调 v1 的 `ask({ preset: Explain 或 Custom(question), quote: 标注文本, anchor: 所在行的 msg_uuid/msg_idx })` → 记录出现在辅助栏 / 追问页签，流式同 v1。已有解释记录的标注显示为「已解答」态，再点直接跳到那张卡（`locateExplainRow`）。
- **样式**：下划虚线 + 悬停问号；不改变文字本身；深浅主题各一色。
- **决策卡**：`DecisionPanel.tsx` 的 question 正文与 mobile 决策卡同样用 `mdComponents`，挂同一个插件即可覆盖；点击的 anchor 是决策卡而不是 transcript 行，v1 的 `ExplainAnchor` 两个字段都是 Option，可以为空。
- **搜索高亮**兼容：`TextBlock` 现有的 `highlightChildren` 在 p/li/td 的 children 上做字符串替换，标注 span 是元素节点，不受影响。

## 6. P2 真机探针（2026-09-21 00:06）

目标：老板正在跑的 Chat 会话 `bd0af768`（Opus 5，transcript 一分钟内刚写过，真实上下文 255K），走 `session_explain::ask` 的真实代码路径（preset Custom）各 fork 一次。临时 example 已删，两条记录留在 `~/.fleet/explain/bd0af768-e258-4fe7-a9ef-34fa8994a81d/`（`343197f6…` 与 `65dc82de…`）。

| 探针 | 问题 | cache_read | cache_create | output | 费用 | 耗时 | 结果 |
|---|---|---|---|---|---|---|---|
| A 路线 A 模拟 | 一字不改重写上一条回复，按护栏加 `[?…]`，最多 5 处 | 257,545 | 2,798 | 1,256 | $0.188 | 24.6 s（流式） | 5 处标注，全部守规则 |
| B 路线 B | 列出 3～5 处欠解释，只输出 JSON `[{quote, why}]` | 258,785 | 1,489 | 372 | $0.154 | 10.9 s | 5 条 JSON |

**A 的标注落点**（原文是一段实验订正汇报）：`acquiescence bias`（术语）、`是减掉一个负偏置造出来的`（省略的推理）、`单类集合上挑错的代价是 40 个百分点`（只给数字没给来源）、`AUROC 只动了 0.004`（反直觉结论）、`算被打分 token 占多少概率质量`（方法名词）。人眼看五处都确实是读者会停下来的地方，没有标 markdown 结构、没有嵌套 code / 加粗、没有紧跟半角括号。去掉标记后与原文 diff 只差一个空格（模型在「而」和 `[?` 之间补了一个中英分隔空格）。

**B 的 quote 匹配**：5 条里 4 条能在原文逐字找到；`4B-p4-cc 拿到全场最高 AUROC 0.960` 因原文是 `**0.960**` 加粗而逐字不匹配，折叠 markdown 标记后才命中。这正是 §2 说的「引文匹配跨格式节点会漏」，路线 B 的锚定要在渲染后的纯文本上做（`selectQuoteIn` 已经是这么做的）。

**成本口径**：A 在这里花 $0.19 是因为让它整段重写（1,256 output token）；真实路线 A 是主 agent 写回复时顺手加标记，增量只有标记本身，5 处约 20～40 个 output token，不产生额外请求。B 是每次一个完整 fork：暖会话 $0.15 + 11 秒，冷会话（>1 小时）要先付一次全量 cache write，255K 上下文约 $4。**自动对每条回复跑 B 不可接受；B 只适合作为老板手动「给这条打标注」的补标手段。**

### 子计划 `explain-marks-guidance` P5：三 harness 遵从率（2026-09-21 00:20～00:35）

| harness | 规则怎么到模型 | 模型 | 标注 | 观察 |
|---|---|---|---|---|
| Claude | prompt 内嵌（P2 探针 A） | Opus 5 | 5/5 守规则 | 见上 |
| dsh | prompt 内嵌，fork `session-5de621d4`（`dsh://`，父模型 haiku 4.5 via openrouter，对齐成功） | claude-haiku-4.5 | 4 处，全守规则 | 落点：`hashing the prefix of your request`、`intermediate representations`、`no longer matching the parent's cached entry`、`Omitting a flag or setting it to a default is not equivalent`；冷缓存 write 59K，11 秒；记录 `9f56c84d…` |
| codex | **真实注入路径**：隔离 `CODEX_HOME` 里 `AGENTS.md` 含 compact 节（第 130 行），`codex_guidance_e2e spawn` | gpt-5.6-luna / low | **0** | 同一份 AGENTS.md 的其他规则被遵从（称「老板」、中文作答），唯独这一节被忽略；input 17.6K，cached 6.9K |
| codex | 同上 | gpt-5.6-sol / medium | **0** | 同样称「老板」、中文，无标注；input 19.3K |

**codex AGENTS.md 预算**：全量 install 到隔离 `CODEX_HOME`（五块 + 7 条 lessons）：带完整英文节 32,504 B（余 264 B）；换 compact 节后 32,144 B（余 624 B）；标题改命令式后 32,186 B（余 582 B）。lessons 块现用 3.7 KiB，其自身上限 6 KiB，所以 lessons 再涨约 0.6 KiB 就会撞 codex 的 32 KiB 上限——这是既有临界状态，本节把余量从 1.2 KB 压到 0.6 KB。

| codex | 规则**内嵌 prompt**，AGENTS.md 不变 | gpt-5.6-sol / medium | **2 处，守规则** | `prefix matching is effectively byte-for-byte`、`the fork can cost almost as much as starting an independent session`；input 39.2K |

| codex（研究 1） | AGENTS.md 完整送达确认：rollout 里 `<environment_context>` user 消息 28K 字符含本节哨兵，**未截断** | — | — | 不是截断问题 |
| codex（研究 2） | 节剪到 AGENTS.md **最顶部**作独立块 + 标题加「you MUST do this in every prose reply」 | gpt-5.6-sol / medium | **3 处，守规则** | `The provider then charges the lower cached-input rate…`、`the entire inherited context becomes an uncached input again`、`the cost increase comes mainly from repeatedly processing the shared input prefix` |
| codex（研究 3，分离变量） | **原位置不动**（interaction 块中部），只把标题改成 MUST 措辞 | gpt-5.6-sol / medium | **2 处，守规则** | `prompt caching generally requires an exact match…`、`the duplicated history is billed at the ordinary input-token rate…` |

**研究结论（老板选「研究」后补）**：起作用的是**措辞强度**，不是位置——原位置只把标题改成「you MUST do this in every prose reply」就从 0 变 2。中性标题的一段说明文被 codex 当成背景知识而非指令。修法已落到 `explain_marks_guidance.rs`：三个变体（zh / en / compact）的标题统一改成命令式，不需要动 prompt-prepend 通道，AGENTS.md 余量不变。

**此前的结论（保留作记录）**：codex 不是不会标，是静态 AGENTS.md 里一段 600 字节的中性规则对它不起作用（同文件的称呼、语言规则都被遵从），规则进 prompt 就遵从。三个 harness 里只有 codex 的真实载体验过；Claude / dsh 的探针规则都在 prompt 里，它们的真实注入路径（`~/.claude/fleet-interaction-mode.md`、dsh 插件的 `fleet dsh-context`）要等合并后老板机器刷新 guidance 才看得到。**codex 的解法待拍板**：走已有的 prompt-prepend 通道（`codex_launch::maybe_prepend_active_plans` 每轮把 TASKS.md 提醒 prepend 到 prompt，同一处多带这一段），AGENTS.md 里的 compact 节可留作兜底或删掉换回 0.6 KB 余量。

## 7. 已拆出的 exec 子计划（parent=`explain-annotations`，见 TASKS.md）

- `explain-marks-guidance`：指令文案单一 owner → Claude 工程会话载体 → Chat brief → codex / dsh → 三 harness 真机遵从率。
- `explain-marks-render`：shared-ts remark 插件 → 桌面 TextBlock → 移动 LazyMarkdown → 决策卡正文 → 眼验与全绿。

两者互不依赖，可以并行开工；render 先做时用探针 A 的文本当 fixture，不用等 guidance 落地。§1 表里「我加的」三项没有建计划，等拍板。

### 实现结果（2026-09-21，第 8 棒，两个 worktree 并行）

**`explain-marks-guidance`**（分支 `prd/explain-marks-guidance`，2 提交）：
- `claw-fleet-core/src/explain_marks_guidance.rs` 单一 owner：`render_explain_marks_section(title, locale)`（zh 约 900 B / en 945 B）、codex 用的 `render_explain_marks_section_compact`（585 B）、`installed_section()` 从 `~/.claude/fleet-interaction-mode.md` 按哨兵 `<!-- fleet:explain-marks:begin/end -->` 抽回。
- 载体：`interaction_mode::render_guidance` 中英各嵌一节（放在「报告正文的写法」之后）；`chat_workspace::chat_claude_md` 追加 `installed_section()`；`codex_guidance` / `dsh_guidance` 的 interaction 块嵌英文。core lib 3087 测试全过。
- 真实注入路径只验了 codex，结果 0 标注（见 §6），待拍板改走 prompt-prepend。

**`explain-marks-render`**（分支 `prd/explain-marks-render`，6 提交，subagent 实现、本棒复验）：
- `shared-ts/explainMarks.ts`：`splitExplainMarks` / `stripExplainMarks` / `remarkExplainMarks`（text 节点切分，跳过 link / linkReference，`[?` 未闭合与空标注留原文），产出 `<span class="explain-mark" data-explain-quote>`；两端 sanitize schema 放行该 class 与属性。插件挂在两端**共享** remark 链上，所以所有 markdown 表面都会把 `[?x]` 显示为 `x`（wiki、handoff note 等无 provider 的地方是纯文本、不可点）。
- **点击语义（老板第二张卡上改的）**：第一版点击标注直接 fork 出解释卡，老板看截图说「没有让用户输入问题或点选 shortcut 的地方」，改为**点击 = 把标注文本设为选区并弹出 v1 的浮条**（解释 / 翻译 / 为什么 / 自定义提问），再点「解释」才 fork——仍是「不输入问题就能展开」，多一次点击换来选问题的机会。实现：`shared-ts/sessionExplain.ts::selectExplainMark` 程序化选中并派发 `fleet:explain-mark-select` 事件，桌面 `SelectionToolbar` / 移动 `SelectionAskBar` 监听该事件当作一次 mouseup 立即读选区（键盘触发没有 mouseup，所以不用 selectionchange 重读）。原来的「同 quote 已答则展开旧卡」快捷路径删掉，因为它会让部分点击不弹浮条。
- 可点判据：标注挂载后看自己是否落在 `[data-msg-idx][data-role='assistant']` 内（与 `readAssistantSelection` 同一条规则），user 行、wiki、handoff note、追问答案里的标注是纯文本。
- 决策卡：桌面 `DecisionPanel.tsx`（FleetAskCard / ElicitationCard）与移动 `DecisionsView.tsx` 的问题正文容器打上 `data-role="assistant" data-msg-idx`，各挂一个浮条实例；选 preset 后走 `DecisionExplainMarks.tsx` 的 ask（anchor 为空），答案**内联在问题下方**。副产品：决策卡正文也能框选追问。
- TTS / 一行摘要走 `stripExplainMarks`（桌面 `decisionText.ts`、移动 `decisionCall.ts`），朗读不会念出括号。
- 复验：desktop / mobile `tsc` 0 错，css-token 守门过，mobile 839 例过，desktop 标注相关 36 例过；mock 截图（深浅主题、点击前后）确认 transcript 标注下划虚线 + 点击出辅助栏卡、决策卡标注点击出内联答案。子代理报告的 `ChatComposer.pasteTable` 1 例偶红与本分支无关，单跑 3 次全过。

## 8. 老板拍板（2026-09-21，第 8 棒决策卡）

1. 路线：**只做 A**。不做手动补标按钮，路线 B / C 不建计划。
2. `[?文本|追问句]` 扩展形式：**不要**。点击一律走 preset Explain，quote = 标注文本。
3. Claude 工程会话的载体：**并入 interaction-mode 文件**（`interaction_mode.rs::render_guidance` 加一节），不建独立 Feature。Chat brief 仍单独追加同一段。
4. 标注密度：**写死 3～5 处**，guidance 明写「一条回复最多 5 处，宁缺毋滥」。

据此 §1 表里「我加的」三项全部作废；`explain-marks-guidance` P2 改为并入 interaction-mode。
