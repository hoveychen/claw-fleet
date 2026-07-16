//! Codex guidance injection — writes a sentinel-wrapped block into the global
//! `~/.codex/AGENTS.md` (or `$CODEX_HOME/AGENTS.md`) so Fleet-driven Codex
//! sessions see the same discipline Claude sessions get.
//!
//! Two concerns are packed into one block:
//!   1. **PRD discipline** (commit / worktree / rhythm / handoff) — the codex
//!      analogue of [`crate::prd_discipline`].
//!   2. **Interaction mode** (`fleet__ask` decision cards) — the codex analogue
//!      of [`crate::interaction_mode`]. Codex reaches `fleet__ask` via the MCP
//!      server Fleet injects at spawn/resume (`fleet_decision_card_args`), so the
//!      tool is live from turn 1 — no `ToolSearch` deferred-loading dance.
//!
//! Why a purpose-built renderer instead of reusing
//! [`crate::prd_discipline::render_guidance`]?  AGENTS.md has a 32 KiB limit
//! (`project_doc_max_bytes`).  The Claude PRD-discipline text alone renders to
//! ~29 KB and the interaction-mode text to ~20 KB; concatenated they blow past
//! 32 KiB.  This module renders a compact codex-tuned combination (~18 KB) that
//! drops the Claude-only mechanics (AskUserQuestion/ToolSearch deferral,
//! `@import` sentinels, hook internals) while keeping every rule a codex session
//! can actually act on.
//!
//! Unlike `prd_discipline` (which writes a separate file and `@import`s it into
//! CLAUDE.md), codex reads AGENTS.md directly with no import mechanism, so the
//! guidance text lives **inline** inside the sentinel block.  Only the block
//! between the markers is Fleet's; any user-authored AGENTS.md content outside
//! the markers is preserved verbatim.

use std::fs;
use std::path::PathBuf;

const BEGIN_MARKER: &str = "<!-- fleet:codex-guidance:begin -->";
const END_MARKER: &str = "<!-- fleet:codex-guidance:end -->";

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

/// Build the compact codex AGENTS.md guidance (PRD discipline + `fleet__ask`
/// interaction mode). Kept well under the 32 KiB AGENTS.md limit.
pub fn render_codex_guidance(user_title: &str, locale: &str) -> String {
    let title = if user_title.is_empty() {
        "Boss".to_string()
    } else {
        user_title.to_string()
    };

    let language_line = match locale {
        "zh" => "本规则配套的 TASKS.md、决策卡文案都用中文书写。",
        "ja" => "本ルールに対応する TASKS.md・意思決定カードは日本語で書いてください。",
        "ko" => "이 규칙과 짝을 이루는 TASKS.md·결정 카드는 한국어로 작성하세요.",
        _ => "Write the paired TASKS.md and decision cards in English.",
    };

    format!(
        "# Fleet Guidance for Codex (managed by Claw Fleet — do not edit this block)\n\
\n\
These rules govern Codex sessions that Claw Fleet launched. They mirror the \
discipline Fleet gives its Claude sessions. Two parts: **PRD discipline** \
(how to run multi-step plans and touch production code) and **Interaction \
mode** (how to end a turn — via a `fleet__ask` decision card). {language_line}\n\
\n\
Address the user as \"{title}\" throughout.\n\
\n\
---\n\
\n\
# Part 1 — PRD Discipline\n\
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
codex session resumes as codex.\n\
\n\
---\n\
\n\
# Part 2 — Interaction Mode (`fleet__ask` decision cards)\n\
\n\
{title} wants every wait-for-input moment delivered as a **decision card**, \
not plain text. When you would otherwise end a turn by yielding control back \
to {title} with a plain-text message, call **`fleet__ask`** instead. \
Mid-turn status lines (a one-sentence note before a tool call) stay as text; \
it's the **final surface** of a turn that must be a card.\n\
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
toolset this turn (rare), respond with plain text as normal.\n\
",
        title = title,
        language_line = language_line,
    )
}

/// Apply codex guidance: write the sentinel-wrapped block into
/// `~/.codex/AGENTS.md`, preserving any user content outside the block.
/// Idempotent.
pub fn apply_codex_guidance(user_title: &str, locale: &str) -> Result<(), String> {
    let dir = codex_home().ok_or("cannot determine codex home")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create codex home: {e}"))?;

    let agents_md = agents_md_path().ok_or("cannot determine codex home")?;
    let existing = fs::read_to_string(&agents_md).unwrap_or_default();
    let guidance = render_codex_guidance(user_title, locale);
    let block = format!("{BEGIN_MARKER}\n{guidance}\n{END_MARKER}\n");
    let new_content = compose_agents_md(&existing, &block);
    fs::write(&agents_md, new_content).map_err(|e| format!("write AGENTS.md: {e}"))?;
    Ok(())
}

/// Remove codex guidance: strip the sentinel block, leaving the rest of
/// AGENTS.md intact. Deletes the file only if it becomes empty. Idempotent.
pub fn remove_codex_guidance() -> Result<(), String> {
    let Some(agents_md) = agents_md_path() else {
        return Ok(());
    };
    let Ok(existing) = fs::read_to_string(&agents_md) else {
        return Ok(());
    };
    let stripped = strip_sentinel_block(&existing);
    if stripped == existing {
        return Ok(());
    }
    if stripped.trim().is_empty() {
        // The whole file was Fleet's block — remove it rather than leave a
        // stray empty AGENTS.md that shadows nested project docs.
        fs::remove_file(&agents_md).map_err(|e| format!("remove AGENTS.md: {e}"))?;
    } else {
        fs::write(&agents_md, stripped).map_err(|e| format!("write AGENTS.md: {e}"))?;
    }
    Ok(())
}

