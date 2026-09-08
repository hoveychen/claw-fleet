//! Per-model facts that more than one surface needs to agree on.
//!
//! Today this holds exactly one fact — the reasoning-effort ladder each model
//! accepts — because that is what [`crate::agent_source::route_launch`] needs to
//! stop dropping effort on a cross-harness switch. It is deliberately a module
//! of its own rather than a private helper in `agent_source`: the same ladders
//! are currently also hand-written in `claw-fleet-desktop/app/modelChoices.ts`
//! (`codexEffortChoices`) and paraphrased in the three `*_guidance.rs`
//! cheat-sheets, and those surfaces are meant to grow into consumers of this
//! module rather than keep their own copies.
//!
//! **Provenance.** The Codex ladders below were read out of this machine's
//! `~/.codex/models_cache.json` (`supported_reasoning_levels`, client 0.153.4,
//! fetched 2026-09-08), not from documentation. That matters because the
//! hand-written copies elsewhere in the tree disagree with it: they claim Codex
//! tops out at `high` and offers a `minimal` level, and the cache says every
//! listed model supports `xhigh`/`max` and none of them offers `minimal`.

/// Every effort level any harness accepts, weakest first.
///
/// The order is the whole point: mapping a level onto a ladder that lacks it
/// means "pick the strongest level this model does support that is no stronger
/// than what was asked for", which needs a total order across both vocabularies.
/// `minimal` is kept even though no current model advertises it — older Codex
/// builds did, and a stored session can still carry it.
const EFFORT_ORDER: [&str; 7] = ["minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

/// Claude's `--effort` ladder. Uniform across the Claude family.
const CLAUDE_LADDER: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// Codex models that add `ultra` ("maximum reasoning with automatic task
/// delegation") on top of the common ladder.
const CODEX_ULTRA_LADDER: &[&str] = &["low", "medium", "high", "xhigh", "max", "ultra"];

/// The common Codex ladder — no `ultra`.
const CODEX_LADDER: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// `gpt-5.5` stops at `xhigh` (and defaults to it).
const CODEX_5_5_LADDER: &[&str] = &["low", "medium", "high", "xhigh"];

/// The effort levels `model` accepts, weakest first, or `None` when the model
/// is not one we have ladder facts for.
///
/// `None` is a real answer, not a failure: a dsh route names a third-party model
/// served through someone's own provider block, and inventing a ladder for it
/// would be worse than admitting we don't know. Callers treat `None` as "don't
/// carry an effort over".
pub fn effort_ladder(model: &str) -> Option<&'static [&'static str]> {
    let m = model.trim().to_ascii_lowercase();
    if m.is_empty() {
        return None;
    }
    // A dsh provider-scoped id (`openrouter/anthropic/…`) or a Codex profile
    // marker names a model whose ladder is the host's business, not ours.
    if m.contains('/') || m.starts_with("profile:") {
        return None;
    }
    // Strip Fleet's own `[1m]` context-window suffix, which is not part of any
    // model id (see `agent_source::source_for_model`).
    let base = m.split('[').next().unwrap_or(&m).trim();
    match crate::agent_source::source_for_model(base)? {
        "claude-code" => Some(CLAUDE_LADDER),
        "codex" => Some(codex_ladder(base)),
        _ => None,
    }
}

/// Ladder for a Codex slug. Unknown `gpt-*` ids get the common ladder rather
/// than `None`: they are Codex models by construction (that is how
/// `source_for_model` classified them), and the common ladder is the subset
/// every listed Codex model in the cache supports, so mapping onto it can only
/// under-shoot, never hand Codex a level it rejects.
fn codex_ladder(model: &str) -> &'static [&'static str] {
    if model.starts_with("gpt-5.5") {
        return CODEX_5_5_LADDER;
    }
    if model.starts_with("gpt-6-astra")
        || model.starts_with("gpt-5.6-sol")
        || model.starts_with("gpt-5.6-terra")
    {
        return CODEX_ULTRA_LADDER;
    }
    CODEX_LADDER
}

/// Rank of an effort level in [`EFFORT_ORDER`], or `None` for a level we don't
/// recognise.
fn rank(effort: &str) -> Option<usize> {
    let e = effort.trim().to_ascii_lowercase();
    EFFORT_ORDER.iter().position(|l| *l == e)
}

