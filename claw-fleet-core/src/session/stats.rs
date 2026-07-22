use super::*;

// ── Context window helpers ────────────────────────────────────────────────────

/// Whether a Claude model belongs to the family that can be opted in to a
/// Parse the `major.minor` version that follows a Claude family token in a
/// model id, e.g. `claude-opus-4-8` + `"opus"` → `Some((4, 8))`. Returns
/// `None` for aliases without a version (`"opus"`, `"sonnet"`) or when the
/// family token isn't present. Trailing date stamps (`claude-opus-4-1-2026…`)
/// are ignored — only the first two numeric tokens after the family count.
fn claude_family_version(model_lower: &str, family: &str) -> Option<(u32, u32)> {
    let rest = model_lower.split(family).nth(1)?;
    let mut nums = rest
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<u32>().ok());
    let major = nums.next()?;
    let minor = nums.next().unwrap_or(0);
    Some((major, minor))
}

/// Whether a Claude model ships with a 1M-token context window. Verified
/// against Anthropic's model docs (2026-05): the 1M window landed at the **.6**
/// minor for BOTH Opus and Sonnet, and every later major carries it forward.
/// Parsing the family + `major.minor` (instead of hard-coding each release)
/// means future minors (Opus 4.9, 4.10) and future majors (5.x, 6.x) are
/// recognised without another edit:
///   * Opus & Sonnet — 4.6+ within major 4, and every later major (5.x …).
///   * Mythos research preview (Project Glasswing) — always 1M.
///   * Opus ≤4.5, Sonnet ≤4.5, all Haiku, Claude 3.x — 200K.
///
/// Note Sonnet 4 / 4.5 are 200K models: Sonnet 4's brief 1M was a public-beta
/// header, not the default, and Sonnet 4.5 never shipped 1M.
fn claude_model_supports_1m(model_lower: &str) -> bool {
    // Always-1M families (Fable, Mythos) are handled unconditionally by
    // `claude_model_always_1m` before this version-gated check is reached, so
    // they're intentionally not matched here.
    // Opus and Sonnet share the same 4.6+ gate; Haiku never qualifies.
    for family in ["opus", "sonnet"] {
        if let Some((major, minor)) = claude_family_version(model_lower, family) {
            return major > 4 || (major == 4 && minor >= 6);
        }
    }
    false
}

/// Claude families whose id carries no parseable `family + major.minor`
/// version, but which nonetheless ship a 1M window — so the opus/sonnet
/// version gate can't recognise them and they're matched by family substring:
///   * Claude Fable 5 (`claude-fable-5`) — always 1M.
///   * Mythos / Mythos research preview (Project Glasswing) — always 1M.
///
/// Every native-1M family (these plus opus/sonnet 4.6+) now reports 1M
/// unconditionally from the first turn; see [`context_window_for_model`].
fn claude_model_always_1m(model_lower: &str) -> bool {
    model_lower.contains("fable") || model_lower.contains("mythos")
}

