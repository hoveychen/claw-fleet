//! USD cost calculation for Claude API usage.
//!
//! Ported from Claude Code's own implementation at
//! `claude-code-fork/src/utils/modelCost.ts`. Prices are hardcoded from
//! https://platform.claude.com/docs/en/about-claude/pricing and must be
//! kept in sync when new models ship.
//!
//! The formula is:
//!   (input_tokens / 1M)               * inputPrice
//! + (output_tokens / 1M)              * outputPrice
//! + (5m cache-write tokens / 1M)      * promptCacheWritePrice
//! + (1h cache-write tokens / 1M)      * promptCacheWrite1hPrice
//! + (cache_read_input_tokens / 1M)    * promptCacheReadPrice
//! + web_search_requests               * webSearchPrice
//!
//! Cache writes are billed by TTL: a 5-minute (`ephemeral_5m`) write costs
//! 1.25× the model's input rate, a 1-hour (`ephemeral_1h`) write costs 2×.
//! Claude Code opts into the 1-hour TTL, so in practice nearly every cache
//! write in a transcript is the 2× kind — pricing them all at 1.25× undercounts
//! spend. Verified against a real `claude -p --output-format json` probe
//! (2026-07-25): a Haiku 4.5 turn with 30057 `ephemeral_1h_input_tokens`
//! reported `total_cost_usd` exactly matching the 2× rate.

#[derive(Clone, Copy, Debug)]
pub struct ModelCosts {
    /// USD per 1M input tokens.
    pub input: f64,
    /// USD per 1M output tokens.
    pub output: f64,
    /// USD per 1M cache-write (creation) tokens at the 5-minute TTL.
    pub cache_write: f64,
    /// USD per 1M cache-write (creation) tokens at the 1-hour TTL (2× `input`).
    /// Equal to `cache_write` for providers with no TTL tiers (GPT / Codex).
    pub cache_write_1h: f64,
    /// USD per 1M cache-read tokens.
    pub cache_read: f64,
    /// USD per web-search request.
    pub web_search: f64,
}

// Sonnet 5: $2 / $10 per Mtok. Announced as introductory pricing through
// 2026-08-31, then made the standard price — the scheduled rise to $3/$15 never
// happened (Anthropic pricing page, checked 2026-09-22).
pub const COST_TIER_2_10: ModelCosts = ModelCosts {
    input: 2.0,
    output: 10.0,
    cache_write: 2.50,
    cache_write_1h: 4.0,
    cache_read: 0.20,
    web_search: 0.01,
};

// Sonnet 4.6 and earlier: $3 / $15 per Mtok.
pub const COST_TIER_3_15: ModelCosts = ModelCosts {
    input: 3.0,
    output: 15.0,
    cache_write: 3.75,
    cache_write_1h: 6.0,
    cache_read: 0.30,
    web_search: 0.01,
};

// Legacy Opus 4 / 4.1: $15 / $75 per Mtok.
pub const COST_TIER_15_75: ModelCosts = ModelCosts {
    input: 15.0,
    output: 75.0,
    cache_write: 18.75,
    cache_write_1h: 30.0,
    cache_read: 1.5,
    web_search: 0.01,
};

// Opus 5.5: $4 / $20 per Mtok. Cache reads are $0.20, i.e. input/20 rather
// than the input/10 every other tier here uses — the Claude Code catalog names
// this tier `tier_4_20_cache_read_0_20` for exactly that reason (read from the
// 2.1.280 binary, 2026-09-22). Do not "normalise" it to 0.40.
pub const COST_TIER_4_20: ModelCosts = ModelCosts {
    input: 4.0,
    output: 20.0,
    cache_write: 5.0,
    cache_write_1h: 8.0,
    cache_read: 0.20,
    web_search: 0.01,
};

