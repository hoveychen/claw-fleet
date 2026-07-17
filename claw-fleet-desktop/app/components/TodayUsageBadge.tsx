import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { TodayUsage } from "../types";
import { TokenReceiptModal } from "./TokenReceiptModal";
import styles from "./TodayUsageBadge.module.css";

/** Compact token count: 1.2M / 34.5K / 780. */
function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return `${n}`;
}

/**
 * Today's cumulative spend counter for the sidebar nav. Polls the `today_usage`
 * backend command (works for both LocalBackend and RemoteBackend) and refreshes
 * promptly on every `sessions-updated` push so it tracks live turns.
 */
export function TodayUsageBadge({
  collapsed = false,
  inline = false,
}: { collapsed?: boolean; inline?: boolean } = {}) {
  const { t } = useTranslation();
  const [usage, setUsage] = useState<TodayUsage | null>(null);
  const [showReceipt, setShowReceipt] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const fetchUsage = async () => {
      try {
        const u = await invoke<TodayUsage>("today_usage");
        if (!cancelled) setUsage(u);
      } catch {
        /* backend not ready / remote offline — keep last value */
      }
    };
    void fetchUsage();
    const timer = setInterval(fetchUsage, 15_000);
    const unlisten = listen("sessions-updated", () => {
      void fetchUsage();
    });
    return () => {
      cancelled = true;
      clearInterval(timer);
      void unlisten.then((f) => f());
    };
  }, []);

  const cost = usage?.costUsd ?? 0;
  // Total tokens = input + output, cumulative across every turn (cache re-reads
  // included), on the same口径 as cost — so a heavy day reads large. Counts all
  // sources (Claude + Codex + Fleet's own LLM calls); the daily report card also
  // sums cumulatively now, but is Claude-only, so the two need not match exactly.
  const inputTokens = usage?.inputTokens ?? 0;
  const outputTokens = usage?.outputTokens ?? 0;
  const tokens = inputTokens + outputTokens;
  const label = t("today_usage.title", "今日累计");
  const costBreakdown =
    usage && usage.fleetCostUsd > 0
      ? `\nagent $${usage.agentCostUsd.toFixed(2)} + fleet $${usage.fleetCostUsd.toFixed(2)}`
      : "";
  const tokenBreakdown =
    usage && tokens > 0 ? `\nin ${fmtTokens(inputTokens)} + out ${fmtTokens(outputTokens)}` : "";
  const title = `${label}: $${cost.toFixed(2)} · ${fmtTokens(tokens)} tok${tokenBreakdown}${costBreakdown}`;

  const receipt = showReceipt ? (
    <TokenReceiptModal onClose={() => setShowReceipt(false)} />
  ) : null;
  const openHint = t("today_usage.open_receipt", "查看今日花费明细");

  // Compact one-line variant for narrow chrome (lite mode's drag bar):
  // "$x · xK tok" on a single row, no section title.
  if (inline) {
    return (
      <>
        <button
          type="button"
          className={styles.badge_inline}
          title={openHint}
          onClick={() => setShowReceipt(true)}
        >
          <span className={styles.cost}>${cost.toFixed(2)}</span>
          <span className={styles.tokens}>{fmtTokens(tokens)}</span>
        </button>
        {receipt}
      </>
    );
  }

  if (collapsed) {
    return (
      <>
        <button
          type="button"
          className={styles.badge_collapsed}
          title={openHint}
          onClick={() => setShowReceipt(true)}
        >
          <span className={styles.cost}>${cost.toFixed(2)}</span>
          <span className={styles.tile_label}>{label}</span>
        </button>
        {receipt}
      </>
    );
  }

  return (
    <section className={styles.section} title={title} data-wizard="today-usage">
      <h3 className={styles.section_title}>{label}</h3>
      <button
        type="button"
        className={styles.row}
        title={openHint}
        onClick={() => setShowReceipt(true)}
      >
        <span className={styles.cost}>${cost.toFixed(2)}</span>
        <span className={styles.tokens}>{fmtTokens(tokens)} tok</span>
      </button>
      {receipt}
    </section>
  );
}
