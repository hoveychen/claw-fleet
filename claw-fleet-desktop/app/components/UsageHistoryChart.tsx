import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  CartesianGrid,
  Line,
  LineChart,
  ReferenceLine,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { useSessionsStore } from "../store";
import styles from "./UsageHistoryChart.module.css";

// Mirrors claw_fleet_core::account::UsageHistoryPoint (snake_case on the wire,
// like the rest of the account/* types). Utilization values are 0–1 fractions.
interface UsageHistoryPoint {
  ts: number;
  five_hour: number | null;
  seven_day: number | null;
  seven_day_sonnet: number | null;
}

interface ChartRow {
  ts: number;
  fiveHour: number | null;
  sevenDay: number | null;
}

interface HeavyMarker {
  ts: number;
  label: string;
  cost: number;
  rank: number; // 1 = highest cost (rank caps at MAX_MARKERS)
}

const WINDOW_MS = 24 * 60 * 60 * 1000;
const REFRESH_MS = 5 * 60 * 1000;
const THRESHOLD_KEY = "usage-heavy-threshold-usd";
const DEFAULT_THRESHOLD = 5;
const MAX_MARKERS = 5;

const FIVE_HOUR_COLOR = "#f97316"; // orange — matches the 5h pool line
const SEVEN_DAY_COLOR = "#3b82f6"; // blue — matches the 7d pool line

// Rank-indexed palette (rank 1 = darkest = most expensive session).
// Same red ramp the rest of the app uses, ordered dark → light.
const HEAVY_COLORS = ["#b91c1c", "#dc2626", "#ef4444", "#f87171", "#fca5a5"];

function loadThreshold(): number {
  const raw = localStorage.getItem(THRESHOLD_KEY);
  const n = raw == null ? NaN : Number(raw);
  return Number.isFinite(n) && n > 0 ? n : DEFAULT_THRESHOLD;
}

