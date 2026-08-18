import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import type { DshSessionCost, DshTokenBreakdown } from "../types";
import styles from "./TokenSpendPanel.module.css";

interface Props {
  /** `dsh://<session-id>` — dsh sessions have no file, so this is a URI. */
  uri: string;
}

function fmtTok(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

/**
 * Token breakdown for a dsh session.
 *
 * The other two panels parse a transcript file; dsh has none, so these numbers
 * come off the projections its own server folds (`session.list`). It reports two
 * things that must stay visually separate — they are not addends of each other:
 *
 *  - **Billed** — the four cumulative buckets dsh meters over the session.
 *  - **Context** — what occupies the window *right now*. Re-reading a large
 *    cached prefix inflates the billed buckets while leaving this untouched,
 *    so a session can show 71k billed against 9k of context.
 *
 * The cost figure comes from one of two places, and the KPI's own sub-label says
 * which: a provider that issues a receipt per generation is **asked** what it
 * charged (never inferred from the token counts above — the same model costs
 * different amounts through different providers, so multiplying by any table
 * Fleet holds would produce a confident wrong number), while a route with one
 * seller and a published price list is priced from those rates. It loads on its
 * own (it needs the full session history and may hit the network), so the token
 * view never waits on it. Routed here from SessionDetail via
 * `tokenPanelForAgentSource`.
 */
export function DshTokenPanel({ uri }: Props) {
  const { t } = useTranslation();
  const [data, setData] = useState<DshTokenBreakdown | null>(null);
  const [cost, setCost] = useState<DshSessionCost | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    invoke<DshTokenBreakdown>("get_dsh_token_breakdown", { uri })
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
    // Deliberately not awaited alongside the breakdown: a slow or failing cost
    // lookup must never keep the token numbers off the screen.
    setCost(null);
    invoke<DshSessionCost>("get_dsh_session_cost", { uri })
      .then((c) => {
        if (!cancelled) setCost(c);
      })
      .catch(() => {
        /* the panel simply shows no cost card */
      });
    return () => {
      cancelled = true;
    };
  }, [uri]);

  if (loading)
    return <div className={styles.empty}>{t("dsh_token.loading") || "Loading…"}</div>;
  if (error) return <div className={styles.empty}>{error}</div>;
  if (!data) return <div className={styles.empty}>{t("dsh_token.no_data") || "No data"}</div>;

  return <DshTokenView data={data} cost={cost} />;
}

interface Row {
  key: string;
  label: string;
  colorTag: string;
  value: number;
}

function fmtUsd(n: number): string {
  // Sub-cent totals are the common case for a short session, and "<$0.01" threw
  // away the figure the panel had just gone to the trouble of obtaining — a
  // DeepSeek-official turn can cost $0.0028 and the user should see that. Four
  // decimals below a dollar; only below a hundredth of a cent, where four
  // decimals really would round to zero, does it fall back to a bound. $0.00 is
  // never printed: it would read as "free" when it means "very cheap".
  if (n > 0 && n < 0.0001) return "<$0.0001";
  return `$${n.toFixed(n < 1 ? 4 : 2)}`;
}

/// Where the figure came from, in the KPI's own words.
///
/// Two kinds of number can sit under "Real spend" and the label must not blur
/// them: a receipt the provider issued, and a published rate applied to counted
/// tokens. Saying "priced by the provider" over a table-priced total would be a
/// small lie in the one place the panel exists to be exact.
function costCallsLabel(
  cost: { pricedCalls: number; tablePricedCalls?: number | null; note: string },
  t: (key: string, vars?: Record<string, unknown>) => string,
): string {
  if (cost.pricedCalls <= 0) return cost.note;
  // A remote probe can be an older build than the desktop it answers — Fleet's
  // RemoteBackend talks to whatever `fleet serve` runs on the other machine — so
  // this field may simply be absent. Treating that as 0 keeps the old wording
  // (which was correct for a receipt-only build); subtracting `undefined` put a
  // literal "NaN by the provider" on screen, which is how this was caught.
  const tablePriced = cost.tablePricedCalls ?? 0;
  const byReceipt = cost.pricedCalls - tablePriced;
  if (tablePriced <= 0) {
    return (
      t("dsh_token.cost_calls", { count: cost.pricedCalls }) ||
      `${cost.pricedCalls} call(s) priced by the provider`
    );
  }
  if (byReceipt <= 0) {
    return (
      t("dsh_token.cost_calls_rates", { count: tablePriced }) ||
      `${tablePriced} call(s) priced from published rates`
    );
  }
  return (
    t("dsh_token.cost_calls_mixed", {
      receipt: byReceipt,
      rates: tablePriced,
    }) || `${byReceipt} by the provider / ${tablePriced} from published rates`
  );
}

