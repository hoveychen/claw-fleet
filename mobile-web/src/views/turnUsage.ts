// Per-turn usage aggregation for the transcript — one `↑in ↓out · model · time`
// line per assistant *turn* (a maximal run of adjacent assistant records), not
// per API record. Mirrors the desktop's `turnUsageByRow` (messageRows.ts);
// kept as a separate copy because mobile-web is a standalone package that
// shares no code with the desktop app.

import type { RawMessage } from "../types";

export interface TurnUsage {
  /** Context size at the turn's last request. */
  inputTokens: number;
  /** Sum of the turn's records — the tokens this turn produced. */
  outputTokens: number;
  model?: string;
}

/**
 * Map from the index (into `rows`) of each turn's FINAL assistant record to
 * that turn's aggregated usage. `inputTokens` is the last record's (context
 * size, not additive); `outputTokens` sums the run. Rows without usage (old
 * servers strip it; optimistic echoes never have it) yield no entry.
 */
export function turnUsageByIndex(rows: RawMessage[]): Map<number, TurnUsage> {
  const out = new Map<number, TurnUsage>();
  let start = -1; // first index of the current assistant run, -1 = none
  const flush = (endExclusive: number) => {
    if (start < 0) return;
    let outputTokens = 0;
    let inputTokens = 0;
    let model: string | undefined;
    let sawUsage = false;
    for (let i = start; i < endExclusive; i++) {
      const m = rows[i].message;
      if (m?.usage) {
        sawUsage = true;
        outputTokens += m.usage.output_tokens ?? 0;
        inputTokens = m.usage.input_tokens ?? inputTokens;
      }
      if (m?.model) model = m.model;
    }
    if (sawUsage) out.set(endExclusive - 1, { inputTokens, outputTokens, model });
    start = -1;
  };
  rows.forEach((row, i) => {
    if (row.type === "assistant") {
      if (start < 0) start = i;
    } else {
      flush(i);
    }
  });
  flush(rows.length);
  return out;
}

/** "claude-opus-4-8" → "opus", "gpt-5.6-sol" → "sol" — the short family name
 *  the usage line shows. Unknown ids pass through untouched. */
export function shortModelName(model?: string): string | undefined {
  if (!model) return undefined;
  const m = /^claude-([a-z]+)/.exec(model);
  if (m) return m[1];
  const g = /^gpt-[\d.]+-([a-z]+)/.exec(model);
  if (g) return g[1];
  return model;
}

/** 4832 → "4.8k", 812 → "812" — compact token counts for chips. */
export function fmtTokens(n: number): string {
  if (n >= 1000) {
    const k = n / 1000;
    return `${k >= 100 ? Math.round(k) : Math.round(k * 10) / 10}k`;
  }
  return String(n);
}
