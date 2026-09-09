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




/// The harness column value for a row.
fn harness_of(e: &ModelEntry) -> &str {
    match e
        .family
        .as_deref()
        .or_else(|| crate::agent_source::source_for_model(&e.id))
    {
        Some("claude-code") => "claude",
        Some(other) => other,
        None => "?",
    }
}

/// The effort cell: the ladder, dot-separated, or a pointer when we don't state
/// one (dsh publishes its own at runtime).
fn effort_cell(e: &ModelEntry, locale: &str) -> String {
    match e.efforts.as_deref() {
        Some(l) => l.join("·"),
        None => if locale == "zh" { "见 dsh" } else { "ask dsh" }.to_string(),
    }
}

/// **The** model cheat-sheet — one document, shared by all three harnesses.
///
/// It used to be three hand-written sheets (claude / codex / dsh), each with a
/// zh and an en variant, each ordering and paraphrasing the same facts its own
/// way. Six copies is six chances to drift, and they had: all three claimed
/// Codex tops out at `high`, all three put Astra at 1.05M context, and the dsh
/// one listed no dsh model at all.
///
/// So there is one sheet now. It is also deliberately terse: a table of facts
/// (tier, window, price, effort ladder) instead of a prose "when to pick"
/// column, with the tier vocabulary explained once underneath. An agent
/// choosing a model needs the ladder and the tier; it does not need a paragraph
/// per model.
pub fn render_sheet(locale: &str) -> String {
    render_sheet_with(locale, |family| {
        crate::agent_source::find_source_by_api_name(
            &crate::agent_source::build_sources(),
            crate::agent_source::normalize_tool(family),
        )
        .is_some()
    })
}

