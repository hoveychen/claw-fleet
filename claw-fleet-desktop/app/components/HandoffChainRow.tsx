import { useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { useSessionsStore } from "../store";
import type { HandoffChain, SessionInfo } from "../types";
import styles from "./HandoffChainRow.module.css";

/**
 * Chip ("接力 n/N") + click-to-expand panel listing the full relay chain:
 * every leg's session, the relay note it left, and when the baton passed.
 *
 * Callers must guard on `session.handoff` being present.
 */
export function HandoffChainRow({ session }: { session: SessionInfo }) {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const [open, setOpen] = useState(false);
  const [chain, setChain] = useState<HandoffChain | null>(null);
  const [loading, setLoading] = useState(false);
  const info = session.handoff!;

  const toggle = async (e: React.SyntheticEvent) => {
    e.stopPropagation();
    const next = !open;
    setOpen(next);
    if (next && !chain && !loading) {
      setLoading(true);
      try {
        const c = await invoke<HandoffChain | null>("get_handoff_chain", {
          sessionId: session.id,
        });
        setChain(c ?? null);
      } catch {
        // panel simply shows the loading label going away; chip stays usable
      }
      setLoading(false);
    }
  };

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

  return (
    <div className={styles.handoff_row}>
      <span
        className={styles.handoff_chip}
        role="button"
        tabIndex={0}
        title={t("card.tip_handoff", { hop: info.hop, len: info.chainLen })}
        onClick={toggle}
        onKeyDown={(e) => e.key === "Enter" && toggle(e)}
      >
        🔗 {t("card.handoff_chip", { hop: info.hop, len: info.chainLen })}
      </span>
      {open && (
        <div className={styles.handoff_panel} onClick={(e) => e.stopPropagation()}>
          {loading && <div className={styles.handoff_note}>{t("card.handoff_loading")}</div>}
          {!loading && chain && legIds.map((sid, i) => (
            <div key={sid}>
              <div className={`${styles.handoff_leg} ${sid === session.id ? styles.handoff_current : ""}`}>
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
                    <span className={styles.handoff_note} title={chain.links[i].note}>
                      {chain.links[i].note}
                    </span>
                    <span className={styles.handoff_time}>
                      {new Date(chain.links[i].handedAt).toLocaleString()}
                    </span>
                  </div>
                </div>
              )}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
