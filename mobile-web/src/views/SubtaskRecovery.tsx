// "恢复中断的子任务" on the task page's 子任务 pane. Mirrors the desktop rail's
// block (claw-fleet-desktop/app/components/SubtaskRecovery.tsx): the core
// classifies each sub-task, this lists the recoverable ones and wakes the ones
// the boss ticks. The core re-probes each id before resuming.

import { useCallback, useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { RotateCcw } from "lucide-react";
import { t } from "../i18n";
import { Presence, useExiting } from "../Presence";
import { randomId } from "../clientId";
import type { FleetTransport } from "../transport";
import {
  defaultRecoverySelection,
  recoverableCount,
  recoveryRows,
  selectedRecoveryIds,
  type RecoveryClass,
  type ResumeOutcome,
  type SubtaskStatus,
} from "../../../shared-ts/subtaskRecovery";
import styles from "./SubtaskRecovery.module.css";

/** Probing reads every sub-task's transcript tail; keep it slow. */
const REFRESH_MS = 30_000;

const CLASS_LABEL: Partial<Record<RecoveryClass, string>> = {
  authFailed: "账号/鉴权失败，恢复也会再失败",
  userInterrupted: "被手动中断",
  failed: "永久错误",
};

export function useSubtaskRecovery(client: FleetTransport | null, coordinatorId: string | null) {
  const [list, setList] = useState<SubtaskStatus[] | null>(null);
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    setList(null);
    if (!client || !coordinatorId) return;
    let alive = true;
    const load = () =>
      client
        .request<SubtaskStatus[]>("subtasks_recovery", { sessionId: coordinatorId })
        .then((l) => alive && setList(l ?? []))
        .catch(() => alive && setList([]));
    void load();
    const timer = window.setInterval(load, REFRESH_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [client, coordinatorId, nonce]);
  const refresh = useCallback(() => setNonce((n) => n + 1), []);
  return { list, refresh };
}

function shortId(id: string): string {
  return id.replace(/^session-/, "").slice(0, 8);
}

/** Banner + button at the top of the 子任务 pane; renders nothing unless at
 *  least one sub-task is recoverable. */
export function SubtaskRecoveryBanner({
  client,
  coordinatorId,
  titleOf,
}: {
  client: FleetTransport | null;
  coordinatorId: string | null;
  titleOf: (sessionId: string) => string | null;
}) {
  const { list, refresh } = useSubtaskRecovery(client, coordinatorId);
  const [open, setOpen] = useState(false);
  const count = recoverableCount(list);
  if (!client || !list || (count === 0 && !open)) return null;
  return (
    <div className={styles.banner} data-testid="subtask-recovery">
      <span className={styles.note}>{t("{0} 个子任务在回合中途断了", count)}</span>
      <button type="button" className={styles.trigger} onClick={() => setOpen(true)}>
        <RotateCcw size={14} />
        {t("恢复中断的子任务")}
      </button>
      <Presence when={open}>
        {open && (
          <RecoverSheet
            client={client}
            list={list}
            titleOf={titleOf}
            onClose={() => {
              setOpen(false);
              refresh();
            }}
          />
        )}
      </Presence>
    </div>
  );
}

function RecoverSheet({
  client,
  list,
  titleOf,
  onClose,
}: {
  client: FleetTransport;
  list: SubtaskStatus[];
  titleOf: (sessionId: string) => string | null;
  onClose: () => void;
}) {
  // Frozen at open so the periodic refresh does not reshuffle rows.
  const [rows] = useState(() => recoveryRows(list));
  const [picked, setPicked] = useState(() => defaultRecoverySelection(list));
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<Map<string, ResumeOutcome> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const ids = useMemo(() => selectedRecoveryIds(rows, picked), [rows, picked]);
  const exiting = useExiting();

  const submit = () => {
    if (busy || results || ids.length === 0) return;
    setBusy(true);
    setError(null);
    client
      // A fresh key per submit: a resend of a lost reply replays the first
      // outcomes instead of reporting every id as refused (now running).
      .request<ResumeOutcome[]>("subtasks_resume", { sessionIds: ids, idempotencyKey: randomId() })
      .then((out) => setResults(new Map((out ?? []).map((o) => [o.sessionId, o]))))
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setBusy(false));
  };

  const toggle = (id: string) =>
    setPicked((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  // Portalled: the pane's tab page animates, and a transformed ancestor would
  // turn `position: fixed` into "fixed to the pane".
  return createPortal(
    <div
      className={styles.overlay}
      data-exiting={exiting || undefined}
      onClick={busy ? undefined : onClose}
    >
      <div
        className={styles.dialog}
        role="dialog"
        aria-modal="true"
        aria-labelledby="subtask-recover-title"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 id="subtask-recover-title" className={styles.title}>
          {t("恢复中断的子任务")}
        </h2>
        <p className={styles.lead}>
          {results ? t("恢复结果：") : t("这些子任务在回合中途断了。勾选的会被唤醒，从中断处接着做。")}
        </p>
        <ul className={styles.list}>
          {rows.map(({ status: s, selectable }) => {
            const res = results?.get(s.sessionId);
            const title = titleOf(s.sessionId) ?? titleOf(s.effectiveId) ?? shortId(s.sessionId);
            const label = CLASS_LABEL[s.class];
            return (
              <li key={s.sessionId} className={styles.row} data-disabled={!selectable || undefined}>
                <label className={styles.rowLabel}>
                  <input
                    type="checkbox"
                    checked={selectable && picked.has(s.sessionId)}
                    disabled={!selectable || busy || !!results}
                    onChange={() => toggle(s.sessionId)}
                  />
                  <span className={styles.rowBody}>
                    <span className={styles.rowTitle}>
                      {title}
                      {s.step && <span className={styles.step}>{t("第 {0} 步", s.step.replace(/^P/, ""))}</span>}
                    </span>
                    <span className={styles.rowDetail}>
                      {!selectable && label && `${t(label)} · `}
                      {s.via && `${t("由「{0}」派出", titleOf(s.via) ?? shortId(s.via))} · `}
                      {s.detail}
                    </span>
                    {res && (
                      <span className={styles.result} data-ok={res.ok} data-testid="recover-result">
                        {res.ok ? t("已唤醒") : res.message}
                      </span>
                    )}
                  </span>
                </label>
              </li>
            );
          })}
        </ul>
        {error && <p className={styles.error}>{error}</p>}
        <div className={styles.actions}>
          <button type="button" className={styles.cancel} onClick={onClose} disabled={busy}>
            {results ? t("关闭") : t("取消")}
          </button>
          {!results && (
            <button
              type="button"
              className={styles.confirm}
              onClick={submit}
              disabled={busy || ids.length === 0}
            >
              {busy ? t("唤醒中…") : t("唤醒 {0} 个", ids.length)}
            </button>
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}
