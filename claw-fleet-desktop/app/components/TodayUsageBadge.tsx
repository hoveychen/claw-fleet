import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { TodayUsage } from "../types";
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
  const tokens = usage?.outputTokens ?? 0;
  const label = t("today_usage.title", "今日累计");
  const breakdown =
    usage && usage.fleetCostUsd > 0
      ? `\nagent $${usage.agentCostUsd.toFixed(2)} + fleet $${usage.fleetCostUsd.toFixed(2)}`
      : "";
  const title = `${label}: $${cost.toFixed(2)} · ${fmtTokens(tokens)} tok${breakdown}`;

  // Compact one-line variant for narrow chrome (lite mode's drag bar):
  // "$x · xK tok" on a single row, no section title.
  if (inline) {
    return (
      <span className={styles.badge_inline} title={title}>
        <span className={styles.cost}>${cost.toFixed(2)}</span>
        <span className={styles.tokens}>{fmtTokens(tokens)}</span>
      </span>
    );
  }

  if (collapsed) {
    return (
      <div className={styles.badge_collapsed} title={title}>
        <span className={styles.cost}>${cost.toFixed(2)}</span>
        <span className={styles.tile_label}>{label}</span>
      </div>
    );
  }

  return (
    <section className={styles.section} title={title} data-wizard="today-usage">
      <h3 className={styles.section_title}>{label}</h3>
      <div className={styles.row}>
        <span className={styles.cost}>${cost.toFixed(2)}</span>
        <span className={styles.tokens}>{fmtTokens(tokens)} tok</span>
      </div>
    </section>
  );
}