/// Whether the sentinel block is present in `~/.codex/AGENTS.md`.
pub fn is_codex_guidance_installed() -> bool {
    let Some(agents_md) = agents_md_path() else {
        return false;
    };
    let Ok(content) = fs::read_to_string(&agents_md) else {
        return false;
    };
    content.contains(BEGIN_MARKER) && content.contains(END_MARKER)
}

/// Re-attach the sentinel block to AGENTS.md content: strip any prior block,
/// then append `block` separated by one blank line from user content.
fn compose_agents_md(existing: &str, block: &str) -> String {
    let stripped = strip_sentinel_block(existing);
    if stripped.trim().is_empty() {
        block.to_string()
    } else {
        format!("{base}\n\n{block}", base = stripped.trim_end_matches('\n'))
    }
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
    fn render_fits_under_agents_md_limit() {
        // AGENTS.md has a 32 KiB (project_doc_max_bytes) limit; the sentinel
        // markers add a little, so budget the rendered body well under it.
        let g = render_codex_guidance("老板", "zh");
        assert!(
            g.len() < 30_000,
            "codex guidance must stay well under the 32 KiB AGENTS.md limit, got {} bytes",
            g.len()
        );
    }

    #[test]
    fn render_carries_both_parts() {
        let g = render_codex_guidance("Boss", "en");
        assert!(
            g.contains("Part 1 — PRD Discipline"),
            "must include PRD discipline part"
        );
        assert!(
            g.contains("Part 2 — Interaction Mode"),
            "must include the fleet__ask interaction part (Boss's 2026-07-16 decision)"
        );
    }

    #[test]
    fn render_carries_worktree_workflow() {
        let g = render_codex_guidance("Boss", "en");
        assert!(
            g.contains("git worktree add -b prd/"),
            "must show worktree creation"
        );
        assert!(
            g.contains("git merge --no-ff"),
            "must mandate --no-ff merge back"
        );
        assert!(
            g.contains("--squash") && g.contains("forbidden"),
            "must forbid --squash so codex doesn't substitute it"
        );
        assert!(g.contains("git worktree remove"), "must show cleanup step");
    }

    #[test]
    fn render_carries_commit_and_rhythm_rules() {
        let g = render_codex_guidance("Boss", "en");
        assert!(g.contains("Rule 1") && g.contains("Commit discipline"));
        assert!(g.contains("Rule 4") && g.contains("Execution rhythm"));
        assert!(
            g.contains("should I continue"),
            "Rule 4 must name the forbidden progress-report checkpoint pattern"
        );
    }

    #[test]
    fn render_carries_fleet_ask_and_divider() {
        let g = render_codex_guidance("Boss", "en");
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
    }

    #[test]
    fn render_teaches_handoff_and_fleet_plan() {
        let g = render_codex_guidance("Boss", "en");
        assert!(
            g.contains("fleet handoff"),
            "must teach the Rule 5 handoff relay"
        );
        assert!(
            g.contains("fleet plan check") && g.contains("fleet plan create"),
            "must teach the fleet plan subcommands"
        );
    }

    #[test]
    fn render_uses_title_and_locale() {
        let g = render_codex_guidance("师父", "zh");
        assert!(g.contains("师父"), "must interpolate the user title");
        assert!(
            g.contains("中文书写"),
            "zh locale must select the Chinese language line"
        );
        let g2 = render_codex_guidance("", "en");
        assert!(g2.contains("Boss"), "empty title falls back to Boss");
        assert!(
            g2.contains("in English"),
            "en locale selects the English language line"
        );
    }

    #[test]
    fn render_distinct_marker_from_other_modes() {
        // AGENTS.md / CLAUDE.md sentinels must not collide across modes.
        assert_ne!(BEGIN_MARKER, "<!-- fleet:prd-discipline:begin -->");
        assert_ne!(BEGIN_MARKER, "<!-- fleet:interaction-mode:begin -->");
    }

    #[test]
    fn compose_preserves_user_content_and_is_idempotent() {
        let block = format!("{BEGIN_MARKER}\ncodex rules here\n{END_MARKER}\n");
        let existing = "# My project\n\nUser-authored AGENTS.md instructions.\n";
        let once = compose_agents_md(existing, &block);
        assert!(
            once.contains("User-authored AGENTS.md instructions."),
            "must keep user content"
        );
        assert!(once.contains("codex rules here"), "must add Fleet block");
        let twice = compose_agents_md(&once, &block);
        assert_eq!(
            once, twice,
            "composing twice must not accumulate content or blank lines"
        );
        assert!(!once.contains("\n\n\n"), "no triple newline: {once:?}");
    }

    #[test]
    fn strip_removes_block_preserves_rest() {
        let input = format!(
            "user content above\n\n{BEGIN_MARKER}\ncodex rules\n{END_MARKER}\n\nuser content below\n"
        );
        let out = strip_sentinel_block(&input);
        assert!(!out.contains(BEGIN_MARKER));
        assert!(!out.contains(END_MARKER));
        assert!(!out.contains("codex rules"));
        assert!(out.contains("user content above"));
        assert!(out.contains("user content below"));
    }

    #[test]
    fn strip_noop_when_absent() {
        let input = "plain AGENTS.md\nno markers here\n";
        assert_eq!(strip_sentinel_block(input), input);
    }
}
