//! Codex guidance injection — composes Fleet's per-concept guidance blocks into
//! the global `~/.codex/AGENTS.md` (or `$CODEX_HOME/AGENTS.md`) so Fleet-driven
//! Codex sessions see the same discipline Claude sessions get.
//!
//! # Per-concept blocks (unified reconcile)
//!
//! Claude receives each guidance *concept* through its own independently
//! toggled file (`fleet-interaction-mode.md`, `fleet-prd-discipline.md`,
//! `fleet-wiki-guidance.md`, `fleet-model-guidance.md`) `@import`ed into
//! `~/.claude/CLAUDE.md`.  Codex has no `@import` mechanism — it reads AGENTS.md
//! directly — so the concepts live **inline** as separate sentinel-wrapped
//! blocks in one file:
//!   - `fleet:codex-prd`         — PRD discipline (commit / worktree / rhythm /
//!                                 handoff / watch), analogue of
//!                                 [`crate::prd_discipline`].
//!   - `fleet:codex-interaction` — `fleet__ask` decision cards, analogue of
//!                                 [`crate::interaction_mode`].
//!   - `fleet:codex-wiki`        — `fleet wiki` knowledge base, analogue of
//!                                 [`crate::wiki_guidance`]  (added in P2).
//!   - `fleet:codex-model`       — model-selection cheat-sheet, analogue of
//!                                 [`crate::model_guidance`] (added in P2).
//!
//! [`reconcile_codex_agents_md`] is the single writer: given which concepts are
//! enabled it composes exactly those blocks (in a stable order), strips any
//! Fleet-managed block that should be absent, migrates away the **legacy**
//! monolithic `fleet:codex-guidance` block, and preserves any user-authored
//! AGENTS.md content outside the markers verbatim.  It is idempotent and
//! order-independent, so the desktop can call it on toggle-change, on startup,
//! and before every spawn without accumulating content.
//!
//! Why purpose-built compact renderers instead of reusing the Claude ones?
//! AGENTS.md has a 32 KiB limit (`project_doc_max_bytes`).  The Claude
//! PRD-discipline text alone renders to ~29 KB and the interaction-mode text to
//! ~20 KB; concatenated they blow past 32 KiB.  Each `render_codex_*_block`
//! renders a compact codex-tuned variant that drops the Claude-only mechanics
//! (AskUserQuestion/ToolSearch deferral, `@import` sentinels, hook internals)
//! while keeping every rule a codex session can actually act on.

use std::fs;
use std::path::PathBuf;

// Per-concept sentinels. Each block is independently composable by
// [`reconcile_codex_agents_md`].
const PRD_BEGIN: &str = "<!-- fleet:codex-prd:begin -->";
const PRD_END: &str = "<!-- fleet:codex-prd:end -->";
const INTERACTION_BEGIN: &str = "<!-- fleet:codex-interaction:begin -->";
const INTERACTION_END: &str = "<!-- fleet:codex-interaction:end -->";

// Legacy monolithic block (PRD + interaction packed together). Pre-dates the
// per-concept split; [`reconcile_codex_agents_md`] strips it on first run so old
// installs migrate to the new structure automatically.
const LEGACY_BEGIN: &str = "<!-- fleet:codex-guidance:begin -->";
const LEGACY_END: &str = "<!-- fleet:codex-guidance:end -->";

/// All Fleet-managed marker pairs, used to strip the file down to user content
/// before recomposing. Extend this when adding a new per-concept block.
const FLEET_MARKERS: &[(&str, &str)] = &[
    (PRD_BEGIN, PRD_END),
    (INTERACTION_BEGIN, INTERACTION_END),
    (LEGACY_BEGIN, LEGACY_END),
];

/// Resolve the codex home dir (`$CODEX_HOME` or `~/.codex`), mirroring
/// [`crate::codex_source`] and [`crate::codex_launch`].
fn codex_home() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("CODEX_HOME") {
        return Some(PathBuf::from(home));
    }
    crate::session::real_home_dir().map(|h| h.join(".codex"))
}