/// Best-effort lookup of a model's input-context-window size (in tokens).
///
/// `_observed_max_input_tokens` (the max `input + cache_creation + cache_read`
/// seen across the session's turns) is **no longer consulted** — it's retained
/// only to keep the signature stable for callers that still thread it through
/// [`compute_context_percent`]. It used to gate an "inferred 1M" fallback back
/// when a `claude-opus-4-6`/`4-7`/`4-8` id on disk was ambiguous between a 200K
/// and a 1M session (Claude Code never writes the `[1m]` flag to JSONL). That
/// ambiguity is gone: every 1M-capable Claude family (Opus/Sonnet 4.6+, 5.x,
/// Fable, Mythos) ships 1M as its **default and only** window — there is no
/// 200K variant to disambiguate — so they report 1M from the first turn. The
/// old fallback under-reported a fresh native-1M session as 200K and inflated
/// the ctx-% badge ~5×.
///
/// Returns `None` when the model family is unrecognised so the caller can
/// decide whether to fall back to a default or skip the computation entirely.
pub fn context_window_for_model(model: &str, _observed_max_input_tokens: u64) -> Option<u64> {
    let m = model.to_lowercase();

    // ── Anthropic / Claude ──────────────────────────────────────────────
    if m.starts_with("claude-")
        || m == "opus"
        || m == "sonnet"
        || m == "haiku"
        || claude_model_always_1m(&m)
    {
        // Native-1M families — Opus/Sonnet 4.6+ and every later major, plus the
        // always-1M families (Fable, Mythos), plus the explicit `[1m]` suffix
        // marker. All ship 1M as their default, sole window (no 200K variant),
        // so report it unconditionally, from the very first turn.
        if m.contains("[1m]") || claude_model_always_1m(&m) || claude_model_supports_1m(&m) {
            return Some(1_000_000);
        }
        // Everything else — Opus/Sonnet ≤4.5, all Haiku, Claude 3.x — is a
        // genuine 200K model.
        return Some(200_000);
    }

    // ── OpenAI ──────────────────────────────────────────────────────────
    // GPT-5 family (incl. Codex slugs gpt-5.6-sol/luna/terra, gpt-5.5, gpt-5.4,
    // gpt-5.1-codex-*): advertised 400K context. This is only a fallback —
    // Codex reports the precise per-session effective window via
    // token_count.model_context_window, which extract_context_percent prefers.
    if m.starts_with("gpt-5") {
        return Some(400_000);
    }
    // o3 / o4-mini: 200 000
    if m.starts_with("o3") || m.starts_with("o4") {
        return Some(200_000);
    }
    // GPT-4o / GPT-4o-mini: 128 000
    if m.starts_with("gpt-4o") {
        return Some(128_000);
    }
    // GPT-4.1: 1 048 576
    if m.starts_with("gpt-4.1") {
        return Some(1_048_576);
    }
    // GPT-4-turbo / GPT-4-1106+: 128 000
    if m.starts_with("gpt-4-turbo") || m.starts_with("gpt-4-1106") || m.starts_with("gpt-4-0125") {
        return Some(128_000);
    }
    // GPT-4 (base 8k)
    if m.starts_with("gpt-4") {
        return Some(8_192);
    }

    // ── Google ──────────────────────────────────────────────────────────
    if m.contains("gemini") {
        return Some(1_000_000);
    }

    None
}

