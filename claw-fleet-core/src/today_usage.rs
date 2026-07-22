//! "Today's cumulative usage" aggregation for the desktop nav-bar / mobile
//! header counter.
//!
//! Attribution口径 (decided by Boss 2026-07-12): "today" = sessions **created
//! today** (local timezone), summing each session's own live `total_cost_usd` /
//! `total_output_tokens` (the incremental figures `StatsAcc` folds off the JSONL
//! tail — the freshest source), **plus** Fleet's own LLM spend today (guard /
//! audit / daily-report summaries, logged in `fleet_llm_usage.jsonl`).
//!
//! Every session contributes its own cost exactly once — we sum
//! `SessionInfo.total_cost_usd`, never `agent_total_cost_usd` (the roll-up that
//! already folds in subagents and would double-count).
//!
//! No new pricing math lives here: session costs come pre-computed by
//! `session::StatsAcc` (via `model_cost::turn_cost_usd`), and Fleet's spend comes
//! from `llm_usage` (also via `turn_cost_usd`).

use serde::{Deserialize, Serialize};

use crate::session::SessionInfo;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TodayUsage {
    /// YYYY-MM-DD in the user's local timezone.
    pub date: String,
    /// Cumulative input tokens today (input + cache creation + cache read, cache
    /// re-reads included) — the "tokens sent to the API" total, on the same口径
    /// as cost. Per agent session it's the cumulative `SessionInfo.total_input_tokens`
    /// (for both Claude and Codex sources, which now agree); plus Fleet's own LLM
    /// calls' input + cache tokens. Summed alongside `output_tokens`. NOTE: this
    /// is cache-read-dominated and can reach billions/day — it is NOT the daily
    /// report's old last-turn snapshot (the report now sums cumulatively too, so
    /// both surfaces agree on口径, though the sidebar also counts Codex + Fleet
    /// which the Claude-only report does not).
    pub input_tokens: u64,
    /// Output tokens today: agent sessions created today + Fleet's own LLM calls.
    pub output_tokens: u64,
    /// Total USD cost today = `agent_cost_usd` + `fleet_cost_usd`.
    pub cost_usd: f64,
    /// Cost from agent (Claude Code) sessions created today.
    pub agent_cost_usd: f64,
    /// Cost from Fleet's own LLM calls today (guard / audit / report summaries).
    pub fleet_cost_usd: f64,
    /// Number of top-level (non-subagent) sessions created today that contributed.
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

/// Sum the live cost / input- / output-token totals of every session **created
/// today**. Returns `(cost_usd, input_tokens, output_tokens,
/// top_level_session_count)`. Pure — the unit tests drive this directly.
fn sum_today_sessions(sessions: &[SessionInfo], day_start_ms: i64) -> (f64, u64, u64, u64) {
    let start = day_start_ms.max(0) as u64;
    let mut cost = 0.0;
    let mut input = 0u64;
    let mut output = 0u64;
    let mut count = 0u64;
    for s in sessions {
        if s.created_at_ms < start {
            continue;
        }
        cost += s.total_cost_usd;
        input = input.saturating_add(s.total_input_tokens);
        output = output.saturating_add(s.total_output_tokens);
        if !s.is_subagent {
            count += 1;
        }
    }
    (cost, input, output, count)
}

/// Aggregate today's cumulative usage from an already-scanned session list.
///
/// `sessions` is whatever the caller already has (e.g. `scan_all_sources`), so
/// this adds no extra JSONL scan. Fleet's own spend is read from
/// `fleet_llm_usage.jsonl` for today's local-day window.
pub fn today_usage(sessions: &[SessionInfo]) -> TodayUsage {
    let now_ms = chrono::Local::now().timestamp_millis();
    let (day_start_ms, day_end_ms, date) = day_bounds_ms(now_ms);

    let (agent_cost_usd, mut input_tokens, mut output_tokens, session_count) =
        sum_today_sessions(sessions, day_start_ms);

    // Fleet's own LLM spend today. The window is by timestamp; every returned
    // bucket is therefore today's local day. Sum across all scenarios. Input is
    // input + cache creation + cache read, matching how agent sessions' input
    // (`total_input_tokens`) already folds in cache.
    let mut fleet_cost_usd = 0.0;
    for b in crate::llm_usage::list_usage_daily_buckets(day_start_ms as u64, day_end_ms as u64) {
        fleet_cost_usd += b.cost_usd;
        input_tokens = input_tokens
            .saturating_add(b.input_tokens)
            .saturating_add(b.cache_creation_tokens)
            .saturating_add(b.cache_read_tokens);
        output_tokens = output_tokens.saturating_add(b.output_tokens);
    }

    TodayUsage {
        date,
        input_tokens,
        output_tokens,
        cost_usd: agent_cost_usd + fleet_cost_usd,
        agent_cost_usd,
        fleet_cost_usd,
        session_count,
    }
}

/// Consolidated per-container token usage for the Fleet Cloud lean deployment.
///
/// One customer per container, so this container's usage **is** the customer's
/// usage. `today` reuses [`today_usage`] (includes Fleet's own LLM overhead for
/// the operational view); the `cumulative_*` fields sum **agent sessions only**
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
    /// Today's window (agent + Fleet overhead), same shape as `/today_usage`.
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
// today** (every SessionInfo, subagents included, each counted once) plus
// Fleet's own LLM spend — so `Σ line.cost_usd == TodayUsage.cost_usd` to the
// cent. Because per-model pricing is linear, folding a model's tokens and
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
    pub input_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub output_tokens: u64,
    /// Official unit prices in USD per 1M tokens, for the receipt's "@ $X/M"
    /// column. Sourced from [`crate::model_cost::get_model_costs`].
    pub input_price: f64,
    pub output_price: f64,
    pub cache_write_price: f64,
    pub cache_read_price: f64,
    /// Line cost = Σ per-turn `turn_cost_usd` for this (source, model).
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
    cache_read: u64,
    output: u64,
    cost: f64,
}

