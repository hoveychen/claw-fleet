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
    /// Input tokens today (incl. cache creation + cache read), on the same口径
    /// as the daily report: per agent session, its last-turn context-window size
    /// (`SessionInfo.total_input_tokens`); plus Fleet's own LLM calls' input +
    /// cache tokens. Summed alongside `output_tokens` so the badge total
    /// (`input + output`) matches the daily report's `input + output`.
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
            session(day_start as u64, 1.0, 100, false),    // exactly at midnight → included
            session(day_start as u64 + 10, 2.0, 200, false), // today → included
            session(day_start as u64 + 20, 0.5, 50, true),  // today subagent → cost yes, count no
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
        assert_eq!(input, 5500, "input = 4000 + 1000 + 500 (yesterday excluded)");
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