// Opus 4.5 / 4.6 / 4.7 / 4.8 and Opus 5: $5 / $25 per Mtok.
pub const COST_TIER_5_25: ModelCosts = ModelCosts {
    input: 5.0,
    output: 25.0,
    cache_write: 6.25,
    cache_write_1h: 10.0,
    cache_read: 0.5,
    web_search: 0.01,
};

// Opus 4.6 "fast mode": $30 / $150 per Mtok.
pub const COST_TIER_30_150: ModelCosts = ModelCosts {
    input: 30.0,
    output: 150.0,
    cache_write: 37.5,
    cache_write_1h: 60.0,
    cache_read: 3.0,
    web_search: 0.01,
};

// Fable 5 / Mythos 5 tier: $10 / $50 per Mtok (per Anthropic's published
// pricing, 2026-06). Shared by `claude-fable-5` and `claude-mythos-5`
// (incl. the invitation-only `-preview`).
pub const COST_TIER_10_50: ModelCosts = ModelCosts {
    input: 10.0,
    output: 50.0,
    cache_write: 12.5,
    cache_write_1h: 20.0,
    cache_read: 1.0,
    web_search: 0.01,
};

// Fable 5.1 / Mythos 5.1: the same $10 / $50 as the 5.0 pair, but cache reads
// are 0.025× input ($0.25) instead of 0.1× (Anthropic pricing page, checked
// 2026-09-22).
pub const COST_TIER_10_50_READ_0_25: ModelCosts = ModelCosts {
    cache_read: 0.25,
    ..COST_TIER_10_50
};

// Haiku 3.5: $0.80 / $4 per Mtok.
pub const COST_HAIKU_35: ModelCosts = ModelCosts {
    input: 0.80,
    output: 4.0,
    cache_write: 1.0,
    cache_write_1h: 1.6,
    cache_read: 0.08,
    web_search: 0.01,
};

// Haiku 4.5: $1 / $5 per Mtok.
pub const COST_HAIKU_45: ModelCosts = ModelCosts {
    input: 1.0,
    output: 5.0,
    cache_write: 1.25,
    cache_write_1h: 2.0,
    cache_read: 0.10,
    web_search: 0.01,
};

// --- OpenAI GPT / Codex tiers ---------------------------------------------
// Used by Codex CLI sessions (see `codex_source.rs`). Codex rollouts report
// the model as e.g. `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`,
// `gpt-5.3-codex`. Prices per 1M tokens from the Standard / short-context
// table at https://developers.openai.com/api/docs/pricing (verified
// 2026-09-22 against the page's embedded price arrays).
// `cache_read` is input * 0.10 (90% cache discount); `cache_write` is
// input * 1.25 (explicit cache-write surcharge) — Codex rollouts don't
// report cache-write tokens so `cache_write` is currently unused for Codex.
// `web_search` is 0 (not applicable to the Codex token accounting we parse).
//
// NOTE: OpenAI's ">272K input tokens per request => 2x input / 1.5x output"
// surcharge is deliberately NOT modeled here. Codex rollouts only give
// cumulative per-session token totals, not per-request counts, so applying a
// per-request multiplier to the cumulative sum would over-count. See
// `codex_source::codex_cost_and_input`.

// gpt-6-astra standard short-context pricing: $10 / $50 per Mtok, with
// cached input at $1 and explicit cache writes at $12.50 (verified 2026-09-06).
pub const COST_GPT_ASTRA: ModelCosts = ModelCosts {
    input: 10.0,
    output: 50.0,
    cache_write: 12.50,
    cache_write_1h: 12.50,
    cache_read: 1.0,
    web_search: 0.0,
};

// gpt-6-sol: $2 / $10 per Mtok — cheaper than gpt-5.6-sol, so it must not
// fall through to the 5.6 Sol tier just because both names contain "sol".
pub const COST_GPT6_SOL: ModelCosts = ModelCosts {
    input: 2.0,
    output: 10.0,
    cache_write: 2.50,
    cache_write_1h: 2.50,
    cache_read: 0.20,
    web_search: 0.0,
};