impl LineAcc {
    /// Fold one turn's tokens + cost into this accumulator.
    fn add(&mut self, input: u64, cache_creation: u64, cache_read: u64, output: u64, cost: f64) {
        self.input += input;
        self.cache_creation += cache_creation;
        self.cache_read += cache_read;
        self.output += output;
        self.cost += cost;
    }
}

/// Fold every finalized assistant turn of one session's JSONL into `by_model`,
/// keyed by `(source, per-turn model)`. Dedups by message id exactly like
/// [`crate::session::StatsAcc`] so a re-logged turn isn't double-counted.
#[cfg(test)]
fn fold_session_turns(
    jsonl: &str,
    source: &str,
    by_model: &mut std::collections::HashMap<(String, String), LineAcc>,
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
        // Only finalized turns (matches StatsAcc / extract_session_metrics'口径).
        if msg.get("stop_reason").map_or(true, |s| s.is_null()) {
            continue;
        }
        let msg_id = msg.get("id").and_then(|i| i.as_str()).unwrap_or_default();
        if !msg_id.is_empty() && !seen.insert(msg_id.to_string()) {
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

        let turn_model = msg.get("model").and_then(|m| m.as_str());
        if let Some(m) = turn_model {
            last_model = Some(m.to_string());
        }
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
                cache_read_tokens: cache_read,
                web_search_requests: web_search,
            },
        );

        by_model
            .entry((source.to_string(), model))
            .or_default()
            .add(input, cache_creation, cache_read, output, cost);
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
        .add(bd.input_tokens, 0, bd.cached_input_tokens, bd.output_tokens, bd.cost_usd);
}

