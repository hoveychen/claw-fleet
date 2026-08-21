//! The composer's trailing attachment block.
//!
//! Both composers (desktop `userAttachments.ts`, mobile `Composer.tsx`) append
//! the files a user attached to a prompt as one trailing block:
//!
//! ```text
//! <prose>\n\nContext files:\n- /abs/one.png\n- /abs/two.pdf
//! ```
//!
//! That block is the only handle the transcript has on those files, so every
//! consumer that wants either half — the authored prose, or the paths — has to
//! agree on exactly the same shape. This module is that one agreement: codex
//! uses it to keep a wall of paths out of a session title, dsh uses it to lift
//! the image paths into real image content blocks.

/// Marker introducing the block. Emitted verbatim by both composers.
const MARKER: &str = "\n\nContext files:\n";

/// Split `text` into its prose and the attachment paths that followed it.
///
/// Returns `(text, [])` unchanged when there is no block. Anchored to the exact
/// shape the composers emit: the marker, then one-or-more `- <non-empty>` lines
/// running to end of string. A prompt that merely mentions "Context files:"
/// mid-sentence, or whose block is followed by more prose, has no block — the
/// mirror of the frontend's `CONTEXT_FILES_RE`.
pub fn split(text: &str) -> (&str, Vec<&str>) {
    let Some(idx) = text.rfind(MARKER) else {
        return (text, Vec::new());
    };
    let tail = &text[idx + MARKER.len()..];
    // Tolerate a single trailing newline (JS `$` matches before a final `\n`).
    let tail = tail.strip_suffix('\n').unwrap_or(tail);
    if tail.is_empty() {
        return (text, Vec::new());
    }
    let mut paths = Vec::new();
    for line in tail.split('\n') {
        // Every line in the block must be `- ` followed by at least one
        // character; one that isn't means this is prose, not our block.
        if !line.starts_with("- ") || line.len() <= 2 {
            return (text, Vec::new());
        }
        paths.push(&line[2..]);
    }
    (&text[..idx], paths)
}

/// Render the block back, for callers that consumed only some of the paths.
///
/// Returns `prose` unchanged when `paths` is empty, so a prompt whose every
/// attachment was lifted elsewhere carries no empty header.
pub fn render(prose: &str, paths: &[&str]) -> String {
    if paths.is_empty() {
        return prose.to_string();
    }
    let listed = paths
        .iter()
        .map(|p| format!("- {p}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{prose}{MARKER}{listed}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_the_block_the_composers_emit() {
        let (prose, paths) = split("看下这个\n\nContext files:\n- /a/one.png\n- /b/two.pdf");
        assert_eq!(prose, "看下这个");
        assert_eq!(paths, vec!["/a/one.png", "/b/two.pdf"]);
    }

    #[test]
    fn tolerates_one_trailing_newline() {
        let (prose, paths) = split("look\n\nContext files:\n- /a/one.png\n");
        assert_eq!(prose, "look");
        assert_eq!(paths, vec!["/a/one.png"]);
    }

    /// A prose mention is not a block, and neither is one followed by more
    /// speech — the user kept typing after attaching.
    #[test]
    fn leaves_prose_mentions_alone() {
        let text = "讨论一下 Context files: 这个功能怎么做";
        assert_eq!(split(text), (text, Vec::new()));

        let kept_typing = "hi\n\nContext files:\n- /a/one.png\nand I kept typing";
        assert_eq!(split(kept_typing), (kept_typing, Vec::new()));
    }

    #[test]
    fn a_prompt_with_no_block_is_returned_whole() {
        assert_eq!(split("just prose"), ("just prose", Vec::new()));
    }

    /// Round-trip: what `split` took apart, `render` puts back byte-for-byte.
    #[test]
    fn render_round_trips_a_split_block() {
        let text = "看下这个\n\nContext files:\n- /a/one.png\n- /b/two.pdf";
        let (prose, paths) = split(text);
        assert_eq!(render(prose, &paths), text);
    }

    #[test]
    fn render_without_paths_emits_no_header() {
        assert_eq!(render("just prose", &[]), "just prose");
    }
}
