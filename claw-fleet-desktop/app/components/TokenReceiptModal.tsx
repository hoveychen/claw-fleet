import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import type {
  DailyUsagePoint,
  ModelReceiptLine,
  TodayUsageBreakdown,
  UsageRangeBreakdown,
} from "../types";
import styles from "./TokenReceiptModal.module.css";

interface Props {
  onClose: () => void;
}

type RangeKey = "today" | "7d" | "30d" | "all";

/** Normalized shape both the today and range breakdowns render through. */
interface ReceiptView {
  /** Header line: a single date, or `from → to` for a multi-day window. */
  label: string;
  lines: ModelReceiptLine[];
  totalInputTokens: number;
  totalCacheCreationTokens: number;
  totalCacheReadTokens: number;
  totalOutputTokens: number;
  totalCostUsd: number;
  /** Per-day trend points (empty for the single-day "today" view). */
  daily: DailyUsagePoint[];
  /** Any Codex session attributed whole-to-one-day → trend is approximate. */
  hasCodexApproximation: boolean;
}

/** 1.23M / 45.6K / 780 — receipt-scale token counts. */
function fmtTok(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return `${n}`;
}

/** Money with cent precision, sub-cent lines get more digits so they aren't $0.00. */
function fmtUsd(n: number): string {
  if (n === 0) return "$0.00";
  if (n >= 0.01) return `$${n.toFixed(2)}`;
  if (n >= 0.0001) return `$${n.toFixed(4)}`;
  return `<$0.0001`;
}

/** Unit price is always $/M tokens. */
function fmtPrice(n: number): string {
  return `$${n.toFixed(2)}/M`;
}

/** Drop the `claude-` noise; leave gpt / others as-is. */
function prettyModel(model: string): string {
  if (!model) return "unknown";
  return model.replace(/^claude-/, "");
}

const SOURCE_LABEL: Record<string, string> = {
  "claude-code": "Claude",
  claude: "Claude",
  codex: "Codex",
  fleet: "Fleet",
};

function sourceLabel(source: string): string {
  return SOURCE_LABEL[source] ?? source;
}

/** Local midnight `offsetDays` days ago, in epoch ms. */
function startOfDayMs(offsetDays = 0): number {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return d.getTime() - offsetDays * 86_400_000;
}

/** Inclusive `[fromMs, toMs]` window for a range preset (today counts as day 1). */
function rangeBounds(range: RangeKey): { fromMs: number; toMs: number } {
  const now = Date.now();
  switch (range) {
    case "7d":
      return { fromMs: startOfDayMs(6), toMs: now };
    case "30d":
      return { fromMs: startOfDayMs(29), toMs: now };
    case "all":
      return { fromMs: 0, toMs: now };
    default:
      return { fromMs: startOfDayMs(0), toMs: now };
  }
}

function normalizeToday(r: TodayUsageBreakdown): ReceiptView {
  return {
    label: r.date,
    lines: r.lines,
    totalInputTokens: r.totalInputTokens,
    totalCacheCreationTokens: r.totalCacheCreationTokens,
    totalCacheReadTokens: r.totalCacheReadTokens,
    totalOutputTokens: r.totalOutputTokens,
    totalCostUsd: r.totalCostUsd,
    daily: [],
    hasCodexApproximation: false,
  };
}

function normalizeRange(r: UsageRangeBreakdown): ReceiptView {
  return {
    label: r.fromDate === r.toDate ? r.fromDate : `${r.fromDate} → ${r.toDate}`,
    lines: r.lines,
    totalInputTokens: r.totalInputTokens,
    totalCacheCreationTokens: r.totalCacheCreationTokens,
    totalCacheReadTokens: r.totalCacheReadTokens,
    totalOutputTokens: r.totalOutputTokens,
    totalCostUsd: r.totalCostUsd,
    daily: r.daily,
    hasCodexApproximation: r.hasCodexApproximation,
  };
}

/**
 * "Receipt" for token spend. Opened by clicking the sidebar counter. Itemises
 * the spend per model — input / cache-write / cache-read / output tokens, each
 * priced at the model's official $/M rate. The default "today" view reconciles
 * to the sidebar counter (agent + Fleet's own LLM spend); the longer ranges
 * (7d / 30d / all) additionally draw a per-day trend from the range breakdown.
 */
