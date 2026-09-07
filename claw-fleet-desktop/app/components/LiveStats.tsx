import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Line, LineChart, ResponsiveContainer, Tooltip } from "recharts";
import { pickHoverPoint } from "../sparkHover";
import type { CostSample, SpeedSample } from "../store";
import { useSessionsStore } from "../store";
import { fmtRailCount, fmtRailMoney } from "../railNumbers";
import { RailStatTile } from "./RailStatTile";
import styles from "./LiveStats.module.css";

function formatClock(ms: number): string {
  const d = new Date(ms);
  return `${d.getHours().toString().padStart(2, "0")}:${d.getMinutes().toString().padStart(2, "0")}:${d.getSeconds().toString().padStart(2, "0")}`;
}

function Spark<T extends { time: number }>({
  data,
  dataKey,
  onHover,
}: {
  data: readonly T[];
  dataKey: "speed" | "costPerMin";
  onHover: (point: T | null) => void;
}) {
  if (data.length < 2) return <div className={styles.spark_placeholder} />;
  return (
    <div className={styles.spark}>
      <ResponsiveContainer width="100%" height={24}>
        <LineChart
          data={data as unknown as object[]}
          margin={{ top: 2, right: 0, left: 0, bottom: 0 }}
          onMouseMove={(s) => onHover(pickHoverPoint(data, s.activeTooltipIndex))}
          onMouseLeave={() => onHover(null)}
        >
          {/* No visible tooltip — the hovered value is written back into the KPI
              above instead, since a floating panel would cover the usage card
              below this 24px-tall sparkline. Mounting Tooltip is still what
              makes recharts track an active index (and draw the activeDot). */}
          <Tooltip content={() => null} cursor={false} />
          <Line
            type="monotone"
            dataKey={dataKey}
            stroke="var(--color-text-dim)"
            strokeOpacity={0.5}
            strokeWidth={1}
            dot={false}
            isAnimationActive={false}
          />
        </LineChart>
      </ResponsiveContainer>
    </div>
  );
}

export function LiveStats({
  collapsed = false,
}: { collapsed?: boolean } = {}) {
  const { t } = useTranslation();
  const speedHistory = useSessionsStore((s) => s.speedHistory);
  const costHistory = useSessionsStore((s) => s.costHistory);
  const [hoverSpeed, setHoverSpeed] = useState<SpeedSample | null>(null);
  const [hoverCost, setHoverCost] = useState<CostSample | null>(null);

  const currentSpeed =
    speedHistory.length > 0 ? speedHistory[speedHistory.length - 1].speed : 0;
  const currentCost =
    costHistory.length > 0
      ? costHistory[costHistory.length - 1].costPerMin
      : 0;

  if (collapsed) {
    return (
      <div className={styles.tiles} data-wizard="token-speed">
        <RailStatTile
          value={fmtRailCount(currentSpeed)}
          label={t("chart.unit")}
          title={`${t("chart.title")}: ${currentSpeed.toFixed(1)} ${t("chart.unit")}`}
        />
        <RailStatTile
          value={fmtRailMoney(currentCost)}
          label={t("cost_chart.unit")}
          title={`${t("cost_chart.title")}: $${currentCost.toFixed(2)} ${t("cost_chart.unit")}`}
        />
      </div>
    );
  }

  const speedShown = hoverSpeed ? hoverSpeed.speed : currentSpeed;
  const costShown = hoverCost ? hoverCost.costPerMin : currentCost;

  return (
    <section className={styles.section} data-wizard="token-speed">
      <h3 className={styles.section_title}>{t("live.section_title")}</h3>
      <div className={styles.kpis}>
        <div className={styles.kpi}>
          <div className={styles.kpi_label}>
            {hoverSpeed
              ? `${t("chart.title")} · ${formatClock(hoverSpeed.time)}`
              : t("chart.title")}
          </div>
          <div className={styles.kpi_value_row}>
            <span
              className={`${styles.kpi_value} ${hoverSpeed ? styles.kpi_value_past : ""}`}
            >
              {speedShown.toFixed(1)}
            </span>
            <span className={styles.kpi_unit}>{t("chart.unit")}</span>
          </div>
          <Spark data={speedHistory} dataKey="speed" onHover={setHoverSpeed} />
        </div>
        <div className={styles.kpi}>
          <div className={styles.kpi_label}>
            {hoverCost
              ? `${t("cost_chart.title")} · ${formatClock(hoverCost.time)}`
              : t("cost_chart.title")}
          </div>
          <div className={styles.kpi_value_row}>
            <span
              className={`${styles.kpi_value} ${hoverCost ? styles.kpi_value_past : ""}`}
            >
              ${costShown.toFixed(2)}
            </span>
            <span className={styles.kpi_unit}>{t("cost_chart.unit")}</span>
          </div>
          <Spark
            data={costHistory}
            dataKey="costPerMin"
            onHover={setHoverCost}
          />
        </div>
      </div>
    </section>
  );
}
