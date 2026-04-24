import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Area,
  AreaChart,
  CartesianGrid,
  Legend,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import styles from "./UsageTrendPanel.module.css";

interface FleetLlmUsageDailyBucket {
  date: string;
  scenario: string;
  calls: number;
  inputTokens: number;
  outputTokens: number;
  cacheCreationTokens: number;
  cacheReadTokens: number;
  costUsd: number;
  hasEstimatedTokens: boolean;
  hasUnpricedCalls: boolean;
}

// Keep in sync with the SCENARIO_* constants in claw-fleet-core/src/llm_usage.rs.
// Order determines stacking order (bottom-up) and legend order.
const SCENARIOS = [
  "guard_command",
  "audit_rules",
  "daily_report_summary",
  "daily_report_lessons",
  "session_analyze",
  "mascot_quips",
] as const;
type Scenario = (typeof SCENARIOS)[number];

// Distinct hues — picked for readability on both light and dark themes.
const SCENARIO_COLORS: Record<Scenario, string> = {
  guard_command: "#ef5a6f",        // red — safety critical
  audit_rules: "#f59e0b",          // amber — advisory
  daily_report_summary: "#3b82f6", // blue — report
  daily_report_lessons: "#6366f1", // indigo — report sibling
  session_analyze: "#10b981",      // emerald — frequent ambient
  mascot_quips: "#a855f7",         // violet — cosmetic
};

type RangeKey = "7d" | "30d" | "all";

type Metric = "tokens" | "cost" | "calls";

interface ChartRow {
  date: string;
  [scenario: string]: string | number;
}

function rangeToMsWindow(range: RangeKey): { fromMs: number; toMs: number } {
  const now = Date.now();
  const toMs = now;
  if (range === "7d") return { fromMs: now - 7 * 86_400_000, toMs };
  if (range === "30d") return { fromMs: now - 30 * 86_400_000, toMs };
  // "all" — cheap sentinel: 5 years back is enough, 0 would also work.
  return { fromMs: now - 5 * 365 * 86_400_000, toMs };
}

function totalTokens(b: FleetLlmUsageDailyBucket): number {
  return (
    b.inputTokens +
    b.outputTokens +
    b.cacheCreationTokens +
    b.cacheReadTokens
  );
}

function metricOf(b: FleetLlmUsageDailyBucket, metric: Metric): number {
  if (metric === "tokens") return totalTokens(b);
  if (metric === "cost") return b.costUsd;
  return b.calls;
}

function formatNumber(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return n.toFixed(0);
}

function formatCost(n: number): string {
  if (n === 0) return "$0";
  if (n < 0.01) return `$${n.toFixed(4)}`;
  if (n < 1) return `$${n.toFixed(3)}`;
  return `$${n.toFixed(2)}`;
}

