//! Real per-session spend for dsh sessions, read from the provider rather than
//! inferred from token counts.
//!
//! # Why not price the tokens
//!
//! Fleet's own price table ([`crate::model_cost`]) knows Claude and GPT tiers.
//! dsh routes through an open model space — OpenRouter alone fronts dozens of
//! providers — and **the same model costs different amounts through different
//! providers**, so multiplying dsh's token counts by any table Fleet holds would
//! produce a confident wrong number. A wrong number is worse than none.
//!
//! So this asks the provider what it actually charged.
//!
//! # …except where the price list is fixed and public
//!
//! That reasoning turns on the word *different*: it is the open model space that
//! makes a table untrustworthy. dsh's built-in `deepseek-official` route has no
//! such variable — one seller, one published price list — so tokens × rate is
//! exact there, not a guess, and it is the only number available: that adapter
//! emits no replay envelope and keeps no response id on success (verified in
//! dsh's source: `llm-deepseek` contains no `replayState`, and the
//! `x-request-id` it reads is attached to errors only), and DeepSeek publishes
//! no receipt endpoint to ask anyway.
//!
//! So the module has two pricing paths, disjoint by construction — receipts
//! keyed on a generation id, and a table keyed on the provider name — and a
//! session that mixes providers is summed once per call. The note always says
//! which calls came from which, because "an invoice said so" and "we applied a
//! published rate to counted tokens" are different kinds of number.
//!
//! # How a session's calls are found (measured)
//!
//! Every successful model call leaves a durable `assistant/message` event whose
//! `data.message.source` is `{kind: "model", provider, model, replayState}`.
//! Measured against a real 120-event session: three model calls, three distinct
//! `responseId`s of the form `gen-1786757116-pDmWgCIKXi2AWZCPaQTI`, each tagged
//! `provider: "openrouter"`. The same id also rides the preceding
//! `assistant/chunk` `finish` frame; this module reads it off the **message**
//! because that is the durable record and there is exactly one per call.
//!
//! ## Where the id sits: two shapes, both live
//!
//! dsh later wrapped the adapter's response record in a replay *envelope* —
//! `interface ReplayEnvelope { response: unknown; blocks?: readonly unknown[] }`
//! in `@deepseek-ai/dsh-llm/lib/types/types.d.ts` — moving the id one level in:
//!
//! - flat (the payload carries `version: 1`): `replayState.responseId`
//! - envelope (`version: 2`): `replayState.response.responseId`
//!
//! Measured on one real install: 64 sessions, 35 flat and 14 envelope, and the
//! envelope ones were the newest — so reading only the flat path meant every
//! session a user was likely to open reported "no model calls" while its ids sat
//! right there on the wire. Both paths are read, and old sessions keep pricing.
//!
//! A session's real spend is therefore the sum over its `responseId`s.
//!
//! # OpenRouter
//!
//! `GET https://openrouter.ai/api/v1/generation?id=<gen-id>` with a bearer key
//! returns `data.total_cost` in **USD** (not credits). A generation record is
//! immutable once written, which is what makes the on-disk cache sound: an id
//! priced once never needs asking again.
//!
//! Calls made through any other provider are counted but not priced — dsh's
//! other adapters expose no cost endpoint. The panel says how many were left
//! out rather than quietly under-reporting the total.
//!
//! # A fresh generation is not queryable right away (measured)
//!
//! `/generation` answers `404 Generation … not found` for an id that was just
//! produced, and starts answering with the real record some time later.
//! Measured on one pair of ids: still 404 at roughly one minute and again at
//! two and a half minutes after the turn, priced normally when re-queried later
//! in the same working session; ids from two days earlier resolved immediately.
//! The exact window is **not** established — only that it is longer than the
//! ~2.5 minutes observed.
//!
//! (An earlier reading of this blamed interrupted turns. That was wrong: the
//! "control" session was five minutes older and queried at a different moment,
//! so it never controlled for anything. The interrupted session priced fine once
//! enough time had passed.)
//!
//! Consequence for the panel: opening a session's cost immediately after a turn
//! can legitimately show fewer priced calls than it has model messages. Those
//! land in `unpriced_calls` with a note and are never folded into the total as a
//! zero — "we could not price this yet" and "this was free" are different facts.
//! Because the on-disk cache only ever stores successful lookups, a later visit
//! re-asks and fills the gap in.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The provider whose cost API this module speaks.
const OPENROUTER: &str = "openrouter";

const GENERATION_ENDPOINT: &str = "https://openrouter.ai/api/v1/generation";

/// The env var holding the OpenRouter secret when `settings.yaml` does not name
/// one — dsh's own conventional choice, and the key `.credentials.yaml` uses.
const DEFAULT_KEY_ENV: &str = "OPENROUTER_API_KEY";

/// Cache file under `~/.fleet`. Generation records are immutable, so an entry
/// never expires.
const CACHE_FILE: &str = "dsh-generation-cost.json";

/// A single billable model call found in a session's durable log.
#[derive(Clone, Debug, PartialEq)]
pub struct GenerationRef {
    /// The provider's own id for this call (`gen-…` on OpenRouter).
    pub id: String,
    /// `openrouter`, `deepseek-official`, …
    pub provider: String,
    pub model: String,
}

/// What a dsh session actually cost, plus an honest account of what could not
/// be priced.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DshSessionCost {
    /// Summed `total_cost` over every priced call, in USD. `None` when nothing
    /// could be priced at all — rendered as "unavailable", never as `$0.00`.
    pub total_usd: Option<f64>,
    /// Model calls whose real cost was obtained.
    pub priced_calls: u32,
    /// The subset of `priced_calls` costed from a published rate table rather
    /// than from the provider's own receipt. The panel must say which it is
    /// showing: "the seller told us" and "we applied a published rate to counted
    /// tokens" are both honest, but they are not the same claim.
    pub table_priced_calls: u32,
    /// OpenRouter calls the API would not price (unknown id, request failed).
    pub unpriced_calls: u32,
    /// Calls through a provider with no cost API — counted, never guessed at.
    pub unpriceable_calls: u32,
    /// Why the total is absent or partial, for the panel to show verbatim.
    /// Empty when every call was priced.
    pub note: String,
}

// ── Fixed-price routes ───────────────────────────────────────────────────────

/// dsh's built-in DeepSeek route (plugin `llm-deepseek`), as it names itself in
/// `assistant/message.source.provider`.
const DEEPSEEK_OFFICIAL: &str = "deepseek-official";

/// One model call Fleet can price from a published table instead of a receipt.
#[derive(Clone, Debug, PartialEq)]
pub struct MeteredCall {
    pub provider: String,
    pub model: String,
    /// The durable event's `seq`, which identifies this call within its session
    /// and so keys its frozen price. `None` when the event carried no usable
    /// `seq`: such a call is priced live and never cached, because a price that
    /// cannot be addressed again cannot be looked up again either.
    ///
    /// Measured before relying on it: across 79 local sessions every one of the
    /// 238 `assistant/message` events carried an integer `seq`, and the 139
    /// `deepseek-official` calls among them were unique within their session.
    pub seq: Option<i64>,
    /// The durable event's `time`, in ms since the epoch — what decides the tier.
    pub at_ms: i64,
    /// Input tokens that were NOT served from cache. dsh reports the two
    /// separately and they do not overlap (measured: a call with
    /// `inputTokens: 3` alongside `cacheReadTokens: 6756`).
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
}

