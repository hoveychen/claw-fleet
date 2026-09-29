import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  CartesianGrid,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { codexRateLimitLabel, codexWindowLabel, type TFunc } from "../codexUsage";
import styles from "./UsageHistoryChart.module.css";

// Mirrors claw_fleet_core::codex_usage_history::CodexUsageHistoryPoint
// (camelCase on the wire, like the rest of the codex-facing types). The
// percentages are the 0–100 ints Codex's app-server hands back — no ×100.
export interface CodexUsageHistoryPoint {
  ts: number;
  primaryPct: number | null;
  secondaryPct: number | null;
  primaryWindowMins: number | null;
  secondaryWindowMins: number | null;
  bars?: CodexUsageHistoryBar[];
}

export interface CodexUsageHistoryBar {
  key: string;
  limitId?: string | null;
  limitName?: string | null;
  normalModelSlug?: string | null;
  windowKind: "primary" | "secondary";
  pct: number;
  windowMins?: number | null;
}

const WINDOW_MS = 24 * 60 * 60 * 1000;
const REFRESH_MS = 5 * 60 * 1000;

// Same colour semantics as the Claude chart: the shorter (session) window is
// orange, the longer (weekly) window is blue.
const SERIES_COLORS = ["#f97316", "#3b82f6", "#22c55e", "#a855f7", "#eab308", "#06b6d4"];

function formatClock(ts: number): string {
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(
    d.getMinutes(),
  ).padStart(2, "0")}`;
}

export function barsForPoint(point: CodexUsageHistoryPoint): CodexUsageHistoryBar[] {
  if (point.bars?.length) return point.bars;
  const bars: CodexUsageHistoryBar[] = [];
  if (point.primaryPct != null) {
    bars.push({
      key: "codex:primary",
      windowKind: "primary",
      pct: point.primaryPct,
      windowMins: point.primaryWindowMins,
    });
  }
  if (point.secondaryPct != null) {
    bars.push({
      key: "codex:secondary",
      windowKind: "secondary",
      pct: point.secondaryPct,
      windowMins: point.secondaryWindowMins,
    });
  }
  return bars;
}

function CodexTooltip({
  active,
  payload,
  label,
}: {
  active?: boolean;
  payload?: Array<{
    name?: string;
    value?: number | null;
    color?: string;
    dataKey?: string | number;
  }>;
  label?: number;
}) {
  if (!active || !payload || payload.length === 0) return null;
  const ts = Number(label);
  return (
    <div className={styles.rtt}>
      <div className={styles.rtt_time}>{formatClock(ts)}</div>
      {payload.map((p) => (
        <div
          key={String(p.dataKey)}
          className={styles.rtt_row}
          style={{ color: p.color }}
        >
          {p.name}: {p.value == null ? "—" : `${p.value}%`}
        </div>
      ))}
    </div>
  );
}

export function CodexUsageHistoryChart({ height = 200 }: { height?: number } = {}) {
  const { t } = useTranslation();
  const [points, setPoints] = useState<CodexUsageHistoryPoint[]>([]);
  const [now, setNow] = useState<number>(() => Date.now());

  useEffect(() => {
    let cancelled = false;
    const fetchHistory = () => {
      const toMs = Date.now();
      const fromMs = toMs - WINDOW_MS;
      invoke<CodexUsageHistoryPoint[]>("get_codex_usage_history", { fromMs, toMs })
        .then((rows) => {
          if (!cancelled) {
            setPoints(rows);
            setNow(toMs);
          }
        })
        .catch(() => {
          /* codex not installed / not logged in — keep the last good series */
        });
    };
    fetchHistory();
    const timer = setInterval(fetchHistory, REFRESH_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, []);

  const fromMs = now - WINDOW_MS;

  const { data, series } = useMemo(() => {
    const latest = new Map<string, CodexUsageHistoryBar>();
    for (const point of points) {
      for (const bar of barsForPoint(point)) latest.set(bar.key, bar);
    }
    const series = Array.from(latest.entries()).map(([key, bar], index) => ({
      key,
      dataKey: `series_${index}`,
      color: SERIES_COLORS[index % SERIES_COLORS.length],
      label: bar.limitName || bar.normalModelSlug || bar.limitId
        ? codexRateLimitLabel(
            { limitId: bar.limitId, limitName: bar.limitName, normalModelSlug: bar.normalModelSlug },
            { usedPercent: bar.pct, windowDurationMins: bar.windowMins },
            t as TFunc,
          )
        : codexWindowLabel(bar.windowMins, t as TFunc),
    }));
    const dataKeyBySeries = new Map(series.map((item) => [item.key, item.dataKey]));
    const data = points.map((point) => {
      const row: Record<string, number> = { ts: point.ts };
      for (const bar of barsForPoint(point)) {
        const dataKey = dataKeyBySeries.get(bar.key);
        if (dataKey) row[dataKey] = bar.pct;
      }
      return row;
    });
    return { data, series };
  }, [points, t]);

  const hasData = series.length > 0;

  return (
    <div className={styles.wrap}>
      <div className={styles.header}>
        <div className={styles.titles}>
          <span className={styles.title}>{t("account.occupancy_title")}</span>
          <span className={styles.subtitle}>
            {t("account.occupancy_subtitle_codex")}
          </span>
        </div>
      </div>

      {!hasData ? (
        <p className={styles.empty}>{t("account.no_history")}</p>
      ) : (
        <ResponsiveContainer width="100%" height={height}>
          <LineChart
            data={data}
            margin={{ top: 16, right: 12, bottom: 4, left: 4 }}
          >
            <CartesianGrid strokeDasharray="3 3" opacity={0.15} />
            <XAxis
              dataKey="ts"
              type="number"
              scale="time"
              domain={[fromMs, now]}
              tickFormatter={formatClock}
              tick={{ fontSize: 11 }}
              minTickGap={48}
            />
            <YAxis
              domain={[0, 100]}
              ticks={[0, 25, 50, 75, 100]}
              tickFormatter={(v) => `${v}%`}
              tick={{ fontSize: 11 }}
              width={48}
            />
            <Tooltip
              content={(props) => (
                <CodexTooltip
                  active={props.active}
                  payload={
                    props.payload as unknown as Array<{
                      name?: string;
                      value?: number | null;
                      color?: string;
                      dataKey?: string | number;
                    }>
                  }
                  label={props.label as number}
                />
              )}
            />
            {series.map((item) => (
              <Line
                key={item.key}
                type="monotone"
                dataKey={item.dataKey}
                name={item.label}
                stroke={item.color}
                dot={false}
                strokeWidth={2}
                connectNulls
                isAnimationActive={false}
              />
            ))}
          </LineChart>
        </ResponsiveContainer>
      )}

      <div className={styles.legend}>
        {series.map((item) => (
          <span key={item.key} className={styles.legend_item}>
            <i style={{ background: item.color }} />
            {item.label}
          </span>
        ))}
      </div>
    </div>
  );
}