fn agents_md_path() -> Option<PathBuf> {
    codex_home().map(|d| d.join("AGENTS.md"))
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

/// Compact codex **PRD discipline** block body (no sentinel markers). Mirrors
/// [`crate::prd_discipline`] with the Claude-only mechanics dropped.
pub fn render_codex_prd_block(user_title: &str, locale: &str) -> String {
    let title = title_or_default(user_title);
    let (prd_lang, _) = language_lines(locale);

    format!(
        "# Fleet PRD Discipline for Codex (managed by Claw Fleet — do not edit this block)\n\
\n\
These rules govern how Codex sessions that Claw Fleet launched run multi-step \
plans and touch production code — the same discipline Fleet gives its Claude \
sessions. {prd_lang} Address the user as \"{title}\" throughout.\n\
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
still pending\") must live on disk so it survives. Fleet automatically \
prepends the workspace's active TASKS.md plans to the prompt of **every** \
turn it drives, so you always see the current plan state — but you must keep \
that file up to date.\n\
\n\
- When you decompose a task into 2+ subtasks, write it to \
`<workspace_root>/TASKS.md` BEFORE starting P1.\n\
- Update plans with the **`fleet plan`** subcommands, not by hand-editing \
markdown — they make the same file change AND attribute your session to the \
plan (so Fleet's UI shows your current P):\n\
  - `fleet plan create <id> --title \"...\" [--parent <parent-id>]` — add a \
new plan block and record this session as its executor. Pass `--parent` for \
side work spun off mid-plan.\n\
  - `fleet plan check <id> <P>` / `uncheck <id> <P>` — tick / untick a task.\n\
  - `fleet plan resume <id> [P]` — take over an existing plan you did not \
create and were not handed.\n\
  - `fleet plan add <id> <P> --text \"...\"` — append a pending task.\n\
  - `fleet plan list` / `get <id>` — read.\n\
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
last box of a plan created with `--parent`, Fleet re-points your focus to the \
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
NOT silently wrap up early. Register a relay:\n\
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
does nothing** — only the actual command spawns a successor. Never use \
scheduled-wakeup / cron to \"continue later\"; they no-op inside Fleet.\n\
\n\
## Rule 6 — Waiting on an external condition (`fleet watch`)\n\
\n\
Handoff continues *work*; `fleet watch` waits for an *event* — a CI run \
finishing, a build producing an artifact, a deploy going live. Do NOT sit in \
a foreground poll loop or a background task waiting for it: a Fleet turn ends \
and the process exits, so anything still waiting is lost. Register a watch and \
end the turn:\n\
\n\
```\n\
fleet watch create --until \"<shell cmd that exits 0 when done>\" --capture \"<shell cmd whose stdout to report>\" --note \"<what you await>\"\n\
```\n\
\n\
- A detached timer polls the condition; the moment it succeeds Fleet resumes \
THIS session and hands the captured output to your next turn. `fleet watch \
stop <id>` cancels. It inherits this session's model / effort / source, so a \
codex session resumes as codex.",
        title = title,
        prd_lang = prd_lang,
    )
}

/// Compact codex **interaction mode** block body (no sentinel markers). Mirrors
/// [`crate::interaction_mode`] but targets `fleet__ask` (codex has no
/// `AskUserQuestion` / ToolSearch deferral).
pub fn render_codex_interaction_block(user_title: &str, locale: &str) -> String {
    let title = title_or_default(user_title);
    let (_, ix_lang) = language_lines(locale);

    format!(
        "# Fleet Interaction Mode for Codex (managed by Claw Fleet — do not edit this block)\n\
\n\
{title} wants every wait-for-input moment delivered as a **decision card**, \
not plain text. When you would otherwise end a turn by yielding control back \
to {title} with a plain-text message, call **`fleet__ask`** instead. \
Mid-turn status lines (a one-sentence note before a tool call) stay as text; \
it's the **final surface** of a turn that must be a card. {ix_lang}\n\
\n\
`fleet__ask` is available from turn 1 (Fleet injects its MCP server at \
spawn/resume). It takes `{{ \"questions\": Question[] }}` — 1 to 4 questions, \
each with 2–4 `options` (do NOT add an \"Other\" option; the UI appends one). \
It is a superset of a plain question card and also supports `html` previews, \
`images`, and `formFields` — reach for those only when a rich preview or \
structured input is genuinely the better answer.\n\
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
auto-added \"Other\" already covers free input.\n\
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
- **When this whole part does NOT apply:** if `fleet__ask` is not in your \
toolset this turn (rare), respond with plain text as normal.",
        title = title,
        ix_lang = ix_lang,
    )
}

/// Wrap a block body in its sentinel markers with a trailing newline.
fn wrap(begin: &str, end: &str, body: &str) -> String {
    format!("{begin}\n{body}\n{end}\n")
}

/// The single writer for `~/.codex/AGENTS.md`. Composes exactly the enabled
/// concept blocks (stable order: PRD, interaction), strips every Fleet-managed
/// block that should be absent (including the legacy monolithic block), and
/// preserves user-authored content outside the markers. Idempotent and
/// order-independent. Deletes the file if the result would be empty.
pub fn reconcile_codex_agents_md(
    prd_on: bool,
    interaction_on: bool,
    user_title: &str,
    locale: &str,
) -> Result<(), String> {
    let agents_md = agents_md_path().ok_or("cannot determine codex home")?;
    let existing = fs::read_to_string(&agents_md).unwrap_or_default();
    let user_content = strip_all_fleet_blocks(&existing);

    let mut blocks = String::new();
    if prd_on {
        blocks.push_str(&wrap(
            PRD_BEGIN,
            PRD_END,
            &render_codex_prd_block(user_title, locale),
        ));
    }
    if interaction_on {
        if !blocks.is_empty() {
            blocks.push('\n');
        }
        blocks.push_str(&wrap(
            INTERACTION_BEGIN,
            INTERACTION_END,
            &render_codex_interaction_block(user_title, locale),
        ));
    }

    let new_content = compose(&user_content, &blocks);

    if new_content.trim().is_empty() {
        // Nothing left (no user content, no blocks) — remove rather than leave a
        // stray empty AGENTS.md that shadows nested project docs.
        if agents_md.exists() {
            fs::remove_file(&agents_md).map_err(|e| format!("remove AGENTS.md: {e}"))?;
        }
        return Ok(());
    }

    let dir = codex_home().ok_or("cannot determine codex home")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create codex home: {e}"))?;
    fs::write(&agents_md, new_content).map_err(|e| format!("write AGENTS.md: {e}"))?;
    Ok(())
}

/// Backward-compat shim: the legacy single "codex guidance" toggle applied both
/// PRD and interaction. Routed through the new reconcile writer so old callers
/// keep working while the per-concept toggles are wired up (P3–P5).
pub fn apply_codex_guidance(user_title: &str, locale: &str) -> Result<(), String> {
    reconcile_codex_agents_md(true, true, user_title, locale)
}

/// Backward-compat shim: remove all Fleet-managed blocks (per-concept + legacy).
pub fn remove_codex_guidance() -> Result<(), String> {
    reconcile_codex_agents_md(false, false, "", "en")
}

/// Whether the codex PRD-discipline block is present in `~/.codex/AGENTS.md`.
pub fn is_codex_prd_installed() -> bool {
    agents_md_contains(PRD_BEGIN)
}

/// Whether the codex interaction-mode block is present in `~/.codex/AGENTS.md`.
pub fn is_codex_interaction_installed() -> bool {
    agents_md_contains(INTERACTION_BEGIN)
}

/// Whether any Fleet-managed codex block (per-concept or legacy) is present.
/// Used by the desktop's setup-plan snapshot until the per-concept flags land.
pub fn is_codex_guidance_installed() -> bool {
    is_codex_prd_installed() || is_codex_interaction_installed() || agents_md_contains(LEGACY_BEGIN)
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
    // Stripping a block from the middle leaves the blank lines that surrounded
    // it stacked together; collapse any run of 3+ newlines to a single blank
    // line so recomposition starts from clean spacing.
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
        let g = render_codex_prd_block("Boss", "en");
        assert!(
            g.contains("git worktree add -b prd/"),
            "must show worktree creation"
        );
        assert!(g.contains("git merge --no-ff"), "must mandate --no-ff merge back");
        assert!(
            g.contains("--squash") && g.contains("forbidden"),
            "must forbid --squash so codex doesn't substitute it"
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
    fn interaction_block_carries_fleet_ask_and_divider() {
        let g = render_codex_interaction_block("Boss", "en");
        assert!(
            g.contains("fleet__ask"),
            "interaction part must reference the fleet__ask tool"
        );
        assert!(
            g.contains("Speech Summary Divider") && g.contains("---"),
            "must teach the TTS divider rule"
        );
        assert!(
            g.contains("Session-end exemption"),
            "must keep the session-end plain-text exemption so codex doesn't loop cards forever"
        );
        // The interaction block must NOT drag in worktree/PRD mechanics — those
        // live in the PRD block now.
        assert!(
            !g.contains("git worktree add"),
            "interaction block must not contain PRD/worktree content"
        );
    }

    #[test]
    fn blocks_use_title_and_locale() {
        let prd = render_codex_prd_block("师父", "zh");
        assert!(prd.contains("师父"), "PRD block must interpolate the user title");
        assert!(
            prd.contains("中文书写"),
            "zh locale must select the Chinese TASKS.md line"
        );
        let ix = render_codex_interaction_block("", "en");
        assert!(ix.contains("Boss"), "empty title falls back to Boss");
        assert!(
            ix.contains("decision-card question and option text in English"),
            "en locale selects the English interaction language line"
        );
    }

    #[test]
    fn each_block_fits_under_agents_md_budget() {
        // AGENTS.md has a 32 KiB limit; both blocks together plus markers must
        // stay comfortably under it.
        let prd = render_codex_prd_block("老板", "zh");
        let ix = render_codex_interaction_block("老板", "zh");
        assert!(
            prd.len() + ix.len() < 30_000,
            "prd ({}) + interaction ({}) must stay well under 32 KiB",
            prd.len(),
            ix.len()
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
    fn strip_block_removes_one_pair_preserves_rest() {
        let input = format!("above\n\n{PRD_BEGIN}\nprd body\n{PRD_END}\n\nbelow\n");
        let out = strip_block(&input, PRD_BEGIN, PRD_END);
        assert!(!out.contains(PRD_BEGIN) && !out.contains("prd body"));
        assert!(out.contains("above") && out.contains("below"));
    }

    #[test]
    fn strip_all_removes_legacy_and_per_concept_blocks() {
        let input = format!(
            "user top\n\n{LEGACY_BEGIN}\nold monolithic\n{LEGACY_END}\n\n\
{PRD_BEGIN}\nprd\n{PRD_END}\n\n{INTERACTION_BEGIN}\nix\n{INTERACTION_END}\n\nuser bottom\n"
        );
        let out = strip_all_fleet_blocks(&input);
        assert!(!out.contains(LEGACY_BEGIN) && !out.contains("old monolithic"));
        assert!(!out.contains(PRD_BEGIN) && !out.contains("prd"));
        assert!(!out.contains(INTERACTION_BEGIN) && !out.contains("ix"));
        assert!(out.contains("user top") && out.contains("user bottom"));
        assert!(!out.contains("\n\n\n"), "collapses blank runs: {out:?}");
    }

    #[test]
    fn markers_are_distinct_across_modes() {
        assert_ne!(PRD_BEGIN, INTERACTION_BEGIN);
        assert_ne!(PRD_BEGIN, LEGACY_BEGIN);
        assert_ne!(INTERACTION_BEGIN, LEGACY_BEGIN);
        assert_ne!(PRD_BEGIN, "<!-- fleet:prd-discipline:begin -->");
        assert_ne!(INTERACTION_BEGIN, "<!-- fleet:interaction-mode:begin -->");
    }

    // --- reconcile_codex_agents_md: exercised against a temp CODEX_HOME ---
    //
    // These tests set $CODEX_HOME to an isolated temp dir. They run serially
    // (guarded by a mutex) because they mutate a process-global env var.

    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_temp_codex_home<T>(f: impl FnOnce(&PathBuf) -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap();
        let base = std::env::temp_dir().join(format!("fleet-codex-guidance-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let prev = std::env::var_os("CODEX_HOME");
        std::env::set_var("CODEX_HOME", &base);
        let out = f(&base);
        match prev {
            Some(v) => std::env::set_var("CODEX_HOME", v),
            None => std::env::remove_var("CODEX_HOME"),
        }
        let _ = fs::remove_dir_all(&base);
        out
    }

    #[test]
    fn reconcile_composes_only_enabled_blocks() {
        with_temp_codex_home(|base| {
            let agents = base.join("AGENTS.md");

            // interaction only
            reconcile_codex_agents_md(false, true, "Boss", "en").unwrap();
            let c = fs::read_to_string(&agents).unwrap();
            assert!(c.contains(INTERACTION_BEGIN) && !c.contains(PRD_BEGIN));

            // both
            reconcile_codex_agents_md(true, true, "Boss", "en").unwrap();
            let c = fs::read_to_string(&agents).unwrap();
            assert!(c.contains(PRD_BEGIN) && c.contains(INTERACTION_BEGIN));

            // prd only — interaction block must be gone
            reconcile_codex_agents_md(true, false, "Boss", "en").unwrap();
            let c = fs::read_to_string(&agents).unwrap();
            assert!(c.contains(PRD_BEGIN) && !c.contains(INTERACTION_BEGIN));

            // none — file removed
            reconcile_codex_agents_md(false, false, "Boss", "en").unwrap();
            assert!(!agents.exists(), "empty reconcile removes the file");
        });
    }

    #[test]
    fn reconcile_is_idempotent() {
        with_temp_codex_home(|base| {
            let agents = base.join("AGENTS.md");
            reconcile_codex_agents_md(true, true, "Boss", "en").unwrap();
            let once = fs::read_to_string(&agents).unwrap();
            reconcile_codex_agents_md(true, true, "Boss", "en").unwrap();
            let twice = fs::read_to_string(&agents).unwrap();
            assert_eq!(once, twice, "composing twice must not accumulate content");
            assert!(!once.contains("\n\n\n"), "no triple newline");
        });
    }

    #[test]
    fn reconcile_preserves_user_content_and_migrates_legacy() {
        with_temp_codex_home(|base| {
            let agents = base.join("AGENTS.md");
            // Seed a legacy monolithic block plus user content.
            let seed = format!(
                "# My project AGENTS\n\nkeep me.\n\n{LEGACY_BEGIN}\nold packed guidance\n{LEGACY_END}\n"
            );
            fs::write(&agents, seed).unwrap();

            reconcile_codex_agents_md(true, true, "Boss", "en").unwrap();
            let c = fs::read_to_string(&agents).unwrap();
            assert!(c.contains("keep me."), "user content preserved");
            assert!(!c.contains(LEGACY_BEGIN), "legacy monolithic block migrated away");
            assert!(!c.contains("old packed guidance"));
            assert!(
                c.contains(PRD_BEGIN) && c.contains(INTERACTION_BEGIN),
                "new blocks written"
            );
        });
    }

    #[test]
    fn is_installed_flags_track_disk() {
        with_temp_codex_home(|_base| {
            assert!(!is_codex_prd_installed() && !is_codex_interaction_installed());
            reconcile_codex_agents_md(true, false, "Boss", "en").unwrap();
            assert!(is_codex_prd_installed() && !is_codex_interaction_installed());
            assert!(is_codex_guidance_installed());
            reconcile_codex_agents_md(false, false, "Boss", "en").unwrap();
            assert!(!is_codex_guidance_installed());
        });
    }
}