/// USD per 1M tokens at **peak** rates for one model. Off-peak is exactly half,
/// so only one row per model is stored.
struct PeakRates {
    model: &'static str,
    cache_hit_input: f64,
    cache_miss_input: f64,
    output: f64,
}

/// DeepSeek's published rates, effective 2026-08-16 16:00 UTC (the release that
/// introduced peak/off-peak). Cross-checked against the official pricing page
/// and DeepSeek's own announcement.
///
/// A model absent from this table is **not** priced — see [`price_metered`].
const DEEPSEEK_PEAK_RATES: &[PeakRates] = &[
    PeakRates {
        model: "deepseek-v4-flash",
        cache_hit_input: 0.014,
        cache_miss_input: 0.44,
        output: 1.32,
    },
    PeakRates {
        model: "deepseek-v4-pro",
        cache_hit_input: 0.044,
        cache_miss_input: 1.32,
        output: 3.96,
    },
    // Published at flash's rates, as its own row on the pricing page.
    PeakRates {
        model: "deepseek-v4-flash-vision-exp",
        cache_hit_input: 0.014,
        cache_miss_input: 0.44,
        output: 1.32,
    },
];

/// Is `at_ms` inside DeepSeek's peak window?
///
/// Peak is 01:00–04:00 and 06:00–10:00 UTC **Monday through Friday** — 7 hours a
/// weekday, 35 a week; every other hour, the whole weekend included, is off-peak
/// at half price. (The 16:30–00:30 window that discounted V3/R1 is retired and
/// must not be used.)
///
/// The weekday is on DeepSeek's own clock, not UTC, so this shifts to UTC+8
/// before splitting the day: the same two windows read 09:00–12:00 and
/// 14:00–18:00 in Beijing time. Taking the weekday in UTC instead would agree at
/// all 168 hours today — both windows end well before 16:00 UTC, which is where
/// a UTC date and a Beijing date begin to disagree — so no test against the
/// published windows could tell the two readings apart. Shifting first makes the
/// right answer structural rather than a coincidence that would break silently
/// if a window ever moved past 16:00 UTC.
fn is_peak(at_ms: i64) -> bool {
    // DeepSeek bills on Beijing time (UTC+8, no DST).
    let local_ms = at_ms + 8 * 3_600_000;
    // 1970-01-01 was a Thursday, so day 0 maps to 4 with Mon = 1 … Sun = 7.
    let weekday = (local_ms.div_euclid(86_400_000) + 3).rem_euclid(7) + 1;
    if weekday > 5 {
        return false;
    }
    let hour = local_ms.div_euclid(3_600_000).rem_euclid(24);
    (9..12).contains(&hour) || (14..18).contains(&hour)
}

/// What one metered call was charged, frozen at the moment it was first priced.
///
/// The tier rides along with the dollars because the panel prints both and they
/// come off the same decision: keeping the money but recomputing the tier would
/// let the "N peak / M off-peak" line contradict the total beside it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
struct MeteredPrice {
    usd: f64,
    peak: bool,
}

/// Apply the published table to one call. `None` when the model is not on it.
///
/// An unknown model is counted, never estimated — the module's whole reason for
/// asking the provider is that a plausible-looking wrong number is worse than
/// an absent one, and that applies just as much to a stale price table.
fn rate_price(call: &MeteredCall) -> Option<MeteredPrice> {
    let rates = DEEPSEEK_PEAK_RATES.iter().find(|r| r.model == call.model)?;
    let peak = is_peak(call.at_ms);
    let scale = if peak { 1.0 } else { 0.5 };
    let usd = scale
        * (rates.cache_miss_input * call.input_tokens as f64
            + rates.cache_hit_input * call.cache_read_tokens as f64
            + rates.output * call.output_tokens as f64)
        / 1_000_000.0;
    Some(MeteredPrice { usd, peak })
}

/// The accounting a metered session produces: the panel's four numbers, plus
/// whatever prices were computed for the first time and are owed to the cache.
#[derive(Debug, Default, PartialEq)]
pub struct MeteredTally {
    pub total: f64,
    pub peak: u32,
    pub off_peak: u32,
    pub unknown: u32,
    fresh: BTreeMap<String, MeteredPrice>,
}

/// The cache key for one call: its session and its position within it.
///
/// DeepSeek issues no id of its own for these calls — that absence is why they
/// are table-priced at all — so the key is synthesised from the two things the
/// durable log does guarantee.
fn metered_key(session_id: &str, seq: i64) -> String {
    format!("{session_id}:{seq}")
}

/// Sum what a fixed-price route charged, tier chosen per call.
///
/// A call already in `cached` keeps the price it was first given; only calls
/// absent from it are priced against today's table, and those come back in
/// [`MeteredTally::fresh`] to be written down. This is what makes the figure an
/// account of what a session cost rather than a running revaluation of it: the
/// rates and the peak schedule both move, and without this the panel silently
/// rewrote every past session each time they did.
///
/// Unknown models are deliberately *not* frozen. Nothing was priced, so there is
/// nothing to hold still — when a later build learns the model, that is a first
/// pricing, not a repricing.
pub fn price_metered(
    session_id: &str,
    calls: &[MeteredCall],
    cached: &BTreeMap<String, MeteredPrice>,
) -> MeteredTally {
    let mut tally = MeteredTally::default();
    for call in calls {
        // No session id and no seq are the same fact: this call has no stable
        // address, so it is priced live and never written down. An empty id
        // would pool unrelated sessions into one keyspace.
        let key = (!session_id.is_empty())
            .then(|| call.seq.map(|seq| metered_key(session_id, seq)))
            .flatten();
        // The frozen price wins outright: it is what this call was charged, and
        // no amount of movement in today's table changes that. Only when this
        // call has never been priced does the table get consulted at all.
        let price = match key.as_ref().and_then(|k| cached.get(k)) {
            Some(frozen) => *frozen,
            None => {
                let Some(price) = rate_price(call) else {
                    tally.unknown += 1;
                    continue;
                };
                if let Some(key) = key {
                    tally.fresh.insert(key, price);
                }
                price
            }
        };
        tally.total += price.usd;
        if price.peak {
            tally.peak += 1;
        } else {
            tally.off_peak += 1;
        }
    }
    tally
}

