import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import type { ModelReceiptLine, TodayUsageBreakdown } from "../types";
import styles from "./TokenReceiptModal.module.css";

interface Props {
  onClose: () => void;
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

/**
 * "Receipt" for today's cumulative token spend. Opened by clicking the sidebar
 * counter. Itemises the same figure per model — input / cache-write / cache-read
 * / output tokens, each priced at the model's official $/M rate — and totals it
 * back to the counter's value (agent + Fleet's own LLM spend).
 */
export function TokenReceiptModal({ onClose }: Props) {
  const { t } = useTranslation();
  const [data, setData] = useState<TodayUsageBreakdown | null>(null);
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
    invoke<TodayUsageBreakdown>("today_usage_breakdown")
      .then((r) => {
        if (!cancelled) setData(r);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const totalTokens = data
    ? data.totalInputTokens +
      data.totalCacheCreationTokens +
      data.totalCacheReadTokens +
      data.totalOutputTokens
    : 0;

  return (
    <div className={styles.overlay} onClick={onClose}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        <div className={styles.header}>
          <span className={styles.title}>
            {t("token_receipt.title", "今日花费明细")}
          </span>
          <button className={styles.close_btn} onClick={onClose} aria-label="close">
            ✕
          </button>
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
                  {t("token_receipt.subtitle", "Token 消费小票")} · {data.date}
                </div>
              </div>

              {data.lines.length === 0 && (
                <div className={styles.empty}>
                  {t("token_receipt.no_usage", "今天还没有用量")}
                </div>
              )}

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
                  <div className={styles.split_row}>
                    <span>
                      {t("token_receipt.split_agent", "会话")} {fmtUsd(data.agentCostUsd)}
                      {data.fleetCostUsd > 0 && (
                        <>
                          {" + "}
                          {t("token_receipt.split_fleet", "Fleet 自身")}{" "}
                          {fmtUsd(data.fleetCostUsd)}
                        </>
                      )}
                    </span>
                    <span className={styles.tok_total}>
                      {fmtTok(totalTokens)} {t("token_receipt.tokens", "tokens")}
                    </span>
                  </div>
                  <div className={styles.footer_note}>
                    {t(
                      "token_receipt.footnote",
                      "价格为各模型官方 $/M 单价 · 含缓存读写 · 与侧边栏计数同口径",
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

function ReceiptLine({ line }: { line: ModelReceiptLine }) {
  const { t } = useTranslation();
  const rows: { label: string; tokens: number; price: number }[] = [
    { label: t("token_receipt.row_input", "输入"), tokens: line.inputTokens, price: line.inputPrice },
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
