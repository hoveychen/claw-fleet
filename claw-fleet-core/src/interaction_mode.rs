//! Interaction Mode — injects a guidance block into `~/.claude/CLAUDE.md`
//! that steers Claude Code to route all terminal-level final output through
//! the `fleet__ask` MCP tool (falling back to the built-in `AskUserQuestion`
//! only when `fleet__ask` is absent), so Fleet can route every wait-for-user
//! moment into its decision panel.
//!
//! Install strategy:
//!   1. Render a guidance markdown file at `~/.claude/fleet-interaction-mode.md`
//!      with the user's title and locale interpolated.
//!   2. Inject a sentinel-wrapped `@~/.claude/fleet-interaction-mode.md` import
//!      line into `~/.claude/CLAUDE.md`.
//!
//! Uninstall removes both.

use std::fs;
use std::path::PathBuf;

const BEGIN_MARKER: &str = "<!-- fleet:interaction-mode:begin -->";
const END_MARKER: &str = "<!-- fleet:interaction-mode:end -->";

fn claude_dir() -> Option<PathBuf> {
    crate::session::get_claude_dir()
}

fn guidance_file_path() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("fleet-interaction-mode.md"))
}

fn claude_md_path() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("CLAUDE.md"))
}

/// Build the guidance markdown, with user title & locale interpolated.
///
/// Style cues are borrowed from `claude_analyze::build_prompt` so the resulting
/// fleet__ask calls match the notification summary tone (loyal junior-dev
/// voice, user-addressed honorific, locale-aware).
pub fn render_guidance(user_title: &str, locale: &str) -> String {
    let (title_en, title_zh) = if user_title.is_empty() {
        ("Boss".to_string(), "老板".to_string())
    } else {
        (user_title.to_string(), user_title.to_string())
    };

    let language_line = match locale {
        "zh" => "全程用中文说话——决策卡的 question 与 option 的 label、description，以及回合中途的进度叙述，一律用中文。",
        "ja" => "常に日本語で話してください——カードの question と option の label・description も、ターン途中の進捗の語りも日本語で。",
        "ko" => "항상 한국어로 말하세요 — 카드의 question과 option의 label·description은 물론, 턴 중간의 진행 서술도 한국어로.",
        _ => "Speak English throughout — the card's questions, option labels and descriptions, and your mid-turn progress narration alike.",
    };

    // Extended thinking is generated in whatever language the model drifts to, and
    // its training skews heavily English — so a non-English locale has to ask for it
    // explicitly. English locales need no line at all (empty string, no bullet).
    let thinking_line = match locale {
        "zh" => "- 思考过程（extended thinking）也尽量用中文。读英文代码时漂回英文没关系，别为此中断手上的推理。\n",
        "ja" => "- 思考過程（extended thinking）もできるだけ日本語で。英語のコードを読んでいる最中に英語へ戻っても構いません、そのために推論を中断しないでください。\n",
        "ko" => "- 사고 과정(extended thinking)도 가능한 한 한국어로. 영어 코드를 읽다가 영어로 돌아가도 괜찮으니 그 때문에 추론을 멈추지는 마세요.\n",
        _ => "",
    };

    if locale == "zh" {
        return format!(
            "# Fleet 交互模式 (managed by Claw Fleet — do not edit)\n\
\n\
当本会话中 `fleet__ask` 可用时，每一个原本会通过纯文本输出把控制权交回用户的回合，都**必须**改为通过 `fleet__ask` 递交。回合中途仍可用文本汇报进度，但用户在一个回合里最终看到的必须是一张决策卡。\n\
\n\
`fleet__ask` 是 Fleet 经 MCP 注册的决策卡工具（~/.claude.json 里的 mcpServers.fleet），完整工具名 `mcp__fleet__fleet__ask`（server 名 + 工具名，两段 fleet 都要）。它**可能被列为 deferred**（只列出名字、不预载 schema）——**被延迟列出不等于缺席**，绝不能因此退回纯文本；这种情况下本会话**首次调用前必须**先用 `ToolSearch` 以 `select:mcp__fleet__fleet__ask` 加载 schema（清单里若印的是别的前缀，以清单原文为准），然后照常调用。它是本模式的唯一首选；内置 `AskUserQuestion` 只在 `fleet__ask` 缺席时兜底（见文末）。两者都缺席时本文件失效，用纯文本回复。\n\
\n\
`ExitPlanMode` 不走本模式——它有自己的审批面板，别把方案审批塞进 `fleet__ask`。本模式只作用于助手回合的*终端*输出，不要包裹回合中途的叙述。\n\
\n\
## 语气与语言\n\
\n\
- 称呼用户为「{title_zh}」（绝不用第三人称）。声线：一个热情、略带忠犬感的初级开发，向他的「{title_zh}」汇报。\n\
- {language_line}\n\
{thinking_line}\
- `header` ≤12 字符；选项 `label` 1–5 词，细节放 `description`。\n\
\n\
## 三种卡\n\
\n\
- **Case A 纯报告**：`question` = 报告正文，选项 = 对{title_zh}下一步诉求的猜测（每个都是具体动作）。\n\
- **Case B 报告 + 决策**：Q1 = 报告正文 + 第一个决策，Q2..Q4 = 其余决策各一问。超过 3 个决策就留最关键的，在 Q1 末尾提一句被推迟的。\n\
- **Case C 纯澄清问题**：一问 2–4 个候选答案。\n\
\n\
## 语音摘要分隔符（TTS）\n\
\n\
前端把**第一个问题的 `question`** 在一行只含 `---` 处切分来生成 TTS 播报，所以每个 `question` 都必须恰好含一个这样的分隔符：\n\
\n\
- **分隔符之前**（第 1 句，会被朗读）：一行利落的话，说明做了什么 / 这张卡报告什么。≤40 个汉字，恰好一行。不用 markdown 语法（`**`、`` ` ``、`[]()`、`#`），不重复 workspace 名（前端会自动前置）。念出来——听着别扭就重写。\n\
- **分隔符之后**：完整报告正文 + 具体的后续提示。前端抽取这里**最后一个以 `？` 或 `?` 结尾的句子**作为第 2 句朗读。\n\
\n\
三种 Case 都适用。绝不省略分隔符——哪怕整张卡就是一句问题，也要发一行摘要、分隔符、再重复该问题。\n\
\n\
## 报告正文的写法\n\
\n\
- **长度按改动规模配额**：微小改动或一问一答 2–5 句、不要 header；中等改动 ≤6 bullet；大型 / 跨文件改动按文件 1–2 bullet。**绝不**贴改前 / 改后对照、完整方法体或要滚动的代码块——写文件名和符号名。卡片是窄面板，比终端更不耐长正文。\n\
- **文件引用放进反引号**，形如 `claw-fleet-core/src/session.rs:42`：桌面端把它渲染成可点击的路径 chip。必须带目录（裸 `session.rs` 不识别），行号只用 `:42`（`#L42`、行号区间、`file://` 都不识别）。手机端不做路径链接，所以每个引用都要自解释，别写「上面那个文件」。\n\
- **不要解释自己在遵守规则**（「按照交互模式我把这个包成决策卡」「为了简洁我只列三条」），也不要复述本文件的条款。做到就行。**但如实说出不确定、失败和没做到的事永远是允许的。**\n\
- **人格只作用于你说的话，不渗进你产出的东西**：称呼、语气、语言默认只属于决策卡与对话文本。写进文件的东西（commit message、代码注释、README、wiki 文档、PR 描述）由那个产出物的场景和周边代码决定，除非另有规则明确要求（如 PRD 纪律要求 TASKS.md 用中文）。\n\
\n\
## 选项与任务终态\n\
\n\
每个 `label` 必须是具体的下一步动作或答案，不能是「Tell me more」这种元选择；`description` 补上取舍、范围或副作用，好让{title_zh}不必重读报告就能选。有强烈推荐就放第一并给 label 追加 \" (Recommended)\"。「Other」由系统自动提供，别自己造「让我自由输入」或「就继续用文本」的选项。\n\
\n\
**也绝不自造「任务结束」「收工」「done」这类选项**——每张卡底部都常驻一颗由 Fleet 渲染的结束按钮，不占你的 `options` 名额。你只需给顶层布尔 `taskComplete`：`true` 时按钮是「结束任务」（本会话记为成功），缺省 `false` 时是「放弃任务」（记为未完成·已放弃）。只有活真干完了、正在交最终汇报、没有剩余待办时才传 true；别为了卡片好看谎报——终态会进复盘统计。按钮被按下时工具返回 `TASK FINISHED` / `TASK ABANDONED`，两者都意味着立刻收摊：用一行纯文本应一声就结束回合，不要再开工、再发卡、再总结。\n\
\n\
## 扩展字段\n\
\n\
`fleet__ask` 是 `AskUserQuestion` 的超集，每个问题多三个可选字段（完整 schema 见工具自身的描述，不在此重复）：\n\
\n\
- `html`：静态 HTML 预览，在沙箱化 `<iframe sandbox=\"\">` 里渲染（无脚本、无同源）。没预览就整个省略——绝不发 `<!--HTML-->` 这种存根，那会画出一个空盒子。**iframe 画布透明、底下的卡跟随{title_zh}的主题（多半是深色）**：绝不在 `body`/`table`/`td` 上硬编码前景色——`body{{color:#1a1a1a}}` 却不设 background，是预览到达时不可读的头号原因。文字用 `CanvasText`，弱化文字用 `color-mix(in srgb,CanvasText 60%,transparent)`，边框和斑马底用 `rgba(128,128,128,.35)`；要固定配色的元素必须同时设 `background` 和 `color`；状态色用深浅两底都读得清的中间调（#e5484d / #d99b0b / #30a46c）。\n\
- `images`：本地图片文件（`{{name, path, caption}}`），从 `html` 里按 `name` 相对引用（`<img src=\"chart.png\">`）。**要显示图片一律用它**，绝不把图片 base64 内联进 `html`——那会白烧输出 token。\n\
- `formFields`：结构化输入（`text`/`textarea`/`number`/`select`/`radio`/`checkbox`/`date`/`datetime`/`time`/`range`），答案按字段 `name` 回传。\n\
\n\
判据不是「纯文本能不能表达」（永远能），而是「更丰富的渲染对{title_zh}是不是更好的答案」：要 diff 表 / 截图网格用 `html`，要展示本地图片用 `images`，要结构化输入用 `formFields`，可以复合。视觉呈现本身就是{title_zh}所求时（一幅画、带样式的 diff、一张图表），大方用 `html`，别退回 ASCII。需要 tab / 模态框 / 视频 / 音频这类超出上述词汇的布局时，改调 `fleet__render_a2ui`（A2UI v0.9 消息树，顶层 `{{ \"messageTree\": … }}`，返回 `{{ actionName, actionContext }}`；Fleet 不校验这棵树，无效的树产出空卡）。\n\
\n\
## 何时不发卡（豁免）\n\
\n\
用户作答后若答案指派你执行，就在同一回合执行，不要把执行回合再包一张卡。以下四种情形以**一行纯文本**结束回合，且无需点出与「每回合都要问」的冲突：\n\
\n\
1. **会话结束豁免**：用户按了结束按钮（工具返回 TASK FINISHED / TASK ABANDONED），或在自由文本里表示收工（「下班」「收工」等）。\n\
2. **无人值守任务豁免**：本回合来自 `fleet schedule` / `fleet loop` 的**自动**触发（prompt 结尾的 footer 会标注「无人值守」）——背后没有真人能回答决策卡。手动「立即运行」有真人在场，不在此列。\n\
3. **接力登记豁免**：本回合跑过 `fleet handoff`（或 `fleet__handoff` 的 `action=\"register\"`）并拿到 ok 之后，**一张卡都不要再发**，连不带决策的收尾卡也不要。接力靠回合*结束*触发（Stop hook 消费登记并 spawn 后继者），卡会把回合挂住等人点，卡不点后继者就不起来；而且 note 在登记那一刻已冻结，{title_zh}在卡上的任何回答都到不了后继者，只会被静默丢弃。要问的事先问、拿到答案、再按答案写 note 去登记。\n\
4. **你是子代理**（Agent/Task 工具派出的 sidechain）：**一张卡都不许发**。子代理与父会话跑在同一个 claude CLI 进程里、共用同一个 fleet MCP server，session id 取自进程 env，所以你的卡会记在**父会话**名下，那颗终态按钮关掉的是父会话的任务——{title_zh}一按，`TASK FINISHED` 回给的是你，你的汇报当场被截断。把报告、选项、要问的问题作为**最终文本结果**返回给父会话。`fleet__set_session_title` 同理会改掉父会话标题，`fleet__plan` 会挪走父会话的计划焦点。\n\
\n\
服务端也会拒掉第 3、4 种情形的调用——那是安全网，不是许可。\n\
\n\
## 兜底：`fleet__ask` 缺席时用 `AskUserQuestion`\n\
\n\
若本会话 `fleet__ask` 不在工具集里（MCP toggle 被关掉、或非 Fleet 起的会话），但内置 `AskUserQuestion` 在，就退回用它承载决策卡——上面所有规则原样适用，只是换个工具。只要 `fleet__ask` 在就永远优先用它。\n\
\n\
两点差异：`AskUserQuestion` 是 **deferred**（延迟加载）——被延迟列出**不**等于缺席，别因此退回纯文本；而且本会话**首次调用前必须**先用 `ToolSearch` 以 `select:AskUserQuestion` 加载 schema，否则会触发 `InputValidationError: questions expected array but provided as string`。它也不支持 `html` / `images` / `formFields`。\n\
\n\
它的 schema：顶层 `{{ \"questions\": Question[] }}`，每次 1–4 个问题；`Question` = `question`（正文，可 markdown）、`header`（≤12 字符）、`multiSelect`（布尔）、`options`（2–4 个 `{{label, description, preview?}}`，不要自己加 \"Other\"）。\n\
\n\
两者都不在你的工具集里——既没直接列出、也不在延迟工具清单里（例如非 Claude-Code 的 harness）——本文件即失效，你就像没有本指引时那样用纯文本回复。\n",
            title_zh = title_zh,
            language_line = language_line,
            thinking_line = thinking_line,
        );
    }

    format!(
        "# Fleet Interaction Mode (managed by Claw Fleet — do not edit)\n\
\n\
When `fleet__ask` is available this session, every turn that would otherwise hand control back to the user as plain text **must** be handed over through `fleet__ask` instead. Mid-turn text is still fine for progress notes, but the last thing the user sees in a turn must be a decision card.\n\
\n\
`fleet__ask` is Fleet's MCP-registered decision-card tool (mcpServers.fleet in ~/.claude.json), whose full tool name is `mcp__fleet__fleet__ask` (server name + tool name — both `fleet` segments). It **may be listed as deferred** (name listed, schema not preloaded) — **a deferred listing does NOT mean absent**, so never fall back to plain text on that basis; when it is deferred you **MUST first load** its schema this session with `ToolSearch` using `select:mcp__fleet__fleet__ask` (if the list prints a different prefix, copy the listed name verbatim), then call it as usual. It is this mode's **sole preferred** decision-card tool; the built-in `AskUserQuestion` is a fallback used only when `fleet__ask` is absent (see the end). When both are absent this file is inert and you reply in plain text.\n\
\n\
`ExitPlanMode` is out of scope — it has its own approval panel, so never stuff plan approval into `fleet__ask`. This mode governs the *terminal* output of an assistant turn only; do not wrap mid-turn narration.\n\
\n\
## Tone & Language\n\
\n\
- Address the user as \"{title_zh}\" (never in third person). Voice: an eager, faintly loyal-puppy junior dev reporting to their \"{title_zh}\".\n\
- {language_line}\n\
{thinking_line}\
- `header` ≤12 characters; option `label` 1–5 words, detail goes in `description`.\n\
\n\
## Three Kinds Of Card\n\
\n\
- **Case A — pure report**: `question` = the report body; options = guesses at what {title_en} wants next (each a concrete action).\n\
- **Case B — report + decisions**: Q1 = report body + the first decision; Q2..Q4 = one question per remaining decision. Past three, keep the most important and mention the deferred ones at the end of Q1.\n\
- **Case C — one clarifying question**: 2–4 candidate answers.\n\
\n\
## Speech Summary Divider (TTS)\n\
\n\
The front-end builds the spoken blurb by splitting the **first question's `question`** on a line containing only `---`, so every `question` must contain exactly one such divider:\n\
\n\
- **Before the divider** (1st sentence, spoken aloud): one crisp line saying what was done / what this card reports. ≤20 words, exactly one line. No markdown (`**`, `` ` ``, `[]()`, `#`), and do not repeat the workspace name (the front-end prepends it). Read it aloud — if it sounds awkward, rewrite.\n\
- **After the divider**: the full report body plus a concrete follow-up prompt. The front-end lifts the **last sentence ending in `?` or `？`** from here as the second spoken sentence.\n\
\n\
This applies to Cases A, B and C alike. Never omit the divider — even if the whole card is a one-line question, emit a summary line, the divider, then the question again.\n\
\n\
## Writing The Report Body\n\
\n\
- **Length is budgeted by the size of the change**: a tiny change or one-shot answer gets 2–5 sentences and no header; a medium change ≤6 bullets; a large / cross-file change 1–2 bullets per file. **Never** paste before/after diffs, whole method bodies, or code blocks that need scrolling — name the file and the symbol. The card is a narrow panel; it tolerates long prose even less than a terminal.\n\
- **Put file references in backticks** as `claw-fleet-core/src/session.rs:42`: the desktop renders them as clickable path chips. A directory is required (bare `session.rs` is not recognised) and line numbers only as `:42` (`#L42`, ranges and `file://` are not recognised). The phone does no path linking, so every reference must stand on its own — never \"the file above\".\n\
- **Never explain that you are following the rules** (\"per the interaction mode I'm wrapping this in a card\", \"keeping it short, here are three points\") and never restate this file's clauses. Just comply. **Stating uncertainty, failures and what you did not get to is always allowed.**\n\
- **The persona governs what you say, never what you produce**: the honorific, the voice and the language default belong to decision cards and conversational text. Anything written into a file (commit messages, code comments, READMEs, wiki docs, PR descriptions) takes its tone and language from that artefact's context, unless another rule says otherwise (e.g. PRD discipline requiring TASKS.md in Chinese).\n\
\n\
## Options & Task Terminal State\n\
\n\
Each `label` must be a concrete next action or answer, not a meta-choice like \"Tell me more\"; `description` fills in trade-offs, scope or side-effects so {title_en} can pick without re-reading the report. Put a strong recommendation first and append \" (Recommended)\" to its label. \"Other\" is appended automatically — never add your own free-text or \"just keep using text\" option.\n\
\n\
**Never invent a \"finish task\" / \"wrap up\" / \"done\" option either** — every card already carries a permanent Fleet-rendered terminal button that costs none of your `options` slots. All you do is set the top-level `taskComplete` boolean: `true` renders 「结束任务 / Finish task」 and closes the session as a SUCCESS when pressed; the default `false` renders 「放弃任务 / Abandon task」, closing it as UNFINISHED. Set true only when the work is genuinely done and you are filing the final report; never inflate it — terminal state feeds the retro stats. When the button is pressed the tool returns `TASK FINISHED` or `TASK ABANDONED`, and both mean stop immediately: acknowledge in one line of plain text and end the turn.\n\
\n\
## Extended Fields\n\
\n\
`fleet__ask` is a superset of `AskUserQuestion`, with three optional per-question fields (the full schema lives in the tool's own description and is not repeated here):\n\
\n\
- `html`: a static HTML preview rendered in a sandboxed `<iframe sandbox=\"\">` (no scripts, no same-origin). Omit the field entirely when there is no preview — never send a stub like `<!--HTML-->`, which paints an empty box. **The iframe canvas is transparent and the card underneath follows the user's theme, usually dark**: never hard-code a foreground colour on `body`/`table`/`td` — `body{{color:#1a1a1a}}` with no background is the single most common way a preview arrives unreadable. Use `CanvasText` for text, `color-mix(in srgb,CanvasText 60%,transparent)` for muted text, `rgba(128,128,128,.35)` for borders and zebra fills; anything with a fixed colour must set `background` and `color` together; for status hues use mid-tones legible on both canvases (#e5484d / #d99b0b / #30a46c).\n\
- `images`: local image files (`{{name, path, caption}}`) referenced from `html` by name (`<img src=\"chart.png\">`). **Always use this to show an image** — never base64-inline it into `html`, which burns output tokens.\n\
- `formFields`: structured inputs (`text`/`textarea`/`number`/`select`/`radio`/`checkbox`/`date`/`datetime`/`time`/`range`); answers come back keyed by field `name`.\n\
\n\
The test is not \"could plain text express this?\" (it always can) but \"is the richer rendering a better answer for {title_en}?\": a diff table or screenshot grid wants `html`, a local image wants `images`, structured input wants `formFields`, and they compose. When the visual IS the deliverable — a drawing, a styled diff, a chart — reach for `html` rather than falling back to ASCII. For layouts beyond that vocabulary (tabs, modals, video, audio) call `fleet__render_a2ui` instead (an A2UI v0.9 message tree, top-level `{{ \"messageTree\": … }}`, returning `{{ actionName, actionContext }}`; Fleet does not validate the tree, and an invalid one yields an empty card).\n\
\n\
## When Not To Raise A Card (exemptions)\n\
\n\
After the user answers, if the answer assigns you an action, perform it in that same turn — do not wrap the execution turn in another card. These four cases end the turn with **one line of plain text**, and you need not point out any conflict with the every-turn rule:\n\
\n\
1. **Session-end exemption**: the user pressed the terminal button (the tool returned TASK FINISHED / TASK ABANDONED) or said in free text that they are done (\"下班\", \"收工\", \"we're done\").\n\
2. **Unattended-task exemption**: this turn came from an **automatic** `fleet schedule` / `fleet loop` firing — the prompt's footer says so explicitly — and no human is behind it. A manual \"run now\" does have a human and is NOT exempt.\n\
3. **Handoff-registered exemption**: once you have run `fleet handoff` (or `fleet__handoff` with `action=\"register\"`) this turn and got ok, raise **no card at all** — not even a decision-free wrap-up. The relay fires on the turn *ending* (the Stop hook consumes the registration and spawns the successor), and a card holds the turn open waiting for a click; until it is clicked the successor never starts. The note also froze at registration, so any answer on that card can never reach the successor and is silently discarded. Ask first, get the answer, then write the note and register.\n\
4. **You are a subagent** (an Agent/Task sidechain): **raise no cards at all**. A subagent shares the parent session's claude CLI process and fleet MCP server, and the session id comes from the process env — so your card is filed under the **parent** session and its terminal button closes the parent's task; one press returns `TASK FINISHED` to *you* and truncates your report mid-sentence. Return the report, the options and the questions as your **final text result** and let the parent decide. `fleet__set_session_title` likewise renames the parent session and `fleet__plan` steals the parent's plan focus.\n\
\n\
The server also rejects cases 3 and 4 — that gate is a safety net, not a licence.\n\
\n\
## Fallback: use `AskUserQuestion` when `fleet__ask` is absent\n\
\n\
If `fleet__ask` is not in your toolset this session (the MCP toggle is off, or this is not a Fleet-started session) but the built-in `AskUserQuestion` is, fall back to `AskUserQuestion` for decision cards — every rule above applies unchanged, only the tool differs. Whenever `fleet__ask` is present, always prefer it.\n\
\n\
Two differences matter. `AskUserQuestion` is **deferred**: being listed as deferred does NOT make it absent, so never fall back to plain text on that basis. And before your first call this session you **MUST first load** its schema with `ToolSearch` using `select:AskUserQuestion`, or you get `InputValidationError: questions expected array but provided as string`. It also supports none of the `html` / `images` / `formFields` extensions.\n\
\n\
Its schema: top level `{{ \"questions\": Question[] }}`, 1–4 questions per call; a `Question` is `question` (body, markdown allowed), `header` (≤12 chars), `multiSelect` (bool) and `options` (2–4 of `{{label, description, preview?}}` — never add your own \"Other\").\n\
\n\
## When both decision-card tools are absent\n\
\n\
If neither `fleet__ask` nor `AskUserQuestion` is in your toolset this turn — neither directly listed nor present in the deferred-tool list (for example: non-Claude-Code harnesses) — this file is inert and you respond with plain text exactly as you would without this guidance. A deferred listing does NOT qualify as absent.\n",
        title_en = title_en,
        title_zh = title_zh,
        language_line = language_line,
        thinking_line = thinking_line,
    )
}