/// Carry `effort` over to `target_model`, clamping it onto that model's ladder.
///
/// Returns `None` when the effort should be dropped — an unrecognised level, or
/// a target whose ladder we have no facts about. Otherwise returns the
/// strongest level the target supports that is no stronger than `effort`, which
/// is `effort` itself whenever the target's ladder contains it.
///
/// Clamping *down* rather than up is deliberate: effort costs money and time, so
/// the failure mode of guessing high is a surprise bill, while guessing low is a
/// weaker turn the user can retry. The one case with no level at or below the
/// request (asking for `minimal` on a ladder that starts at `low`) falls back to
/// the target's weakest level, since dropping the effort entirely would land on
/// the harness default, which is *stronger* than what was asked for.
pub fn map_effort(effort: &str, target_model: &str) -> Option<&'static str> {
    let want = rank(effort)?;
    let ladder = effort_ladder(target_model)?;
    ladder
        .iter()
        .rev()
        .find(|l| rank(l).is_some_and(|r| r <= want))
        .or_else(|| ladder.first())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bug this module exists to fix: a Claude session at `xhigh` handing
    /// off to Codex used to lose the level entirely and land on Codex's
    /// `medium` default, on the stated grounds that "Codex has no xhigh".
    /// Every listed Codex model in the cache does.
    #[test]
    fn xhigh_survives_the_hop_to_codex() {
        for model in ["gpt-6-astra", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            assert_eq!(map_effort("xhigh", model), Some("xhigh"), "{model}");
            assert_eq!(map_effort("max", model), Some("max"), "{model}");
        }
    }

    /// `gpt-5.5` is the one listed model whose ladder really does stop short, so
    /// `max` clamps to its top rather than being carried over or dropped.
    #[test]
    fn max_clamps_down_on_a_shorter_ladder() {
        assert_eq!(map_effort("max", "gpt-5.5"), Some("xhigh"));
        assert_eq!(map_effort("xhigh", "gpt-5.5"), Some("xhigh"));
        assert_eq!(map_effort("high", "gpt-5.5"), Some("high"));
    }

    /// The reverse hop. Codex's `ultra` has no Claude counterpart and clamps to
    /// Claude's top level.
    #[test]
    fn ultra_clamps_to_claude_max() {
        assert_eq!(map_effort("ultra", "claude-opus-5"), Some("max"));
        assert_eq!(map_effort("ultra", "claude-opus-5[1m]"), Some("max"));
        assert_eq!(map_effort("medium", "claude-sonnet-5"), Some("medium"));
    }

    /// A level below the target's floor maps to that floor, not to `None`:
    /// dropping it would silently *raise* the effort to the harness default.
    #[test]
    fn minimal_maps_to_the_ladder_floor() {
        assert_eq!(map_effort("minimal", "gpt-5.6-sol"), Some("low"));
        assert_eq!(map_effort("minimal", "claude-opus-5"), Some("low"));
    }

    /// dsh provider-scoped ids and Codex profile markers name models whose
    /// ladder belongs to the host's config, so we decline rather than invent.
    #[test]
    fn unknown_targets_yield_no_ladder() {
        assert_eq!(effort_ladder("openrouter/anthropic/claude-opus-5"), None);
        assert_eq!(effort_ladder("profile:work"), None);
        assert_eq!(effort_ladder(""), None);
        assert_eq!(map_effort("high", "openrouter/anthropic/claude-opus-5"), None);
    }

    /// An unrecognised effort string is dropped rather than guessed at.
    #[test]
    fn unknown_effort_is_dropped() {
        assert_eq!(map_effort("turbo", "gpt-5.6-sol"), None);
        assert_eq!(map_effort("", "gpt-5.6-sol"), None);
    }

    /// An unknown `gpt-*` slug still routes to Codex, so it gets the subset
    /// ladder every listed Codex model supports rather than no ladder at all.
    #[test]
    fn unknown_codex_slug_gets_the_common_ladder() {
        assert_eq!(effort_ladder("gpt-7-whatever"), Some(CODEX_LADDER));
        assert_eq!(map_effort("ultra", "gpt-7-whatever"), Some("max"));
    }
}
