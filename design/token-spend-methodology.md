# Token Spend Attribution — Methodology

How the Token Spend tab in Session Detail decomposes a task's billed tokens into named source buckets. Implementation lives in [`claw-fleet-core/src/token_analysis.rs`](../claw-fleet-core/src/token_analysis.rs); the desktop panel is [`TokenSpendPanel.tsx`](../claw-fleet-desktop/app/components/TokenSpendPanel.tsx).

## The headline finding

> **86% of a Claude Code session's billed "new content" tokens are NOT in the JSONL transcript** — they are harness-injected content (system prompt + `~/.claude/CLAUDE.md` + project CLAUDE.md + memory files + skill manifest + fleet reminders + cached tool definitions) that gets re-promoted to cache on every TTL expiry (5 min for subagents, 1 hour for main sessions).

This means a naïve "JSONL chars × `k`" estimator captures only 14% of the cost. To get to a useful attribution we need to (a) read the same disk files the harness injects, (b) recognize TTL refresh events as a billing-overhead bucket separate from originating content, and (c) accept that some residual will always remain because the model's extended thinking output and a few harness behaviours are invisible.

## Spike data — validation we ran before writing the Rust port

Four Claude Code main sessions + five subagent JSONLs from `~/.claude/projects/-Users-hoveychen-workspace-claude-fleet/` (1,986 assistant messages, ~17.6 MB).

| Methodology version | Approach | Residual | Coverage |
|---|---|---:|---:|
| v2 | OLS regression on JSONL-visible chars only | 86% | 14% |
| v4 | + disk snapshot (CLAUDE.md / memory / skills / tool defs estimate) + chars/4 + bundle on first turn + TTL refresh classified at `cache_creation > 10k` | 29% | 71% |
| **v5 (production)** | Same as v4, TTL threshold lowered to `cache_creation > 5000` | **21%** | **79%** |

Per-session residual on the same dataset (smaller is better):

| Session | n | new (M tok) | v2 | v5 |
|---|---:|---:|---:|---:|
| ef388d02 (main, opus) | 446 | 1.86 | 87% | 51% |
| a7e699df (main, opus, longest) | 909 | 4.11 | 87% | 75% |
| 07f22e9d (main, opus, recent) | 231 | 0.92 | 60% | 53% |
| 109a7d83 (main, opus) | 198 | 0.71 | 70% | 55% |
| agent-abad… (sub, haiku) | 66 | 0.16 | 68% | **17%** |
| agent-aaaa… (sub, haiku) | 33 | 0.10 | 71% | **0%** |
| agent-ae8e… (sub, haiku) | 16 | 0.12 | 75% | **0%** |
| agent-ab5d… (sub, haiku) | 49 | 0.12 | 62% | **10%** |
| agent-ad3d… (sub, haiku) | 38 | 0.16 | 79% | **11%** |

Subagent residual is essentially clean. Main-session residual is dominated by `ttl_refresh_overhead` — every TTL expiry re-promotes the entire prompt prefix (bundle + history) to cache; this is a billing event, not new content. The user-facing story for a long opus session is "TTL refresh × N events cost you $Y just for re-caching" — actionable.

## Inputs the methodology reads

### From the JSONL transcript
- Every `assistant` message's `usage.{input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens}` + the `cache_creation.{ephemeral_5m, ephemeral_1h}_input_tokens` subfields.
- Each assistant message's content blocks → text / thinking / tool_use char counts (for output attribution).
- Each user message's content → user-text / tool_result chars, with `<system-reminder>...</system-reminder>` regex-extracted into its own bucket.
- `{"type":"system","subtype":"compact_boundary"}` lines mark the end of the pre-compact conversation; the immediately-following user message is treated as the compacted-summary placeholder.
- `<session-id>/subagents/agent-*.jsonl` sibling files for recursive subagent walks.

