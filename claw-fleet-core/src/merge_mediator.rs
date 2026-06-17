//! LLM-driven merge conflict mediator — V2 Phase 2 (PRD §P10).
//!
//! When `worktree::merge_back` returns a `Conflict { files, .. }`, the
//! mediator asks a Sonnet sub-session to produce a clean resolution for
//! each file. Trade-offs:
//!
//! - **Per-file calls**, not one mega-prompt. Smaller blast radius if the
//!   LLM goes off the rails; one resolution can fail without poisoning the
//!   others; cache-friendly because each file is short.
//! - **No tools.** The mediator's output is raw resolved file content,
//!   nothing more — see `MEDIATOR_PROMPT_TEMPLATE` below.
//! - **No conflict markers** allowed in the output (sanity check in P11).
//!
//! P11 takes `MediationResult` and writes the resolved content back into
//! the merge's working tree; P12 wraps retry + AskUserQuestion escalation.

use std::time::Duration;

use crate::llm_provider::{self, LlmProvider};
use crate::worktree::ConflictSpec;

/// Output of one successful mediation. `resolved_content` is the
/// merged-and-clean version of `path`, with no `<<<<<<<` / `=======` /
/// `>>>>>>>` markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediationResult {
    pub path: std::path::PathBuf,
    pub resolved_content: String,
}

/// Reasons mediation can fail for a single file. The caller (P12) decides
/// whether to retry, ask the user, or fail the P-item.
#[derive(Debug, Clone)]
pub enum MediationError {
    /// LLM provider couldn't be reached (binary missing, timeout, etc.).
    ProviderUnavailable,
    /// LLM returned no usable text.
    EmptyResponse,
    /// LLM's response still contained conflict markers — refused.
    MarkersLeftBehind {
        path: std::path::PathBuf,
        first_marker: String,
    },
    /// LLM's response didn't contain the expected wrapper tags.
    MissingWrapper { path: std::path::PathBuf },
}

impl std::fmt::Display for MediationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProviderUnavailable => write!(f, "mediator LLM provider unavailable"),
            Self::EmptyResponse => write!(f, "mediator returned empty response"),
            Self::MarkersLeftBehind { path, first_marker } => {
                write!(
                    f,
                    "mediator left conflict marker {first_marker:?} in {}",
                    path.display()
                )
            }
            Self::MissingWrapper { path } => {
                write!(f, "mediator response for {} missing <resolved> wrapper", path.display())
            }
        }
    }
}

/// Default model for the mediator. Sonnet — same tier as workers; conflict
/// resolution is a focused task where Sonnet's quality is sufficient and
/// the per-call cost stays low.
pub const MEDIATOR_MODEL: &str = "claude-sonnet-4-6";

/// Wall-clock budget per file. Conflict resolution should be quick; if a
/// single file blows past this, escalate.
pub const MEDIATOR_TIMEOUT: Duration = Duration::from_secs(120);

// Prompt template + pure response parsing moved to `claw-fleet-task`'s
// `merge_prompt` module so `fleet-task`'s LocalHost (which spawns `claude`
// directly, with no dependency on this crate) shares one copy. Re-exported here
// to keep `merge_mediator::{render_prompt, extract_resolved, ...}` callers and
// tests working unchanged.
pub use claw_fleet_task::merge_prompt::{
    extract_resolved, first_conflict_marker, render_prompt, MEDIATOR_PROMPT_TEMPLATE,
};

/// Mediate one conflict via the supplied provider. Caller owns the
/// provider so tests can swap in fakes.
///
/// One LLM call per conflicted file using the fixed template:
/// render prompt → call provider → require the `<resolved>` wrapper →
/// reject any leftover conflict markers. No retry, no tools.
pub fn mediate_one(
    provider: &dyn LlmProvider,
    spec: &ConflictSpec,
) -> Result<MediationResult, MediationError> {
    let prompt = render_prompt(spec);
    let completion = provider
        .complete(&prompt, MEDIATOR_MODEL, MEDIATOR_TIMEOUT)
        .ok_or(MediationError::ProviderUnavailable)?;
    if completion.text.trim().is_empty() {
        return Err(MediationError::EmptyResponse);
    }
    let Some(resolved) = extract_resolved(&completion.text) else {
        return Err(MediationError::MissingWrapper {
            path: spec.path.clone(),
        });
    };
    if let Some(marker) = first_conflict_marker(&resolved) {
        return Err(MediationError::MarkersLeftBehind {
            path: spec.path.clone(),
            first_marker: marker,
        });
    }
    Ok(MediationResult {
        path: spec.path.clone(),
        resolved_content: resolved,
    })
}

