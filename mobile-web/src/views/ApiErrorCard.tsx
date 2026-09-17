import { useEffect, useState } from "react";

import {
  quotaResetsInMs,
  type ErrorAction,
  type SyntheticErrorInfo,
} from "../../../shared-ts/syntheticError";
import { t } from "../i18n";
import type { FleetTransport } from "../relay";
import { modelChoicesFor, useModelCatalog } from "../useModelCatalog";
import {
  resumeOverRelay,
  runApiErrorAction,
  type ActionOutcome,
  type ApiErrorSession,
} from "./apiErrorActions";
import styles from "./SessionDetailView.module.css";

/**
 * A turn Claude Code failed out of, with its way out attached.
 *
 * The classification is `shared-ts/syntheticError`, the same module the desktop
 * renders from, so a rate limit reads as a countdown and an expired token reads
 * as "sign in again" on both. What differs is only the transport: the desktop
 * invokes Tauri commands, this goes through the relay (`resume_session`,
 * `proc_run`) — which is why the card is written twice instead of shared as a
 * component.
 *
 * `login` opens `claude auth login` in a pty. The phone genuinely can drive it:
 * `proc_run` is the same proc runner the Terminal page uses, and the OAuth handshake
 * is a URL plus a pasted code. The terminal itself is left to that page — this
 * card starts the shell and says where to finish it, rather than embedding an
 * xterm inside a transcript row on a phone screen.
 */
export function ApiErrorCard({
  info,
  session,
  client,
}: {
  info: SyntheticErrorInfo;
  session?: ApiErrorSession | null;
  client?: FleetTransport | null;
}) {
  const [busy, setBusy] = useState<ErrorAction | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [picking, setPicking] = useState(false);
  const catalog = useModelCatalog(client ?? null);
  const countdown = useCountdown(info);

  const canAct = !!session && !!client;
  // The mobile helper leads with a `["", <default>]` entry for a picker whose
  // "no choice" state is meaningful. Here it is not: the session already HAS a
  // model and it is the one that ran out, so only the real alternatives.
  const models = modelChoicesFor(
    catalog,
    session?.agentSource === "codex" ? "codex" : "claude",
    "",
  ).filter(([value]) => value !== "");

  function report(outcome: ActionOutcome) {
    if (!outcome) return;
    setNote(
      outcome.kind === "resumed"
        ? t("已重新拉起，等会话接上…")
        : outcome.kind === "loginStarted"
          ? t("登录已在终端页启动，去那边粘贴授权码")
          : t("没能执行：") + outcome.detail,
    );
  }

  async function run(action: ErrorAction) {
    if (!session || !client) return;
    if (action === "switchModel") {
      setPicking((v) => !v);
      return;
    }
    setBusy(action);
    setNote(null);
    try {
      report(await runApiErrorAction(action, { info, session, client }));
    } finally {
      setBusy(null);
    }
  }

  async function pickModel(model: string) {
    if (!session || !client) return;
    setPicking(false);
    setBusy("switchModel");
    setNote(null);
    try {
      report(await resumeOverRelay({ info, session, client }, { model }));
    } finally {
      setBusy(null);
    }
  }

  // Never draw a button with nothing behind it — offline, or a transcript with
  // no live session, means the classification renders alone.
  const actions = canAct ? info.actions : [];

  return (
    <div className={styles.apiErrorCard} data-severity={info.severity} data-testid="api-error-card">
      <div className={styles.apiErrorHead}>
        <span className={styles.apiErrorIcon} aria-hidden>
          {info.severity === "wait" ? "◷" : info.severity === "transient" ? "↻" : "⚠"}
        </span>
        <span className={styles.apiErrorTitle}>{titleOf(info)}</span>
        {countdown && (
          <span className={styles.apiErrorCountdown} data-testid="api-error-countdown">
            {countdown === "0" ? t("配额已恢复") : countdown}
          </span>
        )}
      </div>
      {/* Claude Code's own wording — the only part that names which model, which
          limit or which host actually failed. */}
      <div className={styles.apiErrorText}>{info.text}</div>
      {actions.length > 0 && (
        <div className={styles.apiErrorActions}>
          {actions.map((a, i) => (
            <button
              key={a}
              type="button"
              className={i === 0 ? styles.apiErrorPrimary : styles.apiErrorSecondary}
              data-action={a}
              disabled={busy != null}
              onClick={() => void run(a)}
            >
              {busy === a ? t("处理中…") : actionLabel(a)}
            </button>
          ))}
        </div>
      )}
      {picking && (
        <div className={styles.apiErrorPicker} data-testid="api-error-model-picker">
          {models.length === 0 ? (
            <span className={styles.apiErrorNote}>{t("模型列表加载中…")}</span>
          ) : (
            models.map(([value, label]) => (
              <button
                key={value}
                type="button"
                className={styles.apiErrorSecondary}
                data-model={value}
                disabled={busy != null}
                onClick={() => void pickModel(value)}
              >
                {label}
              </button>
            ))
          )}
        </div>
      )}
      {note && (
        <div className={styles.apiErrorNote} data-testid="api-error-note">
          {note}
        </div>
      )}
    </div>
  );
}

function titleOf(info: SyntheticErrorInfo): string {
  switch (info.titleKey) {
    case "detail.api_error.auth":
      return t("登录已失效");
    case "detail.api_error.rate_limit":
      return t("已达用量上限");
    case "detail.api_error.model_quota":
      return t("该模型额度已用完");
    case "detail.api_error.server":
      return t("连不上 API");
    case "detail.api_error.too_long":
      return t("上下文超长");
    case "detail.api_error.compaction_blocked":
      return t("压缩也被配额挡住了");
    case "detail.api_error.billing":
      return t("余额不足");
    case "detail.api_error.model_not_found":
      return t("模型不可用");
    case "detail.api_error.safeguards":
      return t("被安全策略拦下");
    default:
      return t("请求失败");
  }
}

function actionLabel(a: ErrorAction): string {
  switch (a) {
    case "retry":
      return t("重试本轮");
    case "login":
      return t("重新登录");
    case "switchModel":
      return t("换模型继续");
    case "compact":
      return t("压缩后继续");
    case "openUrl":
      return t("打开页面");
  }
}

/** `mm:ss` / `h:mm:ss` until the quota window reopens, from the record's own
 *  `quotaLimits.resetsAt` rather than the wall-clock string in the prose. */
function useCountdown(info: SyntheticErrorInfo): string | null {
  const [now, setNow] = useState(() => Date.now());
  const ms = quotaResetsInMs(info.quota, now);
  const ticking = ms != null;
  useEffect(() => {
    if (!ticking) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [ticking, info.quota?.resetsAt]);
  if (ms == null) return null;
  if (ms <= 0) return "0";
  const total = Math.ceil(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}
