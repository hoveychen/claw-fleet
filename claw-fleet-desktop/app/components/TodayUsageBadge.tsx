import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { TodayUsage } from "../types";
import { fmtRailMoney } from "../railNumbers";
import { singleFlight } from "../singleFlight";
import { RailStatTile } from "./RailStatTile";
import { TokenReceiptModal } from "./TokenReceiptModal";
import styles from "./TodayUsageBadge.module.css";
import { Presence } from "./Presence";
import { SkeletonNumber } from "./loading";

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
}: { collapsed?: boolean } = {}) {
  const { t } = useTranslation();
  const [usage, setUsage] = useState<TodayUsage | null>(null);
  const [showReceipt, setShowReceipt] = useState(false);
  // Set when a fetch fails before any payload arrived, so the skeleton gives
  // way to a dash instead of shimmering forever on a backend that is down.
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const fetchUsage = async () => {
      try {
        // `sessions-updated` arrives in bursts during the startup scan, and
        // this badge is mounted in every sidebar — without the guard a slow
        // `today_usage` collects a copy per event and per 15s tick, and each
        // copy parks one of Tauri's 10 async-runtime threads. See singleFlight.
        const u = await singleFlight("today_usage", () => invoke<TodayUsage>("today_usage"));
        if (!cancelled) {
          setUsage(u);
          setFailed(false);
        }
      } catch {
        /* backend not ready / remote offline — keep last value */
        if (!cancelled) setFailed(true);
      }
    };
    void fetchUsage();
    const timer = setInterval(fetchUsage, 15_000);
    // `sessions-frame` is the real push (desktop IPC, or SSE in the browser
    // build); `sessions-updated` the mock's.
    const unlisten = ["sessions-frame", "sessions-updated"].map((event) =>
      listen(event, () => {
        void fetchUsage();
      }),
    );
    return () => {
      cancelled = true;
      clearInterval(timer);
      for (const u of unlisten) void u.then((f) => f());
    };
  }, []);

  // `null` means the first `today_usage` has not come back yet — it does NOT
  // mean today cost nothing. Rendering it as `$0.00` made a cold launch look
  // like a broken counter: on 2026-09-14 the first invoke took 64s (a dsh usage
  // fold, since fixed) and the sidebar read `$0.00 · 0 tok` the whole time,
  // right next to a live spend rate of $7/min. A day that genuinely cost zero
  // still reads `$0.00`, because by then `usage` is a real payload.
  const loaded = usage !== null;
  const cost = usage?.costUsd ?? 0;
  // Total tokens = input + output, cumulative across every turn (cache re-reads
  // included), on the same basis as cost — so a heavy day reads large. Agent
  // sessions only (Claude + Codex); Fleet's own LLM calls are deliberately not
  // counted here — see `today_usage` in claw-fleet-core. The daily report card
  // also sums cumulatively, but is Claude-only, so the two need not match.
  const inputTokens = usage?.inputTokens ?? 0;
  const outputTokens = usage?.outputTokens ?? 0;
  const tokens = inputTokens + outputTokens;
  const label = t("today_usage.title", "今日 Fleet 花费");
  const railLabel = t("today_usage.rail_label", "今日累计");
  const loadingText = t("today_usage.loading", "统计中…");
  const tokenBreakdown =
    usage && tokens > 0 ? `\nin ${fmtTokens(inputTokens)} + out ${fmtTokens(outputTokens)}` : "";
  const title = loaded
    ? `${label}: $${cost.toFixed(2)} · ${fmtTokens(tokens)} tok${tokenBreakdown}`
    : `${label}: ${loadingText}`;

  // Portalled to <body>: this badge lives in the sidebar scroller, whose
  // mask-image makes it the containing block for position:fixed and would
  // clip the receipt's overlay to the sidebar.
  const receipt = (
    <Presence when={showReceipt}>
      {createPortal(<TokenReceiptModal onClose={() => setShowReceipt(false)} />, document.body)}
    </Presence>
  );
  const openHint = t("today_usage.open_receipt", "查看用量分析");

  if (collapsed) {
    return (
      <>
        {/* Rail tile: `$3.2k`, not `$3165.41` — the exact figure lives in the
            tooltip and in the receipt this opens. */}
        <RailStatTile
          value={loaded ? fmtRailMoney(cost) : "—"}
          loading={!loaded && !failed}
          label={railLabel}
          title={`${title}\n${openHint}`}
          onClick={() => setShowReceipt(true)}
          dataWizard="today-usage"
        />
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
        <span className={styles.cost}>
          {loaded ? `$${cost.toFixed(2)}` : failed ? "—" : <SkeletonNumber width={52} />}
        </span>
        <span className={styles.tokens}>
          {loaded ? `${fmtTokens(tokens)} tok` : failed ? "" : <SkeletonNumber width={44} />}
        </span>
      </button>
      {receipt}
    </section>
  );
}
