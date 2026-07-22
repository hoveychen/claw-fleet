// codex 近 24h 占用率曲线：session/weekly 两条线。数据走 relay `codex_usage_history`
// （见 ../account.ts），是桌面端每次拉 codex 用量时落盘的快照，纯读盘。与 Claude 的
// UsageChart 共用同一套手写 SVG 几何（../usageChart）与样式；差别只在数据源和标签——
// codex 百分比是 0–100 整数，这里 /100 归一后喂给 linePath。
// 对应桌面端 CodexUsageHistoryChart.tsx（那张用 recharts）。

import { useEffect, useMemo, useState } from "react";
import { fetchCodexUsageHistory } from "../account";
import { dateLocale, t } from "../i18n";
import type { RelayClient } from "../relay";
import type { CodexUsageHistoryPoint } from "../types";
import { linePath, timeTicks, type ChartBox } from "../usageChart";
import styles from "./UsageChart.module.css";

const WINDOW_MS = 24 * 3_600_000;
const TICK_STEP_MS = 6 * 3_600_000;

/** viewBox 用户单位；等比缩放到卡片宽度。 */
const W = 320;
const H = 120;

// 与 Claude 图同一套语义：较短窗口（session）橙色，较长窗口（weekly）蓝色。
const PRIMARY_COLOR = "#f97316";
const SECONDARY_COLOR = "#3b82f6";

function clock(ts: number): string {
  return new Date(ts).toLocaleTimeString(dateLocale(), {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

/** 从窗口时长（分钟）派生一条线的紧凑标签：7d / 5h / 30m。窗口时长可能因换套餐等
 *  在某些采样点缺失，所以取最近一个带时长的点。 */
function windowLabel(mins: number | null): string {
  if (mins == null || !Number.isFinite(mins)) return t("用量");
  if (mins >= 1440) return `${Math.round(mins / 1440)}d`;
  if (mins >= 60) return `${Math.round(mins / 60)}h`;
  return `${Math.round(mins)}m`;
}

/** 最近一个带该窗口时长的采样点的时长；都没有则 null。 */
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

export function CodexUsageChart({ client }: { client: RelayClient | null }) {
  const [points, setPoints] = useState<CodexUsageHistoryPoint[] | null>(null);
  // 拉取那一刻的时间戳：窗口右端固定住，避免每次重渲染窗口都在漂。
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

  // codex 是 0–100 整数，/100 归一到 linePath 期望的 0–1。
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