// gpt-6-luna: $0.10 / $0.50 per Mtok.
pub const COST_GPT6_LUNA: ModelCosts = ModelCosts {
    input: 0.10,
    output: 0.50,
    cache_write: 0.125,
    cache_write_1h: 0.125,
    cache_read: 0.01,
    web_search: 0.0,
};

// gpt-5.6-sol (and bare gpt-5.6): $4 / $20 per Mtok (was $5 / $30 until the
// 5.6 family's price cut, first seen 2026-09-22).
pub const COST_GPT_SOL: ModelCosts = ModelCosts {
    input: 4.0,
    output: 20.0,
    cache_write: 5.0,
    cache_write_1h: 5.0,
    cache_read: 0.40,
    web_search: 0.0,
};

// gpt-5.6-terra: $2 / $12 per Mtok (was $2.50 / $15).
pub const COST_GPT_TERRA: ModelCosts = ModelCosts {
    input: 2.0,
    output: 12.0,
    cache_write: 2.50,
    cache_write_1h: 2.50,
    cache_read: 0.20,
    web_search: 0.0,
};

// gpt-5.6-luna: $0.20 / $1.20 per Mtok (was $1 / $6).
pub const COST_GPT_LUNA: ModelCosts = ModelCosts {
    input: 0.20,
    output: 1.20,
    cache_write: 0.25,
    cache_write_1h: 0.25,
    cache_read: 0.02,
    web_search: 0.0,
};

// gpt-5.5: $5 / $30 per Mtok. It used to share the 5.6 Sol tier by
// coincidence; the 5.6 cut split them. OpenAI lists no cache-write price for
// it, so `cache_write` keeps the input * 1.25 convention (unused for Codex).
pub const COST_GPT_5_5: ModelCosts = ModelCosts {
    input: 5.0,
    output: 30.0,
    cache_write: 6.25,
    cache_write_1h: 6.25,
    cache_read: 0.50,
    web_search: 0.0,
};

// gpt-5.3-codex (and other `*-codex` variants): $1.75 / $14 per Mtok.
pub const COST_GPT_CODEX: ModelCosts = ModelCosts {
    input: 1.75,
    output: 14.0,
    cache_write: 2.1875,
    cache_write_1h: 2.1875,
    cache_read: 0.175,
    web_search: 0.0,
};

const DEFAULT_UNKNOWN_COST: ModelCosts = COST_TIER_5_25;

