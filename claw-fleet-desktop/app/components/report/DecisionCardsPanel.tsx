import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import type { DecisionCardStats, DecisionTypeStats } from "../../types";
import styles from "./ReportView.module.css";

// Display order + label key for the three logged card types.
const TYPE_ORDER: Array<{ key: string; labelKey: string }> = [
  { key: "elicitation", labelKey: "report.dc_type_elicitation" },
  { key: "fleet-ask", labelKey: "report.dc_type_fleet_ask" },
  { key: "plan-approval", labelKey: "report.dc_type_plan_approval" },
];

function formatLatency(secsSum: number, count: number): string {
  if (count === 0) return "—";
  const avg = secsSum / count;
  if (avg < 60) return `${Math.round(avg)}s`;
  const m = Math.floor(avg / 60);
  const s = Math.round(avg % 60);
  return `${m}m${s.toString().padStart(2, "0")}s`;
}

function pct(n: number, d: number): string {
  if (d === 0) return "";
  return `${Math.round((n / d) * 100)}%`;
}

export function DecisionCardsPanel({ stats }: { stats?: DecisionCardStats }) {
  const { t } = useTranslation();

  const rows = useMemo(() => {
    const byType = stats?.byType ?? {};
    return TYPE_ORDER.map((tt) => ({
      ...tt,
      s: byType[tt.key] as DecisionTypeStats | undefined,
    })).filter((r) => r.s && r.s.triggered > 0);
  }, [stats]);

  if (rows.length === 0) return null;

  return (
    <div className={styles.chart_card}>
      <h3 className={styles.chart_title}>{t("report.decision_cards")}</h3>
      <table className={styles.dc_table}>
        <thead>
          <tr>
            <th>{t("report.dc_type")}</th>
            <th>{t("report.dc_triggered")}</th>
            <th>{t("report.dc_answered")}</th>
            <th>{t("report.dc_recommended_hit")}</th>
            <th>{t("report.dc_other")}</th>
            <th>{t("report.dc_avg_latency")}</th>
            <th>{t("report.dc_timeout")}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map(({ key, labelKey, s }) => {
            const st = s!;
            return (
              <tr key={key}>
                <td className={styles.dc_type_name}>{t(labelKey)}</td>
                <td>{st.triggered}</td>
                <td>
                  {st.answered}
                  {st.triggered > 0 && (
                    <span className={styles.dc_rate}>{pct(st.answered, st.triggered)}</span>
                  )}
                </td>
                <td>
                  {st.withRecommendation > 0 ? (
                    <>
                      {st.recommendedHit}/{st.withRecommendation}
                      <span className={styles.dc_rate}>
                        {pct(st.recommendedHit, st.withRecommendation)}
                      </span>
                    </>
                  ) : (
                    <span className={styles.dc_dim}>{t("report.dc_no_rec")}</span>
                  )}
                </td>
                <td className={st.otherPick > 0 ? styles.dc_warn : undefined}>
                  {st.otherPick}
                  {st.answered > 0 && st.otherPick > 0 && (
                    <span className={styles.dc_rate}>{pct(st.otherPick, st.answered)}</span>
                  )}
                </td>
                <td>{formatLatency(st.latencySecsSum, st.latencyCount)}</td>
                <td className={st.timeout > 0 ? styles.dc_warn : undefined}>{st.timeout}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