/// Build today's per-model receipt on the same口径 as [`today_usage`].
///
/// `sessions` is the already-scanned session list (subagents included). Each
/// session **created today** contributes every finalized turn of its JSONL,
/// folded per model. Fleet's own LLM spend today is folded per model from
/// `fleet_llm_usage.jsonl`. Lines are returned sorted by descending cost.
pub fn today_usage_breakdown(sessions: &[SessionInfo]) -> TodayUsageBreakdown {
    let now_ms = chrono::Local::now().timestamp_millis();
    let (day_start_ms, day_end_ms, _date) = day_bounds_ms(now_ms);
    let fleet_entries =
        crate::llm_usage::list_usage_entries(day_start_ms.max(0) as u64, day_end_ms as u64);
    let mut cache = usage_cache().lock().unwrap();
    cache.retain_sessions(&live_ids(sessions));
    build_breakdown_cached(sessions, &fleet_entries, now_ms, &mut cache)
}

/// Test-only wrapper: a fresh (empty) cache so each call folds from disk, keeping
/// the receipt tests hermetic and independent of the process-wide cache.
#[cfg(test)]
fn build_breakdown(
    sessions: &[SessionInfo],
    fleet_entries: &[crate::llm_usage::FleetLlmUsageEntry],
    now_ms: i64,
) -> TodayUsageBreakdown {
    build_breakdown_cached(
        sessions,
        fleet_entries,
        now_ms,
        &mut UsageBreakdownCache::default(),
    )
}

/// Pure core of [`today_usage_breakdown`], with Fleet's own LLM entries injected
/// so it can be unit-tested without reading `~/.fleet/fleet_llm_usage.jsonl`
/// (mirrors how [`sum_today_sessions`] is the tested pure helper). Each
/// today-created session is projected into cached `(date, model)` cells via
/// `cache` (folded from disk only on a miss); the today receipt sums every cell.
fn build_breakdown_cached(
    sessions: &[SessionInfo],
    fleet_entries: &[crate::llm_usage::FleetLlmUsageEntry],
    now_ms: i64,
    cache: &mut UsageBreakdownCache,
) -> TodayUsageBreakdown {
    use std::collections::HashMap;

    let (day_start_ms, _day_end_ms, date) = day_bounds_ms(now_ms);
    let start = day_start_ms.max(0) as u64;

    let mut by_model: HashMap<(String, String), LineAcc> = HashMap::new();

    // Agent sessions created today (subagents included). Codex and Claude are
    // both projected through the cache — `cells()` dispatches on `agent_source`,
    // so codex's URI-aware reader is still used and an empty projection (empty
    // file / zero-usage rollout) simply contributes nothing. The today receipt
    // sums **every** cell (undated turns included), preserving the sidebar口径.
    for s in sessions {
        if s.created_at_ms < start {
            continue;
        }
        let cells = cache.cells(s);
        sum_cells_all(cells, &s.agent_source, &mut by_model);
    }

    // Fleet's own LLM calls today, folded per model.
    for e in fleet_entries {
        by_model
            .entry(("fleet".to_string(), e.model.clone()))
            .or_default()
            .add(
                e.input_tokens,
                e.cache_creation_tokens,
                e.cache_read_tokens,
                e.output_tokens,
                e.cost_usd,
            );
    }

    let lines = build_lines(by_model);

    let mut total_input = 0u64;
    let mut total_cache_creation = 0u64;
    let mut total_cache_read = 0u64;
    let mut total_output = 0u64;
    let mut agent_cost = 0.0;
    let mut fleet_cost = 0.0;
    for l in &lines {
        total_input = total_input.saturating_add(l.input_tokens);
        total_cache_creation = total_cache_creation.saturating_add(l.cache_creation_tokens);
        total_cache_read = total_cache_read.saturating_add(l.cache_read_tokens);
        total_output = total_output.saturating_add(l.output_tokens);
        if l.source == "fleet" {
            fleet_cost += l.cost_usd;
        } else {
            agent_cost += l.cost_usd;
        }
    }

    TodayUsageBreakdown {
        date,
        lines,
        total_input_tokens: total_input,
        total_cache_creation_tokens: total_cache_creation,
        total_cache_read_tokens: total_cache_read,
        total_output_tokens: total_output,
        total_cost_usd: agent_cost + fleet_cost,
        agent_cost_usd: agent_cost,
        fleet_cost_usd: fleet_cost,
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
            ModelReceiptLine {
                model,
                source,
                input_tokens: acc.input,
                cache_creation_tokens: acc.cache_creation,
                cache_read_tokens: acc.cache_read,
                output_tokens: acc.output,
                input_price: c.input,
                output_price: c.output,
                cache_write_price: c.cache_write,
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
// The today receipt above is hard-scoped to sessions *created today*. The
// Settings "usage" view wants that same per-model itemisation over a longer
// window (7d / 30d / all) plus a per-day trend line. This section generalises
// the fold to an arbitrary inclusive `[from_ms, to_ms]` window.
//
// Attribution differs by source, and this asymmetry is deliberate:
//   • **Claude** sessions are folded **per turn**: each finalized turn is
//     placed on the trend by its own `timestamp`, so a session spanning several
//     days spreads across those days' points — accurate.
//   • **Codex** rollouts expose only a single cumulative `total_token_usage`
//     snapshot per session (no per-turn deltas), so a Codex session is
//     attributed **whole to its creation day**. `has_codex_approximation` flags
//     this so the UI can footnote that Codex trend placement is approximate.
//
// This path is intentionally separate from `today_usage_breakdown`, which must
// stay session-level so `Σ line.cost == TodayUsage.cost` reconciles with the
// sidebar badge to the cent. The range view has no such invariant, so it can
// afford the more accurate per-turn fold.

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
        let turn_model = msg.get("model").and_then(|m| m.as_str());
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
                cache_read_tokens: cache_read,
                web_search_requests: web_search,
            },
        );

        by_model
            .entry((source.to_string(), model))
            .or_default()
            .add(input, cache_creation, cache_read, output, cost);
        by_day
            .entry(local_date_str(ts_ms))
            .or_default()
            .add(input, cache_creation, cache_read, output, cost);
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
        .add(bd.input_tokens, 0, bd.cached_input_tokens, bd.output_tokens, bd.cost_usd);
    by_day
        .entry(local_date_str(attribute_ms))
        .or_default()
        .add(bd.input_tokens, 0, bd.cached_input_tokens, bd.output_tokens, bd.cost_usd);
    true
}

