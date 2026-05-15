import { useTranslation } from "react-i18next";
import { Line, LineChart, ResponsiveContainer } from "recharts";
import { useSessionsStore } from "../store";
import styles from "./LiveStats.module.css";

function Spark({
  data,
  dataKey,
}: {
  data: readonly { time: number }[];
  dataKey: "speed" | "costPerMin";
}) {
  if (data.length < 2) return <div className={styles.spark_placeholder} />;
  return (
    <div className={styles.spark}>
      <ResponsiveContainer width="100%" height={24}>
        <LineChart
          data={data as object[]}
          margin={{ top: 2, right: 0, left: 0, bottom: 0 }}
        >
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

  const currentSpeed =
    speedHistory.length > 0 ? speedHistory[speedHistory.length - 1].speed : 0;
  const currentCost =
    costHistory.length > 0
      ? costHistory[costHistory.length - 1].costPerMin
      : 0;

  if (collapsed) {
    return (
      <div className={styles.tiles} data-wizard="token-speed">
        <div
          className={styles.tile}
          title={`${t("chart.title")}: ${currentSpeed.toFixed(1)} ${t("chart.unit")}`}
        >
          <span className={styles.tile_value}>
            {Math.round(currentSpeed)}
          </span>
          <span className={styles.tile_label}>{t("chart.unit")}</span>
        </div>
        <div
          className={styles.tile}
          title={`${t("cost_chart.title")}: $${currentCost.toFixed(2)} ${t("cost_chart.unit")}`}
        >
          <span className={styles.tile_value}>
            ${currentCost.toFixed(2)}
          </span>
          <span className={styles.tile_label}>{t("cost_chart.unit")}</span>
        </div>
      </div>
    );
  }

  return (
    <section className={styles.section} data-wizard="token-speed">
      <h3 className={styles.section_title}>{t("live.section_title")}</h3>
      <div className={styles.kpis}>
        <div className={styles.kpi}>
          <div className={styles.kpi_label}>{t("chart.title")}</div>
          <div className={styles.kpi_value_row}>
            <span className={styles.kpi_value}>
              {currentSpeed.toFixed(1)}
            </span>
            <span className={styles.kpi_unit}>{t("chart.unit")}</span>
          </div>
          <Spark data={speedHistory} dataKey="speed" />
        </div>
        <div className={styles.kpi}>
          <div className={styles.kpi_label}>{t("cost_chart.title")}</div>
          <div className={styles.kpi_value_row}>
            <span className={styles.kpi_value}>
              ${currentCost.toFixed(2)}
            </span>
            <span className={styles.kpi_unit}>{t("cost_chart.unit")}</span>
          </div>
          <Spark data={costHistory} dataKey="costPerMin" />
        </div>
      </div>
    </section>
  );
}