/// Pull the calls that a published table can price out of a session's events.
///
/// Only `deepseek-official`: its adapter emits no replay envelope and keeps no
/// response id on success (verified in dsh's own source — `llm-deepseek` has no
/// `replayState` at all, and the `x-request-id` it reads is attached to errors
/// only), so [`generation_refs`] cannot see these calls and a receipt lookup has
/// nothing to ask for. Its price list, unlike OpenRouter's open model space, is
/// fixed and public, so tokens × rate is exact rather than a guess.
pub fn metered_calls(events: &[Value]) -> Vec<MeteredCall> {
    let mut out = Vec::new();
    for event in events {
        if event.get("type").and_then(Value::as_str) != Some("assistant/message") {
            continue;
        }
        let Some(source) = event.pointer("/data/message/source") else {
            continue;
        };
        if source.get("kind").and_then(Value::as_str) != Some("model") {
            continue;
        }
        let provider = source
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if provider != DEEPSEEK_OFFICIAL {
            continue;
        }
        let usage = event.pointer("/data/usage");
        let tokens = |name: &str| -> u64 {
            usage
                .and_then(|u| u.get(name))
                .and_then(Value::as_u64)
                .unwrap_or(0)
        };
        out.push(MeteredCall {
            provider: provider.to_string(),
            model: source
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            seq: event.get("seq").and_then(Value::as_i64),
            at_ms: event.get("time").and_then(Value::as_i64).unwrap_or(0),
            input_tokens: tokens("inputTokens"),
            cache_read_tokens: tokens("cacheReadTokens"),
            output_tokens: tokens("outputTokens"),
        });
    }
    out
}

// ── Extraction ───────────────────────────────────────────────────────────────

/// Pull one [`GenerationRef`] per successful model call out of a session's
/// durable events (the array [`crate::dsh_source`] gets from `session.history`).
///
/// Pure and order-preserving so it can be tested against a recorded log. Ids
/// repeat across a retried step, so duplicates collapse to the first sighting —
/// paying twice for one generation would inflate the total.
pub fn generation_refs(events: &[Value]) -> Vec<GenerationRef> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for event in events {
        if event.get("type").and_then(Value::as_str) != Some("assistant/message") {
            continue;
        }
        let Some(source) = event.pointer("/data/message/source") else {
            continue;
        };
        if source.get("kind").and_then(Value::as_str) != Some("model") {
            continue;
        }
        // Two shapes, both live on one machine. dsh wrapped the adapter's
        // response record in a replay envelope — `{response, blocks?}`, typed in
        // `@deepseek-ai/dsh-llm`'s `ReplayEnvelope` — so what used to sit
        // directly on `replayState` now sits one level in. Sessions written
        // before the change keep the flat shape and must keep pricing, so both
        // are read; the envelope first, because that is what dsh writes now.
        let Some(id) = source
            .pointer("/replayState/response/responseId")
            .or_else(|| source.pointer("/replayState/responseId"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if !seen.insert(id.to_string()) {
            continue;
        }
        out.push(GenerationRef {
            id: id.to_string(),
            provider: source
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            model: source
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        });
    }
    out
}

// ── Credentials ──────────────────────────────────────────────────────────────

/// Resolve the OpenRouter key **the way dsh itself resolves it**, so a user who
/// already configured dsh does not configure Fleet a second time.
///
/// `settings.yaml` declares the provider under `llm-pi-ai.providers.openrouter`,
/// either as an inline `apiKey` or — the shape on a real install — as
/// `apiKeyEnv: OPENROUTER_API_KEY`, *naming the variable that holds the secret*.
/// That name is the lookup key everywhere else:
///
/// 1. the process environment, and
/// 2. `$DSH_HOME/.credentials.yaml`, which per `@deepseek-ai/dsh-credentials-local`
///    is "a YAML mapping of credential reference to value, and nothing else" —
///    **keyed by env-var name, not by provider name**.
///
/// Measured against a real `dsh web`: with `OPENROUTER_API_KEY: sk-…` in that
/// file, `credentials.describe` answers
/// `{configured: true, source: "file", writable: true}` with no env var set.
/// An `openrouter: sk-…` entry is *accepted* by the file (a valid POSIX
/// identifier, so the server still boots) but no provider ever looks that name
/// up — the key silently does nothing. This function used to read exactly that
/// wrong name, which is why it is now derived from `apiKeyEnv`.
///
/// The env layer wins over the file, matching dsh's own documented precedence
/// (a per-run `VAR=… dsh` is operator intent for that run).
///
/// Returns `None` when no key is configured, which is a normal state (the user
/// may not use OpenRouter at all), not an error.
pub fn openrouter_api_key() -> Option<String> {
    let dsh_home = crate::session::get_dsh_dir()?;

    let settings = read_yaml(&dsh_home.join("settings.yaml"));
    let provider = settings
        .as_ref()
        .and_then(|s| s.pointer("/llm-pi-ai/providers/openrouter").cloned())
        .unwrap_or(Value::Null);

    // The variable that holds the secret. `OPENROUTER_API_KEY` is the
    // conventional name and the one a bare install uses, but honour whatever
    // the user pointed `apiKeyEnv` at.
    let var = provider
        .get("apiKeyEnv")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_KEY_ENV);

    if let Some(key) = non_empty(std::env::var(var).ok()) {
        return Some(key);
    }
    if let Some(key) = non_empty(
        provider
            .get("apiKey")
            .and_then(Value::as_str)
            .map(str::to_string),
    ) {
        return Some(key);
    }

    // The managed credential store, keyed by that same env-var name. An empty
    // `{}` (the shipped default) yields nothing.
    let creds = read_yaml(&dsh_home.join(".credentials.yaml"))?;
    non_empty(
        creds
            .get(var)
            .and_then(Value::as_str)
            .map(str::to_string),
    )
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Read a YAML file into a `serde_json::Value`, or `None` when absent/unparseable.
fn read_yaml(path: &std::path::Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_yaml::from_str::<Value>(&text).ok()
}

// ── Cost cache ───────────────────────────────────────────────────────────────

fn cache_path() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join(CACHE_FILE))
}

#[derive(Serialize, Deserialize, Default)]
struct CostCache {
    /// generation id → cost in USD.
    #[serde(default)]
    costs: BTreeMap<String, f64>,
    /// `<session id>:<seq>` → the price that call was first given.
    ///
    /// A separate map rather than more keys in `costs`: these are Fleet's own
    /// synthesised keys, not the provider's ids, and the two must not be able to
    /// collide. `serde(default)` so a cache written before this existed loads.
    #[serde(default)]
    metered: BTreeMap<String, MeteredPrice>,
}

fn load_cache() -> CostCache {
    let Some(path) = cache_path() else {
        return CostCache::default();
    };
    match crate::atomic_json::load_preserving(&path) {
        crate::atomic_json::JsonLoad::Loaded(c) => c,
        _ => CostCache::default(),
    }
}

/// Merge both kinds of fresh entry into the on-disk cache under the
/// cross-process file lock, so a second Fleet process pricing another session
/// cannot drop these entries.
///
/// Receipts and metered prices are written together under one lock: a session
/// that mixes routes produces both, and two separate locked writes would leave
/// a window where only half of it had been recorded.
fn store_cache(fresh: &BTreeMap<String, f64>, fresh_metered: &BTreeMap<String, MeteredPrice>) {
    if fresh.is_empty() && fresh_metered.is_empty() {
        return;
    }
    let Some(path) = cache_path() else { return };
    crate::atomic_json::with_file_lock(&path, || {
        let mut cache: CostCache = match crate::atomic_json::load_preserving(&path) {
            crate::atomic_json::JsonLoad::Loaded(c) => c,
            crate::atomic_json::JsonLoad::Unreadable => return,
            _ => CostCache::default(),
        };
        cache.costs.extend(fresh.iter().map(|(k, v)| (k.clone(), *v)));
        cache
            .metered
            .extend(fresh_metered.iter().map(|(k, v)| (k.clone(), *v)));
        if let Ok(bytes) = serde_json::to_vec_pretty(&cache) {
            if let Err(e) = crate::atomic_json::write_atomic(&path, &bytes) {
                crate::log_debug(&format!("dsh cost cache: write: {e}"));
            }
        }
    });
}

