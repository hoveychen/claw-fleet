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
//! + (cache_creation_input_tokens / 1M)* promptCacheWritePrice
//! + (cache_read_input_tokens / 1M)    * promptCacheReadPrice
//! + web_search_requests               * webSearchPrice

#[derive(Clone, Copy, Debug)]
pub struct ModelCosts {
    /// USD per 1M input tokens.
    pub input: f64,
    /// USD per 1M output tokens.
    pub output: f64,
    /// USD per 1M cache-write (creation) tokens.
    pub cache_write: f64,
    /// USD per 1M cache-read tokens.
    pub cache_read: f64,
    /// USD per web-search request.
    pub web_search: f64,
}

// Standard Sonnet tier: $3 / $15 per Mtok.
pub const COST_TIER_3_15: ModelCosts = ModelCosts {
    input: 3.0,
    output: 15.0,
    cache_write: 3.75,
    cache_read: 0.30,
    web_search: 0.01,
};

// Legacy Opus 4 / 4.1: $15 / $75 per Mtok.
pub const COST_TIER_15_75: ModelCosts = ModelCosts {
    input: 15.0,
    output: 75.0,
    cache_write: 18.75,
    cache_read: 1.5,
    web_search: 0.01,
};

// Opus 4.5 / 4.6 default: $5 / $25 per Mtok.
pub const COST_TIER_5_25: ModelCosts = ModelCosts {
    input: 5.0,
    output: 25.0,
    cache_write: 6.25,
    cache_read: 0.5,
    web_search: 0.01,
};

// Opus 4.6 "fast mode": $30 / $150 per Mtok.
pub const COST_TIER_30_150: ModelCosts = ModelCosts {
    input: 30.0,
    output: 150.0,
    cache_write: 37.5,
    cache_read: 3.0,
    web_search: 0.01,
};

// Mythos (incl. -preview): 5x current Opus 4.6 tier = $25 / $125 per Mtok.
pub const COST_TIER_MYTHOS: ModelCosts = ModelCosts {
    input: 25.0,
    output: 125.0,
    cache_write: 31.25,
    cache_read: 2.5,
    web_search: 0.01,
};

// Haiku 3.5: $0.80 / $4 per Mtok.
pub const COST_HAIKU_35: ModelCosts = ModelCosts {
    input: 0.80,
    output: 4.0,
    cache_write: 1.0,
    cache_read: 0.08,
    web_search: 0.01,
};

// Haiku 4.5: $1 / $5 per Mtok.
pub const COST_HAIKU_45: ModelCosts = ModelCosts {
    input: 1.0,
    output: 5.0,
    cache_write: 1.25,
    cache_read: 0.10,
    web_search: 0.01,
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

    // Mythos (and -preview / dated variants). Substring match tolerates
    // `claude-mythos-preview`, `claude-mythos-1-20260101`, etc.
    if m.contains("mythos") {
        return COST_TIER_MYTHOS;
    }

    // Opus tiers (order matters — 4.6/4.5 before the generic 4/4.1 prefix).
    if m.contains("opus-4-6") || m.contains("opus-4-5") {
        return COST_TIER_5_25;
    }
    if m.contains("opus-4-1") || m.contains("opus-4") {
        return COST_TIER_15_75;
    }

    // Sonnet tiers (4.6/4.5/4, 3.7, 3.5 all share $3/$15).
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

/// Raw token counts for one assistant turn.
#[derive(Clone, Copy, Debug, Default)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub web_search_requests: u64,
}

/// Compute USD cost for one assistant turn under the given model.
pub fn turn_cost_usd(model: &str, usage: &TurnUsage) -> f64 {
    let c = get_model_costs(model);
    (usage.input_tokens as f64 / 1_000_000.0) * c.input
        + (usage.output_tokens as f64 / 1_000_000.0) * c.output
        + (usage.cache_creation_tokens as f64 / 1_000_000.0) * c.cache_write
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

    #[test]
    fn cache_and_websearch() {
        // Sonnet: 100k cache-read ($0.30/M * 0.1 = $0.03) + 10 web searches ($0.01 = $0.10).
        let usage = TurnUsage {
            input_tokens: 0,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cache_read_tokens: 100_000,
            web_search_requests: 10,
        };
        let cost = turn_cost_usd("claude-sonnet-4-5", &usage);
        assert!((cost - 0.13).abs() < 1e-9);
    }

    #[test]
    fn mythos_pricing_and_preview_variants() {
        // 1M input + 1M output on Mythos = $25 + $125 = $150.
        let usage = TurnUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..Default::default()
        };
        for model in [
            "claude-mythos-preview",
            "claude-mythos-1-20260101",
            "mythos",
            "Claude-Mythos-Preview-20260101",
        ] {
            let cost = turn_cost_usd(model, &usage);
            assert!(
                (cost - 150.0).abs() < 1e-9,
                "model {model} priced wrong: {cost}"
            );
        }
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
