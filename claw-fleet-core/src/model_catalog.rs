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
// No `Eq`: prices are `f64`, and float equality is not an equivalence relation.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
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
    /// Display name for the cheat-sheets ("Fable 5.1", "Sol").
    #[serde(default)]
    pub label: Option<String>,
    /// USD per million input / output tokens.
    #[serde(default)]
    pub price_in: Option<f64>,
    #[serde(default)]
    pub price_out: Option<f64>,
    /// Billed against a plan quota, with no per-token price (all of Codex).
    #[serde(default)]
    pub quota_billed: Option<bool>,
    /// The "when to pick this" sentence, shared by all three cheat-sheets.
    #[serde(default)]
    pub note_zh: Option<String>,
    #[serde(default)]
    pub note_en: Option<String>,
    /// The long form of the "when to pick" sentence, with the caveats worth
    /// stating once (data-retention requirements, API quirks, intro pricing).
    ///
    /// Only the Claude-side cheat-sheet prints this: it lands in `CLAUDE.md`,
    /// which has room. The Codex and dsh sheets land in `AGENTS.md` files with a
    /// 32 KiB ceiling that the whole Fleet block set shares, so they print
    /// `note_*` and stay terse. Absent → falls back to `note_*`.
    #[serde(default)]
    pub detail_zh: Option<String>,
    #[serde(default)]
    pub detail_en: Option<String>,
    /// `false` = resolvable but kept out of the cheat-sheet tables (bare
    /// aliases, dsh rows). Absent means listed.
    #[serde(default)]
    pub listed: Option<bool>,
    /// Superseded by this id. Such a row is not printed on its own; it is folded
    /// into the successor's row as "previous X still selectable, same price" —
    /// the shape all three cheat-sheets already used by hand.
    #[serde(default)]
    pub superseded_by: Option<String>,
    /// A difference worth keeping when this row folds into its successor's, e.g.
    /// an API restriction the newer model has and the older one does not.
    #[serde(default)]
    pub legacy_note_zh: Option<String>,
    #[serde(default)]
    pub legacy_note_en: Option<String>,
}

impl ModelEntry {
    /// Whether this row belongs in a cheat-sheet table.
    pub fn is_listed(&self) -> bool {
        self.listed.unwrap_or(true) && self.superseded_by.is_none()
    }