/// Mediate every conflict using `provider`. Pure dependency-injection
/// variant; useful for tests that need to substitute a fake provider.
pub fn mediate_with(
    provider: &dyn LlmProvider,
    conflicts: &[ConflictSpec],
) -> Result<Vec<MediationResult>, MediationError> {
    let mut out = Vec::with_capacity(conflicts.len());
    for spec in conflicts {
        out.push(mediate_one(provider, spec)?);
    }
    Ok(out)
}

/// Mediate every conflict. Stops at the first error so the caller (P12)
/// can decide on a per-file retry policy rather than burning budget on
/// later files that may depend on the earlier resolution being clean.
pub fn mediate(
    conflicts: &[ConflictSpec],
) -> Result<Vec<MediationResult>, MediationError> {
    let provider =
        llm_provider::resolve_provider("claude").ok_or(MediationError::ProviderUnavailable)?;
    mediate_with(provider.as_ref(), conflicts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_provider::{Completion, CompletionUsage, LlmModel};
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Fake LlmProvider that returns a canned response. Records the prompt
    /// it was called with so tests can inspect.
    struct FakeProvider {
        response: Mutex<Option<String>>,
        seen_prompts: Mutex<Vec<String>>,
    }

    impl FakeProvider {
        fn new(response: Option<String>) -> Self {
            Self {
                response: Mutex::new(response),
                seen_prompts: Mutex::new(vec![]),
            }
        }
    }

    impl LlmProvider for FakeProvider {
        fn name(&self) -> &str { "fake" }
        fn display_name(&self) -> &str { "Fake" }
        fn is_available(&self) -> bool { true }
        fn list_models(&self) -> Vec<LlmModel> { vec![] }
        fn default_fast_model(&self) -> &str { "fast" }
        fn default_standard_model(&self) -> &str { "std" }
        fn complete(&self, prompt: &str, _model: &str, _timeout: Duration) -> Option<Completion> {
            self.seen_prompts.lock().unwrap().push(prompt.to_string());
            self.response.lock().unwrap().clone().map(|text| Completion {
                text,
                usage: Some(CompletionUsage::default()),
            })
        }
    }

    fn spec(path: &str, base: &str, ours: &str, theirs: &str) -> ConflictSpec {
        ConflictSpec {
            path: PathBuf::from(path),
            base: base.into(),
            ours: ours.into(),
            theirs: theirs.into(),
        }
    }

    #[test]
    fn render_prompt_substitutes_all_placeholders() {
        let s = spec("src/lib.rs", "fn old() {}\n", "fn ours() {}\n", "fn theirs() {}\n");
        let p = render_prompt(&s);
        assert!(p.contains("Path: src/lib.rs"));
        assert!(p.contains("fn old() {}"));
        assert!(p.contains("fn ours() {}"));
        assert!(p.contains("fn theirs() {}"));
        // No raw placeholders left.
        assert!(!p.contains("{PATH}"));
        assert!(!p.contains("{BASE}"));
        assert!(!p.contains("{OURS}"));
        assert!(!p.contains("{THEIRS}"));
    }

    #[test]
    fn extract_resolved_pulls_inner_content() {
        let r = "noise before\n<resolved>\nclean line\n</resolved>\nnoise after";
        assert_eq!(extract_resolved(r).as_deref(), Some("clean line"));
    }

    #[test]
    fn extract_resolved_returns_none_when_no_wrapper() {
        assert!(extract_resolved("no tags here").is_none());
        assert!(extract_resolved("<resolved>missing close").is_none());
    }

    #[test]
    fn extract_resolved_preserves_internal_newlines() {
        let r = "<resolved>\nline 1\nline 2\nline 3\n</resolved>";
        assert_eq!(
            extract_resolved(r).as_deref(),
            Some("line 1\nline 2\nline 3")
        );
    }

    #[test]
    fn first_conflict_marker_detects_each_kind() {
        assert!(first_conflict_marker("ok\nok\n").is_none());
        assert!(first_conflict_marker("<<<<<<< HEAD\nfoo\n").is_some());
        assert!(first_conflict_marker("foo\n=======\nbar\n").is_some());
        assert!(first_conflict_marker("foo\n>>>>>>> branch\n").is_some());
    }

    #[test]
    fn mediate_one_happy_path() {
        let prov = FakeProvider::new(Some(
            "Sure thing!\n<resolved>\nresolved content\n</resolved>".into(),
        ));
        let s = spec("a.txt", "base", "ours", "theirs");
        let result = mediate_one(&prov, &s).unwrap();
        assert_eq!(result.path, PathBuf::from("a.txt"));
        assert_eq!(result.resolved_content, "resolved content");
        // Provider got the rendered prompt.
        assert_eq!(prov.seen_prompts.lock().unwrap().len(), 1);
        assert!(prov.seen_prompts.lock().unwrap()[0].contains("Path: a.txt"));
    }

    #[test]
    fn mediate_one_rejects_response_with_conflict_markers() {
        let prov = FakeProvider::new(Some(
            "<resolved>\nfoo\n<<<<<<< still here\n</resolved>".into(),
        ));
        let s = spec("a.txt", "b", "o", "t");
        let err = mediate_one(&prov, &s).unwrap_err();
        assert!(matches!(err, MediationError::MarkersLeftBehind { .. }));
    }

    #[test]
    fn mediate_one_rejects_response_missing_wrapper() {
        let prov = FakeProvider::new(Some("here is the file\nwithout tags".into()));
        let s = spec("a.txt", "b", "o", "t");
        let err = mediate_one(&prov, &s).unwrap_err();
        assert!(matches!(err, MediationError::MissingWrapper { .. }));
    }

    #[test]
    fn mediate_one_propagates_provider_unavailable() {
        let prov = FakeProvider::new(None);
        let s = spec("a.txt", "b", "o", "t");
        let err = mediate_one(&prov, &s).unwrap_err();
        assert!(matches!(err, MediationError::ProviderUnavailable));
    }

    #[test]
    fn mediate_one_rejects_empty_response() {
        let prov = FakeProvider::new(Some("   \n  ".into()));
        let s = spec("a.txt", "b", "o", "t");
        let err = mediate_one(&prov, &s).unwrap_err();
        assert!(matches!(err, MediationError::EmptyResponse));
    }

    // The template must encode the four heuristics, with an
    // explicit THEIRS-preference instruction and the marker/wrapper rules.
    #[test]
    fn template_encodes_four_heuristics_including_theirs_preference() {
        let t = MEDIATOR_PROMPT_TEMPLATE;
        // Four numbered instructions present.
        for n in ["1.", "2.", "3.", "4."] {
            assert!(t.contains(n), "template missing instruction {n}");
        }
        // (2) favor THEIRS on overlap.
        assert!(
            t.contains("prefer the intent of THEIRS"),
            "template must instruct favoring THEIRS"
        );
        // (3) no conflict markers.
        assert!(
            t.contains("NO conflict markers"),
            "template must forbid conflict markers"
        );
        // (4) wrap in <resolved>.
        assert!(
            t.contains("<resolved>") && t.contains("</resolved>"),
            "template must require the <resolved> wrapper"
        );
        // (1) preserve non-conflicting changes from both sides.
        assert!(
            t.contains("Preserves every non-conflicting change"),
            "template must instruct preserving non-conflicting changes"
        );
    }

    /// A fake provider that simulates an LLM honoring the THEIRS-preference
    /// instruction: it parses the rendered prompt's `<theirs>...</theirs>`
    /// block and returns it verbatim inside a `<resolved>` wrapper. This
    /// lets us verify the end-to-end contract deterministically — the
    /// resolved content equals the THEIRS branch, with no real network call.
    struct TheirsHonoringProvider;
    impl LlmProvider for TheirsHonoringProvider {
        fn name(&self) -> &str { "theirs" }
        fn display_name(&self) -> &str { "Theirs" }
        fn is_available(&self) -> bool { true }
        fn list_models(&self) -> Vec<LlmModel> { vec![] }
        fn default_fast_model(&self) -> &str { "fast" }
        fn default_standard_model(&self) -> &str { "std" }
        fn complete(&self, prompt: &str, _m: &str, _t: Duration) -> Option<Completion> {
            // Pull the theirs block out of the rendered prompt.
            let start = prompt.find("<theirs>\n")? + "<theirs>\n".len();
            let rest = &prompt[start..];
            let end = rest.find("\n</theirs>")?;
            let theirs = &rest[..end];
            Some(Completion {
                text: format!("<resolved>\n{theirs}\n</resolved>"),
                usage: Some(CompletionUsage::default()),
            })
        }
    }

    // On a 3-way conflict, an LLM that follows the THEIRS-preference
    // instruction resolves the file to the THEIRS (incoming worker) content.
    #[test]
    fn theirs_preference_resolves_to_theirs_branch_content() {
        let s = spec(
            "src/lib.rs",
            "fn base() {}\n",
            "fn ours_overlap() {}\n",
            "fn theirs_overlap() {}\n",
        );
        let prov = TheirsHonoringProvider;
        let result = mediate_one(&prov, &s).unwrap();
        // Resolved content came from the THEIRS branch, not OURS. (The
        // THEIRS branch content is `fn theirs_overlap() {}\n`; one boundary
        // newline is trimmed by extract_resolved, leaving the trailing one.)
        assert_eq!(result.resolved_content, "fn theirs_overlap() {}\n");
        assert!(result.resolved_content.contains("fn theirs_overlap"));
        assert!(!result.resolved_content.contains("ours_overlap"));
        assert!(!result.resolved_content.contains("fn base"));
    }
}
