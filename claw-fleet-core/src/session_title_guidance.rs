//! Session-title guidance — the one place that words "name your own session".
//!
//! Both harnesses that can reach Fleet's MCP server ask the agent to title its
//! own session by calling `fleet__set_session_title`, which lands in
//! [`crate::session_title`] as an *agent* title (never clobbering a human
//! rename). The instruction used to live only inside
//! [`crate::codex_guidance::render_codex_interaction_block`]; Claude sessions
//! never got it, so their titles came entirely from Claude Code's own `ai-title`
//! record — and when that stopped being written (2026-09-06) every new session
//! showed up untitled.
//!
//! Rather than write the same paragraph a second time, the semantics live here
//! and each harness supplies only its own call-shape tail:
//!
//! - [`Harness::Claude`] — the Fleet MCP server is registered from turn 1 and
//!   the tool is called by name like any other.
//! - [`Harness::Codex`] — codex defers MCP tools, so the call must be made with
//!   the fully-qualified direct-call shape inside the outer `exec`.
//!
//! **dsh is deliberately absent.** A dsh session has no Fleet MCP server at all
//! (see [`crate::dsh_guidance`]), so `fleet__set_session_title` does not exist
//! there; its titles come from Fleet's own LLM titler instead. Adding dsh here
//! would mean bridging MCP into dsh first — a different piece of work.

use std::fs;
use std::path::PathBuf;

const BEGIN_MARKER: &str = "<!-- fleet:session-title:begin -->";
const END_MARKER: &str = "<!-- fleet:session-title:end -->";

fn claude_dir() -> Option<PathBuf> {
    crate::session::get_claude_dir()
}

fn guidance_file_path() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("fleet-session-title.md"))
}

fn claude_md_path() -> Option<PathBuf> {
    claude_dir().map(|d| d.join("CLAUDE.md"))
}

/// Which agent runtime the guidance is being rendered for. Only affects the
/// call-shape tail; the naming semantics above it are shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    Claude,
    Codex,
}

/// Render the `## Session title` section, heading included, for `harness`.
///
/// `user_title` is what agents call the user (already defaulted by the caller);
/// `locale` picks the Chinese variant for `"zh"` and English for everything
/// else.
pub fn render_session_title_section(user_title: &str, locale: &str, harness: Harness) -> String {
    if locale == "zh" {
        format!(
            "## 会话标题\n\
\n\
**在你结束第一个回合之前，必须调用一次 `fleet__set_session_title`** 给当前\
会话起一个简洁、有描述性的标题。不要等「主题稳定下来」再说——{title}的第一条\
消息已经足够你命名这件活了，而等下去的实际结果是永远不调：任务列表里那些\
「（无标题）」的会话，都是打算晚点再说的。\n\
\n\
标题要具体（点名那件具体的任务或问题），但不要照抄{title}的第一条消息，也\
不要用「帮忙」「写代码」这类放到几十个会话上都成立的泛标签，更不要反过来让\
{title}给标题。哪怕是一句话就能答完的小问题，也照样起一个——短会话在列表里\
一样要认得出来。如果对话主题后来发生了实质变化，再调用一次换成新标题；\
否则不要每轮都改名。\n\
\n\
{tail}\n",
            title = user_title,
            tail = tail(user_title, locale, harness),
        )
    } else {
        format!(
            "## Session title\n\
\n\
**Call `fleet__set_session_title` once before you end your first turn**, giving \
the current session a concise, descriptive title. Do not wait for the topic to \
\"settle\": {title}'s first message is already enough to name the work, and \
waiting reliably turns into never calling it at all — every \"(untitled)\" row \
in the task list is a session that meant to get around to it.\n\
\n\
Keep the title specific (name the concrete task or question), but do not merely \
copy {title}'s first message, avoid generic labels such as \"Help\" or \"Coding \
task\", and never ask {title} to supply a title. Title even the one-answer \
questions — a short session still has to be recognisable in the list. If the \
topic later changes materially, call it again with the new title; otherwise do \
not rename on every turn.\n\
\n\
{tail}\n",
            title = user_title,
            tail = tail(user_title, locale, harness),
        )
    }
}

