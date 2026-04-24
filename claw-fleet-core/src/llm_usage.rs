//! Fleet-self LLM usage accounting.
//!
//! Every LLM call Fleet itself makes (guard analysis, audit-rule suggestions,
//! daily-report summaries, session outcome analysis, mascot quips, lessons
//! extraction) is logged here. The Settings → Usage tab visualises consumption
//! trending by scenario so users can see where their tokens are going.
//!
//! Storage: JSONL append at `~/.fleet/fleet_llm_usage.jsonl`.
//!
//! Token accuracy: when the provider reports real usage (Claude CLI run with
//! `--output-format json`), those numbers are written verbatim and entries are
//! marked `tokenAccurate = true`. For providers that emit text only (Codex,
//! Cursor), tokens fall back to character-count estimation (~4 chars/token)
//! and entries set `tokenAccurate = false` so the UI can badge the numbers.
//!
//! Cost accuracy: Claude-provider calls record the CLI's own `total_cost_usd`
//! (which includes cache-creation pricing). Codex / Cursor entries set
//! `costAccurate = false` and `costUsd = 0.0`.

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::llm_provider::LlmProvider;
use crate::log_debug;
use crate::model_cost::{turn_cost_usd, TurnUsage};

// ── Scenario tags ────────────────────────────────────────────────────────────
// Keep in sync with the frontend legend in UsageTrendPanel.tsx.

pub const SCENARIO_GUARD_COMMAND: &str = "guard_command";
pub const SCENARIO_AUDIT_RULES: &str = "audit_rules";
pub const SCENARIO_DAILY_REPORT_SUMMARY: &str = "daily_report_summary";
pub const SCENARIO_DAILY_REPORT_LESSONS: &str = "daily_report_lessons";
pub const SCENARIO_SESSION_ANALYZE: &str = "session_analyze";
pub const SCENARIO_MASCOT_QUIPS: &str = "mascot_quips";

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FleetLlmUsageEntry {
    /// Unix ms.
    pub timestamp_ms: u64,
    /// Scenario tag — one of the `SCENARIO_*` constants above.
    pub scenario: String,
    /// Provider name: "claude", "codex", "cursor".
    pub provider: String,
    /// Model alias as passed to `provider.complete()` ("haiku", "sonnet", "opus",
    /// or a codex/cursor model id).
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    /// Wall-clock duration of the `complete()` call.
    pub duration_ms: u64,
    /// USD cost if computable (Claude provider only). 0.0 otherwise.
    pub cost_usd: f64,
    /// True = token counts came from the CLI (accurate).
    /// False = estimated from prompt/response char counts.
    pub token_accurate: bool,
    /// True = `cost_usd` came from `turn_cost_usd` for a priced tier.
    /// False = pricing unknown (non-Claude provider), `cost_usd` is 0.
    pub cost_accurate: bool,
}

/// One (date, scenario) bucket aggregated from raw entries.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FleetLlmUsageDailyBucket {
    /// YYYY-MM-DD in the user's local timezone.
    pub date: String,
    pub scenario: String,
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost_usd: f64,
    /// True if any contributing entry had estimated tokens.
    pub has_estimated_tokens: bool,
    /// True if any contributing entry lacked reliable pricing.
    pub has_unpriced_calls: bool,
}

// ── Storage ──────────────────────────────────────────────────────────────────

fn usage_log_path() -> Option<PathBuf> {
    let home = crate::session::real_home_dir()?;
    let dir = home.join(".fleet");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log_debug(&format!("[llm_usage] failed to create dir: {e}"));
        return None;
    }
    Some(dir.join("fleet_llm_usage.jsonl"))
}

pub fn append_usage_entry(entry: &FleetLlmUsageEntry) {
    let Some(path) = usage_log_path() else { return; };
    let Ok(line) = serde_json::to_string(entry) else { return; };
    match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut file) => {
            let _ = writeln!(file, "{line}");
        }
        Err(e) => log_debug(&format!("[llm_usage] failed to append: {e}")),
    }
}

/// Read all entries in the [from_ms, to_ms] window (inclusive on both ends).
pub fn list_usage_entries(from_ms: u64, to_ms: u64) -> Vec<FleetLlmUsageEntry> {
    let Some(path) = usage_log_path() else { return Vec::new(); };
    let Ok(content) = std::fs::read_to_string(&path) else { return Vec::new(); };
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<FleetLlmUsageEntry>(l).ok())
        .filter(|e| e.timestamp_ms >= from_ms && e.timestamp_ms <= to_ms)
        .collect()
}

/// Aggregate entries into (date, scenario) buckets, sorted by date then scenario.
pub fn list_usage_daily_buckets(from_ms: u64, to_ms: u64) -> Vec<FleetLlmUsageDailyBucket> {
    use std::collections::BTreeMap;

    let entries = list_usage_entries(from_ms, to_ms);
    let mut map: BTreeMap<(String, String), FleetLlmUsageDailyBucket> = BTreeMap::new();

    for e in entries {
        let date = chrono::DateTime::from_timestamp_millis(e.timestamp_ms as i64)
            .unwrap_or_else(chrono::Utc::now)
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string();

        let b = map
            .entry((date.clone(), e.scenario.clone()))
            .or_insert_with(|| FleetLlmUsageDailyBucket {
                date,
                scenario: e.scenario.clone(),
                calls: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                cost_usd: 0.0,
                has_estimated_tokens: false,
                has_unpriced_calls: false,
            });
        b.calls += 1;
        b.input_tokens += e.input_tokens;
        b.output_tokens += e.output_tokens;
        b.cache_creation_tokens += e.cache_creation_tokens;
        b.cache_read_tokens += e.cache_read_tokens;
        b.cost_usd += e.cost_usd;
        if !e.token_accurate {
            b.has_estimated_tokens = true;
        }
        if !e.cost_accurate {
            b.has_unpriced_calls = true;
        }
    }

    map.into_values().collect()
}