// ── OpenRouter ───────────────────────────────────────────────────────────────

/// Parse `data.total_cost` (USD) out of a `/generation` answer.
///
/// Split from the HTTP call so the contract is testable without a key: the
/// endpoint's own shape is the thing most likely to drift.
fn parse_total_cost(body: &str) -> Result<f64, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad JSON: {e}"))?;
    v.pointer("/data/total_cost")
        .and_then(Value::as_f64)
        .ok_or_else(|| "no data.total_cost in response".to_string())
}

fn fetch_generation_cost(key: &str, id: &str) -> Result<f64, String> {
    // Off the async runtime for the same reason as `DshClient`: this runs
    // inside `get_dsh_session_cost`, a tauri `(async)` command on a tokio
    // worker, where reqwest::blocking panics and the panic is swallowed.
    crate::off_runtime::off_runtime(|| {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        let resp = client
            .get(GENERATION_ENDPOINT)
            .query(&[("id", id)])
            .bearer_auth(key)
            .send()
            .map_err(|e| format!("{id}: {e}"))?;
        let status = resp.status();
        let body = resp.text().map_err(|e| format!("{id} body: {e}"))?;
        if !status.is_success() {
            return Err(format!("{id}: HTTP {status}"));
        }
        parse_total_cost(&body)
    })?
}

// ── Public entry point ───────────────────────────────────────────────────────

/// Price a session's model calls, given the refs already extracted from its log.
///
/// Split from [`dsh_session_cost`] so the accounting — which calls are priced,
/// cached, skipped, or failed — is testable without any network. `price` stands
/// in for the OpenRouter call.
fn tally(
    refs: &[GenerationRef],
    cached: &BTreeMap<String, f64>,
    mut price: impl FnMut(&str) -> Result<f64, String>,
) -> (DshSessionCost, BTreeMap<String, f64>) {
    let mut fresh = BTreeMap::new();
    let mut total = 0.0f64;
    let mut priced = 0u32;
    let mut unpriced = 0u32;
    let mut unpriceable = 0u32;
    let mut first_error: Option<String> = None;

    for r in refs {
        if r.provider != OPENROUTER {
            unpriceable += 1;
            continue;
        }
        if let Some(cost) = cached.get(&r.id) {
            total += cost;
            priced += 1;
            continue;
        }
        match price(&r.id) {
            Ok(cost) => {
                fresh.insert(r.id.clone(), cost);
                total += cost;
                priced += 1;
            }
            Err(e) => {
                unpriced += 1;
                first_error.get_or_insert(e);
            }
        }
    }

    let mut notes = Vec::new();
    if unpriceable > 0 {
        notes.push(format!(
            "{unpriceable} call(s) went through a provider with no cost API and are not included"
        ));
    }
    if unpriced > 0 {
        let detail = first_error.unwrap_or_default();
        notes.push(format!("{unpriced} call(s) could not be priced: {detail}"));
    }

    (
        DshSessionCost {
            total_usd: (priced > 0).then_some(total),
            priced_calls: priced,
            // Everything `tally` prices came from a receipt lookup by definition.
            table_priced_calls: 0,
            unpriced_calls: unpriced,
            unpriceable_calls: unpriceable,
            note: notes.join("; "),
        },
        fresh,
    )
}

/// Real spend for a `dsh://` session URI.
///
/// Unlike [`crate::dsh_source::dsh_token_breakdown`] this needs the full
/// `session.history` — the generation ids live in the events, not in the
/// `session.list` projections — and it may go to the network, so the panel
/// fetches it separately from the (cheap, local) token counts rather than
/// making the token view wait on it.
pub fn dsh_session_cost(uri: &str) -> Result<DshSessionCost, String> {
    let events = crate::dsh_source::session_events(uri)?;
    let refs = generation_refs(&events);
    // Calls a published table can price. Disjoint from `refs` by construction —
    // one is keyed on a receipt id the fixed-price routes never emit, the other
    // on the provider name — so a session that mixes both is summed, not
    // double-charged.
    let metered = metered_calls(&events);
    if refs.is_empty() && metered.is_empty() {
        return Ok(DshSessionCost {
            note: "no model calls in this session yet".to_string(),
            ..Default::default()
        });
    }

    // The session's own id namespaces the metered keys, so two sessions cannot
    // collide on a `seq` they both happen to use. Absent (a URI shape this
    // module does not recognise) means nothing is cacheable — an empty id would
    // pool every such session into one keyspace.
    let session_id = crate::dsh_source::DshSource::session_id_of(uri).unwrap_or_default();
    let cache = load_cache();
    let table = price_metered(session_id, &metered, &cache.metered);
    // Freeze before either branch below can return. The no-OpenRouter-key path
    // exits early, and a user with no key is exactly the DeepSeek-only case this
    // cache exists for — writing only on the way past it would leave the most
    // common metered session revaluing itself forever.
    store_cache(&BTreeMap::new(), &table.fresh);
    let (table_total, peak_n, off_peak_n, unknown_model_n) =
        (table.total, table.peak, table.off_peak, table.unknown);

    let needs_openrouter = refs.iter().any(|r| r.provider == OPENROUTER);
    let key = openrouter_api_key();
    if needs_openrouter && key.is_none() {
        return Ok(DshSessionCost {
            unpriced_calls: refs.iter().filter(|r| r.provider == OPENROUTER).count() as u32,
            unpriceable_calls: refs.iter().filter(|r| r.provider != OPENROUTER).count() as u32,
            // The table-priced calls still count: a missing OpenRouter key says
            // nothing about a route that needs no key at all.
            total_usd: (peak_n + off_peak_n > 0).then_some(table_total),
            priced_calls: peak_n + off_peak_n,
            table_priced_calls: peak_n + off_peak_n,
            // Names the exact line to write. A desktop-launched dsh inherits the
            // app's environment, which has no shell profile in it, so the file is
            // the option that actually works there — and it is keyed by the
            // env-var name, not by "openrouter".
            note: "no OpenRouter API key configured — add a line \
                   `OPENROUTER_API_KEY: <key>` to ~/.dsh/.credentials.yaml \
                   (or export that variable before launching)"
                .to_string(),
        });
    }

    let (cost, fresh) = tally(&refs, &cache.costs, |id| match key.as_deref() {
        Some(key) => fetch_generation_cost(key, id),
        None => Err("no OpenRouter API key configured".to_string()),
    });
    store_cache(&fresh, &BTreeMap::new());
    Ok(merge_metered(cost, table_total, peak_n, off_peak_n, unknown_model_n))
}

