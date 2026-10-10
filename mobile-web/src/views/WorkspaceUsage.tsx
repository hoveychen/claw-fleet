// "Spend by workspace" card on the Account & Usage page — the phone's view of
// the desktop receipt's per-workspace table. Lists every workspace's spend over
// a preset window (via relay `usage_range_breakdown`); tapping a row fetches
// the same window narrowed to that workspace and shows its per-model lines.

import { useEffect, useState } from "react";
import { fetchUsageRangeBreakdown } from "../account";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { ModelReceiptLine, UsageRangeBreakdown, WorkspaceUsageLine } from "../types";
import { SkeletonCard } from "./loading";
import styles from "./UsageView.module.css";

type RangeKey = "today" | "7d" | "30d";

/** Source of the mixed-model line that carries a workspace's daily-report days
 *  (`today_usage::REPORT_SOURCE` in core). */
const REPORT_SOURCE = "report";

/** Local midnight `offsetDays` days ago, in epoch ms. */
function startOfDayMs(offsetDays: number): number {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return d.getTime() - offsetDays * 86_400_000;
}

/** Inclusive window for a preset; today counts as day 1. */
function rangeBounds(range: RangeKey): { fromMs: number; toMs: number } {
  const offset = range === "30d" ? 29 : range === "7d" ? 6 : 0;
  return { fromMs: startOfDayMs(offset), toMs: Date.now() };
}

function fmtUsd(n: number): string {
  if (n === 0) return "$0.00";
  if (n >= 0.01) return `$${n.toFixed(2)}`;
  return "<$0.01";
}

function fmtTok(n: number): string {
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(2)}B`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return `${n}`;
}

function wsTokens(w: WorkspaceUsageLine): number {
  return w.inputTokens + w.cacheCreationTokens + w.cacheReadTokens + w.outputTokens;
}

function lineTokens(l: ModelReceiptLine): number {
  return (
    l.inputTokens + l.cacheCreationTokens + l.cacheCreation1hTokens + l.cacheReadTokens + l.outputTokens
  );
}

/** `claude-opus-4-8` → `opus-4-8`; `openrouter/anthropic/x` → `x`. */
function prettyModel(l: ModelReceiptLine): string {
  if (l.source === REPORT_SOURCE) return t("更早日期");
  const m = l.model || "unknown";
  const base = m.includes("/") ? m.slice(m.lastIndexOf("/") + 1) : m;
  return base.replace(/^claude-/, "");
}

export function WorkspaceUsageCard({ client }: { client: FleetTransport | null }) {
  const [range, setRange] = useState<RangeKey>("today");
  const [data, setData] = useState<UsageRangeBreakdown | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [detail, setDetail] = useState<UsageRangeBreakdown | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);

  useEffect(() => {
    if (!client) return;
    let cancelled = false;
    setError(null);
    const { fromMs, toMs } = rangeBounds(range);
    fetchUsageRangeBreakdown(client, fromMs, toMs, null)
      .then((r) => !cancelled && setData(r))
      .catch((e) => !cancelled && setError(e instanceof Error ? e.message : String(e)));
    return () => {
      cancelled = true;
    };
  }, [client, range]);

  useEffect(() => {
    setDetail(null);
    setDetailError(null);
    if (!client || !open) return;
    let cancelled = false;
    const { fromMs, toMs } = rangeBounds(range);
    fetchUsageRangeBreakdown(client, fromMs, toMs, open)
      .then((r) => !cancelled && setDetail(r))
      .catch((e) => !cancelled && setDetailError(e instanceof Error ? e.message : String(e)));
    return () => {
      cancelled = true;
    };
  }, [client, range, open]);

  const ranges: { key: RangeKey; label: string }[] = [
    { key: "today", label: t("今天") },
    { key: "7d", label: t("近 7 天") },
    { key: "30d", label: t("近 30 天") },
  ];
  const rows = data?.byWorkspace ?? [];
  const total = rows.reduce((acc, w) => acc + w.costUsd, 0);

  return (
    <div className={styles.card}>
      <div className={styles.wsRanges} role="tablist">
        {ranges.map((r) => (
          <button
            key={r.key}
            role="tab"
            aria-selected={range === r.key}
            className={styles.wsRange}
            data-active={range === r.key || undefined}
            onClick={() => setRange(r.key)}
          >
            {r.label}
          </button>
        ))}
      </div>
      <div className={styles.divider} />
      {error ? (
        <div className={styles.hint}>{t("用量加载失败：{0}", error)}</div>
      ) : !data ? (
        <div className={styles.wsPad}>
          <SkeletonCard height={120} />
        </div>
      ) : rows.length === 0 ? (
        <div className={styles.hint}>{t("此区间还没有用量")}</div>
      ) : (
        rows.map((w, i) => {
          const expanded = open === w.workspacePath;
          const share = total > 0 ? w.costUsd / total : 0;
          return (
            <div key={w.workspacePath}>
              {i > 0 && <div className={styles.divider} />}
              <button
                className={styles.wsRow}
                aria-expanded={expanded}
                onClick={() => setOpen(expanded ? null : w.workspacePath)}
              >
                <span className={styles.wsHead}>
                  <span className={styles.wsName}>{w.workspaceName || w.workspacePath}</span>
                  <span className={styles.wsCost}>{fmtUsd(w.costUsd)}</span>
                </span>
                <span className={styles.wsMeta}>
                  <span className={styles.wsTrack}>
                    <span className={styles.wsFill} style={{ width: `${share * 100}%` }} />
                  </span>
                  <span>{t("{0} token", fmtTok(wsTokens(w)))}</span>
                </span>
              </button>
              {expanded && (
                <div className={styles.wsDetail}>
                  {detailError ? (
                    <div className={styles.wsDetailLine}>{t("用量加载失败：{0}", detailError)}</div>
                  ) : !detail ? (
                    <SkeletonCard height={40} />
                  ) : (
                    <>
                      {detail.lines.map((l) => (
                        <div key={`${l.source}:${l.model}`} className={styles.wsDetailLine}>
                          <span className={styles.wsModel}>{prettyModel(l)}</span>
                          <span>
                            {fmtTok(lineTokens(l))} · {fmtUsd(l.costUsd)}
                          </span>
                        </div>
                      ))}
                      {detail.lines.some((l) => l.source === REPORT_SOURCE) && (
                        <div className={styles.wsNote}>
                          {t("7 天前的会话记录已清理，这些日子只有该 workspace 的总额，没有按模型的明细")}
                        </div>
                      )}
                    </>
                  )}
                </div>
              )}
            </div>
          );
        })
      )}
    </div>
  );
}