// ── Cache-ready per-session projection ───────────────────────────────────────
//
// Both receipts re-read and re-parse every in-window session's full JSONL on
// every open. To make that cacheable, one session's spend is projected once into
// `(date_bucket, model) -> acc` cells — the smallest form from which BOTH
// receipts reconstruct exactly:
//   * today  sums **every** cell (undated turns included) → same口径 as
//     `fold_session_turns` (whole file, for sessions created today).
//   * range  sums only cells whose non-empty local date is in `[from, to]`,
//     bucketed per day → same口径 as `fold_session_turns_range`.
//
// `date_bucket` is the turn's local `YYYY-MM-DD`, or `""` for a Claude turn with
// no `timestamp` (transcripts predating the field): today counts it, range drops
// it — which is precisely what the two folders above already do. Range windows
// are compared at **date** granularity; that matches the receipt's only caller
// (day-aligned presets `today` / `7d` / `30d` / `all`, always `to = now`), where
// `ts_ms >= from_ms` ⇔ `local_date(ts) >= local_date(from_ms)`.

/// One session's usage as `(date_bucket, model) -> acc`. The agent source is
/// uniform per session, so it is applied at query time and kept out of the key.
type SessionCells = std::collections::HashMap<(String, String), LineAcc>;

/// Project one Claude session's JSONL into `(date, model)` cells in a single
/// pass. Turn selection, msg-id dedup, `last_model` tracking and per-turn cost
/// are identical to [`fold_session_turns`] / [`fold_session_turns_range`]; the
/// only addition is bucketing each folded turn by its local date (`""` when the
/// turn carries no timestamp).
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
        let turn_model = msg.get("model").and_then(|m| m.as_str());
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
                cache_read_tokens: cache_read,
                web_search_requests: web_search,
            },
        );

        cells
            .entry((date, model))
            .or_default()
            .add(input, cache_creation, cache_read, output, cost);
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
        .add(bd.input_tokens, 0, bd.cached_input_tokens, bd.output_tokens, bd.cost_usd);
    cells
}

