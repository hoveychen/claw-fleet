//! "Today's cumulative usage" aggregation for the desktop nav-bar / mobile
//! header counter.
//!
//! Attribution口径 (revised by Boss 2026-08-27): "today" = every **turn**
//! whose own `timestamp` falls in today's local day, whenever its session
//! started. Agent spend only — Fleet's own LLM overhead is excluded (see the
//! note inside [`build_today_usage_cached`]).
//!
//! The original口径 (Boss 2026-07-12) was "sessions **created** today", summing
//! each session's live `SessionInfo.total_cost_usd`. It was replaced because a
//! session that outlived midnight — every handoff chain, every long-running
//! agent — had its post-midnight spend attributed nowhere: the badge showed $37
//! on 2026-08-27 against $225 actually spent, and the receipt's own "近 7 天"
//! view (which already folded per turn) disagreed with its "今天" page by 7.7×.
//!
//! Both the badge ([`today_usage`]) and the receipt
//! ([`today_usage_breakdown`]) are now derived from the same per-turn
//! projection as the arbitrary-range view, so all three agree by construction
//! and `Σ line.cost_usd == TodayUsage.cost_usd` still holds to the cent.
//!
//! No new pricing math lives here: per-turn cost comes from
//! `model_cost::turn_cost_usd`, the same function `session::StatsAcc` uses.

use serde::{Deserialize, Serialize};

use crate::session::SessionInfo;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TodayUsage {
    /// YYYY-MM-DD in the user's local timezone.
    pub date: String,
    /// Input tokens across today's turns (input + cache creation + cache read,
    /// cache re-reads included) — the "tokens sent to the API" total, on the same
    /// 口径 as cost. **Agent sessions only** — Fleet's own LLM calls are excluded
    /// (see [`today_usage`]). NOTE: this is cache-read-dominated and can reach
    /// billions/day — it is NOT the daily report's old last-turn snapshot (the
    /// report sums cumulatively too, so both agree on口径, though the sidebar also
    /// counts Codex which the Claude-only report does not).
    pub input_tokens: u64,
    /// Output tokens across today's turns.
    pub output_tokens: u64,
    /// Total USD cost today — equal to `agent_cost_usd`, since Fleet's own
    /// overhead is excluded from this surface.
    pub cost_usd: f64,
    /// Cost of today's agent (Claude Code / Codex) turns.
    pub agent_cost_usd: f64,
    /// **Always 0.0.** Fleet's own LLM spend is no longer folded into this
    /// surface; the field is retained only so mobile clients that render an
    /// "agent + fleet" split don't read `undefined`. The real per-scenario figures
    /// live in [`crate::llm_usage::list_usage_daily_buckets`], which powers
    /// Settings → Usage.
    pub fleet_cost_usd: f64,
    /// Number of top-level (non-subagent) sessions that spent tokens today,
    /// whenever they started.
    pub session_count: u64,
}

/// Local-timezone [start, end] of the day containing `now_ms`, plus the
/// `YYYY-MM-DD` label. `end` is the last millisecond of the day (inclusive),
/// matching `llm_usage::list_usage_daily_buckets`' inclusive window.
fn day_bounds_ms(now_ms: i64) -> (i64, i64, String) {
    use chrono::{Local, TimeZone};

    let now = chrono::DateTime::from_timestamp_millis(now_ms)
        .unwrap_or_else(chrono::Utc::now)
        .with_timezone(&Local);
    let date = now.date_naive();
    // Local midnight; `.earliest()` handles the DST spring-forward gap by
    // picking the first valid instant.
    let start = date
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).earliest())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(now_ms);
    let end = start + 86_400_000 - 1;
    (start, end, date.format("%Y-%m-%d").to_string())
}

/// Count the top-level (non-subagent) sessions that actually spent tokens on
/// `date`. This is the badge's "N sessions" figure, on the same per-turn口径 as
/// the cost beside it: a session counts on every day it burned tokens, not only
/// on the day its transcript was born.
fn count_sessions_active_on(
    sessions: &[SessionInfo],
    date: &str,
    cache: &mut UsageBreakdownCache,
) -> u64 {
    let mut count = 0u64;
    for s in sessions {
        if s.is_subagent {
            continue;
        }
        // Cells were already folded by the breakdown pass above, so this is a
        // cache hit for every session; no transcript is re-read here.
        if cache.cells(s).keys().any(|(d, _)| d == date) {
            count += 1;
        }
    }
    count
}

/// Aggregate today's cumulative usage from an already-scanned session list.
///
/// `sessions` is whatever the caller already has (e.g. `scan_all_sources`), so
/// this adds no extra JSONL scan. Agent spend only — see the note in the body for
/// why Fleet's own LLM overhead is excluded.
pub fn today_usage(sessions: &[SessionInfo]) -> TodayUsage {
    let now_ms = chrono::Local::now().timestamp_millis();
    let mut cache = usage_cache().lock().unwrap();
    cache.retain_sessions(&live_ids(sessions));
    let out = build_today_usage_cached(sessions, now_ms, &mut cache);
    persist_cache(&mut cache);
    out
}

/// Test-only wrapper: a fresh (empty) cache so each call folds from disk, keeping
/// the badge tests hermetic (mirrors [`build_breakdown`]).
#[cfg(test)]
fn build_today_usage(sessions: &[SessionInfo], now_ms: i64) -> TodayUsage {
    build_today_usage_cached(sessions, now_ms, &mut UsageBreakdownCache::default())
}

/// Pure core of [`today_usage`], with `now_ms` and the projection cache injected
/// so the badge口径 is unit-testable without a global cache or a wall clock.
///
/// The badge is derived from the very receipt it opens
/// ([`build_breakdown_cached`]) rather than from a parallel fold, so
/// `Σ line.cost_usd == TodayUsage.cost_usd` holds by construction. It used to
/// sum each session's live `SessionInfo.total_cost_usd` for sessions *created*
/// today; that made a session which outlived midnight invisible to the badge for
/// the rest of its life (see
/// `sidebar_badge_reconciles_with_the_receipt_across_midnight`).
fn build_today_usage_cached(
    sessions: &[SessionInfo],
    now_ms: i64,
    cache: &mut UsageBreakdownCache,
) -> TodayUsage {
    let b = build_breakdown_cached(sessions, now_ms, cache);
    let session_count = count_sessions_active_on(sessions, &b.date, cache);

    let agent_cost_usd = b.total_cost_usd;
    // `TodayUsage.input_tokens` is the cache-inclusive "tokens sent to the API"
    // figure, so it takes all three input-side rows the receipt itemises apart.
    let input_tokens = b
        .total_input_tokens
        .saturating_add(b.total_cache_creation_tokens)
        .saturating_add(b.total_cache_read_tokens);
    let output_tokens = b.total_output_tokens;
    let date = b.date;

// Fleet's own LLM calls (guard analysis, audit-rule suggestions, daily-report
// summaries, session outcome analysis, mascot quips) are deliberately NOT part of
// this figure. They are Fleet's operational overhead rather than the user's agent
// spend, and their accounting cannot be made to reconcile with the receipt's
// per-row itemisation: entries logged before TTL-aware accounting recorded the
// CLI's last-iteration `usage.input_tokens` against a fully-billed `cost_usd`,
// and Codex-provider calls are logged unpriced (`cost_usd: 0.0`,
// `cost_accurate: false`) with char-estimated tokens. Rather than show a line
// that visibly fails `Σ rows == subtotal`, this surface is agent-only. Fleet's
// own consumption stays visible in Settings → Usage, which renders the raw
// `llm_usage::list_usage_daily_buckets` trend directly.

    TodayUsage {
        date,
        input_tokens,
        output_tokens,
        cost_usd: agent_cost_usd,
        agent_cost_usd,
        // Retained at 0.0 rather than removed: the field crosses the wire to
        // mobile clients (incl. the out-of-tree HarmonyOS app) that would show
        // `undefined` if it vanished. Consumers already guard on `> 0`.
        fleet_cost_usd: 0.0,
        session_count,
    }
}

/// Consolidated per-container token usage for the Fleet Cloud lean deployment.
///
/// One customer per container, so this container's usage **is** the customer's
/// usage. `today` reuses [`today_usage`] (today's agent turns); the
/// `cumulative_*` fields sum **agent sessions only**
/// (claude/codex consumption — the token basis you'd bill on) across every
/// session currently retained on disk.
///
/// Metering only. Billing and quota enforcement are deliberately out of v1, and
/// the cumulative figure is subject to session retention (pruned sessions drop
/// out), so it is a *current usage* view, not an authoritative billing ledger.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CloudUsage {
    /// Today's window (agent sessions only), same shape as `/today_usage`.
    pub today: TodayUsage,
    /// Cumulative input tokens across all retained agent sessions.
    pub cumulative_input_tokens: u64,
    /// Cumulative output tokens across all retained agent sessions.
    pub cumulative_output_tokens: u64,
    /// Cumulative agent cost (USD) across all retained agent sessions.
    pub cumulative_agent_cost_usd: f64,
    /// Number of top-level (non-subagent) sessions counted in the cumulative sum.
    pub cumulative_session_count: u64,
}

/// Build the consolidated cloud usage view from an already-scanned session list
/// (no extra JSONL scan). Pure — driven directly by the unit test.
pub fn cloud_usage(sessions: &[SessionInfo]) -> CloudUsage {
    let today = today_usage(sessions);
    let mut input = 0u64;
    let mut output = 0u64;
    let mut cost = 0.0;
    let mut count = 0u64;
    for s in sessions {
        input = input.saturating_add(s.total_input_tokens);
        output = output.saturating_add(s.total_output_tokens);
        cost += s.total_cost_usd;
        if !s.is_subagent {
            count += 1;
        }
    }
    CloudUsage {
        today,
        cumulative_input_tokens: input,
        cumulative_output_tokens: output,
        cumulative_agent_cost_usd: cost,
        cumulative_session_count: count,
    }
}

// ── Per-model receipt breakdown ──────────────────────────────────────────────
//
// The sidebar badge shows a single `$X · Y tok` figure. Clicking it opens a
// "receipt" that itemises that same figure per model: how many input /
// cache-write / cache-read / output tokens each model consumed today, the
// model's official unit prices ($/Mtok), and the line cost. The receipt total
// is built on the **exact same口径** as [`today_usage`] — sessions **created
// today** (every SessionInfo, subagents included, each counted once), agent
// spend only — so `Σ line.cost_usd == TodayUsage.cost_usd` to the cent. Because per-model pricing is linear, folding a model's tokens and
// pricing once equals summing each turn's cost; we still accumulate per-turn
// cost directly so the total reconciles with `SessionInfo.total_cost_usd`
// (which `StatsAcc` folds the same way) regardless of mid-session model swaps.

/// One receipt line: all of one model's usage today, under one agent source.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ModelReceiptLine {
    /// Raw model id as it appears in the transcript (e.g. `claude-opus-4-8`,
    /// `gpt-5.6-sol`). The frontend prettifies for display.
    pub model: String,
    /// Agent source that ran this model: `claude-code`, `codex`, or `fleet`
    /// (Fleet's own guard / audit / report-summary LLM calls).
    pub source: String,
    /// Net input tokens — cache writes and reads are itemised separately below,
    /// so this must NOT be the all-inclusive "tokens sent to the API" figure
    /// (see `fold_report_model`, which nets the report DB's inclusive count).
    pub input_tokens: u64,
    /// Cache-write tokens at the 5-minute TTL, priced at `cache_write_price`.
    pub cache_creation_tokens: u64,
    /// Cache-write tokens at the 1-hour TTL, priced at `cache_write_1h_price`
    /// (2× input, vs 1.25× for 5-minute writes). Split out rather than blended
    /// so `Σ (tokens × unit price) == cost_usd` still holds for the UI.
    pub cache_creation_1h_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
    /// Official unit prices in USD per 1M tokens, for the receipt's "@ $X/M"
    /// column. Sourced from [`crate::model_cost::get_model_costs`].
    pub input_price: f64,
    pub output_price: f64,
    pub cache_write_price: f64,
    pub cache_write_1h_price: f64,
    pub cache_read_price: f64,
    /// Line cost = Σ per-turn `turn_cost_usd` for this (source, model). Equals
    /// the sum of this line's itemised rows.
    pub cost_usd: f64,
}

/// Today's per-model receipt — the itemised breakdown behind the sidebar badge.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TodayUsageBreakdown {
    /// YYYY-MM-DD in the user's local timezone (same as [`TodayUsage::date`]).
    pub date: String,
    /// One line per (source, model), sorted by descending cost.
    pub lines: Vec<ModelReceiptLine>,
    pub total_input_tokens: u64,
    pub total_cache_creation_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_output_tokens: u64,
    /// Grand total = `agent_cost_usd + fleet_cost_usd`; equals
    /// [`TodayUsage::cost_usd`] to the cent.
    pub total_cost_usd: f64,
    pub agent_cost_usd: f64,
    pub fleet_cost_usd: f64,
}

