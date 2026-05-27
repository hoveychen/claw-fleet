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
}

const WINDOW_MS = 24 * 60 * 60 * 1000;
const REFRESH_MS = 5 * 60 * 1000;
const THRESHOLD_KEY = "usage-heavy-threshold-usd";
const DEFAULT_THRESHOLD = 5;

const FIVE_HOUR_COLOR = "#f97316"; // orange — matches the 5h pool line
const SEVEN_DAY_COLOR = "#3b82f6"; // blue — matches the 7d pool line

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

  // Heavy markers: Claude sessions whose cumulative cost crossed the threshold,
  // placed at the moment the session started, within the visible 24h window.
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
      .sort((a, b) => a.ts - b.ts);
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
            {markers.map((m, i) => (
              <ReferenceLine
                key={`${m.ts}-${i}`}
                x={m.ts}
                stroke="#ef4444"
                strokeDasharray="4 3"
                strokeOpacity={0.7}
                label={{
                  value: "▲",
                  position: "insideTop",
                  fill: "#ef4444",
                  fontSize: 10,
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
          <span className={styles.legend_item} title={markers.map((m) => `${formatClock(m.ts)} · ${m.label} · $${m.cost.toFixed(2)}`).join("\n")}>
            <i style={{ background: "#ef4444" }} />
            {t("account.heavy_marker", { n: markers.length })}
          </span>
        )}
      </div>
    </div>
  );
}