/// Sum **all** cells (today口径: undated included) into `by_model` under `source`.
fn sum_cells_all(
    cells: &SessionCells,
    source: &str,
    by_model: &mut std::collections::HashMap<(String, String), LineAcc>,
) {
    for ((_, model), acc) in cells {
        by_model
            .entry((source.to_string(), model.clone()))
            .or_default()
            .add(acc.input, acc.cache_creation, acc.cache_read, acc.output, acc.cost);
    }
}

/// Sum cells whose non-empty date is within `[from_date, to_date]` (range口径:
/// undated dropped) into both `by_model` and the per-day trend `by_day`. Dates
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
            .add(acc.input, acc.cache_creation, acc.cache_read, acc.output, acc.cost);
        by_day
            .entry(date.clone())
            .or_default()
            .add(acc.input, acc.cache_creation, acc.cache_read, acc.output, acc.cost);
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
/// want. Production goes through [`usage_cache`].
#[derive(Default)]
struct UsageBreakdownCache {
    entries: std::collections::HashMap<String, CacheEntry>,
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
        }
        &self.entries.get(&s.id).unwrap().cells
    }

    /// Drop cached entries for sessions no longer live, bounding the map to the
    /// current session set (sessions get pruned off disk over time).
    fn retain_sessions(&mut self, live_ids: &std::collections::HashSet<&str>) {
        self.entries.retain(|id, _| live_ids.contains(id.as_str()));
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

/// The process-wide cache backing the public receipt entry points.
fn usage_cache() -> &'static std::sync::Mutex<UsageBreakdownCache> {
    static CACHE: std::sync::LazyLock<std::sync::Mutex<UsageBreakdownCache>> =
        std::sync::LazyLock::new(|| std::sync::Mutex::new(UsageBreakdownCache::default()));
    &CACHE
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
    let fleet_entries =
        crate::llm_usage::list_usage_entries(from_ms.max(0) as u64, to_ms.max(0) as u64);
    let mut cache = usage_cache().lock().unwrap();
    cache.retain_sessions(&live_ids(sessions));
    build_range_breakdown_cached(sessions, &fleet_entries, from_ms, to_ms, &mut cache)
}

/// Test-only wrapper: fresh (empty) cache, so each call folds from disk and the
/// range-receipt tests stay hermetic (see [`build_breakdown`]).
#[cfg(test)]
fn build_range_breakdown(
    sessions: &[SessionInfo],
    fleet_entries: &[crate::llm_usage::FleetLlmUsageEntry],
    from_ms: i64,
    to_ms: i64,
) -> UsageRangeBreakdown {
    build_range_breakdown_cached(
        sessions,
        fleet_entries,
        from_ms,
        to_ms,
        &mut UsageBreakdownCache::default(),
    )
}