/// [`render_sheet`] against an injectable availability probe.
///
/// The sheet only lists harnesses that exist on this machine, so tests must be
/// able to say which those are rather than inheriting the developer's
/// `fleet-sources.json` and installed binaries — the same reason
/// [`crate::agent_source::route_launch_with`] takes one.
///
/// **Why filter at all**: the guidance files are regenerated by the app (the
/// desktop re-applies them on every mount), so the sheet can simply reflect this
/// machine. Listing a harness that is not installed, then adding a paragraph
/// telling the agent not to pick from it, asks the reader to do a filtering job
/// the renderer already had the facts to do. `build_sources` is the same probe
/// `route_launch` uses to decide whether a cross-harness spawn is even possible,
/// so the sheet and the spawn agree by construction.
pub fn render_sheet_with(locale: &str, is_available: impl Fn(&str) -> bool) -> String {
    let zh = locale == "zh";
    let mut s = String::new();

    let families: Vec<&str> = ["claude-code", "codex", "dsh"]
        .into_iter()
        .filter(|f| is_available(f))
        .collect();
    // Every probe said no — almost certainly a broken probe rather than a
    // machine with no agent installed at all (something rendered this sheet).
    // An empty table helps nobody, so fall back to listing everything.
    let families: Vec<&str> = if families.is_empty() {
        vec!["claude-code", "codex", "dsh"]
    } else {
        families
    };
    let has = |f: &str| families.contains(&f);

    if zh {
        s.push_str("# Fleet 模型选择速查 (managed by Claw Fleet — do not edit)\n\n");
        s.push_str(
            "给 subagent、workflow agent 或新会话选模型时用。**默认继承父/会话模型**——\
它几乎总是对的;只有当你明确判断某一档更合适时才 override。选模型的入口:`Agent` \
工具的 `model` 参数、`Workflow` 里 `agent()` 的 `opts.model`/`opts.effort`、\
`fleet` spawn 的 `--model`、`cws dispatch` 的 `--model`/`--effort`。\n\n",
        );
        s.push_str("| 模型 | ID | harness | 档次 | 上下文 | effort |\n");
        s.push_str("|---|---|---|---|---|---|\n");
    } else {
        s.push_str("# Fleet model-selection cheat-sheet (managed by Claw Fleet — do not edit)\n\n");
        s.push_str(
            "Use this when picking a model for a subagent, a workflow agent, or a new \
session. **Default to inheriting the parent/session model** — it is almost always \
right; only override when you have a clear reason a different tier fits. The places \
a model gets chosen: the `Agent` tool's `model` param, `Workflow` `agent()`'s \
`opts.model`/`opts.effort`, `fleet` spawn's `--model`, and `cws dispatch`'s \
`--model`/`--effort`.\n\n",
        );
        s.push_str("| Model | ID | Harness | Tier | Context | Effort |\n");
        s.push_str("|---|---|---|---|---|---|\n");
    }

    for family in &families {
        for e in listed_models(family) {
            s.push_str(&format!(
                "| {} | `{}` | {} | {} | {} | {} |\n",
                e.display(),
                e.id,
                harness_of(e),
                e.tier.as_deref().unwrap_or("—"),
                context_cell(e),
                effort_cell(e, locale),
            ));
        }
    }

    // Superseded models, folded into one line rather than a row each.
    let legacy: Vec<String> = catalog()
        .iter()
        .filter(|e| e.superseded_by.is_some())
        // Same harness gate as the table: a superseded model from a harness that
        // is not here would otherwise reappear on this line after its own row
        // was filtered out. Every superseded row happens to be Claude today, so
        // this is guarding the invariant rather than a live bug.
        .filter(|e| {
            e.family
                .as_deref()
                .or_else(|| crate::agent_source::source_for_model(&e.id))
                .is_some_and(|f| has(f))
        })
        .map(|e| {
            let caveat = if zh { &e.legacy_note_zh } else { &e.legacy_note_en };
            match caveat.as_deref() {
                Some(c) if zh => format!("`{}`（{c}）", e.id),
                Some(c) => format!("`{}` ({c})", e.id),
                None => format!("`{}`", e.id),
            }
        })
        .collect();
    if !legacy.is_empty() {
        s.push('\n');
        if zh {
            s.push_str(&format!("前代同价仍可选:{}。\n", legacy.join("、")));
        } else {
            s.push_str(&format!(
                "Previous generations, still selectable at the same price: {}.\n",
                legacy.join(", ")
            ));
        }
    }

    if zh {
        if has("dsh") {
            s.push_str(
                "\n\
`deepseek-v4-flash` **不收图片输入**,要发图走 `-vision-exp` 那个。\
`deepseek-v4-pro` 的上下文列是 `—`:本机还没有它的会话,而 dsh 的窗口是会话运行时\
上报的,没测到就不编。\n",
            );
        }
        s.push_str(
            "\n## 档次\n\n\
- **premium** — 最强推理。硬推理、最终综合、对抗性校验、长程 agentic 主循环。最贵,别拿它做机械活。\n\
- **standard** — 接近 premium 的编码能力,成本明显更低。日常编码、高吞吐生产的默认选择。\n\
- **fast** — 最快最便宜。分类、抽取、简单机械活、可并行的大批量 subagent、延迟敏感任务。\n\
\n\
effort 与档次是**两个独立的旋钮**:档次决定用哪个模型,effort 决定它想多久。\
两边都往上顶最贵。编码和 agentic 一般 `xhigh` 最划算;`low` 给 subagent 和简单\
任务(更少、更集中的工具调用)。\n",
        );
        if has("dsh") {
            s.push_str(
                "\n\
## dsh 怎么点名模型\n\
\n\
dsh 把模型拆成 `provider` + `model` 两段,Fleet 的 spawn 用一个字符串表达,\
以**第一个 `/`** 分界:`openrouter/anthropic/claude-haiku-4.5` → provider \
`openrouter`,model `anthropic/claude-haiku-4.5`。表里只列 dsh 内置的 \
`deepseek-official` 路由(有官方公开价目表);经 openrouter 之类第三方 provider \
的模型不列——同一个模型经不同 provider 价格不同、逐用户不同,要知道本机配了\
什么就读 `~/.dsh/settings.yaml`,别猜。`effort` 一列写「见 dsh」的,是 dsh 在\
运行时自己发布每个模型的真实梯子(它有 `off` 这种别家没有的档),Fleet 不替它\
断言。\n",
            );
        }
        if has("codex") {
            s.push_str(
                "\n\
## 生图(只有 codex 有)\n\
\n\
Claude 侧**没有**生图能力。要位图资产(插画、贴图、mockup、hero 图)时借 codex \
自带的 imagegen skill:`codex exec -m gpt-5.6-luna \"用内置图像生成工具画 …\"`。\
模型是 `gpt-image-2`,走 ChatGPT 配额,**不需要** `OPENAI_API_KEY`。产物落 \
`$CODEX_HOME/generated_images/<thread_id>/`。细节见 wiki `codex/image-generation`。\n",
            );
        }
    } else {
        if has("dsh") {
            s.push_str(
                "\n\
`deepseek-v4-flash` **rejects image input** — send images to the `-vision-exp` row \
instead. `deepseek-v4-pro` shows `—` for context: no session for it has run here, and \
dsh reports the window at runtime, so it is left blank rather than guessed.\n",
            );
        }
        s.push_str(
            "\n## Tiers\n\n\
- **premium** — strongest reasoning. Hard reasoning, final synthesis, adversarial \
verification, long-horizon agentic main loops. The most expensive; don't spend it on \
mechanical work.\n\
- **standard** — near-premium coding at noticeably lower cost. The default for \
everyday coding and high-throughput production.\n\
- **fast** — fastest and cheapest. Classification, extraction, simple mechanical work, \
parallel high-volume subagents, latency-sensitive tasks.\n\
\n\
Tier and effort are **two independent knobs**: the tier picks which model, the effort \
picks how long it thinks. Turning both up is the expensive corner. `xhigh` is usually \
the sweet spot for coding and agentic work; `low` suits subagents and simple tasks \
(fewer, more-consolidated tool calls).\n",
        );
        if has("dsh") {
            s.push_str(
                "\n\
## How dsh names a model\n\
\n\
dsh addresses a model as `provider` + `model`. Fleet's spawn carries one string and \
splits on the **first `/`**: `openrouter/anthropic/claude-haiku-4.5` → provider \
`openrouter`, model `anthropic/claude-haiku-4.5`. The table lists only dsh's built-in \
`deepseek-official` route, which has a published price table; models reached through a \
third-party provider such as openrouter are not listed — the same model costs \
different amounts through different providers and varies per user. Read \
`~/.dsh/settings.yaml` to see what this machine has; don't guess. Rows whose effort \
cell says \"ask dsh\" are ones where dsh publishes each model's real ladder at runtime \
(it has levels such as `off` that no other harness offers), so Fleet does not assert \
one on its behalf.\n",
            );
        }
        if has("codex") {
            s.push_str(
                "\n\
## Image generation (codex only)\n\
\n\
The Claude side has **no** image-generation capability. When you need a raster asset \
(illustration, sprite, mockup, hero image), borrow codex's bundled imagegen skill: \
`codex exec -m gpt-5.6-luna \"use the built-in image generation tool to draw …\"`. The \
model is `gpt-image-2`, it bills against the ChatGPT quota, and it does **not** need \
`OPENAI_API_KEY`. Output lands in `$CODEX_HOME/generated_images/<thread_id>/`. Details \
live in the wiki at `codex/image-generation`.\n",
            );
        }
    }
    s
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

    /// A harness that is not installed here does not appear at all — neither its
    /// rows nor the prose that only makes sense alongside them.
    ///
    /// This replaced a paragraph that listed every harness and then told the
    /// agent not to pick from the missing ones. The guidance file is regenerated
    /// by the app, so the renderer already knows; making the reader filter is
    /// work we can just do.
    #[test]
    fn an_absent_harness_is_omitted_entirely() {
        let claude_only = render_sheet_with("zh", |f| f == "claude-code");
        assert!(claude_only.contains("claude-opus-5"));
        assert!(!claude_only.contains("gpt-5.6-sol"), "codex rows leaked");
        assert!(!claude_only.contains("deepseek"), "dsh rows leaked");
        // The dsh-only and codex-only sections go with them.
        assert!(!claude_only.contains("dsh 怎么点名模型"));
        assert!(!claude_only.contains("生图"));
        assert!(!claude_only.contains("DeepSeek 那三行"));
        // The tier vocabulary is harness-independent and stays.
        assert!(claude_only.contains("## 档次"));

        let no_dsh = render_sheet_with("en", |f| f != "dsh");
        assert!(no_dsh.contains("gpt-5.6-sol"));
        assert!(!no_dsh.contains("deepseek"));
        assert!(!no_dsh.contains("How dsh names a model"));
        // Codex is present, so its section stays.
        assert!(no_dsh.contains("Image generation (codex only)"));
    }

    /// A probe that answers "no" to everything is a broken probe, not a machine
    /// with no agent on it — something had to render this sheet. Falling back to
    /// the full table beats emitting an empty one.
    #[test]
    fn a_probe_that_finds_nothing_falls_back_to_everything() {
        let all = render_sheet_with("zh", |_| false);
        assert!(all.contains("claude-opus-5"));
        assert!(all.contains("gpt-5.6-sol"));
        assert!(all.contains("deepseek-official/deepseek-v4-pro"));
    }

    /// There is **one** sheet, not three. All three harnesses render the same
    /// bytes for a given locale.
    ///
    /// This is the whole point of the merge: three hand-written variants meant
    /// three chances to be wrong about one fact, and all three were wrong about
    /// Codex's effort ladder at once. If someone reintroduces a per-harness
    /// flavour, this fails.
    #[test]
    fn all_three_harnesses_get_the_same_sheet() {
        for locale in ["zh", "en"] {
            let a = crate::model_guidance::render_guidance(locale);
            let b = crate::codex_guidance::render_codex_model_block(locale);
            let c = crate::dsh_guidance::render_dsh_model_block(locale);
            assert_eq!(a, b, "codex sheet diverged ({locale})");
            assert_eq!(a, c, "dsh sheet diverged ({locale})");
            assert!(!a.trim().is_empty());
        }
    }

    /// The sheet must not mix punctuation systems: the English variant had CJK
    /// full-width parens around the legacy caveat, because the clause builder
    /// used `（）` unconditionally.
    #[test]
    fn english_sheet_uses_ascii_punctuation() {
        let en = render_sheet("en");
        for bad in ['（', '）', '：', '；', '、', '，', '。'] {
            assert!(!en.contains(bad), "English sheet contains `{bad}`");
        }
    }

    /// Every listed row carries a tier (which the sheet prints) and a price or a
    /// quota flag (which it does not).
    ///
    /// The sheet dropped its price column deliberately: what a model costs per
    /// token is not how an agent should be choosing one — that is what the tier
    /// vocabulary is for. The price fields stay in the catalog because cost
    /// accounting needs them (`model_cost`), and this keeps them complete so
    /// that consumer can rely on them.
    #[test]
    fn every_listed_row_has_a_tier_and_a_price_or_quota() {
        for family in ["claude-code", "codex", "dsh"] {
            for e in listed_models(family) {
                assert!(e.tier.is_some(), "{} has no tier", e.id);
                let priced = e.price_in.is_some() && e.price_out.is_some();
                let quota = e.quota_billed.unwrap_or(false);
                assert!(priced || quota, "{} has neither a price nor a quota flag", e.id);
            }
        }
    }

    /// dsh's own three rows stay listed, and state exactly what was measured.
    ///
    /// They were `listed = false` at first, on the reasoning that dsh publishes
    /// its real catalog at runtime and a static copy would rot. That reasoning
    /// holds for the ~270 openrouter entries and not for the built-in
    /// `deepseek-official` route. The result was a cheat-sheet written *for a dsh
    /// agent* that named no dsh model at all.
    ///
    /// The window rule is the interesting half. dsh reports
    /// `contextPressure.contextWindow` per session, so the catalog states a
    /// window only where a real local session reported one: both Flash rows did
    /// (1M, consistently), Pro never ran here. Pro is therefore blank — not
    /// filled in from the family's "probably the same". Ladders stay unasserted
    /// for all three; those are dsh's to publish.
    #[test]
    fn dsh_route_models_state_only_what_was_measured() {
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
            assert!(e.tier.is_some(), "{} has no tier", e.id);
            assert_eq!(e.efforts, None, "{} must not assert a ladder", e.id);
        }
        assert_eq!(context_window("deepseek-official/deepseek-v4-flash"), Some(1_000_000));
        assert_eq!(
            context_window("deepseek-official/deepseek-v4-flash-vision-exp"),
            Some(1_000_000)
        );
        assert_eq!(
            context_window("deepseek-official/deepseek-v4-pro"),
            None,
            "no local session measured Pro's window; it must stay blank rather than \
             inherit the family's"
        );
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
