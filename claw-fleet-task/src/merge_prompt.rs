//! Pure prompt rendering + response parsing for LLM merge-conflict mediation.
//!
//! Lives in `claw-fleet-task` (not `claw-fleet-core::merge_mediator`) so BOTH
//! consumers share one copy: core's `merge_mediator` (which owns the actual LLM
//! call via `llm_provider`) and `fleet-task`'s `LocalHost` (which spawns
//! `claude` directly for the P-item merge path). No LLM here — just the fixed
//! template and the `<resolved>...</resolved>` extraction contract.

use crate::worktree::ConflictSpec;

/// Prompt template. Filled with the file path + 3-way contents, wrapped so the
/// LLM's output is easy to extract. The wrapper tags are intentional — relying
/// on the LLM to emit "just the file" is fragile; the tags let us strip preamble
/// noise deterministically. THEIRS bias is the load-bearing rule for
/// repeatability — workers own the P-item's changes, so their version is
/// authoritative on genuine overlap.
pub const MEDIATOR_PROMPT_TEMPLATE: &str = "You are resolving a 3-way merge conflict for a single file. \
You will be given the common ancestor (BASE), the current branch's version (OURS), and \
the incoming branch's version (THEIRS). Produce a single resolved version that:

1. Preserves every non-conflicting change from both sides.
2. For changes that overlap, prefer the intent of THEIRS unless OURS clearly supersedes it.
3. Must contain NO conflict markers (no `<<<<<<<`, `=======`, `>>>>>>>`).
4. Must be the complete, ready-to-write file content — not a diff, not commentary.

Wrap the resolved file content in exactly these tags so it can be extracted:

<resolved>
<file content here>
</resolved>

Path: {PATH}

<base>
{BASE}
</base>

<ours>
{OURS}
</ours>

<theirs>
{THEIRS}
</theirs>

Now emit the resolved file inside <resolved>...</resolved>. Nothing else.";

/// Render the prompt for one conflict. Pure; testable without an LLM.
pub fn render_prompt(spec: &ConflictSpec) -> String {
    MEDIATOR_PROMPT_TEMPLATE
        .replace("{PATH}", &spec.path.display().to_string())
        .replace("{BASE}", &spec.base)
        .replace("{OURS}", &spec.ours)
        .replace("{THEIRS}", &spec.theirs)
}

/// Extract the resolved content from the LLM's response — the inner text between
/// `<resolved>` and `</resolved>`, with a single boundary newline trimmed each
/// side. `None` when the wrapper is absent (caller treats that as an error
/// rather than guessing at unwrapped output).
pub fn extract_resolved(response: &str) -> Option<String> {
    let start = response.find("<resolved>")?;
    let after_open = &response[start + "<resolved>".len()..];
    let end = after_open.find("</resolved>")?;
    let inner = &after_open[..end];
    let trimmed = inner.strip_prefix('\n').unwrap_or(inner);
    let trimmed = trimmed.strip_suffix('\n').unwrap_or(trimmed);
    Some(trimmed.to_string())
}

/// First leftover conflict marker (`<<<<<<<` / `=======` / `>>>>>>>`) in
/// `content`, if any — used to reject half-resolved output.
pub fn first_conflict_marker(content: &str) -> Option<String> {
    for marker in ["<<<<<<<", "=======", ">>>>>>>"] {
        for line in content.lines() {
            if line.starts_with(marker) {
                return Some(line.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn spec() -> ConflictSpec {
        ConflictSpec {
            path: PathBuf::from("src/lib.rs"),
            base: "fn old() {}\n".into(),
            ours: "fn ours() {}\n".into(),
            theirs: "fn theirs() {}\n".into(),
        }
    }

    #[test]
    fn render_prompt_substitutes_all_placeholders() {
        let p = render_prompt(&spec());
        assert!(p.contains("src/lib.rs"));
        assert!(p.contains("fn old() {}"));
        assert!(p.contains("fn ours() {}"));
        assert!(p.contains("fn theirs() {}"));
        assert!(!p.contains("{PATH}") && !p.contains("{BASE}"));
    }

    #[test]
    fn extract_resolved_pulls_inner_content() {
        let resp = "preamble\n<resolved>\nfn merged() {}\n</resolved>\ntrailing";
        assert_eq!(extract_resolved(resp).as_deref(), Some("fn merged() {}"));
    }

    #[test]
    fn extract_resolved_none_without_wrapper() {
        assert!(extract_resolved("no tags here").is_none());
    }

    #[test]
    fn first_conflict_marker_detects_leftovers() {
        assert!(first_conflict_marker("ok\n<<<<<<< HEAD\nx").is_some());
        assert!(first_conflict_marker("clean file\n").is_none());
    }
}