/// Look up pricing for a Claude model name.
///
/// `model` is the raw model string from a JSONL assistant message
/// (e.g. `claude-opus-4-6-20251101`). We do a substring match so that
/// dated aliases and `-thinking` / `-fast` variants all resolve.
///
/// Returns the default (Opus 4.5 tier) if the model is unknown, matching
/// Claude Code's behavior.
pub fn get_model_costs(model: &str) -> ModelCosts {
    let m = model.to_ascii_lowercase();

    // OpenAI GPT / Codex models (Codex CLI sessions). Checked first because
    // no Claude model name contains "gpt", "terra", "luna", or "codex", so
    // these branches never shadow a Claude lookup. Order within the block:
    // most specific tier suffix first, then bare `gpt-*` -> Sol (the
    // conservative, most-expensive gpt-5.6 tier) so an unrecognised gpt
    // model never silently prices below what it likely costs.
    if m.contains("gpt") || m.contains("codex") {
        if m.contains("astra") {
            return COST_GPT_ASTRA;
        }
        // GPT-6 Sol / Luna share their tier names with 5.6 but not its
        // prices, so the generation has to be decided before the name.
        if m.contains("gpt-6") {
            if m.contains("luna") {
                return COST_GPT6_LUNA;
            }
            if m.contains("sol") {
                return COST_GPT6_SOL;
            }
        }
        if m.contains("gpt-5.5") {
            return COST_GPT_5_5;
        }
        if m.contains("terra") {
            return COST_GPT_TERRA;
        }
        if m.contains("luna") {
            return COST_GPT_LUNA;
        }
        if m.contains("codex") {
            return COST_GPT_CODEX;
        }
        return COST_GPT_SOL;
    }

    // Fable 5.1 / Mythos 5.1 keep the $10/$50 rates but read cache at $0.25.
    // Minor parsed like `opus-5-` below, so a dated `claude-fable-5-20260601`
    // (where `-2` is a year) stays on the 5.0 tier.
    if single_digit_minor(&m, "fable-5-") == Some(1)
        || single_digit_minor(&m, "mythos-5-") == Some(1)
    {
        return COST_TIER_10_50_READ_0_25;
    }

    // Fable 5 and Mythos 5 share the $10/$50 tier. Substring match tolerates
    // `claude-fable-5`, the `fable` alias, `claude-mythos-preview`,
    // `claude-mythos-1-20260101`, etc.
    if m.contains("fable") || m.contains("mythos") {
        return COST_TIER_10_50;
    }

    // Opus 4.x tier routing. Substring matching is fragile because
    // "claude-opus-4-7" contains "opus-4" (legacy) but is actually a modern
    // tier. Instead, parse the single-digit minor version after `opus-4-`:
    //   0, 1        -> legacy $15/$75 (Opus 4.0 / 4.1)
    //   2..=9       -> modern $5/$25  (Opus 4.5, 4.6, 4.7, future minors)
    // A bare `opus-4` without a version digit (e.g. the dated 4.0 ID
    // `claude-opus-4-20250514`, where `-2` is the year) falls through to
    // the legacy branch below.
    if let Some(start) = m.find("opus-4-") {
        let rest = &m.as_bytes()[start + "opus-4-".len()..];
        if let Some((&first, tail)) = rest.split_first() {
            let is_single_digit =
                first.is_ascii_digit() && tail.first().map_or(true, |c| !c.is_ascii_digit());
            if is_single_digit {
                return match first {
                    b'0' | b'1' => COST_TIER_15_75,
                    _ => COST_TIER_5_25,
                };
            }
        }
    }
    if m.contains("opus-4") {
        return COST_TIER_15_75;
    }

    // Opus 5.x minor routing, which has to run *before* the major-digit branch
    // below: `claude-opus-5-5` contains `opus-5` and would otherwise be priced
    // at the $5/$25 tier Opus 5 uses, silently overcharging by 25%. Parsed the
    // same way as `opus-4-` above, so the dated `claude-opus-5-5-20260901`
    // still matches while `claude-opus-5-20260801` (where `-2` is a year, not
    // a minor) falls through to the major-digit branch.
    if single_digit_minor(&m, "opus-5-") == Some(5) {
        return COST_TIER_4_20;
    }

    // Opus 5 and later majors: the modern $5/$25 tier (`claude-opus-5` is
    // $5/$25, same as the 4.5+ minors). Matched on the major digit so a future
    // `opus-6` is priced deliberately instead of landing on the unknown-model
    // fallback, which happens to be the same tier only by coincidence.
    if let Some(start) = m.find("opus-") {
        let rest = &m.as_bytes()[start + "opus-".len()..];
        if rest.first().is_some_and(|c| (b'5'..=b'9').contains(c)) {
            return COST_TIER_5_25;
        }
    }

    // Sonnet 5 is $2/$10. `sonnet-5` followed by anything but another digit,
    // so `claude-sonnet-5`, `-5[1m]` and dated ids match but a hypothetical
    // `sonnet-50` would not.
    if let Some(start) = m.find("sonnet-5") {
        let next = m.as_bytes().get(start + "sonnet-5".len());
        if next.map_or(true, |c| !c.is_ascii_digit()) {
            return COST_TIER_2_10;
        }
    }

    // Sonnet 4.6 and earlier (4.6/4.5/4, 3.7, 3.5 all share $3/$15).
    if m.contains("sonnet") {
        return COST_TIER_3_15;
    }

    // Haiku tiers.
    if m.contains("haiku-4-5") || m.contains("haiku-4") {
        return COST_HAIKU_45;
    }
    if m.contains("haiku-3-5") || m.contains("haiku") {
        return COST_HAIKU_35;
    }

    DEFAULT_UNKNOWN_COST
}