    /// Display name, falling back to the id when none is set.
    pub fn display(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.id)
    }

    /// The localized short "when to pick" sentence.
    pub fn note(&self, locale: &str) -> &str {
        let picked = if locale == "zh" { &self.note_zh } else { &self.note_en };
        picked.as_deref().unwrap_or("")
    }

    /// The localized long form, falling back to [`Self::note`].
    pub fn detail(&self, locale: &str) -> &str {
        let picked = if locale == "zh" { &self.detail_zh } else { &self.detail_en };
        picked.as_deref().unwrap_or_else(|| self.note(locale))
    }

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
        if other.label.is_some() {
            self.label = other.label;
        }
        if other.price_in.is_some() {
            self.price_in = other.price_in;
        }
        if other.price_out.is_some() {
            self.price_out = other.price_out;
        }
        if other.quota_billed.is_some() {
            self.quota_billed = other.quota_billed;
        }
        if other.note_zh.is_some() {
            self.note_zh = other.note_zh;
        }
        if other.note_en.is_some() {
            self.note_en = other.note_en;
        }
        if other.detail_zh.is_some() {
            self.detail_zh = other.detail_zh;
        }
        if other.detail_en.is_some() {
            self.detail_en = other.detail_en;
        }
        if other.listed.is_some() {
            self.listed = other.listed;
        }
        if other.superseded_by.is_some() {
            self.superseded_by = other.superseded_by;
        }
        if other.legacy_note_zh.is_some() {
            self.legacy_note_zh = other.legacy_note_zh;
        }
        if other.legacy_note_en.is_some() {
            self.legacy_note_en = other.legacy_note_en;
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

/// Merge a user document over a base catalog, matching on lowercased id.
///
/// **Order is preserved**: the built-in rows keep their file order and new user
/// rows are appended. The cheat-sheet tables are rendered straight off this
/// sequence, so ordering here is what puts Fable above Opus above Sonnet in the
/// generated markdown rather than whatever an id sort would produce.
fn merge(base: Vec<ModelEntry>, overlay: Vec<ModelEntry>) -> Vec<ModelEntry> {
    let mut out = base;
    let mut index: BTreeMap<String, usize> = out
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.trim().to_ascii_lowercase(), i))
        .collect();
    for entry in overlay {
        let key = entry.id.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        match index.get(&key) {
            Some(&i) => out[i].overlay(entry),
            None => {
                index.insert(key, out.len());
                out.push(entry);
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
pub fn catalog() -> &'static [ModelEntry] {
    static CATALOG: OnceLock<Vec<ModelEntry>> = OnceLock::new();
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
    let id = normalize_id(model);
    catalog().iter().find(|e| e.id.trim().eq_ignore_ascii_case(&id))
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
    if let Some(efforts) = entry(&id).and_then(|e| e.efforts.as_deref()) {
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
    entry(representative)?.efforts.as_deref()
}

// ── Cheat-sheet rendering ───────────────────────────────────────────────────
//
// The three `*_guidance.rs` cheat-sheets used to hand-write the same model
// tables, six times over (claude / codex / dsh × zh / en). They drifted: all
// three claimed Codex tops out at `high` and offers `minimal`, and all three
// put Astra at 1.05M context. These helpers render the tables from the catalog
// so a fact is stated once.
//
// What stays per-harness is *prose*, not facts: which family leads, how dense
// the columns are, and dsh's preamble about `provider/model` addressing.

/// Format a token count the way the cheat-sheets write it: `1M`, `272K`.
fn window_label(tokens: u64) -> String {
    if tokens >= 1_000_000 && tokens % 1_000_000 == 0 {
        format!("{}M", tokens / 1_000_000)
    } else if tokens >= 1_000_000 {
        format!("{:.2}M", tokens as f64 / 1_000_000.0)
    } else {
        format!("{}K", tokens / 1_000)
    }
}

/// The context-window cell for a row.
///
/// Claude rows carry no `context` on purpose (the window is a family rule), so
/// this asks that rule — [`crate::session::stats::context_window_for_model`] —
/// rather than duplicating its answer into the table. Codex rows answer from
/// their catalogued value.
fn context_cell(e: &ModelEntry) -> String {
    let tokens = e
        .context
        .or_else(|| crate::session::stats::context_window_for_model(&e.id, 0));
    tokens.map(window_label).unwrap_or_else(|| "—".to_string())
}

/// The price cell: a per-Mtok pair, or the quota note.
fn price_cell(e: &ModelEntry, locale: &str) -> String {
    if e.quota_billed.unwrap_or(false) {
        return if locale == "zh" { "ChatGPT 套餐配额" } else { "ChatGPT-plan quota" }.to_string();
    }
    match (e.price_in, e.price_out) {
        (Some(i), Some(o)) => format!("${} / ${}", trim_price(i), trim_price(o)),
        _ => "—".to_string(),
    }
}

/// `10.0` → `10`, `2.5` → `2.5`. Prices read as money, not as floats.
fn trim_price(v: f64) -> String {
    if (v.fract()).abs() < f64::EPSILON {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// The "(previous `x` still selectable, same price)" clause for a row, built
/// from whichever rows name it in `superseded_by`.
fn legacy_clause(e: &ModelEntry, locale: &str) -> String {
    let olds: Vec<&ModelEntry> = catalog()
        .iter()
        .filter(|o| {
            o.superseded_by
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(&e.id))
        })
        .collect();
    if olds.is_empty() {
        return String::new();
    }
    let list = olds
        .iter()
        .map(|o| format!("`{}`", o.id))
        .collect::<Vec<_>>()
        .join(" / ");
    // Caveats trail the whole clause rather than interrupting it — "前代 `x`
    // 同价仍可选（caveat）", not "前代 `x`（caveat） 同价仍可选".
    let caveats: Vec<&str> = olds
        .iter()
        .filter_map(|o| {
            if locale == "zh" { o.legacy_note_zh.as_deref() } else { o.legacy_note_en.as_deref() }
        })
        .collect();
    if locale == "zh" {
        let tail = if caveats.is_empty() {
            String::new()
        } else {
            format!("（{}）", caveats.join("；"))
        };
        format!("前代 {list} 同价仍可选{tail}")
    } else {
        let tail = if caveats.is_empty() {
            String::new()
        } else {
            format!(" ({})", caveats.join("; "))
        };
        format!("Previous {list} still selectable at the same price{tail}.")
    }
}

/// Append the legacy clause to a model's prose with a sentence break.
///
/// The two prose fields differ: `detail_*` is written as full sentences and
/// already ends in a stop, `note_*` is a bare phrase and does not. Concatenating
/// blindly produced "只用在最难的任务前代 `claude-fable-5` 同价仍可选" — two
/// sentences fused into a run-on. So supply the stop when the prose lacks one.
fn join_prose(prose: &str, legacy: &str, locale: &str) -> String {
    if legacy.is_empty() {
        return prose.to_string();
    }
    if prose.is_empty() {
        return legacy.to_string();
    }
    let ends_sentence = prose.ends_with('。') || prose.ends_with('.') || prose.ends_with('；');
    if locale == "zh" {
        if ends_sentence {
            format!("{prose}{legacy}")
        } else {
            format!("{prose}。{legacy}")
        }
    } else if ends_sentence {
        format!("{prose} {legacy}")
    } else {
        format!("{prose}. {legacy}")
    }
}

/// The listed rows of one family, in catalog order.
pub fn listed_models(family: &str) -> Vec<&'static ModelEntry> {
    catalog()
        .iter()
        .filter(|e| e.is_listed())
        .filter(|e| {
            e.family
                .as_deref()
                .or_else(|| crate::agent_source::source_for_model(&e.id))
                .is_some_and(|f| f == family)
        })
        .collect()
}

/// Render one family's cheat-sheet table as markdown rows (no header).
///
/// `split_price` picks the shape the calling cheat-sheet uses: the Claude-side
/// sheet splits input and output price into their own columns, the Codex and dsh
/// sheets merge them. It also selects the prose length — the split-column sheet
/// is the roomy one in `CLAUDE.md`, so it gets [`ModelEntry::detail`]; the
/// merged-column sheets go into budget-capped `AGENTS.md` files and get the
/// short [`ModelEntry::note`].
pub fn render_rows(family: &str, locale: &str, split_price: bool) -> String {
    render_rows_with(family, locale, split_price, true)
}

/// [`render_rows`] with the price column suppressed.
///
/// A Codex-only table repeats "ChatGPT-plan quota" on every row while the
/// paragraph above it already says so; the column carries no information there.
/// The dsh sheet lists both families in one table, so it keeps the column —
/// there the quota note is a real contrast against dollar amounts.
pub fn render_rows_with(
    family: &str,
    locale: &str,
    split_price: bool,
    show_price: bool,
) -> String {
    let mut out = String::new();
    for e in listed_models(family) {
        let prose = if split_price { e.detail(locale) } else { e.note(locale) };
        let note = join_prose(prose, &legacy_clause(e, locale), locale);
        if split_price && !e.quota_billed.unwrap_or(false) {
            let (pi, po) = (
                e.price_in.map(trim_price).unwrap_or_else(|| "—".into()),
                e.price_out.map(trim_price).unwrap_or_else(|| "—".into()),
            );
            out.push_str(&format!(
                "| {} | `{}` | {} | ${} | ${} | {} |\n",
                e.display(),
                e.id,
                context_cell(e),
                pi,
                po,
                note
            ));
        } else if show_price {
            out.push_str(&format!(
                "| {} | `{}` | {} | {} | {} |\n",
                e.display(),
                e.id,
                context_cell(e),
                price_cell(e, locale),
                note
            ));
        } else {
            out.push_str(&format!(
                "| {} | `{}` | {} | {} |\n",
                e.display(),
                e.id,
                context_cell(e),
                note
            ));
        }
    }
    out
}

/// One sentence describing the effort ladders in `family`, built from the
/// catalog so it cannot claim a level the models do not accept.
///
/// Rows that share a ladder are named together; a row that differs gets its own
/// clause. That is what surfaces "everything supports xhigh **except** gpt-5.5"
/// without anyone maintaining the exception by hand.
pub fn render_effort_line(family: &str, locale: &str) -> String {
    let mut groups: Vec<(String, Vec<&str>)> = Vec::new();
    for e in listed_models(family) {
        let Some(ladder) = e.efforts.as_deref() else { continue };
        let key = ladder.join("/");
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, names)) => names.push(e.display()),
            None => groups.push((key, vec![e.display()])),
        }
    }
    if groups.is_empty() {
        return String::new();
    }
    // A family where everything shares one ladder just states the ladder —
    // naming all four Claude models before an identical list is noise.
    if groups.len() == 1 {
        return groups[0]
            .0
            .split('/')
            .map(|l| format!("`{l}`"))
            .collect::<Vec<_>>()
            .join("/");
    }
    let (colon, sep) = if locale == "zh" { ("：", "；") } else { (": ", "; ") };
    let clauses: Vec<String> = groups
        .iter()
        .map(|(ladder, names)| {
            let levels = ladder
                .split('/')
                .map(|l| format!("`{l}`"))
                .collect::<Vec<_>>()
                .join("/");
            format!("{}{colon}{levels}", names.join(" / "))
        })
        .collect();
    clauses.join(sep)
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

    /// Look a row up in a merged catalog by id, panicking when absent.
    fn find<'a>(catalog: &'a [ModelEntry], id: &str) -> &'a ModelEntry {
        catalog
            .iter()
            .find(|e| e.id == id)
            .unwrap_or_else(|| panic!("no `{id}` row in the merged catalog"))
    }

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

    /// dsh's own three rows must stay listed with a price.
    ///
    /// They were `listed = false` at first, on the reasoning that dsh publishes
    /// its real catalog at runtime and a static copy would rot. That reasoning
    /// holds for the ~270 openrouter entries and does not hold for the built-in
    /// `deepseek-official` route, which has a published price table
    /// (`dsh_cost::DEEPSEEK_PEAK_RATES`). The result was a cheat-sheet written
    /// *for a dsh agent* that named no dsh model at all. This test is what stops
    /// that from happening again quietly.
    #[test]
    fn dsh_route_models_are_listed_with_prices() {
        let rows = listed_models("dsh");
        let ids: Vec<&str> = rows.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "deepseek-official/deepseek-v4-pro",
                "deepseek-official/deepseek-v4-flash",
                "deepseek-official/deepseek-v4-flash-vision-exp",
            ]
        );
        for e in &rows {
            assert!(e.price_in.is_some() && e.price_out.is_some(), "{} has no price", e.id);
            assert!(!e.note("zh").is_empty(), "{} has no zh note", e.id);
            assert!(!e.note("en").is_empty(), "{} has no en note", e.id);
            // Still no ladder and no window: those are dsh's to report, not ours.
            assert_eq!(e.efforts, None, "{} must not assert a ladder", e.id);
            assert_eq!(e.context, None, "{} must not assert a window", e.id);
        }
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
        let sol = find(&merged, "gpt-5.6-sol");
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
        assert_eq!(find(&merged, "gpt-9-future").tier.as_deref(), Some("premium"));
    }

    /// A malformed user file is ignored rather than emptying the catalog: one
    /// slipped keystroke should not strip ladder facts from every session.
    #[test]
    fn malformed_overlay_is_ignored() {
        assert!(parse("this is not toml = = =").is_none());
        let merged = merge(parse(BUILTIN).unwrap(), parse("nope = = =").unwrap_or_default());
        assert!(merged.iter().any(|e| e.id == "gpt-5.6-sol"));
    }
}
