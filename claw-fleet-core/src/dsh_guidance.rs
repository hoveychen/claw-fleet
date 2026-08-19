//! dsh guidance injection — composes Fleet's per-concept guidance blocks into
//! the user-global `$DSH_HOME/AGENTS.md` (default `~/.dsh/AGENTS.md`) so
//! Fleet-driven dsh sessions see the same discipline Claude and Codex sessions
//! get.
//!
//! # Why this file exists (and why it is not a parameter on [`crate::codex_guidance`])
//!
//! Two of the four concepts need *different content* on dsh, not a different
//! label:
//!
//! * **Interaction mode.** dsh has no Fleet MCP server wired in, so there is no
//!   `fleet__ask` — the codex block's ~30 lines about deferred MCP tools and
//!   exec-cell `wait` are actively wrong here. dsh instead ships a native
//!   `ask_user_question` tool (`@deepseek-ai/dsh-tool-ask-user`, present in the
//!   `standard` preset Fleet creates sessions under), whose
//!   `question/requested` frames [`crate::dsh_decisions`] already bridges to
//!   Fleet's elicitation cards.
//! * **Model cheat-sheet.** dsh addresses a model as `provider/model` from
//!   whatever the user configured, so the codex gpt-5.6 price table has no dsh
//!   analogue; what a dsh session actually needs is how to name a model when it
//!   spawns Fleet work.
//!
//! Parameterising one renderer would just stuff two incompatible prose bodies
//! into one `format!`. So this mirrors codex_guidance's *structure* (sentinels /
//! `wrap` / `compose` / `strip_all_fleet_blocks` / `reconcile_*` / `is_*`) and
//! rewrites the prose, exactly as codex_guidance itself did rather than reusing
//! the Claude renderers.
//!
//! # The injection point (verified live)
//!
//! `@deepseek-ai/dsh-agent-instructions` loads `$DSH_HOME/AGENTS.md` as the
//! user-global instruction file (its README: "The user-global file is always
//! `$DSH_HOME/AGENTS.md` with no local overlay"; `$DSH_HOME` defaults to
//! `~/.dsh`). Measured against a real `dsh web` with `DSH_HOME` pointed at a
//! temp dir: a sentinel written to that file came back in `session.history` as
//! a durable `user/message` carrying
//! `source = {kind: "agent-instructions", form: "instructions", baseline: true,
//! …, changes: [{action: "set", scope: "user-global\0AGENTS.md", …}]}`, framed
//! in `<system-reminder>`, ordered immediately after the claimed prompt — i.e.
//! it enters step 1 with the prompt and reaches the first request.
//!
//! Two consequences shape what we write here:
//!
//! * **No `@import`.** dsh does not interpret `@path` (README Known
//!   Limitations), so every concept lives inline in one file, like codex.
//! * **The user-global file is dropped FIRST under budget pressure.** The
//!   plugin "preserves the most specific instruction files first … drops whole
//!   broader files before truncating the most-specific file". `standard`
//!   configures `maxBytes: 65536`, double codex's ceiling, but the budget is
//!   shared with every project `AGENTS.md`/`CLAUDE.md` on the path — so being
//!   compact is not about fitting, it is about not being the first thing
//!   evicted in a repo with large project instructions.

use std::fs;
use std::path::PathBuf;

// Per-concept sentinels. Distinct from the codex ones so a machine running both
// harnesses keeps two independent files.
const PRD_BEGIN: &str = "<!-- fleet:dsh-prd:begin -->";
const PRD_END: &str = "<!-- fleet:dsh-prd:end -->";
const INTERACTION_BEGIN: &str = "<!-- fleet:dsh-interaction:begin -->";
const INTERACTION_END: &str = "<!-- fleet:dsh-interaction:end -->";
const WIKI_BEGIN: &str = "<!-- fleet:dsh-wiki:begin -->";
const WIKI_END: &str = "<!-- fleet:dsh-wiki:end -->";
const MODEL_BEGIN: &str = "<!-- fleet:dsh-model:begin -->";
const MODEL_END: &str = "<!-- fleet:dsh-model:end -->";
const LESSONS_BEGIN: &str = "<!-- fleet:dsh-lessons:begin -->";
const LESSONS_END: &str = "<!-- fleet:dsh-lessons:end -->";

/// All Fleet-managed marker pairs, used to strip the file down to user content
/// before recomposing. Extend this when adding a new per-concept block.
const FLEET_MARKERS: &[(&str, &str)] = &[
    (PRD_BEGIN, PRD_END),
    (INTERACTION_BEGIN, INTERACTION_END),
    (WIKI_BEGIN, WIKI_END),
    (MODEL_BEGIN, MODEL_END),
    (LESSONS_BEGIN, LESSONS_END),
];

/// Resolve dsh's home dir (`$DSH_HOME` or `~/.dsh`).
///
/// Delegates to the canonical resolver so this module, [`crate::skills`], and
/// anything else touching dsh's home cannot drift apart — the plugin resolves
/// the same pair, and writing anywhere else would silently inject nothing.
fn dsh_home() -> Option<PathBuf> {
    crate::session::get_dsh_dir()
}

fn agents_md_path() -> Option<PathBuf> {
    dsh_home().map(|d| d.join("AGENTS.md"))
}

/// Localized "write the paired artifacts in language X" lines, split per concept
/// so each standalone block carries only the sentence relevant to it.
fn language_lines(locale: &str) -> (&'static str, &'static str) {
    match locale {
        "zh" => (
            "本规则配套的 TASKS.md 用中文书写。",
            "决策卡的 question 与 option 文案用中文书写。",
        ),
        "ja" => (
            "本ルールに対応する TASKS.md は日本語で書いてください。",
            "意思決定カードの question と option は日本語で書いてください。",
        ),
        "ko" => (
            "이 규칙과 짝을 이루는 TASKS.md는 한국어로 작성하세요.",
            "결정 카드의 question과 option은 한국어로 작성하세요.",
        ),
        _ => (
            "Write the paired TASKS.md in English.",
            "Write decision-card question and option text in English.",
        ),
    }
}

fn title_or_default(user_title: &str) -> String {
    if user_title.is_empty() {
        "Boss".to_string()
    } else {
        user_title.to_string()
    }
}