/// The minor version right after `prefix` (e.g. `opus-5-` → `5` for
/// `claude-opus-5-5`), but only when it is a single digit. A multi-digit run
/// is a date stamp (`claude-opus-5-20260801`), not a minor, and yields `None`.
fn single_digit_minor(m: &str, prefix: &str) -> Option<u8> {
    let start = m.find(prefix)?;
    let rest = &m.as_bytes()[start + prefix.len()..];
    let (&first, tail) = rest.split_first()?;
    let single = first.is_ascii_digit() && tail.first().map_or(true, |c| !c.is_ascii_digit());
    single.then(|| first - b'0')
}

/// Raw token counts for one assistant turn.
#[derive(Clone, Copy, Debug, Default)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// **Total** cache-write tokens, both TTLs (the API's
    /// `cache_creation_input_tokens`).
    pub cache_creation_tokens: u64,
    /// The subset of `cache_creation_tokens` written at the 1-hour TTL (the
    /// API's `cache_creation.ephemeral_1h_input_tokens`). Billed at
    /// `cache_write_1h`; the remainder is billed at `cache_write`. Leaving this
    /// at 0 prices every write at the cheaper 5-minute rate, so callers that
    /// fold real transcripts must populate it.
    pub cache_creation_1h_tokens: u64,
    pub cache_read_tokens: u64,
    pub web_search_requests: u64,
}

/// Extract the 1-hour-TTL slice of a turn's cache writes out of a raw
/// `message.usage` object: `usage.cache_creation.ephemeral_1h_input_tokens`.
///
/// Every folder that reads transcript usage needs this, so it lives here beside
/// the prices rather than being re-derived per call site. A turn with no
/// `cache_creation` sub-object (older transcripts, Codex rollouts) yields 0,
/// which prices its writes at the 5-minute rate — the best available answer for
/// a turn that never recorded its TTL.
pub fn parse_cache_creation_1h(usage: Option<&serde_json::Value>) -> u64 {
    usage
        .and_then(|u| u.get("cache_creation"))
        .and_then(|c| c.get("ephemeral_1h_input_tokens"))
        .and_then(|n| n.as_u64())
        .unwrap_or(0)
}

