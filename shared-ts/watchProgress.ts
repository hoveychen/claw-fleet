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

/** The subset of `WatchSummary` a task row's watch token needs. */
export interface WatchTokenInput extends WatchProgressInput {
  pollCount: number;
  structuralFailStreak?: number | null;
  targets?: ReadonlyArray<{ phase?: string | null }> | null;
}

/**
 * A watch squeezed into one token for a task-list row, on both apps: a ring
 * when there is a fraction (else the caller draws its watch icon) and a short
 * tail — a percentage, the reported line, elapsed against the expected wait,
 * or elapsed and the poll count. A session watch says how many of its targets
 * are done. `alarming` paints it amber: progress stalled, the wait overran, a
 * target failed, or the `until` command cannot run at all.
 */
export function watchRowToken(
  w: WatchTokenInput,
  now: number,
): { ring: { fraction: number; alarming: boolean } | null; text: string; alarming: boolean } {
  if (w.targets && w.targets.length > 0) {
    const done = w.targets.filter((x) => x.phase === "done").length;
    return {
      ring: null,
      text: `${done}/${w.targets.length}`,
      alarming: w.targets.some((x) => x.phase === "failed"),
    };
  }
  const pv = watchProgressView(w, now);
  const elapsed = compactDuration(now - w.created);
  const text =
    pv.kind === "reported"
      ? pv.fraction !== null
        ? `${Math.round(pv.fraction * 100)}%`
        : pv.text
      : pv.kind === "time"
        ? `${elapsed}/${compactDuration(pv.expectedMs)}`
        : `${elapsed}·${w.pollCount}×`;
  const alarming =
    (w.structuralFailStreak ?? 0) > 0 ||
    (pv.kind === "reported" && pv.stalledMs !== null) ||
    (pv.kind === "time" && pv.overdueMs !== null);
  return { ring: watchRing(pv), text, alarming };
}
