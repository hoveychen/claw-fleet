//! Inline `[?…]` explain marks — the one place that words "mark what you
//! under-explained".
//!
//! The agent wraps a phrase or sentence it glossed over (a trade-off it made
//! without unpacking, a term the user may not know, a conclusion given without
//! its derivation) in `[?` … `]`. Fleet renders the mark as a clickable
//! annotation; one click asks the v1 side-question machinery
//! (`session_explain`) to explain exactly that text, so the user never has to
//! type the question. In plain markdown the mark degrades to the original text
//! plus two characters, so other readers are not harmed.
//!
//! The wording lives here and every carrier embeds it:
//!
//! - Claude engineering sessions: inside the interaction-mode file
//!   ([`crate::interaction_mode::render_guidance`]) — 老板's call, so it shares
//!   that feature's on/off switch rather than getting its own.
//! - Claude chat sessions: `~/.fleet/chat/CLAUDE.md`. The chat workspace drops
//!   the user's global `~/.claude/*.md` (`--setting-sources project`), so the
//!   section is appended to the brief by [`crate::chat_workspace`], read back
//!   from the interaction-mode file on disk via [`installed_section`].
//! - codex / dsh: embedded in their interaction blocks
//!   ([`crate::codex_guidance`], [`crate::dsh_guidance`]). Those blocks are
//!   English-only, so they take the `en` variant. dsh chat sessions read the
//!   chat brief instead (their preset drops the global AGENTS.md).
//!
//! The imperative heading ("you MUST do this in every prose reply") is
//! load-bearing for codex: with the neutral heading, two real `AGENTS.md`
//! spawns (gpt-5.6-luna low, gpt-5.6-sol medium) produced zero marks while
//! obeying every other rule in the same file; strengthening only the heading,
//! at the same position, produced 2–3 compliant marks. Claude and dsh followed
//! the neutral wording when it sat in the prompt, so the stronger heading is
//! applied uniformly rather than special-cased.
//!
//! Probe data behind the rules (2026-09-21, `design/explain-annotations.md`):
//! a `[?…]` not followed by an ASCII `(` is plain text to remark-gfm; a mark
//! containing inline code or emphasis is split into several mdast nodes, which
//! is why nesting is forbidden; five marks on a long reply were all judged
//! well-placed, which is where the cap comes from.

use std::fs;

/// Sentinels around the section inside the interaction-mode file. They let
/// [`installed_section`] lift the exact rendered text without re-deriving the
/// user title and locale the file was written with.
pub const BEGIN_MARKER: &str = "<!-- fleet:explain-marks:begin -->";
pub const END_MARKER: &str = "<!-- fleet:explain-marks:end -->";

/// Render the `## …` section, heading and sentinels included.
///
/// `user_title` is what agents call the user (already defaulted by the
/// caller); `locale` picks the Chinese variant for `"zh"` and English for
/// everything else. Kept under ~900 bytes in English because the codex
/// `AGENTS.md` it is embedded in has a 32 KiB ceiling with little headroom.
pub fn render_explain_marks_section(user_title: &str, locale: &str) -> String {
    let body = if locale == "zh" {
        format!(
            "## 正文标注 `[?…]` —— 每条正文回复都必须做\n\
\n\
写给{title}看的正文里（决策卡的 question、对话回复），凡是**你做了取舍却没展开、\
用了{title}未必熟的术语、给了结论没给推导**的地方，用 `[?` 和 `]` 把那个短语或那句话\
包起来，例如 `[?AUROC 只动了 0.004]`。Fleet 会把它渲染成可点击的标注，{title}一点\
就能就这段文字向你追问，不用自己打字；在纯文本里它只是多两个字符，无害。\n\
\n\
- 一条回复最多 5 处，宁缺毋滥；短回复可以一处都没有。\n\
- 只包一个短语或一句话，不要包整段；不要在代码、表格、标题、链接文本里标。\n\
- 标注里不要嵌套反引号或加粗；`]` 后面不要紧跟半角 `(`，否则会被当成链接。\n\
- 它不是强调符号：标的是「{title}读到这里大概会想问」的位置，不是重点。",
            title = user_title,
        )
    } else {
        format!(
            "## Inline marks `[?…]` — you MUST do this in every prose reply\n\
\n\
In prose written for {title} (a decision card's question, a conversational \
reply), wrap the phrase or sentence where **you made a trade-off without \
unpacking it, used a term {title} may not know, or stated a conclusion without \
its derivation** in `[?` and `]`, e.g. `[?AUROC moved only 0.004]`. Fleet renders \
the mark as a clickable annotation: one click lets {title} ask you about exactly \
that text without typing a question. In plain text it is just two extra \
characters, harmless.\n\
\n\
- At most 5 per reply; fewer is fine and a short reply may have none.\n\
- Wrap one phrase or one sentence, never a paragraph; never inside code, tables, \
headings or link text.\n\
- No backticks or emphasis inside a mark; no ASCII `(` right after the `]`, or \
markdown reads it as a link.\n\
- It is not emphasis: it marks where {title} would probably pause to ask, not \
what matters most.",
            title = user_title,
        )
    };
    format!("{BEGIN_MARKER}\n{body}\n{END_MARKER}")
}

