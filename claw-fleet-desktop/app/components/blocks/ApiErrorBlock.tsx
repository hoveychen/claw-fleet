import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  quotaResetsInMs,
  type ErrorAction,
  type SyntheticErrorInfo,
} from "../../../../shared-ts/syntheticError";
import { useBandOpen } from "./useBandOpen";
import styles from "./ApiErrorBlock.module.css";

/**
 * A turn that failed, rendered as something the user can act on.
 *
 * Claude Code writes these as ordinary assistant records, so before this card
 * they rendered as an assistant bubble: "Failed to authenticate: OAuth session
 * expired and could not be refreshed" arrived as one line of grey prose, in the
 * same shape the model uses to say anything else, with no hint that the fix is
 * one `claude auth login` away. The classification lives in
 * `shared-ts/syntheticError` so the phone and webui show the same card.
 *
 * `onAction` is what separates a card from a poster. Left undefined the card
 * still renders — it just shows no buttons, which is also the right thing for
 * an error with no honest move (`safeguards`).
 */
export function ApiErrorBlock({
  info,
  onAction,
  busy,
}: {
  info: SyntheticErrorInfo;
  /** Runs the chosen action. Omit to render the card without its action bar. */
  onAction?: (action: ErrorAction, info: SyntheticErrorInfo) => void;
  /** The action currently running, so its button can show it is working. */
  busy?: ErrorAction | null;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useBandOpen(false, false);
  const countdown = useQuotaCountdown(info);

  const title = t(info.titleKey, { defaultValue: TITLE_FALLBACK[info.titleKey] ?? "请求失败" });
  const actions = onAction ? info.actions : [];
  // While a quota window is still counting down, a retry is the one action
  // guaranteed to fail — it would spend a resume to earn the same error back.
  // Held shut rather than hidden, so the card still says retry is the move once
  // the clock runs out.
  const waiting = countdown != null && countdown !== "0";

  return (
    <div
      className={styles.root}
      data-severity={info.severity}
      data-code={info.code}
      data-testid="api-error-card"
    >
      <div className={styles.head}>
        <span className={styles.icon} aria-hidden>
          {SEVERITY_ICON[info.severity]}
        </span>
        <span className={styles.title}>{title}</span>
        {countdown && (
          <span className={styles.countdown} data-testid="api-error-countdown">
            {countdown === "0"
              ? t("detail.api_error.reset_now", "配额已恢复")
              : t("detail.api_error.resets_in", { defaultValue: "{{time}} 后恢复", time: countdown })}
          </span>
        )}
      </div>

      {/* Claude Code's own wording, never paraphrased: it is the only part of
          the card that knows which model / which limit / which host failed. */}
      <div className={styles.text}>{info.text}</div>

      {actions.length > 0 && (
        <div className={styles.actions}>
          {actions.map((a, i) => (
            <button
              key={a}
              type="button"
              className={i === 0 ? styles.primary : styles.secondary}
              disabled={busy != null || (waiting && a === "retry")}
              title={waiting && a === "retry" ? t("detail.api_error.retry_blocked", { defaultValue: "配额恢复后再重试" }) : undefined}
              onClick={() => onAction?.(a, info)}
              data-action={a}
            >
              {busy === a
                ? t("detail.api_error.working", "处理中…")
                : t(`detail.api_error.action.${a}`, { defaultValue: ACTION_FALLBACK[a] })}
            </button>
          ))}
        </div>
      )}

      {/* The raw record, for the cases where the card's reading is the thing in
          doubt — a code we have never seen, or a quota block that disagrees
          with the prose. */}
      <button className={styles.foldToggle} type="button" onClick={() => setOpen((v) => !v)}>
        {open ? "▾" : "▸"} {t("detail.api_error.raw", "原始记录")}
      </button>
      {open && (
        <pre className={styles.raw}>
          {JSON.stringify({ error: info.code, quotaLimits: info.quota, text: info.text }, null, 2)}
        </pre>
      )}
    </div>
  );
}

const SEVERITY_ICON: Record<SyntheticErrorInfo["severity"], string> = {
  fatal: "⚠",
  transient: "↻",
  wait: "◷",
};

const TITLE_FALLBACK: Record<string, string> = {
  "detail.api_error.auth": "登录已失效",
  "detail.api_error.rate_limit": "已达用量上限",
  "detail.api_error.model_quota": "该模型额度已用完",
  "detail.api_error.server": "连不上 API",
  "detail.api_error.too_long": "上下文超长",
  "detail.api_error.compaction_blocked": "压缩也被配额挡住了",
  "detail.api_error.billing": "余额不足",
  "detail.api_error.model_not_found": "模型不可用",
  "detail.api_error.safeguards": "被安全策略拦下",
  "detail.api_error.generic": "请求失败",
};

const ACTION_FALLBACK: Record<ErrorAction, string> = {
  retry: "重试本轮",
  login: "重新登录",
  switchModel: "换模型继续",
  compact: "压缩后继续",
  openUrl: "打开页面",
};

/**
 * The remaining quota window as `12:03` / `1:20:00`, or null when this error
 * carries no reset to count toward.
 *
 * Reads `quotaLimits.resetsAt` — a Unix timestamp Claude Code puts on the same
 * record — rather than the wall-clock string in the prose, which is rendered in
 * the *server's* notion of the user's zone and has to be reverse-engineered
 * back into an instant. Ticks once a second and only while a countdown is
 * actually on screen.
 */
function useQuotaCountdown(info: SyntheticErrorInfo): string | null {
  const [now, setNow] = useState(() => Date.now());
  const ms = quotaResetsInMs(info.quota, now);

  useEffect(() => {
    if (ms == null) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
    // Re-armed only when the record itself changes; `ms` is derived from `now`
    // and would restart the interval on every tick.
  }, [info.quota?.resetsAt, ms == null]);

  if (ms == null) return null;
  if (ms <= 0) return "0";
  const total = Math.ceil(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}
