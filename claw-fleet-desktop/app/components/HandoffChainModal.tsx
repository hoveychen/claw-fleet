import { useEffect } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { useSessionsStore } from "../store";
import type { HandoffChain } from "../types";
import styles from "./HandoffChainModal.module.css";

interface Props {
  chain: HandoffChain | null;
  loading: boolean;
  /** The session whose detail opened this modal — its leg is highlighted. */
  currentSessionId: string;
  hop: number;
  len: number;
  onClose: () => void;
  /** Make each leg clickable, handing back that leg's session id. Omitted by
   *  the session detail (you are already inside one of these sessions); the
   *  计划树 passes it so a relay leg jumps straight into that session. */
  onOpenSession?: (sessionId: string) => void;
}

/**
 * Full relay chain in a scrollable centred modal — every leg's session, the
 * relay note it left, and when the baton passed. Split out of the chip so a
 * long chain scrolls inside its own overlay instead of overflowing (and being
 * clipped by) the non-scrolling session-detail header.
 *
 * Rendered through a portal to document.body so the fixed overlay covers the
 * whole viewport regardless of the clipping/transform of its DOM ancestors.
 */
export function HandoffChainModal({
  chain,
  loading,
  currentSessionId,
  hop,
  len,
  onClose,
  onOpenSession,
}: Props) {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);

  // Close on Escape.
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onClose]);

  // Prefer the live session's AI title; fall back to a short id for legs whose
  // transcript is gone or lives on another machine.
  const labelOf = (sid: string) => {
    const s = sessions.find((x) => x.id === sid);
    return s?.aiTitle || `${sid.slice(0, 8)}…`;
  };

  // Ordered session ids on the chain, derived from the links.
  const legIds: string[] = [];
  for (const l of chain?.links ?? []) {
    if (legIds[legIds.length - 1] !== l.fromSessionId) legIds.push(l.fromSessionId);
    legIds.push(l.toSessionId);
  }

  // The backdrop sits inside a clickable session card; stop the close click
  // from bubbling up and selecting the card underneath.
  const close = (e: React.SyntheticEvent) => {
    e.stopPropagation();
    onClose();
  };

  return createPortal(
    <div className={styles.overlay} onClick={close}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        <div className={styles.header}>
          <span className={styles.title}>
            🔗 {t("card.handoff_chip", { hop, len })}
          </span>
          <button className={styles.close_btn} onClick={close}>
            ✕
          </button>
        </div>
        <div className={styles.scroll_area}>
          {loading && <div className={styles.handoff_note}>{t("card.handoff_loading")}</div>}
          {!loading && chain && legIds.map((sid, i) => (
            <div key={sid}>
              <div
                className={`${styles.handoff_leg} ${sid === currentSessionId ? styles.handoff_current : ""} ${onOpenSession ? styles.handoff_leg_clickable : ""}`}
                role={onOpenSession ? "button" : undefined}
                tabIndex={onOpenSession ? 0 : undefined}
                onClick={onOpenSession ? () => onOpenSession(sid) : undefined}
                onKeyDown={
                  onOpenSession
                    ? (e) => {
                        if (e.key === "Enter" || e.key === " ") onOpenSession(sid);
                      }
                    : undefined
                }
              >
                <span className={styles.handoff_leg_no}>{t("card.handoff_hop", { n: i + 1 })}</span>
                <span className={styles.handoff_leg_title} title={sid}>{labelOf(sid)}</span>
              </div>
              {i < chain.links.length && (
                <div className={styles.handoff_link}>
                  <span className={styles.handoff_arrow} aria-hidden>↓</span>
                  <div className={styles.handoff_link_body}>
                    {chain.links[i].nextTask && (
                      <span className={styles.handoff_next}>
                        {chain.links[i].planId} · {chain.links[i].nextTask}
                      </span>
                    )}
                    <span className={styles.handoff_note}>{chain.links[i].note}</span>
                    <span className={styles.handoff_time}>
                      {new Date(chain.links[i].handedAt).toLocaleString()}
                    </span>
                  </div>
                </div>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>,
    document.body,
  );
}
