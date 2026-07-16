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
The work ends by merging the worktree branch back to main as one atomic \
step. For multi-step plans, that merge is the final P-task; for \
single-step changes, it is simply the finishing move once tests are green.\n\
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
  throughout. For multi-step plans, P1..P(n-1) happen here and the final \
  P-task is the merge back to main. For single-step changes, the entire \
  change happens here and you merge back when tests are green.\n\
- **Intermediate commits inside the worktree are explicitly allowed and do \
  NOT violate Rule 1.** Rule 1's \"no proactive commit\" applies to *main*; \
  commits on `prd/<task-id>` inside the worktree are progress markers that \
  no other work can see. For multi-step plans, commit between P-tasks \
  whenever it helps you reason about the next step (e.g. `git diff HEAD~1` \
  when a later P-task regresses behaviour). For single-step changes, you \
  may also break the work into multiple commits inside the worktree if it \
  helps debugging. You still do not need to *ask* {title} for permission \
  to commit inside the worktree — it's free movement on a private branch.\n\
- **The work ends with one atomic merge back to main.** For multi-step \
  plans this is the final P-task; for single-step changes it is simply the \
  finishing move once tests are green. From the main checkout:\n\
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
### Worked example — single-step change\n\
\n\
{title} asks for a single mechanical fix: `LocalBackend::new` should call \
`migrate_zombie_running` once on startup. No multi-step plan, no \
TASKS.md, no P1..Pn — but Rule 3 still applies. Picking \
`fix-zombie-pid` as the `task-id`, the full workflow from the repo root \
is:\n\
\n\
```bash\n\
# 1. Open the worktree off the current main\n\
git worktree add -b prd/fix-zombie-pid .worktrees/fix-zombie-pid main\n\
\n\
# 2. Do the work inside the worktree\n\
cd .worktrees/fix-zombie-pid\n\
#   ... edit src/local_backend.rs, add the unit test ...\n\
cargo test --package claw-fleet-core\n\
git add -A\n\
git commit -m \"fix(supervisor): migrate zombie running sessions on startup\"\n\
\n\
# 3. Switch back to the main checkout and merge with --no-ff\n\
cd <repo-root>\n\
git merge --no-ff prd/fix-zombie-pid\n\
\n\
# 4. Clean up — mandatory, orphaned worktrees accumulate fast\n\
git worktree remove .worktrees/fix-zombie-pid\n\
git branch -d prd/fix-zombie-pid\n\
```\n\
\n\
After step 3, `main` carries one merge commit summarising the fix plus the \
single worktree commit beneath it — the same shape a multi-step plan \
produces, just with one underlying commit instead of N. Steps 1 and 4 are \
the parts agents most often skip; do not skip them.\n\
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
## Rule 5 — Long-context handoff (`fleet handoff`)\n\
\n\
When your context window is running long mid-plan (context usage high, or \
compaction has already fired), do NOT grind on until the window dies, do NOT \
silently wrap up early, and do NOT leave \"hand off to the next session\" \
notes that nothing acts on. Fleet has a first-class relay:\n\
\n\
```\n\
fleet handoff --note \"<交接信息>\" [--plan <plan-id>] [--next <P>]\n\
```\n\
\n\
- **--note is mandatory** and is everything the successor knows beyond \
TASKS.md: what's done, what's in flight, key files, gotchas, the next \
concrete step. Write it like a shift-change briefing.\n\
- **Pass --plan/--next when the work is a TASKS.md plan** so Fleet attributes \
the successor to that plan and P automatically; it resumes the rhythm there \
without any `fleet plan` ceremony of its own.\n\
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
turn, no successor spawns and the plan dies silently the moment you yield — \
the same \"claimed it, never did it\" failure the honesty rules exist to \
kill, just aimed at your own relay. So the LAST thing you do before ending \
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
- **Java / Kotlin**: Gradle and Maven caches (`~/.gradle/caches`, \
  `~/.m2/repository`) are global by default; no action needed.\n\
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
    fn render_rule_3_includes_single_step_worked_example() {
        let g = render_guidance("Boss", "en");
        let r3_pos = g.find("## Rule 3").expect("Rule 3 must exist");
        let r3_end = g[r3_pos..].find("## Rule 4").expect("Rule 4 must follow");
        let r3_body = &g[r3_pos..r3_pos + r3_end];
        assert!(
            r3_body.contains("### Worked example"),
            "Rule 3 must include a worked single-step example so agents have a copy-pastable command line to follow"
        );
        assert!(
            r3_body.contains("git worktree add -b prd/")
                && r3_body.contains("git merge --no-ff prd/")
                && r3_body.contains("git worktree remove")
                && r3_body.contains("git branch -d"),
            "the worked example must show all four mandatory steps end-to-end (add, merge, remove worktree, delete branch)"
        );
        assert!(
            r3_body.contains("```bash"),
            "the worked example must be inside a fenced bash block so it renders correctly and agents recognise it as runnable"
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
