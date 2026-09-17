// Codex 24h utilization curve: session/weekly two lines. Data via relay `codex_usage_history`
// (see ../account.ts), a snapshot persisted each time desktop pulls Codex usage, read-only from disk.
// Shares the same hand-written SVG geometry (../usageChart) and styles with Claude's UsageChart;
// difference only in data source and labels — Codex percentages are 0–100 integers, here /100
// normalized and fed to linePath. Corresponds to desktop CodexUsageHistoryChart.tsx (which uses recharts).

import { useEffect, useMemo, useState } from "react";
import { fetchCodexUsageHistory } from "../account";
import { dateLocale, t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { CodexUsageHistoryPoint } from "../types";
import { linePath, timeTicks, type ChartBox } from "../usageChart";
import styles from "./UsageChart.module.css";

const WINDOW_MS = 24 * 3_600_000;
const TICK_STEP_MS = 6 * 3_600_000;

/** viewBox user units; scaled proportionally to card width. */
const W = 320;
const H = 120;

// Same semantics as Claude chart: shorter window (session) orange, longer window (weekly) blue.
const PRIMARY_COLOR = "#f97316";
const SECONDARY_COLOR = "#3b82f6";

function clock(ts: number): string {
  return new Date(ts).toLocaleTimeString(dateLocale(), {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

/** Derive a compact label for a line from window duration (minutes): 7d / 5h / 30m. Window duration may be
 *  missing at some sample points due to plan changes etc., so take the most recent point with a duration. */
function windowLabel(mins: number | null): string {
  if (mins == null || !Number.isFinite(mins)) return t("用量");
  if (mins >= 1440) return `${Math.round(mins / 1440)}d`;
  if (mins >= 60) return `${Math.round(mins / 60)}h`;
  return `${Math.round(mins)}m`;
}

/** Duration of the most recent sample point with that window duration; null if none. */
function latestWindowMins(
  points: CodexUsageHistoryPoint[],
  pick: (p: CodexUsageHistoryPoint) => number | null,
): number | null {
  for (let i = points.length - 1; i >= 0; i--) {
    const v = pick(points[i]);
    if (v != null) return v;
  }
  return null;
}

export function CodexUsageChart({ client }: { client: FleetTransport | null }) {
  const [points, setPoints] = useState<CodexUsageHistoryPoint[] | null>(null);
  // Timestamp at fetch time: pin the window right edge, avoid window drifting on each re-render.
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!client) return;
    let cancelled = false;
    const to = Date.now();
    fetchCodexUsageHistory(client, to - WINDOW_MS, to)
      .then((rows) => {
        if (cancelled) return;
        setPoints(rows);
        setNow(to);
      })
      .catch(() => {
        if (!cancelled) setPoints([]);
      });
    return () => {
      cancelled = true;
    };
  }, [client]);

  const box: ChartBox = useMemo(
    () => ({ width: W, height: H, fromMs: now - WINDOW_MS, toMs: now }),
    [now],
  );

  // Codex is 0–100 integer, /100 normalized to the 0–1 that linePath expects.
  const primary = useMemo(
    () =>
      linePath(points ?? [], (p) => (p.primaryPct == null ? null : p.primaryPct / 100), box),
    [points, box],
  );
  const secondary = useMemo(
    () =>
      linePath(points ?? [], (p) => (p.secondaryPct == null ? null : p.secondaryPct / 100), box),
    [points, box],
  );
  const ticks = useMemo(() => timeTicks(box, TICK_STEP_MS), [box]);

  const primaryLabel = windowLabel(latestWindowMins(points ?? [], (p) => p.primaryWindowMins));
  const secondaryLabel = windowLabel(
    latestWindowMins(points ?? [], (p) => p.secondaryWindowMins),
  );
  const hasSecondary = (points ?? []).some((p) => p.secondaryPct != null);

  if (points === null) {
    return <div className={styles.hint}>{t("加载中…")}</div>;
  }
  if (!primary && !secondary) {
    return <div className={styles.hint}>{t("还没有攒够采样点，桌面端跑一阵子再看。")}</div>;
  }

  return (
    <div className={styles.wrap}>
      <svg
        className={styles.svg}
        viewBox={`0 0 ${W} ${H}`}
        role="img"
        aria-label={t("codex 近 24 小时占用率")}
      >
        {[0, 0.5, 1].map((f) => (
          <line
            key={f}
            className={styles.grid}
            x1={0}
            x2={W}
            y1={H - f * H}
            y2={H - f * H}
          />
        ))}
        {secondary && (
          <path className={styles.line} d={secondary} stroke={SECONDARY_COLOR} />
        )}
        {primary && <path className={styles.line} d={primary} stroke={PRIMARY_COLOR} />}
      </svg>

      <div className={styles.axis}>
        {ticks.map(([x, ts]) => (
          <span key={ts} className={styles.tick} style={{ left: `${(x / W) * 100}%` }}>
            {clock(ts)}
          </span>
        ))}
      </div>

      <div className={styles.legend}>
        <span className={styles.legendItem}>
          <i style={{ background: PRIMARY_COLOR }} />
          {primaryLabel}
        </span>
        {hasSecondary && (
          <span className={styles.legendItem}>
            <i style={{ background: SECONDARY_COLOR }} />
            {secondaryLabel}
          </span>
        )}
        <span className={styles.scale}>{t("纵轴 0–100%")}</span>
      </div>
    </div>
  );
}