export function UsageTrendPanel() {
  const { t } = useTranslation();
  const [range, setRange] = useState<RangeKey>("7d");
  const [metric, setMetric] = useState<Metric>("tokens");
  const [buckets, setBuckets] = useState<FleetLlmUsageDailyBucket[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const { fromMs, toMs } = rangeToMsWindow(range);
    setLoading(true);
    setError(null);
    invoke<FleetLlmUsageDailyBucket[]>("list_fleet_llm_usage_daily", {
      fromMs,
      toMs,
    })
      .then((rows) => setBuckets(rows))
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [range]);

  // Fill in missing (date, scenario) slots with zeros so the stacked chart
  // renders contiguously — recharts needs every row to carry every series key.
  const { chartData, perScenario, totals, hasEstimated, hasUnpriced } =
    useMemo(() => {
      const perScenario = new Map<Scenario, {
        tokens: number;
        cost: number;
        calls: number;
      }>();
      SCENARIOS.forEach((s) =>
        perScenario.set(s, { tokens: 0, cost: 0, calls: 0 }),
      );

      // Distinct date keys, sorted.
      const dateSet = new Set<string>();
      let hasEstimated = false;
      let hasUnpriced = false;
      for (const b of buckets) {
        dateSet.add(b.date);
        if (b.hasEstimatedTokens) hasEstimated = true;
        if (b.hasUnpricedCalls) hasUnpriced = true;
        const entry = perScenario.get(b.scenario as Scenario);
        if (entry) {
          entry.tokens += totalTokens(b);
          entry.cost += b.costUsd;
          entry.calls += b.calls;
        }
      }
      const dates = Array.from(dateSet).sort();

      const chartData: ChartRow[] = dates.map((date) => {
        const row: ChartRow = { date };
        for (const s of SCENARIOS) row[s] = 0;
        return row;
      });
      const dateIdx = new Map(dates.map((d, i) => [d, i]));
      for (const b of buckets) {
        const i = dateIdx.get(b.date);
        if (i === undefined) continue;
        const row = chartData[i];
        const scenario = b.scenario as Scenario;
        if (!SCENARIOS.includes(scenario)) continue;
        row[scenario] = ((row[scenario] as number) ?? 0) + metricOf(b, metric);
      }

      const totals = Array.from(perScenario.values()).reduce(
        (acc, cur) => ({
          tokens: acc.tokens + cur.tokens,
          cost: acc.cost + cur.cost,
          calls: acc.calls + cur.calls,
        }),
        { tokens: 0, cost: 0, calls: 0 },
      );

      return { chartData, perScenario, totals, hasEstimated, hasUnpriced };
    }, [buckets, metric]);

  const yAxisFormatter = (v: number) =>
    metric === "cost" ? formatCost(v) : formatNumber(v);

  return (
    <div className={styles.root}>
      <div className={styles.header_row}>
        <div className={styles.totals}>
          <div className={styles.total_block}>
            <div className={styles.total_value}>
              {formatNumber(totals.tokens)}
            </div>
            <div className={styles.total_label}>
              {t("usage.total_tokens")}
            </div>
          </div>
          <div className={styles.total_block}>
            <div className={styles.total_value}>
              {formatCost(totals.cost)}
            </div>
            <div className={styles.total_label}>
              {t("usage.total_cost")}
            </div>
          </div>
          <div className={styles.total_block}>
            <div className={styles.total_value}>
              {formatNumber(totals.calls)}
            </div>
            <div className={styles.total_label}>
              {t("usage.total_calls")}
            </div>
          </div>
        </div>

        <div className={styles.controls}>
          <div className={styles.seg}>
            {(["tokens", "cost", "calls"] as const).map((m) => (
              <button
                key={m}
                className={`${styles.seg_btn} ${metric === m ? styles.seg_btn_active : ""}`}
                onClick={() => setMetric(m)}
                type="button"
              >
                {t(`usage.metric_${m}`)}
              </button>
            ))}
          </div>
          <div className={styles.seg}>
            {(["7d", "30d", "all"] as const).map((r) => (
              <button
                key={r}
                className={`${styles.seg_btn} ${range === r ? styles.seg_btn_active : ""}`}
                onClick={() => setRange(r)}
                type="button"
              >
                {t(`usage.range_${r}`)}
              </button>
            ))}
          </div>
        </div>
      </div>

      {(hasEstimated || hasUnpriced) && (
        <div className={styles.disclaimer}>
          {hasEstimated && <span>{t("usage.disclaimer_estimated")}</span>}
          {hasEstimated && hasUnpriced && " "}
          {hasUnpriced && <span>{t("usage.disclaimer_unpriced")}</span>}
        </div>
      )}

      {loading ? (
        <div className={styles.empty}>{t("usage.loading")}</div>
      ) : error ? (
        <div className={styles.empty}>
          {t("usage.error", { error })}
        </div>
      ) : chartData.length === 0 ? (
        <div className={styles.empty}>{t("usage.no_data")}</div>
      ) : (
        <>
          <div className={styles.chart_box}>
            <ResponsiveContainer width="100%" height={240}>
              <AreaChart
                data={chartData}
                margin={{ top: 8, right: 16, left: 8, bottom: 0 }}
              >
                <defs>
                  {SCENARIOS.map((s) => (
                    <linearGradient
                      key={s}
                      id={`usageGrad_${s}`}
                      x1="0"
                      y1="0"
                      x2="0"
                      y2="1"
                    >
                      <stop
                        offset="5%"
                        stopColor={SCENARIO_COLORS[s]}
                        stopOpacity={0.6}
                      />
                      <stop
                        offset="95%"
                        stopColor={SCENARIO_COLORS[s]}
                        stopOpacity={0.1}
                      />
                    </linearGradient>
                  ))}
                </defs>
                <CartesianGrid
                  stroke="var(--color-border)"
                  strokeDasharray="3 3"
                  vertical={false}
                />
                <XAxis
                  dataKey="date"
                  tick={{ fontSize: 10, fill: "var(--color-text-dim)" }}
                  tickLine={false}
                  axisLine={false}
                  interval="preserveStartEnd"
                  minTickGap={30}
                />
                <YAxis
                  tick={{ fontSize: 10, fill: "var(--color-text-dim)" }}
                  tickLine={false}
                  axisLine={false}
                  width={48}
                  tickFormatter={yAxisFormatter}
                />
                <Tooltip
                  contentStyle={{
                    background: "var(--color-bg-secondary)",
                    border: "1px solid var(--color-border)",
                    borderRadius: 6,
                    fontSize: 11,
                    color: "var(--color-text)",
                  }}
                  formatter={(v, name) => {
                    const n = typeof v === "number" ? v : Number(v) || 0;
                    return [
                      metric === "cost" ? formatCost(n) : formatNumber(n),
                      t(`usage.scenario_${String(name)}`),
                    ];
                  }}
                />
                {SCENARIOS.map((s) => (
                  <Area
                    key={s}
                    type="monotone"
                    dataKey={s}
                    stackId="usage"
                    stroke={SCENARIO_COLORS[s]}
                    strokeWidth={1}
                    fill={`url(#usageGrad_${s})`}
                    isAnimationActive={false}
                  />
                ))}
                <Legend
                  iconType="circle"
                  iconSize={8}
                  formatter={(name) => (
                    <span className={styles.legend_label}>
                      {t(`usage.scenario_${name}`)}
                    </span>
                  )}
                  wrapperStyle={{ fontSize: 11, paddingTop: 8 }}
                />
              </AreaChart>
            </ResponsiveContainer>
          </div>

          <table className={styles.table}>
            <thead>
              <tr>
                <th>{t("usage.col_scenario")}</th>
                <th>{t("usage.col_calls")}</th>
                <th>{t("usage.col_tokens")}</th>
                <th>{t("usage.col_cost")}</th>
              </tr>
            </thead>
            <tbody>
              {SCENARIOS.map((s) => {
                const row = perScenario.get(s)!;
                return (
                  <tr key={s}>
                    <td>
                      <span
                        className={styles.dot}
                        style={{ background: SCENARIO_COLORS[s] }}
                      />
                      {t(`usage.scenario_${s}`)}
                    </td>
                    <td>{formatNumber(row.calls)}</td>
                    <td>{formatNumber(row.tokens)}</td>
                    <td>{formatCost(row.cost)}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </>
      )}
    </div>
  );
}