### From disk (at analysis time)
- `~/.claude/CLAUDE.md`
- `<project-root>/CLAUDE.md` if a project_root is provided
- `~/.claude/fleet-interaction-mode.md` + `~/.claude/fleet-prd-discipline.md` (these are `@`-imported by the user's CLAUDE.md)
- `~/.claude/projects/<encoded-path>/memory/MEMORY.md` (the index; linked memory files only get loaded when explicitly read, so they show up in `tool_result_chars`, not here)
- `~/.claude/skills/*/SKILL.md` — frontmatter portion only (the YAML between the leading `---` markers — that's what gets stamped into the available-skills system reminder, the body is loaded on Skill invocation)

### Hardcoded constants (revisit on Claude Code version bumps)
- `CC_SYSTEM_PROMPT_TOKENS = 2,800` — base Claude Code system prompt
- `STOCK_TOOL_DEFS_TOKENS = 8,000` — built-in tool schemas (Read/Bash/Edit/Write/Grep/Glob/Agent/WebFetch/Skill/ToolSearch/TodoWrite/ExitPlanMode etc.). Excludes MCP tools, which expand per server.

A future `fleet recalibrate-tokens` subcommand can replace both constants with exact values from one-shot Anthropic `count_tokens` API calls per Claude Code version.

## Algorithm — per assistant message

`new_content_tokens = input_tokens + cache_creation_input_tokens`. This is "what got promoted into cache or paid full-price this turn".

1. **First turn of the JSONL** — attribute `BUNDLE_TOKENS` to the disk-snapshot buckets:
   - `cc_base_system_prompt` += `CC_SYSTEM_PROMPT_TOKENS`
   - `tool_defs` += `STOCK_TOOL_DEFS_TOKENS`
   - `user_claudemd`, `project_claudemd`, `fleet_reminders`, `memory_files`, `skills_manifest` += their disk-read sizes.
   - Visible content (this turn's user_text/tool_result/etc.) also gets attributed via `chars/4`.
   - Whatever `new_content_tokens` is left over → `residual_unexplained`.

2. **`cache_creation_input_tokens > 5000` on a later turn** — classify as TTL refresh:
   - `ttl_refresh_overhead` += `new_content_tokens` (the entire turn).
   - Don't slice the refresh further: the underlying content was already counted on the first-turn bundle, and slicing again would double-count.

3. **Steady-state turn** — attribute `new_content_tokens` to JSONL-visible content via `chars/4`:
   - `visible_user_text`, `visible_tool_result`, `visible_system_reminder`, `visible_prev_assistant`, `visible_compact_summary` += their respective chars / 4.
   - Anything left → `residual_unexplained`.

4. **Output side** — walk the assistant message's content blocks:
   - `text` block chars / 4 → `output_text`
   - `thinking.text` chars / 4 → `output_thinking_visible` (usually 0 on Opus; Anthropic does not emit the reasoning text into the transcript)
   - `tool_use.input + name` chars / 4 → `output_tool_use`
   - `output_tokens − Σ(visible output blocks)` → `output_reasoning_invisible` (this captures Opus extended thinking which is billed but invisible)

### Token-count heuristic

Why `chars / 4`? Claude Code's own `roughTokenCountEstimation` ([claude-code-fork ref](../../claude-code-fork/src/services/tokenEstimation.ts)) uses `bytesPerToken = 4` for plain text and `bytesPerToken = 2` for `.json` / `.jsonl` files. Spike validation showed `chars/4` universal is accurate enough: tool-result JSON is overcounted by ~50% but its share of the bill is small (<5%), so the bucket-level error stays within the ±20% UI tolerance.

For accurate token counts, Claude Code calls Anthropic's `count_tokens` API. Fleet doesn't have that luxury at analysis time without the user's API key, so we accept the `chars/4` ±20% noise.

## Why the 14 buckets are what they are

The 14 input buckets fall into three families:

1. **Bundle (disk-snapshot, one-time-per-bundle-load)** — `cc_base_system_prompt`, `tool_defs`, `user_claudemd`, `project_claudemd`, `fleet_reminders`, `memory_files`, `skills_manifest`. Each gets counted once per "bundle injection" event (turn 1 of each session). Long sessions add multiple bundle loads at compact boundaries.

2. **Visible (from JSONL, conversational content)** — `visible_user_text`, `visible_tool_result`, `visible_system_reminder`, `visible_prev_assistant`, `visible_compact_summary`. These accumulate across all steady-state turns.

3. **Cache mechanics** — `ttl_refresh_overhead` (cache TTL expired, cached prefix got re-promoted), `residual_unexplained` (chars/4 slippage + sub-5k cache events).

The 4 output buckets are simpler — they correspond to content-block kinds plus the residual for invisible extended thinking.

## What we explicitly don't do

- **Don't try to slice `ttl_refresh_overhead`.** The TTL-refresh cost IS the bundle + history being re-cached. Source-content tokens were already counted on the first load. Slicing would double-count and confuse the UI.
- **Don't attribute `cache_read_input_tokens` by source.** It's reported in `UsageTotals` as a separate billing line. Cache reads are 10× cheaper than creations but accumulate to ~56% of the dollar bill on long sessions; the UI shows the totals but not a per-source slice.
- **Don't pretend the disk snapshot is the historical truth.** Files may have changed since the session ran. The Caveat footer in the UI says so.
- **Don't track MCP tools server-by-server** in v1. MCP tool defs vary too much per session; for sessions with MCPs the `tool_defs` bucket undercounts. Acceptable tradeoff for v1.

## Cost calculation

Anthropic public pricing snapshot (per million tokens), 2026-05:

| Model | input | cache_create_5m | cache_create_1h | cache_read | output |
|---|---:|---:|---:|---:|---:|
| Opus 4.x | $15 | $18.75 | $30 | $1.50 | $75 |
| Sonnet 4.x | $3 | $3.75 | $6 | $0.30 | $15 |
| Haiku 4.5 | $1 | $1.25 | $2 | $0.10 | $5 |

Model is matched against the JSONL's `usage.model` string (substring match for "haiku" / "sonnet", else default to opus rates).

Spike validation: 4 main + 5 subagent sessions estimated $843.75 total. Of that, $476 (56%) was `cache_read`, $225 was `cache_creation`, $150 was `output`. The TTL refresh story holds — cache mechanics dominate the bill.

## Known limitations

- **Long opus sessions have >50% residual lumped as `ttl_refresh_overhead`.** This is correct behaviour, not a bug. The UI presents it as a single bucket because slicing it would either double-count source content or fabricate detail we can't validate.
- **`cc_base_system_prompt` and `tool_defs` are hardcoded.** They drift when Claude Code updates. Acceptable noise: combined they're ~10.8k tokens, dwarfed by the ~70k bundle for a heavy user.
- **Disk snapshot doesn't track historical state.** If you compare a session ran 2 weeks ago against current CLAUDE.md state, expect bundle-level differences. Sessions older than the most recent CLAUDE.md edit will show wrong bundle attribution.
- **MCP tool defs not measured.** Sessions using heavy MCP setups will see those tokens absorbed into `residual_unexplained` or `ttl_refresh_overhead`.

## When to recalibrate

- Claude Code minor version bumps: spot-check first-turn `cache_creation_input_tokens` for a fresh session vs current `bundle_size_tokens`. Drift > 20% → time to re-tune the constants.
- New stock tools added to CC: bump `STOCK_TOOL_DEFS_TOKENS`.
- Significant prompt redesign: bump `CC_SYSTEM_PROMPT_TOKENS`.

For a one-shot calibration, run any post-2026-05 Claude Code session and inspect the first assistant message's `usage.cache_creation_input_tokens`. Subtract the user's measurable disk-snapshot bundle (CLAUDE.md / memory / skills sums). Whatever's left ≈ `CC_SYSTEM_PROMPT_TOKENS + STOCK_TOOL_DEFS_TOKENS`.

## Re-running the spike locally

The spike script is preserved at `/tmp/token_spend_spike/fit_v3.mjs`. To re-validate against new sessions:

```sh
node /tmp/token_spend_spike/fit_v3.mjs <main-jsonl> [<main-jsonl> ...]
# Auto-discovers <session>/subagents/agent-*.jsonl
# Auto-reads disk snapshot from ~/.claude/ and the claude-fleet project root
```

The Rust port has a debug helper:

```sh
cargo run -p claw-fleet-core --example token_sanity_check -- <main-jsonl-path>
```

Output: total attribution breakdown for one task tree (main + subagents). Use this for spot-checks when modifying `token_analysis.rs`.