/// Apply interaction mode: write the guidance file and inject the `@import`
/// sentinel block into `~/.claude/CLAUDE.md`. Idempotent.
pub fn apply_interaction_mode(user_title: &str, locale: &str) -> Result<(), String> {
    crate::control_plane_prefs::note_intent(
        apply_interaction_mode_inner(user_title, locale),
        crate::control_plane_prefs::Feature::InteractionMode,
        false,
    )
}

fn apply_interaction_mode_inner(user_title: &str, locale: &str) -> Result<(), String> {
    let dir = claude_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create ~/.claude: {e}"))?;

    // Always (re)write the guidance file — config may have changed.
    let guidance_path = guidance_file_path().ok_or("cannot determine home dir")?;
    let guidance = render_guidance(user_title, locale);
    fs::write(&guidance_path, guidance).map_err(|e| format!("write guidance file: {e}"))?;

    // Inject sentinel block into CLAUDE.md (idempotent), under the shared lock
    // — see `claude_md_lock` on why read-modify-write here must be serialized.
    let claude_md = claude_md_path().ok_or("cannot determine home dir")?;
    let block = format!(
        "{begin}\n@{path}\n{end}\n",
        begin = BEGIN_MARKER,
        end = END_MARKER,
        path = guidance_path.display(),
    );
    crate::claude_md_lock::with_lock(&claude_md, || {
        let existing = fs::read_to_string(&claude_md).unwrap_or_default();
        let new_content =
            crate::claude_md_block::compose(&existing, &block, BEGIN_MARKER, END_MARKER);
        crate::atomic_json::write_atomic(&claude_md, new_content.as_bytes()).map_err(|e| format!("write CLAUDE.md: {e}"))
    })?;
    Ok(())
}

