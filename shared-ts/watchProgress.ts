/**
 * How far along an active `fleet watch` is. Shared by the desktop watch chip and
 * the mobile session sheet / task rows so both draw the same bar from the same
 * fields.
 *
 * A watch's `until` is a 0/1 gate, so without this all a card could say about a
 * half-hour wait was "polled 64 times". Two sources can do better:
 *   - `reported`: the watch has a `--progress` command and it printed a line.
 *     The core parses `N/M` / `N%` into `progressFraction`; free text has none.
 *   - `time`: no reported progress, but the agent gave `--expect-by`, so elapsed
 *     time against the expected wait is a fair stand-in.
 */

/** The subset of `WatchSummary` this needs — kept structural so both apps can
 *  pass their own generated type. */
export interface WatchProgressInput {
  created: number;
  expectBy?: number | null;
  progress?: string | null;
  progressFraction?: number | null;
  progressChangedAt?: number | null;
}

export type WatchProgressView =
  | {
      kind: "reported";
      text: string;
      /** 0..1, or null for a free-text line. */
      fraction: number | null;
      /** How long the line has sat unchanged, once that crosses
       *  {@link WATCH_STALL_MS}; null while it is still moving. */
      stalledMs: number | null;
    }
  | {
      kind: "time";
      /** 0..1 of the expected wait used up (clamped). */
      fraction: number;
      elapsedMs: number;
      expectedMs: number;
      /** Past the expected time and still waiting. */
      overdueMs: number | null;
    }
  | { kind: "none" };

/** A reported progress unchanged for this long is called out as stalled. Five
 *  minutes is past any sane poll interval yet well short of "I'd have asked". */
export const WATCH_STALL_MS = 5 * 60_000;

export function watchProgressView(
  w: WatchProgressInput,
  now: number,
): WatchProgressView {
  if (w.progress) {
    const since = w.progressChangedAt ?? w.created;
    const still = Math.max(0, now - since);
    return {
      kind: "reported",
      text: w.progress,
      fraction: w.progressFraction ?? null,
      stalledMs: still >= WATCH_STALL_MS ? still : null,
    };
  }
  if (w.expectBy && w.expectBy > w.created) {
    const elapsedMs = Math.max(0, now - w.created);
    const expectedMs = w.expectBy - w.created;
    return {
      kind: "time",
      fraction: Math.min(1, elapsedMs / expectedMs),
      elapsedMs,
      expectedMs,
      overdueMs: now > w.expectBy ? now - w.expectBy : null,
    };
  }
  return { kind: "none" };
}

/** Language-neutral span for a tight chip: "40s" / "3m" / "2h" / "1d". */
export function compactDuration(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  return `${Math.floor(h / 24)}d`;
}

/** What the progress ring (drawn in place of the watch icon) shows: how full,
 *  and whether to paint it amber — reported progress that stopped moving, or a
 *  time-based wait past its expected end. `null` when there is no fraction to
 *  draw (no progress source, or a free-text line), so the icon stays. */
export function watchRing(
  pv: WatchProgressView,
): { fraction: number; alarming: boolean } | null {
  if (pv.kind === "reported" && pv.fraction !== null) {
    return { fraction: pv.fraction, alarming: pv.stalledMs !== null };
  }
  if (pv.kind === "time") {
    return { fraction: pv.fraction, alarming: pv.overdueMs !== null };
  }
  return null;
}