/// The codex variant: same heading and rules, fewer words. `~/.codex/AGENTS.md`
/// carries five Fleet blocks plus a 6 KiB lessons budget under a 32 KiB
/// ceiling; with the full English section the file measured 32,504 bytes on
/// 2026-09-21 (264 bytes of headroom, lessons at 3.7 KiB of their 6 KiB), so
/// the codex block takes this ~half-size rendering instead.
pub fn render_explain_marks_section_compact(user_title: &str) -> String {
    let body = format!(
        "## Inline marks `[?…]` — you MUST do this in every prose reply\n\
\n\
In prose for {title}, wrap a phrase where you made a trade-off without unpacking \
it, used a term {title} may not know, or gave a conclusion without its derivation \
in `[?` … `]`, e.g. `[?AUROC moved only 0.004]`. Fleet renders it clickable so \
{title} can ask about that text without typing. At most 5 per reply; one phrase \
or sentence each; never in code, tables, headings or links; no backticks or \
emphasis inside; no ASCII `(` right after `]`. It marks where {title} would ask, \
not what matters most.",
        title = user_title,
    );
    format!("{BEGIN_MARKER}\n{body}\n{END_MARKER}")
}

/// The section as currently written into the interaction-mode file, or `None`
/// when that feature is off (file absent) or predates the section.
///
/// Read back from disk rather than re-rendered so a carrier that does not know
/// the user title / locale (the chat brief) emits exactly what the engineering
/// sessions got, and so a switched-off interaction mode reads as "no marks".
pub fn installed_section() -> Option<String> {
    let path = crate::session::get_claude_dir()?.join("fleet-interaction-mode.md");
    let content = fs::read_to_string(path).ok()?;
    extract_section(&content)
}

/// Lift the sentinel-delimited section (sentinels included) out of `content`.
pub fn extract_section(content: &str) -> Option<String> {
    let start = content.find(BEGIN_MARKER)?;
    let end_rel = content[start..].find(END_MARKER)?;
    Some(content[start..start + end_rel + END_MARKER.len()].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zh_section_carries_the_rules_and_the_title() {
        let s = render_explain_marks_section("老板", "zh");
        assert!(s.starts_with(BEGIN_MARKER));
        assert!(s.ends_with(END_MARKER));
        assert!(s.contains("## 正文标注 `[?…]`"));
        assert!(s.contains("最多 5 处"));
        assert!(s.contains("不要嵌套反引号或加粗"));
        assert!(s.contains("不要紧跟半角 `(`"));
        assert!(s.contains("老板一点"), "user title is interpolated");
        assert!(!s.contains("{title}"), "no unexpanded placeholder");
    }

    #[test]
    fn en_section_carries_the_rules_and_stays_small() {
        let s = render_explain_marks_section("Boss", "en");
        assert!(s.contains("## Inline marks `[?…]`"));
        assert!(s.contains("At most 5 per reply"));
        assert!(s.contains("No backticks or emphasis inside a mark"));
        assert!(s.contains("no ASCII `(` right after the `]`"));
        assert!(s.contains("lets Boss ask"));
        // codex AGENTS.md has a 32 KiB ceiling and was at 31.5 KiB before this
        // section existed; keep the embedded English variant well under 1 KiB.
        assert!(s.len() < 1024, "en section is {} bytes", s.len());
    }

    #[test]
    fn compact_variant_keeps_every_rule_in_half_the_bytes() {
        let full = render_explain_marks_section("Boss", "en");
        let s = render_explain_marks_section_compact("Boss");
        assert!(
            s.contains("## Inline marks `[?…]`"),
            "same heading as the full variant"
        );
        assert!(s.contains("At most 5 per reply"));
        assert!(s.contains("no backticks or emphasis inside"));
        assert!(s.contains("no ASCII `(` right after `]`"));
        assert!(s.contains("never in code, tables, headings or links"));
        // Measured 585 vs 945 bytes on 2026-09-21; the point is the ~360 bytes
        // of AGENTS.md headroom they buy, so guard the ratio, not the exact size.
        assert!(s.len() < 640, "compact section is {} bytes", s.len());
        assert!(
            s.len() * 3 < full.len() * 2,
            "compact ({}) vs full ({})",
            s.len(),
            full.len()
        );
    }

    #[test]
    fn extract_lifts_exactly_the_sentinel_block() {
        let section = render_explain_marks_section("老板", "zh");
        let file = format!("# header\n\nprose before\n\n{section}\n\n## Next section\nmore");
        assert_eq!(extract_section(&file).as_deref(), Some(section.as_str()));
        assert_eq!(extract_section("no sentinels here"), None);
        assert_eq!(
            extract_section(BEGIN_MARKER),
            None,
            "unterminated block is not a section"
        );
    }
}