/// Fold the table-priced calls into the receipt-priced result.
///
/// Kept separate from [`dsh_session_cost`] so the composition is unit-testable
/// without a live dsh or a network round trip.
fn merge_metered(
    receipts: DshSessionCost,
    table_total: f64,
    peak_n: u32,
    off_peak_n: u32,
    unknown_model_n: u32,
) -> DshSessionCost {
    let table_priced = peak_n + off_peak_n;
    if table_priced == 0 && unknown_model_n == 0 {
        return receipts;
    }
    let mut notes: Vec<String> = receipts
        .note
        .split("; ")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if table_priced > 0 {
        // Says where the figure came from, because it is a different kind of
        // number than the receipt total next to it: a published rate applied to
        // counted tokens, not what an invoice reported.
        notes.push(format!(
            "{table_priced} call(s) priced from DeepSeek's published rates \
             ({peak_n} peak / {off_peak_n} off-peak)"
        ));
    }
    if unknown_model_n > 0 {
        notes.push(format!(
            "{unknown_model_n} call(s) on a model with no published rate in this \
             build are not included"
        ));
    }
    let total = match receipts.total_usd {
        Some(receipt_total) => Some(receipt_total + table_total),
        None if table_priced > 0 => Some(table_total),
        None => None,
    };
    DshSessionCost {
        total_usd: total,
        priced_calls: receipts.priced_calls + table_priced,
        table_priced_calls: receipts.table_priced_calls + table_priced,
        unpriced_calls: receipts.unpriced_calls,
        unpriceable_calls: receipts.unpriceable_calls + unknown_model_n,
        note: notes.join("; "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Two model calls plus noise, shaped like the real log: the `assistant/chunk`
    /// finish frame carries the same id as its `assistant/message`, and a step can
    /// repeat an id.
    fn recorded_events() -> Vec<Value> {
        vec![
            json!({"type": "user/message", "data": {"message": {"role": "user"}}}),
            json!({"type": "assistant/chunk", "data": {"chunk": {
                "type": "finish",
                "replayState": {"provider": "openrouter", "responseId": "gen-1"}
            }}}),
            json!({"type": "assistant/message", "data": {"message": {
                "role": "assistant",
                "source": {
                    "kind": "model",
                    "provider": "openrouter",
                    "model": "anthropic/claude-haiku-4.5",
                    "replayState": {"responseId": "gen-1"}
                }
            }}}),
            // A duplicate of the same generation — must not be counted twice.
            json!({"type": "assistant/message", "data": {"message": {
                "source": {
                    "kind": "model",
                    "provider": "openrouter",
                    "model": "anthropic/claude-haiku-4.5",
                    "replayState": {"responseId": "gen-1"}
                }
            }}}),
            json!({"type": "assistant/message", "data": {"message": {
                "source": {
                    "kind": "model",
                    "provider": "deepseek-official",
                    "model": "deepseek-chat",
                    "replayState": {"responseId": "gen-2"}
                }
            }}}),
            // A Fleet-injected instruction message: durable, but not a model call.
            json!({"type": "user/message", "data": {"message": {
                "source": {"kind": "agent-instructions"}
            }}}),
        ]
    }

    #[test]
    fn extracts_one_ref_per_model_call_and_dedupes() {
        let refs = generation_refs(&recorded_events());
        assert_eq!(
            refs,
            vec![
                GenerationRef {
                    id: "gen-1".into(),
                    provider: "openrouter".into(),
                    model: "anthropic/claude-haiku-4.5".into(),
                },
                GenerationRef {
                    id: "gen-2".into(),
                    provider: "deepseek-official".into(),
                    model: "deepseek-chat".into(),
                },
            ],
            "one ref per distinct responseId, chunk frames and non-model sources ignored"
        );
    }

    /// One `assistant/message` source copied verbatim off the wire from a
    /// current dsh session (`session-edde1334-…`, read through
    /// `POST /api/session.history`).
    ///
    /// dsh wrapped the adapter's response record in a replay *envelope*: what
    /// used to sit directly on `replayState` now sits on `replayState.response`,
    /// alongside a `blocks` array. The installed package types it as
    /// `interface ReplayEnvelope { response: unknown; blocks?: readonly unknown[] }`
    /// (`@deepseek-ai/dsh-llm/lib/types/types.d.ts`), and the payload inside
    /// carries `version: 2` where the old flat one carried `version: 1`.
    fn v2_envelope_event() -> Value {
        json!({"type": "assistant/message", "data": {
            "turn": 1,
            "step": 1,
            "usage": {"inputTokens": 3, "outputTokens": 14, "cacheWriteTokens": 15069},
            "message": {
                "role": "assistant",
                "id": "msg-v2",
                "source": {
                    "kind": "model",
                    "provider": "openrouter",
                    "model": "anthropic/claude-haiku-4.5",
                    "replayState": {
                        "response": {
                            "kind": "pi-ai",
                            "version": 2,
                            "api": "openai-completions",
                            "provider": "openrouter",
                            "model": "anthropic/claude-haiku-4.5",
                            "responseId": "gen-1787026253-Te7maM1es7yqmeDdhUS6",
                            "stopReason": "stop"
                        },
                        "blocks": [{"type": "text"}]
                    }
                }
            }
        }})
    }

    /// The regression: every session dsh writes now uses the v2 envelope, so
    /// reading only the flat v1 path priced nothing and the panel said "no model
    /// calls" for all of them.
    ///
    /// Measured on a real install: of 64 sessions, 35 were v1 (priced fine — one
    /// summed to $0.0048613 through the live OpenRouter API) and 14 were v2 with
    /// a real `gen-…` id that Fleet could not see. The v2 ones were the newest,
    /// which is why every session the user opened looked unpriced.
    #[test]
    fn extracts_the_id_from_the_v2_replay_envelope() {
        let refs = generation_refs(&[v2_envelope_event()]);
        assert_eq!(
            refs,
            vec![GenerationRef {
                id: "gen-1787026253-Te7maM1es7yqmeDdhUS6".into(),
                provider: "openrouter".into(),
                model: "anthropic/claude-haiku-4.5".into(),
            }],
            "a current dsh session's generation id lives at \
             replayState.response.responseId"
        );
    }

    /// Both shapes coexist on one machine — a fix that only reads v2 would stop
    /// pricing the sessions that used to work.
    #[test]
    fn extracts_ids_from_both_replay_shapes_in_one_log() {
        let mut events = recorded_events();
        events.push(v2_envelope_event());
        let ids: Vec<String> = generation_refs(&events)
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(
            ids,
            vec!["gen-1", "gen-2", "gen-1787026253-Te7maM1es7yqmeDdhUS6"],
            "old flat sessions must keep pricing alongside the new envelope"
        );
    }

    /// One `deepseek-official` `assistant/message`, verbatim off the wire (the
    /// probe session run against dsh's built-in route). Note what is *not* here:
    /// no `replayState`, so no id — which is why these calls need a table rather
    /// than a receipt.
    fn official_event(at_ms: i64, model: &str, input: u64, cache_read: u64, output: u64) -> Value {
        json!({
            "type": "assistant/message",
            "seq": 20,
            "time": at_ms,
            "data": {
                "turn": 1,
                "step": 1,
                "usage": {
                    "inputTokens": input,
                    "outputTokens": output,
                    "cacheReadTokens": cache_read
                },
                "message": {
                    "role": "assistant",
                    "id": "msg-official",
                    "source": {
                        "kind": "model",
                        "provider": "deepseek-official",
                        "model": model
                    }
                }
            }
        })
    }

    /// The session id the metered tests price against, so their frozen keys are
    /// namespaced the same way the real entry point namespaces them.
    const TEST_SESSION: &str = "session-test";

    /// Price with a cold cache — what most of these tests want, since they are
    /// checking the published-rate arithmetic rather than the freeze.
    fn priced(calls: &[MeteredCall]) -> (f64, u32, u32, u32) {
        let t = price_metered(TEST_SESSION, calls, &BTreeMap::new());
        (t.total, t.peak, t.off_peak, t.unknown)
    }

    /// 2026-08-18 19:47:32 UTC, a Tuesday — the probe call's real timestamp.
    /// Off-peak.
    const OFF_PEAK_MS: i64 = 1_787_082_452_518;
    /// Same Tuesday, 03:00 UTC. Inside the 01:00–04:00 peak window.
    const PEAK_MS: i64 = 1_787_022_000_000;

    #[test]
    fn extracts_official_calls_with_their_tokens_and_timestamp() {
        let calls = metered_calls(&[
            official_event(OFF_PEAK_MS, "deepseek-v4-flash", 12938, 0, 1),
            // Another provider's call must not be swept in — it is priced by
            // receipt, and counting it here would double-charge the session.
            v2_envelope_event(),
        ]);
        assert_eq!(
            calls,
            vec![MeteredCall {
                provider: "deepseek-official".into(),
                model: "deepseek-v4-flash".into(),
                seq: Some(20),
                at_ms: OFF_PEAK_MS,
                input_tokens: 12938,
                cache_read_tokens: 0,
                output_tokens: 1,
            }],
            "only the fixed-price route's calls, with the event's own timestamp"
        );
    }

    #[test]
    fn prices_off_peak_at_half_the_peak_rate() {
        // 1M cache-miss input + 1M output on flash, so the arithmetic is the
        // published rate itself: peak 0.44 + 1.32 = 1.76, off-peak half of that.
        let call = |at_ms| MeteredCall {
            provider: "deepseek-official".into(),
            model: "deepseek-v4-flash".into(),
            seq: Some(20),
            at_ms,
            input_tokens: 1_000_000,
            cache_read_tokens: 0,
            output_tokens: 1_000_000,
        };
        let (peak_total, peak_n, off_n, unknown) = priced(&[call(PEAK_MS)]);
        assert!(
            (peak_total - 1.76).abs() < 1e-9,
            "peak flash 1M in + 1M out must be $1.76, got {peak_total}"
        );
        assert_eq!((peak_n, off_n, unknown), (1, 0, 0));

        let (off_total, peak_n, off_n, _) = priced(&[call(OFF_PEAK_MS)]);
        assert!(
            (off_total - 0.88).abs() < 1e-9,
            "off-peak is exactly half — $0.88 expected, got {off_total}"
        );
        assert_eq!((peak_n, off_n), (0, 1));
    }

    #[test]
    fn prices_the_three_token_classes_separately() {
        // Cache hits are an order of magnitude cheaper than misses, so folding
        // them together would overstate a cache-heavy session by ~30×.
        let (total, _, off_n, _) = priced(&[MeteredCall {
            provider: "deepseek-official".into(),
            model: "deepseek-v4-pro".into(),
            seq: Some(20),
            at_ms: OFF_PEAK_MS,
            input_tokens: 1_000_000,
            cache_read_tokens: 1_000_000,
            output_tokens: 1_000_000,
        }]);
        // Off-peak pro: 0.66 miss + 0.022 hit + 1.98 out.
        assert!(
            (total - 2.662).abs() < 1e-9,
            "expected 0.66 + 0.022 + 1.98 = $2.662, got {total}"
        );
        assert_eq!(off_n, 1);
    }

    #[test]
    fn an_unknown_model_is_counted_not_estimated() {
        let (total, peak_n, off_n, unknown) = priced(&[MeteredCall {
            provider: "deepseek-official".into(),
            model: "deepseek-v5-unreleased".into(),
            seq: Some(20),
            at_ms: OFF_PEAK_MS,
            input_tokens: 1_000_000,
            cache_read_tokens: 0,
            output_tokens: 1_000_000,
        }]);
        assert_eq!(
            (total, peak_n, off_n, unknown),
            (0.0, 0, 0, 1),
            "a model the table does not know must not be guessed at"
        );
    }

    /// The vision model is on the same published table at flash's rates. Leaving
    /// it out is safe (it lands in `unknown` rather than being guessed at) but
    /// under-reports every session that used it.
    #[test]
    fn the_vision_model_is_priced_at_flash_rates() {
        let (total, peak_n, off_n, unknown) = priced(&[MeteredCall {
            provider: "deepseek-official".into(),
            model: "deepseek-v4-flash-vision-exp".into(),
            seq: Some(20),
            at_ms: PEAK_MS,
            input_tokens: 1_000_000,
            cache_read_tokens: 1_000_000,
            output_tokens: 1_000_000,
        }]);
        // Peak flash: 0.44 miss + 0.014 hit + 1.32 out.
        assert!(
            (total - 1.774).abs() < 1e-9,
            "expected 0.44 + 0.014 + 1.32 = $1.774, got {total}"
        );
        assert_eq!((peak_n, off_n, unknown), (1, 0, 0));
    }

    /// The window boundaries themselves, since an off-by-one hour would misprice
    /// every call in a whole hour by 2×.
    #[test]
    fn peak_window_covers_exactly_0104_and_0610_utc() {
        let at = |hour: i64| hour * 3_600_000;
        for h in [1, 2, 3, 6, 7, 8, 9] {
            assert!(is_peak(at(h)), "{h:02}:00 UTC is peak");
        }
        for h in [0, 4, 5, 10, 11, 16, 23] {
            assert!(!is_peak(at(h)), "{h:02}:00 UTC is off-peak");
        }
    }

    /// The published window is Monday–Friday, so the same clock hour on a
    /// Saturday is off-peak. Reading the hour alone bills 14 hours a week at
    /// 2× the real rate, and miscounts the same calls in the peak/off-peak
    /// tally the panel prints.
    #[test]
    fn the_weekend_is_off_peak_at_every_hour() {
        // Saturday 2026-08-22 02:00 UTC — inside the 01:00–04:00 hour band.
        assert!(
            !is_peak(1_787_364_000_000),
            "Saturday 02:00 UTC is off-peak: the window is Mon–Fri"
        );
        // Saturday 2026-08-22 07:00 UTC — inside the 06:00–10:00 hour band.
        assert!(
            !is_peak(1_787_382_000_000),
            "Saturday 07:00 UTC is off-peak"
        );
        // Sunday 2026-08-23 09:00 UTC.
        assert!(!is_peak(1_787_475_600_000), "Sunday 09:00 UTC is off-peak");
        // The bracketing weekdays must stay peak, or the fix has over-reached:
        // Friday 2026-08-21 09:00 UTC and Monday 2026-08-24 01:00 UTC.
        assert!(is_peak(1_787_302_800_000), "Friday 09:00 UTC is peak");
        assert!(is_peak(1_787_533_200_000), "Monday 01:00 UTC is peak");
    }

    /// One flash call, 1M cache-miss input + 1M output, on Saturday
    /// 2026-08-22 02:00 UTC. Today's table prices it off-peak at $0.88; the
    /// same call was priced at peak, $1.76, before the Mon–Fri rule landed.
    fn weekend_call(seq: Option<i64>) -> MeteredCall {
        MeteredCall {
            provider: "deepseek-official".into(),
            model: "deepseek-v4-flash".into(),
            seq,
            at_ms: 1_787_364_000_000,
            input_tokens: 1_000_000,
            cache_read_tokens: 0,
            output_tokens: 1_000_000,
        }
    }

    /// The freeze contract. A call priced once keeps that price when the rates
    /// or the peak schedule move under it.
    ///
    /// Without this the panel is not reporting what a session cost — it is
    /// revaluing it at today's prices every time it is opened, which is exactly
    /// what the Mon–Fri weekend fix silently did to every past weekend session
    /// the moment it landed. The receipt half of this module has always behaved
    /// this way (an invoice does not change); this is the table half catching up.
    #[test]
    fn a_priced_call_keeps_the_price_it_was_first_given() {
        let cached = BTreeMap::from([(
            metered_key(TEST_SESSION, 20),
            MeteredPrice {
                usd: 1.76,
                peak: true,
            },
        )]);
        let t = price_metered(TEST_SESSION, &[weekend_call(Some(20))], &cached);
        assert!(
            (t.total - 1.76).abs() < 1e-9,
            "the frozen price must survive a rule change: expected $1.76, got \
             {} (today's table would say $0.88)",
            t.total
        );
        assert_eq!(
            (t.peak, t.off_peak),
            (1, 0),
            "the tier freezes with the money — recomputing it would let the \
             panel's peak/off-peak line contradict the total beside it"
        );
        assert!(
            t.fresh.is_empty(),
            "nothing was priced anew, so nothing is owed to the cache"
        );
    }

    /// A first pricing must come back out to be written down, or the freeze
    /// never takes hold and every visit re-prices from scratch.
    #[test]
    fn a_first_pricing_is_handed_back_to_be_written_down() {
        let t = price_metered(TEST_SESSION, &[weekend_call(Some(20))], &BTreeMap::new());
        assert!((t.total - 0.88).abs() < 1e-9, "priced at today's table");
        assert_eq!(
            t.fresh.get(&metered_key(TEST_SESSION, 20)),
            Some(&MeteredPrice {
                usd: 0.88,
                peak: false
            }),
            "the computed price is owed to the cache under its session:seq key"
        );
    }

    /// A call with no `seq` has no stable address, so it is priced live and
    /// never written down — a cache entry nothing could ever look up again is
    /// worse than none, since the next session's `seq` would collide with it.
    #[test]
    fn a_call_with_no_seq_is_priced_but_not_cached() {
        let t = price_metered(TEST_SESSION, &[weekend_call(None)], &BTreeMap::new());
        assert!((t.total - 0.88).abs() < 1e-9);
        assert_eq!(t.off_peak, 1);
        assert!(t.fresh.is_empty(), "unaddressable, so uncacheable");
    }

    /// The same guard for the other half of the key: an unrecognised URI yields
    /// no session id, and an empty one would pool unrelated sessions together.
    #[test]
    fn an_empty_session_id_caches_nothing() {
        let t = price_metered("", &[weekend_call(Some(20))], &BTreeMap::new());
        assert!((t.total - 0.88).abs() < 1e-9);
        assert!(t.fresh.is_empty(), "no session id, no keyspace");
    }

    /// A weekend call must be charged the off-peak half, not just counted as
    /// off-peak — the tally and the dollar figure come off the same flag.
    #[test]
    fn a_weekend_call_is_billed_at_the_off_peak_half() {
        let (total, peak_n, off_n, _) = priced(&[MeteredCall {
            provider: "deepseek-official".into(),
            model: "deepseek-v4-flash".into(),
            seq: Some(20),
            // Saturday 2026-08-22 02:00 UTC.
            at_ms: 1_787_364_000_000,
            input_tokens: 1_000_000,
            cache_read_tokens: 0,
            output_tokens: 1_000_000,
        }]);
        assert!(
            (total - 0.88).abs() < 1e-9,
            "weekend flash 1M in + 1M out is off-peak $0.88, got {total}"
        );
        assert_eq!((peak_n, off_n), (0, 1));
    }

    /// A session that used both routes must have its two totals added, each
    /// counted once — the failure this guards is a double charge or a lost half.
    #[test]
    fn merges_receipt_and_table_totals_without_double_counting() {
        let receipts = DshSessionCost {
            total_usd: Some(0.05),
            priced_calls: 2,
            table_priced_calls: 0,
            unpriced_calls: 1,
            unpriceable_calls: 0,
            note: "1 call(s) could not be priced: 404".to_string(),
        };
        let merged = merge_metered(receipts, 0.25, 1, 2, 0);
        assert_eq!(merged.total_usd, Some(0.30), "0.05 receipt + 0.25 table");
        assert_eq!(merged.priced_calls, 5, "2 by receipt + 3 by table");
        assert_eq!(merged.unpriced_calls, 1, "the receipt gap is untouched");
        assert!(
            merged.note.contains("could not be priced")
                && merged.note.contains("1 peak / 2 off-peak"),
            "both stories must survive the merge: {}",
            merged.note
        );
    }

    /// The common case for a DeepSeek-official-only session: there is no receipt
    /// total at all, so the table total must become the total rather than being
    /// dropped next to a `None`.
    #[test]
    fn a_table_only_session_gets_a_total() {
        let merged = merge_metered(DshSessionCost::default(), 0.017, 0, 1, 0);
        assert_eq!(merged.total_usd, Some(0.017));
        assert_eq!(merged.priced_calls, 1);
        assert!(merged.note.contains("published rates"), "{}", merged.note);
    }

    /// Nothing to fold in must leave the receipt result byte-identical, so the
    /// OpenRouter-only path cannot be perturbed by this feature.
    #[test]
    fn merging_nothing_leaves_the_receipt_result_alone() {
        let receipts = DshSessionCost {
            total_usd: Some(0.05),
            priced_calls: 2,
            table_priced_calls: 0,
            unpriced_calls: 0,
            unpriceable_calls: 1,
            note: "1 call(s) went through a provider with no cost API".to_string(),
        };
        assert_eq!(merge_metered(receipts.clone(), 0.0, 0, 0, 0), receipts);
    }

    #[test]
    fn extraction_tolerates_a_log_with_nothing_to_price() {
        assert!(generation_refs(&[]).is_empty());
        assert!(generation_refs(&[json!({"type": "assistant/message", "data": {}})]).is_empty());
        assert!(
            generation_refs(&[json!({"type": "assistant/message", "data": {"message": {
                "source": {"kind": "model", "provider": "openrouter", "replayState": {}}
            }}})])
            .is_empty(),
            "a model message with no responseId is not billable to anything"
        );
    }

    #[test]
    fn total_cost_is_read_from_the_documented_field_in_usd() {
        let body = r#"{"data":{"id":"gen-1","total_cost":0.00123,"tokens_prompt":10}}"#;
        assert_eq!(parse_total_cost(body).unwrap(), 0.00123);
    }

    #[test]
    fn a_response_without_total_cost_is_an_error_not_a_zero() {
        // Pricing a call at $0 because the field moved would silently under-report
        // the session — the whole failure this module exists to avoid.
        assert!(parse_total_cost(r#"{"data":{"id":"gen-1"}}"#).is_err());
        assert!(parse_total_cost("not json").is_err());
    }

    #[test]
    fn tally_sums_priced_calls_and_reports_the_rest() {
        let refs = generation_refs(&recorded_events());
        let (cost, fresh) = tally(&refs, &BTreeMap::new(), |id| {
            assert_eq!(id, "gen-1", "only the openrouter call is priced");
            Ok(0.25)
        });
        assert_eq!(cost.total_usd, Some(0.25));
        assert_eq!(cost.priced_calls, 1);
        assert_eq!(cost.unpriceable_calls, 1, "the deepseek call is counted, not guessed");
        assert_eq!(cost.unpriced_calls, 0);
        assert!(
            cost.note.contains("no cost API"),
            "the excluded call must be visible in the note: {}",
            cost.note
        );
        assert_eq!(fresh.get("gen-1"), Some(&0.25), "the new price is cached");
    }

    #[test]
    fn tally_reuses_the_cache_and_never_refetches() {
        let refs = generation_refs(&recorded_events());
        let cached = BTreeMap::from([("gen-1".to_string(), 0.5)]);
        let (cost, fresh) = tally(&refs, &cached, |id| {
            panic!("must not fetch a cached id: {id}");
        });
        assert_eq!(cost.total_usd, Some(0.5));
        assert!(fresh.is_empty(), "nothing new to write back");
    }

    #[test]
    fn tally_surfaces_a_failed_lookup_instead_of_dropping_it() {
        let refs = vec![GenerationRef {
            id: "gen-x".into(),
            provider: "openrouter".into(),
            model: "m".into(),
        }];
        let (cost, fresh) = tally(&refs, &BTreeMap::new(), |_| Err("HTTP 404".into()));
        assert_eq!(cost.total_usd, None, "nothing priced → no total, not $0");
        assert_eq!(cost.unpriced_calls, 1);
        assert!(cost.note.contains("HTTP 404"), "note: {}", cost.note);
        assert!(fresh.is_empty());
    }

    // --- key resolution, against a temp DSH_HOME ---

    fn with_temp_dsh_home<T>(f: impl FnOnce(&PathBuf) -> T) -> T {
        let _guard = crate::session::fleet_home_lock();
        let base = std::env::temp_dir().join(format!("fleet-dsh-cost-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let prev = std::env::var_os("DSH_HOME");
        std::env::set_var("DSH_HOME", &base);
        let out = f(&base);
        match prev {
            Some(v) => std::env::set_var("DSH_HOME", v),
            None => std::env::remove_var("DSH_HOME"),
        }
        let _ = std::fs::remove_dir_all(&base);
        out
    }

    #[test]
    fn key_comes_from_the_env_var_settings_yaml_names() {
        with_temp_dsh_home(|base| {
            // The shape on a real install.
            std::fs::write(
                base.join("settings.yaml"),
                "llm-pi-ai:\n  providers:\n    openrouter:\n      apiKeyEnv: FLEET_TEST_OR_KEY\n",
            )
            .unwrap();
            std::env::remove_var("FLEET_TEST_OR_KEY");
            assert_eq!(openrouter_api_key(), None, "named var unset → no key");

            std::env::set_var("FLEET_TEST_OR_KEY", "sk-or-test");
            assert_eq!(openrouter_api_key().as_deref(), Some("sk-or-test"));
            std::env::remove_var("FLEET_TEST_OR_KEY");
        });
    }

    /// The credential store is keyed by **environment-variable name**, not by
    /// provider name. Measured against a real `dsh web`: with
    /// `OPENROUTER_API_KEY: sk-…` in `$DSH_HOME/.credentials.yaml`,
    /// `credentials.describe` reports `{configured: true, source: "file"}`.
    /// A `openrouter: sk-…` entry is accepted by the file (it is a valid POSIX
    /// identifier) but nothing ever looks it up — measured too: the server still
    /// boots, the key is simply never used.
    #[test]
    fn key_comes_from_the_credentials_file_under_its_env_var_name() {
        with_temp_dsh_home(|base| {
            std::fs::write(
                base.join("settings.yaml"),
                "llm-pi-ai:\n  providers:\n    openrouter:\n      apiKeyEnv: FLEET_TEST_CRED_VAR\n",
            )
            .unwrap();
            std::env::remove_var("FLEET_TEST_CRED_VAR");

            // The provider-named form dsh never reads must NOT be picked up.
            std::fs::write(base.join(".credentials.yaml"), "openrouter: wrong-form\n").unwrap();
            assert_eq!(
                openrouter_api_key(),
                None,
                "a provider-named entry is not a credential dsh would use"
            );

            // The real form: the env-var name settings.yaml points at.
            std::fs::write(
                base.join(".credentials.yaml"),
                "FLEET_TEST_CRED_VAR: sk-from-file\n",
            )
            .unwrap();
            assert_eq!(openrouter_api_key().as_deref(), Some("sk-from-file"));
        });
    }

    #[test]
    fn key_falls_back_to_inline_settings_then_credentials() {
        with_temp_dsh_home(|base| {
            std::fs::write(
                base.join("settings.yaml"),
                "llm-pi-ai:\n  providers:\n    openrouter:\n      apiKey: inline-key\n",
            )
            .unwrap();
            assert_eq!(openrouter_api_key().as_deref(), Some("inline-key"));

            // No provider block at all: the lookup falls back to the
            // conventional env-var name, which is also the credential-file key.
            std::fs::write(base.join("settings.yaml"), "llm-pi-ai: {}\n").unwrap();
            std::fs::write(
                base.join(".credentials.yaml"),
                "OPENROUTER_API_KEY: cred-key\n",
            )
            .unwrap();
            assert_eq!(openrouter_api_key().as_deref(), Some("cred-key"));
        });
    }

    #[test]
    fn an_unconfigured_install_yields_no_key_rather_than_an_error() {
        with_temp_dsh_home(|base| {
            // Exactly what ships: an empty credentials map, no provider settings.
            std::fs::write(base.join(".credentials.yaml"), "{}\n").unwrap();
            assert_eq!(openrouter_api_key(), None);
        });
    }
}