/// Compact dsh **PRD discipline** block body (no sentinel markers).
///
/// Same rules as [`crate::codex_guidance::render_codex_prd_block`], with two dsh
/// substitutions: the `fleet__*` MCP paragraph becomes the `fleet` CLI (dsh has
/// no Fleet MCP), and Rule 2 states the attribution caveat that follows from dsh
/// running every session inside one shared server process.
pub fn render_dsh_prd_block(user_title: &str, locale: &str) -> String {
    let title = title_or_default(user_title);
    let (prd_lang, _) = language_lines(locale);

    format!(
        "# Fleet PRD Discipline for dsh (managed by Claw Fleet — do not edit this block)\n\
\n\
These rules govern how dsh sessions that Claw Fleet launched run multi-step \
plans and touch production code — the same discipline Fleet gives its Claude \
and Codex sessions. {prd_lang} Address the user as \"{title}\" throughout.\n\
\n\
A **multi-step plan** = any task you split into 2+ sequential subtasks \
(P1, P2, ..., Pn). The rules below hold from the moment you start such a plan \
until it is fully done. Rule 3 (worktree) is **global** — it applies to any \
change touching production code, even a single-step one.\n\
\n\
## Rule 1 — Commit discipline\n\
\n\
\"Commit\" here means commits on the **main / default branch**. Commits on a \
worktree feature branch (`prd/<task-id>`) are governed by Rule 3 and are \
always allowed — they are NOT Rule 1 violations.\n\
\n\
- **Do NOT proactively propose or run `git commit` on main** mid-plan — not \
after P1, not at any \"natural checkpoint\". The plan is the unit of work.\n\
- **You MAY commit on main only when:** (1) {title} explicitly asks this \
turn, OR (2) you just finished the **last** P-task (all TASKS.md boxes \
checked) AND surfaced completion to {title}. Under Rule 3 that final commit \
takes the form of `git merge --no-ff` from the worktree branch.\n\
- **`git push` is always gated** — never push without {title}'s explicit \
approval in the current turn.\n\
- **Why:** {title} got tired of \"shall I commit?\" interruptions every few \
minutes during long plans, and the green hash makes agents forget remaining \
work. Treat the whole plan as one unit.\n\
\n\
## Rule 2 — TASKS.md as the durable macro plan\n\
\n\
Context compaction flattens conversation history; the macro plan (\"P3..P10 \
still pending\") must live on disk so it survives. Fleet prepends the \
workspace's active TASKS.md plans to the prompt of **every** turn it drives, \
so you always see the current plan state — but you must keep that file up to \
date.\n\
\n\
- When you decompose a task into 2+ subtasks, write it to \
`<workspace_root>/TASKS.md` BEFORE starting P1.\n\
- Update plans with the **`fleet plan`** subcommands from your shell, not by \
hand-editing markdown. **dsh has no Fleet MCP tools** — there is no \
`fleet__plan` here, so the CLI is the only path (unlike Claude and Codex \
sessions, whose guidance points at MCP first):\n\
  - `fleet plan create <id> --title \"...\" [--parent <parent-id>]` — add a \
new plan block. Pass `--parent` for side work spun off mid-plan.\n\
  - `fleet plan check <id> <P>` / `uncheck <id> <P>` — tick / untick a task.\n\
  - `fleet plan resume <id> [P]` — take over an existing plan you did not \
create and were not handed.\n\
  - `fleet plan add <id> <P> --text \"...\"` — append a pending task.\n\
  - `fleet plan list` / `get <id>` — read.\n\
- **Attribution is best-effort on dsh and may be missing.** `fleet plan` \
records \"which session is on which plan\" from `FLEET_SESSION_ID`, but every \
dsh session shares one `dsh web` process, so there is no per-session value to \
read. The checkbox edit itself always lands — that is the part that matters. \
Never hand-edit TASKS.md to work around a missing-session-id warning.\n\
- Each plan lives inside a sentinel pair with a unique kebab-case `id`:\n\
\n\
```markdown\n\
<!-- fleet:prd:begin id=\"auth-refactor\" v=\"2\" -->\n\
\n\
**Plan:** Migrate session middleware to the new auth crate\n\
\n\
- [ ] **P1** — Audit call sites\n\
- [x] **P2** — Swap middleware impl\n\
\n\
<!-- fleet:prd:end id=\"auth-refactor\" -->\n\
```\n\
\n\
- Use `- [ ]` pending / `- [x]` done. **Only edit your own block**; treat \
every other plan's block as another session's in-flight work.\n\
- **Child plans backtrack automatically:** when you `fleet plan check` the \
last box of a plan created with `--parent`, Fleet re-points focus to the \
nearest unfinished ancestor and prints the next P — follow it, do NOT stop \
just because the child finished.\n\
\n\
## Rule 3 — Worktree-based workflow (GLOBAL)\n\
\n\
**Any change that touches production code MUST be developed inside an \
isolated git worktree** — whether it's a multi-step plan or a one-line fix. \
The main checkout stays clean throughout.\n\
\n\
```bash\n\
# 1. Open a worktree off current main (task-id = plan id, or a short slug)\n\
git worktree add -b prd/<task-id> .worktrees/<task-id> main\n\
\n\
# 2. Do all the work inside it; commit freely on the branch\n\
cd .worktrees/<task-id>\n\
#   ... edit, test ...\n\
git add -A && git commit -m \"...\"\n\
\n\
# 3. Merge back with --no-ff (this IS the single Rule-1-allowed main commit)\n\
cd <repo-root>\n\
git merge --no-ff prd/<task-id>\n\
\n\
# 4. Clean up — mandatory\n\
git worktree remove .worktrees/<task-id>\n\
git branch -d prd/<task-id>\n\
```\n\
\n\
- Intermediate commits inside the worktree are **encouraged** and do NOT \
violate Rule 1 — they're private progress markers.\n\
- `--no-ff` is mandatory; `--ff-only` and `--squash` are forbidden (main \
must keep per-commit history under one merge commit).\n\
- **Before removing the worktree, rescue untracked / gitignored artifacts.** \
`git merge` only carries committed content; `git worktree remove` then \
deletes the working dir and any never-tracked file is gone for good \
(`.gitignore` means \"not version-controlled\", NOT \"safe to delete\"). Run \
`git status --ignored`; if the worktree holds a generated artifact not \
trivially reproducible from committed code, STOP and ask {title} before \
removing.\n\
- If the merge conflicts or a post-merge test regresses, resolve **in \
place** — do NOT `git reset --hard`, do NOT abandon the worktree, do NOT \
amend the merge commit. Surface it to {title}.\n\
- Do NOT push the branch to a remote. Add `.worktrees/` to `.gitignore` (ask \
{title} first the first time).\n\
- **Exemptions** (Rule 3 off): pure documentation, configuration-only \
changes (CI YAML, dotfiles, `.gitignore`), and urgent hotfixes {title} \
approved landing directly on main.\n\
\n\
## Rule 4 — Execution rhythm\n\
\n\
Carry out a multi-step plan in **one continuous rhythm**. Each non-final \
P-task: (1) dev, (2) test/verify, (3) commit inside the worktree, then \
`fleet plan check <id> <P>` and **immediately continue to the next P-task in \
the same turn**.\n\
\n\
- Do NOT pause to ask \"should I continue with P2?\" or \"want to review \
progress before P4?\" or \"I've done a lot, want me to summarise?\". Those \
proactive progress-report checkpoints are exactly what this rule kills — \
{title} can already see progress via TASKS.md checkboxes and worktree \
commits.\n\
- **The rhythm pauses ONLY for:** (1) the final-merge acceptance gate \
(surface a \"ready to merge\" summary and wait for {title}'s go-ahead before \
`git merge --no-ff`); (2) a genuine direction-of-work fork where {title}'s \
judgement is needed (API A vs B, keep/drop compatibility); (3) a test/verify \
red light that survives ONE repair attempt — then stop, do NOT loop \
fix→retry→fix→retry; (4) a destructive operation (rebase, force-push, branch \
deletion, `git reset --hard`).\n\
\n\
## Rule 5 — Long-context handoff\n\
\n\
When your context is running long mid-plan, do NOT grind until it dies and do \
NOT silently wrap up early. Register a relay from your shell:\n\
\n\
```\n\
fleet handoff --note \"<shift-change briefing>\" [--plan <plan-id>] [--next <P>]\n\
```\n\
\n\
- `--note` is mandatory: what's done, what's in flight, key files, gotchas, \
the next concrete step. Pass `--plan`/`--next` for a TASKS.md plan so the \
successor is attributed automatically.\n\
- Commit worktree progress first, run the `fleet handoff` command, wait for \
`ok: handoff registered`, then end the turn. **Narrating a handoff in prose \
does nothing** — only the actual command spawns a successor.\n\
\n\
## Rule 6 — Waiting on an external condition (`fleet watch`)\n\
\n\
Handoff continues *work*; `fleet watch` waits for an *event* — a CI run \
finishing, a build producing an artifact, a deploy going live. Do NOT sit in \
a foreground poll loop or a background job waiting for it: a Fleet turn ends \
and anything still waiting is lost. Register a watch and end the turn:\n\
\n\
```\n\
fleet watch create --until \"<shell cmd that exits 0 when done>\" --capture \"<shell cmd whose stdout to report>\" --note \"<what you await>\"\n\
```\n\
\n\
- A detached timer polls the condition; the moment it succeeds Fleet resumes \
THIS session and hands the captured output to your next turn. `fleet watch \
stop <id>` cancels. It inherits this session's model / effort / source, so a \
dsh session resumes as dsh.\n\
- Pick the scheduling relay by *need*: **repeat periodically (cron) → \
`fleet loop`** (CLI alias `fleet cron`; Fleet-managed, durable, spawns a fresh \
session each interval); **fire once at a future time → `fleet schedule`** \
(`--at`/`--in`); **wait for an event then continue → `fleet watch`** (above).\n\
- Both `fleet loop` and `fleet schedule` take an optional `--until <shell cmd>` \
as a **cheap non-LLM gate** — a probe run each tick (or once due) that spawns \
the paid LLM session only when it exits 0. Poll often, pay for an LLM only \
when there is real work. Do NOT default to an LLM session every tick.",
        title = title,
        prd_lang = prd_lang,
    )
}

