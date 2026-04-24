//! Populate ~/.fleet/fleet_llm_usage.jsonl with synthetic entries spanning the
//! last 7 days so the Usage tab has something to render.
//!
//! Run with: cargo run -p claw-fleet-core --example seed_llm_usage
//!
//! Exercises both paths:
//! - `append_usage_entry` for backdated entries (can't backdate via the wrapper
//!   because `complete_accounted` stamps with SystemTime::now()).
//! - `complete_accounted` with a stub provider for today, proving the full
//!   pipeline (estimation + pricing) writes a correct record.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use claw_fleet_core::llm_provider::{LlmModel, LlmProvider};
use claw_fleet_core::llm_usage::{
    append_usage_entry, complete_accounted, list_usage_daily_buckets, FleetLlmUsageEntry,
    SCENARIO_AUDIT_RULES, SCENARIO_DAILY_REPORT_LESSONS, SCENARIO_DAILY_REPORT_SUMMARY,
    SCENARIO_GUARD_COMMAND, SCENARIO_MASCOT_QUIPS, SCENARIO_SESSION_ANALYZE,
};
use claw_fleet_core::model_cost::{turn_cost_usd, TurnUsage};

struct StubClaude;

impl LlmProvider for StubClaude {
    fn name(&self) -> &str { "claude" }
    fn display_name(&self) -> &str { "Stub Claude" }
    fn is_available(&self) -> bool { true }
    fn list_models(&self) -> Vec<LlmModel> { Vec::new() }
    fn default_fast_model(&self) -> &str { "haiku" }
    fn default_standard_model(&self) -> &str { "sonnet" }
    fn complete(&self, _prompt: &str, _model: &str, _timeout: Duration) -> Option<String> {
        Some(
            "Stub response: this is the synthesized answer the stub provider returns for \
             the purpose of exercising the accounting wrapper end to end."
                .into(),
        )
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn append_backdated(
    days_ago: u64,
    hour_offset: u64,
    scenario: &str,
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
) {
    let now = now_ms();
    let ts = now - days_ago * 86_400_000 - hour_offset * 3_600_000;
    let usage = TurnUsage { input_tokens, output_tokens, ..Default::default() };
    let canonical = match model {
        "haiku" => "claude-haiku-4-5",
        "sonnet" => "claude-sonnet-4-6",
        "opus" => "claude-opus-4-7",
        other => other,
    };
    let cost_usd = turn_cost_usd(canonical, &usage);
    append_usage_entry(&FleetLlmUsageEntry {
        timestamp_ms: ts,
        scenario: scenario.to_string(),
        provider: "claude".into(),
        model: model.into(),
        input_tokens,
        output_tokens,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        duration_ms: 1200,
        cost_usd,
        token_accurate: false,
        cost_accurate: true,
    });
}

fn main() {
    // Spread realistic-looking traffic across the last 7 days. Volumes reflect
    // the actual cadences in the code:
    //   guard_command — runs on every command that triggers guard analysis
    //   session_analyze — runs at session close, a few per day
    //   mascot_quips — low-rate cosmetic
    //   audit_rules — user-triggered
    //   daily_report_summary / _lessons — once per day each
    let profiles: &[(&str, &str, u64, u64, u64)] = &[
        // (scenario, model, calls_per_day, input_tokens, output_tokens)
        (SCENARIO_GUARD_COMMAND, "haiku", 18, 900, 120),
        (SCENARIO_SESSION_ANALYZE, "haiku", 6, 2400, 300),
        (SCENARIO_AUDIT_RULES, "sonnet", 1, 3500, 800),
        (SCENARIO_MASCOT_QUIPS, "haiku", 3, 450, 80),
        (SCENARIO_DAILY_REPORT_SUMMARY, "sonnet", 1, 6000, 1100),
        (SCENARIO_DAILY_REPORT_LESSONS, "sonnet", 1, 4500, 900),
    ];

    let mut written = 0usize;
    for day in 0..7u64 {
        for (scenario, model, per_day, input_tokens, output_tokens) in profiles {
            for call in 0..*per_day {
                let hour_offset = (call as u64 * 17 + day) % 18;
                // Vary tokens a bit so the chart isn't flat.
                let jitter_in = input_tokens + (day * 37 + call as u64 * 53) % 300;
                let jitter_out = output_tokens + (day * 11 + call as u64 * 19) % 80;
                append_backdated(
                    day,
                    hour_offset,
                    scenario,
                    model,
                    jitter_in,
                    jitter_out,
                );
                written += 1;
            }
        }
    }

    // Also drive one real call through the wrapper to prove the full path writes
    // a priced entry for today.
    let stub = StubClaude;
    let resp = complete_accounted(
        &stub,
        "Explain in one sentence why Fleet tracks its own LLM consumption.",
        "haiku",
        Duration::from_secs(5),
        SCENARIO_GUARD_COMMAND,
    );
    println!("complete_accounted → {}", resp.is_some());

    let buckets = list_usage_daily_buckets(0, now_ms());
    println!("seeded {written} backdated entries + 1 live call");
    println!("aggregated into {} (date, scenario) buckets", buckets.len());
    // Show a sample.
    for b in buckets.iter().take(8) {
        println!(
            "  {} {:<24} calls={:<4} tokens={:<7} cost=${:.4}",
            b.date,
            b.scenario,
            b.calls,
            b.input_tokens + b.output_tokens,
            b.cost_usd
        );
    }
    if buckets.len() > 8 {
        println!("  … and {} more", buckets.len() - 8);
    }
}