function formatClock(ts: number): string {
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(
    d.getMinutes(),
  ).padStart(2, "0")}`;
}

function pct(frac: number | null): number | null {
  return frac == null ? null : Math.round(frac * 1000) / 10;
}

export function UsageHistoryChart({ height = 200 }: { height?: number } = {}) {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const [points, setPoints] = useState<UsageHistoryPoint[]>([]);
  const [threshold, setThreshold] = useState<number>(loadThreshold);
  // `now` advances on each refresh so the 24h window and markers stay current.
  const [now, setNow] = useState<number>(() => Date.now());

  useEffect(() => {
    let cancelled = false;
    const fetchHistory = () => {
      const toMs = Date.now();
      const fromMs = toMs - WINDOW_MS;
      invoke<UsageHistoryPoint[]>("get_usage_history", { fromMs, toMs })
        .then((rows) => {
          if (!cancelled) {
            setPoints(rows);
            setNow(toMs);
          }
        })
        .catch(() => {
          /* offline / not logged in — keep the last good series */
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
        fiveHour: pct(p.five_hour),
        sevenDay: pct(p.seven_day),
      })),
    [points],
  );

  // Heavy markers: the top-N (by cumulative cost) Claude sessions whose cost
  // crossed the threshold, placed at the moment each session started. Ranking
  // is by cost so the most expensive run always lands the darkest dot; ties
  // break on earlier start time.
  const markers: HeavyMarker[] = useMemo(() => {
    return sessions
      .filter(
        (s) =>
          s.agentSource === "claude-code" &&
          s.totalCostUsd >= threshold &&
          s.createdAtMs >= fromMs &&
          s.createdAtMs <= now,
      )
      .map((s) => ({
        ts: s.createdAtMs,
        label: s.aiTitle || s.slug || s.workspaceName || "session",
        cost: s.totalCostUsd,
      }))
      .sort((a, b) => b.cost - a.cost || a.ts - b.ts)
      .slice(0, MAX_MARKERS)
      .map((m, i) => ({ ...m, rank: i + 1 }));
  }, [sessions, threshold, fromMs, now]);

  const onThresholdChange = (raw: string) => {
    const n = Number(raw);
    if (Number.isFinite(n) && n > 0) {
      setThreshold(n);
      localStorage.setItem(THRESHOLD_KEY, String(n));
    }
  };

  const hasData = data.some((r) => r.fiveHour != null || r.sevenDay != null);

  return (
    <div className={styles.wrap}>
      <div className={styles.header}>
        <div className={styles.titles}>
          <span className={styles.title}>{t("account.occupancy_title")}</span>
          <span className={styles.subtitle}>
            {t("account.occupancy_subtitle")}
          </span>
        </div>
        <label className={styles.threshold} title={t("account.heavy_hint")}>
          {t("account.heavy_threshold")}
          <span className={styles.dollar}>$</span>
          <input
            type="number"
            min={0.1}
            step={0.5}
            value={threshold}
            onChange={(e) => onThresholdChange(e.target.value)}
            className={styles.threshold_input}
          />
        </label>
      </div>

      {!hasData ? (
        <p className={styles.empty}>{t("account.no_history")}</p>
      ) : (
        <ResponsiveContainer width="100%" height={height}>
          <LineChart
            data={data}
            margin={{ top: 8, right: 12, bottom: 4, left: -16 }}
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
              tickFormatter={(v) => `${v}%`}
              tick={{ fontSize: 11 }}
              width={44}
            />
            <Tooltip
              labelFormatter={(ts) => formatClock(Number(ts))}
              formatter={(value, name) => [
                value == null ? "—" : `${value}%`,
                String(name),
              ]}
              contentStyle={{ fontSize: 12 }}
            />
            <Line
              type="monotone"
              dataKey="fiveHour"
              name={t("account.five_hour")}
              stroke={FIVE_HOUR_COLOR}
              dot={false}
              strokeWidth={2}
              connectNulls
              isAnimationActive={false}
            />
            <Line
              type="monotone"
              dataKey="sevenDay"
              name={t("account.seven_day")}
              stroke={SEVEN_DAY_COLOR}
              dot={false}
              strokeWidth={2}
              connectNulls
              isAnimationActive={false}
            />
            {markers.map((m) => (
              <ReferenceLine
                key={`${m.ts}-${m.rank}`}
                x={m.ts}
                stroke="transparent"
                ifOverflow="extendDomain"
                label={(labelProps: { viewBox?: { x?: number; y?: number } }) => {
                  const vb = labelProps.viewBox ?? {};
                  const cx = vb.x ?? 0;
                  const cy = (vb.y ?? 0) + 9;
                  const color = HEAVY_COLORS[m.rank - 1] ?? HEAVY_COLORS[HEAVY_COLORS.length - 1];
                  const tip = `#${m.rank} · ${m.label} · $${m.cost.toFixed(2)} · ${formatClock(m.ts)}`;
                  return (
                    <g style={{ cursor: "help" }}>
                      <title>{tip}</title>
                      <circle
                        cx={cx}
                        cy={cy}
                        r={8}
                        fill={color}
                        stroke="var(--color-bg, #0b0b0b)"
                        strokeWidth={1.5}
                      />
                      <text
                        x={cx}
                        y={cy + 0.5}
                        textAnchor="middle"
                        dominantBaseline="central"
                        fontSize={9.5}
                        fontWeight={700}
                        fill="#fff"
                        style={{ pointerEvents: "none" }}
                      >
                        {m.rank}
                      </text>
                    </g>
                  );
                }}
              />
            ))}
          </LineChart>
        </ResponsiveContainer>
      )}

      <div className={styles.legend}>
        <span className={styles.legend_item}>
          <i style={{ background: FIVE_HOUR_COLOR }} />
          {t("account.five_hour")}
        </span>
        <span className={styles.legend_item}>
          <i style={{ background: SEVEN_DAY_COLOR }} />
          {t("account.seven_day")}
        </span>
        {markers.length > 0 && (
          <span
            className={styles.legend_item}
            title={markers
              .map((m) => `#${m.rank} · ${formatClock(m.ts)} · ${m.label} · $${m.cost.toFixed(2)}`)
              .join("\n")}
          >
            <span className={styles.legend_dots}>
              {markers.map((m) => (
                <i
                  key={m.rank}
                  className={styles.legend_dot}
                  style={{
                    background: HEAVY_COLORS[m.rank - 1] ?? HEAVY_COLORS[HEAVY_COLORS.length - 1],
                  }}
                />
              ))}
            </span>
            {t("account.heavy_marker", { n: markers.length })}
          </span>
        )}
      </div>
    </div>
  );
}
