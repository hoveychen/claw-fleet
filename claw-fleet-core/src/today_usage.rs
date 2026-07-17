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

/// Fold every finalized assistant turn of one session's JSONL into `by_model`,
/// keyed by `(source, per-turn model)`. Dedups by message id exactly like
/// [`crate::session::StatsAcc`] so a re-logged turn isn't double-counted.
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

        let acc = by_model.entry((source.to_string(), model)).or_default();
        acc.input += input;
        acc.cache_creation += cache_creation;
        acc.cache_read += cache_read;
        acc.output += output;
        acc.cost += cost;
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
    let acc = by_model
        .entry(("codex".to_string(), key_model))
        .or_default();
    acc.input += bd.input_tokens;
    acc.cache_read += bd.cached_input_tokens;
    acc.output += bd.output_tokens;
    acc.cost += bd.cost_usd;
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
    build_breakdown(sessions, &fleet_entries, now_ms)
}

/// Pure core of [`today_usage_breakdown`], with Fleet's own LLM entries injected
/// so it can be unit-tested without reading `~/.fleet/fleet_llm_usage.jsonl`
/// (mirrors how [`sum_today_sessions`] is the tested pure helper). Reads each
/// today-created session's JSONL from disk; the caller controls those paths.
fn build_breakdown(
    sessions: &[SessionInfo],
    fleet_entries: &[crate::llm_usage::FleetLlmUsageEntry],
    now_ms: i64,
) -> TodayUsageBreakdown {
    use crate::model_cost::get_model_costs;
    use std::collections::HashMap;

    let (day_start_ms, _day_end_ms, date) = day_bounds_ms(now_ms);
    let start = day_start_ms.max(0) as u64;

    let mut by_model: HashMap<(String, String), LineAcc> = HashMap::new();

    // Agent sessions created today (subagents included, each JSONL read once).
    for s in sessions {
        if s.created_at_ms < start {
            continue;
        }
        // Codex rollouts are shaped differently from Claude JSONL (no
        // `type:"assistant"` turns with a `message.usage`; instead a cumulative
        // `token_count` event with codex-native field names) AND their stored
        // `jsonl_path` is a `codex://` URI over a possibly-zstd rollout, not a
        // readable filesystem path. So `fold_session_turns` + a plain
        // `read_to_string` would drop every codex agent session from the
        // receipt. Fold them via the URI-aware codex reader instead.
        if s.agent_source == "codex" {
            fold_codex_session(&s.jsonl_path, &mut by_model);
            continue;
        }
        let jsonl = std::fs::read_to_string(&s.jsonl_path).unwrap_or_default();
        if jsonl.is_empty() {
            continue;
        }
        fold_session_turns(&jsonl, &s.agent_source, &mut by_model);
    }

    // Fleet's own LLM calls today, folded per model.
    for e in fleet_entries {
        let acc = by_model
            .entry(("fleet".to_string(), e.model.clone()))
            .or_default();
        acc.input += e.input_tokens;
        acc.cache_creation += e.cache_creation_tokens;
        acc.cache_read += e.cache_read_tokens;
        acc.output += e.output_tokens;
        acc.cost += e.cost_usd;
    }

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
    // Descending cost, then model name for a stable order among equal costs.
    lines.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.model.cmp(&b.model))
    });

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
