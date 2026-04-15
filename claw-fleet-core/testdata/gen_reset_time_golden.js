#!/usr/bin/env node
// Generates golden outputs for formatResetTime() reverse-engineering tests.
// The formatResetTime function below is copied verbatim from
// claude-code-fork/src/utils/format.ts so golden outputs exactly match
// real Claude Code runtime behavior.
//
// Usage: node gen_reset_time_golden.js > reset_time_golden.jsonl
// Re-run this whenever claude-code-fork/src/utils/format.ts changes.

function getTimeZone() {
  return Intl.DateTimeFormat().resolvedOptions().timeZone;
}

// VERBATIM from claude-code-fork/src/utils/format.ts (formatResetTime)
function formatResetTime(timestampInSeconds, showTimezone = false, showTime = true) {
  if (!timestampInSeconds) return undefined;
  const date = new Date(timestampInSeconds * 1000);
  const now = new Date();
  const minutes = date.getMinutes();
  const hoursUntilReset = (date.getTime() - now.getTime()) / (1000 * 60 * 60);
  if (hoursUntilReset > 24) {
    const dateOptions = {
      month: 'short',
      day: 'numeric',
      hour: showTime ? 'numeric' : undefined,
      minute: !showTime || minutes === 0 ? undefined : '2-digit',
      hour12: showTime ? true : undefined,
    };
    if (date.getFullYear() !== now.getFullYear()) {
      dateOptions.year = 'numeric';
    }
    const dateString = date.toLocaleString('en-US', dateOptions);
    return (
      dateString.replace(/ ([AP]M)/i, (_m, ampm) => ampm.toLowerCase()) +
      (showTimezone ? ` (${getTimeZone()})` : '')
    );
  }
  const timeString = date.toLocaleTimeString('en-US', {
    hour: 'numeric',
    minute: minutes === 0 ? undefined : '2-digit',
    hour12: true,
  });
  return (
    timeString.replace(/ ([AP]M)/i, (_m, ampm) => ampm.toLowerCase()) +
    (showTimezone ? ` (${getTimeZone()})` : '')
  );
}

// IMPORTANT: the `now` used in generation is captured via NOW_UNIX env var
// so tests can reproduce the exact branch selection (>24h vs ≤24h).
// Override Date.now so formatResetTime sees a deterministic "now".
const FIXED_NOW_UNIX = parseInt(process.env.NOW_UNIX || '0', 10);
if (FIXED_NOW_UNIX > 0) {
  const realDate = Date;
  // eslint-disable-next-line no-global-assign
  Date = class extends realDate {
    constructor(...args) {
      if (args.length === 0) {
        super(FIXED_NOW_UNIX * 1000);
      } else {
        super(...args);
      }
    }
    static now() {
      return FIXED_NOW_UNIX * 1000;
    }
  };
  // Preserve static methods we might use
  Date.UTC = realDate.UTC;
  Date.parse = realDate.parse;
}

// ── Test case matrix ─────────────────────────────────────────────────────────
// Each entry: { label, now_iso, resets_at_iso }
// now_iso defines the "generation moment" so the >24h branch is reproducible.
// resets_at_iso is the real rate-limit reset instant.
//
// The TZ is controlled by the TZ env var at invocation. We run this script
// multiple times with different TZ values to cover multiple IANA zones.