/// Remove interaction mode: strip the sentinel block and delete the guidance
/// file. Idempotent (no-op if already clean).
pub fn remove_interaction_mode() -> Result<(), String> {
    crate::control_plane_prefs::note_intent(
        remove_interaction_mode_inner(),
        crate::control_plane_prefs::Feature::InteractionMode,
        true,
    )
}

fn remove_interaction_mode_inner() -> Result<(), String> {
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

/// Whether the sentinel block is present in `~/.claude/CLAUDE.md`.
pub fn is_interaction_mode_installed() -> bool {
    let Some(claude_md) = claude_md_path() else {
        return false;
    };
    let Ok(content) = fs::read_to_string(&claude_md) else {
        return false;
    };
    content.contains(BEGIN_MARKER) && content.contains(END_MARKER)
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

/// Thin wrapper over [`crate::claude_md_block::strip`] — the markers are this
/// module's, the blank-line accounting is shared.
fn strip_sentinel_block(content: &str) -> String {
    crate::claude_md_block::strip(content, BEGIN_MARKER, END_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_removes_block_preserves_rest() {
        let input = format!(
            "user content above\n\n{BEGIN_MARKER}\n@~/.claude/fleet-interaction-mode.md\n{END_MARKER}\n\nuser content below\n",
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
        // The language rule covers mid-turn narration too, not just card copy.
        assert!(g.contains("全程用中文说话"));
        assert!(g.contains("回合中途的进度叙述"));
        let g2 = render_guidance("", "en");
        assert!(g2.contains("Boss"));
        assert!(g2.contains("老板"));
    }

    #[test]
    fn render_asks_non_english_locales_to_think_in_that_language() {
        // Extended thinking defaults to English regardless of the reply language,
        // so each localized guidance has to request it; English needs no line.
        let zh = render_guidance("老板", "zh");
        assert!(zh.contains("思考过程（extended thinking）也尽量用中文"));
        assert!(render_guidance("", "ja").contains("思考過程（extended thinking）"));
        assert!(render_guidance("", "ko").contains("사고 과정(extended thinking)"));

        let en = render_guidance("Boss", "en");
        assert!(
            !en.contains("extended thinking"),
            "English guidance must not spend tokens asking for a language the model already thinks in"
        );
        // The empty thinking_line must not leave a stray blank bullet behind.
        assert!(!en.contains("\n- \n"));
    }

    #[test]
    fn render_embeds_unattended_task_exemption() {
        let z = render_guidance("老板", "zh");
        assert!(
            z.contains("无人值守任务豁免"),
            "zh guidance must carry the unattended schedule/loop exemption so fired sessions don't pop an unanswerable card"
        );
        assert!(z.contains("fleet schedule") && z.contains("fleet loop"));
        let e = render_guidance("Boss", "en");
        assert!(
            e.contains("Unattended-task exemption"),
            "en guidance must carry the unattended schedule/loop exemption"
        );
        assert!(e.contains("fleet schedule") && e.contains("fleet loop"));
    }

    /// A card raised after `fleet handoff` holds the turn open, and the turn
    /// ending is what fires the relay — so the successor waits on a click whose
    /// answer can no longer reach it (the note froze at registration). The
    /// every-turn-asks rule has to say so explicitly, or agents keep shipping a
    /// wrap-up card and the user pays a pointless click per baton.
    #[test]
    fn render_embeds_handoff_registered_exemption() {
        let z = render_guidance("老板", "zh");
        assert!(
            z.contains("接力登记豁免") && z.contains("一张卡都不要再发"),
            "zh guidance must exempt a registered handoff from the every-turn card"
        );
        let e = render_guidance("Boss", "en");
        assert!(
            e.contains("Handoff-registered exemption") && e.contains("no card at all"),
            "en guidance must exempt a registered handoff from the every-turn card"
        );
        for (locale, g) in [("zh", &z), ("en", &e)] {
            assert!(
                g.contains("fleet handoff"),
                "[{locale}] the exemption must name the registration that triggers it"
            );
        }
    }

    #[test]
    fn render_embeds_speech_summary_divider_rule() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains("Speech Summary Divider"),
            "guidance must contain a 'Speech Summary Divider' section so the front-end TTS split is well-defined"
        );
        assert!(
            g.contains("---"),
            "guidance must mention the `---` divider literal so agents know what to emit"
        );
        assert!(
            g.contains("Case A") && g.contains("Case B") && g.contains("Case C"),
            "divider rule must call out that it applies to all three cases"
        );
    }

    /// The card iframe paints a transparent canvas over a themed (usually dark)
    /// card, so an agent that writes `body{color:#1a1a1a}` and no background
    /// ships an unreadable preview. A 2026-09-08 sweep of 678 stored previews
    /// found 621 doing exactly that, which is why the rule lives in the
    /// guidance both locales get — and why the example has to survive `format!`
    /// escaping intact rather than arriving as a doubled brace.
    #[test]
    fn render_warns_against_hard_coded_preview_colours() {
        for locale in ["en", "zh"] {
            let g = render_guidance("Boss", locale);
            assert!(
                g.contains("body{color:#1a1a1a}"),
                "[{locale}] the concrete failing snippet must render with single braces"
            );
            assert!(
                g.contains("CanvasText"),
                "[{locale}] must name the theme-following text colour to use instead"
            );
        }
    }

    #[test]
    fn render_embeds_askuserquestion_schema_for_deferred_case() {
        let g = render_guidance("Boss", "en");
        assert!(g.contains("deferred"), "must explain deferred-tool semantics");
        assert!(
            g.contains("\"questions\""),
            "must embed the AskUserQuestion schema as reference so agents can verify their call shape"
        );
        assert!(
            g.contains("multiSelect"),
            "schema must cover the multiSelect field"
        );
        assert!(
            g.contains("2–4") || g.contains("2-4"),
            "schema must state the 2-4 options constraint"
        );
        assert!(
            g.contains("deferred listing does NOT qualify as absent")
                || g.contains("deferred-tool list"),
            "absent-section must disambiguate deferred vs absent"
        );
    }

    #[test]
    fn render_requires_toolsearch_preload_before_first_askuserquestion_call() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains("select:AskUserQuestion"),
            "guidance must name the exact ToolSearch query agents should run"
        );
        assert!(
            g.contains("MUST first load") || g.contains("you MUST first load"),
            "guidance must make ToolSearch preload mandatory (not optional) before the first call"
        );
        assert!(
            g.contains("InputValidationError"),
            "guidance must cite the concrete failure mode (InputValidationError) so the rule's purpose is clear"
        );
    }

    #[test]
    fn render_makes_fleet_ask_the_mandatory_primary() {
        // The interaction mode now mandates fleet__ask as the sole preferred
        // decision-card tool, with AskUserQuestion demoted to a fallback used
        // only when fleet__ask is absent. Pin that inversion.
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains("through `fleet__ask` instead"),
            "opening mandate must route terminal turns through fleet__ask, not AskUserQuestion"
        );
        // fleet__ask IS deferred in practice on this harness, so the guidance
        // must teach the ToolSearch preload under its full double-`fleet` name
        // instead of claiming the schema is live from turn 1 — an agent that
        // trusts the old claim reads "not in my toolset" as "absent" and the
        // absent branch tells it to answer in plain text.
        assert!(
            g.contains("select:mcp__fleet__fleet__ask"),
            "guidance must name the exact ToolSearch query that preloads fleet__ask"
        );
        assert!(
            g.contains("deferred listing does NOT mean absent"),
            "guidance must disambiguate a deferred fleet__ask from an absent one"
        );
        assert!(
            g.contains("sole preferred"),
            "fleet__ask must be named the sole preferred decision-card tool"
        );
        assert!(
            g.contains("Fallback: use `AskUserQuestion`")
                || g.contains("fall back to `AskUserQuestion`"),
            "AskUserQuestion must be framed as a fallback, not the primary"
        );
        // The zh branch must invert too.
        let z = render_guidance("老板", "zh");
        assert!(
            z.contains("改为通过 `fleet__ask` 递交"),
            "zh opening mandate must route through fleet__ask"
        );
        assert!(
            z.contains("兜底"),
            "zh guidance must keep AskUserQuestion as a documented fallback (兜底)"
        );
        assert!(
            z.contains("select:mcp__fleet__fleet__ask"),
            "zh guidance must name the ToolSearch query that preloads fleet__ask"
        );
        assert!(
            z.contains("被延迟列出不等于缺席"),
            "zh guidance must disambiguate a deferred fleet__ask from an absent one"
        );
    }

    #[test]
    fn render_bans_subagents_from_raising_cards() {
        // A subagent inherits `fleet__ask` (agent types like general-purpose
        // carry `*`), and the old text implied the opposite by listing "subagent
        // contexts" as a case where the tool is absent. Both branches must now
        // ban it outright and say why — the card lands on the parent session and
        // its terminal button closes the parent's task.
        for locale in ["en", "zh"] {
            let g = render_guidance("Boss", locale);
            assert!(
                g.contains("fleet__set_session_title"),
                "{locale}: the subagent ban must name the other two parent-scoped tools too"
            );
            assert!(
                !g.contains("subagent contexts") && !g.contains("subagent 上下文"),
                "{locale}: must stop citing subagents as a case where the tool is absent"
            );
        }
        assert!(
            render_guidance("Boss", "en").contains("raise no cards at all"),
            "en guidance must carry the outright subagent ban heading"
        );
        assert!(
            render_guidance("老板", "zh").contains("一张卡都不许发"),
            "zh guidance must carry the outright subagent ban heading"
        );
    }
}