/// Compact dsh **interaction mode** block body (no sentinel markers).
///
/// Targets dsh's native `ask_user_question` tool, which [`crate::dsh_decisions`]
/// bridges onto Fleet's elicitation cards. This is NOT the codex text with a
/// renamed tool: dsh has no Fleet MCP server, so everything the codex block says
/// about deferred MCP tools, `mcp__fleet__fleet__ask`, exec-cell `wait`, and
/// `fleet__set_session_title` is absent here, and the card's field vocabulary is
/// the narrower one `ask_user_question` actually accepts.
pub fn render_dsh_interaction_block(user_title: &str, locale: &str) -> String {
    let title = title_or_default(user_title);
    let (_, ix_lang) = language_lines(locale);

    format!(
        "# Fleet Interaction Mode for dsh (managed by Claw Fleet — do not edit this block)\n\
\n\
{title} wants every wait-for-input moment delivered as a **decision card**, \
not plain text. When you would otherwise end a turn by yielding control back \
to {title} with a plain-text message, call **`ask_user_question`** instead. \
Mid-turn status lines (a one-sentence note before a tool call) stay as text; \
it's the **final surface** of a turn that must be a card. {ix_lang}\n\
\n\
`ask_user_question` is your own built-in tool, always in your tool list — \
there is nothing to discover and no MCP server involved. When Fleet is driving \
this session it intercepts the question and renders it as a Decision Card in \
the desktop app (and on {title}'s phone), then feeds the answer back to you as \
the tool result. When dsh runs outside Fleet the same call falls back to dsh's \
own UI, so the tool is always safe to reach for.\n\
\n\
## Shape\n\
\n\
`ask_user_question` takes a non-empty `questions` array. Per question: a \
stable `id` (echoed back in the answer), the `question` text, an optional \
short `header`, optional `options` (each `{{label, description}}`), and \
`multi_select`. Keep it to 1–4 questions with 2–4 options each — that is what \
the card renders cleanly.\n\
\n\
- **Do NOT add your own \"Other\" / \"let me type\" option.** The card always \
offers a free-text box; whatever {title} types arrives as `custom` on that \
question's answer (`{{id, selected, custom?}}`), overriding `selected` for a \
single-select question.\n\
- Keep `header` short (≤12 characters) — it is a chip label, and Fleet \
truncates a long one.\n\
- Put supporting detail in `description`, not in the option `label`.\n\
\n\
## Tone\n\
\n\
- Address the user as \"{title}\" (never third person). Voice: an \
enthusiastic, slightly-devoted junior dev reporting to {title}.\n\
- Question text, option labels, and descriptions all in {title}'s language.\n\
\n\
## Speech Summary Divider (required in every `question` field)\n\
\n\
Fleet reads the card aloud. Every `question` value MUST contain exactly one \
line that is only `---`:\n\
- **Before the divider:** ONE crisp plain-prose sentence saying what the card \
reports / asks (≤40 Chinese chars, no markdown). This is spoken first.\n\
- **After the divider:** the full report body (markdown / tables / lists OK) \
followed by the concrete question. The last sentence ending in `？`/`?` is \
spoken second.\n\
\n\
Example `question` value:\n\
```\n\
已定位到超时的根因。\n\
\n\
---\n\
\n\
根因是管道 stdin 让进程阻塞等 EOF，加 `</dev/null` 即秒回。\n\
\n\
接下来要不要我把这个修复接进 spawn 逻辑？\n\
```\n\
\n\
## Mapping your output into a card\n\
\n\
- **Case A — pure report / status (no pending decision):** one question, the \
report as the `question` field. Options = 2–3 guesses at {title}'s likely \
next ask (concrete next actions) + one \"任务结束\" / \"done\" option to close \
the turn.\n\
- **Case B — report + pending decisions:** pack into one call. Q1 = report \
body + first decision, options = its resolutions. Q2..Q4 = the remaining \
decisions (most consequential first; mention any deferred ones at the tail \
of Q1).\n\
- **Case C — single clarifying question:** one question, 2–4 candidate \
answers.\n\
\n\
## Option quality\n\
\n\
- Each `label` (1–5 words) is a concrete action/answer, not a meta-choice \
like \"tell me more\". Put trade-offs/scope in `description`.\n\
- If you have a clear recommendation, put it first and append \
\" (Recommended)\" to its label.\n\
- Never emit an option whose effect is just \"continue with text\" — the \
free-text box already covers that.\n\
\n\
## Termination / loop safety\n\
\n\
- After {title} answers with a concrete action, **execute it in the same \
turn** — do NOT immediately re-wrap in another card unless you genuinely \
reach another wait-for-input surface.\n\
- **Session-end exemption:** when {title} picks an option that clearly closes \
the conversation (\"任务结束\", \"收工\", \"done\"), end with a one-line \
plain-text acknowledgement instead of another card. This is the only case \
where a terminal turn is plain text.\n\
- **Plan mode is the exception dsh already enforces:** when you are in plan \
mode, present the plan through `exit_plan_mode`, not through \
`ask_user_question`. Do not ask \"should I proceed?\" as a card there.",
        title = title,
        ix_lang = ix_lang,
    )
}