/// The per-harness call-shape paragraph. Kept separate from the semantics above
/// so a wording change to "how to name a session" is a one-line edit that both
/// harnesses pick up.
fn tail(user_title: &str, locale: &str, harness: Harness) -> String {
    match (locale == "zh", harness) {
        (true, Harness::Claude) => format!(
            "Fleet 启动的会话从第 1 轮起就注册好了这个工具，按名直接调用即可，\
无需 `ToolSearch` 预加载。它是非阻塞的：若调用返回未注册／未知工具错误，\
说明本会话不是 Fleet 起的，直接继续手上的活，不要重试、也不要为此打断{title}。",
            title = user_title,
        ),
        (false, Harness::Claude) => format!(
            "In a Fleet-launched session this tool is registered from turn 1 — call it \
by name, no `ToolSearch` preload needed. It is non-blocking. If the call \
returns a not-registered / unknown-tool error, this session was not spawned by \
Fleet: continue the task without retrying or interrupting {title}.",
            title = user_title,
        ),
        (true, Harness::Codex) => format!(
            "和其他 Fleet MCP 工具一样，Codex 可能把它从上来那份工具清单里延迟掉；\
在外层 `exec` 里用确切的直调写法 \
`await tools.mcp__fleet__fleet__set_session_title({{ title: \"<简洁标题>\" }});` \
调用它。**不要去看 `ALL_TOOLS`、不要搜工具清单、不要按「看起来有没有」来决定\
调不调、也不要用动态查找：**延迟的 MCP 工具即使可调用也不会出现在那些清单里。\
这个工具是非阻塞的。若直调返回未注册／未知工具错误，直接继续手上的活，\
不要重试、也不要为此打断{title}。",
            title = user_title,
        ),
        (false, Harness::Codex) => format!(
            "Like other Fleet MCP tools, Codex may defer it from the upfront tool list; \
invoke it inside the outer `exec` with the exact direct-call shape \
`await tools.mcp__fleet__fleet__set_session_title({{ title: \"<concise title>\" }});`. \
**Do not inspect `ALL_TOOLS`, search tool lists, gate the call on apparent \
availability, or use dynamic lookup:** deferred MCP tools are absent from \
those lists even when they are callable. This tool is non-blocking. If the \
direct call returns a not-registered / unknown-tool error, continue the task \
without retrying or interrupting {title}.",
            title = user_title,
        ),
    }
}

/// The whole `~/.claude/fleet-session-title.md` file: the managed-file header
/// plus the Claude flavour of the shared section.
pub fn render_guidance(user_title: &str, locale: &str) -> String {
    let title = if user_title.trim().is_empty() {
        if locale == "zh" {
            "老板"
        } else {
            "Boss"
        }
    } else {
        user_title.trim()
    };
    let header = if locale == "zh" {
        "# Fleet 会话标题 (managed by Claw Fleet — do not edit)\n\n"
    } else {
        "# Fleet Session Title (managed by Claw Fleet — do not edit)\n\n"
    };
    format!(
        "{header}{section}",
        section = render_session_title_section(title, locale, Harness::Claude),
    )
}

/// Write the guidance file and inject its `@import` sentinel block into
/// `~/.claude/CLAUDE.md`. Idempotent.
pub fn apply_session_title_guidance(user_title: &str, locale: &str) -> Result<(), String> {
    crate::control_plane_prefs::note_intent(
        apply_inner(user_title, locale),
        crate::control_plane_prefs::Feature::SessionTitleGuidance,
        false,
    )
}

fn apply_inner(user_title: &str, locale: &str) -> Result<(), String> {
    let dir = claude_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create ~/.claude: {e}"))?;

    // Always (re)write the guidance file — title or locale may have changed.
    let guidance_path = guidance_file_path().ok_or("cannot determine home dir")?;
    fs::write(&guidance_path, render_guidance(user_title, locale))
        .map_err(|e| format!("write guidance file: {e}"))?;

    // Locked read-modify-write — see `claude_md_lock`.
    let claude_md = claude_md_path().ok_or("cannot determine home dir")?;
    let block = format!(
        "{begin}\n@{path}\n{end}\n",
        begin = BEGIN_MARKER,
        end = END_MARKER,
        path = guidance_path.display(),
    );
    crate::claude_md_lock::with_lock(&claude_md, || {
        let existing = fs::read_to_string(&claude_md).unwrap_or_default();
        let stripped = strip_sentinel_block(&existing);
        let new_content = if stripped.is_empty() {
            block
        } else if stripped.ends_with('\n') {
            format!("{stripped}\n{block}")
        } else {
            format!("{stripped}\n\n{block}")
        };
        crate::atomic_json::write_atomic(&claude_md, new_content.as_bytes()).map_err(|e| format!("write CLAUDE.md: {e}"))
    })
}

/// Strip the sentinel block and delete the guidance file. Idempotent.
pub fn remove_session_title_guidance() -> Result<(), String> {
    crate::control_plane_prefs::note_intent(
        remove_inner(),
        crate::control_plane_prefs::Feature::SessionTitleGuidance,
        true,
    )
}