/// Compute context-window utilisation (0.0 – 1.0) from raw token counts.
///
/// `observed_max_input_tokens` (the largest single-turn total input seen across
/// the session) is forwarded to [`context_window_for_model`] for signature
/// compatibility but is no longer consulted — the window is now determined by
/// model family alone. Pass `0` when unknown.
///
/// Returns `None` when the model is unrecognised.
pub fn compute_context_percent(
    input_tokens: u64,
    model: Option<&str>,
    observed_max_input_tokens: u64,
) -> Option<f64> {
    let window = context_window_for_model(model?, observed_max_input_tokens)?;
    if window == 0 {
        return None;
    }
    Some((input_tokens as f64 / window as f64).min(1.0))
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SessionStats {
    /// Tokens/sec over the last 5-minute window.
    pub token_speed: f64,
    /// Cumulative output tokens across all finalized assistant turns.
    pub total_output_tokens: u64,
    /// Cumulative input tokens across all finalized assistant turns —
    /// `Σ(input_tokens + cache_creation_input_tokens + cache_read_input_tokens)`.
    /// This is the "tokens actually sent to the API" total (cache re-reads
    /// included), on the same口径 as `total_cost_usd`, NOT the last-turn
    /// context-window snapshot. The snapshot lives separately in
    /// `SessionAcc::context_usage` and still drives `context_percent`.
    pub total_input_tokens: u64,
    /// Cumulative USD cost across all finalized assistant turns.
    pub total_cost_usd: f64,
    /// USD/min over the last 5-minute window.
    pub cost_speed_usd_per_min: f64,
    /// Number of `compact_boundary` events in the transcript.
    pub compact_count: u32,
    /// Sum of `compactMetadata.preTokens` across all compact events
    /// (context size before each compaction).
    pub compact_pre_tokens: u64,
    /// Sum of `compactMetadata.postTokens` across all compact events
    /// (summary size produced by each compaction).
    pub compact_post_tokens: u64,
    /// Estimated USD cost spent on compact LLM calls. The compact
    /// invocation itself is not recorded as a separate assistant turn,
    /// so we approximate as `cache_read_price × preTokens +
    /// output_price × postTokens` using the model that was active just
    /// before each compaction.
    pub compact_cost_usd: f64,
}

/// Running state behind [`compute_session_stats`], split out so a session's
/// stats can be advanced by only the lines appended since the last scan.
///
/// Transcripts are append-only, so every field here is either a running sum, a
/// running max, or a last-write-wins scalar — all of which fold correctly over
/// batches. The one field that does *not* is `seen_msg_ids`: a finalized
/// assistant message can be re-logged, and dedup is what keeps its tokens and
/// cost from being counted twice.
///
/// **`seen_msg_ids` must stay complete for the whole file — a bounded window is
/// not sound.** Measured across the 120 largest transcripts on disk: every one
/// of them re-logs message ids (42k duplicate instances), and while most repeats
/// sit within 10 lines of each other, 100 of them span more than 100 lines and
/// the widest is 2437. A sliding window would silently miss those and
/// double-count the turn's tokens and USD.
///
/// Ids are stored as 64-bit hashes rather than strings to keep the per-session
/// footprint at ~8 bytes/turn; a collision would drop one turn from the totals,
/// which at a few thousand ids against a 64-bit space is not a real risk.
#[derive(Clone, Debug, Default)]
pub struct StatsAcc {
    total_output: u64,
    total_input: u64,
    total_cost: f64,
    /// (timestamp_secs, output_tokens, turn_cost_usd) — feeds the speed window.
    timed: Vec<(f64, u64, f64)>,
    seen_msg_ids: HashSet<u64>,
    last_model: Option<String>,
    compact_count: u32,
    compact_pre_tokens: u64,
    compact_post_tokens: u64,
    compact_cost_usd: f64,
}

fn hash_msg_id(id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

impl StatsAcc {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold a batch of newly-appended transcript lines into the running state.
    /// Safe to call repeatedly; re-feeding a line that was already folded in is
    /// a no-op for token/cost totals thanks to the msg-id dedup above.
    pub fn push_lines(&mut self, lines: &[&str]) {
        for line in lines {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                self.push_value(&v);
            }
        }
    }

    /// Fold one already-parsed record. Split out from [`push_lines`] so that
    /// [`SessionAcc`] can parse each line once and fan the same `Value` out to
    /// every extractor, instead of re-parsing the file per extractor.
    pub fn push_value(&mut self, v: &Value) {
        use crate::model_cost::{get_model_costs, turn_cost_usd, TurnUsage};

        {
            // `compact_boundary` is a system meta event Claude Code emits each time
            // it summarises the conversation. The summary LLM call itself is not
            // logged as a standalone assistant turn, so its true cost is not in
            // the transcript — we approximate from `compactMetadata`.
            if v.get("type").and_then(|t| t.as_str()) == Some("system")
                && v.get("subtype").and_then(|s| s.as_str()) == Some("compact_boundary")
            {
                self.compact_count += 1;
                let meta = v.get("compactMetadata");
                let pre = meta
                    .and_then(|m| m.get("preTokens"))
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0);
                let post = meta
                    .and_then(|m| m.get("postTokens"))
                    .and_then(|t| t.as_u64())
                    .unwrap_or(0);
                self.compact_pre_tokens += pre;
                self.compact_post_tokens += post;

                // Price the compact call against the most recently seen model.
                // `get_model_costs("")` falls back to the default tier when no
                // assistant turn has been seen yet (defensive — compact almost
                // never precedes the first assistant turn).
                let pricing_model = self.last_model.as_deref().unwrap_or("");
                let costs = get_model_costs(pricing_model);
                self.compact_cost_usd += (pre as f64 / 1_000_000.0) * costs.cache_read
                    + (post as f64 / 1_000_000.0) * costs.output;
                return;
            }

            if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
                return;
            }
            let Some(msg) = v.get("message").and_then(|m| m.as_object()) else {
                return;
            };
            // Only count finalized messages
            if msg.get("stop_reason").map_or(true, |s| s.is_null()) {
                return;
            }
            let msg_id = msg.get("id").and_then(|i| i.as_str()).unwrap_or_default();
            if !msg_id.is_empty() && !self.seen_msg_ids.insert(hash_msg_id(msg_id)) {
                return;
            }

            let usage = msg.get("usage");
            let input_tokens = usage
                .and_then(|u| u.get("input_tokens"))
                .and_then(|t| t.as_u64())
                .unwrap_or(0);
            let output_tokens = usage
                .and_then(|u| u.get("output_tokens"))
                .and_then(|t| t.as_u64())
                .unwrap_or(0);
            let cache_creation_tokens = usage
                .and_then(|u| u.get("cache_creation_input_tokens"))
                .and_then(|t| t.as_u64())
                .unwrap_or(0);
            let cache_read_tokens = usage
                .and_then(|u| u.get("cache_read_input_tokens"))
                .and_then(|t| t.as_u64())
                .unwrap_or(0);
            let web_search_requests = usage
                .and_then(|u| u.get("server_tool_use"))
                .and_then(|s| s.get("web_search_requests"))
                .and_then(|t| t.as_u64())
                .unwrap_or(0);

            self.total_output += output_tokens;
            // Cumulative input across finalized turns, cache re-reads included, so
            // the total tracks `total_cost` (which also folds every turn) rather
            // than the last-turn context-window snapshot.
            self.total_input += input_tokens + cache_creation_tokens + cache_read_tokens;

            // Per-turn cost uses this turn's own model; fall back to most-recently-
            // seen model when a turn omits it (model can change mid-session).
            let turn_model = msg.get("model").and_then(|m| m.as_str());
            if let Some(m) = turn_model {
                self.last_model = Some(m.to_string());
            }
            let cost_model = turn_model.or(self.last_model.as_deref()).unwrap_or("");
            let turn_cost = turn_cost_usd(
                cost_model,
                &TurnUsage {
                    input_tokens,
                    output_tokens,
                    cache_creation_tokens,
                    cache_read_tokens,
                    web_search_requests,
                },
            );
            self.total_cost += turn_cost;

            // Timestamp for speed
            if let Some(ts_str) = v.get("timestamp").and_then(|t| t.as_str()) {
                if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                    self.timed
                        .push((dt.timestamp() as f64, output_tokens, turn_cost));
                }
            }
        }
    }

    /// Materialise the stats. `now_secs` is the clock the speed window is
    /// measured against — taken as a parameter so the window is testable
    /// without freezing the system clock.
    pub fn finish_at(&self, now_secs: f64) -> SessionStats {
        // Speed: tokens/s and cost/min over the last 5-minute window.
        //
        // Divide by `now - first_ts`, not `last_ts - first_ts`. The inter-turn
        // gap version makes speed a step function that holds the old rate until
        // the oldest turn slides out of the window — so a session that finished
        // a burst 4 minutes ago still reports the burst's speed, inflating
        // fleet-wide totals while nothing is actually generating. Measuring
        // against "now" lets speed decay smoothly as the idle tail grows.
        let (token_speed, cost_speed_usd_per_min) = if self.timed.len() >= 2 {
            let window_start = now_secs - 300.0;

            let recent: Vec<_> = self
                .timed
                .iter()
                .filter(|(ts, _, _)| *ts > window_start)
                .collect();

            if recent.len() >= 2 {
                let total_recent_tokens: u64 = recent.iter().map(|(_, t, _)| t).sum();
                let total_recent_cost: f64 = recent.iter().map(|(_, _, c)| c).sum();
                let first_ts = recent.first().map(|(ts, _, _)| *ts).unwrap_or(0.0);
                let duration = now_secs - first_ts;
                if duration > 0.0 {
                    (
                        total_recent_tokens as f64 / duration,
                        total_recent_cost * 60.0 / duration,
                    )
                } else {
                    (0.0, 0.0)
                }
            } else {
                (0.0, 0.0)
            }
        } else {
            (0.0, 0.0)
        };

        SessionStats {
            token_speed,
            total_output_tokens: self.total_output,
            total_input_tokens: self.total_input,
            total_cost_usd: self.total_cost,
            cost_speed_usd_per_min,
            compact_count: self.compact_count,
            compact_pre_tokens: self.compact_pre_tokens,
            compact_post_tokens: self.compact_post_tokens,
            compact_cost_usd: self.compact_cost_usd,
        }
    }

    /// Same as [`finish_at`], against the wall clock.
    pub fn finish(&self) -> SessionStats {
        self.finish_at(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
        )
    }

    /// Drop speed samples that can no longer influence the 5-minute window.
    /// Called as the accumulator is carried across scans so `timed` doesn't
    /// grow without bound on a long-lived session. The margin over 300s is
    /// deliberate slack — pruning exactly at the window edge would race the
    /// next `finish` call's slightly later clock.
    pub fn prune_timed(&mut self, now_secs: f64) {
        let keep_after = now_secs - 900.0;
        self.timed.retain(|(ts, _, _)| *ts > keep_after);
    }
}

