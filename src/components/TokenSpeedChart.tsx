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
import styles from "./TokenSpeedChart.module.css";

function formatTime(ms: number): string {
  const d = new Date(ms);
  return `${d.getHours().toString().padStart(2, "0")}:${d.getMinutes().toString().padStart(2, "0")}:${d.getSeconds().toString().padStart(2, "0")}`;
}

export function TokenSpeedChart() {
  const { t } = useTranslation();
  const speedHistory = useSessionsStore((s) => s.speedHistory);

  const currentSpeed =
    speedHistory.length > 0 ? speedHistory[speedHistory.length - 1].speed : 0;

  return (
    <div className={styles.panel}>
      <div className={styles.header}>
        <span className={styles.title}>{t("chart.title")}</span>
        <span className={styles.current}>
          {currentSpeed.toFixed(1)}{" "}
          <span className={styles.unit}>{t("chart.unit")}</span>
        </span>
      </div>

      {speedHistory.length < 2 ? (
        <div className={styles.no_data}>{t("chart.no_data")}</div>
      ) : (
        <ResponsiveContainer width="100%" height={80}>
          <AreaChart
            data={speedHistory}
            margin={{ top: 4, right: 4, left: -20, bottom: 0 }}
          >
            <defs>
              <linearGradient id="speedGrad" x1="0" y1="0" x2="0" y2="1">
                <stop offset="5%" stopColor="var(--color-accent)" stopOpacity={0.4} />
                <stop offset="95%" stopColor="var(--color-accent)" stopOpacity={0} />
              </linearGradient>
            </defs>
            <XAxis
              dataKey="time"
              tickFormatter={formatTime}
              tick={{ fontSize: 9, fill: "var(--color-text-dim)" }}
              tickLine={false}
              axisLine={false}
              interval="preserveStartEnd"
              minTickGap={40}
            />
            <YAxis
              tick={{ fontSize: 9, fill: "var(--color-text-dim)" }}
              tickLine={false}
              axisLine={false}
              width={30}
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
              formatter={(v) => [`${(v as number).toFixed(1)} tok/s`, ""]}
            />
            <Area
              type="monotone"
              dataKey="speed"
              stroke="var(--color-accent)"
              strokeWidth={1.5}
              fill="url(#speedGrad)"
              dot={false}
              isAnimationActive={false}
            />
          </AreaChart>
        </ResponsiveContainer>
      )}
    </div>
  );
}