/// Compact dsh **wiki knowledge base** block body (no sentinel markers).
///
/// The `fleet wiki` CLI is agent-agnostic, so this is the codex text with the
/// MCP-first paragraph dropped — dsh has no `fleet__wiki` tool.
pub fn render_dsh_wiki_block(locale: &str) -> String {
    if locale == "zh" {
        return "# Fleet Wiki 知识库 for dsh (managed by Claw Fleet — do not edit this block)\n\
\n\
当你产出**值得留存**的 HTML 报告、可交互 demo 或 markdown 文档(调研报告、\
架构说明、性能分析、数据可视化等)时,完成后用 Fleet 的 wiki 知识库归档,\
不要只把文件留在工作目录里:\n\
\n\
```\nfleet wiki publish <path> [--slug <slug>] [--title \"<标题>\"]\n```\n\
\n\
dsh 没有 Fleet 的 MCP 工具,所以一律走上面这个 CLI(Claude / Codex 会话的\
指引里那个「优先用 `fleet__wiki`」在这里不适用)。\n\
\n\
- `<path>` 可以是单个 `.html` / `.md` 文件,或含 `index.html` 的目录(目录会\
连同相对引用的 js/css/图片一起入库)。\n\
- 同一份文档迭代**复用同一个 slug** 重新 publish——生成新版本、旧版保留可回看;\
别为同一份内容起新 slug。\n\
- slug 用小写字母/数字/连字符;`/` 是**虚拟目录**分隔符(如 `--slug arch/overview` \
归到 `arch` 目录),同一主题共用前缀。已发布的可 `fleet wiki mv <旧> <新>` 改键搬家,\
版本历史一起带走。\n\
- 只归档最终成品,草稿/中间产物/一次性调试页不 publish。\n\
- markdown 里用 `[[slug]]` 或 `[[slug|显示文字]]` 交叉引用,渲染后可点击;\
引用前先 `fleet wiki list --all` 确认目标 slug 已发布。\n\
\n\
## 读取知识库\n\
\n\
需要某篇正文时(尤其用户在 prompt 里 `[[slug]]` 引用了它)用 `cat` 直接读,\
**别**去 `~/.fleet/wiki/` 手动拼版本目录:\n\
\n\
```\n\
fleet wiki cat <slug>                         # 当前版本正文\n\
fleet wiki cat <slug> --version <version-id>  # 历史版本\n\
fleet wiki cat <slug> --file assets/app.js    # 目录型文档里的其他文件\n\
```\n\
\n\
- `fleet wiki list`——只列当前 workspace 已发布,开工前回顾已有调研成果;`--all` 跨全部。\n\
- `fleet wiki search <关键词>`——搜标题/slug/正文并给片段(默认限当前 workspace)。\n\
- `fleet wiki show <slug>`——看版本历史和 entry 文件名。\n\
\n\
## 什么时候不该用 wiki\n\
\n\
- **一次性看一眼**(过目一张图/一段 diff)→ 直接在回复里给,或做成一张决策卡,不要 publish。\n\
- **要沉淀、之后还要读回来**→ wiki。只有 wiki 的文档能被后续 session `fleet wiki cat` \
读回、`[[slug]]` 交叉引用、按 workspace 筛选与全文搜索,也只有 wiki 收得下带 \
`assets/` 的多文件目录。\n\
\n\
判据不是「哪个能渲染」——而是「这份产物之后还会不会被读第二次」。会 → wiki。\n"
            .to_string();
    }
    "# Fleet Wiki knowledge base for dsh (managed by Claw Fleet — do not edit this block)\n\
\n\
When you produce a **durable** HTML report, interactive demo, or markdown \
document (research reports, architecture notes, performance analyses, data \
visualizations…), archive it into the Fleet wiki when done instead of leaving \
it in the workdir:\n\
\n\
```\nfleet wiki publish <path> [--slug <slug>] [--title \"<title>\"]\n```\n\
\n\
dsh has no Fleet MCP tools, so always use this CLI — the \"prefer `fleet__wiki`\" \
advice in the Claude / Codex guidance does not apply here.\n\
\n\
- `<path>` is a single `.html` / `.md` file, or a directory with an \
`index.html` entry (its relatively-referenced js/css/images are archived too).\n\
- Iterating on the same doc → **re-publish with the same slug** (new version, \
old ones stay browsable); don't mint a new slug for the same content.\n\
- Slugs are lowercase letters/digits/hyphens; `/` is a **virtual directory** \
separator (`--slug arch/overview` files under an `arch` folder) — share a \
prefix across docs on one topic. Move a published doc with \
`fleet wiki mv <old> <new>`, version history included.\n\
- Publish finished artifacts only — no drafts, intermediates, or debug pages.\n\
- Cross-link other docs with `[[slug]]` / `[[slug|display text]]` (clickable \
when rendered); check the target exists first with `fleet wiki list --all`.\n\
\n\
## Reading a wiki doc\n\
\n\
When you need a doc's content — especially when the user referenced it as \
`[[slug]]` — read it with `cat`; do **not** hand-assemble version paths under \
`~/.fleet/wiki/`:\n\
\n\
```\n\
fleet wiki cat <slug>                         # current version's content\n\
fleet wiki cat <slug> --version <version-id>  # a historical version\n\
fleet wiki cat <slug> --file assets/app.js    # another file in a dir doc\n\
```\n\
\n\
- `fleet wiki list` shows docs published from the current workspace (review \
what this project already investigated before starting); `--all` spans every \
workspace.\n\
- `fleet wiki search <term>` finds docs by title / slug / body; \
`fleet wiki show <slug>` shows version history and the entry filename.\n\
\n\
## When NOT to use the wiki\n\
\n\
- **A one-time glance** (show a chart or a diff, then discard) → put it in \
your reply or a decision card. Don't publish it.\n\
- **Something read again later** → the wiki. Only wiki docs can be read back \
by a later session with `fleet wiki cat`, cross-linked with `[[slug]]`, \
filtered by workspace, and full-text searched — and only the wiki accepts a \
multi-file directory with an `assets/` folder.\n\
\n\
The test is not \"which one can render this\" — it's \"will this output be read \
a second time?\" If yes → wiki.\n"
        .to_string()
}

