import { useTranslation } from "react-i18next";
import {
  Area,
  AreaChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { useSessionsStore } from "../store";
import { useCollapsed } from "./useCollapsed";
import styles from "./TokenSpeedChart.module.css";

function formatTime(ms: number): string {
  const d = new Date(ms);
  return `${d.getHours().toString().padStart(2, "0")}:${d.getMinutes().toString().padStart(2, "0")}:${d.getSeconds().toString().padStart(2, "0")}`;
}

const WINDOW_MS = 5 * 60 * 1000;

/** USD cost rate chart. Y axis = $/min; the area under the curve over the
 * 5-minute window is approximately the total USD spent in that window. */
export function CostSpeedChart({ compact = false }: { compact?: boolean } = {}) {
  const { t } = useTranslation();
  const costHistory = useSessionsStore((s) => s.costHistory);
  const [collapsed, setCollapsed] = useCollapsed("cost-speed-chart", !compact);

  const currentRate =
    costHistory.length > 0 ? costHistory[costHistory.length - 1].costPerMin : 0;
  const domainEnd =
    costHistory.length > 0
      ? costHistory[costHistory.length - 1].time
      : Date.now();
  const domainStart = domainEnd - WINDOW_MS;

  // Sum of the trapezoidal area under the curve, in USD. Each sample's
  // costPerMin is a rate; area = mean(rate) * duration_min.
  const windowTotalUsd = (() => {
    if (costHistory.length < 2) return 0;
    let sum = 0;
    for (let i = 1; i < costHistory.length; i++) {
      const dtMin = (costHistory[i].time - costHistory[i - 1].time) / 60000;
      const avg = (costHistory[i].costPerMin + costHistory[i - 1].costPerMin) / 2;
      sum += avg * dtMin;
    }
    return sum;
  })();

  return (
    <div className={styles.panel}>
      <button
        type="button"
        className={styles.header}
        onClick={() => setCollapsed(!collapsed)}
        aria-expanded={!collapsed}
      >
        <span className={styles.title_row}>
          <span className={`${styles.chevron} ${collapsed ? styles.chevron_collapsed : ""}`}>▾</span>
          <span className={styles.title}>{t("cost_chart.title")}</span>
        </span>
        <span className={styles.current}>
          ${currentRate.toFixed(2)}{" "}
          <span className={styles.unit}>{t("cost_chart.unit")}</span>
        </span>
      </button>

      {!collapsed && (
        costHistory.length < 2 ? (
          <div className={styles.no_data}>{t("chart.no_data")}</div>
        ) : (
          <>
            <ResponsiveContainer width="100%" height={compact ? 56 : 80}>
              <AreaChart
                data={costHistory}
                margin={compact ? { top: 2, right: 2, left: 2, bottom: 0 } : { top: 4, right: 4, left: 0, bottom: 0 }}
              >
                <defs>
                  <linearGradient id="costGrad" x1="0" y1="0" x2="0" y2="1">
                    <stop offset="5%" stopColor="var(--color-warning, #f59e0b)" stopOpacity={0.4} />
                    <stop offset="95%" stopColor="var(--color-warning, #f59e0b)" stopOpacity={0} />
                  </linearGradient>
                </defs>
                {compact ? (
                  <XAxis dataKey="time" type="number" scale="time" domain={[domainStart, domainEnd]} hide />
                ) : (
                  <XAxis
                    dataKey="time"
                    type="number"
                    scale="time"
                    domain={[domainStart, domainEnd]}
                    tickFormatter={formatTime}
                    tick={{ fontSize: 9, fill: "var(--color-text-dim)" }}
                    tickLine={false}
                    axisLine={false}
                    interval="preserveStartEnd"
                    minTickGap={40}
                  />
                )}
                <YAxis
                  tick={{ fontSize: compact ? 8 : 9, fill: "var(--color-text-dim)" }}
                  tickLine={false}
                  axisLine={false}
                  width={compact ? 28 : 30}
                  tickFormatter={(v) => `$${(v as number).toFixed(2)}`}
                />
                <Tooltip
                  contentStyle={{
                    background: "var(--color-bg-secondary)",
                    border: "1px solid var(--color-border)",
                    borderRadius: 6,
                    fontSize: 11,
                    color: "var(--color-text)",
                  }}
                  labelFormatter={(v) => formatTime(v as number)}
                  formatter={(v) => [`$${(v as number).toFixed(2)}/min`, ""]}
                />
                <Area
                  type="monotone"
                  dataKey="costPerMin"
                  stroke="var(--color-warning, #f59e0b)"
                  strokeWidth={1.5}
                  fill="url(#costGrad)"
                  dot={false}
                  isAnimationActive={false}
                />
              </AreaChart>
            </ResponsiveContainer>
            <div className={styles.window_total}>
              {t("cost_chart.window_total", { amount: windowTotalUsd.toFixed(2) })}
            </div>
          </>
        )
      )}
    </div>
  );
}
