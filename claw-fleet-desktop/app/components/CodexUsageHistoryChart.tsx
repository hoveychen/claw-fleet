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
import { codexWindowLabel, type TFunc } from "./UsagePanel";
import styles from "./UsageHistoryChart.module.css";

// Mirrors claw_fleet_core::codex_usage_history::CodexUsageHistoryPoint
// (camelCase on the wire, like the rest of the codex-facing types). The
// percentages are the 0–100 ints Codex's app-server hands back — no ×100.
interface CodexUsageHistoryPoint {
  ts: number;
  primaryPct: number | null;
  secondaryPct: number | null;
  primaryWindowMins: number | null;
  secondaryWindowMins: number | null;
}

interface ChartRow {
  ts: number;
  primary: number | null;
  secondary: number | null;
}

const WINDOW_MS = 24 * 60 * 60 * 1000;
const REFRESH_MS = 5 * 60 * 1000;

// Same colour semantics as the Claude chart: the shorter (session) window is
// orange, the longer (weekly) window is blue.
const PRIMARY_COLOR = "#f97316"; // orange — session/primary window
const SECONDARY_COLOR = "#3b82f6"; // blue — weekly/secondary window

function formatClock(ts: number): string {
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(
    d.getMinutes(),
  ).padStart(2, "0")}`;
}

// The window length for a series can be null on an old sample (e.g. a plan
// change), so derive each line's label from the most recent point that carried
// a duration — then reuse the exact same label logic the live bars use.
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

  const data: ChartRow[] = useMemo(
    () =>
      points.map((p) => ({
        ts: p.ts,
        primary: p.primaryPct,
        secondary: p.secondaryPct,
      })),
    [points],
  );

  const primaryLabel = codexWindowLabel(
    latestWindowMins(points, (p) => p.primaryWindowMins),
    t as TFunc,
  );
  const secondaryLabel = codexWindowLabel(
    latestWindowMins(points, (p) => p.secondaryWindowMins),
    t as TFunc,
  );

  const hasSecondary = data.some((r) => r.secondary != null);
  const hasData = data.some((r) => r.primary != null || r.secondary != null);

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
            <Line
              type="monotone"
              dataKey="primary"
              name={primaryLabel}
              stroke={PRIMARY_COLOR}
              dot={false}
              strokeWidth={2}
              connectNulls
              isAnimationActive={false}
            />
            <Line
              type="monotone"
              dataKey="secondary"
              name={secondaryLabel}
              stroke={SECONDARY_COLOR}
              dot={false}
              strokeWidth={2}
              connectNulls
              isAnimationActive={false}
            />
          </LineChart>
        </ResponsiveContainer>
      )}

      <div className={styles.legend}>
        <span className={styles.legend_item}>
          <i style={{ background: PRIMARY_COLOR }} />
          {primaryLabel}
        </span>
        {hasSecondary && (
          <span className={styles.legend_item}>
            <i style={{ background: SECONDARY_COLOR }} />
            {secondaryLabel}
          </span>
        )}
      </div>
    </div>
  );
}