/// Compute USD cost for one assistant turn under the given model.
pub fn turn_cost_usd(model: &str, usage: &TurnUsage) -> f64 {
    let c = get_model_costs(model);
    // `cache_creation_1h_tokens` is a subset of the total; saturating_sub keeps a
    // malformed pair (1h > total) from wrapping into an astronomical 5m figure.
    let write_1h = usage
        .cache_creation_1h_tokens
        .min(usage.cache_creation_tokens);
    let write_5m = usage.cache_creation_tokens.saturating_sub(write_1h);
    (usage.input_tokens as f64 / 1_000_000.0) * c.input
        + (usage.output_tokens as f64 / 1_000_000.0) * c.output
        + (write_5m as f64 / 1_000_000.0) * c.cache_write
        + (write_1h as f64 / 1_000_000.0) * c.cache_write_1h
        + (usage.cache_read_tokens as f64 / 1_000_000.0) * c.cache_read
        + (usage.web_search_requests as f64) * c.web_search
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sonnet_pricing() {
        // 1M input + 1M output on Sonnet = $3 + $15 = $18.
        let usage = TurnUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        let cost = turn_cost_usd("claude-sonnet-4-6-20251101", &usage);
        assert!((cost - 18.0).abs() < 1e-9);
    }

    #[test]
    fn opus_46_default_tier() {
        // Opus 4.6 default pricing = $5/$25.
        let usage = TurnUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        let cost = turn_cost_usd("claude-opus-4-6-20251101", &usage);
        assert!((cost - 30.0).abs() < 1e-9);
    }

    /// Anthropic's cache-write surcharges are multiples of the model's input
    /// rate: 1.25× for a 5-minute TTL, 2× for a 1-hour TTL. Pin that on every
    /// Claude tier so a future tier added with a hand-typed `cache_write_1h`
    /// can't quietly drift.
    #[test]
    fn claude_cache_write_tiers_are_input_multiples() {
        for c in [
            COST_TIER_2_10,
            COST_TIER_3_15,
            COST_TIER_15_75,
            COST_TIER_4_20,
            COST_TIER_5_25,
            COST_TIER_30_150,
            COST_TIER_10_50,
            COST_TIER_10_50_READ_0_25,
            COST_HAIKU_35,
            COST_HAIKU_45,
        ] {
            assert!((c.cache_write - c.input * 1.25).abs() < 1e-9, "5m: {c:?}");
            assert!((c.cache_write_1h - c.input * 2.0).abs() < 1e-9, "1h: {c:?}");
        }
    }

    /// A 1h write costs 2× input, a 5m write 1.25×, and the 1h figure is a
    /// *subset* of the total — never additive on top of it.
    #[test]
    fn one_hour_subset_is_not_double_charged() {
        let all_1h = TurnUsage {
            cache_creation_tokens: 1_000_000,
            cache_creation_1h_tokens: 1_000_000,
            ..Default::default()
        };
        // Sonnet 4.6: 1M of pure 1h writes = $6.00, not $6.00 + $3.75.
        assert!((turn_cost_usd("claude-sonnet-4-6", &all_1h) - 6.0).abs() < 1e-9);

        // A malformed pair (1h > total) clamps instead of wrapping into an
        // astronomical 5m remainder.
        let malformed = TurnUsage {
            cache_creation_tokens: 1_000_000,
            cache_creation_1h_tokens: 9_000_000,
            ..Default::default()
        };
        assert!((turn_cost_usd("claude-sonnet-4-6", &malformed) - 6.0).abs() < 1e-9);
    }

    #[test]
    fn sonnet_5_is_cheaper_than_sonnet_4_6() {
        // Sonnet 5's $2/$10 became its standard price; 1M in + 1M out = $12.
        let usage = TurnUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        for model in [
            "claude-sonnet-5",
            "Claude-Sonnet-5",
            "claude-sonnet-5[1m]",
            "claude-sonnet-5-20260601",
        ] {
            let cost = turn_cost_usd(model, &usage);
            assert!((cost - 12.0).abs() < 1e-9, "{model} -> {cost}, want 12.0");
        }
        // Earlier Sonnets keep $3/$15.
        assert!((turn_cost_usd("claude-sonnet-4-6", &usage) - 18.0).abs() < 1e-9);
    }

    #[test]
    fn cache_and_websearch() {
        // Sonnet: 100k cache-read ($0.30/M * 0.1 = $0.03) + 10 web searches ($0.01 = $0.10).
        let usage = TurnUsage {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_creation_1h_tokens: 0,
            cache_read_tokens: 100_000,
            web_search_requests: 10,
        };
        let cost = turn_cost_usd("claude-sonnet-4-5", &usage);
        assert!((cost - 0.13).abs() < 1e-9);
    }

    #[test]
    fn mythos_pricing_and_preview_variants() {
        // Mythos 5 is officially $10/$50 per Mtok (same tier as Fable 5).
        // 1M input + 1M output = $10 + $50 = $60.
        let usage = TurnUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        for model in [
            "claude-mythos-preview",
            "claude-mythos-5",
            "claude-mythos-1-20260101",
            "mythos",
            "Claude-Mythos-Preview-20260101",
        ] {
            let cost = turn_cost_usd(model, &usage);
            assert!(
                (cost - 60.0).abs() < 1e-9,
                "model {model} priced wrong: {cost}"
            );
        }
    }

    #[test]
    fn fable_pricing() {
        // Claude Fable 5 / 5.1: $10/$50 per Mtok. 1M input + 1M output = $60.
        // `claude-fable-5-1` is covered by the same substring branch as its
        // predecessor — it ships at the same price, so there is nothing to
        // route separately, but the id must not fall through to a default tier.
        let usage = TurnUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        for model in [
            "claude-fable-5",
            "claude-fable-5-1",
            "fable",
            "Claude-Fable-5",
        ] {
            let cost = turn_cost_usd(model, &usage);
            assert!(
                (cost - 60.0).abs() < 1e-9,
                "model {model} priced wrong: {cost} (expected 60.0 @ $10/$50 tier)"
            );
        }
    }

    #[test]
    fn fable_and_mythos_5_1_read_cache_at_a_quarter_dollar() {
        let reads = TurnUsage {
            cache_read_tokens: 1_000_000,
            ..Default::default()
        };
        for model in [
            "claude-fable-5-1",
            "claude-fable-5-1[1m]",
            "claude-mythos-5-1",
        ] {
            let cost = turn_cost_usd(model, &reads);
            assert!((cost - 0.25).abs() < 1e-9, "{model} -> {cost}, want 0.25");
        }
        // The 5.0 generation (and a dated 5.0 id) still reads at $1.
        for model in [
            "claude-fable-5",
            "claude-fable-5-20260601",
            "claude-mythos-5",
        ] {
            let cost = turn_cost_usd(model, &reads);
            assert!((cost - 1.0).abs() < 1e-9, "{model} -> {cost}, want 1.0");
        }
    }

    #[test]
    fn opus_47_uses_modern_tier() {
        // Regression: substring match against "opus-4" used to mis-route
        // Opus 4.7 into the legacy $15/$75 tier, tripling the cost.
        let usage = TurnUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        for model in [
            "claude-opus-4-7",
            "claude-opus-4-7-20260401",
            "Claude-Opus-4-7",
        ] {
            let cost = turn_cost_usd(model, &usage);
            assert!(
                (cost - 30.0).abs() < 1e-9,
                "model {model} priced wrong: {cost} (expected 30.0 @ $5/$25 tier)"
            );
        }
    }

    #[test]
    fn opus_5_uses_modern_tier() {
        // Opus 5 is $5/$25 per Mtok. The explicit branch pins that instead of
        // leaning on DEFAULT_UNKNOWN_COST, which is the same tier today but is
        // a fallback, not a statement about Opus 5.
        let usage = TurnUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        for model in ["claude-opus-5", "Claude-Opus-5", "claude-opus-5-20260801"] {
            let cost = turn_cost_usd(model, &usage);
            assert!(
                (cost - 30.0).abs() < 1e-9,
                "model {model} priced wrong: {cost} (expected 30.0 @ $5/$25 tier)"
            );
        }
    }

    #[test]
    fn opus_5_5_is_cheaper_than_opus_5() {
        // Opus 5.5 is $4/$20, a 20% cut from Opus 5's $5/$25. The regression
        // this pins: `claude-opus-5-5` contains `opus-5`, so without the minor
        // branch it lands on the $5/$25 tier and every turn is overcharged.
        let usage = TurnUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        for model in [
            "claude-opus-5-5",
            "Claude-Opus-5-5",
            "claude-opus-5-5[1m]",
            "claude-opus-5-5-20260901",
        ] {
            let cost = turn_cost_usd(model, &usage);
            assert!(
                (cost - 24.0).abs() < 1e-9,
                "model {model} priced wrong: {cost} (expected 24.0 @ $4/$20 tier)"
            );
        }

        // Cache reads are input/20 on this tier, not the input/10 every other
        // Claude tier uses.
        let reads = TurnUsage {
            cache_read_tokens: 1_000_000,
            ..Default::default()
        };
        let cost = turn_cost_usd("claude-opus-5-5", &reads);
        assert!(
            (cost - 0.20).abs() < 1e-9,
            "cache read priced wrong: {cost}"
        );
    }

    #[test]
    fn opus_4_original_still_legacy() {
        // The bare Opus 4 ID has no minor version in the string
        // (e.g. "claude-opus-4-20250514" — the trailing digits are the date).
        // It must remain on the legacy $15/$75 tier.
        let usage = TurnUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        let cost_40 = turn_cost_usd("claude-opus-4-20250514", &usage);
        assert!(
            (cost_40 - 90.0).abs() < 1e-9,
            "opus-4.0 priced wrong: {cost_40}"
        );
        let cost_41 = turn_cost_usd("claude-opus-4-1-20250805", &usage);
        assert!(
            (cost_41 - 90.0).abs() < 1e-9,
            "opus-4.1 priced wrong: {cost_41}"
        );
    }

    #[test]
    fn gpt_codex_tiers() {
        // 1M input + 1M output per tier.
        let usage = TurnUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        // 5.6 Sol: $4 + $20 = $24. Bare gpt-5.6 also routes to Sol.
        for model in ["gpt-5.6-sol", "gpt-5.6", "GPT-5.6-Sol"] {
            let cost = turn_cost_usd(model, &usage);
            assert!((cost - 24.0).abs() < 1e-9, "{model} -> {cost}, want 24.0");
        }
        // 5.6 Terra: $2 + $12 = $14.
        assert!((turn_cost_usd("gpt-5.6-terra", &usage) - 14.0).abs() < 1e-9);
        // 5.6 Luna: $0.20 + $1.20 = $1.40.
        assert!((turn_cost_usd("gpt-5.6-luna", &usage) - 1.40).abs() < 1e-9);
        // GPT-6 Sol: $2 + $10 = $12 — not the 5.6 Sol tier.
        assert!((turn_cost_usd("gpt-6-sol", &usage) - 12.0).abs() < 1e-9);
        // GPT-6 Luna: $0.10 + $0.50 = $0.60 — not the 5.6 Luna tier.
        assert!((turn_cost_usd("GPT-6-Luna", &usage) - 0.60).abs() < 1e-9);
        // gpt-5.5 kept its $5 + $30 = $35 when 5.6 was cut.
        assert!((turn_cost_usd("gpt-5.5", &usage) - 35.0).abs() < 1e-9);
        // Codex tier: $1.75 + $14 = $15.75.
        assert!((turn_cost_usd("gpt-5.3-codex", &usage) - 15.75).abs() < 1e-9);
    }

    #[test]
    fn gpt_astra_uses_its_own_standard_short_context_pricing() {
        let costs = get_model_costs("gpt-6-astra");
        assert_eq!(costs.input, 10.0);
        assert_eq!(costs.cache_read, 1.0);
        assert_eq!(costs.cache_write, 12.50);
        assert_eq!(costs.cache_write_1h, 12.50);
        assert_eq!(costs.output, 50.0);

        let usage = TurnUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_read_tokens: 1_000_000,
            ..Default::default()
        };
        assert!((turn_cost_usd("GPT-6-Astra", &usage) - 61.0).abs() < 1e-9);
    }

    #[test]
    fn gpt_cached_input_discount() {
        // 1M cache-read tokens at 90% off input.
        let usage = TurnUsage {
            cache_read_tokens: 1_000_000,
            ..Default::default()
        };
        assert!((turn_cost_usd("gpt-5.6-sol", &usage) - 0.40).abs() < 1e-9);
        assert!((turn_cost_usd("gpt-6-sol", &usage) - 0.20).abs() < 1e-9);
        assert!((turn_cost_usd("gpt-6-luna", &usage) - 0.01).abs() < 1e-9);
    }

    #[test]
    fn unknown_model_falls_back() {
        let usage = TurnUsage {
            input_tokens: 1_000_000,
            output_tokens: 0,
            ..Default::default()
        };
        // Unknown falls back to $5/$25 tier.
        let cost = turn_cost_usd("some-future-model", &usage);
        assert!((cost - 5.0).abs() < 1e-9);
    }
}
