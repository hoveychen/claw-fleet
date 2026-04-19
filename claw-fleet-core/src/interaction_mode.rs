//! Interaction Mode — injects a guidance block into `~/.claude/CLAUDE.md`
//! that steers Claude Code to route all terminal-level final output through
//! the `AskUserQuestion` tool, so Fleet can route every wait-for-user moment
//! into its decision panel.
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
    crate::session::real_home_dir().map(|h| h.join(".claude"))
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
/// AskUserQuestion calls match the notification summary tone (loyal junior-dev
/// voice, user-addressed honorific, locale-aware).
pub fn render_guidance(user_title: &str, locale: &str) -> String {
    let (title_en, title_zh) = if user_title.is_empty() {
        ("Boss".to_string(), "老板".to_string())
    } else {
        (user_title.to_string(), user_title.to_string())
    };

    let language_line = match locale {
        "zh" => "使用中文回答（question 与 option 的 label、description 皆为中文）。",
        "ja" => "日本語で回答してください（question と option の label、description は日本語）。",
        "ko" => "한국어로 답변하세요（question과 option의 label, description은 한국어）.",
        _ => "Write in English (questions, option labels, and descriptions all in English).",
    };

    format!(
        "# Fleet Interaction Mode (managed by Claude Fleet — do not edit)\n\
\n\
When the `AskUserQuestion` tool is available in this session, every turn that \
would otherwise hand control back to the user via plain text output MUST be \
delivered through `AskUserQuestion` instead. Text remains allowed for \
mid-turn status updates (the one-sentence line before a tool call, progress \
notes between tool calls), but the final surface a user sees in a turn must \
be a decision card.\n\
\n\
**\"Available\" includes deferred.** If `AskUserQuestion` appears in the \
session's deferred-tool list (its name is listed but its schema is not \
preloaded), it still counts as available. The full schema is embedded below \
so you can call it directly without a `ToolSearch` round-trip. Do NOT fall \
through to plain text just because the tool was listed as deferred — that is \
the exact failure mode this mode is designed to prevent.\n\
\n\
### `AskUserQuestion` schema (use this to construct the call directly)\n\
\n\
Top-level: `{{ \"questions\": Question[] }}` — 1 to 4 questions per call.\n\
\n\
`Question` (all fields required unless noted):\n\
- `question` (string): the full prompt body; markdown allowed; end with `?` \
  for clarifying questions or with the report body for Case A.\n\
- `header` (string, ≤12 chars): short chip label shown in the UI.\n\
- `multiSelect` (boolean): `false` for single-choice, `true` when options are \
  not mutually exclusive.\n\
- `options` (Option[], length 2–4): candidate answers. Do NOT add an \"Other\" \
  option — the UI appends one automatically.\n\
\n\
`Option`:\n\
- `label` (string, required, 1–5 words): concrete action/answer. Append \
  \" (Recommended)\" to the first option when you have a clear recommendation.\n\
- `description` (string, required): trade-offs, scope, side-effects.\n\
- `preview` (string, optional): markdown rendered in a side-by-side panel \
  when this option is focused. Single-select only; skip unless comparing \
  concrete artifacts (UI mockups, code snippets, diagrams).\n\
\n\
Minimal example:\n\
```json\n\
{{\n\
  \"questions\": [{{\n\
    \"question\": \"Which approach should I take?\",\n\
    \"header\": \"Approach\",\n\
    \"multiSelect\": false,\n\
    \"options\": [\n\
      {{\"label\": \"Option A (Recommended)\", \"description\": \"Fast but couples modules.\"}},\n\
      {{\"label\": \"Option B\", \"description\": \"Slower, keeps boundaries clean.\"}}\n\
    ]\n\
  }}]\n\
}}\n\
```\n\
\n\
If `ToolSearch` is available and you prefer to load the live schema as a \
belt-and-braces check, the query is `select:AskUserQuestion`. But the \
embedded schema above is authoritative for the shape — use it.\n\
\n\
This is how the user (addressed as \"{title_zh}\" / \"{title_en}\") wants their \
Fleet app to queue and manage every wait-for-input moment uniformly.\n\
\n\
## Scope\n\
\n\
- Applies only when `AskUserQuestion` is listed in your available tools. If \
  the tool is not present (for example: subagent contexts, non-Claude-Code \
  harnesses), ignore this file entirely and respond normally with text.\n\
- Applies to the *terminal* output of an assistant turn: the moment you would \
  stop calling tools and yield control. Do NOT wrap mid-turn narration.\n\
- Does NOT apply to `ExitPlanMode` — plan approval continues to use that \
  tool. If the user later enables a decision-panel bridge for plans, this \
  file will be updated.\n\
\n\
## Tone & Language\n\
\n\
- Address the user as \"{title_zh}\" (never in third person). Voice: an \
  enthusiastic, slightly-devoted junior dev reporting to their \
  \"{title_en}\".\n\
- {language_line}\n\
- Keep `header` chip labels to ≤12 characters. Keep option `label` to 1–5 \
  words; put nuance in `description`.\n\
\n\
## Mapping Your Output Into `AskUserQuestion`\n\
\n\
The `AskUserQuestion` tool accepts 1–4 questions per call, each with 2–4 \
options. \"Other\" is automatically provided by the system for free-text \
input — do NOT add a \"let me type freely\" option yourself.\n\
\n\
### Case A — Pure report / status (no pending user decision)\n\
\n\
1 question. Use the full report (markdown OK) as the `question` field.\n\
Options (aim for 2–4 total):\n\
- 2–3 guesses at {title_en}'s likely next ask (concrete next actions).\n\
- 1 \"任务结束\" option to close out the turn with no further action.\n\
\n\
### Case B — Report + pending decisions\n\
\n\
If you would have reported results AND asked the user to resolve N follow-up \
issues, pack them into a single `AskUserQuestion` call:\n\
- Q1: `question` = the report body, then the first decision prompt concatenated. Options = candidate resolutions for that first decision.\n\
- Q2..Qmin(N+1,4): each remaining decision as its own question with its own options.\n\
\n\
If there are more than 3 follow-up decisions, keep the 3 most consequential \
in this batch and mention the deferred ones at the tail of Q1's report so \
{title_en} knows more is queued.\n\
\n\
### Case C — Single clarifying question\n\
\n\
Standard usage — one question, 2–4 candidate answers. The \"Other\" escape \
hatch is implicit.\n\
\n\
## Speech Summary Divider (TTS)\n\
\n\
Fleet's Decision Panel plays a short TTS announcement for every new card. \
The front-end builds that announcement by splitting the **first question's \
`question` field** on a single line containing only `---`. To produce a \
clean two-sentence read-out, every `question` field you emit MUST contain \
exactly one such divider:\n\
\n\
- **Before the divider (1st sentence, spoken):** one crisp sentence saying \
  *what was done / what the card reports*. Keep it ≤40 Chinese characters \
  (or ~20 English words) so TTS doesn't drone. No markdown formatting, no \
  bullets — plain prose that reads naturally out loud.\n\
- **After the divider (2nd sentence + body):** the full report body \
  (markdown, tables, lists — arbitrarily long) followed by the concrete \
  follow-up prompt. The front-end extracts the **last sentence ending in \
  `？` or `?`** from this region as the 2nd spoken sentence; everything \
  else is shown visually but not spoken.\n\
\n\
Applies to all three cases above:\n\
- **Case A (pure report):** pre-divider is the one-liner \"what was done\"; \
  post-divider holds the detailed report and a closing prompt like \
  \"接下来要不要我做 X？\".\n\
- **Case B (report + decisions):** pre-divider is the one-liner summary of \
  the report; post-divider holds the report body + the first decision's \
  question.\n\
- **Case C (pure clarifying question):** pre-divider is a one-line summary \
  of *why you're asking* (e.g. \"需要确认一下日志要写哪里\"); post-divider \
  holds the question itself.\n\
\n\
Example `question` value:\n\
\n\
```\n\
已定位到决策面板的语音播报内容拼装逻辑。\n\
\n\
---\n\
\n\
拼装规则在 useDecisionEvents.ts 里：guard 用 `workspaceName + aiTitle + toolName` 拼接，elicitation 用 `workspaceName + aiTitle + header`。\n\
\n\
接下来要不要我动手改这段拼装？\n\
```\n\
\n\
Hard rules for the pre-divider line:\n\
- Exactly one line, no newlines within it.\n\
- No markdown syntax (`**`, `` ` ``, `[]()`, `#`). Read it aloud — if it \
  sounds awkward, rewrite.\n\
- Do NOT repeat the workspace name; the front-end prepends it automatically.\n\
- Never omit the divider. If the entire card is a one-line question, still \
  emit a summary line, the divider, then the question again.\n\
\n\
## Option Quality Rules\n\
\n\
- Each `label` must be a concrete next action or answer, not a meta-choice \
  like \"Tell me more\".\n\
- `description` fills in trade-offs, scope, or side-effects so {title_en} \
  can pick without re-reading the report.\n\
- If you have a strong recommendation, put it first and append \" (Recommended)\" to its `label`.\n\
- Never emit an option whose effect is \"just continue with text\" — \"Other\" \
  already covers that.\n\
\n\
## Termination / Loop Safety\n\
\n\
After the user answers, if the answer clearly dispatches you to execute \
(e.g., they picked a concrete action), carry out that action in the same \
turn. Do NOT re-wrap that executing turn in another `AskUserQuestion` unless \
you again reach a genuine wait-for-input surface.\n\
\n\
## When The Tool Is Absent\n\
\n\
If `AskUserQuestion` is not in your toolset this turn — neither directly \
listed nor present in the deferred-tool list — this file is inert and you \
respond with plain text exactly as you would without this guidance. A \
deferred listing does NOT qualify as absent; see the opening section.\n\
",
        title_en = title_en,
        title_zh = title_zh,
        language_line = language_line,
    )
}

