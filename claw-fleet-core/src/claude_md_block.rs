//! The `<!-- fleet:<name>:begin -->` … `<!-- fleet:<name>:end -->` sentinel
//! block that every guidance carrier injects into `~/.claude/CLAUDE.md`.
//!
//! Five carriers (interaction mode, PRD discipline, wiki, model, session title)
//! each maintain one block, and each used to carry its own byte-identical copy
//! of the strip/compose pair. They live here once, so the blank-line accounting
//! below is decided in one place instead of five.
//!
//! **The accounting, and why it is load-bearing.** A carrier re-applies by
//! stripping its old block and appending a fresh one after a blank-line
//! separator. Removing the block used to leave that separator behind, so every
//! re-apply of whichever block was *not* last moved one blank line to the top
//! of the file and appended the block at the bottom. Nothing ever collected
//! them: the old code only collapsed a run of blank lines at the very end. On a
//! real host that had been re-applying for weeks, `CLAUDE.md` was 117 lines of
//! which 102 were blank — the five `@import`s were pushed under a 98-line hole.
//!
//! Two rules keep it flat, and both are deliberately conservative about content
//! that is not ours:
//!
//! 1. [`strip`] removes the blank line that immediately *followed* the block,
//!    because that separator was written by the same composer that wrote the
//!    block — it belongs to the block, not to the user's prose.
//! 2. [`compose`] trims leading and trailing newlines off what is left. A
//!    CLAUDE.md never legitimately starts with blank lines, so trimming the top
//!    both prevents the growth and heals a file that already grew. Blank lines
//!    *between* the user's own paragraphs are never touched.

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

/// Re-attach `block` to `existing`: strip any prior copy, then append after
/// exactly one blank line. `block` is expected to end in a newline.
pub(crate) fn compose(existing: &str, block: &str, begin: &str, end: &str) -> String {
    let stripped = strip(existing, begin, end);
    let base = stripped.trim_start_matches('\n').trim_end_matches('\n');
    if base.is_empty() {
        block.to_string()
    } else {
        format!("{base}\n\n{block}")
    }
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
        assert!(out.contains("other.md"), "must not eat a neighbouring block");
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

    /// The invariant that matters in production: re-applying forever must not
    /// change the file after the first time.
    #[test]
    fn compose_is_idempotent_wherever_the_block_sits() {
        for existing in [
            String::new(),
            "user stuff\n".to_string(),
            format!("user stuff\n\n{}", block()),
            // The shape that used to grow: our block first, another after it.
            format!("{}\n{OTHER}", block()),
        ] {
            let once = compose(&existing, &block(), BEGIN, END);
            let twice = compose(&once, &block(), BEGIN, END);
            assert_eq!(once, twice, "composing twice changed {existing:?}");
            assert!(!once.contains("\n\n\n"), "blank run in {once:?}");
            assert!(!once.starts_with('\n'), "leading blank in {once:?}");
            assert!(once.ends_with(&block()), "block must land last: {once:?}");
        }
    }

    /// A file that already grew heals on the next apply rather than staying
    /// bloated forever — the 98-line hole is not something a user can be asked
    /// to go delete by hand.
    #[test]
    fn compose_heals_an_already_bloated_file() {
        let existing = format!("{}{OTHER}", "\n".repeat(98));
        let out = compose(&existing, &block(), BEGIN, END);
        assert_eq!(out, format!("{OTHER}\n{}", block()));
    }
}
