// Usage curve over the past 24h: one line each for the 5h pool and 7d Opus pool.
// Data is a snapshot persisted by the desktop background sampler (relay `usage_history`,
// pure disk read), so refresh is cheap. Hand-written SVG — the desktop chart uses recharts,
// but here we only need two polylines and a few grid lines, not worth importing a chart library.

import { useEffect, useMemo, useState } from "react";
import { fetchUsageHistory } from "../account";
import { dateLocale, t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { UsageHistoryPoint } from "../types";
import { linePath, timeTicks, type ChartBox } from "../usageChart";
import styles from "./UsageChart.module.css";

const WINDOW_MS = 24 * 3_600_000;
const TICK_STEP_MS = 6 * 3_600_000;

/** viewBox user units; scale proportionally to card width. */
const W = 320;
const H = 120;

const FIVE_HOUR_COLOR = "#f97316";
const SEVEN_DAY_COLOR = "#3b82f6";

function clock(ts: number): string {
  return new Date(ts).toLocaleTimeString(dateLocale(), {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

export function UsageChart({ client }: { client: FleetTransport | null }) {
  const [points, setPoints] = useState<UsageHistoryPoint[] | null>(null);
  // Timestamp at the moment of fetch: lock the window's right edge to prevent the
  // window from drifting on each re-render.
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!client) return;
    let cancelled = false;
    const to = Date.now();
    fetchUsageHistory(client, to - WINDOW_MS, to)
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

  const fiveHour = useMemo(
    () => linePath(points ?? [], (p) => p.fiveHour, box),
    [points, box],
  );
  const sevenDay = useMemo(
    () => linePath(points ?? [], (p) => p.sevenDay, box),
    [points, box],
  );
  const ticks = useMemo(() => timeTicks(box, TICK_STEP_MS), [box]);

  if (points === null) {
    return <div className={styles.hint}>{t("加载中…")}</div>;
  }
  if (!fiveHour && !sevenDay) {
    return <div className={styles.hint}>{t("还没有攒够采样点，桌面端跑一阵子再看。")}</div>;
  }

  return (
    <div className={styles.wrap}>
      <svg
        className={styles.svg}
        viewBox={`0 0 ${W} ${H}`}
        role="img"
        aria-label={t("近 24 小时占用率")}
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
        {sevenDay && (
          <path className={styles.line} d={sevenDay} stroke={SEVEN_DAY_COLOR} />
        )}
        {fiveHour && (
          <path className={styles.line} d={fiveHour} stroke={FIVE_HOUR_COLOR} />
        )}
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
          <i style={{ background: FIVE_HOUR_COLOR }} />
          5h
        </span>
        <span className={styles.legendItem}>
          <i style={{ background: SEVEN_DAY_COLOR }} />
          7d Opus
        </span>
        <span className={styles.scale}>{t("纵轴 0–100%")}</span>
      </div>
    </div>
  );
}
