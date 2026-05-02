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
This mode locks down two failure modes that hurt long multi-step plans:\n\
\n\
1. **Mid-plan commit nagging.** The agent finishes one P-task, gets a \"should \
I commit now?\" reflex, and {title} has to keep saying \"no, keep going.\"\n\
2. **Post-compression task amnesia.** After context compression the agent \
remembers it just finished P2 but loses the macro state that P3..Pn are still \
pending.\n\
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
     to {title}.\n\
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
  unsure of macro state), the file is automatically re-injected as a \
  system-reminder by Fleet's UserPromptSubmit hook — but you can also `Read` \
  it explicitly when you need it.\n\
- When the plan is fully complete, you may delete TASKS.md (or leave it for \
  history — {title}'s call when committing).\n\
\n\
**Format** — keep it dead simple, GitHub-task-list compatible:\n\
\n\
```markdown\n\
# TASKS\n\
\n\
<!-- fleet:prd:begin -->\n\
\n\
**Plan:** {{one-line description of the overall goal}}\n\
\n\
- [ ] **P1** — {{short title}}\n\
  - {{optional sub-bullets / acceptance notes}}\n\
- [ ] **P2** — {{short title}}\n\
- [x] **P3** — {{short title}} ← already done\n\
- [ ] **P4** — {{short title}}\n\
\n\
<!-- fleet:prd:end -->\n\
```\n\
\n\
Rules of thumb for the format:\n\
- Outermost sentinel `<!-- fleet:prd:begin -->` / `<!-- fleet:prd:end -->` \
  marks the active plan region — Fleet only re-injects what's between them. \
  Anything outside is ignored (you can keep historical plans below if you \
  want).\n\
- Use `- [ ]` for pending and `- [x]` for done. Don't invent new statuses; \
  the simple checkbox is the contract.\n\
- Keep P-task titles ≤ 60 chars. Long acceptance notes go in sub-bullets.\n\
- {language_line}\n\
\n\
## When this mode does NOT apply\n\
\n\
- One-shot tasks where decomposition would be ceremony.\n\
- Pure conversation / Q&A turns where no code is changing.\n\
- Tasks {title} explicitly asks you to keep \"informal\" or \"quick\".\n\
\n\
In those cases, ignore both rules — no TASKS.md needed, normal commit \
etiquette applies.\n\
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
}
