import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Line, LineChart, ResponsiveContainer, Tooltip } from "recharts";
import { pickHoverPoint } from "../sparkHover";
import type { CostSample, SpeedSample } from "../store";
import { useSessionsStore } from "../store";
import { fmtRailCount, fmtRailMoney } from "../railNumbers";
import { RailStatTile } from "./RailStatTile";
import { SkeletonNumber } from "./loading";
import styles from "./LiveStats.module.css";
import stat from "./SidebarStat.module.css";

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
  if (data.length < 2) return <div className={stat.spark_placeholder} />;
  return (
    <div className={stat.spark}>
      <ResponsiveContainer width="100%" height={18}>
        <LineChart
          data={data as unknown as object[]}
          margin={{ top: 2, right: 0, left: 0, bottom: 0 }}
          onMouseMove={(s) => onHover(pickHoverPoint(data, s.activeTooltipIndex))}
          onMouseLeave={() => onHover(null)}
        >
          {/* No visible tooltip — the hovered value is written back into the KPI
              above instead, since a floating panel would cover the usage card
              below this 18px-tall sparkline. Mounting Tooltip is still what
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

  // Both histories gain a sample on every scan tick, so an empty history means
  // "no scan has landed yet" — not "0 tok/s". Render a placeholder until then.
  const speedReady = speedHistory.length > 0;
  const costReady = costHistory.length > 0;
  const currentSpeed = speedReady ? speedHistory[speedHistory.length - 1].speed : 0;
  const currentCost = costReady ? costHistory[costHistory.length - 1].costPerMin : 0;

  if (collapsed) {
    // Before the first scan sample there is no value yet: number skeleton, not 0.
    return (
      <div className={styles.tiles} data-wizard="token-speed">
        <RailStatTile
          value={speedReady ? fmtRailCount(currentSpeed) : ""}
          loading={!speedReady}
          label={t("chart.unit")}
          title={
            speedReady
              ? `${t("chart.title")}: ${currentSpeed.toFixed(1)} ${t("chart.unit")}`
              : `${t("chart.title")}: ${t("loading")}`
          }
        />
        <RailStatTile
          value={costReady ? fmtRailMoney(currentCost) : ""}
          loading={!costReady}
          label={t("cost_chart.unit")}
          title={
            costReady
              ? `${t("cost_chart.title")}: $${currentCost.toFixed(2)} ${t("cost_chart.unit")}`
              : `${t("cost_chart.title")}: ${t("loading")}`
          }
        />
      </div>
    );
  }

  const speedShown = hoverSpeed ? hoverSpeed.speed : currentSpeed;
  const costShown = hoverCost ? hoverCost.costPerMin : currentCost;

  // Unfolded sidebar: one card per metric, stacked — a side-by-side pair does
  // not fit in 116px.
  return (
    <div className={styles.cards} data-wizard="token-speed">
      <div className={stat.card}>
        <span className={stat.caption}>
          {hoverSpeed ? formatClock(hoverSpeed.time) : t("chart.title")}
        </span>
        <span className={stat.value_row}>
          <span className={`${stat.value}${hoverSpeed ? ` ${stat.value_past}` : ""}`}>
            {speedReady ? speedShown.toFixed(1) : <SkeletonNumber width={28} />}
          </span>
          <span className={stat.unit}>{t("chart.unit")}</span>
        </span>
        <Spark data={speedHistory} dataKey="speed" onHover={setHoverSpeed} />
      </div>
      <div className={stat.card}>
        <span className={stat.caption}>
          {hoverCost ? formatClock(hoverCost.time) : t("cost_chart.title")}
        </span>
        <span className={stat.value_row}>
          <span className={`${stat.value}${hoverCost ? ` ${stat.value_past}` : ""}`}>
            {costReady ? `$${costShown.toFixed(2)}` : <SkeletonNumber width={36} />}
          </span>
          <span className={stat.unit}>{t("cost_chart.unit")}</span>
        </span>
        <Spark data={costHistory} dataKey="costPerMin" onHover={setHoverCost} />
      </div>
    </div>
  );
}