fn remove_inner() -> Result<(), String> {
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

/// The `## Session title` section exactly as currently installed on disk, or
/// `None` when the feature is not installed.
///
/// For the one caller that needs the section but cannot reach the user's global
/// `CLAUDE.md`: a chat-workspace session launches with `--setting-sources
/// project`, which drops that file (and with it the `@import` sentinel block) to
/// keep the 22k-token engineering doctrine out of a conversation — see
/// [`crate::chat_workspace`]. Reading the rendered file back, rather than
/// re-rendering, is what keeps the user's configured `title`/`locale` and the
/// settings-panel toggle authoritative: `remove_session_title_guidance` deletes
/// this file, so a switched-off feature reads as `None` here too.
pub fn installed_section() -> Option<String> {
    let content = fs::read_to_string(guidance_file_path()?).ok()?;
    // Drop the managed-file `# …` header; keep from the `## …` heading on.
    let start = content
        .split_inclusive('\n')
        .scan(0usize, |offset, line| {
            let at = *offset;
            *offset += line.len();
            Some((at, line))
        })
        .find(|(_, line)| line.starts_with("## "))
        .map(|(at, _)| at)?;
    Some(content[start..].trim_end().to_string())
}

/// Whether the sentinel block is present in `~/.claude/CLAUDE.md`.
pub fn is_session_title_guidance_installed() -> bool {
    let Some(claude_md) = claude_md_path() else {
        return false;
    };
    let Ok(content) = fs::read_to_string(&claude_md) else {
        return false;
    };
    content.contains(BEGIN_MARKER) && content.contains(END_MARKER)
}

// Deliberately a module-local copy, matching `interaction_mode`,
// `prd_discipline`, `wiki_guidance` and `model_guidance`: each owns its own
// markers, and folding all five into one helper is a refactor of four unrelated
// modules, not part of this change.
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
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_names_the_tool_and_the_user() {
        for locale in ["en", "zh"] {
            for harness in [Harness::Claude, Harness::Codex] {
                let s = render_session_title_section("Boss", locale, harness);
                assert!(
                    s.contains("fleet__set_session_title"),
                    "{locale}/{harness:?} lost the tool name"
                );
                assert!(
                    s.contains("Boss"),
                    "{locale}/{harness:?} left the user-title slot unsubstituted"
                );
                assert!(
                    !s.contains("{title}"),
                    "{locale}/{harness:?} leaked a raw {{title}} placeholder"
                );
            }
        }
    }

    /// The deadline is the whole instruction. Measured on 2026-09-07, the
    /// original "once the conversation has a stable topic" wording got 3 of 5
    /// real Fleet sessions titled: the long ones complied, and the short ones —
    /// where "stable topic" never felt reached — stayed 「（无标题）」 forever.
    /// Both locales must name a point in time by which the call has to have
    /// happened, and must not walk it back to waiting for a settled topic.
    #[test]
    fn both_locales_demand_the_call_within_the_first_turn() {
        for harness in [Harness::Claude, Harness::Codex] {
            let zh = render_session_title_section("老板", "zh", harness);
            assert!(
                zh.contains("第一个回合之前"),
                "zh/{harness:?} lost the first-turn deadline"
            );
            assert!(
                !zh.contains("一旦对话有了稳定的主题"),
                "zh/{harness:?} reverted to waiting for a stable topic"
            );

            let en = render_session_title_section("Boss", "en", harness);
            assert!(
                en.contains("before you end your first turn"),
                "en/{harness:?} lost the first-turn deadline"
            );
            assert!(
                !en.contains("Once the conversation has a stable topic"),
                "en/{harness:?} reverted to waiting for a stable topic"
            );
        }
    }

    /// Only the codex tail may teach the deferred direct-call shape — telling a
    /// Claude session to write `await tools.mcp__…` would send it hunting for a
    /// code-mode `exec` it does not have.
    #[test]
    fn only_codex_gets_the_deferred_direct_call_shape() {
        for locale in ["en", "zh"] {
            assert!(render_session_title_section("Boss", locale, Harness::Codex)
                .contains("await tools.mcp__fleet__fleet__set_session_title"));
            assert!(!render_session_title_section("Boss", locale, Harness::Claude)
                .contains("await tools."));
        }
    }

    #[test]
    fn markers_unique_vs_other_fleet_blocks() {
        assert!(BEGIN_MARKER.contains("session-title"));
        assert_ne!(BEGIN_MARKER, "<!-- fleet:interaction-mode:begin -->");
        assert_ne!(BEGIN_MARKER, "<!-- fleet:prd-discipline:begin -->");
        assert_ne!(BEGIN_MARKER, "<!-- fleet:wiki-guidance:begin -->");
    }

    #[test]
    fn guidance_file_carries_the_managed_header_and_the_claude_section() {
        let zh = render_guidance("", "zh");
        assert!(zh.starts_with("# Fleet 会话标题 (managed by Claw Fleet"));
        assert!(zh.contains("老板"), "empty title falls back to the zh default");
        assert!(zh.contains("fleet__set_session_title"));

        let en = render_guidance("Chief", "en");
        assert!(en.starts_with("# Fleet Session Title (managed by Claw Fleet"));
        assert!(en.contains("Chief"));
    }

    #[test]
    fn strip_removes_only_our_block() {
        let content = format!(
            "user rules\n\n{BEGIN_MARKER}\n@/home/x/.claude/fleet-session-title.md\n{END_MARKER}\n\
<!-- fleet:wiki-guidance:begin -->\n@/home/x/.claude/fleet-wiki-guidance.md\n\
<!-- fleet:wiki-guidance:end -->\n"
        );
        let stripped = strip_sentinel_block(&content);
        assert!(!stripped.contains("fleet-session-title.md"));
        assert!(
            stripped.contains("fleet-wiki-guidance.md"),
            "stripping ours must not eat a neighbouring block"
        );
        assert!(stripped.contains("user rules"));
    }
}