/// Pure core of [`usage_range_breakdown`], with Fleet's own LLM entries injected
/// so it can be unit-tested without reading `~/.fleet/fleet_llm_usage.jsonl`.
/// Each in-window session is projected through `cache` (folded from disk only on
/// a miss) and summed over the date window `[from, to]`.
fn build_range_breakdown_cached(
    sessions: &[SessionInfo],
    fleet_entries: &[crate::llm_usage::FleetLlmUsageEntry],
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
                &from_date,
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
            &from_date,
            &to_date,
            &mut by_model,
            &mut by_day,
        );
    }

    // Fleet's own LLM calls in the window (already timestamp-filtered), folded
    // per model and per day.
    for e in fleet_entries {
        by_model
            .entry(("fleet".to_string(), e.model.clone()))
            .or_default()
            .add(
                e.input_tokens,
                e.cache_creation_tokens,
                e.cache_read_tokens,
                e.output_tokens,
                e.cost_usd,
            );
        by_day
            .entry(local_date_str(e.timestamp_ms as i64))
            .or_default()
            .add(
                e.input_tokens,
                e.cache_creation_tokens,
                e.cache_read_tokens,
                e.output_tokens,
                e.cost_usd,
            );
    }

    let lines = build_lines(by_model);

    let mut total_input = 0u64;
    let mut total_cache_creation = 0u64;
    let mut total_cache_read = 0u64;
    let mut total_output = 0u64;
    let mut agent_cost = 0.0;
    let mut fleet_cost = 0.0;
    for l in &lines {
        total_input = total_input.saturating_add(l.input_tokens);
        total_cache_creation = total_cache_creation.saturating_add(l.cache_creation_tokens);
        total_cache_read = total_cache_read.saturating_add(l.cache_read_tokens);
        total_output = total_output.saturating_add(l.output_tokens);
        if l.source == "fleet" {
            fleet_cost += l.cost_usd;
        } else {
            agent_cost += l.cost_usd;
        }
    }

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
        from_date: local_date_str(from_ms),
        to_date: local_date_str(to_ms),
        lines,
        daily,
        total_input_tokens: total_input,
        total_cache_creation_tokens: total_cache_creation,
        total_cache_read_tokens: total_cache_read,
        total_output_tokens: total_output,
        total_cost_usd: agent_cost + fleet_cost,
        agent_cost_usd: agent_cost,
        fleet_cost_usd: fleet_cost,
        has_codex_approximation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(created_at_ms: u64, cost: f64, output: u64, is_subagent: bool) -> SessionInfo {
        session_with_input(created_at_ms, cost, 0, output, is_subagent)
    }

    fn session_with_input(
        created_at_ms: u64,
        cost: f64,
        input: u64,
        output: u64,
        is_subagent: bool,
    ) -> SessionInfo {
        // Only the fields `sum_today_sessions` reads matter; the rest default.
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
    fn sums_only_sessions_created_today() {
        let day_start = 1_784_000_000_000i64;
        let sessions = vec![
            session(day_start as u64 - 1, 5.0, 1000, false), // yesterday → excluded
            session(day_start as u64, 1.0, 100, false),      // exactly at midnight → included
            session(day_start as u64 + 10, 2.0, 200, false), // today → included
            session(day_start as u64 + 20, 0.5, 50, true),   // today subagent → cost yes, count no
        ];
        let (cost, _input, output, count) = sum_today_sessions(&sessions, day_start);
        assert!((cost - 3.5).abs() < 1e-9, "cost was {cost}");
        assert_eq!(output, 350);
        assert_eq!(count, 2); // two non-subagent sessions
    }

    #[test]
    fn sums_input_tokens_of_today_sessions() {
        // Regression: "today's cumulative" must count input on the same口径 as
        // the daily report (input + output), not output alone.
        let day_start = 1_784_000_000_000i64;
        let sessions = vec![
            session_with_input(day_start as u64 - 1, 5.0, 9999, 1000, false), // yesterday → excluded
            session_with_input(day_start as u64, 1.0, 4000, 100, false),      // today
            session_with_input(day_start as u64 + 10, 2.0, 1000, 200, false), // today
            session_with_input(day_start as u64 + 20, 0.5, 500, 50, true),    // today subagent
        ];
        let (_cost, input, output, _count) = sum_today_sessions(&sessions, day_start);
        assert_eq!(
            input, 5500,
            "input = 4000 + 1000 + 500 (yesterday excluded)"
        );
        assert_eq!(output, 350);
    }

    #[test]
    fn cloud_usage_cumulative_sums_all_sessions_regardless_of_date() {
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
    }

    #[test]
    fn empty_when_nothing_today() {
        let day_start = 1_784_000_000_000i64;
        let sessions = vec![session(day_start as u64 - 100, 9.0, 999, false)];
        let (cost, input, output, count) = sum_today_sessions(&sessions, day_start);
        assert_eq!((cost, input, output, count), (0.0, 0, 0, 0));
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
        s.jsonl_path = path.to_string_lossy().to_string();
        s.agent_source = source.to_string();
        s
    }

    fn turn(id: &str, model: &str, input: u64, cc: u64, cr: u64, output: u64) -> String {
        serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-07-17T10:00:00Z",
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
        let b = build_breakdown(&sessions, &[], now);

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
        let b = build_breakdown(&sessions, &[], now);

        assert_eq!(b.lines.len(), 2);
        assert_eq!(b.lines[0].model, "claude-opus-4-8", "pricier model first");
        assert_eq!(b.lines[1].model, "claude-haiku-4-5");
        assert!((b.lines[0].cost_usd - 30.0).abs() < 1e-6);
        assert!((b.lines[1].cost_usd - 6.0).abs() < 1e-6);
        assert!((b.total_cost_usd - 36.0).abs() < 1e-6);
        assert_eq!(b.total_output_tokens, 2_000_000);
    }

    #[test]
    fn excludes_sessions_created_before_today() {
        let jsonl = turn("m1", "claude-sonnet-4-5", 1000, 0, 0, 100);
        let mut s = today_session_with_jsonl("excludes", "claude-code", &jsonl, false);
        // Move creation to well before today's local midnight.
        s.created_at_ms = 1_000_000_000_000; // 2001 — definitely not today
        let now = chrono::Local::now().timestamp_millis();
        let b = build_breakdown(&[s], &[], now);
        assert!(b.lines.is_empty(), "yesterday's session excluded");
        assert_eq!(b.total_cost_usd, 0.0);
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
        let b = build_breakdown(&[s], &[], now);

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
        acc.add(11, 0, 0, 7, 0.25);
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
        let b = build_range_breakdown(&[s], &[], from, to);

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
        let b = build_range_breakdown(&[s], &[], from, to);

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
        let b = build_range_breakdown(&[s], &[], from, to);

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
        let b = build_range_breakdown(&[s], &[], from, to);

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
        let b = build_range_breakdown(&[s], &[], from, to);

        assert!(b.lines.is_empty());
        assert!(b.daily.is_empty());
        assert!(!b.has_codex_approximation);
    }

    #[test]
    fn fleet_entries_fold_per_day_and_source() {
        use crate::llm_usage::FleetLlmUsageEntry;
        let when = ts("2026-07-15T10:00:00Z");
        let e = FleetLlmUsageEntry {
            timestamp_ms: when as u64,
            scenario: "guard_command".to_string(),
            provider: "claude".to_string(),
            model: "claude-haiku-4-5".to_string(),
            input_tokens: 1000,
            output_tokens: 100,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            duration_ms: 0,
            cost_usd: 0.5,
            token_accurate: true,
            cost_accurate: true,
        };
        let from = ts("2026-07-14T00:00:00Z");
        let to = ts("2026-07-16T00:00:00Z");
        let b = build_range_breakdown(&[], &[e], from, to);

        assert_eq!(b.lines.len(), 1);
        assert_eq!(b.lines[0].source, "fleet");
        assert!((b.fleet_cost_usd - 0.5).abs() < 1e-9);
        assert!((b.agent_cost_usd - 0.0).abs() < 1e-9);
        assert_eq!(b.daily.len(), 1);
        assert_eq!(b.daily[0].date, local_date_str(when));
        assert!((b.daily[0].cost_usd - 0.5).abs() < 1e-9);
    }

    /// The cache-ready `(date, model)` cells must reconstruct BOTH receipts
    /// bit-for-bit: whole-table sum == today's `fold_session_turns` (undated
    /// turns counted), date-window sum == range's `fold_session_turns_range`
    /// (undated turns dropped, per-day trend preserved).
    #[test]
    fn session_cells_reproduce_both_receipt_folders() {
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

        // today口径: whole-table sum == fold_session_turns.
        let mut old_today = std::collections::HashMap::new();
        fold_session_turns(jsonl, "claude-code", &mut old_today);
        let mut new_today = std::collections::HashMap::new();
        sum_cells_all(&cells, "claude-code", &mut new_today);
        assert_eq!(snap(&old_today), snap(&new_today), "today口径 mismatch");

        // range口径: date-window sum == fold_session_turns_range. The window
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