/// The batch-free original, kept as the oracle [`SessionAcc`]'s tests compare
/// against — the scan path itself now folds incrementally and no longer calls it.
#[cfg(test)]
pub(crate) fn compute_session_stats(lines: &[&str]) -> SessionStats {
    let mut acc = StatsAcc::new();
    acc.push_lines(lines);
    acc.finish()
}

#[cfg(test)]
mod input_accumulation_tests {
    use super::*;

    fn turn(id: &str, input: u64, cache_create: u64, cache_read: u64, output: u64) -> String {
        serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-07-16T10:00:00Z",
            "message": {
                "id": id,
                "model": "claude-sonnet-4-5",
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": input,
                    "cache_creation_input_tokens": cache_create,
                    "cache_read_input_tokens": cache_read,
                    "output_tokens": output
                }
            }
        })
        .to_string()
    }

    #[test]
    fn total_input_accumulates_input_plus_cache_across_turns() {
        // Two finalized turns; each turn's input = input + cache_create + cache_read.
        // Turn 1: 100 + 2000 + 0     = 2100
        // Turn 2: 150 + 0    + 5000  = 5150   (cache re-read of turn 1's context)
        // Cumulative must be 7250 — NOT the last-turn snapshot (5150).
        let lines = vec![
            turn("m1", 100, 2000, 0, 30),
            turn("m2", 150, 0, 5000, 40),
        ];
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let stats = compute_session_stats(&refs);
        assert_eq!(stats.total_input_tokens, 7250, "cumulative input incl cache");
        assert_eq!(stats.total_output_tokens, 70);
    }

    #[test]
    fn duplicate_msg_id_not_double_counted() {
        // Re-logging the same finalized message must not double its input.
        let lines = vec![turn("dup", 100, 0, 900, 10), turn("dup", 100, 0, 900, 10)];
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let stats = compute_session_stats(&refs);
        assert_eq!(stats.total_input_tokens, 1000, "dedup by msg id");
    }
}

