import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import type { CodexTokenBreakdown } from "../types";
import styles from "./TokenSpendPanel.module.css";

interface Props {
  jsonlPath: string;
}

function fmtTok(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

function fmtUsd(n: number): string {
  if (n >= 1) return `$${n.toFixed(2)}`;
  if (n >= 0.01) return `$${n.toFixed(3)}`;
  return `$${n.toFixed(4)}`;
}

/**
 * Token breakdown for a Codex rollout. Codex has none of Claude's
 * source-attribution buckets (system prompt / tools / CLAUDE.md / memory /
 * MCP), so this renders the raw `total_token_usage` split Codex actually
 * records: full-price input, cached input, and output. Routed here from
 * SessionDetail when `agentSource === "codex"` — Claude sessions keep the
 * richer `TokenSpendPanel`.
 */
export function CodexTokenPanel({ jsonlPath }: Props) {
  const { t } = useTranslation();
  const [data, setData] = useState<CodexTokenBreakdown | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    invoke<CodexTokenBreakdown>("get_codex_token_breakdown", { jsonlPath })
      .then((r) => {
        if (cancelled) return;
        setData(r);
        setLoading(false);
      })
      .catch((e) => {
        if (cancelled) return;
        setError(String(e));
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [jsonlPath]);

  if (loading)
    return <div className={styles.empty}>{t("codex_token.loading") || "Loading…"}</div>;
  if (error) return <div className={styles.empty}>{error}</div>;
  if (!data) return <div className={styles.empty}>{t("codex_token.no_data") || "No data"}</div>;

  return <CodexTokenView data={data} />;
}

function CodexTokenView({ data }: { data: CodexTokenBreakdown }) {
  const { t } = useTranslation();

  const rows: { key: string; label: string; colorTag: string; value: number }[] = [
    {
      key: "input",
      label: t("codex_token.row_input") || "Input (full-price)",
      colorTag: "system",
      value: data.inputTokens,
    },
    {
      key: "cached",
      label: t("codex_token.row_cached") || "Cached input (cache-read)",
      colorTag: "info",
      value: data.cachedInputTokens,
    },
    {
      key: "output",
      label: t("codex_token.row_output") || "Output (incl. reasoning)",
      colorTag: "warn",
      value: data.outputTokens,
    },
  ];
  const total = data.totalTokens;

  return (
    <div className={styles.panel}>
      {/* KPI cards */}
      <div className={styles.kpi_row}>
        <KpiCard
          label={t("codex_token.kpi_total") || "Total tokens"}
          primary={fmtTok(total)}
          secondary={data.model ? data.model.replace(/^gpt-/, "") : ""}
        />
        <KpiCard
          label={t("codex_token.kpi_cost") || "Cost"}
          primary={fmtUsd(data.costUsd)}
        />
        <KpiCard
          label={t("codex_token.kpi_context") || "Context used"}
          primary={
            data.contextPercent != null
              ? `${(data.contextPercent * 100).toFixed(0)}%`
              : "—"
          }
        />
      </div>

      {/* Stacked bar */}
      <div className={styles.section_label}>
        {t("codex_token.section_breakdown") || "Token breakdown"}
      </div>
      <div className={styles.stacked_bar}>
        {rows.map((r) => {
          if (r.value <= 0) return null;
          const pct = total > 0 ? (r.value / total) * 100 : 0;
          if (pct < 0.5) return null;
          return (
            <div
              key={r.key}
              className={styles.bar_segment}
              data-color={r.colorTag}
              style={{ flexGrow: pct }}
              title={`${r.label}: ${fmtTok(r.value)} (${pct.toFixed(1)}%)`}
            />
          );
        })}
      </div>

      {/* Legend table */}
      <div className={styles.legend_table}>
        <div className={styles.legend_head}>
          <span>{t("codex_token.col_source") || "Source"}</span>
          <span className={styles.col_num}>{t("codex_token.col_tokens") || "Tokens"}</span>
          <span className={styles.col_num}>%</span>
        </div>
        {rows.map((r) => {
          const pct = total > 0 ? (r.value / total) * 100 : 0;
          return (
            <div key={r.key} className={styles.legend_row}>
              <span className={styles.legend_label}>
                <span className={styles.dot} data-color={r.colorTag} />
                {r.label}
              </span>
              <span className={styles.col_num}>{fmtTok(r.value)}</span>
              <span className={styles.col_num}>{pct.toFixed(1)}%</span>
            </div>
          );
        })}
        <div className={`${styles.legend_row} ${styles.legend_total ?? ""}`}>
          <span className={styles.legend_label}>
            <strong>{t("codex_token.row_total") || "Total"}</strong>
          </span>
          <span className={styles.col_num}>
            <strong>{fmtTok(total)}</strong>
          </span>
          <span className={styles.col_num}>100%</span>
        </div>
      </div>

      {/* Caveat */}
      <div className={styles.caveat}>
        {t("codex_token.caveat") ||
          "From Codex's cumulative total_token_usage. Cached input is a subset of raw input billed at the cache-read rate; output already includes reasoning tokens. Cost uses the model's public price tier."}
      </div>
    </div>
  );
}

function KpiCard({
  label,
  primary,
  secondary,
}: {
  label: string;
  primary: string;
  secondary?: string;
}) {
  return (
    <div className={styles.kpi_card}>
      <div className={styles.kpi_label}>{label}</div>
      <div className={styles.kpi_primary}>{primary}</div>
      {secondary && <div className={styles.kpi_secondary}>{secondary}</div>}
    </div>
  );
}