/// Compact dsh **model-selection** block body (no sentinel markers).
///
/// Two halves, both of which a dsh session actually needs and neither of which
/// the codex block provides: how dsh itself addresses a model (`provider/model`,
/// which is also the string Fleet's spawn selector passes through), and which
/// model to name when this session spawns *Fleet* work through the `fleet` CLI.
/// There is deliberately no dsh price table — dsh bills through whichever
/// provider the user configured, so any number here would be a guess.
pub fn render_dsh_model_block(locale: &str) -> String {
    if locale == "zh" {
        return "# Fleet 模型选择速查 for dsh (managed by Claw Fleet — do not edit this block)\n\
\n\
## dsh 自己怎么点名模型\n\
\n\
dsh 把模型拆成 `provider` + `model` 两段,Fleet 的 spawn 用一个字符串表达,\
以**第一个 `/`** 分界:`openrouter/anthropic/claude-haiku-4.5` → provider \
`openrouter`,model `anthropic/claude-haiku-4.5`。不含 `/` 的字符串不指定 \
provider,会话就留在 harness 自身配置的模型上。\n\
\n\
可用的 provider / model 取决于**用户 `~/.dsh/settings.yaml` 里配了什么**,\
所以这里不列价目表:同一个模型经不同 provider 价格不同,凭印象报价就是编造。\
要知道当前配了什么,读那个配置文件,别猜。\n\
\n\
## 你派活给 Fleet 会话时选哪档(`fleet` CLI 的 `--model` / `--effort`)\n\
\n\
| 模型 | ID | 上下文 | 输入/输出 $/1M | 何时选 |\n\
|---|---|---|---|---|\n\
| Fable 5 | `claude-fable-5` | 1M | $10 / $50 | 最强推理+超长程;只用在最难任务 |\n\
| Opus 5 | `claude-opus-5` | 1M | $5 / $25 | 默认主力,自主 agentic / 编码 / 长程任务 |\n\
| Sonnet 5 | `claude-sonnet-5` | 1M | $3 / $15 | 近 Opus 编码、成本更低;并行 subagent 首选 |\n\
| Haiku 4.5 | `claude-haiku-4-5` | 200K | $1 / $5 | 最快最便宜;分类/抽取/机械活 |\n\
| Sol / Terra / Luna | `gpt-5.6-sol` / `-terra` / `-luna` | — | 走 ChatGPT 套餐配额 | Codex 侧强 / 中 / 快三档 |\n\
\n\
Claude effort:`low`/`medium`/`high`/`xhigh`/`max`,`xhigh` 是编码/agentic 最佳档;\
Codex effort 只有 `minimal`/`low`/`medium`/`high`。\n\
\n\
## 怎么挑\n\
\n\
- 机械、可并行、量大的活 → 便宜快档(Haiku / Sonnet;Luna / Terra)+ 低 effort。\n\
- 硬推理、最终综合、把关校验 → 最强档(Opus / Fable;Sol)+ high/xhigh。\n\
- 拿不准就别指定,让它继承默认。\n"
            .to_string();
    }
    "# Fleet model-selection cheat-sheet for dsh (managed by Claw Fleet — do not edit this block)\n\
\n\
## How dsh names a model\n\
\n\
dsh addresses a model as `provider` + `model`. Fleet's spawn carries one \
string and splits on the **first `/`**: `openrouter/anthropic/claude-haiku-4.5` \
→ provider `openrouter`, model `anthropic/claude-haiku-4.5`. A string with no \
`/` names no provider and leaves the session on whatever the harness itself is \
configured with.\n\
\n\
Which providers and models exist depends on **what the user configured in \
`~/.dsh/settings.yaml`**, so there is no price table here: the same model costs \
different amounts through different providers, and quoting a number from memory \
would be inventing one. Read that config when you need to know; don't guess.\n\
\n\
## Picking a tier when you hand work to a Fleet session (`fleet` CLI `--model` / `--effort`)\n\
\n\
| Model | ID | Context | In/Out $/1M | When to pick |\n\
|---|---|---|---|---|\n\
| Fable 5 | `claude-fable-5` | 1M | $10 / $50 | Strongest reasoning + longest horizon; hardest tasks only |\n\
| Opus 5 | `claude-opus-5` | 1M | $5 / $25 | Default workhorse: autonomous agentic / coding / long-horizon |\n\
| Sonnet 5 | `claude-sonnet-5` | 1M | $3 / $15 | Near-Opus coding at lower cost; value pick for parallel work |\n\
| Haiku 4.5 | `claude-haiku-4-5` | 200K | $1 / $5 | Fastest / cheapest; classification, extraction, mechanical work |\n\
| Sol / Terra / Luna | `gpt-5.6-sol` / `-terra` / `-luna` | — | ChatGPT-plan quota | Codex's strong / balanced / fast tiers |\n\
\n\
Claude effort: `low`/`medium`/`high`/`xhigh`/`max` (`xhigh` is best for coding \
and agentic work); Codex effort is only `minimal`/`low`/`medium`/`high`.\n\
\n\
## How to pick\n\
\n\
- Mechanical, parallel, high-volume work → the cheap/fast tier \
(Haiku / Sonnet; Luna / Terra) at low effort.\n\
- Hard reasoning, final synthesis, adversarial verification → the strongest \
tier (Opus / Fable; Sol) at high/xhigh.\n\
- When in doubt, don't specify one — let it inherit the default.\n"
        .to_string()
}

/// Collapse all whitespace runs (incl. newlines) to single spaces so a
/// multi-paragraph lesson renders as one compact AGENTS.md bullet.
fn flatten_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Render the daily-report lessons block body for dsh's AGENTS.md, or `None`
/// when there are no lessons.
///
/// Budget-capped the same way as the codex one. dsh's ceiling is 64 KiB rather
/// than 32, but the user-global file is the *first* thing the instruction loader
/// drops when a project's own AGENTS.md is large, so spending the extra headroom
/// here would only make the whole Fleet block set easier to evict.
fn render_dsh_lessons_block(
    lessons: &[crate::lessons_store::ManagedLesson],
    locale: &str,
) -> Option<String> {
    if lessons.is_empty() {
        return None;
    }
    const MAX_BYTES: usize = 6 * 1024;
    let zh = locale == "zh";
    let header = if zh {
        "# Fleet 用户经验 (managed by Claw Fleet — 勿手改)\n\n用户从每日报告加入的可迁移经验，逐条遵守："
    } else {
        "# Fleet user lessons (managed by Claw Fleet — do not edit)\n\nTransferable lessons the user added from the daily report — follow each:"
    };
    let mut out = String::from(header);
    let mut included = 0usize;
    for l in lessons {
        let content = flatten_ws(&l.content);
        let reason = flatten_ws(&l.reason);
        let line = if reason.is_empty() {
            format!("\n\n- {content}")
        } else if zh {
            format!("\n\n- {content}（原因：{reason}）")
        } else {
            format!("\n\n- {content} (why: {reason})")
        };
        if out.len() + line.len() > MAX_BYTES {
            break;
        }
        out.push_str(&line);
        included += 1;
    }
    let omitted = lessons.len() - included;
    if omitted > 0 {
        let note = if zh {
            format!("\n\n（另有 {omitted} 条因 AGENTS.md 篇幅上限省略；在桌面端 Memory 面板可查看全部）")
        } else {
            format!("\n\n({omitted} more omitted for the AGENTS.md size cap; see all in the desktop Memory panel)")
        };
        out.push_str(&note);
    }
    Some(out)
}

/// Wrap a block body in its sentinel markers with a trailing newline.
fn wrap(begin: &str, end: &str, body: &str) -> String {
    format!("{begin}\n{body}\n{end}\n")
}

/// Which per-concept dsh guidance blocks should be present in AGENTS.md.
#[derive(Debug, Clone, Copy, Default)]
pub struct DshGuidanceSet {
    pub prd: bool,
    pub interaction: bool,
    pub wiki: bool,
    pub model: bool,
    /// Include the daily-report lessons block (body read from the managed
    /// `~/.claude/fleet-lessons.md` at compose time, budget-capped).
    pub lessons: bool,
}