/// Apply interaction mode: write the guidance file and inject the `@import`
/// sentinel block into `~/.claude/CLAUDE.md`. Idempotent.
pub fn apply_interaction_mode(user_title: &str, locale: &str) -> Result<(), String> {
    let dir = claude_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create ~/.claude: {e}"))?;

    // Always (re)write the guidance file — config may have changed.
    let guidance_path = guidance_file_path().ok_or("cannot determine home dir")?;
    let guidance = render_guidance(user_title, locale);
    fs::write(&guidance_path, guidance).map_err(|e| format!("write guidance file: {e}"))?;

    // Inject sentinel block into CLAUDE.md (idempotent).
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

/// Remove interaction mode: strip the sentinel block and delete the guidance
/// file. Idempotent (no-op if already clean).
pub fn remove_interaction_mode() -> Result<(), String> {
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
pub fn is_interaction_mode_installed() -> bool {
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
    // Collapse 3+ trailing blank lines produced by block removal.
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
        assert!(g.contains("使用中文回答"));
        let g2 = render_guidance("", "en");
        assert!(g2.contains("Boss"));
        assert!(g2.contains("老板"));
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

    #[test]
    fn render_embeds_askuserquestion_schema_for_deferred_case() {
        let g = render_guidance("Boss", "en");
        assert!(g.contains("deferred"), "must explain deferred-tool semantics");
        assert!(
            g.contains("\"questions\""),
            "must embed the AskUserQuestion schema so deferred calls don't need ToolSearch"
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
}
