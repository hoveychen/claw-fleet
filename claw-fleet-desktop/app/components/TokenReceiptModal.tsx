import { Fragment, useEffect, useMemo, useState } from "react";
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
interface UsageView {
  /** Window label: a single date, or `from → to` for a multi-day window. */
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

/** 1.23M / 45.6K / 780 — compact token counts. */
function fmtTok(n: number): string {
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(2)}B`;
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

/** Axis-tick money: no cents once the scale is in the tens. */
function fmtAxisUsd(n: number): string {
  if (n >= 1000) return `$${(n / 1000).toFixed(1)}k`;
  if (n >= 10) return `$${Math.round(n)}`;
  return `$${n.toFixed(2)}`;
}

/** Unit price is always $/M tokens. */
function fmtPrice(n: number): string {
  return `$${n.toFixed(2)}/M`;
}

function fmtPct(ratio: number): string {
  if (!Number.isFinite(ratio) || ratio <= 0) return "0%";
  if (ratio < 0.001) return "<0.1%";
  return `${(ratio * 100).toFixed(1)}%`;
}

/** `2026-09-29` → `09-29`, for axis ticks where the year is noise. */
function shortDate(date: string): string {
  return /^\d{4}-\d{2}-\d{2}$/.test(date) ? date.slice(5) : date;
}

/** Drop the `claude-` noise and any `<provider>/<org>/` prefix; leave gpt /
 * others as-is. A dsh route ships as e.g. `openrouter/anthropic/claude-opus-5`,
 * so this reduces it to `opus-5` for the model column. */
function prettyModel(model: string): string {
  if (!model) return "unknown";
  const base = model.includes("/") ? model.slice(model.lastIndexOf("/") + 1) : model;
  return base.replace(/^claude-/, "");
}

const SOURCE_LABEL: Record<string, string> = {
  "claude-code": "Claude",
  claude: "Claude",
  codex: "Codex",
  dsh: "DeepSeek Harness",
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

function normalizeToday(r: TodayUsageBreakdown): UsageView {
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

function normalizeRange(r: UsageRangeBreakdown): UsageView {
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

/** The four billed token kinds, in the order every section of the panel uses. */
type TokenKind = "input" | "cacheWrite" | "cacheRead" | "output";

const KIND_CLASS: Record<TokenKind, string> = {
  input: styles.k_input,
  cacheWrite: styles.k_cache_write,
  cacheRead: styles.k_cache_read,
  output: styles.k_output,
};

/** SVG fill counterparts of KIND_CLASS (which sets `background`). */
const KIND_FILL: Record<TokenKind, string> = {
  input: styles.f_input,
  cacheWrite: styles.f_cache_write,
  cacheRead: styles.f_cache_read,
  output: styles.f_output,
};

/**
 * Usage analytics for token spend. Opened by clicking the sidebar counter.
 * KPI strip, per-day trend (multi-day ranges), token-mix bar, and a per-model
 * table whose rows expand into the $/M itemisation. The default "today" view
 * reconciles to the sidebar counter; the longer ranges (7d / 30d / all) come
 * from the range breakdown and additionally carry the daily series.
 */
export function TokenReceiptModal({ onClose }: Props) {
  const { t } = useTranslation();
  const [range, setRange] = useState<RangeKey>("today");
  const [data, setData] = useState<UsageView | null>(null);
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
          <div className={styles.title_block}>
            <span className={styles.title}>{t("token_receipt.title", "Fleet 用量分析")}</span>
            {data && <span className={styles.window_label}>{data.label}</span>}
          </div>
          <div className={styles.range_bar} role="tablist">
            {ranges.map((r) => (
              <button
                key={r.key}
                role="tab"
                aria-selected={range === r.key}
                className={`${styles.range_btn} ${range === r.key ? styles.range_btn_active : ""}`}
                onClick={() => setRange(r.key)}
              >
                {r.label}
              </button>
            ))}
          </div>
          <button className={styles.close_btn} onClick={onClose} aria-label={t("common.close", "关闭")}>
            ✕
          </button>
        </div>

        <div className={styles.body}>
          {error && <div className={styles.empty}>{error}</div>}
          {!error && !data && (
            <div className={styles.empty}>{t("token_receipt.loading", "统计中…")}</div>
          )}
          {data && data.lines.length === 0 && (
            <div className={styles.empty}>
              {t("token_receipt.no_usage_range", "此区间还没有用量")}
            </div>
          )}
          {data && data.lines.length > 0 && <UsageBody data={data} />}
        </div>
      </div>
    </div>
  );
}

function UsageBody({ data }: { data: UsageView }) {
  const { t } = useTranslation();
  const totalTokens =
    data.totalInputTokens +
    data.totalCacheCreationTokens +
    data.totalCacheReadTokens +
    data.totalOutputTokens;
  // Share of prompt-side tokens served from cache — the lever that dominates
  // agent spend, so it earns a KPI of its own.
  const promptTokens =
    data.totalInputTokens + data.totalCacheCreationTokens + data.totalCacheReadTokens;
  const cacheHit = promptTokens > 0 ? data.totalCacheReadTokens / promptTokens : 0;
  const days = data.daily.length;
  const lines = useMemo(
    () => [...data.lines].sort((a, b) => b.costUsd - a.costUsd),
    [data.lines],
  );

  return (
    <>
      <div className={styles.kpis}>
        <Kpi label={t("token_receipt.kpi_cost", "花费")} value={fmtUsd(data.totalCostUsd)} />
        <Kpi label={t("token_receipt.kpi_tokens", "Tokens")} value={fmtTok(totalTokens)} />
        <Kpi
          label={t("token_receipt.kpi_cache_hit", "缓存命中率")}
          value={fmtPct(cacheHit)}
          sub={t("token_receipt.kpi_cache_hit_sub", "缓存读取 / 全部输入")}
        />
        {days > 1 ? (
          <Kpi
            label={t("token_receipt.kpi_daily_avg", "日均花费")}
            value={fmtUsd(data.totalCostUsd / days)}
            sub={t("token_receipt.kpi_days", "{{count}} 天", { count: days })}
          />
        ) : (
          <Kpi
            label={t("token_receipt.kpi_output", "输出 tokens")}
            value={fmtTok(data.totalOutputTokens)}
          />
        )}
      </div>

      {days > 0 && <TrendChart daily={data.daily} />}

      <TokenMix
        values={{
          input: data.totalInputTokens,
          cacheWrite: data.totalCacheCreationTokens,
          cacheRead: data.totalCacheReadTokens,
          output: data.totalOutputTokens,
        }}
      />

      <ModelTable lines={lines} totalCost={data.totalCostUsd} />

      {data.hasCodexApproximation && (
        <div className={styles.note_warn}>
          {t(
            "token_receipt.codex_note",
            "该 Codex 会话有未带时间戳的轮次,这部分归到了会话起始日,趋势为近似",
          )}
        </div>
      )}
      {/* Agent spend only — Fleet's own guard / report LLM calls live in
          Settings → Usage, not here. */}
      <div className={styles.footnote}>
        {t(
          "token_receipt.footnote",
          "仅统计 Fleet 启动的会话 · 价格为各模型官方 $/M 单价 · 含缓存读写 · 今日与侧边栏计数同口径",
        )}
      </div>
    </>
  );
}

function Kpi({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className={styles.kpi}>
      <div className={styles.kpi_label}>{label}</div>
      <div className={styles.kpi_value}>{value}</div>
      {sub && <div className={styles.kpi_sub}>{sub}</div>}
    </div>
  );
}

function SectionHead({ title, aside }: { title: string; aside?: string }) {
  return (
    <div className={styles.section_head}>
      <span className={styles.section_title}>{title}</span>
      {aside && <span className={styles.section_aside}>{aside}</span>}
    </div>
  );
}

type TrendMetric = "cost" | "tokens";

/** Bottom-to-top stacking order of a tokens-mode bar. */
const STACK: { kind: TokenKind; tokens: (d: DailyUsagePoint) => number }[] = [
  { kind: "cacheRead", tokens: (d) => d.cacheReadTokens },
  { kind: "cacheWrite", tokens: (d) => d.cacheCreationTokens },
  { kind: "input", tokens: (d) => d.inputTokens },
  { kind: "output", tokens: (d) => d.outputTokens },
];

function dayTokens(d: DailyUsagePoint): number {
  return d.inputTokens + d.cacheCreationTokens + d.cacheReadTokens + d.outputTokens;
}

/**
 * Per-day trend with a labelled y scale. "cost" draws one bar of spend per
 * day; "tokens" stacks the four token kinds. The daily series carries no
 * per-kind or per-model money, so a stacked view can only be in tokens.
 */
function TrendChart({ daily }: { daily: DailyUsagePoint[] }) {
  const { t } = useTranslation();
  const [metric, setMetric] = useState<TrendMetric>("cost");
  const [hover, setHover] = useState<number | null>(null);
  const value = metric === "cost" ? (d: DailyUsagePoint) => d.costUsd : dayTokens;
  const fmtAxis = metric === "cost" ? fmtAxisUsd : fmtTok;
  const peak = Math.max(...daily.map(value), 0);
  // Linear scale from zero so bar-height ratios equal the value ratios; the
  // top gridline is the peak day, the middle one its half.
  const max = Math.max(peak, metric === "cost" ? 0.0001 : 1);
  const W = 720;
  const H = 120;
  const n = daily.length;
  const gap = n > 1 ? (n > 60 ? 1 : 2) : 0;
  const barW = Math.max(1, (W - gap * (n - 1)) / n);
  const focus = hover !== null ? daily[hover] : null;
  const mid = Math.floor((n - 1) / 2);

  const kindLabel: Record<TokenKind, string> = {
    input: t("token_receipt.row_input", "输入"),
    cacheWrite: t("token_receipt.row_cache_write", "缓存写入"),
    cacheRead: t("token_receipt.row_cache_read", "缓存读取"),
    output: t("token_receipt.row_output", "输出"),
  };

  let readout: string;
  if (!focus) {
    readout = t("token_receipt.trend_peak", "峰值 {{value}}", {
      value: metric === "cost" ? fmtUsd(peak) : `${fmtTok(peak)} tok`,
    });
  } else if (metric === "cost") {
    readout = `${focus.date} · ${fmtUsd(focus.costUsd)} · ${fmtTok(dayTokens(focus))} tok`;
  } else {
    readout = [
      focus.date,
      ...[...STACK].reverse().map((s) => `${kindLabel[s.kind]} ${fmtTok(s.tokens(focus))}`),
    ].join(" · ");
  }

  const metrics: { key: TrendMetric; label: string }[] = [
    { key: "cost", label: t("token_receipt.trend_metric_cost", "花费") },
    { key: "tokens", label: t("token_receipt.trend_metric_tokens", "Tokens") },
  ];

  return (
    <section className={styles.section}>
      <div className={styles.section_head}>
        <span className={styles.section_title}>
          {t("token_receipt.trend_title", "每日趋势")}
        </span>
        <span className={styles.trend_tools}>
          <span className={styles.section_aside}>{readout}</span>
          <span className={styles.metric_bar} role="tablist">
            {metrics.map((m) => (
              <button
                key={m.key}
                role="tab"
                aria-selected={metric === m.key}
                className={`${styles.metric_btn} ${metric === m.key ? styles.metric_btn_active : ""}`}
                onClick={() => setMetric(m.key)}
              >
                {m.label}
              </button>
            ))}
          </span>
        </span>
      </div>
      <div className={styles.chart}>
        <div className={styles.y_axis}>
          <span>{fmtAxis(max)}</span>
          <span>{fmtAxis(max / 2)}</span>
          <span>{metric === "cost" ? "$0" : "0"}</span>
        </div>
        <div className={styles.plot}>
          <svg
            viewBox={`0 0 ${W} ${H}`}
            className={styles.chart_svg}
            preserveAspectRatio="none"
            onMouseLeave={() => setHover(null)}
          >
            {[0, H / 2, H].map((y) => (
              <line key={y} x1={0} x2={W} y1={y} y2={y} className={styles.grid_line} />
            ))}
            {daily.map((d, i) => {
              const x = i * (barW + gap);
              const active = hover === i ? styles.bar_active : "";
              let bars;
              if (metric === "cost") {
                const h = Math.max(1, (Math.max(d.costUsd, 0) / max) * H);
                bars = (
                  <rect x={x} y={H - h} width={barW} height={h} className={`${styles.bar} ${active}`} />
                );
              } else {
                let top = H;
                bars = STACK.map((s) => {
                  const h = (Math.max(s.tokens(d), 0) / max) * H;
                  if (h <= 0) return null;
                  top -= h;
                  return (
                    <rect
                      key={s.kind}
                      x={x}
                      y={top}
                      width={barW}
                      height={h}
                      className={`${styles.stack_seg} ${KIND_FILL[s.kind]} ${active}`}
                    />
                  );
                });
              }
              return (
                <g key={d.date} onMouseEnter={() => setHover(i)}>
                  {/* Full-height hit target so thin bars are still easy to hover. */}
                  <rect x={x} y={0} width={barW + gap} height={H} fill="transparent" />
                  {bars}
                </g>
              );
            })}
          </svg>
          <div className={styles.x_axis}>
            <span>{shortDate(daily[0].date)}</span>
            {n > 2 && <span>{shortDate(daily[mid].date)}</span>}
            {n > 1 && <span>{shortDate(daily[n - 1].date)}</span>}
          </div>
        </div>
      </div>
    </section>
  );
}

/** 100%-stacked bar of the four token kinds, with a legend carrying the counts. */
function TokenMix({ values }: { values: Record<TokenKind, number> }) {
  const { t } = useTranslation();
  const total = values.input + values.cacheWrite + values.cacheRead + values.output;
  const kinds: { kind: TokenKind; label: string }[] = [
    { kind: "input", label: t("token_receipt.row_input", "输入") },
    { kind: "cacheWrite", label: t("token_receipt.row_cache_write", "缓存写入") },
    { kind: "cacheRead", label: t("token_receipt.row_cache_read", "缓存读取") },
    { kind: "output", label: t("token_receipt.row_output", "输出") },
  ];

  return (
    <section className={styles.section}>
      <SectionHead
        title={t("token_receipt.mix_title", "Token 构成")}
        aside={`${fmtTok(total)} ${t("token_receipt.tokens", "tokens")}`}
      />
      <div className={styles.mix_bar}>
        {kinds.map(({ kind }) =>
          values[kind] > 0 ? (
            <span
              key={kind}
              className={`${styles.mix_seg} ${KIND_CLASS[kind]}`}
              style={{ flexGrow: values[kind] }}
            />
          ) : null,
        )}
      </div>
      <div className={styles.mix_legend}>
        {kinds.map(({ kind, label }) => (
          <div key={kind} className={styles.mix_item}>
            <span className={`${styles.swatch} ${KIND_CLASS[kind]}`} />
            <span className={styles.mix_label}>{label}</span>
            <span className={styles.mix_value}>{fmtTok(values[kind])}</span>
            <span className={styles.mix_pct}>{fmtPct(total > 0 ? values[kind] / total : 0)}</span>
          </div>
        ))}
      </div>
    </section>
  );
}

function ModelTable({ lines, totalCost }: { lines: ModelReceiptLine[]; totalCost: number }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState<string | null>(null);

  return (
    <section className={styles.section}>
      <SectionHead
        title={t("token_receipt.models_title", "按模型")}
        aside={t("token_receipt.models_hint", "点击行查看单价明细")}
      />
      <table className={styles.table}>
        <thead>
          <tr>
            <th className={styles.col_model}>{t("token_receipt.col_model", "模型")}</th>
            <th className={styles.num}>{t("token_receipt.row_input", "输入")}</th>
            <th className={styles.num}>{t("token_receipt.row_cache_write", "缓存写入")}</th>
            <th className={styles.num}>{t("token_receipt.row_cache_read", "缓存读取")}</th>
            <th className={styles.num}>{t("token_receipt.row_output", "输出")}</th>
            <th className={styles.num}>{t("token_receipt.col_cost", "花费")}</th>
            <th className={styles.col_share}>{t("token_receipt.col_share", "占比")}</th>
          </tr>
        </thead>
        <tbody>
          {lines.map((line, i) => {
            const key = `${line.source}:${line.model}:${i}`;
            const expanded = open === key;
            const share = totalCost > 0 ? line.costUsd / totalCost : 0;
            const flagged = line.pricedByProvider || line.unpricedCalls > 0;
            return (
              <Fragment key={key}>
                <tr
                  className={`${styles.row} ${expanded ? styles.row_open : ""}`}
                  onClick={() => setOpen(expanded ? null : key)}
                >
                  <td className={styles.col_model}>
                    <span className={styles.caret}>{expanded ? "▾" : "▸"}</span>
                    <span className={styles.model}>{prettyModel(line.model)}</span>
                    <span className={styles.source}>{sourceLabel(line.source)}</span>
                    {flagged && <span className={styles.flag}>*</span>}
                  </td>
                  <td className={styles.num}>{fmtTok(line.inputTokens)}</td>
                  <td className={styles.num}>
                    {fmtTok(line.cacheCreationTokens + line.cacheCreation1hTokens)}
                  </td>
                  <td className={styles.num}>{fmtTok(line.cacheReadTokens)}</td>
                  <td className={styles.num}>{fmtTok(line.outputTokens)}</td>
                  <td className={`${styles.num} ${styles.cost}`}>{fmtUsd(line.costUsd)}</td>
                  <td className={styles.col_share}>
                    <span className={styles.share_track}>
                      <span className={styles.share_fill} style={{ width: `${share * 100}%` }} />
                    </span>
                    <span className={styles.share_pct}>{fmtPct(share)}</span>
                  </td>
                </tr>
                {expanded && (
                  <tr className={styles.detail_row}>
                    <td colSpan={7}>
                      <LineDetail line={line} />
                    </td>
                  </tr>
                )}
              </Fragment>
            );
          })}
        </tbody>
        <tfoot>
          <tr>
            <td className={styles.col_model}>{t("token_receipt.grand_total", "合计")}</td>
            <td className={styles.num}>{fmtTok(sum(lines, (l) => l.inputTokens))}</td>
            <td className={styles.num}>
              {fmtTok(sum(lines, (l) => l.cacheCreationTokens + l.cacheCreation1hTokens))}
            </td>
            <td className={styles.num}>{fmtTok(sum(lines, (l) => l.cacheReadTokens))}</td>
            <td className={styles.num}>{fmtTok(sum(lines, (l) => l.outputTokens))}</td>
            <td className={`${styles.num} ${styles.cost}`}>{fmtUsd(totalCost)}</td>
            <td className={styles.col_share} />
          </tr>
        </tfoot>
      </table>
    </section>
  );
}

function sum(lines: ModelReceiptLine[], f: (l: ModelReceiptLine) => number): number {
  return lines.reduce((acc, l) => acc + f(l), 0);
}

/** The $/M itemisation behind one model row: tokens × unit price = subtotal. */
function LineDetail({ line }: { line: ModelReceiptLine }) {
  const { t } = useTranslation();
  // Provider-priced (dsh): the spend is the provider's own charge for an open
  // model space, which Fleet's reference $/M table cannot reproduce — so the
  // per-row "× $X/M" column is deliberately withheld and only the tokens + real
  // subtotal are shown. For every other source the rows reconcile to the
  // subtotal to the cent.
  const priced = line.pricedByProvider;
  // Cache writes are billed by TTL — a 1-hour write costs 2× the model's input
  // rate, a 5-minute write 1.25× — so they get one row each. Blending them into
  // a single row would leave `Σ rows ≠ subtotal`.
  const rows: { label: string; tokens: number; price: number }[] = [
    { label: t("token_receipt.row_input", "输入"), tokens: line.inputTokens, price: line.inputPrice },
    {
      label: t("token_receipt.row_cache_write_1h", "缓存写入 1h"),
      tokens: line.cacheCreation1hTokens,
      price: line.cacheWrite1hPrice,
    },
    {
      label: t("token_receipt.row_cache_write_5m", "缓存写入 5m"),
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
    <div className={styles.detail}>
      <div className={styles.detail_model}>{line.model || "unknown"}</div>
      <div className={styles.detail_grid}>
        {rows.map((r) =>
          r.tokens > 0 ? (
            <Fragment key={r.label}>
              <span className={styles.detail_label}>{r.label}</span>
              <span className={styles.num}>{fmtTok(r.tokens)}</span>
              <span className={`${styles.num} ${styles.dim}`}>
                {priced ? "—" : `× ${fmtPrice(r.price)}`}
              </span>
              <span className={styles.num}>
                {priced ? "" : fmtUsd((r.tokens / 1_000_000) * r.price)}
              </span>
            </Fragment>
          ) : null,
        )}
        <span className={styles.detail_total_label}>{t("token_receipt.subtotal", "小计")}</span>
        <span />
        <span />
        <span className={`${styles.num} ${styles.detail_total}`}>{fmtUsd(line.costUsd)}</span>
      </div>
      {priced && (
        <div className={styles.note_warn}>
          *{" "}
          {t(
            "token_receipt.provider_priced",
            "该行消费按 provider 发票或官方标价折算,非 Fleet 的 $/M 参考价,故不逐行计价。实际扣费以账户币种为准",
          )}
        </div>
      )}
      {line.unpricedCalls > 0 && (
        // Tokens without money. A fresh OpenRouter generation 404s for some
        // minutes, and a route with no published rate never prices at all —
        // both would otherwise read as "this part was free".
        <div className={styles.note_warn}>
          *{" "}
          {t("token_receipt.unpriced_calls", "另有 {{count}} 次调用暂无法定价,其 token 已计入、金额未计入", {
            count: line.unpricedCalls,
          })}
        </div>
      )}
    </div>
  );
}