/// The single writer for `$DSH_HOME/AGENTS.md`. Composes exactly the enabled
/// concept blocks (stable order: PRD, interaction, wiki, model, lessons), strips
/// every Fleet-managed block that should be absent, and preserves user-authored
/// content outside the markers. Idempotent and order-independent. Deletes the
/// file if the result would be empty.
pub fn reconcile_dsh_agents_md(
    set: DshGuidanceSet,
    user_title: &str,
    locale: &str,
) -> Result<(), String> {
    let agents_md = agents_md_path().ok_or("cannot determine dsh home")?;
    let existing = fs::read_to_string(&agents_md).unwrap_or_default();
    let user_content = strip_all_fleet_blocks(&existing);

    let mut blocks = String::new();
    let mut push = |begin: &str, end: &str, body: String| {
        if !blocks.is_empty() {
            blocks.push('\n');
        }
        blocks.push_str(&wrap(begin, end, &body));
    };
    if set.prd {
        push(PRD_BEGIN, PRD_END, render_dsh_prd_block(user_title, locale));
    }
    if set.interaction {
        push(
            INTERACTION_BEGIN,
            INTERACTION_END,
            render_dsh_interaction_block(user_title, locale),
        );
    }
    if set.wiki {
        push(WIKI_BEGIN, WIKI_END, render_dsh_wiki_block(locale));
    }
    if set.model {
        push(MODEL_BEGIN, MODEL_END, render_dsh_model_block(locale));
    }
    if set.lessons {
        let lessons = crate::lessons_store::list_lessons();
        if let Some(body) = render_dsh_lessons_block(&lessons, locale) {
            push(LESSONS_BEGIN, LESSONS_END, body);
        }
    }

    let new_content = compose(&user_content, &blocks);

    if new_content.trim().is_empty() {
        // Nothing left (no user content, no blocks) — remove rather than leave a
        // stray empty AGENTS.md.
        if agents_md.exists() {
            fs::remove_file(&agents_md).map_err(|e| format!("remove AGENTS.md: {e}"))?;
        }
        return Ok(());
    }

    let dir = dsh_home().ok_or("cannot determine dsh home")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create dsh home: {e}"))?;
    fs::write(&agents_md, new_content).map_err(|e| format!("write AGENTS.md: {e}"))?;
    Ok(())
}

/// Mirror the Claude-side concept toggles onto dsh's AGENTS.md, reading which
/// concepts are enabled from their Claude carriers (the `@import` sentinels in
/// `~/.claude/CLAUDE.md`). The dsh analogue of
/// [`crate::codex_guidance::reconcile_codex_from_claude_state`]; both are called
/// together after any concept toggle and on startup.
///
/// Skips entirely for a machine with no dsh: unlike `~/.codex`, `~/.dsh` is new
/// enough that conjuring it for a user who has never run dsh would be a visible
/// surprise. The skip is conditional on there being nothing to clean up, so a
/// user who uninstalls dsh still gets their file reconciled (and removed) rather
/// than left with a stale block.
pub fn reconcile_dsh_from_claude_state(user_title: &str, locale: &str) -> Result<(), String> {
    let dsh_present = dsh_home().map(|d| d.exists()).unwrap_or(false)
        || crate::dsh_server::is_available();
    let has_file = agents_md_path().map(|p| p.exists()).unwrap_or(false);
    if !dsh_present && !has_file {
        return Ok(());
    }
    let set = DshGuidanceSet {
        prd: crate::prd_discipline::is_prd_discipline_installed(),
        interaction: crate::interaction_mode::is_interaction_mode_installed(),
        wiki: crate::wiki_guidance::is_wiki_guidance_installed(),
        model: crate::model_guidance::is_model_guidance_installed(),
        lessons: dsh_present && !crate::lessons_store::list_lessons().is_empty(),
    };
    reconcile_dsh_agents_md(set, user_title, locale)?;

    // Fleet's cordis plugin rides the same switch as the PRD block, because the
    // plugin is what delivers the PRD block's dynamic half (the active-plans
    // reminder). Reported rather than swallowed: with the plugin uninstalled
    // there is no fallback channel, so a silent failure would mean a dsh session
    // quietly running without Fleet's context.
    crate::dsh_plugin::reconcile_dsh_patch(set.prd)
}

/// Whether the dsh PRD-discipline block is present in `$DSH_HOME/AGENTS.md`.
///
/// Also the gate on the dynamic half of PRD injection: [`crate::dsh_source`]
/// prepends the active-plans reminder only when this is true.
pub fn is_dsh_prd_installed() -> bool {
    agents_md_contains(PRD_BEGIN)
}

/// Whether the dsh interaction-mode block is present in `$DSH_HOME/AGENTS.md`.
pub fn is_dsh_interaction_installed() -> bool {
    agents_md_contains(INTERACTION_BEGIN)
}

/// Whether the dsh wiki block is present in `$DSH_HOME/AGENTS.md`.
pub fn is_dsh_wiki_installed() -> bool {
    agents_md_contains(WIKI_BEGIN)
}

/// Whether the dsh model block is present in `$DSH_HOME/AGENTS.md`.
pub fn is_dsh_model_installed() -> bool {
    agents_md_contains(MODEL_BEGIN)
}

/// Whether any Fleet-managed dsh block is present.
pub fn is_dsh_guidance_installed() -> bool {
    is_dsh_prd_installed()
        || is_dsh_interaction_installed()
        || is_dsh_wiki_installed()
        || is_dsh_model_installed()
}

fn agents_md_contains(needle: &str) -> bool {
    let Some(agents_md) = agents_md_path() else {
        return false;
    };
    let Ok(content) = fs::read_to_string(&agents_md) else {
        return false;
    };
    content.contains(needle)
}

/// Append `blocks` to `user_content` separated by one blank line. Either side
/// may be empty.
fn compose(user_content: &str, blocks: &str) -> String {
    let base = user_content.trim_end_matches('\n');
    let blocks = blocks.trim_end_matches('\n');
    if base.trim().is_empty() {
        return if blocks.is_empty() {
            String::new()
        } else {
            format!("{blocks}\n")
        };
    }
    if blocks.is_empty() {
        return format!("{base}\n");
    }
    format!("{base}\n\n{blocks}\n")
}

/// Strip every Fleet-managed marker pair from `content`, leaving only
/// user-authored text. Collapses 3+ trailing blank lines left behind.
fn strip_all_fleet_blocks(content: &str) -> String {
    let mut out = content.to_string();
    for (begin, end) in FLEET_MARKERS {
        out = strip_block(&out, begin, end);
    }
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    out
}