#[cfg(test)]
mod context_window_tests {
    use super::*;

    // Native-1M families (Opus/Sonnet 4.6+, 5.x) ship 1M as the *only* window —
    // there is no 200K variant to disambiguate. So the reported window must be
    // 1M from the first turn, with NO observed-input evidence
    // (`observed_max_input_tokens = 0`). This is the regression the ctx% badge
    // exposed: an Opus 4.8 session under 195K input was read as a 200K window,
    // inflating utilisation ~5×.
    #[test]
    fn native_1m_families_report_1m_without_observed_input() {
        for model in [
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-sonnet-5",
            "claude-sonnet-4-6",
            "claude-opus-4-8[1m]",
        ] {
            assert_eq!(
                context_window_for_model(model, 0),
                Some(1_000_000),
                "{model} is native 1M and must report 1M with zero observed input",
            );
        }
    }

    // The ctx% for the reported session: ~188K last-turn input against Opus
    // 4.8's true 1M window is ~19%, not the 94% a 200K denominator produced.
    #[test]
    fn opus_4_8_context_percent_uses_1m_denominator() {
        let pct = compute_context_percent(188_000, Some("claude-opus-4-8"), 0).unwrap();
        assert!(
            (0.15..0.25).contains(&pct),
            "expected ~0.19 against a 1M window, got {pct}",
        );
    }

    // Guard the boundary: models that genuinely default to 200K must stay 200K,
    // so the fix doesn't over-reach. Opus/Sonnet ≤4.5, all Haiku, Claude 3.x.
    #[test]
    fn pre_4_6_and_haiku_stay_200k() {
        for model in [
            "claude-opus-4-5",
            "claude-sonnet-4-5",
            "claude-sonnet-4-0",
            "claude-haiku-4-5",
            "claude-3-5-haiku-20241022",
        ] {
            assert_eq!(
                context_window_for_model(model, 0),
                Some(200_000),
                "{model} defaults to a 200K window",
            );
        }
    }
}

