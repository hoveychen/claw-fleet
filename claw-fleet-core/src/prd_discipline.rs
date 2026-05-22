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

    format!(
        "# Fleet PRD Discipline (managed by Claude Fleet — do not edit)\n\
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
progress, want to review before P4?\". {title} can already see TASKS.md \
checkboxes and (when Rule 3 is active) worktree commits — the progress is \
legible without {title}'s interruption.\n\
\n\
## Rule 1 — Commit discipline during multi-step plans\n\
\n\
A **multi-step plan** here means: any task you decomposed into 2 or more \
sequential subtasks (P1, P2, ..., Pn — or numbered todos, or any equivalent). \
Once you are inside such a plan, the following rules apply until the plan is \
fully done:\n\
\n\
- **DO NOT proactively propose `git commit`.** Not after P1, not after P2, \
not at any \"natural checkpoint\" you sense. The plan is the unit of work, \
not the individual P-task.\n\
- **DO NOT actually run `git commit` either**, except in the two cases below.\n\
- **You MAY commit only when:**\n\
  1. {title} explicitly asks for a commit in this turn, OR\n\
  2. You have just finished the **last** P-task in the plan (i.e. all items \
     in TASKS.md are checked) AND you have surfaced that the plan is complete \
     to {title}. When Rule 3 (worktree workflow) is active, this single \
     allowed commit on main takes the form of `git merge --no-ff` from the \
     worktree branch — see Rule 3 for the exact procedure.\n\
- **`git push` is always gated** — never push without {title}'s explicit \
  approval in the current turn, regardless of plan state.\n\
- **Why:** {title} got tired of being interrupted with \"shall I commit?\" \
  questions every 5–10 minutes during long PRDs. Each interruption costs a \
  full conversational turn and the agent often forgets remaining work after \
  the commit succeeds (the green hash feels like \"done\"). Treating the \
  whole plan as one unit eliminates the interruption pattern and the \
  post-commit amnesia.\n\
\n\
### What \"finished\" means\n\
\n\
Plan completion requires ALL of:\n\
- Every P-task / numbered todo is marked complete in TASKS.md (see Rule 2).\n\
- Build / type-check / tests have been run (per the project's existing rules).\n\
- A summary of what changed has been surfaced to {title}.\n\
\n\
Until those three are true, the plan is not done; do not propose committing.\n\
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
<!-- fleet:prd:begin id=\"auth-refactor\" -->\n\
\n\
**Plan:** Migrate session middleware to the new auth crate\n\
\n\
- [x] **P1** — Audit existing call sites\n\
- [ ] **P2** — Swap middleware impl\n\
- [ ] **P3** — Update integration tests\n\
\n\
<!-- fleet:prd:end id=\"auth-refactor\" -->\n\
\n\
<!-- fleet:prd:begin id=\"prd-multiplan\" -->\n\
\n\
**Plan:** Teach TASKS.md to host parallel plans\n\
\n\
- [ ] **P1** — New sentinel format with `id=\"...\"`\n\
- [ ] **P2** — Hook scans all blocks and re-injects each\n\
\n\
<!-- fleet:prd:end id=\"prd-multiplan\" -->\n\
```\n\
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
Rules of thumb for the format itself:\n\
- Use `- [ ]` for pending and `- [x]` for done. Don't invent new statuses; \
  the simple checkbox is the contract.\n\
- Keep P-task titles ≤ 60 chars. Long acceptance notes go in sub-bullets.\n\
- {language_line}\n\
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
A multi-step plan that touches production code MUST be developed inside an \
isolated git worktree, not directly on the main checkout. The plan's final \
P-task is fixed: it merges the worktree branch back to main as one atomic step.\n\
\n\
**The contract:**\n\
\n\
- **Before P1**, create the worktree at `<repo-root>/.worktrees/<plan-id>` on \
  a fresh branch `prd/<plan-id>` based on the current main:\n\
\n\
  ```\n\
  git worktree add -b prd/<plan-id> .worktrees/<plan-id> main\n\
  ```\n\
\n\
  Use the same `plan-id` you picked for the TASKS.md sentinel block (Rule 2). \
  All P-tasks except the final one run inside this worktree; the main \
  checkout stays clean throughout the plan.\n\
- **Intermediate commits inside the worktree are explicitly allowed and do \
  NOT violate Rule 1.** Rule 1's \"no proactive commit\" applies to *main*; \
  commits on `prd/<plan-id>` inside the worktree are progress markers that no \
  other plan can see. Commit between P-tasks whenever it helps you reason \
  about the next step (e.g. `git diff HEAD~1` when a later P-task regresses \
  behaviour). You still do not need to *ask* {title} for permission to \
  commit inside the worktree — it's free movement on a private branch.\n\
- **The final P-task is fixed: merge the worktree branch back to main as one \
  atomic step.** From the main checkout:\n\
\n\
  ```\n\
  git merge --no-ff prd/<plan-id>\n\
  ```\n\
\n\
  The `--no-ff` is mandatory. `--ff-only` and `--squash` are forbidden — we \
  keep every worktree commit visible in main's history alongside a single \
  merge commit summarising the feature, so the plan stays auditable at \
  P-task granularity. This `git merge --no-ff` IS the single Rule-1-allowed \
  commit on main for the entire plan; do not run any additional `git commit` \
  before or after it.\n\
- **After a successful merge, clean up.** Run `git worktree remove \
  .worktrees/<plan-id>` then `git branch -d prd/<plan-id>`. If the merge \
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
Rule 3 covers plans that change production code (features, refactors, \
non-trivial bug fixes). It does NOT cover:\n\
- Pure documentation changes (READMEs, docstrings, changelogs).\n\
- Configuration-only changes (CI YAML, dotfiles, `.gitignore` itself, \
  formatter configs).\n\
- Single-step tasks (already exempted by Rule 1).\n\
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
After step 3, tick the TASKS.md checkbox and **proceed to the next P-task \
immediately**. Do NOT pause to summarise. Do NOT ask \"should I continue \
with P2?\" or \"want to review progress before P4?\". Do NOT offer \"I've \
written quite a few P-tasks now, want me to summarise?\". Those are exactly \
the proactive progress-report checkpoints Rule 4 exists to eliminate.\n\
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
### Failure mode this rule kills\n\
\n\
The interruption mode Rule 4 kills is *not* \"agent asks before destructive \
things\" or \"agent asks when genuinely stuck\" — those are healthy. What's \
killed is the **proactive progress-report checkpoint**: \"I've completed \
P1–P3, want me to review before P4?\", \"P4 is done, shall I continue?\", \
\"we've made a lot of progress, want me to summarise?\". The plan \
checkboxes and (under Rule 3) the worktree commits already show progress. \
The agent's job is to execute the plan, not narrate it.\n\
\n\
## When this mode does NOT apply\n\
\n\
- One-shot tasks where decomposition would be ceremony.\n\
- Pure conversation / Q&A turns where no code is changing.\n\
- Tasks {title} explicitly asks you to keep \"informal\" or \"quick\".\n\
\n\
In those cases, ignore all four rules — no TASKS.md, no worktree, no \
enforced rhythm, normal commit etiquette applies.\n\
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
    let stripped = strip_sentinel_block(&existing);
    let block = format!(
        "{begin}\n@{path}\n{end}\n",
        begin = BEGIN_MARKER,
        end = END_MARKER,
        path = guidance_path.display(),
    );
    let new_content = if stripped.is_empty() {
        block
    } else if stripped.ends_with('\n') {
        format!("{stripped}\n{block}")
    } else {
        format!("{stripped}\n\n{block}")
    };
    fs::write(&claude_md, new_content).map_err(|e| format!("write CLAUDE.md: {e}"))?;
    Ok(())
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
            g.contains("prd/<plan-id>"),
            "guidance must pin the branch-name convention so agents don't invent their own prefix"
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
    fn render_summary_section_references_all_four_rules() {
        let g = render_guidance("Boss", "en");
        assert!(
            g.contains("ignore all four rules"),
            "the 'When this mode does NOT apply' summary must escalate to four rules now that Rule 4 exists"
        );
    }
}