/// Mutable accumulator for one receipt line while folding turns.
#[derive(Default)]
struct LineAcc {
    input: u64,
    cache_creation: u64,
    /// The 1-hour-TTL subset of `cache_creation`, billed at 2× input instead of
    /// 1.25×. Tracked separately so the receipt can itemise the two write rates
    /// and still have `Σ rows == cost`.
    cache_creation_1h: u64,
    cache_read: u64,
    output: u64,
    cost: f64,
}

impl LineAcc {
    /// Fold one turn's tokens + cost into this accumulator. `cache_creation_1h`
    /// is the 1-hour-TTL *subset* of `cache_creation`, never additive on top.
    fn add(
        &mut self,
        input: u64,
        cache_creation: u64,
        cache_creation_1h: u64,
        cache_read: u64,
        output: u64,
        cost: f64,
    ) {
        self.input += input;
        self.cache_creation += cache_creation;
        self.cache_creation_1h += cache_creation_1h;
        self.cache_read += cache_read;
        self.output += output;
        self.cost += cost;
    }
}

/// Fold one codex session's cumulative token usage into `by_model` under the
/// `("codex", model)` key. Unlike Claude's per-turn deltas, codex reports a
/// single cumulative `total_token_usage` snapshot per session, so this
/// contributes one folded figure: full-price input (`raw − cached`), cached
/// input billed at the cache-read rate, and output (already incl. reasoning).
///
/// `uri` is the session's stored `jsonl_path`, which for codex is a `codex://`
/// URI (not a filesystem path) that may point at a zstd-compressed rollout — so
/// this delegates to [`crate::codex_source::codex_token_breakdown`], which
/// resolves + decompresses + parses it, rather than reading the path directly.
/// That reuse also keeps the cost here in lock-step with the per-session
/// `CodexTokenPanel`.
#[cfg(test)]
fn fold_codex_session(
    uri: &str,
    by_model: &mut std::collections::HashMap<(String, String), LineAcc>,
) {
    let Ok(bd) = crate::codex_source::codex_token_breakdown(uri) else {
        return;
    };
    if bd.total_tokens == 0 {
        return;
    }
    // `bd.input_tokens` is already the full-price portion (cached excluded);
    // codex has no cache-write bucket so `cache_creation` stays 0. `bd.cost_usd`
    // is the canonical cost from the same price tier the panel uses.
    let key_model = bd.model.clone().unwrap_or_else(|| "gpt".to_string());
    by_model
        .entry(("codex".to_string(), key_model))
        .or_default()
        .add(bd.input_tokens, 0, 0, bd.cached_input_tokens, bd.output_tokens, bd.cost_usd);
}

/// Build today's per-model receipt on the same口径 as [`today_usage`].
///
/// `sessions` is the already-scanned session list (subagents included). Every
/// finalized turn **timestamped today** contributes, whenever its session
/// started, folded per model. Fleet's own LLM spend is excluded (agent-only
/// surface). Lines are returned sorted by descending cost.
pub fn today_usage_breakdown(sessions: &[SessionInfo]) -> TodayUsageBreakdown {
    let now_ms = chrono::Local::now().timestamp_millis();
    let mut cache = usage_cache().lock().unwrap();
    cache.retain_sessions(&live_ids(sessions));
    let out = build_breakdown_cached(sessions, now_ms, &mut cache);
    persist_cache(&mut cache);
    out
}

/// Test-only wrapper: a fresh (empty) cache so each call folds from disk, keeping
/// the receipt tests hermetic and independent of the process-wide cache.
#[cfg(test)]
fn build_breakdown(sessions: &[SessionInfo], now_ms: i64) -> TodayUsageBreakdown {
    build_breakdown_cached(sessions, now_ms, &mut UsageBreakdownCache::default())
}

/// Pure core of [`today_usage_breakdown`], with `now_ms` and the projection cache
/// injected so it is unit-testable without a wall clock or the global cache.
///
/// Today is just the `[local midnight, now]` window of
/// [`build_range_breakdown_cached`], so this delegates rather than re-folding:
/// one attribution rule, one code path. A session's turns land on the day each
/// turn's own `timestamp` names — **not** the day its transcript was born. The
/// birth-day rule this replaced silently dropped every session that outlived
/// midnight: on 2026-08-27 the receipt read $37 against $225 actually spent,
/// because six sessions born 08-26 were still running (see
/// `counts_today_turns_of_a_session_created_yesterday`).
///
/// The range fold's report-DB backfill is a no-op here: its `from_date` equals
/// the live floor for a same-day window, so nothing is read from the report DB.
fn build_breakdown_cached(
    sessions: &[SessionInfo],
    now_ms: i64,
    cache: &mut UsageBreakdownCache,
) -> TodayUsageBreakdown {
    let (day_start_ms, _day_end_ms, date) = day_bounds_ms(now_ms);
    let r = build_range_breakdown_cached(sessions, day_start_ms, now_ms, cache);

    TodayUsageBreakdown {
        // The range header reports the earliest day it found data for, which is
        // empty-window-dependent; today's label is always today.
        date,
        lines: r.lines,
        total_input_tokens: r.total_input_tokens,
        total_cache_creation_tokens: r.total_cache_creation_tokens,
        total_cache_read_tokens: r.total_cache_read_tokens,
        total_output_tokens: r.total_output_tokens,
        total_cost_usd: r.total_cost_usd,
        agent_cost_usd: r.agent_cost_usd,
        // Always 0: Fleet's own overhead is excluded from this receipt (see the
        // note in `today_usage`). Kept on the wire so mobile clients that read
        // the split don't see `undefined`.
        fleet_cost_usd: 0.0,
    }
}

/// Map a `(source, model) → LineAcc` fold into receipt lines, priced via
/// [`crate::model_cost::get_model_costs`] and sorted by descending cost (then
/// model name for a stable order among equal costs). Shared by the today and
/// range breakdowns so both surfaces price and order lines identically.
fn build_lines(
    by_model: std::collections::HashMap<(String, String), LineAcc>,
) -> Vec<ModelReceiptLine> {
    use crate::model_cost::get_model_costs;
    let mut lines: Vec<ModelReceiptLine> = by_model
        .into_iter()
        .map(|((source, model), acc)| {
            let c = get_model_costs(&model);
            // `acc.cache_creation` is the total; the 1h figure is its subset, so
            // the 5-minute row is the remainder. saturating_sub keeps a
            // malformed pair from wrapping the row into nonsense.
            let write_1h = acc.cache_creation_1h.min(acc.cache_creation);
            ModelReceiptLine {
                model,
                source,
                input_tokens: acc.input,
                cache_creation_tokens: acc.cache_creation.saturating_sub(write_1h),
                cache_creation_1h_tokens: write_1h,
                cache_read_tokens: acc.cache_read,
                output_tokens: acc.output,
                input_price: c.input,
                output_price: c.output,
                cache_write_price: c.cache_write,
                cache_write_1h_price: c.cache_write_1h,
                cache_read_price: c.cache_read,
                cost_usd: acc.cost,
            }
        })
        .collect();
    lines.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.model.cmp(&b.model))
    });
    lines
}

// ── Arbitrary-range breakdown (receipt + per-day trend) ──────────────────────
//
// This is **the** fold: the arbitrary inclusive `[from_ms, to_ms]` window that
// every usage surface goes through. The today receipt and the sidebar badge are
// its `[local midnight, now]` case (see `build_breakdown_cached`), so all three
// agree by construction and `Σ line.cost == TodayUsage.cost` still reconciles to
// the cent. The Settings "usage" view adds the longer presets (7d / 30d / all)
// and the per-day trend line.
//
// Attribution differs by source, and this asymmetry is deliberate:
//   • **Claude** sessions are folded **per turn**: each finalized turn is
//     placed on the trend by its own `timestamp`, so a session spanning several
//     days spreads across those days' points — accurate.
//   • **Codex** rollouts expose only a single cumulative `total_token_usage`
//     snapshot per session (no per-turn deltas), so a Codex session is
//     attributed **whole to its creation day**. `has_codex_approximation` flags
//     this so the UI can footnote that Codex trend placement is approximate.
//     That also means a Codex session outliving midnight still under-reports on
//     the later days — the Claude-side bug this fold fixed has no Codex cure
//     until codex logs per-turn deltas.

/// One day's totals in a range breakdown — a point on the trend chart.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DailyUsagePoint {
    /// YYYY-MM-DD in the user's local timezone.
    pub date: String,
    pub input_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

/// Per-model receipt + per-day trend over an arbitrary `[from_ms, to_ms]` window.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct UsageRangeBreakdown {
    /// Inclusive window bounds echoed back as YYYY-MM-DD (local).
    pub from_date: String,
    pub to_date: String,
    /// One line per (source, model) over the whole window, sorted by desc cost.
    pub lines: Vec<ModelReceiptLine>,
    /// Per-day totals, ascending by date. Days with no usage are omitted.
    pub daily: Vec<DailyUsagePoint>,
    pub total_input_tokens: u64,
    pub total_cache_creation_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub agent_cost_usd: f64,
    pub fleet_cost_usd: f64,
    /// True if any Codex session contributed (whole-session-to-one-day
    /// attribution — its trend placement is approximate). Drives a UI footnote.
    pub has_codex_approximation: bool,
}

/// Epoch-ms → `YYYY-MM-DD` in the user's local timezone.
fn local_date_str(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_else(chrono::Utc::now)
        .with_timezone(&chrono::Local)
        .format("%Y-%m-%d")
        .to_string()
}