/// Strip a single `begin..end` marker pair (inclusive) from `content`.
fn strip_block(content: &str, begin: &str, end: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_block = false;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == begin {
            in_block = true;
            continue;
        }
        if trimmed == end {
            in_block = false;
            continue;
        }
        if !in_block {
            out.push_str(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prd_block_carries_worktree_and_rhythm() {
        let g = render_dsh_prd_block("Boss", "en");
        assert!(g.contains("git worktree add -b prd/"), "must show worktree creation");
        assert!(g.contains("git merge --no-ff"), "must mandate --no-ff merge back");
        assert!(
            g.contains("--squash") && g.contains("forbidden"),
            "must forbid --squash so dsh doesn't substitute it"
        );
        assert!(g.contains("git worktree remove"), "must show cleanup step");
        assert!(g.contains("Rule 1") && g.contains("Commit discipline"));
        assert!(g.contains("Rule 4") && g.contains("Execution rhythm"));
        assert!(
            g.contains("should I continue"),
            "Rule 4 must name the forbidden progress-report checkpoint pattern"
        );
        assert!(g.contains("fleet handoff"), "must teach the Rule 5 handoff relay");
        assert!(g.contains("fleet watch create"), "must teach the Rule 6 watch");
        assert!(
            g.contains("fleet plan check") && g.contains("fleet plan create"),
            "must teach the fleet plan subcommands"
        );
    }

    #[test]
    fn prd_block_points_at_the_cli_not_mcp() {
        // The single biggest way this block could go wrong is by inheriting
        // codex's "prefer the fleet__* MCP tools" paragraph: dsh has no Fleet
        // MCP server, so an agent that hunts for `fleet__plan` finds nothing and
        // may conclude plan updates are unavailable.
        let g = render_dsh_prd_block("Boss", "en");
        assert!(
            !g.contains("fleet__plan") || g.contains("no Fleet MCP"),
            "must not send a dsh session looking for MCP tools it does not have"
        );
        assert!(
            g.contains("no Fleet MCP"),
            "must say outright that the MCP path is absent here"
        );
        assert!(
            g.contains("FLEET_SESSION_ID") && g.contains("shares one"),
            "must state the shared-server attribution caveat rather than promising \
             attribution dsh cannot deliver"
        );
    }

    #[test]
    fn interaction_block_targets_ask_user_question() {
        let g = render_dsh_interaction_block("Boss", "en");
        assert!(
            g.contains("ask_user_question"),
            "interaction block must reference dsh's own tool"
        );
        assert!(
            !g.contains("fleet__ask") && !g.contains("mcp__fleet__"),
            "must NOT reference the MCP tool dsh does not have"
        );
        assert!(
            g.contains("Speech Summary Divider") && g.contains("---"),
            "must teach the TTS divider rule"
        );
        assert!(
            g.contains("Session-end exemption"),
            "must keep the session-end plain-text exemption so dsh doesn't loop cards forever"
        );
        assert!(
            g.contains("custom"),
            "must explain that free text comes back as `custom` so the model does not \
             invent its own Other option"
        );
        assert!(
            g.contains("exit_plan_mode"),
            "must defer to dsh's own plan-mode channel instead of carding the plan"
        );
        assert!(
            !g.contains("git worktree add"),
            "interaction block must not contain PRD/worktree content"
        );
    }

    #[test]
    fn blocks_use_title_and_locale() {
        let prd = render_dsh_prd_block("师父", "zh");
        assert!(prd.contains("师父"), "PRD block must interpolate the user title");
        assert!(
            prd.contains("中文书写"),
            "zh locale must select the Chinese TASKS.md line"
        );
        let ix = render_dsh_interaction_block("", "en");
        assert!(ix.contains("Boss"), "empty title falls back to Boss");
        assert!(
            ix.contains("decision-card question and option text in English"),
            "en locale selects the English interaction language line"
        );
    }

    #[test]
    fn all_blocks_stay_compact_against_the_eviction_order() {
        // dsh's ceiling is 65536, but the user-global file is the first one the
        // instruction loader drops when a project's own AGENTS.md is large. Hold
        // the same budget codex uses so Fleet's blocks are never the reason a
        // repo's instructions get evicted.
        let prd = render_dsh_prd_block("老板", "zh");
        let ix = render_dsh_interaction_block("老板", "zh");
        let wiki = render_dsh_wiki_block("zh");
        let model = render_dsh_model_block("zh");
        let total = prd.len() + ix.len() + wiki.len() + model.len();
        assert!(
            total < 30_000,
            "prd+interaction+wiki+model = {total} must stay well under dsh's 64 KiB \
(prd {}, ix {}, wiki {}, model {})",
            prd.len(),
            ix.len(),
            wiki.len(),
            model.len()
        );
    }

    #[test]
    fn wiki_and_model_blocks_carry_their_essence() {
        let wiki = render_dsh_wiki_block("en");
        assert!(wiki.contains("fleet wiki publish"), "wiki must teach publish");
        assert!(wiki.contains("[[slug]]"), "wiki must teach cross-links");
        assert!(!wiki.contains("git worktree"), "wiki block must not drag in PRD content");
        let model = render_dsh_model_block("en");
        assert!(
            model.contains("provider") && model.contains("first `/`"),
            "model block must teach dsh's own provider/model addressing"
        );
        assert!(
            model.contains("settings.yaml") && model.contains("don't guess"),
            "model block must send the agent to the real config instead of quoting \
             prices dsh does not fix"
        );
        assert!(
            model.contains("claude-opus-5"),
            "model block must still cover the tiers a dsh session spawns Fleet work with"
        );
    }

    #[test]
    fn compose_appends_blocks_to_user_content() {
        let out = compose("# My AGENTS\n\nuser stuff.\n", "BLOCK\n");
        assert!(out.contains("user stuff."), "keeps user content");
        assert!(out.contains("BLOCK"), "adds the block");
        assert!(out.starts_with("# My AGENTS"), "user content first");
        assert!(!out.contains("\n\n\n"), "no triple newline: {out:?}");
    }

    #[test]
    fn compose_handles_empty_sides() {
        assert_eq!(compose("", ""), "");
        assert_eq!(compose("", "BLOCK\n"), "BLOCK\n");
        assert_eq!(compose("user\n", ""), "user\n");
    }

    #[test]
    fn strip_all_removes_every_concept_block() {
        let input = format!(
            "user top\n\n{PRD_BEGIN}\nprd\n{PRD_END}\n\n{INTERACTION_BEGIN}\nix\n{INTERACTION_END}\
\n\n{WIKI_BEGIN}\nwiki\n{WIKI_END}\n\nuser bottom\n"
        );
        let out = strip_all_fleet_blocks(&input);
        assert!(!out.contains(PRD_BEGIN) && !out.contains("prd"));
        assert!(!out.contains(INTERACTION_BEGIN) && !out.contains("ix"));
        assert!(!out.contains(WIKI_BEGIN) && !out.contains("wiki"));
        assert!(out.contains("user top") && out.contains("user bottom"));
        assert!(!out.contains("\n\n\n"), "collapses blank runs: {out:?}");
    }

    #[test]
    fn markers_never_collide_with_the_codex_ones() {
        // Both files can exist on one machine; a shared sentinel would let each
        // reconciler strip the other's block.
        assert!(PRD_BEGIN.contains("dsh-prd"));
        assert_ne!(PRD_BEGIN, "<!-- fleet:codex-prd:begin -->");
        assert_ne!(INTERACTION_BEGIN, "<!-- fleet:codex-interaction:begin -->");
        assert_ne!(WIKI_BEGIN, "<!-- fleet:codex-wiki:begin -->");
        assert_ne!(MODEL_BEGIN, "<!-- fleet:codex-model:begin -->");
        assert_ne!(LESSONS_BEGIN, "<!-- fleet:codex-lessons:begin -->");
    }

    // --- reconcile_dsh_agents_md: exercised against a temp DSH_HOME ---
    //
    // These tests repoint $DSH_HOME. They serialize on the shared process-wide
    // home lock (the same one CODEX_HOME/FLEET_HOME mutators take) so no other
    // test reads a Fleet home while ours is repointed — see
    // `tests/home_env_lock_guard.rs`, which enforces this.

    fn with_temp_dsh_home<T>(f: impl FnOnce(&PathBuf) -> T) -> T {
        let _guard = crate::session::fleet_home_lock();
        let base =
            std::env::temp_dir().join(format!("fleet-dsh-guidance-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let prev = std::env::var_os("DSH_HOME");
        std::env::set_var("DSH_HOME", &base);
        let out = f(&base);
        match prev {
            Some(v) => std::env::set_var("DSH_HOME", v),
            None => std::env::remove_var("DSH_HOME"),
        }
        let _ = fs::remove_dir_all(&base);
        out
    }

    /// Point `$DSH_HOME` at a path that does **not** exist, for the "machine
    /// with no dsh" case. Self-guarding like [`with_temp_dsh_home`], so callers
    /// need no lock of their own.
    fn with_absent_dsh_home<T>(f: impl FnOnce(&PathBuf) -> T) -> T {
        let _guard = crate::session::fleet_home_lock();
        let base = std::env::temp_dir()
            .join(format!("fleet-dsh-absent-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let prev = std::env::var_os("DSH_HOME");
        std::env::set_var("DSH_HOME", &base);
        let out = f(&base);
        match prev {
            Some(v) => std::env::set_var("DSH_HOME", v),
            None => std::env::remove_var("DSH_HOME"),
        }
        let _ = fs::remove_dir_all(&base);
        out
    }

    fn set(prd: bool, interaction: bool, wiki: bool, model: bool) -> DshGuidanceSet {
        DshGuidanceSet {
            prd,
            interaction,
            wiki,
            model,
            lessons: false,
        }
    }

    fn ml(content: &str, reason: &str) -> crate::lessons_store::ManagedLesson {
        crate::lessons_store::ManagedLesson {
            id: format!("{}", content.len()),
            content: content.to_string(),
            reason: reason.to_string(),
            workspace_name: "ws".to_string(),
            session_id: "sid".to_string(),
        }
    }

    #[test]
    fn lessons_block_none_when_empty() {
        assert!(render_dsh_lessons_block(&[], "en").is_none());
    }

    #[test]
    fn lessons_block_renders_each_and_caps() {
        let body = render_dsh_lessons_block(
            &[ml("Line one.\n\nLine two.", "Because reasons."), ml("Second.", "")],
            "en",
        )
        .unwrap();
        assert!(body.contains("- Line one. Line two. (why: Because reasons.)"));
        assert!(body.contains("- Second."));
        assert!(!body.contains("Second. (why:"));

        let big = "x".repeat(2000);
        let many: Vec<_> = (0..10).map(|_| ml(&big, "")).collect();
        let capped = render_dsh_lessons_block(&many, "en").unwrap();
        assert!(capped.len() <= 6 * 1024 + 200, "block honors the byte cap");
        assert!(capped.contains("more omitted"), "omitted count is surfaced");
    }

    #[test]
    fn reconcile_composes_only_enabled_blocks() {
        with_temp_dsh_home(|base| {
            let agents = base.join("AGENTS.md");

            reconcile_dsh_agents_md(set(false, true, false, false), "Boss", "en").unwrap();
            let c = fs::read_to_string(&agents).unwrap();
            assert!(c.contains(INTERACTION_BEGIN) && !c.contains(PRD_BEGIN));

            reconcile_dsh_agents_md(set(true, true, true, true), "Boss", "en").unwrap();
            let c = fs::read_to_string(&agents).unwrap();
            assert!(
                c.contains(PRD_BEGIN)
                    && c.contains(INTERACTION_BEGIN)
                    && c.contains(WIKI_BEGIN)
                    && c.contains(MODEL_BEGIN)
            );

            reconcile_dsh_agents_md(set(true, false, false, true), "Boss", "en").unwrap();
            let c = fs::read_to_string(&agents).unwrap();
            assert!(c.contains(PRD_BEGIN) && c.contains(MODEL_BEGIN));
            assert!(!c.contains(INTERACTION_BEGIN) && !c.contains(WIKI_BEGIN));

            reconcile_dsh_agents_md(set(false, false, false, false), "Boss", "en").unwrap();
            assert!(!agents.exists(), "empty reconcile removes the file");
        });
    }

    #[test]
    fn reconcile_is_idempotent_and_preserves_user_content() {
        with_temp_dsh_home(|base| {
            let agents = base.join("AGENTS.md");
            fs::write(&agents, "# My dsh AGENTS\n\nkeep me.\n").unwrap();

            reconcile_dsh_agents_md(set(true, true, true, true), "Boss", "en").unwrap();
            let once = fs::read_to_string(&agents).unwrap();
            reconcile_dsh_agents_md(set(true, true, true, true), "Boss", "en").unwrap();
            let twice = fs::read_to_string(&agents).unwrap();
            assert_eq!(once, twice, "composing twice must not accumulate content");
            assert!(once.contains("keep me."), "user content preserved");
            assert!(once.starts_with("# My dsh AGENTS"), "user content stays first");
            assert!(!once.contains("\n\n\n"), "no triple newline");
        });
    }

    #[test]
    fn is_installed_flags_track_disk() {
        with_temp_dsh_home(|_base| {
            assert!(!is_dsh_prd_installed() && !is_dsh_interaction_installed());
            assert!(!is_dsh_wiki_installed() && !is_dsh_model_installed());
            reconcile_dsh_agents_md(set(true, false, false, true), "Boss", "en").unwrap();
            assert!(is_dsh_prd_installed() && is_dsh_model_installed());
            assert!(!is_dsh_interaction_installed() && !is_dsh_wiki_installed());
            assert!(is_dsh_guidance_installed());
            reconcile_dsh_agents_md(set(false, false, false, false), "Boss", "en").unwrap();
            assert!(!is_dsh_guidance_installed());
        });
    }

    #[test]
    fn reconcile_from_claude_state_skips_a_machine_with_no_dsh() {
        // A dsh home that does not exist and no AGENTS.md to clean up: Fleet must
        // not conjure ~/.dsh for a Claude-only user. (`is_available()` can still
        // be true if the machine has the binary — then writing IS correct, so the
        // assertion only covers the no-binary case.)
        with_absent_dsh_home(|home| {
            if crate::dsh_server::is_available() {
                // The binary IS installed here, so writing the file is the
                // correct behaviour and there is nothing to assert about skipping.
                return;
            }
            reconcile_dsh_from_claude_state("Boss", "en").unwrap();
            assert!(!home.exists(), "must not create a dsh home out of nowhere");
        });
    }
}