const CASES = [
  // Branch B (≤24h): same day, round hour
  { label: 'b_same_day_round', now_iso: '2026-04-15T10:00:00Z', resets_at_iso: '2026-04-15T18:00:00Z' },
  // Branch B (≤24h): same day, non-round minutes
  { label: 'b_same_day_minutes', now_iso: '2026-04-15T10:00:00Z', resets_at_iso: '2026-04-15T18:30:00Z' },
  // Branch B (≤24h): next day wall-clock (because reset crosses midnight in local tz)
  { label: 'b_next_day_round', now_iso: '2026-04-15T20:00:00Z', resets_at_iso: '2026-04-16T06:00:00Z' },
  // Branch B (≤24h): 12am midnight edge — 07:00 UTC = 00:00 PDT/PST = 15:00 CST = 07:00 UTC
  // Note: wall-clock 12am only happens in tz where resets_at % 24h == midnight.
  { label: 'b_midnight_12am_utc', now_iso: '2026-04-14T20:00:00Z', resets_at_iso: '2026-04-15T00:00:00Z' },
  // Branch B (≤24h): 12pm noon edge
  { label: 'b_noon_12pm_utc', now_iso: '2026-04-15T05:00:00Z', resets_at_iso: '2026-04-15T12:00:00Z' },
  // Branch B (≤24h): 1am edge
  { label: 'b_one_am', now_iso: '2026-04-15T05:00:00Z', resets_at_iso: '2026-04-15T09:00:00Z' },
  // Branch B boundary: exactly 24h is NOT >24 → still B
  { label: 'b_boundary_exact_24h', now_iso: '2026-04-15T10:00:00Z', resets_at_iso: '2026-04-16T10:00:00Z' },
  // Branch A (>24h): 25h out, round hour
  { label: 'a_25h_round', now_iso: '2026-04-15T10:00:00Z', resets_at_iso: '2026-04-16T12:00:00Z' },
  // Branch A (>24h): 30h out, non-round minutes
  { label: 'a_30h_minutes', now_iso: '2026-04-15T10:00:00Z', resets_at_iso: '2026-04-16T16:15:00Z' },
  // Branch A: 7 days out (typical weekly limit)
  { label: 'a_7d_round', now_iso: '2026-04-15T10:00:00Z', resets_at_iso: '2026-04-22T10:00:00Z' },
  // Branch A: crosses year boundary
  { label: 'a_cross_year', now_iso: '2026-12-30T10:00:00Z', resets_at_iso: '2027-01-02T19:00:00Z' },
  // Branch A: crosses year, non-round
  { label: 'a_cross_year_minutes', now_iso: '2026-12-30T10:00:00Z', resets_at_iso: '2027-01-02T19:15:00Z' },
  // Every month short-name — 12 cases
  { label: 'a_jan', now_iso: '2026-01-01T00:00:00Z', resets_at_iso: '2026-01-10T15:00:00Z' },
  { label: 'a_feb', now_iso: '2026-02-01T00:00:00Z', resets_at_iso: '2026-02-10T15:00:00Z' },
  { label: 'a_mar', now_iso: '2026-03-01T00:00:00Z', resets_at_iso: '2026-03-10T15:00:00Z' },
  { label: 'a_apr', now_iso: '2026-04-01T00:00:00Z', resets_at_iso: '2026-04-10T15:00:00Z' },
  { label: 'a_may', now_iso: '2026-05-01T00:00:00Z', resets_at_iso: '2026-05-10T15:00:00Z' },
  { label: 'a_jun', now_iso: '2026-06-01T00:00:00Z', resets_at_iso: '2026-06-10T15:00:00Z' },
  { label: 'a_jul', now_iso: '2026-07-01T00:00:00Z', resets_at_iso: '2026-07-10T15:00:00Z' },
  { label: 'a_aug', now_iso: '2026-08-01T00:00:00Z', resets_at_iso: '2026-08-10T15:00:00Z' },
  { label: 'a_sep', now_iso: '2026-09-01T00:00:00Z', resets_at_iso: '2026-09-10T15:00:00Z' },
  { label: 'a_oct', now_iso: '2026-10-01T00:00:00Z', resets_at_iso: '2026-10-10T15:00:00Z' },
  { label: 'a_nov', now_iso: '2026-11-01T00:00:00Z', resets_at_iso: '2026-11-10T15:00:00Z' },
  { label: 'a_dec', now_iso: '2026-12-01T00:00:00Z', resets_at_iso: '2026-12-10T15:00:00Z' },
];

const TZ_LABEL = process.env.TZ || 'UTC';

for (const c of CASES) {
  const now_unix = Math.floor(new Date(c.now_iso).getTime() / 1000);
  const resets_unix = Math.floor(new Date(c.resets_at_iso).getTime() / 1000);

  // Re-install the Date override for each case so `now` is this case's now_iso.
  const realDate = globalThis.__origDate || Date;
  if (!globalThis.__origDate) globalThis.__origDate = realDate;
  const origDate = globalThis.__origDate;
  globalThis.Date = class extends origDate {
    constructor(...args) {
      if (args.length === 0) {
        super(now_unix * 1000);
      } else {
        super(...args);
      }
    }
    static now() { return now_unix * 1000; }
  };
  globalThis.Date.UTC = origDate.UTC;
  globalThis.Date.parse = origDate.parse;

  const output = formatResetTime(resets_unix, true, true);
  console.log(JSON.stringify({
    label: c.label,
    tz: TZ_LABEL,
    now_unix,
    resets_at_unix: resets_unix,
    output,
  }));
}
