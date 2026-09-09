//! The model catalog — per-model facts that more than one surface has to agree
//! on (which effort levels a model accepts, which capability tier it sits in).
//!
//! **The facts live in `models.toml`, not in this file.** `claw-fleet-core/models.toml`
//! is `include_str!`d as the built-in default so every Fleet build ships a current
//! copy; `~/.fleet/models.toml` overlays it **field by field**, so a user entry that
//! sets only `efforts` changes only the ladder and inherits the rest. An id the
//! built-in file doesn't know is simply added.
//!
//! A malformed user file is ignored and the built-in catalog stands. That is
//! deliberate: one slipped keystroke in an optional override should not strip
//! ladder facts from every session on the machine.
//!
//! Today's only consumer is [`crate::agent_source::route_launch`], which needs
//! the ladder to stop dropping effort on a cross-harness switch. The three
//! `*_guidance.rs` cheat-sheets and the two hand-written UI pickers
//! (`modelChoices.ts`, `Composer.tsx`) are meant to grow into consumers rather
//! than keep their own copies.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// The built-in catalog, compiled in so a fresh install is never empty.
const BUILTIN: &str = include_str!("../models.toml");

/// Every effort level any harness accepts, weakest first.
///
/// The order is the whole point: mapping a level onto a ladder that lacks it
/// means "the strongest level this model does support that is no stronger than
/// what was asked for", which needs a total order across all three vocabularies.
/// `off` and `minimal` sit below `low` — `off` is a real dsh level, and
/// `minimal` is one older Codex builds offered and a stored session can still
/// carry.
const EFFORT_ORDER: [&str; 8] = [
    "off", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];

/// One model's entry, as written in `models.toml`.
///
/// Every field but `id` is optional so a user override can name a single one.
/// `efforts = None` means "we don't assert a ladder for this model" — which for
/// a dsh entry means "ask dsh" — and is a different statement from an empty
/// list.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct ModelEntry {
    pub id: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub tier: Option<String>,
    #[serde(default)]
    pub efforts: Option<Vec<String>>,
    #[serde(default)]
    pub default_effort: Option<String>,
    /// Default context window in tokens. A **fallback** only — a session that
    /// reports its own effective window (every Codex turn does) wins.
    #[serde(default)]
    pub context: Option<u64>,
    /// Largest window the model can be opened with. Recorded, not yet consumed.
    #[serde(default)]
    pub max_context: Option<u64>,
}

impl ModelEntry {
    /// Overlay `other`'s set fields onto `self`, leaving the rest alone. This is
    /// what makes a user entry that sets only `efforts` a ladder override rather
    /// than a wholesale replacement that blanks out `tier`.
    fn overlay(&mut self, other: ModelEntry) {
        if other.family.is_some() {
            self.family = other.family;
        }
        if other.tier.is_some() {
            self.tier = other.tier;
        }
        if other.efforts.is_some() {
            self.efforts = other.efforts;
        }
        if other.default_effort.is_some() {
            self.default_effort = other.default_effort;
        }
        if other.context.is_some() {
            self.context = other.context;
        }
        if other.max_context.is_some() {
            self.max_context = other.max_context;
        }
    }
}

#[derive(Deserialize)]
struct CatalogFile {
    #[serde(default)]
    model: Vec<ModelEntry>,
}

/// Parse a catalog document, or `None` when it is not valid TOML.
fn parse(doc: &str) -> Option<Vec<ModelEntry>> {
    toml::from_str::<CatalogFile>(doc).ok().map(|f| f.model)
}

/// Merge a user document over a base catalog, keyed by lowercased id.
fn merge(base: Vec<ModelEntry>, overlay: Vec<ModelEntry>) -> BTreeMap<String, ModelEntry> {
    let mut out: BTreeMap<String, ModelEntry> = base
        .into_iter()
        .map(|e| (e.id.trim().to_ascii_lowercase(), e))
        .collect();
    for entry in overlay {
        let key = entry.id.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        match out.get_mut(&key) {
            Some(existing) => existing.overlay(entry),
            None => {
                out.insert(key, entry);
            }
        }
    }
    out
}

