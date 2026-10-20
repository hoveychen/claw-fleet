// "Recover interrupted sub-tasks" on the task rail. After a network drop or an
// account error the main conversation is resumed by the usual mechanisms but
// its sub-tasks stay dead mid-turn; this lists the ones the core classifies as
// recoverable and wakes the ones the boss ticks. The core re-probes each id
// before resuming, so a stale list cannot wake a sub-task that already reported.

import { useCallback, useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { RotateCcw } from "lucide-react";
import {
  defaultRecoverySelection,
  recoverableCount,
  recoveryRows,
  selectedRecoveryIds,
  type ResumeOutcome,
  type SubtaskStatus,
} from "../../../shared-ts/subtaskRecovery";
import { Presence, useExiting } from "./Presence";
import { Spinner } from "./loading";
import styles from "./SubtaskRecovery.module.css";

/** How often the open task's sub-tasks are re-classified. A probe reads each
 *  sub-task's transcript tail, so this is deliberately slower than the rail's
 *  other refreshes. */
const REFRESH_MS = 30_000;

/** The coordinator's sub-tasks as the core classifies them; null until the
 *  first read lands or while `coordinatorId` is null. */
export function useSubtaskRecovery(coordinatorId: string | null) {
  const [list, setList] = useState<SubtaskStatus[] | null>(null);
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    setList(null);
    if (!coordinatorId) return;
    let alive = true;
    const load = () =>
      invoke<SubtaskStatus[]>("list_subtask_recovery", { sessionId: coordinatorId })
        .then((l) => alive && setList(l ?? []))
        .catch(() => alive && setList([]));
    void load();
    const timer = window.setInterval(load, REFRESH_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [coordinatorId, nonce]);
  const refresh = useCallback(() => setNonce((n) => n + 1), []);
  return { list, refresh };
}

/** The rail block: shown only while at least one sub-task is recoverable. */
export function SubtaskRecoveryBlock({
  coordinatorId,
  titleOf,
}: {
  coordinatorId: string | null;
  /** Display title for a sub-task session id; null when unknown here. */
  titleOf: (sessionId: string) => string | null;
}) {
  const { t } = useTranslation();
  const { list, refresh } = useSubtaskRecovery(coordinatorId);
  const [open, setOpen] = useState(false);
  const count = recoverableCount(list);
  if (!list || (count === 0 && !open)) return null;
  return (
    <section className={styles.block} data-testid="subtask-recovery">
      <p className={styles.note}>
        {t("home.recover_note", "{{count}} sub-tasks stopped mid-turn", { count })}
      </p>
      <button type="button" className={styles.trigger} onClick={() => setOpen(true)}>
        <RotateCcw size={13} strokeWidth={1.9} />
        {t("home.recover_button", "Recover interrupted sub-tasks")}
      </button>
      <Presence when={open}>
        {open && (
          <RecoverDialog
            list={list}
            titleOf={titleOf}
            onClose={() => {
              setOpen(false);
              refresh();
            }}
          />
        )}
      </Presence>
    </section>
  );
}

function shortId(id: string): string {
  return id.replace(/^session-/, "").slice(0, 8);
}

export function RecoverDialog({
  list,
  titleOf,
  onClose,
}: {
  list: SubtaskStatus[];
  titleOf: (sessionId: string) => string | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const exiting = useExiting();
  // Frozen at open: the 30s refresh must not reshuffle rows under the cursor.
  const [rows] = useState(() => recoveryRows(list));
  const [picked, setPicked] = useState(() => defaultRecoverySelection(list));
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<Map<string, ResumeOutcome> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const ids = useMemo(() => selectedRecoveryIds(rows, picked), [rows, picked]);

  const submit = useCallback(() => {
    if (busy || results || ids.length === 0) return;
    setBusy(true);
    setError(null);
    invoke<ResumeOutcome[]>("resume_subtasks", { sessionIds: ids })
      .then((out) => setResults(new Map((out ?? []).map((o) => [o.sessionId, o]))))
      .catch((e) => setError(String(e)))
      .finally(() => setBusy(false));
  }, [busy, results, ids]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (busy) return;
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onClose]);

  const toggle = (id: string) =>
    setPicked((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

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
        aria-busy={busy || undefined}
        onClick={(e) => e.stopPropagation()}
      >
        <h2 id="subtask-recover-title" className={styles.title}>
          {t("home.recover_title", "Recover interrupted sub-tasks")}
        </h2>
        <p className={styles.lead}>
          {results
            ? t("home.recover_lead_done", "Results:")
            : t(
                "home.recover_lead",
                "These sub-tasks stopped mid-turn. The ticked ones are resumed and told to continue from where they stopped.",
              )}
        </p>
        <ul className={styles.list}>
          {rows.map(({ status: s, selectable }) => {
            const res = results?.get(s.sessionId);
            const title = titleOf(s.sessionId) ?? titleOf(s.effectiveId) ?? shortId(s.sessionId);
            return (
              <li key={s.sessionId} className={`${styles.row} ${selectable ? "" : styles.row_disabled}`}>
                <label className={styles.row_label}>
                  <input
                    type="checkbox"
                    checked={selectable && picked.has(s.sessionId)}
                    disabled={!selectable || busy || !!results}
                    onChange={() => toggle(s.sessionId)}
                  />
                  <span className={styles.row_body}>
                    <span className={styles.row_title}>
                      {title}
                      {s.step && (
                        <span className={styles.step}>
                          {t("home.recover_step", "Step {{n}}", { n: s.step.replace(/^P/, "") })}
                        </span>
                      )}
                    </span>
                    <span className={styles.row_detail}>
                      {!selectable && `${t(`home.recover_class_${s.class}`, s.class)} · `}
                      {s.via &&
                        `${t("home.recover_via", "Spawned by \"{{title}}\"", {
                          title: titleOf(s.via) ?? shortId(s.via),
                        })} · `}
                      {s.detail}
                    </span>
                    {res && (
                      <span
                        className={`${styles.result} ${res.ok ? styles.result_ok : styles.result_err}`}
                        data-testid="recover-result"
                      >
                        {res.ok ? t("home.recover_ok", "Resumed") : res.message}
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
          <button className={styles.btn} onClick={onClose} disabled={busy}>
            {results ? t("home.recover_close", "Close") : t("cancel")}
          </button>
          {!results && (
            <button
              className={`${styles.btn} ${styles.btn_primary}`}
              onClick={submit}
              disabled={busy || ids.length === 0}
              autoFocus
            >
              {busy && <Spinner size={12} />}
              {t("home.recover_confirm", "Resume {{count}}", { count: ids.length })}
            </button>
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}