// ── Accounted completion wrapper ─────────────────────────────────────────────

/// Rough token estimate (~4 chars per token). Overcounts for dense code, under-
/// counts for CJK. Good enough for trending; not good enough for billing.
fn estimate_tokens(s: &str) -> u64 {
    (s.chars().count() as u64).div_ceil(4)
}

/// Resolve a Claude CLI alias ("haiku", "sonnet", "opus") to the current
/// canonical model id that `model_cost::get_model_costs` recognises.
/// Non-alias inputs pass through untouched.
fn canonical_claude_model(alias: &str) -> &str {
    match alias {
        "haiku" => "claude-haiku-4-5",
        "sonnet" => "claude-sonnet-4-6",
        "opus" => "claude-opus-4-7",
        other => other,
    }
}

/// Call `provider.complete()` and record a usage log entry tagged with
/// `scenario`. Returns the completion text, or None on failure (no entry
/// written on failure — we don't log effort that produced no output).
///
/// When the provider reports real usage (Claude CLI JSON output), those
/// numbers are written verbatim. Otherwise tokens are estimated from char
/// counts and cost is priced via `model_cost::turn_cost_usd` for Claude models.
pub fn complete_accounted(
    provider: &dyn LlmProvider,
    prompt: &str,
    model: &str,
    timeout: Duration,
    scenario: &str,
) -> Option<String> {
    let started = Instant::now();
    let completion = provider.complete(prompt, model, timeout)?;
    let duration_ms = started.elapsed().as_millis() as u64;

    let (input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens,
         cost_usd, token_accurate, cost_accurate) = match &completion.usage {
        Some(u) => (
            u.input_tokens,
            u.output_tokens,
            u.cache_creation_tokens,
            u.cache_read_tokens,
            u.total_cost_usd,
            true,
            true,
        ),
        None => {
            let input = estimate_tokens(prompt);
            let output = estimate_tokens(&completion.text);
            let (cost, cost_acc) = if provider.name() == "claude" {
                let canonical = canonical_claude_model(model);
                let usage = TurnUsage {
                    input_tokens: input,
                    output_tokens: output,
                    ..Default::default()
                };
                (turn_cost_usd(canonical, &usage), true)
            } else {
                (0.0, false)
            };
            (input, output, 0, 0, cost, false, cost_acc)
        }
    };

    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    append_usage_entry(&FleetLlmUsageEntry {
        timestamp_ms,
        scenario: scenario.to_string(),
        provider: provider.name().to_string(),
        model: model.to_string(),
        input_tokens,
        output_tokens,
        cache_creation_tokens,
        cache_read_tokens,
        duration_ms,
        cost_usd,
        token_accurate,
        cost_accurate,
    });

    Some(completion.text)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_basic() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }

    #[test]
    fn canonical_claude_aliases() {
        assert_eq!(canonical_claude_model("haiku"), "claude-haiku-4-5");
        assert_eq!(canonical_claude_model("sonnet"), "claude-sonnet-4-6");
        assert_eq!(canonical_claude_model("opus"), "claude-opus-4-7");
        assert_eq!(canonical_claude_model("claude-opus-4-6"), "claude-opus-4-6");
    }

    #[test]
    fn daily_buckets_aggregate_by_date_and_scenario() {
        // 2026-01-15 12:00:00 UTC = 1768521600000 ms
        let base = 1768521600000u64;
        let entries = vec![
            FleetLlmUsageEntry {
                timestamp_ms: base,
                scenario: "guard_command".into(),
                provider: "claude".into(),
                model: "haiku".into(),
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                duration_ms: 1000,
                cost_usd: 0.0004,
                token_accurate: false,
                cost_accurate: true,
            },
            FleetLlmUsageEntry {
                timestamp_ms: base + 3600_000,
                scenario: "guard_command".into(),
                provider: "claude".into(),
                model: "haiku".into(),
                input_tokens: 200,
                output_tokens: 80,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                duration_ms: 1200,
                cost_usd: 0.0007,
                token_accurate: false,
                cost_accurate: true,
            },
        ];

        // Simulate list_usage_daily_buckets aggregation logic directly
        // (without hitting the filesystem).
        use std::collections::BTreeMap;
        let mut map: BTreeMap<(String, String), FleetLlmUsageDailyBucket> = BTreeMap::new();
        for e in entries {
            let date = chrono::DateTime::from_timestamp_millis(e.timestamp_ms as i64)
                .unwrap()
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string();
            let b = map.entry((date.clone(), e.scenario.clone())).or_insert_with(|| {
                FleetLlmUsageDailyBucket {
                    date,
                    scenario: e.scenario.clone(),
                    calls: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                    cost_usd: 0.0,
                    has_estimated_tokens: false,
                    has_unpriced_calls: false,
                }
            });
            b.calls += 1;
            b.input_tokens += e.input_tokens;
            b.output_tokens += e.output_tokens;
            b.cost_usd += e.cost_usd;
            if !e.token_accurate { b.has_estimated_tokens = true; }
            if !e.cost_accurate { b.has_unpriced_calls = true; }
        }
        let buckets: Vec<_> = map.into_values().collect();
        assert_eq!(buckets.len(), 1, "same day + same scenario should collapse");
        assert_eq!(buckets[0].calls, 2);
        assert_eq!(buckets[0].input_tokens, 300);
        assert_eq!(buckets[0].output_tokens, 130);
        assert!((buckets[0].cost_usd - 0.0011).abs() < 1e-9);
        assert!(buckets[0].has_estimated_tokens);
        assert!(!buckets[0].has_unpriced_calls);
    }
}