export function DshTokenView({
  data,
  cost,
}: {
  data: DshTokenBreakdown;
  cost?: DshSessionCost | null;
}) {
  const { t } = useTranslation();

  const billed: Row[] = [
    {
      key: "uncached",
      label: t("dsh_token.row_uncached") || "Input (full-price)",
      colorTag: "warn",
      value: data.uncachedInputTokens,
    },
    {
      key: "cache_read",
      label: t("dsh_token.row_cache_read") || "Cache read",
      colorTag: "info",
      value: data.cacheReadTokens,
    },
    {
      key: "cache_write",
      label: t("dsh_token.row_cache_write") || "Cache write",
      colorTag: "system",
      value: data.cacheWriteTokens,
    },
    {
      key: "output",
      label: t("dsh_token.row_output") || "Output",
      colorTag: "residual",
      value: data.outputTokens,
    },
  ];

  const context: Row[] = [
    {
      key: "system",
      label: t("dsh_token.row_system") || "System prompt",
      colorTag: "system",
      value: data.systemTokens,
    },
    {
      key: "tools",
      label: t("dsh_token.row_tools") || "Tool definitions",
      colorTag: "info",
      value: data.toolsTokens,
    },
    {
      key: "messages",
      label: t("dsh_token.row_messages") || "Messages",
      colorTag: "warn",
      value: data.messageTokens,
    },
  ];
  const contextTotal = context.reduce((a, r) => a + r.value, 0);

  return (
    <div className={styles.panel}>
      <div className={styles.kpi_row}>
        <KpiCard
          label={t("dsh_token.kpi_total") || "Total tokens (billed)"}
          primary={fmtTok(data.totalTokens)}
        />
        <KpiCard
          label={t("dsh_token.kpi_cost") || "Real spend"}
          primary={cost?.totalUsd != null ? fmtUsd(cost.totalUsd) : "—"}
          secondary={cost ? costCallsLabel(cost, t) : ""}
        />
        <KpiCard
          label={t("dsh_token.kpi_context") || "Context used"}
          primary={
            data.contextPercent != null ? `${(data.contextPercent * 100).toFixed(0)}%` : "—"
          }
          secondary={
            data.projectedTokens != null && data.contextWindow != null
              ? `${fmtTok(data.projectedTokens)} / ${fmtTok(data.contextWindow)}`
              : ""
          }
        />
      </div>

      <div className={styles.section_label}>
        {t("dsh_token.section_billed") || "Billed tokens (cumulative)"}
      </div>
      <StackedBar rows={billed} total={data.totalTokens} />
      <LegendTable rows={billed} total={data.totalTokens} totalLabel={t("dsh_token.row_total") || "Total"} />

      <div className={styles.section_label}>
        {t("dsh_token.section_context") || "Context window composition (now)"}
      </div>
      <StackedBar rows={context} total={contextTotal} />
      <LegendTable
        rows={context}
        total={contextTotal}
        totalLabel={t("dsh_token.row_context_total") || "In window"}
      />

      {cost?.note && cost.pricedCalls > 0 && (
        <div className={styles.caveat}>{cost.note}</div>
      )}

      <div className={styles.caveat}>
        {t("dsh_token.caveat") ||
          "Token counts come from dsh's own session projections. The billed buckets are cumulative over the whole session; the context composition is a snapshot of the window right now, so the two do not add up. The spend figure is what the provider actually charged for this session's generations — it is never derived from the token counts, because the same model costs different amounts through different providers."}
      </div>
    </div>
  );
}

function StackedBar({ rows, total }: { rows: Row[]; total: number }) {
  return (
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
  );
}

function LegendTable({
  rows,
  total,
  totalLabel,
}: {
  rows: Row[];
  total: number;
  totalLabel: string;
}) {
  const { t } = useTranslation();
  return (
    <div className={styles.legend_table}>
      <div className={styles.legend_head}>
        <span>{t("dsh_token.col_source") || "Source"}</span>
        <span className={styles.col_num}>{t("dsh_token.col_tokens") || "Tokens"}</span>
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
          <strong>{totalLabel}</strong>
        </span>
        <span className={styles.col_num}>
          <strong>{fmtTok(total)}</strong>
        </span>
        <span className={styles.col_num}>{total > 0 ? "100%" : "—"}</span>
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