export function TokenReceiptModal({ onClose }: Props) {
  const { t } = useTranslation();
  const [range, setRange] = useState<RangeKey>("today");
  const [data, setData] = useState<ReceiptView | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onClose]);

  useEffect(() => {
    let cancelled = false;
    setData(null);
    setError(null);
    const req =
      range === "today"
        ? invoke<TodayUsageBreakdown>("today_usage_breakdown").then(normalizeToday)
        : (() => {
            const { fromMs, toMs } = rangeBounds(range);
            return invoke<UsageRangeBreakdown>("usage_range_breakdown", {
              fromMs,
              toMs,
            }).then(normalizeRange);
          })();
    req
      .then((r) => {
        if (!cancelled) setData(r);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [range]);

  const totalTokens = data
    ? data.totalInputTokens +
      data.totalCacheCreationTokens +
      data.totalCacheReadTokens +
      data.totalOutputTokens
    : 0;

  const ranges: { key: RangeKey; label: string }[] = [
    { key: "today", label: t("token_receipt.range_today", "今天") },
    { key: "7d", label: t("token_receipt.range_7d", "近 7 天") },
    { key: "30d", label: t("token_receipt.range_30d", "近 30 天") },
    { key: "all", label: t("token_receipt.range_all", "全部") },
  ];

  return (
    <div className={styles.overlay} onClick={onClose}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        <div className={styles.header}>
          <span className={styles.title}>
            {t("token_receipt.title", "花费明细")}
          </span>
          <button className={styles.close_btn} onClick={onClose} aria-label="close">
            ✕
          </button>
        </div>

        <div className={styles.range_bar}>
          {ranges.map((r) => (
            <button
              key={r.key}
              className={`${styles.range_btn} ${range === r.key ? styles.range_btn_active : ""}`}
              onClick={() => setRange(r.key)}
            >
              {r.label}
            </button>
          ))}
        </div>

        <div className={styles.body}>
          {error && <div className={styles.empty}>{error}</div>}
          {!error && !data && (
            <div className={styles.empty}>{t("token_receipt.loading", "统计中…")}</div>
          )}
          {data && (
            <div className={styles.receipt}>
              <div className={styles.receipt_head}>
                <div className={styles.shop}>CLAW FLEET</div>
                <div className={styles.receipt_sub}>
                  {t("token_receipt.subtitle", "Token 消费小票")} · {data.label}
                </div>
              </div>

              {data.lines.length === 0 && (
                <div className={styles.empty}>
                  {t("token_receipt.no_usage_range", "此区间还没有用量")}
                </div>
              )}

              {data.daily.length > 0 && <TrendChart daily={data.daily} />}

              {data.lines.map((line, i) => (
                <ReceiptLine key={`${line.source}:${line.model}:${i}`} line={line} />
              ))}

              {data.lines.length > 0 && (
                <>
                  <div className={styles.divider_double} />
                  <div className={styles.grand_row}>
                    <span className={styles.grand_label}>
                      {t("token_receipt.grand_total", "合计")}
                    </span>
                    <span className={styles.grand_value}>{fmtUsd(data.totalCostUsd)}</span>
                  </div>
                  {/* Agent spend only — Fleet's own guard / report LLM calls are
                      not on this receipt, so there is no agent-vs-fleet split to
                      show. Fleet's own consumption lives in Settings → Usage. */}
                  <div className={styles.split_row}>
                    <span />
                    <span className={styles.tok_total}>
                      {fmtTok(totalTokens)} {t("token_receipt.tokens", "tokens")}
                    </span>
                  </div>
                  {data.hasCodexApproximation && (
                    <div className={styles.codex_note}>
                      {t(
                        "token_receipt.codex_note",
                        "Codex 无逐轮明细,其用量整体归到会话起始日,趋势为近似",
                      )}
                    </div>
                  )}
                  <div className={styles.footer_note}>
                    {t(
                      "token_receipt.footnote",
                      "价格为各模型官方 $/M 单价 · 含缓存读写 · 今日与侧边栏计数同口径",
                    )}
                  </div>
                </>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/** Compact per-day cost bar chart (the trend behind a multi-day receipt). */
function TrendChart({ daily }: { daily: DailyUsagePoint[] }) {
  const { t } = useTranslation();
  const max = Math.max(...daily.map((d) => d.costUsd), 0.0001);
  // √-scale the bar heights. Spend spans two orders of magnitude across a long
  // window (early days at a few $, recent days at thousands), so a linear scale
  // crushes every older day to a sub-pixel sliver — the whole reason history
  // looked "empty". sqrt lifts small days into a visible band while preserving
  // ordering; the exact figure stays in each bar's tooltip.
  const sqrtMax = Math.sqrt(max);
  const W = 424;
  const H = 92;
  const pad = 4;
  const n = daily.length;
  const gap = n > 1 ? 2 : 0;
  const barW = Math.max(1, (W - pad * 2 - gap * (n - 1)) / n);

  return (
    <div className={styles.chart}>
      <div className={styles.chart_title}>
        {t("token_receipt.trend_title", "每日花费")}
        <span className={styles.chart_scale_hint}>
          {t("token_receipt.trend_scale_hint", "√ 刻度")}
        </span>
      </div>
      <svg
        viewBox={`0 0 ${W} ${H}`}
        className={styles.chart_svg}
        preserveAspectRatio="none"
      >
        {daily.map((d, i) => {
          const h = Math.max(
            1,
            (Math.sqrt(Math.max(d.costUsd, 0)) / sqrtMax) * (H - pad * 2),
          );
          const x = pad + i * (barW + gap);
          const y = H - pad - h;
          const tok =
            d.inputTokens + d.cacheCreationTokens + d.cacheReadTokens + d.outputTokens;
          return (
            <rect
              key={d.date}
              x={x}
              y={y}
              width={barW}
              height={h}
              rx={1}
              className={styles.bar}
            >
              <title>{`${d.date} · ${fmtUsd(d.costUsd)} · ${fmtTok(tok)} tok`}</title>
            </rect>
          );
        })}
      </svg>
      <div className={styles.chart_axis}>
        <span>{daily[0].date}</span>
        {daily.length > 1 && <span>{daily[daily.length - 1].date}</span>}
      </div>
    </div>
  );
}

function ReceiptLine({ line }: { line: ModelReceiptLine }) {
  const { t } = useTranslation();
  // Cache writes are billed by TTL — a 1-hour write costs 2× the model's input
  // rate, a 5-minute write 1.25× — so they get one row each. Blending them into
  // a single row would leave `Σ rows ≠ subtotal`, which is exactly the receipt
  // bug this split fixed.
  const rows: { label: string; tokens: number; price: number }[] = [
    { label: t("token_receipt.row_input", "输入"), tokens: line.inputTokens, price: line.inputPrice },
    {
      label: t("token_receipt.row_cache_write_1h", "缓存写入 1h"),
      tokens: line.cacheCreation1hTokens,
      price: line.cacheWrite1hPrice,
    },
    {
      label: t("token_receipt.row_cache_write", "缓存写入"),
      tokens: line.cacheCreationTokens,
      price: line.cacheWritePrice,
    },
    {
      label: t("token_receipt.row_cache_read", "缓存读取"),
      tokens: line.cacheReadTokens,
      price: line.cacheReadPrice,
    },
    {
      label: t("token_receipt.row_output", "输出"),
      tokens: line.outputTokens,
      price: line.outputPrice,
    },
  ];

  return (
    <div className={styles.line}>
      <div className={styles.line_head}>
        <span className={styles.model}>{prettyModel(line.model)}</span>
        <span className={styles.source}>{sourceLabel(line.source)}</span>
      </div>
      {rows.map((r) =>
        r.tokens > 0 ? (
          <div key={r.label} className={styles.tok_row}>
            <span className={styles.tok_label}>{r.label}</span>
            <span className={styles.tok_count}>{fmtTok(r.tokens)}</span>
            <span className={styles.tok_price}>× {fmtPrice(r.price)}</span>
            <span className={styles.tok_sub}>
              {fmtUsd((r.tokens / 1_000_000) * r.price)}
            </span>
          </div>
        ) : null,
      )}
      <div className={styles.line_total}>
        <span>{t("token_receipt.subtotal", "小计")}</span>
        <span className={styles.line_total_value}>{fmtUsd(line.costUsd)}</span>
      </div>
    </div>
  );
}