/// Path of the optional user override, or `None` when there is no home dir.
fn user_catalog_path() -> Option<std::path::PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("models.toml"))
}

/// The merged catalog, computed once per process.
///
/// Cached rather than re-read per lookup because `route_launch` sits on the
/// spawn path: re-parsing two TOML documents to answer "does Sol take xhigh"
/// would be silly. The cost is that editing `~/.fleet/models.toml` takes effect
/// in processes started afterwards, which matches how the rest of Fleet's
/// `~/.fleet/*` config behaves.
fn catalog() -> &'static BTreeMap<String, ModelEntry> {
    static CATALOG: OnceLock<BTreeMap<String, ModelEntry>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let base = parse(BUILTIN).unwrap_or_default();
        let overlay = user_catalog_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|doc| parse(&doc))
            .unwrap_or_default();
        merge(base, overlay)
    })
}

/// Strip Fleet's own `[1m]` context-window suffix, which is not part of any
/// model id (see [`crate::agent_source::source_for_model`]), and lowercase.
fn normalize_id(model: &str) -> String {
    let m = model.trim().to_ascii_lowercase();
    m.split('[').next().unwrap_or(&m).trim().to_string()
}

/// The catalog entry for `model`, if the catalog names it.
///
/// Matching is exact on the normalized id. A dated Claude alias
/// (`claude-haiku-4-5-20251001`) or an unlisted slug falls through to the
/// family defaults in [`effort_ladder`] rather than being fuzzy-matched here —
/// guessing which catalog row a near-miss id "meant" is how a picker ends up
/// offering a level the model rejects.
pub fn entry(model: &str) -> Option<&'static ModelEntry> {
    catalog().get(&normalize_id(model))
}

/// The capability tier (`fast` / `standard` / `premium`) the catalog assigns
/// `model`, or `None` when it is not catalogued.
pub fn tier(model: &str) -> Option<&'static str> {
    entry(model)?.tier.as_deref()
}

/// The catalogued default context window for `model`, in tokens.
///
/// `None` means the catalog states no window for it — which is the case for
/// every Claude row on purpose: Claude's window follows a *family rule*
/// (Opus/Sonnet 4.6+, 5.x, Fable and Mythos are natively 1M; everything else is
/// 200K) that also has to honour dated aliases and Fleet's own `[1m]` suffix.
/// That rule cannot be enumerated id-by-id, so it stays in
/// [`crate::session::stats::context_window_for_model`] and this table does not
/// try to shadow it.
pub fn context_window(model: &str) -> Option<u64> {
    entry(model)?.context
}

/// The effort levels `model` accepts, weakest first, or `None` when we have no
/// ladder facts for it.
///
/// `None` is a real answer, not a failure. A dsh route names a model served
/// through the user's own provider block, and dsh publishes that model's true
/// ladder at runtime — so the catalog deliberately leaves `efforts` unset on dsh
/// rows, and callers treat `None` as "don't carry an effort over" rather than
/// inventing one.
pub fn effort_ladder(model: &str) -> Option<&'static [String]> {
    let id = normalize_id(model);
    if id.is_empty() {
        return None;
    }
    if let Some(efforts) = catalog().get(&id).and_then(|e| e.efforts.as_deref()) {
        return Some(efforts);
    }
    // A Codex profile marker routes to Codex, but the model it selects comes
    // from the host's own `<CODEX_HOME>/<name>.config.toml` — which may well be
    // a third-party model behind a provider block. Falling through to the Codex
    // family ladder here would state a ladder for a model we cannot even name.
    if id.starts_with("profile:") {
        return None;
    }
    // Not catalogued (or catalogued without a ladder). Fall back to the family
    // ladder for the harness the id names — but only for Claude and Codex,
    // whose ladders are uniform enough to generalise. A dsh id gets `None`.
    match crate::agent_source::source_for_model(&id)? {
        "claude-code" => family_ladder("claude-fable-5-1"),
        // Unlisted `gpt-*` slugs are Codex models by construction. Luna's ladder
        // is the subset every listed Codex model supports, so mapping onto it
        // can only under-shoot, never hand Codex a level it rejects.
        "codex" => family_ladder("gpt-5.6-luna"),
        _ => None,
    }
}