/// Parse an ISO-8601 / RFC-3339 timestamp (e.g. `2026-07-17T10:00:00.123Z` or
/// with a `+HH:MM` offset) to epoch milliseconds. `None` if unparseable — such
/// a turn can't be placed on the trend, so callers skip it.
fn parse_iso_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// Like [`fold_session_turns`] but windowed to `[from_ms, to_ms]` with per-turn
/// date attribution: a finalized turn contributes only if its own `timestamp`
/// falls in the window, and it lands in both the per-model map and the per-day
/// map keyed by the turn's local date. Turns without a parseable timestamp are
/// skipped (they can't be placed on the trend). Model tracking still advances on
/// skipped turns so a later dated turn that omits `model` resolves correctly.
#[cfg(test)]
fn fold_session_turns_range(
    jsonl: &str,
    source: &str,
    from_ms: i64,
    to_ms: i64,
    by_model: &mut std::collections::HashMap<(String, String), LineAcc>,
    by_day: &mut std::collections::BTreeMap<String, LineAcc>,
) {
    use crate::model_cost::{turn_cost_usd, TurnUsage};
    use std::collections::HashSet;

    let mut seen: HashSet<String> = HashSet::new();
    let mut last_model: Option<String> = None;

    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(msg) = v.get("message").and_then(|m| m.as_object()) else {
            continue;
        };
        if msg.get("stop_reason").map_or(true, |s| s.is_null()) {
            continue;
        }
        let msg_id = msg.get("id").and_then(|i| i.as_str()).unwrap_or_default();
        if !msg_id.is_empty() && !seen.insert(msg_id.to_string()) {
            continue;
        }

        // Advance model tracking even for out-of-window / undated turns.
        // Placeholder ids (`<synthetic>` control turns, `unknown`) never open a
        // line of their own; their usage folds under the real model in flight.
        let turn_model = msg
            .get("model")
            .and_then(|m| m.as_str())
            .filter(|m| crate::session::is_real_model_id(m));
        if let Some(m) = turn_model {
            last_model = Some(m.to_string());
        }

        // Per-turn timestamp is the top-level `timestamp` field (ISO 8601).
        let Some(ts_ms) = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(parse_iso_ms)
        else {
            continue;
        };
        if ts_ms < from_ms || ts_ms > to_ms {
            continue;
        }

        let usage = msg.get("usage");
        let input = usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let output = usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cache_creation = usage
            .and_then(|u| u.get("cache_creation_input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cache_read = usage
            .and_then(|u| u.get("cache_read_input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let web_search = usage
            .and_then(|u| u.get("server_tool_use"))
            .and_then(|s| s.get("web_search_requests"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);

        let model = turn_model
            .map(|s| s.to_string())
            .or_else(|| last_model.clone())
            .unwrap_or_default();

        let cost = turn_cost_usd(
            &model,
            &TurnUsage {
                input_tokens: input,
                output_tokens: output,
                cache_creation_tokens: cache_creation,
                cache_creation_1h_tokens: crate::model_cost::parse_cache_creation_1h(usage),
                cache_read_tokens: cache_read,
                web_search_requests: web_search,
            },
        );

        let cache_creation_1h = crate::model_cost::parse_cache_creation_1h(usage);
        by_model
            .entry((source.to_string(), model))
            .or_default()
            .add(input, cache_creation, cache_creation_1h, cache_read, output, cost);
        by_day.entry(local_date_str(ts_ms)).or_default().add(
            input,
            cache_creation,
            cache_creation_1h,
            cache_read,
            output,
            cost,
        );
    }
}

/// Fold a Codex session into the range breakdown, attributing its whole
/// cumulative snapshot to `attribute_ms`'s local day (see the module comment on
/// why Codex can't be split per turn). Returns `true` if it contributed.
#[cfg(test)]
fn fold_codex_session_range(
    uri: &str,
    attribute_ms: i64,
    by_model: &mut std::collections::HashMap<(String, String), LineAcc>,
    by_day: &mut std::collections::BTreeMap<String, LineAcc>,
) -> bool {
    let Ok(bd) = crate::codex_source::codex_token_breakdown(uri) else {
        return false;
    };
    if bd.total_tokens == 0 {
        return false;
    }
    let key_model = bd.model.clone().unwrap_or_else(|| "gpt".to_string());
    // `bd.input_tokens` is already full-price (cached excluded); no cache-write
    // bucket for codex, so `cache_creation` stays 0.
    by_model
        .entry(("codex".to_string(), key_model))
        .or_default()
        .add(bd.input_tokens, 0, 0, bd.cached_input_tokens, bd.output_tokens, bd.cost_usd);
    by_day
        .entry(local_date_str(attribute_ms))
        .or_default()
        .add(bd.input_tokens, 0, 0, bd.cached_input_tokens, bd.output_tokens, bd.cost_usd);
    true
}

// ── Cache-ready per-session projection ───────────────────────────────────────
//
// Both receipts re-read and re-parse every in-window session's full JSONL on
// every open. To make that cacheable, one session's spend is projected once into
// `(date_bucket, model) -> acc` cells — the smallest form from which every
// receipt window reconstructs exactly: sum the cells whose non-empty local date
// falls in `[from, to]`, bucketed per day. Today is just the `[midnight, now]`
// window of that same fold (see [`build_breakdown_cached`]), so there is one
// attribution rule, not two.
//
// `date_bucket` is the turn's local `YYYY-MM-DD`, or `""` for a Claude turn with
// no `timestamp` (transcripts predating the field). Undated turns are dropped:
// a turn we can't place on a day can't be claimed by a day-bounded window
// either. Windows are compared at **date** granularity; that matches the
// receipt's only caller (day-aligned presets `today` / `7d` / `30d` / `all`,
// always `to = now`), where
// `ts_ms >= from_ms` ⇔ `local_date(ts) >= local_date(from_ms)`.

/// One session's usage as `(date_bucket, model) -> acc`. The agent source is
/// uniform per session, so it is applied at query time and kept out of the key.
type SessionCells = std::collections::HashMap<(String, String), LineAcc>;

/// Project one Claude session's JSONL into `(date, model)` cells in a single
/// pass. Turn selection, msg-id dedup, `last_model` tracking and per-turn cost
/// are identical to [`fold_session_turns_range`]; the only addition is bucketing
/// each folded turn by its local date (`""` when the turn carries no timestamp).
fn fold_claude_session_cells(jsonl: &str) -> SessionCells {
    use crate::model_cost::{turn_cost_usd, TurnUsage};
    use std::collections::HashSet;

    let mut cells = SessionCells::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut last_model: Option<String> = None;

    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(msg) = v.get("message").and_then(|m| m.as_object()) else {
            continue;
        };
        if msg.get("stop_reason").map_or(true, |s| s.is_null()) {
            continue;
        }
        let msg_id = msg.get("id").and_then(|i| i.as_str()).unwrap_or_default();
        if !msg_id.is_empty() && !seen.insert(msg_id.to_string()) {
            continue;
        }

        // Advance model tracking even for undated turns (matches the range folder).
        // Placeholder ids (`<synthetic>` control turns, `unknown`) never open a
        // line of their own; their usage folds under the real model in flight.
        let turn_model = msg
            .get("model")
            .and_then(|m| m.as_str())
            .filter(|m| crate::session::is_real_model_id(m));
        if let Some(m) = turn_model {
            last_model = Some(m.to_string());
        }

        // Empty date bucket = no timestamp: today includes it, range excludes it.
        let date = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(parse_iso_ms)
            .map(local_date_str)
            .unwrap_or_default();

        let usage = msg.get("usage");
        let input = usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let output = usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cache_creation = usage
            .and_then(|u| u.get("cache_creation_input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cache_read = usage
            .and_then(|u| u.get("cache_read_input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let web_search = usage
            .and_then(|u| u.get("server_tool_use"))
            .and_then(|s| s.get("web_search_requests"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);

        let cache_creation_1h = crate::model_cost::parse_cache_creation_1h(usage);

        let model = turn_model
            .map(|s| s.to_string())
            .or_else(|| last_model.clone())
            .unwrap_or_default();
        let cost = turn_cost_usd(
            &model,
            &TurnUsage {
                input_tokens: input,
                output_tokens: output,
                cache_creation_tokens: cache_creation,
                cache_creation_1h_tokens: cache_creation_1h,
                cache_read_tokens: cache_read,
                web_search_requests: web_search,
            },
        );

        cells
            .entry((date, model))
            .or_default()
            .add(input, cache_creation, cache_creation_1h, cache_read, output, cost);
    }
    cells
}

/// Project one Codex session into a single `(creation-day, model)` cell. Codex
/// reports one cumulative snapshot per session (no per-turn split), so the whole
/// figure attributes to `attribute_ms`'s local day — same as
/// [`fold_codex_session_range`]. Returns empty cells on error / zero usage.
fn fold_codex_session_cells(uri: &str, attribute_ms: i64) -> SessionCells {
    let mut cells = SessionCells::new();
    let Ok(bd) = crate::codex_source::codex_token_breakdown(uri) else {
        return cells;
    };
    if bd.total_tokens == 0 {
        return cells;
    }
    let model = bd.model.clone().unwrap_or_else(|| "gpt".to_string());
    cells
        .entry((local_date_str(attribute_ms), model))
        .or_default()
        .add(bd.input_tokens, 0, 0, bd.cached_input_tokens, bd.output_tokens, bd.cost_usd);
    cells
}

/// Sum cells whose non-empty date is within `[from_date, to_date]` (undated
/// dropped) into both `by_model` and the per-day trend `by_day`. Dates
/// are `YYYY-MM-DD`, so lexicographic comparison is chronological.
fn sum_cells_window(
    cells: &SessionCells,
    source: &str,
    from_date: &str,
    to_date: &str,
    by_model: &mut std::collections::HashMap<(String, String), LineAcc>,
    by_day: &mut std::collections::BTreeMap<String, LineAcc>,
) {
    for ((date, model), acc) in cells {
        if date.is_empty() || date.as_str() < from_date || date.as_str() > to_date {
            continue;
        }
        by_model
            .entry((source.to_string(), model.clone()))
            .or_default()
            .add(
                acc.input,
                acc.cache_creation,
                acc.cache_creation_1h,
                acc.cache_read,
                acc.output,
                acc.cost,
            );
        by_day.entry(date.clone()).or_default().add(
            acc.input,
            acc.cache_creation,
            acc.cache_creation_1h,
            acc.cache_read,
            acc.output,
            acc.cost,
        );
    }
}

// ── Per-session cell cache ───────────────────────────────────────────────────
//
// The receipts used to re-read and re-parse every in-window session's full JSONL
// on every open / range switch. This cache projects each session into
// `(date, model)` cells once and reuses them until the session's usage changes,
// so repeat opens pay only aggregation, not disk + JSON parsing.
//
// Invalidation is by a cheap fingerprint taken from the already-scanned
// `SessionInfo` — `(last_activity_ms, total_input_tokens, total_output_tokens)` —
// not a filesystem stat: those fields change exactly when a session logs new
// usage, and a restart recomputes them identically, which lets the on-disk cache
// (P3) validate a persisted entry against a fresh scan without touching the
// transcript. Codex rollouts (a `codex://` URI, not a stat-able path) ride the
// same fingerprint.

/// Fingerprint gating a cached projection: changes iff the session's usage did.
type Fingerprint = (u64, u64, u64);

struct CacheEntry {
    fingerprint: Fingerprint,
    cells: SessionCells,
}

/// Process-wide projection cache. A fresh (`default`) instance is empty, so every
/// lookup is a miss that folds from disk — exactly what the hermetic unit tests
/// want. Production goes through [`usage_cache`], which warm-starts from disk.
#[derive(Default)]
struct UsageBreakdownCache {
    entries: std::collections::HashMap<String, CacheEntry>,
    /// Set whenever a fold or eviction changed `entries`, so the public entry
    /// points persist to disk only when there is something new to write.
    dirty: bool,
}

impl UsageBreakdownCache {
    fn fingerprint(s: &SessionInfo) -> Fingerprint {
        (
            s.last_activity_ms,
            s.total_input_tokens,
            s.total_output_tokens,
        )
    }

    /// Cells for `s`, folding its JSONL/rollout only on a miss or a fingerprint
    /// change. The fold (disk read + JSON parse) is the expensive step this
    /// cache exists to skip.
    fn cells(&mut self, s: &SessionInfo) -> &SessionCells {
        let fp = Self::fingerprint(s);
        let stale = self.entries.get(&s.id).map_or(true, |e| e.fingerprint != fp);
        if stale {
            let cells = fold_session_cells(s);
            self.entries.insert(
                s.id.clone(),
                CacheEntry {
                    fingerprint: fp,
                    cells,
                },
            );
            self.dirty = true;
        }
        &self.entries.get(&s.id).unwrap().cells
    }

    /// Drop cached entries for sessions no longer live, bounding the map to the
    /// current session set (sessions get pruned off disk over time).
    fn retain_sessions(&mut self, live_ids: &std::collections::HashSet<&str>) {
        let before = self.entries.len();
        self.entries.retain(|id, _| live_ids.contains(id.as_str()));
        if self.entries.len() != before {
            self.dirty = true;
        }
    }
}

/// Fold one session's JSONL/rollout into cells — the expensive read+parse a cache
/// hit avoids. Codex uses the URI-aware reader; Claude reads the file directly.
fn fold_session_cells(s: &SessionInfo) -> SessionCells {
    if s.agent_source == "codex" {
        fold_codex_session_cells(&s.jsonl_path, s.created_at_ms as i64)
    } else {
        let jsonl = std::fs::read_to_string(&s.jsonl_path).unwrap_or_default();
        fold_claude_session_cells(&jsonl)
    }
}

/// Ids of the sessions currently live, for [`UsageBreakdownCache::retain_sessions`].
fn live_ids(sessions: &[SessionInfo]) -> std::collections::HashSet<&str> {
    sessions.iter().map(|s| s.id.as_str()).collect()
}

// ── On-disk persistence ──────────────────────────────────────────────────────
//
// Persisting the cache lets a **cold start** (fresh process, e.g. after a Fleet
// restart) skip re-parsing every transcript: the first modal open validates each
// persisted entry's fingerprint against the fresh scan and re-folds only the
// sessions whose usage actually changed while Fleet was down. The on-disk shape
// is a flat list of cells (JSON can't key a map by a `(date, model)` tuple) tagged
// with a schema version; bump [`CACHE_SCHEMA_VERSION`] whenever the cell shape or
// fold semantics change so stale files are discarded rather than mis-read.

/// Bump when the cell shape or the projection semantics change.
const CACHE_SCHEMA_VERSION: u32 = 2;

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedCell {
    date: String,
    model: String,
    input: u64,
    cache_creation: u64,
    #[serde(default)]
    cache_creation_1h: u64,
    cache_read: u64,
    output: u64,
    cost: f64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedEntry {
    fingerprint: Fingerprint,
    cells: Vec<PersistedCell>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedCache {
    version: u32,
    entries: std::collections::HashMap<String, PersistedEntry>,
}

impl UsageBreakdownCache {
    fn to_persisted(&self) -> PersistedCache {
        let entries = self
            .entries
            .iter()
            .map(|(id, e)| {
                let cells = e
                    .cells
                    .iter()
                    .map(|((date, model), acc)| PersistedCell {
                        date: date.clone(),
                        model: model.clone(),
                        input: acc.input,
                        cache_creation: acc.cache_creation,
                        cache_creation_1h: acc.cache_creation_1h,
                        cache_read: acc.cache_read,
                        output: acc.output,
                        cost: acc.cost,
                    })
                    .collect();
                (
                    id.clone(),
                    PersistedEntry {
                        fingerprint: e.fingerprint,
                        cells,
                    },
                )
            })
            .collect();
        PersistedCache {
            version: CACHE_SCHEMA_VERSION,
            entries,
        }
    }

    fn from_persisted(p: PersistedCache) -> Self {
        // Version mismatch → start empty so every session re-folds under the
        // current semantics instead of trusting an incompatible projection.
        if p.version != CACHE_SCHEMA_VERSION {
            return Self::default();
        }
        let entries = p
            .entries
            .into_iter()
            .map(|(id, e)| {
                let mut cells = SessionCells::new();
                for c in e.cells {
                    let mut acc = LineAcc::default();
                    acc.add(
                        c.input,
                        c.cache_creation,
                        c.cache_creation_1h,
                        c.cache_read,
                        c.output,
                        c.cost,
                    );
                    cells.insert((c.date, c.model), acc);
                }
                (
                    id,
                    CacheEntry {
                        fingerprint: e.fingerprint,
                        cells,
                    },
                )
            })
            .collect();
        Self {
            entries,
            dirty: false,
        }
    }

    /// Load the persisted cache, or an empty one when the file is absent, torn,
    /// or a stale shape — every failure path self-heals into a full re-fold.
    fn load_from(path: &std::path::Path) -> Self {
        let Ok(bytes) = std::fs::read(path) else {
            return Self::default();
        };
        serde_json::from_slice::<PersistedCache>(&bytes)
            .map(Self::from_persisted)
            .unwrap_or_default()
    }

    /// Persist atomically (temp file + rename) so a crash mid-write can't leave a
    /// torn file that would poison the next load.
    fn store_to(&self, path: &std::path::Path) {
        let Ok(json) = serde_json::to_vec(&self.to_persisted()) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// `~/.fleet/usage-breakdown-cache.json` — the on-disk projection cache.
fn cache_path() -> Option<std::path::PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("usage-breakdown-cache.json"))
}

/// Flush the cache to disk if a fold or eviction changed it since the last write.
fn persist_cache(cache: &mut UsageBreakdownCache) {
    if !cache.dirty {
        return;
    }
    if let Some(p) = cache_path() {
        cache.store_to(&p);
    }
    cache.dirty = false;
}

/// The process-wide cache backing the public receipt entry points, warm-started
/// from disk so the first post-restart open skips re-parsing unchanged sessions.
fn usage_cache() -> &'static std::sync::Mutex<UsageBreakdownCache> {
    static CACHE: std::sync::LazyLock<std::sync::Mutex<UsageBreakdownCache>> =
        std::sync::LazyLock::new(|| {
            let c = cache_path()
                .map(|p| UsageBreakdownCache::load_from(&p))
                .unwrap_or_default();
            std::sync::Mutex::new(c)
        });
    &CACHE
}

/// Guards against overlapping warms: a scan fires every few seconds, but only one
/// background fold should run at a time.
static WARMING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Pre-fold any new/changed sessions into the cache **off the caller's thread**,
/// so the first receipt open after a scan finds everything warm instead of paying
/// the full transcript re-parse. Skips when a warm is already in flight (the next
/// scan re-warms whatever changed since). Called from the desktop scan loop.
///
/// After the on-disk cache (see [`usage_cache`]) this is cheap on every run but
/// the first-ever one: it re-folds only the sessions whose fingerprint changed.
pub fn warm_usage_cache(sessions: &[SessionInfo]) {
    use std::sync::atomic::Ordering;
    if WARMING.swap(true, Ordering::AcqRel) {
        return; // a warm is already running
    }
    let owned: Vec<SessionInfo> = sessions.to_vec();
    std::thread::spawn(move || {
        // Reset the guard even if the fold panics (e.g. a poisoned lock), so a
        // one-off failure can't wedge warming off permanently.
        struct WarmGuard;
        impl Drop for WarmGuard {
            fn drop(&mut self) {
                WARMING.store(false, Ordering::Release);
            }
        }
        let _guard = WarmGuard;

        let mut cache = usage_cache().lock().unwrap();
        cache.retain_sessions(&live_ids(&owned));
        for s in &owned {
            let _ = cache.cells(s);
        }
        persist_cache(&mut cache);
    });
}

/// Per-model receipt + per-day trend over an arbitrary inclusive window.
///
/// `sessions` is the already-scanned session list; each in-window session is
/// projected into cached cells (folded from disk only on a miss). Fleet's own LLM
/// spend in the window is folded per model from `fleet_llm_usage.jsonl`.
pub fn usage_range_breakdown(
    sessions: &[SessionInfo],
    from_ms: i64,
    to_ms: i64,
) -> UsageRangeBreakdown {
    let mut cache = usage_cache().lock().unwrap();
    cache.retain_sessions(&live_ids(sessions));
    let out = build_range_breakdown_cached(sessions, from_ms, to_ms, &mut cache);
    persist_cache(&mut cache);
    out
}

/// Test-only wrapper: fresh (empty) cache, so each call folds from disk and the
/// range-receipt tests stay hermetic (see [`build_breakdown`]).
#[cfg(test)]
fn build_range_breakdown(
    sessions: &[SessionInfo],
    from_ms: i64,
    to_ms: i64,
) -> UsageRangeBreakdown {
    build_range_breakdown_cached(sessions, from_ms, to_ms, &mut UsageBreakdownCache::default())
}

/// Pure core of [`usage_range_breakdown`], with Fleet's own LLM entries injected
/// so it can be unit-tested without reading `~/.fleet/fleet_llm_usage.jsonl`.
/// Each in-window session is projected through `cache` (folded from disk only on
/// a miss) and summed over the date window `[from, to]`.
/// Infer the agent-source label for a report-sourced receipt line from its
/// model id. The daily-report DB keys usage by model, not by source, so we map
/// gpt/codex model ids to Codex and everything else to Claude Code. (Fleet's own
/// guard/audit LLM overhead is never recorded in daily reports, so it simply
/// does not appear on report-sourced days — a negligible $1–5/day omission.)
fn infer_report_source(model: &str) -> String {
    let m = model.to_ascii_lowercase();
    if m.contains("gpt") || m.contains("codex") {
        "codex".to_string()
    } else {
        "claude-code".to_string()
    }
}

/// Backfill `by_model` / `by_day` from the durable daily-report DB for dates in
/// `[from_date, upto_exclusive)` — the window portion older than the live 7-day
/// session pool. The live scan drops transcripts whose mtime ages past 7 days
/// (`session::parse_session_info`), so without this every multi-day range folded
/// the same ~7-day pool and undercounted historical spend by design. The report
/// DB, in contrast, persists each day's per-model tokens+cost back to install
/// time regardless of transcript retention.
///
/// Per-model cost uses the report's stored `cost_usd` when present (exact, cache
/// included); for pre-cost (v0) reports it recomputes token×price so ancient
/// days still show an approximate input+output-only figure instead of $0.
fn fold_report_days(
    from_date: &str,
    upto_exclusive: &str,
    by_model: &mut std::collections::HashMap<(String, String), LineAcc>,
    by_day: &mut std::collections::BTreeMap<String, LineAcc>,
) {
    if from_date >= upto_exclusive {
        return; // live pool already covers the whole requested window
    }
    let Ok(store) = crate::daily_report::ReportStore::open() else {
        return;
    };
    let Ok(dates) = store.list_dates() else {
        return;
    };
    for date in dates {
        let d = date.as_str();
        if d < from_date || d >= upto_exclusive {
            continue;
        }
        let Ok(Some(report)) = store.get_report(d) else {
            continue;
        };
        for (model, mt) in &report.metrics.model_breakdown {
            fold_report_model(&date, model, mt, by_model, by_day);
        }
    }
}

/// Fold one report day's `(model, ModelTokens)` entry into the receipt
/// accumulators. Split out of [`fold_report_days`] so the口径 conversion below
/// is testable without a report DB on disk.
///
/// **The口径 conversion:** `ModelTokens::input_tokens` is
/// `Σ(input + cache_write + cache_read)` — every token sent to the API, matching
/// the stored `cost_usd`. The receipt itemises input *separately* from the two
/// cache rows, so the cache figures must be netted out or the Input row shows
/// the whole API-side volume at the full input price while the cache rows list
/// it a second time (opus-4-8 over 30 days: a $92k itemisation under a $17.6k
/// subtotal).
fn fold_report_model(
    date: &str,
    model: &str,
    mt: &crate::daily_report::ModelTokens,
    by_model: &mut std::collections::HashMap<(String, String), LineAcc>,
    by_day: &mut std::collections::BTreeMap<String, LineAcc>,
) {
    use crate::model_cost::{turn_cost_usd, TurnUsage};

    // saturating_sub: a v0/legacy report whose `input_tokens` predates the
    // all-inclusive口径 can be smaller than its own cache figures; clamping to 0
    // is the honest answer there rather than wrapping to ~1.8e19.
    let net_input = mt
        .input_tokens
        .saturating_sub(mt.cache_creation_tokens)
        .saturating_sub(mt.cache_read_tokens);

    let cost = if mt.cost_usd > 0.0 {
        mt.cost_usd
    } else {
        // Pre-cost (v0) reports: recompute so ancient days show an approximate
        // figure instead of $0. Prices the netted input, not the inclusive
        // total — charging the full input rate on tokens the cache rows already
        // bill is exactly the double-count above.
        turn_cost_usd(
            model,
            &TurnUsage {
                input_tokens: net_input,
                output_tokens: mt.output_tokens,
                cache_creation_tokens: mt.cache_creation_tokens,
                // Reports written before TTL-aware pricing carry no 1h subset,
                // so their writes can only be priced at the 5-minute rate.
                cache_creation_1h_tokens: mt.cache_creation_1h_tokens,
                cache_read_tokens: mt.cache_read_tokens,
                web_search_requests: 0,
            },
        )
    };

    // Report rows written before the placeholder guard landed are keyed under
    // `<synthetic>` — real money from a session whose model was never recorded.
    // They can't be re-attributed (the transcripts are gone), so collapse them
    // into the `unknown` placeholder the rest of the codebase already uses
    // instead of presenting a control-turn marker as if it were a model.
    let model = if crate::session::is_real_model_id(model) {
        model
    } else {
        "unknown"
    };
    let source = infer_report_source(model);
    by_model
        .entry((source, model.to_string()))
        .or_default()
        .add(
            net_input,
            mt.cache_creation_tokens,
            mt.cache_creation_1h_tokens,
            mt.cache_read_tokens,
            mt.output_tokens,
            cost,
        );
    by_day.entry(date.to_string()).or_default().add(
        net_input,
        mt.cache_creation_tokens,
        mt.cache_creation_1h_tokens,
        mt.cache_read_tokens,
        mt.output_tokens,
        cost,
    );
}

fn build_range_breakdown_cached(
    sessions: &[SessionInfo],
    from_ms: i64,
    to_ms: i64,
    cache: &mut UsageBreakdownCache,
) -> UsageRangeBreakdown {
    use std::collections::{BTreeMap, HashMap};

    let mut by_model: HashMap<(String, String), LineAcc> = HashMap::new();
    let mut by_day: BTreeMap<String, LineAcc> = BTreeMap::new();
    let mut has_codex_approximation = false;

    // Date-granularity window bounds; cells carry `YYYY-MM-DD` keys.
    let from_date = local_date_str(from_ms);
    let to_date = local_date_str(to_ms);

    // The live session pool only reliably covers the last ~7 days: any transcript
    // whose mtime ages past 7 days is dropped by `session::parse_session_info`,
    // so folding live sessions can only answer for `[live_floor_date, to_date]`.
    // Days older than that are served from the durable daily-report DB below.
    // `live_from_date` clamps the live fold to whichever of the two floors is
    // later, so the live and report halves stay date-disjoint (no double-count).
    let live_floor_date = local_date_str(to_ms - 7 * 86_400_000);
    let live_from_date = if from_date.as_str() > live_floor_date.as_str() {
        from_date.clone()
    } else {
        live_floor_date.clone()
    };

    for s in sessions {
        if s.agent_source == "codex" {
            // Whole-session attribution to the creation day; include only if
            // that instant falls in the window.
            let created = s.created_at_ms as i64;
            if created < from_ms || created > to_ms {
                continue;
            }
            let cells = cache.cells(s);
            if !cells.is_empty() {
                has_codex_approximation = true;
            }
            sum_cells_window(
                cells,
                &s.agent_source,
                &live_from_date,
                &to_date,
                &mut by_model,
                &mut by_day,
            );
            continue;
        }
        // Claude: prune sessions that cannot overlap the window before the
        // (cache-miss-only) projection — a session with no activity at/after
        // `from_ms`, or created after `to_ms`, has no in-window turns.
        if (s.last_activity_ms as i64) < from_ms || (s.created_at_ms as i64) > to_ms {
            continue;
        }
        let cells = cache.cells(s);
        sum_cells_window(
            cells,
            &s.agent_source,
            &live_from_date,
            &to_date,
            &mut by_model,
            &mut by_day,
        );
    }

    // Backfill the pre-live-window portion `[from_date, live_from_date)` from the
    // durable daily-report DB. No-op for the today/7d presets (whose `from_date`
    // is already at or after the live floor).
    fold_report_days(&from_date, &live_from_date, &mut by_model, &mut by_day);

    let lines = build_lines(by_model);

    let mut total_input = 0u64;
    let mut total_cache_creation = 0u64;
    let mut total_cache_read = 0u64;
    let mut total_output = 0u64;
    let mut agent_cost = 0.0;
    for l in &lines {
        total_input = total_input.saturating_add(l.input_tokens);
        // The line splits its writes by TTL, so the header total takes both rows.
        total_cache_creation = total_cache_creation
            .saturating_add(l.cache_creation_tokens)
            .saturating_add(l.cache_creation_1h_tokens);
        total_cache_read = total_cache_read.saturating_add(l.cache_read_tokens);
        total_output = total_output.saturating_add(l.output_tokens);
        agent_cost += l.cost_usd;
    }

    // Header `from_date` = the earliest day we actually have data for, not the
    // raw requested lower bound. The "全部" preset requests `from_ms = 0`, which
    // would otherwise render a misleading `1970-01-01`; the real floor is the
    // first day present in the trend (report-backfilled or live).
    let actual_from_date = by_day
        .keys()
        .next()
        .cloned()
        .unwrap_or_else(|| from_date.clone());

    // BTreeMap iterates ascending by date, so the trend is already ordered.
    let daily: Vec<DailyUsagePoint> = by_day
        .into_iter()
        .map(|(date, acc)| DailyUsagePoint {
            date,
            input_tokens: acc.input,
            cache_creation_tokens: acc.cache_creation,
            cache_read_tokens: acc.cache_read,
            output_tokens: acc.output,
            cost_usd: acc.cost,
        })
        .collect();

    UsageRangeBreakdown {
        from_date: actual_from_date,
        to_date,
        lines,
        daily,
        total_input_tokens: total_input,
        total_cache_creation_tokens: total_cache_creation,
        total_cache_read_tokens: total_cache_read,
        total_output_tokens: total_output,
        total_cost_usd: agent_cost,
        agent_cost_usd: agent_cost,
        // Always 0: Fleet's own overhead is excluded from this receipt (see the
        // note in `today_usage`). Kept on the wire so mobile clients that read
        // the split don't see `undefined`.
        fleet_cost_usd: 0.0,
        has_codex_approximation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_with_input(
        created_at_ms: u64,
        cost: f64,
        input: u64,
        output: u64,
        is_subagent: bool,
    ) -> SessionInfo {
        // Only the fields `cloud_usage`'s cumulative sum reads matter; the rest
        // default.
        let mut s: SessionInfo = serde_json::from_value(serde_json::json!({
            "id": "x",
            "workspacePath": "/tmp",
            "workspaceName": "tmp",
            "isSubagent": is_subagent,
            "parentSessionId": null,
            "agentType": null,
            "agentDescription": null,
            "slug": null,
            "aiTitle": null,
            "status": "waitingInput",
            "tokenSpeed": 0.0,
            "agentTokenSpeed": 0.0,
            "totalOutputTokens": output,
            "totalInputTokens": input,
            "totalCostUsd": cost,
            "agentTotalCostUsd": cost,
            "costSpeedUsdPerMin": 0.0,
            "lastMessagePreview": null,
            "lastActivityMs": 0,
            "createdAtMs": created_at_ms,
            "jsonlPath": "/tmp/x.jsonl",
            "model": null,
            "thinkingLevel": null,
            "pid": null,
            "pidPrecise": false,
            "agentSource": "claude-code"
        }))
        .expect("construct SessionInfo");
        s.created_at_ms = created_at_ms;
        s
    }

    #[test]
    fn day_bounds_are_a_full_local_day() {
        // 2026-07-12T15:30:00Z — pick any instant; bounds must span exactly 24h-1ms.
        let now_ms = 1_784_000_000_000; // arbitrary fixed instant
        let (start, end, date) = day_bounds_ms(now_ms);
        assert_eq!(end - start, 86_400_000 - 1);
        assert!(start <= now_ms && now_ms <= end);
        assert_eq!(date.len(), 10); // YYYY-MM-DD
    }

    #[test]
    fn cloud_usage_cumulative_sums_all_sessions_regardless_of_date() {
        // `cloud_usage` folds today's window through the shared projection cache,
        // which persists to `$FLEET_HOME`; keep that off the real ~/.fleet.
        let _lock = crate::session::fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("FLEET_HOME", tmp.path());

        // Cumulative is the customer billing basis: every retained agent session
        // counts regardless of when it was created; subagents contribute tokens
        // but not to the top-level session count.
        let old = 1_000_000_000_000u64; // 2001 — definitely not today
        let sessions = vec![
            session_with_input(old, 3.0, 1000, 200, false),
            session_with_input(old + 5, 1.0, 500, 50, true), // subagent
            session_with_input(9_000_000_000_000, 2.0, 400, 80, false),
        ];
        let u = cloud_usage(&sessions);
        assert_eq!(u.cumulative_input_tokens, 1900); // 1000 + 500 + 400
        assert_eq!(u.cumulative_output_tokens, 330); // 200 + 50 + 80
        assert!((u.cumulative_agent_cost_usd - 6.0).abs() < 1e-9);
        assert_eq!(u.cumulative_session_count, 2); // subagent excluded from count

        std::env::remove_var("FLEET_HOME");
    }
}

#[cfg(test)]
mod breakdown_tests {
    use super::*;

    /// Write a JSONL file with the given finalized assistant turns and return a
    /// SessionInfo pointing at it, created `now` (so it lands in "today").
    fn today_session_with_jsonl(
        tag: &str,
        source: &str,
        jsonl: &str,
        is_subagent: bool,
    ) -> SessionInfo {
        let now_ms = chrono::Local::now().timestamp_millis() as u64;
        // `tag` keeps parallel tests off each other's temp file (a time-based
        // path alone races when two tests run in the same millisecond).
        let dir = std::env::temp_dir().join(format!("fleet-brk-test-{tag}-{is_subagent}-{source}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        std::fs::write(&path, jsonl).unwrap();
        let mut s: SessionInfo = serde_json::from_value(serde_json::json!({
            "id": "x",
            "workspacePath": "/tmp",
            "workspaceName": "tmp",
            "isSubagent": is_subagent,
            "parentSessionId": null,
            "agentType": null,
            "agentDescription": null,
            "slug": null,
            "aiTitle": null,
            "status": "waitingInput",
            "tokenSpeed": 0.0,
            "agentTokenSpeed": 0.0,
            "totalOutputTokens": 0,
            "totalInputTokens": 0,
            "totalCostUsd": 0.0,
            "agentTotalCostUsd": 0.0,
            "costSpeedUsdPerMin": 0.0,
            "lastMessagePreview": null,
            "lastActivityMs": 0,
            "createdAtMs": now_ms,
            "jsonlPath": path.to_string_lossy(),
            "model": null,
            "thinkingLevel": null,
            "pid": null,
            "pidPrecise": false,
            "agentSource": source
        }))
        .expect("construct SessionInfo");
        s.created_at_ms = now_ms;
        // A live session's `last_activity_ms` is its transcript mtime; the window
        // prune skips sessions with no activity at/after the window start, so a
        // fixture left at 0 would be pruned before its turns are ever read.
        s.last_activity_ms = now_ms;
        // The projection cache is keyed by session id, so fixtures sharing one id
        // would serve each other's cells when a test passes several sessions.
        s.id = tag.to_string();
        s.jsonl_path = path.to_string_lossy().to_string();
        s.agent_source = source.to_string();
        s
    }

    /// A turn stamped **now**, so it lands in today's window. The today receipt
    /// attributes by each turn's own timestamp, so a fixed past date would put
    /// every fixture turn outside the window under test.
    fn turn(id: &str, model: &str, input: u64, cc: u64, cr: u64, output: u64) -> String {
        turn_now(id, model, input, cc, cr, output)
    }

    /// Same as [`turn`] but with an explicit ISO timestamp.
    fn turn_stamped(
        id: &str,
        iso: &str,
        model: &str,
        input: u64,
        cc: u64,
        cr: u64,
        output: u64,
    ) -> String {
        serde_json::json!({
            "type": "assistant",
            "timestamp": iso,
            "message": {
                "id": id,
                "model": model,
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": input,
                    "cache_creation_input_tokens": cc,
                    "cache_read_input_tokens": cr,
                    "output_tokens": output
                }
            }
        })
        .to_string()
    }

    /// `turn_stamped` at the current instant (RFC-3339 with local offset).
    fn turn_now(id: &str, model: &str, input: u64, cc: u64, cr: u64, output: u64) -> String {
        let iso = chrono::Local::now().to_rfc3339();
        turn_stamped(id, &iso, model, input, cc, cr, output)
    }

    #[test]
    fn per_model_tokens_and_cost_reconcile_with_extract() {
        // One Sonnet turn: 1000 in / 2000 cw / 5000 cr / 500 out.
        // cost = 1000/1e6*3 + 500/1e6*15 + 2000/1e6*3.75 + 5000/1e6*0.30
        //      = 0.003 + 0.0075 + 0.0075 + 0.0015 = 0.0195
        let jsonl = turn("m1", "claude-sonnet-4-5", 1000, 2000, 5000, 500);
        let sessions = vec![today_session_with_jsonl(
            "reconcile",
            "claude-code",
            &jsonl,
            false,
        )];
        let now = chrono::Local::now().timestamp_millis();
        let b = build_breakdown(&sessions, now);

        assert_eq!(b.lines.len(), 1, "one model line");
        let l = &b.lines[0];
        assert_eq!(l.model, "claude-sonnet-4-5");
        assert_eq!(l.source, "claude-code");
        assert_eq!(l.input_tokens, 1000);
        assert_eq!(l.cache_creation_tokens, 2000);
        assert_eq!(l.cache_read_tokens, 5000);
        assert_eq!(l.output_tokens, 500);
        assert!((l.input_price - 3.0).abs() < 1e-9);
        assert!((l.output_price - 15.0).abs() < 1e-9);
        assert!(
            (l.cost_usd - 0.0195).abs() < 1e-9,
            "line cost {}",
            l.cost_usd
        );

        // Grand total reconciles with the extract-based per-session cost
        // (the same fold `SessionInfo.total_cost_usd` uses).
        let extracted = crate::daily_report::extract_session_metrics(&jsonl);
        assert!(
            (b.total_cost_usd - extracted.cost_usd).abs() < 1e-9,
            "breakdown total {} vs extract {}",
            b.total_cost_usd,
            extracted.cost_usd
        );
        assert!((b.agent_cost_usd - 0.0195).abs() < 1e-9);
        assert_eq!(b.fleet_cost_usd, 0.0);
    }

    #[test]
    fn two_models_split_into_two_lines_sorted_by_cost() {
        // Opus 4.8 turn is far pricier than a Haiku turn → Opus line first.
        let jsonl = [
            turn("op", "claude-opus-4-8", 1_000_000, 0, 0, 1_000_000), // $5 + $25 = $30
            turn("hk", "claude-haiku-4-5", 1_000_000, 0, 0, 1_000_000), // $1 + $5 = $6
        ]
        .join("\n");
        let sessions = vec![today_session_with_jsonl(
            "twomodels",
            "claude-code",
            &jsonl,
            false,
        )];
        let now = chrono::Local::now().timestamp_millis();
        let b = build_breakdown(&sessions, now);

        assert_eq!(b.lines.len(), 2);
        assert_eq!(b.lines[0].model, "claude-opus-4-8", "pricier model first");
        assert_eq!(b.lines[1].model, "claude-haiku-4-5");
        assert!((b.lines[0].cost_usd - 30.0).abs() < 1e-6);
        assert!((b.lines[1].cost_usd - 6.0).abs() < 1e-6);
        assert!((b.total_cost_usd - 36.0).abs() < 1e-6);
        assert_eq!(b.total_output_tokens, 2_000_000);
    }

    /// Turns dated before today stay off today's receipt even when the session
    /// itself was created today — the window is over turn timestamps, not over
    /// session lifetimes. (A session created today can hold older turns after a
    /// `--resume` that re-logs history, or a clock step.)
    #[test]
    fn excludes_turns_dated_before_today() {
        let jsonl = turn_stamped(
            "m1",
            "2001-09-09T01:46:40Z",
            "claude-sonnet-4-5",
            1000,
            0,
            0,
            100,
        );
        let s = today_session_with_jsonl("excludes", "claude-code", &jsonl, false);
        let now = chrono::Local::now().timestamp_millis();
        let b = build_breakdown(&[s], now);
        assert!(b.lines.is_empty(), "a 2001-dated turn is not today's spend");
        assert_eq!(b.total_cost_usd, 0.0);
    }

    /// A session that started **yesterday** and is still burning tokens today
    /// must have its today-dated turns on today's receipt.
    ///
    /// Attributing by the transcript's birth day instead dropped every
    /// long-lived / handoff session's post-midnight spend: on 2026-08-27 the
    /// receipt's "today" page showed 46.8M tok / $37 while the 7-day view's
    /// today bar — which already folded per turn — showed 359.8M tok / $225,
    /// because six sessions born 08-26 were still running.
    #[test]
    fn counts_today_turns_of_a_session_created_yesterday() {
        let jsonl = turn("carry", "claude-sonnet-4-5", 1000, 0, 0, 100);
        let mut s = today_session_with_jsonl("carryover", "claude-code", &jsonl, false);
        let now = chrono::Local::now().timestamp_millis();
        // 30h ago is before today's local midnight at every hour of the day.
        s.created_at_ms = (now - 30 * 3_600_000).max(0) as u64;
        s.last_activity_ms = now.max(0) as u64;

        let b = build_breakdown(&[s], now);

        assert_eq!(
            b.lines.len(),
            1,
            "today's turns from a yesterday-born session were dropped"
        );
        // 1000/1e6*$3 + 100/1e6*$15 = 0.003 + 0.0015
        assert!(
            (b.total_cost_usd - 0.0045).abs() < 1e-9,
            "total {}",
            b.total_cost_usd
        );
    }

    /// The sidebar badge must agree with the receipt it opens, on the same
    /// per-turn口径 — including the carry-over session above. `today_usage`
    /// summed `SessionInfo.total_cost_usd` for sessions *created* today, so a
    /// session that outlived midnight vanished from the badge for the rest of
    /// its life.
    #[test]
    fn sidebar_badge_reconciles_with_the_receipt_across_midnight() {
        let jsonl = turn("badge", "claude-sonnet-4-5", 1000, 0, 0, 100);
        let mut s = today_session_with_jsonl("badge", "claude-code", &jsonl, false);
        let now = chrono::Local::now().timestamp_millis();
        s.created_at_ms = (now - 30 * 3_600_000).max(0) as u64;
        s.last_activity_ms = now.max(0) as u64;
        // Live session-level totals deliberately disagree with the transcript:
        // the badge must fold the same cells the receipt does, not these.
        s.total_cost_usd = 999.0;
        s.total_input_tokens = 7;
        s.total_output_tokens = 7;

        let b = build_breakdown(&[s.clone()], now);
        let u = build_today_usage(&[s], now);

        assert!(
            (u.cost_usd - b.total_cost_usd).abs() < 1e-9,
            "badge ${} vs receipt ${}",
            u.cost_usd,
            b.total_cost_usd
        );
        assert!((u.cost_usd - 0.0045).abs() < 1e-9, "badge ${}", u.cost_usd);
        assert_eq!(u.output_tokens, 100, "badge output tokens");
        assert_eq!(u.input_tokens, 1000, "badge input tokens (cache-inclusive)");
        assert_eq!(u.session_count, 1, "the carry-over session counts today");
    }

    /// The badge's cost / output figures cover exactly today's turns, and its
    /// session count covers exactly the non-subagent sessions that spent
    /// something today. (Replaces the retired `sums_only_sessions_created_today`,
    /// which asserted the birth-day口径.)
    #[test]
    fn badge_counts_only_today_dated_turns() {
        let yesterday_iso =
            chrono::DateTime::from_timestamp_millis(
                chrono::Local::now().timestamp_millis() - 30 * 3_600_000,
            )
            .unwrap()
            .with_timezone(&chrono::Local)
            .to_rfc3339();

        // 1000 in / 100 out on Sonnet = $0.003 + $0.0015 = $0.0045 per session.
        let today_a = today_session_with_jsonl(
            "badge-a",
            "claude-code",
            &turn("a", "claude-sonnet-4-5", 1000, 0, 0, 100),
            false,
        );
        let today_b = today_session_with_jsonl(
            "badge-b",
            "claude-code",
            &turn("b", "claude-sonnet-4-5", 1000, 0, 0, 100),
            false,
        );
        // Subagent: its spend counts, its head does not.
        let sub = today_session_with_jsonl(
            "badge-sub",
            "claude-code",
            &turn("s", "claude-sonnet-4-5", 1000, 0, 0, 100),
            true,
        );
        // Only yesterday-dated turns → contributes nothing to today.
        let stale = today_session_with_jsonl(
            "badge-stale",
            "claude-code",
            &turn_stamped("o", &yesterday_iso, "claude-sonnet-4-5", 9_000, 0, 0, 900),
            false,
        );

        // After the fixtures: their `created_at_ms` is stamped at construction,
        // and a window ending before that prunes them all.
        let now = chrono::Local::now().timestamp_millis();
        let u = build_today_usage(&[today_a, today_b, sub, stale], now);

        assert!(
            (u.cost_usd - 0.0135).abs() < 1e-9,
            "3 × $0.0045 (yesterday's turns excluded), got ${}",
            u.cost_usd
        );
        assert_eq!(u.output_tokens, 300, "3 × 100 output");
        assert_eq!(u.session_count, 2, "subagent and idle-today session excluded");
    }

    /// The badge's `input_tokens` is the cache-inclusive "sent to the API"
    /// figure, so cache writes and reads count toward it — the receipt itemises
    /// them apart, the badge does not. (Replaces the retired
    /// `sums_input_tokens_of_today_sessions`.)
    #[test]
    fn badge_input_tokens_are_cache_inclusive() {
        let jsonl = turn("cache", "claude-sonnet-4-5", 1000, 2000, 5000, 500);
        let s = today_session_with_jsonl("badge-cache", "claude-code", &jsonl, false);
        let now = chrono::Local::now().timestamp_millis();

        let u = build_today_usage(&[s], now);

        assert_eq!(u.input_tokens, 8000, "1000 input + 2000 write + 5000 read");
        assert_eq!(u.output_tokens, 500);
    }

    /// Nothing spent today → an all-zero badge, not last night's figure.
    /// (Replaces the retired `empty_when_nothing_today`.)
    #[test]
    fn badge_empty_when_nothing_today() {
        let jsonl = turn_stamped(
            "old",
            "2001-09-09T01:46:40Z",
            "claude-sonnet-4-5",
            9_000,
            0,
            0,
            900,
        );
        let s = today_session_with_jsonl("badge-empty", "claude-code", &jsonl, false);
        let now = chrono::Local::now().timestamp_millis();

        let u = build_today_usage(&[s], now);

        assert_eq!(u.cost_usd, 0.0);
        assert_eq!((u.input_tokens, u.output_tokens, u.session_count), (0, 0, 0));
    }

    /// A minimal codex rollout: one `turn_context` (for the model) and one
    /// cumulative `token_count` event (codex-native field names).
    fn codex_rollout(model: &str, raw_input: u64, cached: u64, output: u64) -> String {
        [
            serde_json::json!({ "type": "turn_context", "payload": { "model": model } })
                .to_string(),
            serde_json::json!({
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": { "total_token_usage": {
                        "input_tokens": raw_input,
                        "cached_input_tokens": cached,
                        "output_tokens": output
                    }}
                }
            })
            .to_string(),
        ]
        .join("\n")
    }

    #[test]
    fn codex_session_folds_into_a_line_with_cache_read() {
        // Regression: codex agent sessions used to be dropped from the receipt
        // entirely (fold_session_turns is Claude-shaped, needs `type:"assistant"`
        // turns codex rollouts don't have). They must now fold from the native
        // cumulative snapshot: full-price input = raw − cached, cached →
        // cache-read, output, and no cache-write bucket.
        let jsonl = codex_rollout("gpt-5.6-sol", 100_000, 60_000, 4_000);
        // Codex sessions store a `codex://` URI over an absolute path, not a
        // readable filesystem path — write the rollout to an absolute temp file
        // and point jsonl_path at `codex://<abs>` so the URI-aware reader (which
        // build_breakdown uses for codex) resolves it, exactly like real data.
        let dir = std::env::temp_dir().join("fleet-brk-test-codexfold");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rollout.jsonl");
        std::fs::write(&path, &jsonl).unwrap();
        let mut s = today_session_with_jsonl("codexfold", "codex", "", false);
        s.jsonl_path = format!("codex://{}", path.to_string_lossy());
        let now = chrono::Local::now().timestamp_millis();
        let b = build_breakdown(&[s], now);

        assert_eq!(
            b.lines.len(),
            1,
            "codex session must produce one receipt line"
        );
        let l = &b.lines[0];
        assert_eq!(l.source, "codex");
        assert_eq!(l.model, "gpt-5.6-sol");
        assert_eq!(l.input_tokens, 40_000, "full-price input = raw − cached");
        assert_eq!(l.cache_read_tokens, 60_000, "cached_input → cache_read");
        assert_eq!(
            l.cache_creation_tokens, 0,
            "codex has no cache-write bucket"
        );
        assert_eq!(l.output_tokens, 4_000);
        assert!(l.cost_usd > 0.0, "codex line must carry a cost");

        // Cost reconciles with the canonical model_cost fold over the same
        // buckets — proving the fold routes cached to the cache-read rate.
        let expected = crate::model_cost::turn_cost_usd(
            "gpt-5.6-sol",
            &crate::model_cost::TurnUsage {
                input_tokens: 40_000,
                output_tokens: 4_000,
                cache_creation_tokens: 0,
                cache_creation_1h_tokens: 0,
                cache_read_tokens: 60_000,
                web_search_requests: 0,
            },
        );
        assert!(
            (l.cost_usd - expected).abs() < 1e-9,
            "codex line cost {} vs expected {}",
            l.cost_usd,
            expected
        );
        assert_eq!(b.total_cache_read_tokens, 60_000);
        assert!((b.agent_cost_usd - expected).abs() < 1e-9);
    }
}

#[cfg(test)]
mod range_breakdown_tests {
    use super::*;

    fn ts(iso: &str) -> i64 {
        parse_iso_ms(iso).expect("parse ts")
    }

    fn session_skeleton(source: &str, created_ms: i64, last_activity_ms: i64) -> serde_json::Value {
        serde_json::json!({
            "id": "x",
            "workspacePath": "/tmp",
            "workspaceName": "tmp",
            "isSubagent": false,
            "parentSessionId": null,
            "agentType": null,
            "agentDescription": null,
            "slug": null,
            "aiTitle": null,
            "status": "waitingInput",
            "tokenSpeed": 0.0,
            "agentTokenSpeed": 0.0,
            "totalOutputTokens": 0,
            "totalInputTokens": 0,
            "totalCostUsd": 0.0,
            "agentTotalCostUsd": 0.0,
            "costSpeedUsdPerMin": 0.0,
            "lastMessagePreview": null,
            "lastActivityMs": last_activity_ms.max(0),
            "createdAtMs": created_ms.max(0),
            "jsonlPath": "/tmp/x.jsonl",
            "model": null,
            "thinkingLevel": null,
            "pid": null,
            "pidPrecise": false,
            "agentSource": source
        })
    }

    /// The projection fold (disk read + JSON parse) must run only when a
    /// session's usage changed. Seed a distinctive cell for a HIT, prove the
    /// bogus `jsonl_path` is never read while the fingerprint holds, then bump
    /// usage and prove the change forces a re-fold (which reads the bogus path →
    /// empty cells).
    #[test]
    fn cache_reuses_on_matching_fingerprint_and_refolds_on_change() {
        let mut s: SessionInfo =
            serde_json::from_value(session_skeleton("claude-code", 1000, 2000)).unwrap();
        s.id = "sess-cache".to_string();
        s.jsonl_path = "/nonexistent/never-read.jsonl".to_string();
        s.last_activity_ms = 100;
        s.total_input_tokens = 5;
        s.total_output_tokens = 2;

        let mut cache = UsageBreakdownCache::default();

        // Pre-seed a distinctive cell under this session's current fingerprint.
        let mut seeded = SessionCells::new();
        let mut acc = LineAcc::default();
        acc.add(11, 0, 0, 0, 7, 0.25);
        seeded.insert(("2026-07-21".to_string(), "m".to_string()), acc);
        cache.entries.insert(
            s.id.clone(),
            CacheEntry {
                fingerprint: UsageBreakdownCache::fingerprint(&s),
                cells: seeded,
            },
        );

        // HIT: fingerprint matches → seeded cell returned, bogus path untouched.
        {
            let cells = cache.cells(&s);
            assert_eq!(cells.len(), 1);
            assert_eq!(
                cells
                    .get(&("2026-07-21".to_string(), "m".to_string()))
                    .unwrap()
                    .output,
                7
            );
        }

        // MISS: bump usage → fingerprint changes → re-fold reads the bogus path
        // → empty cells (a real changed session would re-parse fresh content).
        s.total_output_tokens = 3;
        assert!(
            cache.cells(&s).is_empty(),
            "a fingerprint change must force a re-fold"
        );
    }

    /// The on-disk cache must survive a round-trip byte-for-byte and discard a
    /// file written under a different schema version.
    #[test]
    fn disk_cache_round_trips_and_rejects_version_mismatch() {
        let mut cache = UsageBreakdownCache::default();
        let mut cells = SessionCells::new();
        let mut acc = LineAcc::default();
        acc.add(100, 5, 3, 50, 20, 1.5);
        cells.insert(("2026-07-21".to_string(), "claude-opus-4-8".to_string()), acc);
        cache.entries.insert(
            "s1".to_string(),
            CacheEntry {
                fingerprint: (9, 8, 7),
                cells,
            },
        );

        let dir = std::env::temp_dir().join("fleet-usage-cache-roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cache.json");
        cache.store_to(&path);

        let loaded = UsageBreakdownCache::load_from(&path);
        let e = loaded.entries.get("s1").expect("entry survived round-trip");
        assert_eq!(e.fingerprint, (9, 8, 7));
        let acc = e
            .cells
            .get(&("2026-07-21".to_string(), "claude-opus-4-8".to_string()))
            .expect("cell survived round-trip");
        assert_eq!(
            (acc.input, acc.cache_creation, acc.cache_read, acc.output),
            (100, 5, 50, 20)
        );
        assert!((acc.cost - 1.5).abs() < 1e-9);

        // A file tagged with a different schema version invalidates the whole cache.
        let mut persisted = cache.to_persisted();
        persisted.version = CACHE_SCHEMA_VERSION + 1;
        std::fs::write(&path, serde_json::to_vec(&persisted).unwrap()).unwrap();
        assert!(
            UsageBreakdownCache::load_from(&path).entries.is_empty(),
            "version mismatch must invalidate the whole cache"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Claude session whose JSONL holds `jsonl`, with explicit created /
    /// last-activity so the window prune is exercised realistically.
    fn claude_session(tag: &str, created_ms: i64, last_activity_ms: i64, jsonl: &str) -> SessionInfo {
        let dir = std::env::temp_dir().join(format!("fleet-range-test-{tag}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        std::fs::write(&path, jsonl).unwrap();
        let mut s: SessionInfo =
            serde_json::from_value(session_skeleton("claude-code", created_ms, last_activity_ms))
                .expect("construct SessionInfo");
        s.created_at_ms = created_ms.max(0) as u64;
        s.last_activity_ms = last_activity_ms.max(0) as u64;
        s.jsonl_path = path.to_string_lossy().to_string();
        s.agent_source = "claude-code".to_string();
        s
    }

    fn turn_at(
        id: &str,
        iso: &str,
        model: &str,
        input: u64,
        cc: u64,
        cr: u64,
        output: u64,
    ) -> String {
        serde_json::json!({
            "type": "assistant",
            "timestamp": iso,
            "message": {
                "id": id,
                "model": model,
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": input,
                    "cache_creation_input_tokens": cc,
                    "cache_read_input_tokens": cr,
                    "output_tokens": output
                }
            }
        })
        .to_string()
    }

    #[test]
    fn turns_bucket_per_day_and_reconcile() {
        // Two turns 48h apart → two distinct local days in any timezone.
        let d1 = "2026-07-15T10:00:00Z";
        let d2 = "2026-07-17T10:00:00Z";
        let jsonl = [
            turn_at("a", d1, "claude-sonnet-4-5", 1000, 0, 0, 100),
            turn_at("b", d2, "claude-sonnet-4-5", 2000, 0, 0, 200),
        ]
        .join("\n");
        let from = ts(d1) - 1000;
        let to = ts(d2) + 1000;
        let s = claude_session("twoday", from, to, &jsonl);
        let b = build_range_breakdown(&[s], from, to);

        assert_eq!(b.daily.len(), 2, "one point per day");
        assert!(b.daily[0].date <= b.daily[1].date, "ascending by date");
        assert_eq!(b.daily[0].date, local_date_str(ts(d1)));
        assert_eq!(b.daily[1].date, local_date_str(ts(d2)));
        assert_eq!(b.daily[0].input_tokens, 1000);
        assert_eq!(b.daily[1].input_tokens, 2000);
        assert_eq!(b.total_input_tokens, 3000);
        assert_eq!(b.total_output_tokens, 300);
        assert!(!b.has_codex_approximation);

        // Σ daily cost == grand total, and == Σ line cost.
        let daily_sum: f64 = b.daily.iter().map(|p| p.cost_usd).sum();
        let line_sum: f64 = b.lines.iter().map(|l| l.cost_usd).sum();
        assert!((daily_sum - b.total_cost_usd).abs() < 1e-9);
        assert!((line_sum - b.total_cost_usd).abs() < 1e-9);
    }

    #[test]
    fn out_of_window_turns_excluded_but_spanning_session_still_read() {
        // Session created before `from` and active after `to` — must be read
        // (not pruned) and filtered per turn: only the middle turn counts.
        let before = "2026-07-10T10:00:00Z";
        let inside = "2026-07-15T10:00:00Z";
        let after = "2026-07-20T10:00:00Z";
        let jsonl = [
            turn_at("x", before, "claude-haiku-4-5", 5000, 0, 0, 500),
            turn_at("y", inside, "claude-haiku-4-5", 1000, 0, 0, 100),
            turn_at("z", after, "claude-haiku-4-5", 9000, 0, 0, 900),
        ]
        .join("\n");
        let from = ts(inside) - 3_600_000; // 1h before
        let to = ts(inside) + 3_600_000; // 1h after
        let s = claude_session("filter", ts(before), ts(after), &jsonl);
        let b = build_range_breakdown(&[s], from, to);

        assert_eq!(b.lines.len(), 1);
        assert_eq!(b.daily.len(), 1, "only the in-window day");
        assert_eq!(b.total_input_tokens, 1000, "only the inside turn");
        assert_eq!(b.total_output_tokens, 100);
    }

    #[test]
    fn session_entirely_before_window_contributes_nothing() {
        // last_activity before `from` → pruned; even if read, its lone turn is
        // dated before `from` and filtered out. Either way: no contribution.
        let old = "2026-07-01T10:00:00Z";
        let jsonl = turn_at("p", old, "claude-opus-4-8", 1_000_000, 0, 0, 1_000_000);
        let s = claude_session("prune", ts(old), ts(old), &jsonl);
        let from = ts("2026-07-15T00:00:00Z");
        let to = ts("2026-07-16T00:00:00Z");
        let b = build_range_breakdown(&[s], from, to);

        assert!(b.lines.is_empty());
        assert!(b.daily.is_empty());
        assert_eq!(b.total_cost_usd, 0.0);
    }

    /// A minimal codex rollout: one `turn_context` (model) + one cumulative
    /// `token_count` event, written to an absolute temp file, returned as a
    /// `codex://<abs>` URI SessionInfo (created within `created_ms`'s day).
    fn codex_session(tag: &str, created_ms: i64, model: &str, raw: u64, cached: u64, output: u64) -> SessionInfo {
        let jsonl = [
            serde_json::json!({ "type": "turn_context", "payload": { "model": model } }).to_string(),
            serde_json::json!({
                "type": "event_msg",
                "payload": { "type": "token_count", "info": { "total_token_usage": {
                    "input_tokens": raw, "cached_input_tokens": cached, "output_tokens": output
                }}}
            })
            .to_string(),
        ]
        .join("\n");
        let dir = std::env::temp_dir().join(format!("fleet-range-test-{tag}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rollout.jsonl");
        std::fs::write(&path, &jsonl).unwrap();
        let mut s: SessionInfo =
            serde_json::from_value(session_skeleton("codex", created_ms, created_ms))
                .expect("construct SessionInfo");
        s.created_at_ms = created_ms.max(0) as u64;
        s.last_activity_ms = created_ms.max(0) as u64;
        s.jsonl_path = format!("codex://{}", path.to_string_lossy());
        s.agent_source = "codex".to_string();
        s
    }

    #[test]
    fn codex_session_attributed_whole_to_creation_day_and_flagged() {
        let created = ts("2026-07-15T12:00:00Z");
        let s = codex_session("codexin", created, "gpt-5.6-sol", 100_000, 60_000, 4_000);
        let from = ts("2026-07-14T00:00:00Z");
        let to = ts("2026-07-16T00:00:00Z");
        let b = build_range_breakdown(&[s], from, to);

        assert!(b.has_codex_approximation, "codex contributed → flag set");
        assert_eq!(b.lines.len(), 1);
        assert_eq!(b.lines[0].source, "codex");
        assert_eq!(b.lines[0].input_tokens, 40_000, "full-price = raw − cached");
        assert_eq!(b.lines[0].cache_read_tokens, 60_000);
        assert_eq!(b.daily.len(), 1);
        assert_eq!(b.daily[0].date, local_date_str(created), "trend at creation day");
    }

    #[test]
    fn codex_session_created_outside_window_excluded() {
        let created = ts("2026-07-01T12:00:00Z"); // before window
        let s = codex_session("codexout", created, "gpt-5.6-sol", 100_000, 60_000, 4_000);
        let from = ts("2026-07-14T00:00:00Z");
        let to = ts("2026-07-16T00:00:00Z");
        let b = build_range_breakdown(&[s], from, to);

        assert!(b.lines.is_empty());
        assert!(b.daily.is_empty());
        assert!(!b.has_codex_approximation);
    }

    #[test]
    /// The sidebar's "今日累计" must not count Fleet's own overhead either. This
    /// one drives the real `today_usage()` (which reads
    /// `$FLEET_HOME/.fleet/fleet_llm_usage.jsonl`) rather than a pure helper, so
    /// it seeds a today-stamped entry under a temp home and asserts the badge
    /// stays at zero with no agent sessions.
    #[test]
    fn today_total_excludes_fleet_own_spend() {
        use crate::llm_usage::{append_usage_entry, FleetLlmUsageEntry};
        let _lock = crate::session::fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("FLEET_HOME", tmp.path());

        append_usage_entry(&FleetLlmUsageEntry {
            timestamp_ms: chrono::Local::now().timestamp_millis() as u64,
            scenario: "guard_command".to_string(),
            provider: "claude".to_string(),
            model: "haiku".to_string(),
            input_tokens: 10,
            output_tokens: 465,
            cache_creation_tokens: 30_426,
            cache_creation_1h_tokens: 30_426,
            cache_read_tokens: 17_464,
            duration_ms: 0,
            cost_usd: 0.0662,
            token_accurate: true,
            cost_accurate: true,
        });

        let u = today_usage(&[]);
        std::env::remove_var("FLEET_HOME");

        assert!((u.cost_usd - 0.0).abs() < 1e-9, "今日累计 counted Fleet: ${}", u.cost_usd);
        assert!((u.fleet_cost_usd - 0.0).abs() < 1e-9);
        assert_eq!(u.input_tokens, 0, "Fleet's input tokens leaked in");
        assert_eq!(u.output_tokens, 0, "Fleet's output tokens leaked in");
    }

    /// Same exclusion on the "today" preset, which is the口径 the sidebar badge
    /// reconciles against. Drives the public `today_usage_breakdown` under a temp
    /// `$FLEET_HOME` holding a real fleet entry — so it proves the receipt never
    /// consults `fleet_llm_usage.jsonl`, not merely that a parameter is unused.
    #[test]
    fn today_receipt_excludes_fleet_own_spend() {
        let _lock = crate::session::fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("FLEET_HOME", tmp.path());
        seed_fleet_entry(chrono::Local::now().timestamp_millis());

        let b = today_usage_breakdown(&[]);
        std::env::remove_var("FLEET_HOME");

        assert!(b.lines.is_empty(), "fleet opened a line on today's receipt");
        assert!((b.total_cost_usd - 0.0).abs() < 1e-9);
        assert_eq!(b.total_output_tokens, 0, "fleet tokens leaked into the header");
    }

    /// Fleet's own LLM calls (guard analysis, audit-rule suggestions, report
    /// summaries, session outcome analysis) are Fleet's operational overhead, not
    /// the user's agent spend, and their accounting can't be made to reconcile:
    /// entries logged before TTL-aware accounting recorded a truncated input
    /// figure against a fully-billed cost, and Codex-provider calls are logged
    /// unpriced (cost 0.0) with char-estimated tokens. So the receipt is
    /// agent-only. Fleet's own consumption stays visible in Settings → Usage,
    /// which reads the raw `fleet_llm_usage.jsonl` buckets directly.
    #[test]
    fn range_receipt_excludes_fleet_own_spend() {
        let _lock = crate::session::fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("FLEET_HOME", tmp.path());
        let when = chrono::Local::now().timestamp_millis();
        seed_fleet_entry(when);

        let b = usage_range_breakdown(&[], when - 86_400_000, when + 1000);
        std::env::remove_var("FLEET_HOME");

        assert!(
            b.lines.iter().all(|l| l.source != "fleet"),
            "fleet line still on the receipt: {:?}",
            b.lines.iter().map(|l| &l.source).collect::<Vec<_>>()
        );
        assert!((b.fleet_cost_usd - 0.0).abs() < 1e-9, "fleet cost leaked into the split");
        assert!(
            b.daily.iter().all(|d| d.cost_usd == 0.0),
            "fleet spend leaked into the daily trend"
        );
    }

    /// Append one today-stamped Fleet entry to `$FLEET_HOME/.fleet/`. Numbers are
    /// a real logged guard call (Haiku 4.5, 1h cache writes).
    fn seed_fleet_entry(timestamp_ms: i64) {
        crate::llm_usage::append_usage_entry(&crate::llm_usage::FleetLlmUsageEntry {
            timestamp_ms: timestamp_ms.max(0) as u64,
            scenario: "guard_command".to_string(),
            provider: "claude".to_string(),
            model: "haiku".to_string(),
            input_tokens: 10,
            output_tokens: 465,
            cache_creation_tokens: 30_426,
            cache_creation_1h_tokens: 30_426,
            cache_read_tokens: 17_464,
            duration_ms: 0,
            cost_usd: 0.0662,
            token_accurate: true,
            cost_accurate: true,
        });
    }


    /// A `<synthetic>` control turn must not open a receipt line of its own:
    /// it is not a model, so `get_model_costs` prices it at the unknown-model
    /// fallback. Its usage belongs to the conversation's real model — the same
    /// rule `session::parse` already applies when picking a session's model.
    #[test]
    fn synthetic_control_turns_fold_under_the_real_model() {
        let jsonl = concat!(
            r#"{"type":"assistant","timestamp":"2026-07-20T10:00:00.000Z","message":{"id":"a","model":"claude-opus-4-8","stop_reason":"end_turn","usage":{"input_tokens":100,"output_tokens":20,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-07-20T10:01:00.000Z","message":{"id":"b","model":"<synthetic>","stop_reason":"end_turn","usage":{"input_tokens":7,"output_tokens":3,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}"#,
        );
        let cells = fold_claude_session_cells(jsonl);
        let models: Vec<&str> = cells.keys().map(|(_, m)| m.as_str()).collect();
        assert!(
            !models.contains(&"<synthetic>"),
            "control turn opened its own line: {models:?}"
        );
        let acc = cells
            .get(&("2026-07-20".to_string(), "claude-opus-4-8".to_string()))
            .expect("real-model cell");
        assert_eq!(acc.input, 107, "control turn's usage folds into the real model");
        assert_eq!(acc.output, 23);
    }

    /// The report DB stores `inputTokens` as `Σ(input + cache_write +
    /// cache_read)` — every token sent to the API, matching its `cost_usd`. The
    /// receipt itemises input separately from the two cache rows, so the fold
    /// has to net them out. Left inclusive, the Input row shows the whole
    /// API-side volume at the full input price *and* the cache rows list it
    /// again (opus-4-8 over 30 days: a $92k itemisation under a $17.6k
    /// subtotal).
    #[test]
    fn report_fold_nets_cache_out_of_the_input_row() {
        use crate::daily_report::ModelTokens;
        let mt = ModelTokens {
            // 700 net input + 200 cache writes + 100 cache reads = 1000 sent.
            input_tokens: 1000,
            output_tokens: 50,
            cache_creation_tokens: 200,
            cache_creation_1h_tokens: 200,
            cache_read_tokens: 100,
            cost_usd: 1.23,
        };
        let mut by_model = std::collections::HashMap::new();
        let mut by_day = std::collections::BTreeMap::new();
        fold_report_model(
            "2026-07-01",
            "claude-opus-4-8",
            &mt,
            &mut by_model,
            &mut by_day,
        );

        let acc = by_model
            .get(&("claude-code".to_string(), "claude-opus-4-8".to_string()))
            .expect("line keyed by inferred source");
        assert_eq!(acc.input, 700, "input row must be net of both cache figures");
        assert_eq!(acc.cache_creation, 200);
        assert_eq!(acc.cache_creation_1h, 200);
        assert_eq!(acc.cache_read, 100);
        assert!((acc.cost - 1.23).abs() < 1e-9, "stored cost passes through");
        assert_eq!(by_day["2026-07-01"].input, 700, "the trend nets it too");
    }

    /// Report rows already on disk keyed under `<synthetic>` hold real money from
    /// a session whose model was never recorded. They can't be re-attributed, so
    /// they must at least stop masquerading as a model: collapse to `unknown`.
    #[test]
    fn report_fold_collapses_placeholder_models_to_unknown() {
        use crate::daily_report::ModelTokens;
        let mt = ModelTokens {
            input_tokens: 1000,
            output_tokens: 50,
            cache_creation_tokens: 200,
            cache_creation_1h_tokens: 0,
            cache_read_tokens: 100,
            cost_usd: 62.42,
        };
        let mut by_model = std::collections::HashMap::new();
        let mut by_day = std::collections::BTreeMap::new();
        fold_report_model("2026-07-24", "<synthetic>", &mt, &mut by_model, &mut by_day);
        let keys: Vec<&(String, String)> = by_model.keys().collect();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].1, "unknown", "placeholder must not name a line");
        // The money stays on the receipt — the grand total is derived from lines.
        assert!((by_model.values().next().unwrap().cost - 62.42).abs() < 1e-9);
    }

    /// A legacy report whose `input_tokens` predates the all-inclusive口径 can be
    /// smaller than its own cache figures; the fold must clamp to 0 rather than
    /// wrap a u64 subtraction into ~1.8e19 tokens.
    #[test]
    fn report_fold_clamps_when_input_is_smaller_than_cache() {
        use crate::daily_report::ModelTokens;
        let mt = ModelTokens {
            input_tokens: 50,
            output_tokens: 10,
            cache_creation_tokens: 200,
            cache_creation_1h_tokens: 0,
            cache_read_tokens: 900,
            cost_usd: 0.5,
        };
        let mut by_model = std::collections::HashMap::new();
        let mut by_day = std::collections::BTreeMap::new();
        fold_report_model("2026-04-20", "claude-opus-4-8", &mt, &mut by_model, &mut by_day);
        let acc = by_model.values().next().unwrap();
        assert_eq!(acc.input, 0, "clamped, not wrapped");
    }

    /// **The receipt invariant:** every itemised row the UI renders is
    /// `tokens × unit price`, so their sum must equal the line's own subtotal.
    /// A line whose cache writes are 1-hour TTL breaks that unless the line
    /// exposes the two write rates separately — the UI can't price a blended
    /// bucket. 1M of Haiku 4.5 1h writes costs $2.00; billed at the 5-minute
    /// $1.25/M the rows would only add up to $1.25 under a $2.00 subtotal.
    #[test]
    fn receipt_line_rows_sum_to_its_subtotal() {
        // A real transcript turn whose cache writes are all 1-hour TTL — the
        // shape Claude Code actually produces. Opus 4.8: 1M 1h writes = $10.00,
        // plus 1M input ($5) and 1M output ($25).
        let jsonl = concat!(
            r#"{"type":"assistant","timestamp":"2026-07-15T10:00:00.000Z","message":{"id":"a","model":"claude-opus-4-8","#,
            r#""stop_reason":"end_turn","usage":{"input_tokens":1000000,"output_tokens":1000000,"#,
            r#""cache_creation_input_tokens":1000000,"cache_read_input_tokens":0,"#,
            r#""cache_creation":{"ephemeral_1h_input_tokens":1000000,"ephemeral_5m_input_tokens":0}}}}"#,
        );
        let from = ts("2026-07-14T00:00:00Z");
        let to = ts("2026-07-16T00:00:00Z");
        let sess = claude_session("rows-reconcile", ts("2026-07-15T09:00:00Z"), to, jsonl);
        let b = build_range_breakdown(&[sess], from, to);

        let l = &b.lines[0];
        let per_m = |tok: u64, price: f64| (tok as f64 / 1_000_000.0) * price;
        let rows = per_m(l.input_tokens, l.input_price)
            + per_m(l.cache_creation_tokens, l.cache_write_price)
            + per_m(l.cache_creation_1h_tokens, l.cache_write_1h_price)
            + per_m(l.cache_read_tokens, l.cache_read_price)
            + per_m(l.output_tokens, l.output_price);
        assert!(
            (rows - l.cost_usd).abs() < 1e-9,
            "rows ${rows} vs subtotal ${}",
            l.cost_usd
        );
        assert!((l.cost_usd - 40.0).abs() < 1e-9, "expected $40.00, got ${}", l.cost_usd);
    }


    /// The cache-ready `(date, model)` cells must reconstruct the receipt
    /// bit-for-bit: date-window sum == `fold_session_turns_range` (undated turns
    /// dropped, per-day trend preserved). Every window — today included — goes
    /// through this one fold.
    #[test]
    fn session_cells_reproduce_the_receipt_folder() {
        // Dated turns on two days, one undated turn, a duplicate id, and a
        // non-finalized turn — every branch the two folders special-case.
        let jsonl = concat!(
            r#"{"type":"assistant","timestamp":"2026-07-20T10:00:00.000Z","message":{"id":"a","model":"claude-opus-4-8","stop_reason":"end_turn","usage":{"input_tokens":100,"output_tokens":20,"cache_creation_input_tokens":5,"cache_read_input_tokens":50}}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-07-21T10:00:00.000Z","message":{"id":"b","model":"claude-opus-4-8","stop_reason":"end_turn","usage":{"input_tokens":200,"output_tokens":40,"cache_creation_input_tokens":0,"cache_read_input_tokens":10}}}"#,
            "\n",
            r#"{"type":"assistant","message":{"id":"c","model":"claude-sonnet-5","stop_reason":"end_turn","usage":{"input_tokens":7,"output_tokens":3}}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-07-21T11:00:00.000Z","message":{"id":"b","model":"claude-opus-4-8","stop_reason":"end_turn","usage":{"input_tokens":999,"output_tokens":999}}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-07-21T12:00:00.000Z","message":{"id":"d","model":"claude-opus-4-8","stop_reason":null,"usage":{"input_tokens":999,"output_tokens":999}}}"#,
            "\n",
        );

        let cells = fold_claude_session_cells(jsonl);

        // date-window sum == fold_session_turns_range. The window
        // brackets both dated turns with 2 days of margin each side, so ms- and
        // date-filtering agree; the undated turn `c` is dropped by both.
        let from = ts("2026-07-18T00:00:00Z");
        let to = ts("2026-07-23T00:00:00Z");
        let mut old_model = std::collections::HashMap::new();
        let mut old_day = std::collections::BTreeMap::new();
        fold_session_turns_range(jsonl, "claude-code", from, to, &mut old_model, &mut old_day);
        let mut new_model = std::collections::HashMap::new();
        let mut new_day = std::collections::BTreeMap::new();
        sum_cells_window(
            &cells,
            "claude-code",
            &local_date_str(from),
            &local_date_str(to),
            &mut new_model,
            &mut new_day,
        );
        assert_eq!(snap(&old_model), snap(&new_model), "range by_model mismatch");
        assert_eq!(snap_day(&old_day), snap_day(&new_day), "range by_day mismatch");

        // The undated turn `c` lands in the "" bucket and never reaches the trend.
        assert!(cells.keys().any(|(d, _)| d.is_empty()), "undated turn not bucketed");
        assert!(!new_day.keys().any(|d| d.is_empty()), "range trend leaked an undated turn");
    }

    /// Cache writes carry a TTL and Anthropic prices the two tiers differently:
    /// an `ephemeral_5m` write costs 1.25× the model's input rate, an
    /// `ephemeral_1h` write costs 2×. The numbers below come from a real
    /// `claude -p --model haiku --output-format json` probe (2026-07-25) whose
    /// `usage.cache_creation` was 100% `ephemeral_1h_input_tokens`; the CLI
    /// reported `total_cost_usd = 0.0626776`, which is exactly
    /// `530×$1 + 56×$5 + 30057×$2.00 + 17536×$0.10`. Pricing that same turn at
    /// the 5-minute rate ($1.25/M) yields $0.0401 — a 36% undercount.
    #[test]
    fn one_hour_cache_writes_priced_at_2x_input() {
        let jsonl = concat!(
            r#"{"type":"assistant","timestamp":"2026-07-25T10:00:00.000Z","message":{"id":"a","model":"claude-haiku-4-5","#,
            r#""stop_reason":"end_turn","usage":{"input_tokens":530,"output_tokens":56,"#,
            r#""cache_creation_input_tokens":30057,"cache_read_input_tokens":17536,"#,
            r#""cache_creation":{"ephemeral_1h_input_tokens":30057,"ephemeral_5m_input_tokens":0}}}}"#,
        );
        let cells = fold_claude_session_cells(jsonl);
        let cost: f64 = cells.values().map(|a| a.cost).sum();
        assert!(
            (cost - 0.0626776).abs() < 1e-9,
            "expected the CLI's own $0.0626776, got ${cost}"
        );
    }

    /// A turn that mixes both TTLs must bill each portion at its own rate:
    /// 1M input + 1M output on Sonnet 5, with 1M of cache writes split evenly,
    /// = $3 + $15 + 0.5M×$3.75 + 0.5M×$6.00 = $22.875.
    #[test]
    fn mixed_ttl_cache_writes_split_by_rate() {
        let jsonl = concat!(
            r#"{"type":"assistant","timestamp":"2026-07-25T10:00:00.000Z","message":{"id":"a","model":"claude-sonnet-5","#,
            r#""stop_reason":"end_turn","usage":{"input_tokens":1000000,"output_tokens":1000000,"#,
            r#""cache_creation_input_tokens":1000000,"cache_read_input_tokens":0,"#,
            r#""cache_creation":{"ephemeral_1h_input_tokens":500000,"ephemeral_5m_input_tokens":500000}}}}"#,
        );
        let cells = fold_claude_session_cells(jsonl);
        let cost: f64 = cells.values().map(|a| a.cost).sum();
        assert!((cost - 22.875).abs() < 1e-9, "expected $22.875, got ${cost}");
    }

    /// Normalize a `by_model` map to a comparable snapshot (cost → micro-USD int
    /// so f64 summation order doesn't fail the equality).
    fn snap(
        m: &std::collections::HashMap<(String, String), LineAcc>,
    ) -> std::collections::BTreeMap<(String, String), (u64, u64, u64, u64, i64)> {
        m.iter()
            .map(|(k, a)| {
                (
                    k.clone(),
                    (
                        a.input,
                        a.cache_creation,
                        a.cache_read,
                        a.output,
                        (a.cost * 1e6).round() as i64,
                    ),
                )
            })
            .collect()
    }

    fn snap_day(
        m: &std::collections::BTreeMap<String, LineAcc>,
    ) -> std::collections::BTreeMap<String, (u64, u64, u64, u64, i64)> {
        m.iter()
            .map(|(k, a)| {
                (
                    k.clone(),
                    (
                        a.input,
                        a.cache_creation,
                        a.cache_read,
                        a.output,
                        (a.cost * 1e6).round() as i64,
                    ),
                )
            })
            .collect()
    }
}
