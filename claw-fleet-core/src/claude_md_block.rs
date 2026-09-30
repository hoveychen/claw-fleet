//! The `<!-- fleet:<name>:begin -->` … `<!-- fleet:<name>:end -->` sentinel
//! block that older Fleet builds injected into `~/.claude/CLAUDE.md`, one per
//! guidance carrier (interaction mode, PRD discipline, wiki, model, session
//! title). Only [`strip`] is left: the scope migration uses it to take the
//! blocks back out. The writer side (`compose`) went with the global writes.
//!
//! **Why [`strip`] also takes the blank line after the block.** The composer
//! that wrote a block appended it after a blank-line separator, so that
//! separator belongs to the block, not to the user's prose. Leaving it behind
//! was how one real host's `CLAUDE.md` grew to 117 lines of which 102 were
//! blank. Blank lines *between* the user's own paragraphs are never touched.

/// Remove the block delimited by `begin`/`end`, along with the blank separator
/// line that followed it. Content outside the block — including another
/// carrier's block — is preserved verbatim.
pub(crate) fn strip(content: &str, begin: &str, end: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_block = false;
    // Set when the previous kept-or-dropped line was our `end` marker, so the
    // separator that belonged to the block goes out with it.
    let mut just_closed = false;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == begin {
            in_block = true;
            just_closed = false;
            continue;
        }
        if trimmed == end {
            in_block = false;
            just_closed = true;
            continue;
        }
        if in_block {
            continue;
        }
        if just_closed {
            just_closed = false;
            if trimmed.is_empty() {
                continue;
            }
        }
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEGIN: &str = "<!-- fleet:demo:begin -->";
    const END: &str = "<!-- fleet:demo:end -->";
    const OTHER: &str =
        "<!-- fleet:other:begin -->\n@/home/x/.claude/other.md\n<!-- fleet:other:end -->\n";

    fn block() -> String {
        format!("{BEGIN}\n@/home/x/.claude/demo.md\n{END}\n")
    }

    #[test]
    fn strip_leaves_content_and_other_blocks_alone() {
        let content = format!("user rules\n\n{}{OTHER}", block());
        let out = strip(&content, BEGIN, END);
        assert!(!out.contains("demo.md"));
        assert!(
            out.contains("other.md"),
            "must not eat a neighbouring block"
        );
        assert!(out.starts_with("user rules\n\n"));
    }

    #[test]
    fn strip_is_a_noop_without_the_block() {
        let content = "just some rules\n";
        assert_eq!(strip(content, BEGIN, END), content);
        assert_eq!(strip(OTHER, BEGIN, END), OTHER);
    }

    /// The separator belongs to the block: it was written by the composer, not
    /// by the user, so it must leave with the block. This is the whole bug —
    /// leaving it behind is what put 98 blank lines at the top of a real
    /// CLAUDE.md.
    #[test]
    fn strip_takes_the_separator_that_followed_the_block() {
        let content = format!("{}\n{OTHER}", block());
        assert_eq!(strip(&content, BEGIN, END), OTHER);
    }

    /// Only the *one* separator, though — a bigger gap is the user's.
    #[test]
    fn strip_takes_only_one_blank_line() {
        let content = format!("{}\n\n\nuser prose\n", block());
        assert_eq!(strip(&content, BEGIN, END), "\n\nuser prose\n");
    }
}