/// The ladder of a known representative model, used as a family default.
fn family_ladder(representative: &str) -> Option<&'static [String]> {
    catalog().get(representative)?.efforts.as_deref()
}

/// Rank of an effort level in [`EFFORT_ORDER`], or `None` for an unrecognised
/// one.
fn rank(effort: &str) -> Option<usize> {
    let e = effort.trim().to_ascii_lowercase();
    EFFORT_ORDER.iter().position(|l| *l == e)
}

/// Carry `effort` over to `target_model`, clamping it onto that model's ladder.
///
/// Returns `None` when the effort should be dropped — an unrecognised level, or
/// a target whose ladder we have no facts about. Otherwise returns the strongest
/// level the target supports that is no stronger than `effort`, which is
/// `effort` itself whenever the target's ladder contains it.
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
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The built-in file has to parse — it is compiled in, so a syntax error
    /// here is a silently empty catalog in every build, not a startup failure.
    #[test]
    fn builtin_catalog_parses_and_is_not_empty() {
        let entries = parse(BUILTIN).expect("built-in models.toml must be valid TOML");
        assert!(entries.len() >= 10, "got {} entries", entries.len());
        for e in &entries {
            assert!(!e.id.trim().is_empty(), "every entry needs an id");
        }
    }

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
    fn weakest_levels_map_to_the_ladder_floor() {
        assert_eq!(map_effort("minimal", "gpt-5.6-sol"), Some("low"));
        assert_eq!(map_effort("off", "claude-opus-5"), Some("low"));
    }

    /// dsh rows carry a tier but deliberately no ladder — dsh publishes the real
    /// one at runtime — so effort is dropped rather than invented.
    #[test]
    fn dsh_rows_have_a_tier_but_no_ladder() {
        assert_eq!(tier("deepseek-official/deepseek-v4-pro"), Some("premium"));
        assert_eq!(
            tier("deepseek-official/deepseek-v4-flash-vision-exp"),
            Some("fast")
        );
        assert_eq!(effort_ladder("deepseek-official/deepseek-v4-pro"), None);
        assert_eq!(map_effort("high", "deepseek-official/deepseek-v4-pro"), None);
        // An uncatalogued dsh id is equally declined rather than guessed at.
        assert_eq!(effort_ladder("openrouter/anthropic/claude-opus-5"), None);
    }

    /// Codex profile markers name a model the host's profile file picks, so the
    /// ladder is not ours to state.
    #[test]
    fn profile_markers_yield_no_ladder() {
        assert_eq!(effort_ladder("profile:work"), None);
        assert_eq!(effort_ladder(""), None);
    }

    /// An unrecognised effort string is dropped rather than guessed at.
    #[test]
    fn unknown_effort_is_dropped() {
        assert_eq!(map_effort("turbo", "gpt-5.6-sol"), None);
        assert_eq!(map_effort("", "gpt-5.6-sol"), None);
    }

    /// An uncatalogued id still routes to a harness, so it gets that family's
    /// conservative ladder rather than no ladder at all. This is what keeps a
    /// dated Claude alias and a Codex slug newer than this build working.
    #[test]
    fn uncatalogued_ids_fall_back_to_the_family_ladder() {
        assert_eq!(map_effort("max", "claude-haiku-4-5-20251001"), Some("max"));
        assert_eq!(map_effort("ultra", "gpt-7-whatever"), Some("max"));
    }

    /// Every id either picker can hand us must be catalogued with a tier.
    ///
    /// The ladder would survive an omission (the family fallback covers it), but
    /// `tier` would not — an uncatalogued id silently drops out of any
    /// cross-harness tier mapping. This list is the two hand-written pickers,
    /// `claw-fleet-desktop/app/modelChoices.ts` (`CLAUDE_MODEL_CHOICES` +
    /// `CODEX_MODEL_CHOICES`) and `mobile-web/src/views/Composer.tsx`, which
    /// hold the same ids. When those grow an entry, this test is what fails.
    #[test]
    fn every_selectable_model_has_a_tier() {
        for id in [
            "claude-fable-5-1",
            "claude-fable-5",
            "claude-opus-5",
            "claude-opus-4-8",
            "claude-sonnet-5",
            "claude-sonnet-4-6",
            "claude-haiku-4-5-20251001",
            "gpt-6-astra",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
        ] {
            assert!(tier(id).is_some(), "{id} is selectable but has no tier");
            assert!(effort_ladder(id).is_some(), "{id} has no ladder");
        }
    }

    /// Codex rows carry a window; Claude rows deliberately do not, because
    /// Claude's window is a family rule (dated aliases, the `[1m]` suffix) that
    /// an id-keyed table cannot express. `context_window` returning `None` for
    /// Claude is what keeps `stats.rs`'s family logic in charge.
    #[test]
    fn codex_rows_carry_a_window_and_claude_rows_do_not() {
        assert_eq!(context_window("gpt-6-astra"), Some(272_000));
        assert_eq!(context_window("gpt-5.6-sol"), Some(272_000));
        assert_eq!(context_window("gpt-5.5"), Some(272_000));
        assert_eq!(entry("gpt-6-astra").unwrap().max_context, Some(872_000));
        // gpt-5.5 is the one whose ceiling equals its default window.
        assert_eq!(entry("gpt-5.5").unwrap().max_context, Some(272_000));

        assert_eq!(context_window("claude-opus-5"), None);
        assert_eq!(context_window("claude-haiku-4-5-20251001"), None);
    }

    /// The bare aliases are legal `--model` values, so they resolve too.
    #[test]
    fn bare_claude_aliases_resolve() {
        assert_eq!(tier("opus"), Some("premium"));
        assert_eq!(tier("sonnet"), Some("standard"));
        assert_eq!(tier("haiku"), Some("fast"));
        assert_eq!(tier("fable"), Some("premium"));
    }

    /// Fleet's `[1m]` suffix is not part of any model id and must not stop a
    /// lookup from finding the row.
    #[test]
    fn context_suffix_is_stripped_before_lookup() {
        assert_eq!(tier("claude-opus-5[1m]"), tier("claude-opus-5"));
        assert_eq!(tier("  Claude-Opus-5  "), Some("premium"));
    }

    /// A user entry that names one field overrides that field only. Blanking
    /// `tier` here would break the cross-harness tier mapping as a side effect
    /// of someone pinning a ladder.
    #[test]
    fn user_overlay_is_field_level() {
        let base = parse(BUILTIN).unwrap();
        let overlay = parse("[[model]]\nid = \"gpt-5.6-sol\"\nefforts = [\"low\", \"medium\"]\n")
            .unwrap();
        let merged = merge(base, overlay);
        let sol = &merged["gpt-5.6-sol"];
        assert_eq!(
            sol.efforts.as_deref(),
            Some(["low".to_string(), "medium".to_string()].as_slice())
        );
        assert_eq!(sol.tier.as_deref(), Some("premium"), "tier must survive");
        assert_eq!(sol.family.as_deref(), Some("codex"), "family must survive");
    }

    /// An id the built-in file doesn't know is added, not rejected.
    #[test]
    fn user_overlay_can_add_a_model() {
        let overlay =
            parse("[[model]]\nid = \"gpt-9-future\"\ntier = \"premium\"\nefforts = [\"low\"]\n")
                .unwrap();
        let merged = merge(parse(BUILTIN).unwrap(), overlay);
        assert_eq!(merged["gpt-9-future"].tier.as_deref(), Some("premium"));
    }

    /// A malformed user file is ignored rather than emptying the catalog: one
    /// slipped keystroke should not strip ladder facts from every session.
    #[test]
    fn malformed_overlay_is_ignored() {
        assert!(parse("this is not toml = = =").is_none());
        let merged = merge(parse(BUILTIN).unwrap(), parse("nope = = =").unwrap_or_default());
        assert!(merged.contains_key("gpt-5.6-sol"));
    }
}
